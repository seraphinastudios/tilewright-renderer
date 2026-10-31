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
allowed cross-backend visual tolerance. The world and atlas scenarios draw
procedural textures generated from constants in
`tilewright-render-testkit` (`src/world.rs` and `src/atlas.rs`); the atlas's
characters and ground tiles are original geometric shapes, not derived from
any game or third-party art. `corpus/manifest.json` records every fixture
texture's identity, size, SHA-256, source, author, license, transformation
and notice; validation regenerates the identities and digests and checks the
provenance fields.

## Bundled UI font data

The 20 px common-Latin bitmap UI font candidate in
`crates/tilewright-render-ui/assets/` derives only from the unmodified
[`fonts/ttf/RobotoMono-Regular.ttf`](https://github.com/googlefonts/RobotoMono/blob/895ec691990d041dd727c7b5afa3ce56525d98e6/fonts/ttf/RobotoMono-Regular.ttf)
at `googlefonts/RobotoMono` revision
`895ec691990d041dd727c7b5afa3ce56525d98e6` (source SHA-256
`af0bff7599c3df3831755c16e39b3c496df74b8c8d8a1161b14dc8461be17cb4`).
Copyright 2015 The Roboto Mono Project Authors
(https://github.com/googlefonts/robotomono). The exact upstream `OFL.txt` is
retained beside the source; it declares SIL Open Font License 1.1 and no
Reserved Font Names after its copyright statement. Embedded TTF copyright and
license records agree. Font source, atlas, metrics and generated Rust font data
remain `OFL-1.1`, not MIT; the license permits embedding/bundling with software
while requiring retention of this notice and license.

`tools/ui-font/generate.py` is new MIT development tooling. It independently
parses the pinned source's Unicode cmap and name records, then rasterizes an
explicit verified byte stream at 20 px with Pillow 11.1.0 / FreeType 2.13.3,
`Layout.BASIC`, independent scalar masks at a left-baseline anchor. It packs
337 mapped printable common-Latin/symbol scalars into a padded 512x256 white
RGBA8 ATX1 atlas and emits per-glyph frames, bearings and advances, exact scalar
coverage and a visible U+FFFD fallback. It also emits a synthetic 1x1 white
ATX1 solid texture. The font does not provide general Unicode coverage,
combining/shaping or bidi; unsupported text must be flagged and preserved.

`assets/manifest.json` records exact provenance, notices, tool image, options,
coverage and hashes. `source-cmap.json` retains the full source cmap/name
inventory. `tools/ui-font/reproduce.sh` regenerates twice offline in the exact
image and compares bytes; `check.py` validates all source/derived identities,
coverage and bounds without a rasterizer. No host font discovery, runtime font
parsing/rasterization, remote fetch or new Rust dependency is introduced.
Pillow (HPND) and FreeType (FTL) are isolated development tools only; none of
their source is copied or linked into the renderer. See the generator README
and retained OFL notice for the full record.

## Unicode shaping font source pack

`assets/unicode/` is a separate trusted source/editor font pack, not a large
embedded dependency of a public renderer crate. It retains 17 unmodified upright
variable TrueType fonts from the OFL directories of
[`google/fonts` revision `6c1dbe27dcf7d817d4b62cec033ba77fa8e2a5ec`](https://github.com/google/fonts/tree/6c1dbe27dcf7d817d4b62cec033ba77fa8e2a5ec/ofl).
`assets/unicode/manifest.json` (SHA-256
`94b81a712fb41a66eb744c88ae2ed312cc5d0ec1a491e9821fcc278d99410f13`)
records exact upstream paths, destination paths, byte counts/digests, copyright,
author, original notice identities, Reserved Font Names and transformation
(`none; original upstream font bytes retained`). Each font's original upstream
OFL.txt is beside it. All 34 source/notice files also matched the pinned GitHub
tree's Git blob identities during acquisition verification.

The stable insertion order is Noto Sans, Arabic, Hebrew, Devanagari, Bengali,
Gujarati, Gurmukhi, Tamil, Telugu, Kannada, Malayalam, Oriya, Sinhala, Thai,
SC CJK, KR Hangul and monochrome Noto Emoji. Fonts total 39,611,796 bytes; notices total
74,478 bytes. Font bytes are ordinary Git blobs with an exact source-pack
attribute exception, not LFS pointers. No font is subsetted, renamed, modified,
rasterized or relicensed. Runtime shaping/rasterization occurs in the renderer
from explicitly supplied digest-verified immutable bytes; it does not search
system fonts or fetch network fonts. Consumers explicitly load the separate
trusted pack; public crates do not implicitly embed it.

The source data are OFL-1.1, not MIT. Noto Sans SC and KR declare Reserved Font Name
`Source` in its original Adobe notice; the other notices declare none. Exact
embedded name/copyright/license records are inventoried rather than rewritten.
The pack's 49,070 mapped scalars cover common Latin/Greek/Cyrillic and combining
marks, Arabic/Hebrew, the named Indic/Sinhala families, Thai, available SC CJK, Korean Hangul
and monochrome emoji. This finite cmap repertoire does not claim every Unicode
scalar or shaping/variation sequence; missing-glyph diagnostics remain required.

`tools/unicode-fonts/` is independently authored MIT Python tooling. The offline
checker verifies the pinned manifest first, exact font/notice bytes, full
cmap/name inventory, license/RFN records, bounded sizes, representative script
scalars and exact file sets. Adversarial tests cover altered manifests, corrupt
or truncated font/LFS-pointer bytes, path traversal, symlinks and malformed SFNT
bounds. The explicit opt-in acquisition script allows only the fixed revision on
the public raw.githubusercontent.com host, rejects redirects, sends no
credentials/cookies, bounds each download and publishes only a checked staged
pack. It is never invoked by a build/runtime. No upstream implementation code,
GPL/AGPL font, external rasterizer or host font is bundled by this tooling.
