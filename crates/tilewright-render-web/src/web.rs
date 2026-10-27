// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! The `wasm-bindgen` surface. Browsers are single-threaded here: the
//! renderer lives in an `Rc<RefCell<…>>` so an asynchronous readback can
//! own it while it waits. A shared state marks the renderer busy from the
//! moment `readFrame` is called until it settles, so every call made in the
//! meantime fails with the `busy` code instead of racing the frame, and
//! marks it destroyed once `destroy` is called. Destroying also signals the
//! renderer lost, so a readback still waiting ends at its next poll.

use crate::{
    errors::{CodedError, parse_digest, safe_integer},
    packed::{SceneHeader, decode_scene},
    textures::WebTextures,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};
use tilewright_render_api::{
    BackendPreference, FrameOutput, RenderRequest, Renderer, TargetDescriptor, TargetKind,
};
use tilewright_render_wgpu::{LossSignal, WgpuRenderer};
use tilewright_scene::{ResourceId, SceneBundle, SceneLimits, Viewport};
use wasm_bindgen::{JsCast, prelude::*};
use web_sys::{HtmlCanvasElement, OffscreenCanvas};

thread_local! {
    /// The page's trap handler; see [`set_trap_handler`].
    static TRAP_HANDLER: std::cell::RefCell<Option<js_sys::Function>> =
        const { std::cell::RefCell::new(None) };
}

/// Calls `handler` with the panic message whenever this module panics,
/// just before the panic traps the module. A trap aborts mid-call: a
/// readback in progress never settles, and the module, which every renderer
/// of the bundle shares, is left in an undefined state. The TypeScript
/// wrapper uses the handler to stop every renderer and reject waiting
/// readbacks.
#[wasm_bindgen(js_name = setTrapHandler)]
pub fn set_trap_handler(handler: js_sys::Function) {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    TRAP_HANDLER.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            *slot = Some(handler);
        }
    });
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let message = JsValue::from_str(&info.to_string());
            TRAP_HANDLER.with(|slot| {
                if let Ok(slot) = slot.try_borrow()
                    && let Some(handler) = slot.as_ref()
                {
                    let _ = handler.call1(&JsValue::NULL, &message);
                }
            });
            previous(info);
        }));
    });
}

/// Test hooks, built only with the `test-hooks` feature: the browser tests
/// make the module panic at a chosen point to check trap handling.
#[cfg(feature = "test-hooks")]
mod test_hooks {
    use wasm_bindgen::prelude::*;

    thread_local! {
        static PANIC_AT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    }

    /// Panics at the next `point` reached: `"submit"`, `"readback"` or
    /// `"create"`.
    #[wasm_bindgen(js_name = panicForTest)]
    pub fn panic_for_test(point: &str) {
        let point = match point {
            "submit" => "submit",
            "readback" => "readback",
            "create" => "create",
            _ => return,
        };
        PANIC_AT.with(|at| at.set(Some(point)));
    }

    pub(super) fn reached(point: &'static str) {
        if PANIC_AT.with(std::cell::Cell::get) == Some(point) {
            PANIC_AT.with(|at| at.set(None));
            panic!("test panic at {point}");
        }
    }
}

/// A point where the browser tests can make the module panic.
#[cfg(feature = "test-hooks")]
fn test_point(point: &'static str) {
    test_hooks::reached(point);
}

#[cfg(not(feature = "test-hooks"))]
const fn test_point(_point: &'static str) {}

fn js_error(error: impl Into<CodedError>) -> JsError {
    JsError::new(&error.into().message())
}

fn render_error(error: &tilewright_render_api::Error) -> JsError {
    js_error(CodedError::from(error))
}

fn busy() -> JsError {
    js_error(CodedError::new(
        "busy",
        "the renderer is reading a frame back; wait for readFrame to settle",
    ))
}

fn destroyed() -> JsError {
    js_error(CodedError::new("destroyed", "the renderer was destroyed"))
}

struct Inner {
    /// `None` once destroyed: the device and its caches are released.
    renderer: Option<WgpuRenderer>,
    scene: Option<SceneBundle>,
}

impl Inner {
    fn renderer(&mut self) -> Result<&mut WgpuRenderer, JsError> {
        self.renderer.as_mut().ok_or_else(destroyed)
    }

    /// Releases the device, its caches and the scene.
    fn release(&mut self) {
        self.renderer = None;
        self.scene = None;
    }
}

#[derive(Default)]
struct State {
    busy: Cell<bool>,
    destroyed: Cell<bool>,
}

/// Marks the renderer busy until dropped.
struct Busy(Rc<State>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.busy.set(false);
    }
}

