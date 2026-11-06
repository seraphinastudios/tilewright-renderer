// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! Pinned observable Unicode output over a fixed multilingual corpus.
//!
//! The snapshot records only public results: cluster/byte mapping and glyph
//! advance rectangles, line placement, caret, hit, visual motion, selection,
//! compiled quad geometry and atlas bytes (digest). Floats are printed
//! exactly. Regenerate deliberately with `TILEWRIGHT_BLESS_UNICODE_BASELINE=1`
//! and review the diff; any change is a rendering behavior change.

use std::{fmt::Write as _, panic::AssertUnwindSafe};

use super::tests::explicit_pack;
use super::*;

const SNAPSHOT: &str = "src/unicode/baseline.txt";

/// `(name, text, size, wrap width)`.
const CORPUS: &[(&str, &str, f32, Option<f32>)] = &[
    ("empty", "", 20.0, None),
    ("latin-ligatures", "office ffi fl Wavy", 20.0, None),
    ("kerning-pairs", "AVATAR Toyota WAVE Yo", 20.0, None),
    ("kerning-hyphen-wrap", "T-V-A AV-To LT-VA", 20.0, Some(40.0)),
    ("combining", "e\u{301}x", 20.0, None),
    ("trailing-spaces", "ab  ", 20.0, None),
    ("hebrew", "שלום עולם", 20.0, None),
    ("arabic", "العربية مرحبا 123", 20.0, None),
    ("mixed-ltr-base", "abc אבג xyz", 20.0, None),
    ("mixed-rtl-base", "שלום abc 123", 20.0, None),
    ("rtl-override-ligature", "\u{202e}ffi\u{202c}", 32.0, None),
    ("devanagari", "नमस्ते क्षि", 20.0, None),
    ("cjk", "简体中文 日本語 한국어", 20.0, None),
    ("emoji", "👩🏽‍💻 🇺🇳 👨‍👩‍👧‍👦 a", 20.0, None),
    ("missing-scalar", "a\u{10ffff}b", 20.0, None),
    ("tabs", "a\tb\tcd\te", 20.0, None),
    ("tabs-mixed-direction", "x \u{5d0}\t1", 16.0, None),
    ("tabs-rtl-columns", "\u{5d0} ab\tcd", 16.0, None),
    ("tabs-wrap", "ab\tcd\tef gh\tij", 20.0, Some(90.0)),
    ("hard-newlines", "a\nb\r\nc", 20.0, None),
    ("trailing-newline", "ab\n", 20.0, None),
    ("vertical-controls", "a\u{b}b\u{c}c\u{85}d", 20.0, None),
    ("line-separator", "ab\u{2028}cd", 20.0, None),
    ("paragraph-separator", "ab\u{2029}cd", 20.0, None),
    ("mixed-paragraph-separator", "a\u{2029}\u{5d0}", 20.0, None),
    (
        "rtl-paragraph-then-ltr",
        "\u{5d0}\u{5d1}\n\u{5d2} ab",
        20.0,
        None,
    ),
    (
        "wrap-words",
        "one two three four five six",
        20.0,
        Some(60.0),
    ),
    (
        "wrap-glyphs",
        "abcdefghijklmnopqrstuvwxyz",
        20.0,
        Some(40.0),
    ),
    ("wrap-mixed", "abc אבג דהו xyz", 20.0, Some(50.0)),
    ("wrap-rtl", "שלום עולם טוב", 20.0, Some(50.0)),
    ("wrap-crlf", "one two three\r\nfour", 20.0, Some(50.0)),
];

fn f(value: f32) -> String {
    format!("{value:?}")
}

fn rect(r: crate::UiRect) -> String {
    format!("[{} {} {} {}]", f(r.x), f(r.y), f(r.width), f(r.height))
}

fn cursor(c: TextCursor) -> String {
    format!(
        "{}{}",
        c.byte,
        match c.affinity {
            TextAffinity::Before => "b",
            TextAffinity::After => "a",
        }
    )
}

