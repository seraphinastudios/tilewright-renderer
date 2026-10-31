# Immutable UI font assets

`tilewright-render-ui/assets/` contains the 20 pixel **candidate** bitmap UI font,
not an editor painter or a runtime TrueType dependency. Owner acceptance of the
limited common-Latin UI scope is separate from preparing this reviewable asset.
The renderer consumes immutable ATX1 RGBA8 bytes and glyph metrics. It performs
no host font lookup, font parsing, rasterization, shaping, or network access.

The source is the unmodified `fonts/ttf/RobotoMono-Regular.ttf` from
[`googlefonts/RobotoMono` at `895ec691990d041dd727c7b5afa3ce56525d98e6`](https://github.com/googlefonts/RobotoMono/blob/895ec691990d041dd727c7b5afa3ce56525d98e6/fonts/ttf/RobotoMono-Regular.ttf),
version `3.001; ttfautohint (v1.8.4.7-5d5b)`, SHA-256
`af0bff7599c3df3831755c16e39b3c496df74b8c8d8a1161b14dc8461be17cb4`.
Its 125,748 bytes are retained for offline reproducibility. The exact upstream
OFL 1.1 notice is beside it as `OFL.txt`; no Reserved Font Names are declared
after that notice's copyright statement. The TTF, derived raster atlas,
metrics and generated Rust glyph data retain **OFL-1.1**, including in source
and binary packages; they are not relicensed MIT. The generator/checker code is
new MIT code. No upstream implementation source was adapted.

`manifest.json` records source/notice URLs, revisions, digests, generated file
hashes, rasterizer options, coverage and exact tool image. `source-cmap.json`
records all 876 mapped source scalars and the embedded copyright/name/license
records. `metrics.json` records the exact selected 337 scalars: printable ASCII,
Latin-1 excluding soft hyphen, Latin Extended-A and available selected symbols.
The requested hyphens U+2010..2012, U+201F, arrows U+2190..2193 and square U+25A1
are absent in the pinned source and explicitly omitted. Newline layout is a
compiler operation, not a glyph. Space and nonbreaking space have positive
advance and no frame. U+FFFD is a visible fallback. Other scalars, combining
marks, bidi controls and shaped scripts require explicit unsupported-text
diagnostics and preservation of original UTF-8 by the consumer; this asset
does not claim general Unicode coverage or shaping.

Glyph bearings are offsets from the left baseline, so negative `bearing[1]`
is above the baseline. Native size is 20 px; ascent/descent are 21/6 px and
line gap is explicitly zero. Advances come from independent scalars under
Pillow `Layout.BASIC`; kerning and shaping are not applied. Atlas frames include
the complete grayscale raster mask. Two transparent pixels separate frames.
The 512x256 atlas uses white RGB with antialiased alpha; `solid.atx` is a
separate white 1x1 texture. Both use the renderer's existing ATX1 envelope.

Check all hashes, exact coverage, source cmap, frame bounds/overlap, visible
fallback, transparent spacing and Rust/JSON consistency without external tools:

```sh
python3 tools/ui-font/check.py
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tools/ui-font -p 'test_*.py'
```

For byte-exact regeneration, use the existing development image pinned by
digest, with Python 3.13.5, Pillow 11.1.0 and FreeType 2.13.3 (development tools
only; Pillow HPND and FreeType FTL licenses). No rasterizer code is bundled with
the crate. The generator passes verified TTF **bytes** to Pillow, preventing
ambient font fallback. The API behavior follows the
[Pillow 11.1.0 ImageFont documentation](https://pillow.readthedocs.io/en/11.1.0/reference/ImageFont.html).

```sh
sh tools/ui-font/reproduce.sh
```

This builds twice in separate output directories with the pinned cached image,
one CPU, 2 GiB, no network, read-only source/script mounts and no credentials or
Docker socket inside the container, then compares both generations and the
checked-in files. The script fails if the image is absent; it never installs
tools on the host or silently pulls an image. Image construction/acquisition is
an explicit development-environment preparation step, not a build/runtime
download. The dependency-free checker is the normal repository validation seam.