/// A renderer drawing into one canvas.
#[wasm_bindgen]
pub struct WebRenderer {
    inner: Rc<RefCell<Inner>>,
    state: Rc<State>,
    textures: Arc<WebTextures>,
    /// Ends a readback still waiting when the renderer is destroyed.
    loss: LossSignal,
    backend: String,
    adapter: String,
}

impl WebRenderer {
    /// Fails with `destroyed` or `busy` when the renderer cannot take a call.
    fn ready(&self) -> Result<(), JsError> {
        if self.state.destroyed.get() {
            return Err(destroyed());
        }
        if self.state.busy.get() {
            return Err(busy());
        }
        Ok(())
    }
}

#[wasm_bindgen]
impl WebRenderer {
    /// Creates a renderer on an `HTMLCanvasElement` or `OffscreenCanvas`
    /// whose drawing buffer is `width` × `height` pixels. `backend` is
    /// `"auto"` (WebGPU when the browser provides an adapter, else WebGL2),
    /// `"webgpu"` or `"webgl2"`; explicit choices never fall back.
    ///
    /// # Errors
    /// Rejects with `invalid-argument`, `invalid-target` or
    /// `backend-unavailable`.
    pub async fn create(
        canvas: JsValue,
        width: u32,
        height: u32,
        backend: String,
    ) -> Result<WebRenderer, JsError> {
        let preference = match backend.as_str() {
            "auto" => BackendPreference::Automatic,
            "webgpu" => BackendPreference::WebGpu,
            "webgl2" => BackendPreference::WebGl2,
            _ => {
                return Err(js_error(CodedError::new(
                    "invalid-argument",
                    "backend must be \"auto\", \"webgpu\" or \"webgl2\"",
                )));
            }
        };
        test_point("create");
        let target = TargetDescriptor {
            kind: TargetKind::Embedded,
            width,
            height,
            backend: preference,
        };
        let renderer = if let Some(canvas) = canvas.dyn_ref::<HtmlCanvasElement>() {
            WgpuRenderer::with_canvas(target, canvas.clone()).await
        } else if let Some(canvas) = canvas.dyn_ref::<OffscreenCanvas>() {
            WgpuRenderer::with_offscreen_canvas(target, canvas.clone()).await
        } else {
            return Err(js_error(CodedError::new(
                "invalid-argument",
                "canvas must be an HTMLCanvasElement or OffscreenCanvas",
            )));
        }
        .map_err(|error| match error {
            tilewright_render_api::Error::DeviceLost => js_error(CodedError::new(
                "device-lost",
                "the canvas's WebGL2 context is lost; create the renderer on a new canvas",
            )),
            error => render_error(&error),
        })?;
        let backend = match renderer.adapter().backend.as_str() {
            "BrowserWebGpu" => "webgpu",
            "Gl" => "webgl2",
            other => other,
        }
        .to_owned();
        let adapter = renderer.adapter().name.clone();
        let loss = renderer.loss_signal();
        let textures =
            WebTextures::default().with_maximum_dimension(renderer.maximum_texture_dimension());
        Ok(Self {
            inner: Rc::new(RefCell::new(Inner {
                renderer: Some(renderer),
                scene: None,
            })),
            state: Rc::new(State::default()),
            textures: Arc::new(textures),
            loss,
            backend,
            adapter,
        })
    }

