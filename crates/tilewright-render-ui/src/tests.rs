// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

#![allow(clippy::float_cmp, clippy::cast_precision_loss)]

use super::*;
use tilewright_scene::{
    Digest256, Layer, ResourceId, SceneBundle, SceneLimits, TextureRect, Viewport,
};

const FRAME: TextureRect = TextureRect {
    x: 2,
    y: 3,
    width: 8,
    height: 10,
};
const GLYPHS: &[BitmapGlyph] = &[
    BitmapGlyph {
        character: ' ',
        frame: None,
        bearing: [0.0, 0.0],
        advance: 5.0,
    },
    BitmapGlyph {
        character: 'A',
        frame: Some(FRAME),
        bearing: [1.0, -8.0],
        advance: 9.0,
    },
    BitmapGlyph {
        character: 'é',
        frame: Some(FRAME),
        bearing: [1.0, -8.0],
        advance: 9.0,
    },
    BitmapGlyph {
        character: '\u{301}',
        frame: Some(FRAME),
        bearing: [1.0, -8.0],
        advance: 0.0,
    },
    BitmapGlyph {
        character: '\u{202e}',
        frame: Some(FRAME),
        bearing: [1.0, -8.0],
        advance: 0.0,
    },
    BitmapGlyph {
        character: '\u{fffd}',
        frame: Some(FRAME),
        bearing: [1.0, -8.0],
        advance: 9.0,
    },
];

