// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

use super::*;

fn legacy_pack() -> FontPack {
    let bytes: Arc<[u8]> =
        Arc::from(include_bytes!("../../assets/RobotoMono-Regular.ttf").as_slice());
    let sha256 = Sha256::digest(&bytes).into();
    FontPack::new(
        vec![FontBlob {
            family: "Roboto Mono".into(),
            bytes,
            sha256,
        }],
        TextLimits::default(),
    )
    .unwrap()
}

#[test]
fn empty_and_combining_carets_are_grapheme_safe() {
    let mut system = TextSystem::new(legacy_pack(), TextLimits::default()).unwrap();
    let empty = system.layout("", 18.0, None).unwrap();
    assert!(empty.caret(TextCursor::default()).is_some());
    let layout = system.layout("e\u{301}x", 18.0, None).unwrap();
    assert_eq!(layout.grapheme_boundaries(), [0, 3, 4]);
    assert!(
        layout
            .caret(TextCursor {
                byte: 1,
                affinity: TextAffinity::After
            })
            .is_none()
    );
    assert_eq!(layout.text(), "e\u{301}x");
}

#[test]
fn digest_and_budget_fail_closed() {
    let mut blobs = legacy_pack().fonts().to_vec();
    blobs[0].sha256 = [0; 32];
    assert!(matches!(
        FontPack::new(blobs, TextLimits::default()),
        Err(TextError::DigestMismatch)
    ));
    let mut system = TextSystem::new(
        legacy_pack(),
        TextLimits {
            maximum_text_bytes: 4,
            ..TextLimits::default()
        },
    )
    .unwrap();
    assert!(matches!(
        system.layout("hello", 18.0, None),
        Err(TextError::LimitExceeded)
    ));
    assert!(matches!(
        system.layout("x", f32::NAN, None),
        Err(TextError::InvalidValue)
    ));
}

fn explicit_pack() -> Option<FontPack> {
    // Test-only explicit fixture root. Production text code never reads env/FS.
    let root = std::env::var_os("TILEWRIGHT_UNICODE_FONT_ROOT")?;
    let root = std::path::Path::new(&root);
    let fonts = crate::unicode_fonts::FONTS
        .iter()
        .map(|spec| {
            let bytes = std::fs::read(root.join(spec.path)).unwrap();
            assert_eq!(bytes.len(), spec.bytes);
            FontBlob {
                family: spec.family.into(),
                bytes: bytes.into(),
                sha256: spec.sha256,
            }
        })
        .collect();
    Some(FontPack::new(fonts, TextLimits::default()).unwrap())
}

#[test]
fn explicit_font_pack_shapes_multilingual_original_text() {
    let Some(pack) = explicit_pack() else {
        return;
    };
    let mut system = TextSystem::new(pack, TextLimits::default()).unwrap();
    for text in [
        "office e\u{301}",
        "العربية مرحبا 123",
        "שלום abc 123",
        "नमस्ते क्षि",
        "বাংলা",
        "தமிழ்",
        "简体中文 日本語 한국어 한",
        "👩🏽‍💻 🇺🇳 👨‍👩‍👧‍👦",
    ] {
        let layout = system.layout(text, 20.0, None).unwrap();
        assert_eq!(layout.text(), text);
        assert!(
            layout.missing_glyphs().is_empty(),
            "missing in {text:?}: {:?}",
            layout.missing_glyphs()
        );
        assert!(layout.size()[0] > 0.0);
        let frame = compile_label(&mut system, text);
        assert!(!frame.quads.is_empty());
        assert!(frame.unsupported.is_empty());
        for boundary in layout.grapheme_boundaries() {
            let cursor = TextCursor {
                byte: *boundary,
                affinity: TextAffinity::After,
            };
            assert!(
                layout.caret(cursor).is_some(),
                "no caret {text:?} {boundary}"
            );
        }
        for x in 0_u16..100 {
            let cursor = layout.hit([f32::from(x), 10.0]);
            assert!(layout.grapheme_boundaries().contains(&cursor.byte));
        }
    }
}

