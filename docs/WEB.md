# Browser package

The renderer runs in browsers as the same Rust core compiled to
`wasm32-unknown-unknown`. It draws with WebGPU, or with WebGL2 where WebGPU is
unavailable, behind a TypeScript API:

- `crates/tilewright-render-web` holds the `wasm-bindgen` bindings, a texture
  store and the packed-scene decoder. Its version follows the workspace; it is
  not published to crates.io.
- `packages/tilewright-render` is the TypeScript package
  `@seraphinastudios/tilewright-render`. It is private for now: build it from
  this repository and depend on the built directory.

Pages build scenes, never packs or content files. Consumers turn their state
into quads, as native consumers do, and the renderer validates every scene
exactly as the native renderers do.

## Bundles

The package ships two WebAssembly bundles with the same API:

| Bundle | Backends | `.wasm` | gzip −9 |
| --- | --- | --- | --- |
| `webgpu` | WebGPU | 649 KB | 157 KB |
| `full` | WebGPU and WebGL2 | 4,133 KB | 1,089 KB |

WebGL2 needs wgpu's portable core, its shader translator and a GL backend in
the bundle; WebGPU uses the browser's own implementation. `create` loads only
the bundle it needs, so WebGPU browsers download the small one. Both include
the CPU reference renderer (about 7 KB gzipped) behind `renderReference`. The
sizes are for the `web` Cargo profile (size-optimised, link-time optimisation,
one codegen unit) without a separate `wasm-opt` pass; `tools/build-web.sh`
prints them for every build.

## Build

You need:

- the `wasm32-unknown-unknown` Rust target (`rust-toolchain.toml` lists it);
- the `wasm-bindgen` command-line tool at exactly the locked `wasm-bindgen`
  version (0.2.127; `policy/toolchain.json` pins the Linux release archive by
  SHA-256);
- Node 22 or newer with npm.

```sh
tools/build-web.sh                   # both bundles, glue and TypeScript
(cd packages/tilewright-render && npm test)
tools/validate-web.sh                # Clippy for wasm32, build, browser tests
```

`TILEWRIGHT_WEB_TEST_HOOKS=1 tools/build-web.sh` builds the `full` bundle with
the trap tests' hooks, as `tools/validate-web.sh` does; never ship that build.
Without the hooks `npm test` skips the trap tests.

The build writes `packages/tilewright-render/wasm/{webgpu,full}/` and
`packages/tilewright-render/dist/`; neither is committed. Bundlers that
understand `new URL('…', import.meta.url)` find each `.wasm` file beside its
JavaScript; otherwise pass `wasm` URLs to `create`.

## Use

```ts
import { SpriteSheet, TilewrightRenderer } from '@seraphinastudios/tilewright-render';

const canvas = document.querySelector('canvas')!;
const renderer = await TilewrightRenderer.create(canvas); // WebGPU, else WebGL2

// A layered character sheet is one texture; quads pick its frames.
const body = renderer.uploadImage(1, await createImageBitmap(bodySheet));
const sheet = SpriteSheet.covering(body, 64, 64); // 13 × 21 frames for an 832 × 1344 sheet

const scene = renderer.scene({ clearColor: [0, 0, 0, 1] });
let revision = 1;
function frame(time: number) {
  scene.reset({ revision: revision++, clockMillis: Math.floor(time) });
  scene.sprite({ id: 1, texture: body, frame: sheet.frame(walkFrame(time)), x: 100, y: 80, width: 64, height: 64 });
  // …one quad per layer and object…
  renderer.submit(scene);
  renderer.render(); // false when the browser skipped presenting this frame
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
```

- `create(canvas, { backend, bundle, wasm, width, height })`. `backend` is
  `auto` (default), `webgpu` or `webgl2`. An explicit backend never falls
  back. A canvas keeps the first context type it gets, so a WebGPU failure
  after `auto` chose WebGPU cannot switch that canvas to WebGL2. With the
  `webgpu` bundle, `auto` on a browser without WebGPU fails with
  `backend-unavailable`.
