# Frame-time harness

Two harnesses measure how long the renderer takes to draw a large,
representative 2D scene. One runs natively through wgpu; the other runs in
Chromium through the browser package. They are measuring tools, not CI
gates: CI only runs each one for a few frames to keep it working.

## The scene

`tilewright_render_testkit::crowd_scene` (ported to JavaScript in
`packages/tilewright-render/bench/crowd.js`) draws:

- a top-down ground of 32 × 32 tiles from one tile atlas, filling the view;
- a crowd of walking characters. Each character is three quads, body, tunic
  and hair, drawn from character sheets laid out like common open sprite
  sheets: 13 × 21 frames of 64 × 64 texels, walk cycles in rows 8–11.

Every frame moves each character one pixel along its direction and
advances its walk cycle every four frames, so positions, frames and draw
order change every frame. At 1,920 × 1,080 with 1,000 characters the scene
holds 5,040 quads.

`--sheets` picks how the character layers are stored:

- `atlas` packs the three layer sheets into one 832 × 4,032 atlas. A
  character's layers then share a texture, and the whole crowd draws in a
  handful of batches.
- `layers` keeps one 832 × 1,344 sheet per layer, as layered sheets
  usually ship. Characters draw whole, layer by layer, so each character
  switches texture three times: about 3,000 draws for 1,000 characters.

All texels are generated from constants.

## What is measured

In the default `serial` mode, each frame:

1. builds the scene: the Rust `SceneBundle`, or the TypeScript
   `SceneBuilder` records;
2. submits and draws it: native `WgpuRenderer::draw` offscreen, or the
   browser's `submit` and `render` to the canvas;
3. waits until the GPU has finished (`finish`).

The frame time is the whole sequence, run without overlap. A game loop
overlaps one frame's CPU work with the previous frame's GPU work, so this is
an upper bound on its frame time. The harnesses report the 50th, 95th and
99th percentiles and the maximum, for the frame and for its CPU part. They
also report the share of frames within 16.7 ms (60 FPS) and 33.3 ms
(30 FPS). Warm-up frames, which upload textures and grow buffers, are
excluded.

The browser harness's `raf` mode instead drives frames from
`requestAnimationFrame` without waiting for the GPU, as a game does, and
reports the interval between callbacks: what a player sees. Intervals
jitter around whole 60 Hz refreshes, so a frame is on time unless it
arrives at least half a refresh late, that is, unless it missed a refresh:
under 1.5 refreshes (25 ms) at 60 FPS and under 2.5 (41.7 ms) at 30 FPS.
These thresholds assume a 60 Hz `requestAnimationFrame` clock, as headless
Chromium has; the harness warns when the median warm-up interval is not
about 16.7 ms, as on a 120 or 144 Hz display with `--headed`. `raf` mode needs
at least one warm-up frame, so that the first measured frame has an
interval.

## Run it

Native, on any Vulkan or Direct3D 12 device. Pick a Vulkan driver with
`VK_DRIVER_FILES`, for example Mesa's lavapipe:

```sh
cargo run --release -p tilewright-render-wgpu --example frame_time -- \
  --width 1920 --height 1080 --characters 1000 --frames 300 --sheets atlas
VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json \
  cargo run --release -p tilewright-render-wgpu --example frame_time -- --json
```

In browsers, after `tools/build-web.sh`:

```sh
cd packages/tilewright-render
npm run bench -- --backend all --characters 1000 --frames 300
npm run bench -- --gpu --backend webgl2       # WebGL2 on the machine's GPU
npm run bench -- --gpu --backend webgl2 --mode raf
```

By default the browser runs both backends on SwiftShader, its CPU
implementation, as the tests do. `--gpu` lets headless Chrome draw WebGL2 on
the GPU. Its WebGPU stays on SwiftShader, because headless Chrome on Linux
offers no hardware WebGPU adapter. For hardware WebGPU, point
`TILEWRIGHT_CHROMIUM` at Playwright's `chrome-headless-shell` and enable
Vulkan:

```sh
TILEWRIGHT_CHROMIUM=…/chrome-headless-shell npm run bench -- --gpu --backend webgpu \
  --chrome-flag=--enable-features=Vulkan --chrome-flag=--use-angle=vulkan
```

`--headed` runs a visible browser; it still draws on SwiftShader unless
`--gpu` is given too. `--json` prints the samples' summaries as JSON.

## Results

All on one host: a Ryzen 9 9950X3D with its integrated GPU and an RTX 3090,
Linux 7.0, Mesa 26.0.8 (LLVM 21.1.8), NVIDIA 595.91.07, release builds with
Rust 1.97.1, and wgpu 30.0.1. Scene at 1,920 × 1,080 with 1,000 characters,
300 frames after 30 warm-up frames.

### Native (Vulkan)

Measured on 2026-10-28, with characters drawn whole, while the host also
ran other builds (load average about 5–7). That mainly slows lavapipe,
which renders on the CPU.

| Device | Sheets | p50 | p95 | p99 | CPU p50 | ≤ 16.7 ms |
| --- | --- | --- | --- | --- | --- | --- |
| RTX 3090 | atlas | 1.7 ms | 2.1 ms | 2.7 ms | 1.5 ms | 100% |
| RTX 3090 | layers | 1.9 ms | 1.9 ms | 2.4 ms | 1.7 ms | 100% |
| RADV, Ryzen iGPU | atlas | 5.5 ms | 6.2 ms | 6.9 ms | 1.6 ms | 100% |
| RADV, Ryzen iGPU | layers | 6.0 ms | 8.0 ms | 8.2 ms | 2.0 ms | 100% |
| lavapipe (CPU) | atlas | 13.8 ms | 15.7 ms | 16.8 ms | 1.6 ms | 98.7% |
| lavapipe (CPU) | layers | 15.3 ms | 19.4 ms | 21.2 ms | 2.2 ms | 75.7% |

On 2026-10-20, on an idle host, lavapipe's atlas p95 was 14.8 ms, and at
1,280 × 720 the integrated GPU's p95 was 5.2 ms and lavapipe's 11.8 ms
(atlas). The CPU part is scene building (about 0.5 ms) plus drawing and
submission (1 to 1.5 ms), the same on every device; per-layer sheets add
about 0.5 ms for their extra draws.

### Chrome 154 (WebAssembly)

Measured on 2026-10-20 with atlas sheets, whose single texture batches the
same whatever the order the characters draw in.

| Backend and device | Mode | p50 | p95 | p99 | CPU p50 | 60 FPS | 30 FPS |
| --- | --- | --- | --- | --- | --- | --- | --- |
| WebGL2, RTX 3090 (ANGLE, `--gpu`) | serial | 3.9 ms | 5.1 ms | 5.9 ms | 3.0 ms | 99.7% | 100% |
| WebGL2, RTX 3090 (ANGLE, `--gpu`) | raf | 16.7 ms interval | 16.8 ms | 16.8 ms | 3.5 ms | 100% | 100% |
| WebGPU, RTX 3090 (headless shell 156, Vulkan) | serial | 6.2 ms | 8.1 ms | 10.0 ms | 3.3 ms | 99.7% | 100% |
| WebGPU, SwiftShader (CPU) | serial | 41.4 ms | 49.7 ms | 981 ms | 3.8 ms | 0% | 0% |
| WebGL2, SwiftShader (CPU) | serial | 54.8 ms | 63.2 ms | 401 ms | 3.7 ms | 0% | 0% |

The browser's CPU part, about 3–4 ms for 5,040 quads, covers building the
packed records in JavaScript, decoding and validating them in WebAssembly,
and drawing. The GPU devices hold 60 FPS. SwiftShader, which browsers use
when they have no usable GPU, emulates the GPU on the CPU: it cannot hold
30 FPS at 1080p with this crowd and shows long stalls. Weak devices that
fall back to it need a smaller view or fewer quads.

Per-pixel fill cost, for plain, framed and glowing layers, is measured
separately by the `fill_rate` example; see
[the architecture notes](ARCHITECTURE.md).
