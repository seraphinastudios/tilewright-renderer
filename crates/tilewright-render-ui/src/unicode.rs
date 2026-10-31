// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! Shared Unicode shaping, geometry and immutable glyph resources.
//!
//! Fonts are explicit verified bytes. This module does not discover fonts,
//! inspect environment variables, open files, or access a network.

mod layout;
mod paint;

use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::Arc,
};

use cosmic_text::{CacheKey, Fallback, FontSystem, SwashCache, SwashImage, fontdb};
use sha2::{Digest, Sha256};
use unicode_script::Script;

use crate::UiError;
pub use layout::{TextAffinity, TextCursor, UnicodeCluster, UnicodeLayout, VisualMotion};
pub use paint::{CompiledUnicodeUi, UnicodeTexture};

/// Explicit immutable font input. `sha256` identifies the original font bytes.
#[derive(Clone, Debug)]
pub struct FontBlob {
    pub family: String,
    pub bytes: Arc<[u8]>,
    pub sha256: [u8; 32],
}

/// Caller budgets may tighten, but cannot enlarge, the hard limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextLimits {
    pub maximum_fonts: usize,
    pub maximum_font_bytes: usize,
    pub maximum_text_bytes: usize,
    pub maximum_glyphs: usize,
    pub maximum_lines: usize,
    pub maximum_cached_layouts: usize,
    pub maximum_layout_cache_bytes: usize,
    pub maximum_raster_pixels: usize,
}

impl Default for TextLimits {
    fn default() -> Self {
        Self {
            maximum_fonts: 32,
            maximum_font_bytes: 64 * 1024 * 1024,
            maximum_text_bytes: 65_536,
            maximum_glyphs: 65_536,
            maximum_lines: 4096,
            maximum_cached_layouts: 64,
            maximum_layout_cache_bytes: 8 * 1024 * 1024,
            maximum_raster_pixels: 16 * 1024 * 1024,
        }
    }
}

/// Verified ordered font bytes. Large international packs belong in explicit
/// application assets, rather than being embedded in this registry package.
#[derive(Clone, Debug)]
pub struct FontPack {
    fonts: Arc<[FontBlob]>,
    digest: [u8; 32],
}

impl FontPack {
    /// Checks budgets and each immutable byte digest before parsing any font.
    ///
    /// # Errors
    /// Rejects empty, oversized, duplicated or incorrectly identified inputs.
    pub fn new(fonts: Vec<FontBlob>, limits: TextLimits) -> Result<Self, TextError> {
        if fonts.is_empty() || fonts.len() > limits.maximum_fonts.min(32) {
            return Err(TextError::LimitExceeded);
        }
        let mut bytes = 0_usize;
        let mut digest = Sha256::new();
        for (index, font) in fonts.iter().enumerate() {
            bytes = bytes
                .checked_add(font.bytes.len())
                .ok_or(TextError::LimitExceeded)?;
            if font.bytes.is_empty()
                || font.bytes.len() > 32 * 1024 * 1024
                || bytes > limits.maximum_font_bytes.min(64 * 1024 * 1024)
                || font.family.is_empty()
                || font.family.len() > 128
                || font.family.chars().any(char::is_control)
                || fonts[..index].iter().any(|old| old.family == font.family)
            {
                return Err(TextError::InvalidFont);
            }
            let actual: [u8; 32] = Sha256::digest(&font.bytes).into();
            if actual != font.sha256 {
                return Err(TextError::DigestMismatch);
            }
            digest.update((font.family.len() as u64).to_le_bytes());
            digest.update(font.family.as_bytes());
            digest.update(font.sha256);
        }
        Ok(Self {
            fonts: fonts.into(),
            digest: digest.finalize().into(),
        })
    }

    #[must_use]
    pub const fn digest(&self) -> [u8; 32] {
        self.digest
    }

    #[must_use]
    pub fn fonts(&self) -> &[FontBlob] {
        &self.fonts
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextError {
    InvalidFont,
    DigestMismatch,
    InvalidValue,
    LimitExceeded,
    Rasterization,
    Ui(UiError),
}
impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Unicode text error: {self:?}")
    }
}
impl std::error::Error for TextError {}
impl From<UiError> for TextError {
    fn from(value: UiError) -> Self {
        Self::Ui(value)
    }
}

// Empty explicit lists disable all platform-specific fallback preferences.
// The remaining fontdb candidates are deterministically ranked by fixed style,
// weight and database IDs, assigned in verified FontPack order.
struct ExplicitFallback;
impl Fallback for ExplicitFallback {
    fn common_fallback(&self) -> &[&'static str] {
        &[]
    }
    fn forbidden_fallback(&self) -> &[&'static str] {
        &[]
    }
    fn script_fallback(&self, _: Script, _: &str) -> &[&'static str] {
        &[]
    }
}

/// A renderer session's text engine. Mutability is confined to shaping scratch
/// and bounded layout caches; returned layouts and textures remain immutable.
pub struct TextSystem {
    pub(crate) fonts: FontSystem,
    pub(crate) swash: SwashCache,
    pack: FontPack,
    limits: TextLimits,
    layouts: VecDeque<UnicodeLayout>,
    layout_bytes: usize,
    raster: HashMap<CacheKey, Arc<SwashImage>>,
    raster_order: VecDeque<CacheKey>,
    raster_bytes: usize,
    last_atlas: Option<UnicodeTexture>,
    next_revision: u64,
}

impl TextSystem {
    /// Creates the text engine exclusively from verified in-memory fonts.
    ///
    /// # Errors
    /// Rejects font parse failures, unexpected faces and missing family names.
    pub fn new(pack: FontPack, limits: TextLimits) -> Result<Self, TextError> {
        // Revalidate tighter construction limits even if the pack was built
        // with different caller budgets.
        let pack = FontPack::new(pack.fonts.to_vec(), limits)?;
        let mut db = fontdb::Database::new();
        for font in pack.fonts.iter() {
            let before = db.faces().count();
            db.load_font_source(fontdb::Source::Binary(Arc::new(font.bytes.clone())));
            if db.faces().count() != before + 1 {
                return Err(TextError::InvalidFont);
            }
            if !db
                .faces()
                .skip(before)
                .any(|face| face.families.iter().any(|(name, _)| name == &font.family))
            {
                return Err(TextError::InvalidFont);
            }
        }
        let primary = &pack.fonts[0].family;
        db.set_sans_serif_family(primary);
        db.set_serif_family(primary);
        db.set_monospace_family(primary);
        let fonts =
            FontSystem::new_with_locale_and_db_and_fallback("en-US".into(), db, ExplicitFallback);
        Ok(Self {
            fonts,
            swash: SwashCache::new(),
            pack,
            limits,
            layouts: VecDeque::new(),
            layout_bytes: 0,
            raster: HashMap::new(),
            raster_order: VecDeque::new(),
            raster_bytes: 0,
            last_atlas: None,
            next_revision: 1,
        })
    }

    #[must_use]
    pub const fn font_pack(&self) -> &FontPack {
        &self.pack
    }

    /// Clears retained layouts and raster scratch, without affecting immutable
    /// layouts/resources held by a consumer or in-flight frame.
    pub fn clear_cache(&mut self) {
        self.layouts.clear();
        self.layout_bytes = 0;
        self.swash = SwashCache::new();
        self.raster.clear();
        self.raster_order.clear();
        self.raster_bytes = 0;
        self.last_atlas = None;
    }

    #[must_use]
    pub fn cached_layouts(&self) -> usize {
        self.layouts.len()
    }
}

#[cfg(test)]
mod tests;
