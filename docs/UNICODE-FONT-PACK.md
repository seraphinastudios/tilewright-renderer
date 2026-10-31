# Separate Unicode font delivery

The renderer's public UI crate carries `assets/unicode-font-pack.json`, the
exact immutable metadata manifest for the separate Noto source font pack.
Large font files stay in the source release and a separate data archive;
`tools/check-font-crate.py` rejects missing/different metadata and source font
payloads in the UI `.crate`, including payloads renamed to other extensions.

The pack has 17 unchanged OFL-1.1 faces from `google/fonts` revision
`6c1dbe27dcf7d817d4b62cec033ba77fa8e2a5ec`: Noto Sans, Arabic, Hebrew,
Devanagari, Bengali, Gujarati, Gurmukhi, Tamil, Telugu, Kannada, Malayalam,
Oriya, Sinhala, Thai, SC CJK, KR and monochrome Emoji. All original notices,
including the Noto Sans SC Reserved Font Name `Source`, remain beside the fonts.
`PROVENANCE.md` and `THIRD_PARTY_NOTICES.md` record the exact licensing.

The 423,554-byte manifest has SHA-256
`94b81a712fb41a66eb744c88ae2ed312cc5d0ec1a491e9821fcc278d99410f13`.
Font bytes total 39,611,796; original notices total 74,478. The exact inventory,
source and destination paths, sizes, hashes, copyright/name records and fallback
insertion order are in the manifest. The 49,070 mapped scalars form a finite
repertoire; missing scalars and shaping/variation sequences require diagnostics.

Run the dependency-free checks offline:

```sh
python3 tools/unicode-fonts/check.py
python3 -m unittest discover -s tools/unicode-fonts -p 'test_*.py'
python3 -m unittest tools.test_font_pack
```

Prepare only the separate archive in an existing empty output directory:

```sh
python3 tools/package-unicode-fonts.py --root . --output /absolute/empty-output
```

This creates `tilewright-unicode-noto-v1.tar.gz`,
`unicode-font-pack-provenance.json` and `unicode-font-pack-sbom.cdx.json` without
network access or credentials. The archive has normalized file order, modes,
UID/GID and timestamps, including gzip metadata. Its sole root contains
`manifest.json` and the exact `fonts/<family>/…` TTF/notice pairs. No source code,
project content, recovery state or additional files/directories are admitted.
The release package script includes all three artifacts in the top-level
`SHA256SUMS`; existing release artifact/upload globs already include them.
Preparing these artifacts does not publish or authorize a source/registry release.

Consumers prepare their own byte-identical pack from the published UI crate's
verified manifest and fixed public upstream sources, or use the separate source
font archive with the same verification. Preparation is explicit; packages and
runtime never silently download. The editor's fixed package directory is
`font-pack/` beside `bin/`. Native launch receives an explicit `--font-pack`
root and loads only bounded digest-verified bytes. It never searches OS fonts.

The shared MIT `tools/font_pack.py` delivery helper validates manifest identity,
paths, duplicate paths/keys, exact file and directory sets, per-file/total
bounds, hashes, links and special files. It provides reproducible archiving and
opt-in credential-free acquisition for consumers. The font source checker owns
font/cmap/license interpretation; the delivery helper does not duplicate that
parser. Failed explicit acquisitions leave only their new private partial
output for inspection; existing destinations are refused.