    /// `"webgpu"` or `"webgl2"`.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn backend(&self) -> String {
        self.backend.clone()
    }

    /// The adapter's name as the browser reports it; often empty.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn adapter(&self) -> String {
        self.adapter.clone()
    }

    /// The largest texture side this device accepts, at most 8,192.
    #[wasm_bindgen(getter, js_name = maximumTextureSide)]
    #[must_use]
    pub fn maximum_texture_side(&self) -> u32 {
        self.textures.maximum_dimension()
    }

    /// Stores `width` × `height` RGBA8 pixels (row-major, unpremultiplied)
    /// as texture `id` at `revision` and returns the SHA-256 of its envelope
    /// in hexadecimal. A stored pair never changes: the same pixels are
    /// accepted again, and different pixels need a new revision (or
    /// [`WebRenderer::remove_texture`] first).
    ///
    /// # Errors
    /// Throws `invalid-argument`, `invalid-texture` (including different
    /// pixels under a stored pair) or `limit-exceeded` (including a side
    /// above the device's limit).
    #[wasm_bindgen(js_name = uploadTexture)]
    pub fn upload_texture(
        &self,
        id: f64,
        revision: f64,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<String, JsError> {
        self.ready()?;
        let id = safe_integer(id, "id").map_err(js_error)?;
        let revision = safe_integer(revision, "revision").map_err(js_error)?;
        let digest = self
            .textures
            .insert(ResourceId(id), revision, width, height, rgba)
            .map_err(js_error)?;
        Ok(digest
            .0
            .iter()
            .fold(String::with_capacity(64), |mut hex, byte| {
                hex.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
                hex.push(char::from(b"0123456789abcdef"[usize::from(byte & 15)]));
                hex
            }))
    }

    /// Forgets an uploaded texture named by its full identity, the digest
    /// [`WebRenderer::upload_texture`] returned included; returns whether it
    /// was stored. A stale identity removes nothing. Scenes submitted later
    /// cannot name it.
    ///
    /// # Errors
    /// Throws `invalid-argument` for an identity that is not a safe integer
    /// or a digest that is not 64 lowercase hexadecimal digits.
    #[wasm_bindgen(js_name = removeTexture)]
    pub fn remove_texture(&self, id: f64, revision: f64, digest: &str) -> Result<bool, JsError> {
        self.ready()?;
        let id = safe_integer(id, "id").map_err(js_error)?;
        let revision = safe_integer(revision, "revision").map_err(js_error)?;
        let digest = parse_digest(digest).map_err(js_error)?;
        Ok(self.textures.remove(ResourceId(id), revision, digest))
    }

    /// Validates and stores the scene that [`WebRenderer::render`] and
    /// [`WebRenderer::read_frame`] draw. `clear` is four colour channels;
    /// `quads` holds packed quad records.
    ///
    /// # Errors
    /// Throws `invalid-argument`, `invalid-scene`, `unknown-texture`,
    /// `limit-exceeded` or `busy`; the previous scene stays current.
    #[allow(clippy::too_many_arguments)]
    pub fn submit(
        &self,
        revision: f64,
        clock_millis: f64,
        width: u32,
        height: u32,
        scale_milli: u32,
        clear: &[f32],
        quads: &[u8],
    ) -> Result<(), JsError> {
        self.ready()?;
        test_point("submit");
        let [red, green, blue, alpha] = clear else {
            return Err(js_error(CodedError::new(
                "invalid-argument",
                "clear must hold four channels",
            )));
        };
        let header = SceneHeader {
            revision: safe_integer(revision, "revision").map_err(js_error)?,
            clock_millis: safe_integer(clock_millis, "clockMillis").map_err(js_error)?,
            viewport: Viewport {
                width,
                height,
                scale_milli,
            },
            clear_color: [*red, *green, *blue, *alpha],
        };
        let scene = decode_scene(header, quads, &self.textures, SceneLimits::default())
            .map_err(|error| js_error(&error))?;
        let mut inner = self.inner.try_borrow_mut().map_err(|_| busy())?;
        // A scene of another size could never be drawn; the previous scene
        // stays current.
        let target = inner.renderer()?.target();
        if (width, height) != (target.width, target.height) {
            return Err(js_error(CodedError::new(
                "invalid-target",
                format!(
                    "the scene is {width} × {height} but the drawing buffer is {} × {}",
                    target.width, target.height
                ),
            )));
        }
        inner.scene = Some(scene);
        Ok(())
    }

    /// Draws the current scene into the canvas.
    ///
    /// # Errors
    /// Throws `invalid-target` without a scene of the canvas's size,
    /// `surface-unavailable` when the browser skipped the frame, and
    /// otherwise a resource or backend code, or `busy`.
    pub fn render(&self) -> Result<(), JsError> {
        self.ready()?;
        let mut inner = self.inner.try_borrow_mut().map_err(|_| busy())?;
        let Inner { renderer, scene } = &mut *inner;
        let renderer = renderer.as_mut().ok_or_else(destroyed)?;
        let scene = scene.as_ref().ok_or_else(no_scene)?;
        renderer
            .present(&RenderRequest {
                scene,
                resources: self.textures.clone(),
            })
            .map(|_| ())
            .map_err(|error| render_error(&error))
    }

    /// Resizes the drawing buffer. Submit a scene of the new size before
    /// the next render.
    ///
    /// # Errors
    /// Throws `invalid-target` for a size outside the renderer's bounds, or
    /// `busy`.
    pub fn resize(&self, width: u32, height: u32) -> Result<(), JsError> {
        self.ready()?;
        self.inner
            .try_borrow_mut()
            .map_err(|_| busy())?
            .renderer()?
            .resize(width, height)
            .map_err(|error| render_error(&error))
    }

    /// Renders the current scene offscreen and resolves with the frame's
    /// colour, semantic, depth and coverage planes. The renderer is busy
    /// from this call until the promise settles. If it is destroyed
    /// meanwhile, the promise rejects with `destroyed` and the device is
    /// released.
    #[wasm_bindgen(js_name = readFrame)]
    pub fn read_frame(&self) -> js_sys::Promise {
        if let Err(error) = self.ready() {
            return js_sys::Promise::reject(&error.into());
        }
        self.state.busy.set(true);
        let guard = Busy(Rc::clone(&self.state));
        let inner = Rc::clone(&self.inner);
        let textures = self.textures.clone();
        // The borrow is held across the readback, while the busy state
        // turns every other call away.
        #[allow(clippy::await_holding_refcell_ref)]
        wasm_bindgen_futures::future_to_promise(async move {
            let mut inner = inner.try_borrow_mut().map_err(|_| busy())?;
            // Destroyed before the readback started: nothing else matters.
            if guard.0.destroyed.get() {
                inner.release();
                return Err(destroyed().into());
            }
            let Inner { renderer, scene } = &mut *inner;
            let scene = scene.as_ref().ok_or_else(no_scene)?;
            test_point("readback");
            let frame = renderer
                .as_mut()
                .ok_or_else(destroyed)?
                .render_async(RenderRequest {
                    scene,
                    resources: textures,
                })
                .await;
            if guard.0.destroyed.get() {
                inner.release();
                return Err(destroyed().into());
            }
            drop(guard);
            Ok(WebFrame::from(frame.map_err(|error| render_error(&error))?).into())
        })
    }

    /// Releases the device and every cached texture; every later call fails
    /// with `destroyed`. A readback in flight stops waiting at its next poll,
    /// rejects with `destroyed` and releases the device.
    pub fn destroy(&self) {
        self.state.destroyed.set(true);
        self.loss.mark_lost();
        self.textures.clear();
        if !self.state.busy.get()
            && let Ok(mut inner) = self.inner.try_borrow_mut()
        {
            inner.release();
        }
    }

    /// Renders the current scene with the CPU reference renderer: the
    /// deterministic frame every GPU backend matches within one 8-bit step,
    /// with exact picking planes. For conformance checks and tests.
    ///
    /// # Errors
    /// Throws `invalid-target` without a scene, a resource code, or `busy`.
    #[wasm_bindgen(js_name = renderReference)]
    pub fn render_reference(&self) -> Result<WebFrame, JsError> {
        self.ready()?;
        let inner = self.inner.try_borrow().map_err(|_| busy())?;
        let scene = inner.scene.as_ref().ok_or_else(no_scene)?;
        let viewport = scene.viewport();
        let frame = tilewright_render_testkit::ReferenceRenderer::new(
            tilewright_render_testkit::offscreen_target(viewport.width, viewport.height),
        )
        .and_then(|mut reference| {
            reference.render(RenderRequest {
                scene,
                resources: self.textures.clone(),
            })
        })
        .map_err(|error| render_error(&error))?;
        Ok(WebFrame::from(frame))
    }

    /// Work counts from the most recent frame.
    ///
    /// # Errors
    /// Throws `busy` during a readback.
    pub fn metrics(&self) -> Result<WebMetrics, JsError> {
        self.ready()?;
        let inner = self.inner.try_borrow().map_err(|_| busy())?;
        let metrics = inner.renderer.as_ref().ok_or_else(destroyed)?.metrics();
        #[allow(clippy::cast_precision_loss)]
        let number = |value: u64| value as f64;
        Ok(WebMetrics {
            frames_submitted: number(metrics.frames_submitted),
            resource_requests: number(metrics.resource_requests),
            uploaded_bytes: number(metrics.uploaded_bytes),
            vertex_count: number(metrics.vertex_count),
            vertex_allocation_bytes: number(metrics.vertex_allocation_bytes),
            skipped_frames: number(metrics.skipped_frames),
            surface_reconfigurations: number(metrics.surface_reconfigurations),
            device_recoveries: number(metrics.device_recoveries),
            target_allocation_bytes: number(metrics.target_allocation_bytes),
            readback_bytes: number(metrics.readback_bytes),
            frame_cpu_micros: number(metrics.frame_cpu_micros),
        })
    }
}

