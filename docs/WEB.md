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
| `webgpu` | WebGPU | 606 KB | 145 KB |
| `full` | WebGPU and WebGL2 | 4,090 KB | 1,077 KB |

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
  after `auto` chose WebGPU cannot switch that canvas to WebGL2.
- `uploadTexture({ id, revision, width, height, rgba })` stores unpremultiplied
  RGBA8 pixels and returns a handle with the texture's SHA-256. The texture
  goes through the same verified resource contract as native textures.
  `uploadImage` reads an image through a 2D canvas, which can round partly
  transparent texels; upload exact pixels where that matters.
  `removeTexture` forgets a texture.
- `SceneBuilder` (or `renderer.scene()`) packs quads and sprites with every
  scene field: frames, UVs, depth, painter order, tint, greyscale, glow,
  pickability, sampling and layer. `reset` keeps its buffer, so rebuilding a
  scene every frame allocates nothing once the buffer has grown.
- `SpriteSheet.grid`, `SpriteSheet.covering` and `new SpriteSheet({ origin,
  spacing, … })` number frames row by row, as `tilewright_scene::SpriteSheet`
  does.
- `submit(scene)` validates and keeps a scene; `render()` presents it;
  `resize(width, height)` changes the drawing buffer (submit a scene of the
  new size next).
- `readFrame()` renders offscreen and resolves with RGBA plus the exact
  semantic, depth and coverage planes. `renderReference()` returns the CPU
  reference frame for the same scene.
- `metrics()` reports work counts; `destroy()` releases the device.

Errors are `TilewrightError`s with a stable `code` (see `src/errors.ts`),
such as `invalid-scene`, `unknown-texture`, `resource` (including a frame
outside its texture), `limit-exceeded`, `backend-unavailable`,
`surface-lost`, `device-lost` and `busy` (a call made while `readFrame` is in
flight).

## Packed scenes

A scene crosses into WebAssembly as one buffer of 156-byte little-endian
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

Unknown flags, an unknown layer, frame or glow values without their flag, a
buffer that is not whole records, more quads than the scene limit (checked
before decoding) and a texture the store does not hold are rejected. The
decoded quads then pass the full scene validation.

## Limits

- Scenes have the native limits: 131,072 quads, 8,192 pixels per side,
  16,777,216 pixels, 4,096 distinct textures and the raster-sample budget.
- A texture is at most 8,192 pixels per side and 16 MiB as an envelope
  (about 4.2 megapixels, for example 2,047 × 2,048).
- The page's texture store holds at most 256 MiB of envelopes and 16,384
  textures. The renderer keeps its own decoded copy of each texture a scene
  uses, within the native 256 MiB working set, plus the GPU copy.
- WebGPU limits follow the browser's adapter. WebGL2 uses wgpu's WebGL2
  downlevel limits with the adapter's texture size, up to 8,192.
- `frame_cpu_micros` and `startup_micros` are zero in browsers, where Rust has
  no clock; measure time with `performance.now()`.
- Readback never blocks the page. WebGPU resolves it from the browser's event
  loop; on WebGL2 the renderer polls once per browser task, because WebGL2
  reports finished GPU work only between tasks.

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

The browser is never downloaded. `TILEWRIGHT_CHROMIUM` names a Chromium
executable; otherwise `PLAYWRIGHT_BROWSERS_PATH` selects Playwright's
installed browsers; otherwise the installed Google Chrome is used. The tests
pass `--enable-unsafe-webgpu --ignore-gpu-blocklist --enable-gpu`. Headless
Chromium serves WebGPU through SwiftShader, its CPU implementation, whether
or not a GPU is present; without GPU compositing, presenting to a WebGPU
canvas destroys the device in headless mode.

What runs where:

| Where | WebGL2 | WebGPU |
| --- | --- | --- |
| CI (GitHub `ubuntu-24.04`, no GPU, `Renderer validation`) | Chrome, SwiftShader | Chrome, SwiftShader; required |
| A developer machine with a GPU | Chrome on the GPU through ANGLE | Chrome, SwiftShader |

`tools/validate-web.sh` requires the WebGPU test to run
(`TILEWRIGHT_REQUIRE_WEBGPU=1`); set it to `0` to skip WebGPU on a browser
without an adapter.