fn record_layout(out: &mut String, layout: &UnicodeLayout) {
    let [w, h] = layout.size();
    writeln!(out, "size {} {}", f(w), f(h)).unwrap();
    writeln!(out, "boundaries {:?}", layout.grapheme_boundaries()).unwrap();
    writeln!(out, "missing {:?}", layout.missing_glyphs()).unwrap();
    for cluster in layout.clusters() {
        writeln!(
            out,
            "glyph {:?} {} {}{}",
            cluster.bytes,
            rect(cluster.bounds),
            if cluster.right_to_left { "rtl" } else { "ltr" },
            if cluster.missing { " missing" } else { "" }
        )
        .unwrap();
    }
    let mut lines = Vec::<(u32, f32, f32)>::new();
    for &byte in layout.grapheme_boundaries() {
        for affinity in [TextAffinity::Before, TextAffinity::After] {
            let c = TextCursor { byte, affinity };
            match layout.caret(c) {
                Some(r) => {
                    writeln!(out, "caret {} {}", cursor(c), rect(r)).unwrap();
                    if !lines.iter().any(|line| line.0 == r.y.to_bits()) {
                        lines.push((r.y.to_bits(), r.y, r.height));
                    }
                }
                None => writeln!(out, "caret {} none", cursor(c)).unwrap(),
            }
        }
    }
    lines.sort_by(|a, b| a.1.total_cmp(&b.1));
    writeln!(out, "lines {}", lines.len()).unwrap();
    for (_, y, height) in &lines {
        // Record only transitions of the hit result along each visual line.
        let probe_y = y + height * 0.5;
        let mut last = None;
        let mut x = -4.0_f32;
        while x <= w + 4.0 {
            let hit = layout.hit([x, probe_y]);
            if last != Some(hit) {
                writeln!(out, "hit {} {} -> {}", f(x), f(probe_y), cursor(hit)).unwrap();
                last = Some(hit);
            }
            x += 0.5;
        }
    }
    let start = TextCursor::default();
    for motion in [VisualMotion::Right, VisualMotion::Left] {
        let mut c = if motion == VisualMotion::Right {
            start
        } else {
            layout.move_visual(start, VisualMotion::End)
        };
        let mut path = vec![cursor(c)];
        for _ in 0..64 {
            let next = layout.move_visual(c, motion);
            if next == c {
                break;
            }
            c = next;
            path.push(cursor(c));
        }
        writeln!(out, "move {motion:?} {}", path.join(" ")).unwrap();
    }
    for &byte in layout.grapheme_boundaries() {
        let c = TextCursor {
            byte,
            affinity: TextAffinity::After,
        };
        writeln!(
            out,
            "home/end {} -> {} {}",
            cursor(c),
            cursor(layout.move_visual(c, VisualMotion::Home)),
            cursor(layout.move_visual(c, VisualMotion::End))
        )
        .unwrap();
    }
    let boundaries = layout.grapheme_boundaries();
    let mut pairs: Vec<_> = boundaries.windows(2).map(|w| (w[0], w[1])).collect();
    if boundaries.len() > 2 {
        pairs.push((boundaries[0], boundaries[boundaries.len() - 1]));
        pairs.push((boundaries[1], boundaries[boundaries.len() - 2]));
    }
    for (a, b) in pairs {
        let rects = layout.selection_rects(
            TextCursor {
                byte: a,
                affinity: TextAffinity::After,
            },
            TextCursor {
                byte: b,
                affinity: TextAffinity::Before,
            },
        );
        let rects = rects.into_iter().fold(String::new(), |mut all, r| {
            write!(all, " {}", rect(r)).unwrap();
            all
        });
        writeln!(out, "select {a}..{b}:{rects}").unwrap();
    }
}

