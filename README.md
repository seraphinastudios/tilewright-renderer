# Tilewright renderer

Tilewright renderer is an MIT-licensed Rust GPU renderer for isometric and
top-down tile worlds. Game clients, editors, previewers, and deterministic
offscreen tools share it. The workspace defines a renderer-neutral scene and
API, an explicit resource-provider boundary, a world compiler, a software
reference implementation, a private wgpu backend, a thin SDL3 presentation
bridge, a UI seam, a conformance corpus, and the `tilewright-render` proof CLI.

Tilewright is maintained by Seraphina Studios alongside
[tilewright-world](https://github.com/seraphinastudios/tilewright-world), the
world format and toolkit, and
[tilewright-editor](https://github.com/seraphinastudios/tilewright-editor).

## Development model

Development is primarily agent-driven, under maintainer direction, review,
provenance controls, tests, and repository validation. Direct human-written
code contributions are welcome under the same controls. See
[provenance](PROVENANCE.md) for the reuse record.

Art, maps, and other game content drawn by the renderer are supplied by each
game with its own credits and licenses; they are not part of the renderer's
MIT-licensed code. Projections, transforms, composed frames, and test output
are deterministic program output.

No public scene or renderer API exposes SDL3 or wgpu handles. Consumer-specific
state, filesystem/network discovery, game rules, protocol messages, event loops,
and editor transactions remain outside this repository.

## Validate

The pinned baseline is Rust 1.97.1, edition 2024. SDL 3.4.18 is compiled from
the Cargo-locked source rather than selected from the host. The aggregate required check
is `Renderer validation`.

```sh
cargo build --locked --workspace
cargo test --locked --workspace --all-targets
cargo run --locked --package tilewright-render -- --version
cargo run --locked --package tilewright-render -- probe
cargo run --locked --package tilewright-render -- corpus
SDL_VIDEO_DRIVER=x11 xvfb-run -a -s '-screen 0 1024x768x24' \
  cargo run --locked --package tilewright-render --features sdl3 -- window
tools/validate.sh
```

`tools/validate.sh` runs formatting, strict Clippy (native and wasm32), unit/doc tests, shader
validation, dependency/architecture/provenance policy, deterministic corpus
comparison, Linux Vulkan offscreen proof, Windows/D3D12 cross-check, release
build, SBOM creation, a release dry run, and the browser package's build and
headless Chromium WebGPU/WebGL2 tests. The SDL bridge is an explicit
`sdl3` feature so the default proof CLI remains portable to the native Windows
toolchain while Linux validation exercises a real window under Xvfb.

## Offscreen proof

Output is a small header followed by 64×64 RGBA8 bytes. The path must not
exist; persistence is same-directory, durable, and no-clobber.

```sh
cargo run --locked --package tilewright-render -- offscreen /tmp/tilewright-proof.rgba
test "$(wc -c </tmp/tilewright-proof.rgba)" -eq 16407
rm /tmp/tilewright-proof.rgba
```

## World views

`tilewright-render-world` compiles a tile world (camera, height field, and
objects) into a scene: isometric or top-down projection, terrain stretching,
draw order, multi-tile footprints, and effects. The CLI renders a
[world-view file](docs/WORLD-VIEW.md) whose textures are digest-verified PNGs:

```sh
cargo run --locked --package tilewright-render -- world view.json /tmp/frame.png
cargo run --locked --package tilewright-render -- world view.json /tmp/ref.png --reference
```

`--reference` renders with the CPU reference instead of the GPU.

## Browsers

The same renderer runs in browsers, compiled to WebAssembly and drawing with
WebGPU or WebGL2, behind the TypeScript package in `packages/tilewright-render`.
See the [browser package](docs/WEB.md) for its API, bundle sizes, build and
tests, and the [frame-time harness](docs/FRAME-TIME.md) for measured frame
times natively and in Chromium:

```sh
tools/build-web.sh
tools/validate-web.sh
```

Scenes are lists of textured quads; a quad can draw one frame of an atlas or
sprite sheet, so layered character sheets share one texture. See the
[scene contract](docs/SCENE.md), [architecture and limits](docs/ARCHITECTURE.md), [GPU lifecycle](docs/GPU-LIFECYCLE.md),
[corpus policy](corpus/README.md), and [provenance](PROVENANCE.md).

The eight renderer libraries publish to crates.io from verified source-release
archives. Cargo versions remain independent of source tags; see
[crate release and initial ownership](docs/CRATE-RELEASE.md). The diagnostics CLI
remains a GitHub release application.