fn no_scene() -> JsError {
    js_error(CodedError::new(
        "invalid-target",
        "no scene has been submitted",
    ))
}

/// One frame read back: RGBA8 colour and the exact picking planes.
#[wasm_bindgen]
pub struct WebFrame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    semantic_ids: Vec<u32>,
    depth: Vec<i16>,
    coverage: Vec<u8>,
}

impl From<FrameOutput> for WebFrame {
    fn from(frame: FrameOutput) -> Self {
        Self {
            width: frame.width,
            height: frame.height,
            rgba: frame.rgba8,
            semantic_ids: frame.semantic_ids,
            depth: frame.depth,
            coverage: frame.coverage,
        }
    }
}

#[wasm_bindgen]
impl WebFrame {
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Row-major RGBA8 pixels.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn rgba(&self) -> Vec<u8> {
        self.rgba.clone()
    }

    /// The semantic ID of the topmost pickable quad per pixel; 0 for none.
    #[wasm_bindgen(getter, js_name = semanticIds)]
    #[must_use]
    pub fn semantic_ids(&self) -> Vec<u32> {
        self.semantic_ids.clone()
    }

    /// The depth of that quad; −32,768 for none.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn depth(&self) -> Vec<i16> {
        self.depth.clone()
    }

    /// That quad's alpha coverage.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn coverage(&self) -> Vec<u8> {
        self.coverage.clone()
    }
}

/// Work counts from the most recent frame.
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct WebMetrics {
    #[wasm_bindgen(js_name = framesSubmitted)]
    pub frames_submitted: f64,
    #[wasm_bindgen(js_name = resourceRequests)]
    pub resource_requests: f64,
    #[wasm_bindgen(js_name = uploadedBytes)]
    pub uploaded_bytes: f64,
    #[wasm_bindgen(js_name = vertexCount)]
    pub vertex_count: f64,
    #[wasm_bindgen(js_name = vertexAllocationBytes)]
    pub vertex_allocation_bytes: f64,
    #[wasm_bindgen(js_name = skippedFrames)]
    pub skipped_frames: f64,
    #[wasm_bindgen(js_name = surfaceReconfigurations)]
    pub surface_reconfigurations: f64,
    #[wasm_bindgen(js_name = deviceRecoveries)]
    pub device_recoveries: f64,
    #[wasm_bindgen(js_name = targetAllocationBytes)]
    pub target_allocation_bytes: f64,
    #[wasm_bindgen(js_name = readbackBytes)]
    pub readback_bytes: f64,
    /// Zero in browsers, where Rust has no clock.
    #[wasm_bindgen(js_name = frameCpuMicros)]
    pub frame_cpu_micros: f64,
}