fn record_compile(out: &mut String, system: &mut TextSystem, text: &str, size: f32, scale: u32) {
    let clip = crate::UiRect {
        x: 0.0,
        y: 0.0,
        width: 1024.0,
        height: 256.0,
    };
    let result = system.compile(
        crate::UiViewport {
            logical_size: [1024, 256],
            physical_size: [1024 * scale, 256 * scale],
        },
        crate::UiDisplayList {
            primitives: &[crate::UiPrimitive::Text {
                // Fractional origin exercises subpixel raster placement.
                origin: [3.25, 5.5],
                clip,
                paint: crate::UiPaint {
                    semantic_id: tilewright_scene::SemanticId(1),
                    painter_order: 0,
                    color: [1.0; 4],
                },
                font: crate::builtin::FONT_ID,
                size,
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
    );
    let compiled = match result {
        Ok(compiled) => compiled,
        Err(error) => {
            writeln!(out, "compile x{scale} error {error:?}").unwrap();
            return;
        }
    };
    writeln!(
        out,
        "compile x{scale} unsupported {:?}",
        compiled.unsupported
    )
    .unwrap();
    for quad in &compiled.quads {
        let frame = quad
            .frame
            .map(|r| format!("{} {} {} {}", r.x, r.y, r.width, r.height));
        let positions: Vec<_> = quad
            .positions
            .iter()
            .map(|p| format!("{},{}", f(p[0]), f(p[1])))
            .collect();
        writeln!(
            out,
            "quad part {} frame {} pos {}",
            quad.part,
            frame.unwrap_or_default(),
            positions.join(" ")
        )
        .unwrap();
    }
    for resource in &compiled.resources {
        let bytes = &resource.bytes;
        let width = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let height = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let digest = resource
            .identity
            .digest
            .0
            .iter()
            .fold(String::new(), |mut hex, b| {
                write!(hex, "{b:02x}").unwrap();
                hex
            });
        writeln!(out, "atlas {width}x{height} sha256 {digest}").unwrap();
    }
}

fn record_case(
    out: &mut String,
    system: &mut TextSystem,
    name: &str,
    text: &str,
    size: f32,
    width: Option<f32>,
) -> bool {
    writeln!(
        out,
        "\n## {name} {:?} size {} width {:?}",
        text,
        f(size),
        width
    )
    .unwrap();
    match std::panic::catch_unwind(AssertUnwindSafe(|| system.layout(text, size, width))) {
        Ok(Ok(layout)) => record_layout(out, &layout),
        Ok(Err(error)) => writeln!(out, "layout error {error:?}").unwrap(),
        Err(_) => {
            // A panicking engine may hold inconsistent scratch state.
            writeln!(out, "layout PANIC").unwrap();
            return false;
        }
    }
    if width.is_none() {
        for scale in [1, 2] {
            if std::panic::catch_unwind(AssertUnwindSafe(|| {
                record_compile(out, system, text, size, scale);
            }))
            .is_err()
            {
                writeln!(out, "compile x{scale} PANIC").unwrap();
                return false;
            }
        }
    }
    true
}

#[test]
fn unicode_output_matches_pinned_baseline() {
    let mut out = String::from(
        "# Unicode text baseline. Regenerate with TILEWRIGHT_BLESS_UNICODE_BASELINE=1.\n",
    );
    let mut system = TextSystem::new(explicit_pack(), TextLimits::default()).unwrap();
    for (name, text, size, width) in CORPUS {
        if !record_case(&mut out, &mut system, name, text, *size, *width) {
            system = TextSystem::new(explicit_pack(), TextLimits::default()).unwrap();
        }
    }
    let mut legacy = TextSystem::new(super::tests::legacy_pack(), TextLimits::default()).unwrap();
    record_case(
        &mut out,
        &mut legacy,
        "legacy-roboto-mono",
        "W.i AV",
        18.0,
        None,
    );

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(SNAPSHOT);
    // Test-only explicit opt-in; production text code never reads env/FS.
    if std::env::var_os("TILEWRIGHT_BLESS_UNICODE_BASELINE").is_some() {
        std::fs::write(&path, &out).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    if expected != out {
        let line = expected
            .lines()
            .zip(out.lines())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| expected.lines().count().min(out.lines().count()));
        panic!(
            "Unicode output differs from {SNAPSHOT} at line {}:\n expected: {:?}\n   actual: {:?}\n\
             Rerun with TILEWRIGHT_BLESS_UNICODE_BASELINE=1 and review the diff.",
            line + 1,
            expected.lines().nth(line),
            out.lines().nth(line)
        );
    }
}
