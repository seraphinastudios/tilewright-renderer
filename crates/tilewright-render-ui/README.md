# Shared UI and Unicode text candidate

The additive positioned display list compiles logical rectangles and text into
existing `tilewright_scene::Quad` values on `Layer::Interface`. The original
`UiQuad`, `TextRun` and `UiFrame` struct layouts remain unchanged. This worktree
is a local candidate; it does not identify a published registry release.

## Unicode text

`FontPack::new` verifies an ordered set of `FontBlob { family, bytes, sha256 }`
inputs. `TextSystem::new` accepts that pack and explicit `TextLimits`; create one
engine per renderer session. The large OFL Noto pack is separate application
data. `unicode_fonts::FONTS` and `FONT_PACK_MANIFEST` identify the exact files,
lengths, digests and notices. The consumer supplies verified bytes from its
explicit trusted asset source; this crate never searches the filesystem or loads
system fonts. The published crate contains identities, not the 39.6 MB font pack.

`TextSystem::layout(text, size, width)` performs advanced shaping, deterministic
font fallback and bidirectional layout. A width enables wrapping; `None` preserves
a single visual line per source line. Layout retains original UTF-8 without
normalization and exposes byte cluster ranges, extended-grapheme boundaries,
`caret`, `hit`, `move_visual` and `selection_rects`. `TextCursor` carries an
original byte offset plus `TextAffinity` to distinguish bidi/wrap boundaries.
Invalid interior-grapheme cursors have no caret/selection geometry. Ligature
interiors use advance subdivisions; this API does not expose font-specific GDEF
ligature caret positions. Selection may produce multiple disconnected rectangles.

`TextSystem::compile(viewport, list, solid, first_resource, limits)` consumes the
same positioned display list and emits ordinary interface quads plus immutable
`UnicodeTexture` ATX1 resources. `builtin::FONT_ID` selects the pack primary role;
unknown role IDs fail. Bind the returned resources by full ID/revision/digest,
retain them while frames are in flight, and reserve their IDs against World.
The engine advances revisions when atlas bytes change and reuses identity for
unchanged bytes. Retire the engine resource namespace before replacing an engine.
The CPU reference and GPU renderers consume identical glyph bytes and scene
geometry. The engine supports monochrome outline fonts (including bundled emoji);
color bitmap/outline font tables are rejected before rasterization.

The pack covers the accepted Latin, combining, Arabic, Hebrew, Indic, Han/kana,
Hangul and emoji corpus. Coverage is finite, not every Unicode scalar or emoji
sequence. Missing glyphs retain their original byte ranges and produce a visible
font fallback and diagnostics. Consumers preserve all source text; they decide
whether missing visible coverage should prevent a specific review action.

Defaults cap fonts at 32/64 MiB, one text request at 64 KiB, glyphs at 65536 and
lines at 4096; caller limits can tighten these values. Size is 1–256 logical
pixels, raster size at most 1024 pixels, and the atlas at most 16 Mi pixels
(64 MiB RGBA plus 12 ATX1 header bytes). Set a smaller raster-pixel budget if the
consumer resource-provider byte cap is lower. Glyph outline bounds are checked
before pixel allocation. Retained raster images are capped at 4096/16 MiB;
layouts at 256 entries and an 8 MiB conservative text/glyph retention charge.
Private shaping scratch is rebuilt every 256 uncached requests from the same
immutable database, preserving face IDs and existing layouts. `clear_cache`
drops retained work without invalidating caller-held layouts/resources.

Focus, IME preedit, selection state, input generation fencing, and source edits
belong to the consumer. Compose preedit only into transient display text and use
this exact layout for caret geometry and IME placement; commits reach document
authority through its existing review/transaction path.

## Legacy bitmap text

The original `compile_ui` API below retains its bounded unshaped repertoire.

```rust
use tilewright_render_ui::{builtin, BitmapFontSet, UiDisplayList, UiLimits,
    UiPaint, UiPrimitive, UiRect, UiTexture, UiViewport, compile_ui};
use tilewright_scene::{Digest256, ResourceId, SemanticId};

// Reserve both IDs against the consumer's world resource namespace.
let atlas = UiTexture { id: ResourceId(100), revision: 1,
    digest: Digest256(builtin::ATLAS_SHA256) };
let white = UiTexture { id: ResourceId(101), revision: 1,
    digest: Digest256(builtin::SOLID_SHA256) };
let font = builtin::builtin_font(atlas)?;
let font_values = [font];
let fonts = BitmapFontSet::new(&font_values)?;
let primitives = [UiPrimitive::Text {
    origin: [12.0, 8.0],
    clip: UiRect { x: 0.0, y: 0.0, width: 320.0, height: 200.0 },
    paint: UiPaint { semantic_id: SemanticId(100), painter_order: 1,
        color: [1.0; 4] },
    font: builtin::FONT_ID, size: 20.0, text: "Café inspector",
}];
let compiled = compile_ui(
    UiViewport { logical_size: [320, 200], physical_size: [480, 300] },
    UiDisplayList { primitives: &primitives }, &fonts, white, UiLimits::default(),
)?;
# Ok::<(), tilewright_render_ui::UiError>(())
```

The consumer supplies the immutable `builtin::ATLAS_BYTES` and `SOLID_BYTES`
through its ordinary digest-verified resource provider. The compiler never
accesses images, files, windows, GPUs or system fonts. The bundled font candidate
retains OFL-1.1; exact upstream revision, generation options, supported scalars,
digests and license are in `assets/manifest.json` and `assets/OFL.txt`.

Origin is the top-left of a logical line box; the shared compiler computes its
baseline from the font ascent. Glyph bearing is relative to that baseline.
Metrics and original UTF-8 byte positions remain logical; emitted positions are
physical pixels after exactly one per-axis viewport conversion. Clips intersect
the logical viewport. Partial clipping changes UVs and geometry while preserving
the original atlas frame, including at fractional DPI. Spaces advance without
quads. Newline advances to the next baseline; there is no wrapping or shaping.

Unsupported scalars draw a visible fallback and produce diagnostics containing
the original character and exact UTF-8 byte interval. Combining marks, bidi
controls and complex scripts are unsupported even if a supplied font has a glyph.
`font.supports_text(full_original_text)` uses the same support rule without
truncation or normalization. Consumers must preserve the original field value and
disable lossy visual editing of unsupported text. Use the Unicode path above for international editing.

Input byte, scalar, primitive, line, viewport and output budgets are bounded by
caller limits and absolute caps. Validation and a dry compilation pass precede
output allocation. A semantic ID receives consecutive checked `u16` parts across
all primitives, including clipped glyphs, spaces and line breaks; overflow is an
error. Painter order is supplied explicitly and is not rewritten.

Before composing world and UI quads, call `validate_ui_composition` and then the
normal `SceneBundle::from_quads` validation. The consumer must reserve UI semantic
IDs, route immutable providers by full resource identity, and route input through
panel/control hit regions before world picking. An opaque nonpickable quad does
not erase world semantic masks. Control state, layout, focus, editing, pointer
capture and displayed-generation input checks belong to the consumer.