- `uploadTexture({ id, revision, width, height, rgba })` stores unpremultiplied
  RGBA8 pixels and returns a handle with the texture's SHA-256. The texture
  goes through the same verified resource contract as native textures. A
  stored identity and revision never change: uploading the same pixels again
  is accepted, and different pixels must use a new revision, or
  `removeTexture` first. Sizes and pixel lengths are checked before any
  pixels are copied into WebAssembly, against `maximumTextureSide`, the
  device's limit (at most 8,192; WebGL2 devices may allow 4,096 or less),
  with `limit-exceeded` above it.
  `uploadImage` takes `ImageData` exactly, and reads other image sources at
  their intrinsic size through a 2D canvas, which can round partly
  transparent texels. Handles are frozen. `removeTexture` forgets a texture
  named by its whole handle; a stale digest removes nothing. Submit a scene
  without a texture before removing it: the current scene may still draw it
  from the renderer's cache, or fail with `resource`, and `renderReference`
  fails.
- Quads name a texture by its full identity, the handle's ID, revision and
  digest, so a stale handle is refused (`unknown-texture`) rather than
  drawing other pixels. Reuse handles: digests are parsed once per handle.
- `SceneBuilder` (or `renderer.scene()`) packs quads and sprites with every
  scene field: frames, UVs, depth, painter order, tint, greyscale, glow,
  pickability, sampling and layer. `reset` keeps its buffer, so rebuilding a
  scene every frame allocates nothing once the buffer has grown.
- `SpriteSheet.grid`, `SpriteSheet.covering` and `new SpriteSheet({ origin,
  spacing, … })` number frames row by row, as `tilewright_scene::SpriteSheet`
  does.
- `submit(scene)` validates and keeps a scene; a scene that fails, including
  one of another size than the drawing buffer, leaves the previous scene
  current. `render()` presents it; `resize(width, height)` changes the
  drawing buffer (submit a scene of the new size next). The renderer owns
  the canvas's drawing-buffer size: one set on the canvas directly is reset
  at the next `render()`. The canvas is opaque on both backends: a
  transparent clear colour or quad never shows the page through it. A
  builder's options are validated and frozen;
  change them with `reset`. Colours, points and UVs must have exactly their
  number of entries, so a reused buffer never keeps an earlier quad's
  values.
- `readFrame()` renders offscreen and resolves with RGBA plus the exact
  semantic, depth and coverage planes. From the call until it settles the
  renderer is busy: every other call fails with `busy`. `renderReference()`
  returns the CPU reference frame for the same scene.
- `metrics()` reports work counts. `destroy()` destroys the device, gives up
  a WebGL2 context at once rather than at garbage collection, so destroyed
  renderers never count against the browser's limit of live contexts, and
  drops the textures. A readback in flight then stops waiting and rejects
  with `destroyed`, and so does every later call. Create a later renderer on
  a new canvas.
- A lost WebGL2 context, for example when the browser drops the oldest of
  too many contexts, fails every later frame and readback (`render`,
  `readFrame`), and any readback waiting, with `device-lost`. Other calls
  keep working, but the renderer never draws again: create one on a new
  canvas. An uncaptured GPU error fails every later frame with `internal`.
- A WebAssembly trap, which would be a renderer bug, aborts mid-call and
  leaves the module every renderer shares in an undefined state, so every
  renderer of the page stops: its calls, `create` and any readback waiting
  fail with the same `internal` error. Reload the page. A trap that is not a
  Rust panic, such as WebAssembly running out of memory, during `readFrame`
  is not reported: that promise never settles.

Errors are `TilewrightError`s with a stable `code` (see `src/errors.ts`),
such as `invalid-scene`, `unknown-texture`, `resource` (including a frame
outside its texture), `limit-exceeded`, `backend-unavailable`,
`surface-lost`, `device-lost` (recreate the renderer), `busy` and
`destroyed`. Arguments are checked before they reach WebAssembly, so a
fractional or out-of-range size never wraps.

## Packed scenes

A scene crosses into WebAssembly as one buffer of 188-byte little-endian
records, one per quad, so a frame of thousands of quads costs one copy. The
scene-wide values (revision, clock, viewport, scale and clear colour) travel
beside it. `SceneBuilder` writes the records; Rust code can use
`tilewright_render_web::packed::encode_quad`.

