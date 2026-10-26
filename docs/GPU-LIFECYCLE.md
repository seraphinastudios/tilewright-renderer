# GPU lifecycle and ownership

One `WgpuRenderer` owns one adapter, device, queue, and pipeline. Every offscreen
frame uses the same render path: validate scene/target agreement, reuse bounded
color/depth/readback targets and a geometrically grown vertex buffer, submit
once, wait for bounded readback,
strip WebGPU row padding, and produce exact semantic planes. Resize validates
the new total pixel budget and increments recovery metrics.

The SDL bridge borrows an existing window or creates one proof window, then asks
the existing `WgpuRenderer` to present with its adapter, device, queue, resource
bindings, shader, depth/stencil configuration, and scene encoder. A native
surface cannot outlive that borrow. Timeout and occlusion skip a frame; outdated
surfaces are reconfigured and retried once; lost/device failures are typed for
owner-driven recreation. The bridge never owns the event loop. Window and
embedded consumers must call it on SDL's video thread.

Consumer recovery policy is explicit: `SurfaceUnavailable` skips the current
frame, `SurfaceLost` recreates the native surface path, and `DeviceLost`
recreates the owning `WgpuRenderer` and its caches. The programmable test double
can queue all three failures followed by a successful frame. The Xvfb proof
presents twice through one renderer and window to exercise reuse; offscreen
tests repeatedly create, render, resize, reuse, and destroy renderer instances.

Linux runtime proof uses Vulkan (including Mesa software Vulkan in CI) and
launches the actual SDL window path under Xvfb. Native Windows uses D3D12. CI
cross-checks the non-SDL renderer crates and default portable CLI for the
Windows GNU target; native D3D12 presentation remains a documented manual test
because the GNU cross toolchain is not a Windows runtime.

`Automatic` natively enables only Vulkan and D3D12, requests the high-performance class,
and does not force a fallback adapter. wgpu may still return a CPU adapter when
it is the only conforming implementation, as in headless CI; diagnostics record
that device type, driver, backend, startup time, maximum 2D texture dimension,
and buffer limit. Explicit Vulkan/D3D12 requests never cross-fallback to the
other backend. Adapter and surface capability failures include the requested
policy and missing capability.

The sole unsafe assertion is private to `raw_handle_bridge`: an ephemeral
wrapper is declared `Send + Sync` only to synchronously copy immutable raw SDL
handles into wgpu. It is never stored, sent, or used for an SDL operation, and
the returned surface retains the original window lifetime.

## Browsers

On `wasm32` the renderer is created asynchronously: `WgpuRenderer::with_canvas`
(or `with_offscreen_canvas`) owns a surface on the canvas, and `present`
draws the scene into it, configuring the surface on first use and after each
resize. `new_async` creates an offscreen-only renderer where the backend
allows one (WebGL2 needs a canvas). `Automatic` uses WebGPU when the browser
offers an adapter and WebGL2 otherwise; WebGPU support is settled before the
canvas gets a context, because a canvas keeps its first context type.
`WebGpu` and `WebGl2` never fall back, and are unavailable natively.

Browsers cannot block on the GPU, so `Renderer::render` returns
`BackendUnavailable` there; `render_async` produces the same frame with every
plane. Native `render` is `render_async` driven to completion. WebGPU
resolves the readback mapping from the browser's event loop; on WebGL2 the
renderer polls the device once per browser task until the mapping settles.
Device creation uses wgpu's WebGL2 downlevel limits on a GL adapter; a
browser that offers an adapter but refuses the device reports
`BackendUnavailable`. wgpu's device-lost notification marks the renderer
lost, natively and in browsers: every later frame, and a readback waiting
when it happens, fails with `DeviceLost`, and the owner recreates the
renderer. A suboptimal owned surface presents its frame and is reconfigured
before the next one, as a lost one is. `surface_reconfigurations` counts each
resize, each retry of an outdated surface, and each of these recoveries; the
first configuration of a surface, and the one after a resize, add nothing more.
