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

use parley::{
    FontContext, FontFamilyName, LayoutContext,
    fontique::{Blob, Collection, CollectionOptions, GenericFamily, SourceCache},
};
use sha2::{Digest, Sha256};
use swash::scale::{ScaleContext, image::Image as SwashImage};

use crate::UiError;
pub use layout::{TextAffinity, TextCursor, UnicodeCluster, UnicodeLayout, VisualMotion};
use paint::GlyphKey;
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
            maximum_cached_layouts: 256,
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

/// A verified pack font as registered with the layout engine.
pub(crate) struct RegisteredFont {
    /// Identity of the exact byte blob Parley reports for shaped runs.
    blob: u64,
    /// Face index within the font bytes.
    pub(crate) index: u32,
}

/// A renderer session's text engine. Mutability is confined to shaping scratch
/// and bounded layout caches; returned layouts and textures remain immutable.
pub struct TextSystem {
    fonts: FontContext,
    /// The collection exactly as registered, before any query cache grew.
    registered: Collection,
    layout_context: LayoutContext,
    pub(crate) scale: ScaleContext,
    /// Ordered pack families; every layout queries this complete stack.
    families: Arc<[FontFamilyName<'static>]>,
    pub(crate) faces: Vec<RegisteredFont>,
    pack: FontPack,
    limits: TextLimits,
    layouts: VecDeque<UnicodeLayout>,
    layout_bytes: usize,
    raster: HashMap<GlyphKey, Arc<SwashImage>>,
    raster_order: VecDeque<GlyphKey>,
    raster_bytes: usize,
    last_atlas: Option<UnicodeTexture>,
    next_revision: u64,
    shape_misses: u16,
}

impl TextSystem {
    /// Creates the text engine exclusively from verified in-memory fonts.
    ///
    /// The font collection is created with system fonts disabled and only
    /// pack blobs registered, and shaped runs from any other font are
    /// rejected. No installed font can be enumerated, loaded or used as
    /// fallback, even if a consumer enables fontique's `system` feature.
    ///
    /// # Errors
    /// Rejects font parse failures, unexpected faces and missing family names.
    pub fn new(pack: FontPack, limits: TextLimits) -> Result<Self, TextError> {
        // Revalidate tighter construction limits even if the pack was built
        // with different caller budgets.
        let FontPack { fonts, .. } = pack;
        let pack = FontPack::new(fonts.to_vec(), limits)?;
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        let mut faces = Vec::with_capacity(pack.fonts.len());
        let mut primary = None;
        for font in pack.fonts.iter() {
            let blob = Blob::new(Arc::new(font.bytes.clone()));
            let id = blob.id();
            let registered = collection.register_fonts(blob, None);
            // Exactly one face, published under the declared family name.
            let [(family, infos)] = registered.as_slice() else {
                return Err(TextError::InvalidFont);
            };
            let [info] = infos.as_slice() else {
                return Err(TextError::InvalidFont);
            };
            if collection.family_name(*family) != Some(font.family.as_str()) {
                return Err(TextError::InvalidFont);
            }
            primary.get_or_insert(*family);
            faces.push(RegisteredFont {
                blob: id,
                index: info.index(),
            });
        }
        let primary = primary.ok_or(TextError::InvalidFont)?;
        // Generic roles resolve only to the primary pack family.
        for generic in [
            GenericFamily::SansSerif,
            GenericFamily::Serif,
            GenericFamily::Monospace,
        ] {
            collection.set_generic_families(generic, std::iter::once(primary));
        }
        let families = pack
            .fonts
            .iter()
            .map(|font| FontFamilyName::Named(font.family.clone().into()))
            .collect();
        Ok(Self {
            fonts: FontContext {
                collection: collection.clone(),
                source_cache: SourceCache::default(),
            },
            registered: collection,
            layout_context: LayoutContext::new(),
            scale: ScaleContext::new(),
            families,
            faces,
            pack,
            limits,
            layouts: VecDeque::new(),
            layout_bytes: 0,
            raster: HashMap::new(),
            raster_order: VecDeque::new(),
            raster_bytes: 0,
            last_atlas: None,
            next_revision: 1,
            shape_misses: 0,
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
        self.raster.clear();
        self.raster_order.clear();
        self.raster_bytes = 0;
        self.last_atlas = None;
        self.reset_shaping();
    }

    fn reset_shaping(&mut self) {
        // Layouts own their glyph data and never refer to engine state, so
        // callers' layouts stay valid. Restoring the registered collection
        // discards all library-private query, shaping and scaling caches and
        // their retained high-water marks.
        self.fonts = FontContext {
            collection: self.registered.clone(),
            source_cache: SourceCache::default(),
        };
        self.layout_context = LayoutContext::new();
        self.scale = ScaleContext::new();
        self.shape_misses = 0;
    }

    /// The pack index of a shaped run's font; only registered pack bytes match.
    fn pack_font(&self, blob: u64) -> Result<u16, TextError> {
        self.faces
            .iter()
            .position(|face| face.blob == blob)
            .and_then(|index| u16::try_from(index).ok())
            .ok_or(TextError::InvalidFont)
    }

    #[must_use]
    pub fn cached_layouts(&self) -> usize {
        self.layouts.len()
    }
}

#[cfg(test)]
mod baseline;
#[cfg(test)]
mod tests;
