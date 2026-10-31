# Pinned Unicode font source pack

The separate trusted font pack in `assets/unicode/` is input to the renderer's
explicit immutable font-byte API. It contains 16 **unmodified** upright variable
TrueType faces from one `google/fonts` revision,
[`6c1dbe27dcf7d817d4b62cec033ba77fa8e2a5ec`](https://github.com/google/fonts/tree/6c1dbe27dcf7d817d4b62cec033ba77fa8e2a5ec/ofl).
The original fonts total **29,197,208 bytes**; their exact full OFL notices add
70,090 bytes. A deterministic USTAR/gzip level-9 transport of the manifest and
source/notice paths is 17,805,052 bytes (measured with Python 3.14). Neither a public renderer crate nor its normal build embeds this
large pack. No font is downloaded during builds or runtime.

`assets/unicode/manifest.json` is the immutable pack identity. Its SHA-256 is
`a332b35dcb2874f8517552a9ee6e3a6509e6728dfdbe71b1efbde270069aa2d8`.
It records the ordered fallback inventory, per-file upstream paths/byte lengths/
SHA-256, author, copyright, full notice identity, Reserved Font Names,
transformation (`none`), exact Unicode cmap ranges and selected embedded SFNT
name/license records. `fonts` array order defines insertion order; it starts
with Noto Sans and ends with Noto Sans SC and Noto Emoji. Weight 400 must be
selected explicitly for normal text; some upstream legacy family records name
the minimum variable weight “Thin”. Font source/licensing data remain OFL-1.1;
these new Python tools are MIT.

The pack supplies Latin, Greek, Cyrillic and combining marks; Arabic and Hebrew;
Devanagari, Bengali, Gujarati, Gurmukhi, Tamil, Telugu, Kannada, Malayalam, Oriya
and Sinhala; Thai; SC CJK (including available kana); and monochrome emoji.
Its exact union contains **37,236 mapped scalars**. This is a finite repertoire,
not a promise of every Unicode character, language, regional CJK appearance or
emoji sequence. In particular, cmap presence is not proof that a shaping or
variation sequence works. The text engine must preserve the original UTF-8 and
report uncovered scalars explicitly. Runtime shaping/rasterization belongs to
the renderer, not this acquisition/checking tool.

All notices declare SIL OFL 1.1. Noto Sans SC's upstream notice retains
`Copyright 2014-2021 Adobe (http://www.adobe.com/), with Reserved Font Name 'Source'`.
No other bundled notice declares Reserved Font Names. Noto Emoji's notice is
`Copyright 2013 Google LLC`; the other notices identify the respective Noto
Project Authors. Exact original notices and embedded records are retained,
including differing upstream copyright descriptions; no font is renamed,
subsetted, rasterized, edited or relicensed. OFL embedding/bundling requires
retaining the notices and license. No GPL/AGPL font or font implementation is
included.

Run the normal offline verification without external Python packages:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 tools/unicode-fonts/check.py
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tools/unicode-fonts -p 'test_*.py'
```

The checker pins the manifest before using any paths, bounds bytes, verifies
source and notice hashes, rejects LFS pointers/symlinks/unexpected font files,
reconstructs exact cmap/name inventory, and checks representative script
scalars plus the replacement-character mapping. It never searches host fonts or
rasterizes. Tests exercise mutated manifests, wrong/truncated/pointer bytes,
path traversal, symlinks and malformed SFNT tables. Font-byte authenticity comes
from the pinned manifest; this parser is development validation, not a runtime
font security boundary.

For deliberate reacquisition only, into a **new** output directory:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 tools/unicode-fonts/acquire.py --output-dir /tmp/tilewright-unicode-fonts
PYTHONDONTWRITEBYTECODE=1 python3 tools/unicode-fonts/check.py --assets /tmp/tilewright-unicode-fonts
```

Acquisition uses the one fixed revision on allowlisted
`https://raw.githubusercontent.com/google/fonts/`, denies redirects, sends no
credentials/cookies, ignores proxy configuration, bounds every response and
checks exact bytes before publishing a verified staged directory. It cannot
update the checked-in pack or choose a moving branch. Failure removes only its
private staging directory. No host packages, tools or fonts are installed.

The distributable pack is a directory preserving `manifest.json` and every
manifest font/notice relative path. Consumers load that explicit local directory
or package their trusted bytes; runtime ZIP/TAR extraction is unnecessary.
Release compression is transport only and does not change manifest identity.
Source Git attributes exempt exactly the `assets/unicode/fonts/*/*.ttf` pack
from LFS, retaining actual source bytes in normal offline checkouts/archives.