| Word | Field |
| --- | --- |
| 0 | semantic ID (`u32`) |
| 1 | part (`u16`, low half) and depth (`i16`, high half) |
| 2 | painter order (`u32`) |
| 3 | flags: bit 0 pickable, bit 1 linear, bit 2 glow, bit 3 frame; bits 8–9 layer (0 world, 1 effect, 2 interface) |
| 4–5 | resource ID, low and high 32 bits |
| 6–7 | resource revision, low and high 32 bits |
| 8–11 | frame x, y, width, height (`u32`; zero without the frame flag) |
| 12–19 | corner positions (`f32`, four x/y pairs) |
| 20–27 | corner texture coordinates (`f32`, four u/v pairs) |
| 28–31 | colour (`f32` RGBA) |
| 32 | greyscale (`f32`) |
| 33–36 | glow colour (`f32` RGBA; zero without the glow flag) |
| 37–38 | glow radius (`f32` u/v; zero without the glow flag) |
| bytes 156–187 | the texture's SHA-256 digest |

Unknown flags, an unknown layer, frame or glow values without their flag, a
buffer that is not whole records, more quads than the scene limit (checked
before decoding) and a texture the store does not hold under that exact
digest are rejected. `SceneBuilder` stops at the same quad limit before it
allocates more. The
decoded quads then pass the full scene validation.

## Limits

- Scenes have the native limits: 131,072 quads, 8,192 pixels per side,
  16,777,216 pixels, 4,096 distinct textures and the raster-sample budget.
- A texture is at most 8,192 pixels per side and 16 MiB as an envelope
  (about 4.2 megapixels, for example 2,047 × 2,048).
- Each renderer has its own texture store, holding at most 256 MiB of
  envelopes and 16,384 textures; handles do not cross renderers. The
  renderer keeps its own decoded copy of each texture a scene uses, within
  the native 256 MiB working set, plus the GPU copy.
- Devices use wgpu's downlevel limits, WebGL2's for WebGL2, with the
  adapter's 2D texture size up to 8,192, not every limit the adapter
  offers.
- `frame_cpu_micros` and `startup_micros` are zero in browsers, where Rust has
  no clock; measure time with `performance.now()`.
- Readback never blocks the page: the renderer checks once per browser task
  until the mapping settles. WebGPU settles it from the browser's event
  loop; WebGL2 reports finished GPU work only between tasks.

## Browser tests

`packages/tilewright-render/test/smoke.test.mjs` serves the package on
`http://localhost` (WebGPU requires a secure context) and drives headless
Chromium with `playwright-core`. For each of WebGL2 and WebGPU it:

- builds the corpus atlas scene (`test/fixture.js`, a port of testkit's
  `atlas_scene`) with the TypeScript API;
- checks that the CPU reference renders it to the corpus manifest's exact
  digests, so the packed records reproduce the Rust scene exactly;
- checks that the GPU frame's semantic, depth and coverage planes equal the
  reference and that its RGBA is within one 8-bit step;
- checks the presented canvas pixels, typed errors, and a resize.

A third test loses WebGL2 contexts through `WEBGL_lose_context`: while idle,
before the first frame and during a readback, each expecting `device-lost`;
and with the loss hidden from the renderer, where only `destroy` ends the
waiting readback, with `destroyed`. It then creates and destroys 20
renderers and checks that a live one keeps drawing. Two trap tests make the
module panic in `submit` and inside a readback and expect `internal` from
every renderer; they need the test hooks. Each test fails after two minutes
rather than hanging.

The browser is never downloaded. `TILEWRIGHT_CHROMIUM` names a Chromium
executable; otherwise `PLAYWRIGHT_BROWSERS_PATH` selects Playwright's
installed browsers; otherwise the installed Google Chrome is used.

The tests run WebGPU and WebGL2 on SwiftShader, Chromium's CPU
implementation, on every machine:

```text
--enable-unsafe-webgpu --enable-features=Vulkan --use-vulkan=swiftshader
--use-angle=swiftshader --use-webgpu-adapter=swiftshader
```

So a developer machine renders exactly as the GPU-less CI runner does.
SwiftShader has to back Vulkan and ANGLE as well as WebGPU: with any other
compositor, presenting to a WebGPU canvas destroys the device in headless
Chromium.

| Where | WebGL2 | WebGPU |
| --- | --- | --- |
| CI (GitHub `ubuntu-24.04`, no GPU, inside `Renderer validation`) | Chrome, SwiftShader | Chrome, SwiftShader |
| Any developer machine | Chrome, SwiftShader | Chrome, SwiftShader |

Both backends are required, by `npm test` as by `tools/validate-web.sh`.
`TILEWRIGHT_REQUIRE_WEBGPU=0` skips WebGPU, rather than failing, on a browser
that offers no WebGPU adapter. The
tests check correctness, not speed, so they do not need a GPU.