fn texture(id: u64) -> UiTexture {
    UiTexture {
        id: ResourceId(id),
        revision: 1,
        digest: Digest256([1; 32]),
    }
}
fn metrics() -> FontMetrics {
    FontMetrics {
        native_size: 10.0,
        ascent: 8.0,
        descent: 2.0,
        line_gap: 2.0,
    }
}
fn font() -> BitmapFont<'static> {
    BitmapFont::new(
        FontId(1),
        texture(2),
        [16, 16],
        metrics(),
        GLYPHS,
        '\u{fffd}',
    )
    .unwrap()
}
fn rect(x: f32, y: f32, width: f32, height: f32) -> UiRect {
    UiRect {
        x,
        y,
        width,
        height,
    }
}
fn clip() -> UiRect {
    rect(0.0, 0.0, 100.0, 100.0)
}
fn paint(id: u32) -> UiPaint {
    UiPaint {
        semantic_id: SemanticId(id),
        painter_order: 0,
        color: [1.0; 4],
    }
}
fn text(value: &str) -> UiPrimitive<'_> {
    UiPrimitive::Text {
        origin: [0.0, 0.0],
        clip: clip(),
        paint: paint(1),
        font: FontId(1),
        size: 10.0,
        text: value,
    }
}
fn solid(id: u32) -> UiPrimitive<'static> {
    UiPrimitive::Solid {
        bounds: rect(0.0, 0.0, 10.0, 10.0),
        clip: clip(),
        paint: paint(id),
        pickable: true,
    }
}
fn compile(
    primitives: &[UiPrimitive<'_>],
    physical: u32,
    limits: UiLimits,
) -> Result<CompiledUi, UiError> {
    let fonts = [font()];
    compile_ui(
        UiViewport {
            logical_size: [100, 100],
            physical_size: [physical, physical],
        },
        UiDisplayList { primitives },
        &BitmapFontSet::new(&fonts).unwrap(),
        texture(1),
        limits,
    )
}

#[test]
fn scales_once_at_all_required_dpis_and_uses_interface_scene_contract() {
    for physical in [100, 125, 150, 200] {
        let result = compile(&[solid(2), text("A")], physical, UiLimits::default()).unwrap();
        let scale = physical as f32 / 100.0;
        assert_eq!(
            result.quads[1].positions,
            [
                [scale, 0.0],
                [9.0 * scale, 0.0],
                [9.0 * scale, 10.0 * scale],
                [scale, 10.0 * scale]
            ]
        );
        assert_eq!(result.text_layouts[0].glyphs[0].baseline, [0.0, 8.0]);
        assert_eq!(result.text_layouts[0].size, [9.0, 10.0]);
        assert_eq!(result.quads[1].layer, Layer::Interface);
        assert!(!result.quads[1].pickable);
        SceneBundle::from_quads(
            1,
            0,
            Viewport {
                width: physical,
                height: physical,
                scale_milli: 1000,
            },
            [0.0; 4],
            result.quads,
            SceneLimits::default(),
        )
        .unwrap();
    }
}

#[test]
fn every_clip_edge_crops_uvs_without_changing_original_frame() {
    let cases = [
        (rect(3.0, 0.0, 100.0, 100.0), [0.25, 0.0], [1.0, 1.0]),
        (rect(0.0, 0.0, 7.0, 100.0), [0.0, 0.0], [0.75, 1.0]),
        (rect(0.0, 2.5, 100.0, 100.0), [0.0, 0.25], [1.0, 1.0]),
        (rect(0.0, 0.0, 100.0, 7.5), [0.0, 0.0], [1.0, 0.75]),
    ];
    for (clip, uv_min, uv_max) in cases {
        let UiPrimitive::Text {
            origin,
            paint,
            font,
            size,
            text,
            ..
        } = text("A")
        else {
            unreachable!()
        };
        let result = compile(
            &[UiPrimitive::Text {
                origin,
                clip,
                paint,
                font,
                size,
                text,
            }],
            125,
            UiLimits::default(),
        )
        .unwrap();
        assert_eq!(result.quads[0].frame, Some(FRAME));
        assert_eq!(result.quads[0].uvs[0], uv_min);
        assert_eq!(result.quads[0].uvs[2], uv_max);
    }
}

#[test]
fn clips_solid_and_empty_panel_and_target_without_spurious_quads() {
    let result = compile(
        &[UiPrimitive::Solid {
            bounds: rect(-10.0, -10.0, 20.0, 20.0),
            clip: clip(),
            paint: paint(1),
            pickable: true,
        }],
        150,
        UiLimits::default(),
    )
    .unwrap();
    assert_eq!(result.quads[0].positions[0], [0.0, 0.0]);
    assert_eq!(result.quads[0].uvs[0], [0.5, 0.5]);
    for empty in [rect(0.0, 0.0, 0.0, 20.0), rect(200.0, 200.0, 10.0, 10.0)] {
        let result = compile(
            &[UiPrimitive::Solid {
                bounds: clip(),
                clip: empty,
                paint: paint(1),
                pickable: true,
            }],
            100,
            UiLimits::default(),
        )
        .unwrap();
        assert!(result.quads.is_empty());
    }
}

#[test]
fn whitespace_baselines_and_original_utf8_boundaries_are_explicit() {
    let result = compile(&[text("A é\nA")], 100, UiLimits::default()).unwrap();
    assert_eq!(result.quads.len(), 3);
    assert_eq!(result.text_layouts[0].size, [23.0, 22.0]);
    assert_eq!(result.text_layouts[0].lines, 2);
    let glyphs = &result.text_layouts[0].glyphs;
    assert_eq!((glyphs[2].byte_start, glyphs[2].byte_end), (2, 4));
    assert_eq!(glyphs[1].advance, 5.0);
    assert_eq!(glyphs[4].baseline, [0.0, 20.0]);
    assert_eq!(
        result
            .quads
            .iter()
            .map(|quad| quad.part)
            .collect::<Vec<_>>(),
        vec![0, 2, 4]
    );
    assert!(result.unsupported.is_empty());
}

#[test]
fn unsupported_combining_bidi_and_unknown_scalars_are_visible_and_lossless() {
    let value = "Aé😀\u{301}\u{202e}\r";
    let result = compile(&[text(value)], 100, UiLimits::default()).unwrap();
    assert_eq!(result.unsupported.len(), 4);
    assert_eq!(result.quads.len(), 6);
    assert!(!font().supports_text(value));
    assert!(font().supports_text("A é\nA"));
    for diagnostic in &result.unsupported {
        assert_eq!(
            &value[diagnostic.byte_start..diagnostic.byte_end],
            diagnostic.character.to_string()
        );
    }
    assert_eq!(result.unsupported[0].character, '😀');
    assert_eq!(result.unsupported[0].byte_start, 3);
    assert_eq!(
        result.unsupported[1].reason,
        UnsupportedReason::RequiresShaping
    );
    // Even an explicitly supplied mark/control glyph cannot claim shaping.
    assert!(!font().supports_text("\u{301}\u{202e}"));
    assert_eq!(
        compile(&[text("Z")], 100, UiLimits::default())
            .unwrap()
            .unsupported[0]
            .reason,
        UnsupportedReason::MissingGlyph
    );
}

#[test]
fn parts_are_unique_across_primitives_and_stable_under_clipping() {
    let input = [solid(1), text(" A"), solid(1), text("A")];
    let first = compile(&input, 100, UiLimits::default()).unwrap();
    assert_eq!(
        first.quads.iter().map(|quad| quad.part).collect::<Vec<_>>(),
        vec![0, 2, 3, 4]
    );
    assert_eq!(first, compile(&input, 100, UiLimits::default()).unwrap());
    let overflow = " ".repeat(65_536);
    assert!(compile(&[text(&overflow)], 100, UiLimits::default()).is_ok());
    assert_eq!(
        compile(&[text(&overflow), solid(1)], 100, UiLimits::default()),
        Err(UiError::PartOverflow)
    );
}

#[test]
fn validates_all_budgets_and_nonfinite_values() {
    let defaults = UiLimits::default();
    for limits in [
        UiLimits {
            maximum_primitives: 0,
            ..defaults
        },
        UiLimits {
            maximum_text_bytes: 0,
            ..defaults
        },
        UiLimits {
            maximum_glyphs: 0,
            ..defaults
        },
        UiLimits {
            maximum_quads: 0,
            ..defaults
        },
        UiLimits {
            maximum_lines: 0,
            ..defaults
        },
        UiLimits {
            maximum_pixels: 9999,
            ..defaults
        },
        UiLimits {
            maximum_dimension: 99,
            ..defaults
        },
    ] {
        assert_eq!(
            compile(&[text("A")], 100, limits),
            Err(UiError::LimitExceeded)
        );
    }
    for invalid in [f32::NAN, f32::INFINITY, -1.0] {
        assert_eq!(
            compile(
                &[UiPrimitive::Text {
                    origin: [0.0; 2],
                    clip: clip(),
                    paint: paint(1),
                    font: FontId(1),
                    size: invalid,
                    text: "A"
                }],
                100,
                defaults
            ),
            Err(UiError::InvalidValue)
        );
    }
    assert_eq!(
        compile(
            &[UiPrimitive::Solid {
                bounds: rect(f32::MAX, 0.0, 1.0, 1.0),
                clip: clip(),
                paint: paint(1),
                pickable: false
            }],
            100,
            defaults
        ),
        Err(UiError::InvalidValue)
    );
    assert_eq!(
        compile(
            &[UiPrimitive::Text {
                origin: [0.0; 2],
                clip: clip(),
                paint: paint(1),
                font: FontId(3),
                size: 10.0,
                text: "A"
            }],
            100,
            defaults
        ),
        Err(UiError::UnknownFont)
    );
    let mut invalid = paint(1);
    invalid.color[0] = f32::NAN;
    assert_eq!(
        compile(
            &[UiPrimitive::Solid {
                bounds: clip(),
                clip: clip(),
                paint: invalid,
                pickable: true
            }],
            100,
            defaults
        ),
        Err(UiError::InvalidValue)
    );
}

#[test]
fn malformed_fonts_missing_fallback_and_atlas_mismatch_are_rejected() {
    fn make(
        metrics: FontMetrics,
        glyphs: &[BitmapGlyph],
        fallback: char,
    ) -> Result<BitmapFont<'_>, UiError> {
        BitmapFont::new(FontId(1), texture(2), [16, 16], metrics, glyphs, fallback)
    }
    assert_eq!(
        make(
            FontMetrics {
                native_size: f32::NAN,
                ..metrics()
            },
            GLYPHS,
            '\u{fffd}'
        ),
        Err(UiError::InvalidFont)
    );
    assert_eq!(make(metrics(), GLYPHS, 'Z'), Err(UiError::InvalidFont));
    let mut glyphs = GLYPHS.to_vec();
    glyphs[1].frame = Some(TextureRect { x: 15, ..FRAME });
    assert_eq!(
        make(metrics(), &glyphs, '\u{fffd}'),
        Err(UiError::InvalidFont)
    );
    glyphs[1] = GLYPHS[1];
    glyphs[2] = GLYPHS[1];
    assert_eq!(
        make(metrics(), &glyphs, '\u{fffd}'),
        Err(UiError::InvalidFont)
    );
    assert!(BitmapFontSet::new(&[font(), font()]).is_err());
    let fonts = [font()];
    assert_eq!(
        compile_ui(
            UiViewport {
                logical_size: [100; 2],
                physical_size: [100; 2]
            },
            UiDisplayList { primitives: &[] },
            &BitmapFontSet::new(&fonts).unwrap(),
            texture(2),
            UiLimits::default()
        ),
        Err(UiError::ResourceCollision)
    );
}

