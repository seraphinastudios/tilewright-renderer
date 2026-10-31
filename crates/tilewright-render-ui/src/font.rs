// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

use tilewright_scene::TextureRect;

use crate::{UiError, UiTexture};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FontId(pub u32);

/// All distances are native atlas pixels. Descent is a positive distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    pub native_size: f32,
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
}

/// Top-left bearing relative to the baseline; a space has no frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BitmapGlyph {
    pub character: char,
    pub frame: Option<TextureRect>,
    pub bearing: [f32; 2],
    pub advance: f32,
}

/// Validated borrowed immutable metrics. Atlas bytes are supplied independently
/// through the renderer's digest-verified resource provider.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BitmapFont<'a> {
    id: FontId,
    texture: UiTexture,
    atlas_size: [u32; 2],
    metrics: FontMetrics,
    glyphs: &'a [BitmapGlyph],
    fallback: char,
}

impl<'a> BitmapFont<'a> {
    /// Glyphs must be strictly scalar-sorted; the fallback must have visible ink.
    ///
    /// # Errors
    /// Rejects invalid identities, metrics, frames, duplicate scalars or an
    /// absent/invisible fallback. No glyph data is copied or allocated.
    pub fn new(
        id: FontId,
        texture: UiTexture,
        atlas_size: [u32; 2],
        metrics: FontMetrics,
        glyphs: &'a [BitmapGlyph],
        fallback: char,
    ) -> Result<Self, UiError> {
        if id.0 == 0
            || !texture.valid()
            || atlas_size.contains(&0)
            || atlas_size.iter().any(|side| *side > 8192)
            || u64::from(atlas_size[0]) * u64::from(atlas_size[1]) > 16_777_216
            || glyphs.is_empty()
            || glyphs.len() > 65_536
            || !metrics.native_size.is_finite()
            || !(0.0..=1024.0).contains(&metrics.native_size)
            || metrics.native_size == 0.0
            || [metrics.ascent, metrics.descent, metrics.line_gap]
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=4096.0).contains(v))
            || metrics.ascent + metrics.descent <= 0.0
            || glyphs
                .windows(2)
                .any(|pair| pair[0].character >= pair[1].character)
            || glyphs.iter().any(|glyph| {
                !glyph.advance.is_finite()
                    || !(0.0..=4096.0).contains(&glyph.advance)
                    || glyph
                        .bearing
                        .iter()
                        .any(|v| !v.is_finite() || v.abs() > 4096.0)
                    || glyph
                        .frame
                        .is_some_and(|frame| !frame.fits(atlas_size[0], atlas_size[1]))
                    || (glyph.frame.is_none()
                        && glyph.character != ' '
                        && glyph.character != '\u{a0}')
            })
        {
            return Err(UiError::InvalidFont);
        }
        let font = Self {
            id,
            texture,
            atlas_size,
            metrics,
            glyphs,
            fallback,
        };
        if font
            .glyph(fallback)
            .is_none_or(|glyph| glyph.frame.is_none())
        {
            return Err(UiError::InvalidFont);
        }
        if font
            .glyph(' ')
            .is_none_or(|glyph| glyph.frame.is_some() || glyph.advance <= 0.0)
            || font
                .glyph('\u{a0}')
                .is_some_and(|glyph| glyph.frame.is_some())
        {
            return Err(UiError::InvalidFont);
        }
        Ok(font)
    }

    #[must_use]
    pub const fn id(&self) -> FontId {
        self.id
    }
    #[must_use]
    pub const fn texture(&self) -> UiTexture {
        self.texture
    }
    #[must_use]
    pub const fn atlas_size(&self) -> [u32; 2] {
        self.atlas_size
    }
    #[must_use]
    pub const fn metrics(&self) -> FontMetrics {
        self.metrics
    }
    #[must_use]
    pub const fn glyphs(&self) -> &'a [BitmapGlyph] {
        self.glyphs
    }
    #[must_use]
    pub const fn fallback(&self) -> char {
        self.fallback
    }

    #[must_use]
    pub fn glyph(&self, character: char) -> Option<&'a BitmapGlyph> {
        self.glyphs
            .binary_search_by_key(&character, |glyph| glyph.character)
            .ok()
            .map(|index| &self.glyphs[index])
    }

    /// Whether every original scalar can be displayed without substitution or
    /// unsupported shaping. Checks the whole string without normalization.
    #[must_use]
    pub fn supports_text(&self, text: &str) -> bool {
        text.chars().all(|character| {
            character == '\n'
                || (crate::compile::simple_scalar(character) && self.glyph(character).is_some())
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BitmapFontSet<'a> {
    fonts: &'a [BitmapFont<'a>],
}

impl<'a> BitmapFontSet<'a> {
    /// # Errors
    /// Rejects duplicate font IDs or resource IDs with conflicting identities,
    /// and sets larger than 32 fonts. Does not allocate.
    pub fn new(fonts: &'a [BitmapFont<'a>]) -> Result<Self, UiError> {
        if fonts.len() > 32 {
            return Err(UiError::LimitExceeded);
        }
        for (index, font) in fonts.iter().enumerate() {
            for previous in &fonts[..index] {
                if previous.id == font.id {
                    return Err(UiError::InvalidFont);
                }
                if previous.texture.id == font.texture.id && previous.texture != font.texture {
                    return Err(UiError::ResourceCollision);
                }
                if previous.texture == font.texture && previous.atlas_size != font.atlas_size {
                    return Err(UiError::InvalidFont);
                }
            }
        }
        Ok(Self { fonts })
    }

    #[must_use]
    pub fn get(&self, id: FontId) -> Option<&BitmapFont<'a>> {
        self.fonts.iter().find(|font| font.id == id)
    }

    pub(crate) fn check_solid(&self, solid: UiTexture) -> Result<(), UiError> {
        if self.fonts.iter().any(|font| font.texture.id == solid.id) {
            // A solid texture is an independent white 1x1 resource, never the
            // whole font atlas, even if the supplied digest happens to match.
            return Err(UiError::ResourceCollision);
        }
        Ok(())
    }
}
