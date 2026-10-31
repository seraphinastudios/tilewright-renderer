// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! Explicit immutable common-Latin Roboto Mono candidate assets. The font data
//! retain OFL-1.1; see the packaged assets/manifest.json and assets/OFL.txt.
//! No font discovery, rasterization or ambient fallback occurs at runtime.

use crate::{BitmapFont, FontId, UiError, UiTexture};
use tilewright_scene::Digest256;

mod generated {
    include!("../assets/glyphs.rs");
}

pub use generated::{ATLAS_SHA256, ATLAS_SIZE, FALLBACK, GLYPHS, METRICS, SOLID_SHA256};

pub const FONT_ID: FontId = FontId(1);
pub const ATLAS_BYTES: &[u8] = include_bytes!("../assets/atlas.atx");
pub const SOLID_BYTES: &[u8] = include_bytes!("../assets/solid.atx");
/// Font copyright and OFL notice for consumer redistribution.
pub const BUILTIN_FONT_LICENSE: &str = include_str!("../assets/OFL.txt");

/// Binds the immutable built-in metrics to a caller-reserved resource ID and
/// revision. The resource provider must supply `ATLAS_BYTES` at this identity.
///
/// # Errors
/// Rejects a digest other than the exact atlas digest, or an invalid identity.
pub fn builtin_font(texture: UiTexture) -> Result<BitmapFont<'static>, UiError> {
    if texture.digest != Digest256(ATLAS_SHA256) {
        return Err(UiError::InvalidFont);
    }
    BitmapFont::new(FONT_ID, texture, ATLAS_SIZE, METRICS, GLYPHS, FALLBACK)
}
