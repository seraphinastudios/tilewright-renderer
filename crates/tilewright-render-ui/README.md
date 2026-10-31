# Shared UI compiler candidate

The additive positioned display list compiles logical rectangles and text into
existing `tilewright_scene::Quad` values on `Layer::Interface`. The original
`UiQuad`, `TextRun` and `UiFrame` struct layouts remain unchanged. This worktree
is a local candidate; it does not identify a published registry release.

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
disable lossy visual editing of unsupported text. This bounded font candidate is
pending the owner's font-scope choice.

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