fn compile_label(system: &mut TextSystem, text: &str) -> CompiledUnicodeUi {
    let rect = crate::UiRect {
        x: 0.0,
        y: 0.0,
        width: 512.0,
        height: 128.0,
    };
    system
        .compile(
            crate::UiViewport {
                logical_size: [512, 128],
                physical_size: [512, 128],
            },
            crate::UiDisplayList {
                primitives: &[crate::UiPrimitive::Text {
                    origin: [0.0, 0.0],
                    clip: rect,
                    paint: crate::UiPaint {
                        semantic_id: tilewright_scene::SemanticId(1),
                        painter_order: 0,
                        color: [1.0; 4],
                    },
                    font: crate::builtin::FONT_ID,
                    size: 20.0,
                    text,
                }],
            },
            crate::UiTexture {
                id: tilewright_scene::ResourceId(2),
                revision: 1,
                digest: tilewright_scene::Digest256(crate::builtin::SOLID_SHA256),
            },
            3,
            crate::UiLimits::default(),
        )
        .unwrap()
}

#[test]
fn atlas_is_immutable_deterministic_and_keeps_larger_previous_glyphs() {
    let mut system = TextSystem::new(legacy_pack(), TextLimits::default()).unwrap();
    let first = compile_label(&mut system, "W.i");
    let second = compile_label(&mut system, "W.i");
    assert_eq!(first.resources[0].identity, second.resources[0].identity);
    assert_eq!(first.resources[0].bytes, second.resources[0].bytes);
    let changed = compile_label(&mut system, "different");
    assert_ne!(
        first.resources[0].identity.digest,
        changed.resources[0].identity.digest
    );
    assert!(changed.resources[0].identity.revision > first.resources[0].identity.revision);
    let bytes = &first.resources[0].bytes;
    let width = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let height = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    for quad in first.quads {
        assert!(quad.frame.unwrap().fits(width, height));
        assert!(!quad.pickable);
    }
    assert_eq!(
        first.resources[0].identity.digest.0,
        <[u8; 32]>::from(Sha256::digest(bytes))
    );
}

#[test]
fn bidi_affinities_visual_motion_ligature_and_wrapping() {
    let Some(pack) = explicit_pack() else {
        return;
    };
    let mut system = TextSystem::new(pack, TextLimits::default()).unwrap();
    let layout = system.layout("abc אבג xyz", 20.0, None).unwrap();
    let left = layout
        .caret(TextCursor {
            byte: 4,
            affinity: TextAffinity::Before,
        })
        .unwrap();
    let right = layout
        .caret(TextCursor {
            byte: 4,
            affinity: TextAffinity::After,
        })
        .unwrap();
    assert!((left.x - right.x).abs() > 1.0);
    let start = TextCursor {
        byte: 0,
        affinity: TextAffinity::After,
    };
    let mut cursor = start;
    for _ in 0..30 {
        let next = layout.move_visual(cursor, VisualMotion::Right);
        assert!(layout.caret(next).unwrap().x >= layout.caret(cursor).unwrap().x);
        if next == cursor {
            break;
        }
        cursor = next;
    }
    assert_eq!(layout.move_visual(cursor, VisualMotion::Home).byte, 0);
    let segments = layout.selection_rects(
        TextCursor {
            byte: 2,
            affinity: TextAffinity::After,
        },
        TextCursor {
            byte: 6,
            affinity: TextAffinity::Before,
        },
    );
    assert!(
        segments.len() >= 2,
        "mixed selection must stay disconnected: {segments:?}"
    );
    let ligature = system.layout("ffi", 20.0, None).unwrap();
    let a = ligature
        .caret(TextCursor {
            byte: 1,
            affinity: TextAffinity::After,
        })
        .unwrap();
    let b = ligature
        .caret(TextCursor {
            byte: 2,
            affinity: TextAffinity::After,
        })
        .unwrap();
    assert!(a.x < b.x);
    let wrapped = system
        .layout("one two three\r\nfour", 20.0, Some(50.0))
        .unwrap();
    assert_eq!(wrapped.text(), "one two three\r\nfour");
    assert!(wrapped.size()[1] > 30.0);
}

#[test]
fn cache_eviction_and_shaping_reset_preserve_retained_layouts() {
    let mut system = TextSystem::new(
        legacy_pack(),
        TextLimits {
            maximum_cached_layouts: 2,
            ..TextLimits::default()
        },
    )
    .unwrap();
    let old = system.layout("retained ffi", 18.0, None).unwrap();
    let before = old.caret(TextCursor::default());
    for index in 0..260 {
        system
            .layout(&format!("label {index}"), 18.0, None)
            .unwrap();
    }
    assert_eq!(system.cached_layouts(), 2);
    assert_eq!(before, old.caret(TextCursor::default()));
    assert_eq!(old.text(), "retained ffi");
    let frame = compile_label(&mut system, "W.i");
    system.clear_cache();
    assert_eq!(system.cached_layouts(), 0);
    let after = compile_label(&mut system, "W.i");
    assert_eq!(frame.resources[0].bytes, after.resources[0].bytes);
    assert!(after.resources[0].identity.revision > frame.resources[0].identity.revision);
}