#[test]
fn composition_rejects_semantic_and_resource_identity_collisions() {
    let result = compile(&[solid(1)], 100, UiLimits::default()).unwrap();
    assert_eq!(
        validate_ui_composition(&result.quads, &result),
        Err(UiError::IdentityCollision)
    );
    let world = [tilewright_scene::Quad {
        semantic_id: SemanticId(99),
        resource_revision: 2,
        ..result.quads[0]
    }];
    assert_eq!(
        validate_ui_composition(&world, &result),
        Err(UiError::ResourceCollision)
    );
    let world = [tilewright_scene::Quad {
        semantic_id: SemanticId(99),
        ..result.quads[0]
    }];
    assert_eq!(validate_ui_composition(&world, &result), Ok(()));
}

#[test]
fn builtin_immutable_font_validates_common_latin_and_exact_digest() {
    let texture = UiTexture {
        digest: Digest256(builtin::ATLAS_SHA256),
        ..texture(2)
    };
    let font = builtin::builtin_font(texture).unwrap();
    assert!(font.supports_text("Toolbar Café Œuvre"));
    assert!(!font.supports_text("e\u{301}"));
    assert_eq!(
        builtin::builtin_font(super::tests::texture(2)),
        Err(UiError::InvalidFont)
    );
    assert_eq!(&builtin::ATLAS_BYTES[..4], b"ATX1");
    assert_eq!(builtin::SOLID_BYTES.len(), 16);
}
