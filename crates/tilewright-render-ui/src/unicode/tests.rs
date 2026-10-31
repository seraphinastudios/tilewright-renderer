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
