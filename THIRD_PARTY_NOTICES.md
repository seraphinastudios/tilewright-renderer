# Third-party notices

Rust dependencies retain their licenses as reported by Cargo metadata and the
release SBOM. `policy/dependencies.json` records the approved SPDX licenses and
exceptions, registry sources, and direct-dependency purposes; `Cargo.lock` fixes
the exact resolved versions and checksums.

The raw SDL window handle adapter in `tilewright-render-sdl3` is an independently
adapted form of the public `sdl3` 0.18.4
`examples/raw-window-handle-with-wgpu/main.rs` technique. The upstream crate and
example are MIT licensed. The destination narrows it to a private synchronous
bridge, adds explicit safety invariants, and exposes no wgpu handle.

All scene fixtures, corpus descriptions, semantic masks, visual expectations,
and WGSL shader source are new synthetic MIT material authored for this
repository. No game implementation, media, authored game content, or GPL/AGPL
material is distributed here.