#[test]
fn unknown_roles_limits_and_missing_scalars_are_explicit() {
    let mut system = TextSystem::new(legacy_pack(), TextLimits::default()).unwrap();
    let layout = system.layout("a\u{10ffff}b", 20.0, None).unwrap();
    assert_eq!(layout.missing_glyphs(), &[1..5]);
    assert_eq!(layout.text(), "a\u{10ffff}b");
    let viewport = crate::UiViewport {
        logical_size: [128, 128],
        physical_size: [128, 128],
    };
    let solid = crate::UiTexture {
        id: tilewright_scene::ResourceId(2),
        revision: 1,
        digest: tilewright_scene::Digest256(crate::builtin::SOLID_SHA256),
    };
    let rect = crate::UiRect {
        x: 0.0,
        y: 0.0,
        width: 128.0,
        height: 128.0,
    };
    let mut primitive = crate::UiPrimitive::Text {
        origin: [0.0, 0.0],
        clip: rect,
        paint: crate::UiPaint {
            semantic_id: tilewright_scene::SemanticId(1),
            painter_order: 0,
            color: [1.0; 4],
        },
        font: crate::FontId(2),
        size: 20.0,
        text: "test",
    };
    assert!(matches!(
        system.compile(
            viewport,
            crate::UiDisplayList {
                primitives: &[primitive]
            },
            solid,
            3,
            crate::UiLimits::default()
        ),
        Err(TextError::Ui(UiError::UnknownFont))
    ));
    if let crate::UiPrimitive::Text { font, .. } = &mut primitive {
        *font = crate::builtin::FONT_ID;
    }
    assert!(matches!(
        system.compile(
            viewport,
            crate::UiDisplayList {
                primitives: &[primitive]
            },
            solid,
            3,
            crate::UiLimits {
                maximum_quads: 0,
                ..crate::UiLimits::default()
            }
        ),
        Err(TextError::LimitExceeded)
    ));
    assert!(matches!(
        system.compile(
            viewport,
            crate::UiDisplayList {
                primitives: &[primitive]
            },
            solid,
            solid.id.0,
            crate::UiLimits::default()
        ),
        Err(TextError::InvalidValue)
    ));
}

#[test]
fn primitive_order_survives_equal_painter_order_and_clipping() {
    let mut system = TextSystem::new(legacy_pack(), TextLimits::default()).unwrap();
    let rect = crate::UiRect {
        x: 0.0,
        y: 0.0,
        width: 128.0,
        height: 128.0,
    };
    let paint = crate::UiPaint {
        semantic_id: tilewright_scene::SemanticId(1),
        painter_order: 0,
        color: [1.0; 4],
    };
    let primitives = [
        crate::UiPrimitive::Text {
            origin: [0.0, 0.0],
            clip: rect,
            paint,
            font: crate::builtin::FONT_ID,
            size: 20.0,
            text: "a",
        },
        crate::UiPrimitive::Solid {
            bounds: rect,
            clip: rect,
            paint,
            pickable: true,
        },
    ];
    let output = system
        .compile(
            crate::UiViewport {
                logical_size: [128, 128],
                physical_size: [128, 128],
            },
            crate::UiDisplayList {
                primitives: &primitives,
            },
            crate::UiTexture {
                id: tilewright_scene::ResourceId(2),
                revision: 1,
                digest: tilewright_scene::Digest256(crate::builtin::SOLID_SHA256),
            },
            3,
            crate::UiLimits::default(),
        )
        .unwrap();
    assert_eq!(output.quads.len(), 2);
    assert!(!output.quads[0].pickable);
    assert!(output.quads[1].pickable);
    assert_eq!(output.quads[0].part, 0);
    assert_eq!(output.quads[1].part, 1);
}

