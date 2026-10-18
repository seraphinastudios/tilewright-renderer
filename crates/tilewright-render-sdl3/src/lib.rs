// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! Thin SDL3 presentation adapter. It owns no scene, resources, event loop,
//! input policy, or GPU device and must be called on SDL's video thread.

use sdl3::video::Window;
use std::fmt;
use tilewright_render_api::{RecoveryMetrics, RenderRequest, Renderer, TargetKind};
use tilewright_render_wgpu::WgpuRenderer;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationProof {
    pub width: u32,
    pub height: u32,
    pub adapter_name: String,
    pub backend: String,
    pub format: String,
    pub reconfigured: bool,
    pub metrics: RecoveryMetrics,
}

/// Creates a resizable SDL proof window and presents the supplied scene twice
/// through the caller's existing renderer device. The second presentation
/// exercises target, pipeline, texture, and vertex-cache reuse.
///
/// # Errors
/// Returns an SDL, target, resource, surface, or renderer error.
pub fn present_window(
    renderer: &mut WgpuRenderer,
    request: RenderRequest<'_>,
) -> Result<PresentationProof, Error> {
    if renderer.target().kind != TargetKind::Window {
        return Err(Error::InvalidTarget);
    }
    let target = renderer.target();
    let sdl = sdl3::init().map_err(|error| Error::Sdl(error.to_string()))?;
    let video = sdl.video().map_err(|error| Error::Sdl(error.to_string()))?;
    let window = video
        .window("Tilewright renderer proof", target.width, target.height)
        .position_centered()
        .resizable()
        .build()
        .map_err(|error| Error::Sdl(error.to_string()))?;
    present_existing(renderer, &window, request.clone())?;
    present_existing(renderer, &window, request)
}

/// Presents the supplied scene into a caller-owned SDL window. The native
/// surface borrow cannot outlive the window and the existing renderer retains
/// sole device/queue ownership.
///
/// # Errors
/// Returns a target error when pixel dimensions differ, or a typed renderer
/// error when resource resolution, surface acquisition, or presentation fails.
pub fn present_existing(
    renderer: &mut WgpuRenderer,
    window: &Window,
    request: RenderRequest<'_>,
) -> Result<PresentationProof, Error> {
    if !matches!(
        renderer.target().kind,
        TargetKind::Window | TargetKind::Embedded
    ) {
        return Err(Error::InvalidTarget);
    }
    let (width, height) = window.size_in_pixels();
    if width != renderer.target().width || height != renderer.target().height {
        return Err(Error::InvalidTarget);
    }
    let presentation = renderer
        .present_surface(raw_handle_bridge::BorrowedHandle(window), &request)
        .map_err(|error| Error::Renderer(error.to_string()))?;
    let adapter = renderer.adapter();
    Ok(PresentationProof {
        width: presentation.width,
        height: presentation.height,
        adapter_name: adapter.name.clone(),
        backend: adapter.backend.clone(),
        format: presentation.format,
        reconfigured: presentation.reconfigured,
        metrics: renderer.metrics(),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidTarget,
    Sdl(String),
    Renderer(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SDL renderer adapter error: {self:?}")
    }
}

impl std::error::Error for Error {}

mod raw_handle_bridge {
    use sdl3::video::Window;
    use wgpu::rwh::{HasDisplayHandle, HasWindowHandle};

    /// SDL restricts Window operations to its video thread. wgpu's surface API
    /// requires a Send + Sync handle source even though it copies the handles
    /// during this synchronous call. This wrapper never escapes the call, and
    /// the returned surface retains the original Window borrow.
    pub(super) struct BorrowedHandle<'a>(pub(super) &'a Window);

    // SAFETY: the wrapper is constructed and consumed on the SDL video thread,
    // never shared or stored, and only delegates immutable raw-handle access.
    unsafe impl Send for BorrowedHandle<'_> {}
    // SAFETY: identical constraint; no SDL operation crosses a thread boundary.
    unsafe impl Sync for BorrowedHandle<'_> {}

    impl HasWindowHandle for BorrowedHandle<'_> {
        fn window_handle(&self) -> Result<wgpu::rwh::WindowHandle<'_>, wgpu::rwh::HandleError> {
            self.0.window_handle()
        }
    }

    impl HasDisplayHandle for BorrowedHandle<'_> {
        fn display_handle(&self) -> Result<wgpu::rwh::DisplayHandle<'_>, wgpu::rwh::HandleError> {
            self.0.display_handle()
        }
    }
}
