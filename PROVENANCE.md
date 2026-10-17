# Provenance review

The repository is independently authored MIT work by Seraphina Studios.
Independent implementation remains the default. Before any outside source is
inspected as implementation reference or reused, confirm that its license is
compatible with MIT and record the exact source, revision, destination,
transformation, and notice here. GPL/AGPL material is never copied, adapted, or
used as a build or runtime dependency.

The only adapted implementation technique is the SDL3 raw-window-handle bridge
identified in `THIRD_PARTY_NOTICES.md`. Its exact source is the MIT-licensed
`sdl3` crate version 0.18.4 from crates.io, file
`examples/raw-window-handle-with-wgpu/main.rs`. Dependency identity and checksum
are fixed by `Cargo.lock`; the dependency is also retained normally rather than
vendored. Review found no embedded third-party asset or incompatible license.

The corpus is generated solely from constants in
`tilewright-render-testkit::synthetic_scene`. Its immutable identity string is
`tilewright-renderer-synthetic-resource-v1`. The fixture contains colored
rectangles only. `corpus/manifest.json`
records exact RGBA and semantic-plane digests, clock, dimensions, and the sole
allowed cross-backend visual tolerance.