/// Run with --release --nocapture and the explicit test font root for evidence.
/// Timings are measured output, not machine-dependent pass/fail thresholds.
#[test]
fn performance_international_ui() {
    let Some(pack) = explicit_pack() else {
        return;
    };
    let start = std::time::Instant::now();
    let mut system = TextSystem::new(pack, TextLimits::default()).unwrap();
    let initialization = start.elapsed();
    let mut labels: Vec<_> = (0..100)
        .map(|index| format!("Panel {index}: العربية שלום नमस्ते 日本語 한국어 👩🏽‍💻 e\u{301}"))
        .collect();
    let sample = "العربية שלום नमस्ते 日本語 한국어 👩🏽‍💻 e\u{301} ";
    let mut draft = String::new();
    while draft.len() + sample.len() <= 4096 {
        draft.push_str(sample);
    }
    draft.extend(std::iter::repeat_n('x', 4096 - draft.len()));
    labels.push(draft);
    let clip = crate::UiRect {
        x: 0.0,
        y: 0.0,
        width: 2048.0,
        height: 2048.0,
    };
    let primitives: Vec<_> = labels
        .iter()
        .enumerate()
        .map(|(index, text)| crate::UiPrimitive::Text {
            origin: [0.0, f32::from(u16::try_from(index).unwrap()) * 19.0],
            clip,
            paint: crate::UiPaint {
                semantic_id: tilewright_scene::SemanticId(u64::try_from(index + 1).unwrap()),
                painter_order: u32::try_from(index).unwrap(),
                color: [1.0; 4],
            },
            font: crate::builtin::FONT_ID,
            size: 14.0,
            text,
        })
        .collect();
    let viewport = crate::UiViewport {
        logical_size: [2048, 2048],
        physical_size: [2048, 2048],
    };
    let solid = crate::UiTexture {
        id: tilewright_scene::ResourceId(2),
        revision: 1,
        digest: tilewright_scene::Digest256(crate::builtin::SOLID_SHA256),
    };
    let start = std::time::Instant::now();
    let cold = system
        .compile(
            viewport,
            crate::UiDisplayList {
                primitives: &primitives,
            },
            solid,
            3,
            crate::UiLimits::default(),
        )
        .unwrap();
    let cold_elapsed = start.elapsed();
    assert!(cold.unsupported.is_empty());
    let mut warm = Vec::new();
    for _ in 0..5 {
        let start = std::time::Instant::now();
        let frame = system
            .compile(
                viewport,
                crate::UiDisplayList {
                    primitives: &primitives,
                },
                solid,
                3,
                crate::UiLimits::default(),
            )
            .unwrap();
        warm.push(start.elapsed().as_micros());
        assert_eq!(frame.resources[0].identity, cold.resources[0].identity);
        assert_eq!(frame.resources[0].bytes, cold.resources[0].bytes);
    }
    warm.sort_unstable();
    assert_eq!(system.cached_layouts(), 101);
    assert!(system.raster.len() <= 4096);
    assert!(system.raster_bytes <= 16 * 1024 * 1024);
    let font = crate::builtin::builtin_font(crate::UiTexture {
        id: tilewright_scene::ResourceId(1),
        revision: 1,
        digest: tilewright_scene::Digest256(crate::builtin::ATLAS_SHA256),
    })
    .unwrap();
    let values = [font];
    let fonts = crate::BitmapFontSet::new(&values).unwrap();
    let start = std::time::Instant::now();
    let legacy = crate::compile_ui(
        viewport,
        crate::UiDisplayList {
            primitives: &primitives,
        },
        &fonts,
        solid,
        crate::UiLimits::default(),
    )
    .unwrap();
    let legacy_elapsed = start.elapsed();
    println!(
        "{{\"case\":\"100 international labels + 4096-byte draft\",\"font_bytes\":{},\"initialization_us\":{},\"cold_shape_raster_pack_us\":{},\"warm_median_us\":{},\"warm_min_us\":{},\"warm_max_us\":{},\"legacy_unshaped_us\":{},\"legacy_unsupported_scalars\":{},\"atlas_bytes\":{},\"raster_cache_bytes\":{},\"raster_cache_entries\":{},\"layout_cache_entries\":{},\"layout_retention_charge\":{},\"quads\":{}}}",
        crate::unicode_fonts::TOTAL_FONT_BYTES,
        initialization.as_micros(),
        cold_elapsed.as_micros(),
        warm[2],
        warm[0],
        warm[4],
        legacy_elapsed.as_micros(),
        legacy.unsupported.len(),
        cold.resources[0].bytes.len(),
        system.raster_bytes,
        system.raster.len(),
        system.cached_layouts(),
        system.layout_bytes,
        cold.quads.len()
    );
}
