// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

use std::{collections::HashMap, ops::Range, sync::Arc};

use cosmic_text::{CacheKey, Command, SwashContent, SwashImage};
use sha2::{Digest, Sha256};
use tilewright_scene::{Digest256, Quad, ResourceId, SemanticId, TextureRect};

use super::{TextError, TextSystem, UnicodeLayout};
use crate::{
    UiDisplayList, UiError, UiLimits, UiPaint, UiPrimitive, UiRect, UiTexture, UiViewport, builtin,
    compile::clipped_quad,
};

/// Immutable ATX1 atlas bytes, ready for a bounded `ResourceProvider`.
#[derive(Clone, Debug)]
pub struct UnicodeTexture {
    pub identity: UiTexture,
    pub bytes: Arc<[u8]>,
}

#[derive(Clone, Debug, Default)]
pub struct CompiledUnicodeUi {
    pub quads: Vec<Quad>,
    pub layouts: Vec<(usize, UnicodeLayout)>,
    pub resources: Vec<UnicodeTexture>,
    pub unsupported: Vec<(usize, Range<usize>)>,
}

enum PendingQuad {
    Solid(Quad),
    Image(ImageQuad),
}

struct ImageQuad {
    bounds: UiRect,
    clip: Option<UiRect>,
    paint: UiPaint,
    part: usize,
    frame: TextureRect,
}

impl TextSystem {
    /// Compiles the shared UI display list using advanced Unicode shaping.
    /// `builtin::FONT_ID` selects the explicitly supplied pack's primary role.
    /// All other font IDs are rejected. Text is not pickable.
    ///
    /// `first_resource` reserves one atlas ID, distinct from `solid` and World
    /// resources. A text engine owns that resource's monotonically increasing
    /// revisions for its lifetime. Caller must retire it before reusing the ID
    /// with another engine. Unchanged atlas bytes reuse their exact identity.
    ///
    /// # Errors
    /// Returns no partial frame on invalid input, missing role, exceeded budgets,
    /// atlas overflow or raster failure. Missing glyph coverage is diagnostic.
    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
    pub fn compile(
        &mut self,
        viewport: UiViewport,
        list: UiDisplayList<'_>,
        solid: UiTexture,
        first_resource: u64,
        limits: UiLimits,
    ) -> Result<CompiledUnicodeUi, TextError> {
        validate(viewport, list, solid, first_resource, limits)?;
        let scale = [
            viewport.physical_size[0] as f32 / viewport.logical_size[0] as f32,
            viewport.physical_size[1] as f32 / viewport.logical_size[1] as f32,
        ];
        // A single isotropic raster scale with a final viewport transform keeps
        // logical geometry invariant under nonuniform target scaling.
        let raster_scale = scale[0].max(scale[1]);
        let target = UiRect {
            x: 0.0,
            y: 0.0,
            width: viewport.logical_size[0] as f32,
            height: viewport.logical_size[1] as f32,
        };
        let mut output = CompiledUnicodeUi::default();
        let mut parts = HashMap::<SemanticId, usize>::new();
        let mut pending = Vec::<PendingQuad>::new();
        let mut has_images = false;
        let mut atlas = Atlas::new(self.limits.maximum_raster_pixels.min(16 * 1024 * 1024));
        let mut placed = HashMap::<CacheKey, TextureRect>::new();
        let mut glyph_count = 0_usize;
        for (index, primitive) in list.primitives.iter().enumerate() {
            match *primitive {
                UiPrimitive::Solid {
                    bounds,
                    clip,
                    paint,
                    pickable,
                } => {
                    let part = claim(&mut parts, paint.semantic_id, 1)?;
                    if let Some(quad) = clipped_quad(
                        bounds,
                        clip.intersect(target),
                        scale,
                        paint,
                        part,
                        solid,
                        None,
                        pickable,
                    )? {
                        pending.push(PendingQuad::Solid(quad));
                    }
                }
                UiPrimitive::Text {
                    origin,
                    clip,
                    paint,
                    size,
                    text,
                    ..
                } => {
                    if size * raster_scale > 1024.0 {
                        return Err(TextError::LimitExceeded);
                    }
                    let layout = self.layout(text, size, None)?;
                    if layout.0.buffer.layout_runs().count() > limits.maximum_lines.min(4096) {
                        return Err(TextError::LimitExceeded);
                    }
                    glyph_count = glyph_count
                        .checked_add(layout.clusters().len())
                        .ok_or(TextError::LimitExceeded)?;
                    if glyph_count > limits.maximum_glyphs.min(262_144) {
                        return Err(TextError::LimitExceeded);
                    }
                    let first_part = claim(&mut parts, paint.semantic_id, layout.clusters().len())?;
                    output.unsupported.extend(
                        layout
                            .missing_glyphs()
                            .iter()
                            .cloned()
                            .map(|range| (index, range)),
                    );
                    let mut ordinal = 0;
                    for run in layout.0.buffer.layout_runs() {
                        for glyph in run.glyphs {
                            // Offsets are physical in cosmic's API; its own
                            // x/y advances are scaled exactly once.
                            let raster_position = [
                                (glyph.x + glyph.font_size * glyph.x_offset + origin[0])
                                    * raster_scale,
                                (glyph.y - glyph.font_size * glyph.y_offset
                                    + origin[1]
                                    + run.line_y)
                                    * raster_scale,
                            ];
                            if raster_position
                                .iter()
                                .any(|value| !value.is_finite() || value.abs() > 16_777_216.0)
                            {
                                return Err(TextError::InvalidValue);
                            }
                            let physical = glyph.physical(
                                (
                                    origin[0] * raster_scale,
                                    (origin[1] + run.line_y) * raster_scale,
                                ),
                                raster_scale,
                            );
                            let image = self.raster_image(physical.cache_key)?;
                            let Some(image) = image else {
                                ordinal += 1;
                                continue;
                            };
                            let placement = image.placement;
                            let bounds = UiRect {
                                x: (physical.x as f32 + placement.left as f32) / raster_scale,
                                y: (physical.y as f32 - placement.top as f32) / raster_scale,
                                width: placement.width as f32 / raster_scale,
                                height: placement.height as f32 / raster_scale,
                            };
                            let visible = clip.intersect(target);
                            if visible.and_then(|c| bounds.intersect(c)).is_some() {
                                let frame = if let Some(frame) = placed.get(&physical.cache_key) {
                                    *frame
                                } else {
                                    let frame = atlas.insert(&image)?;
                                    placed.insert(physical.cache_key, frame);
                                    frame
                                };
                                has_images = true;
                                pending.push(PendingQuad::Image(ImageQuad {
                                    bounds,
                                    clip: visible,
                                    paint,
                                    part: first_part + ordinal,
                                    frame,
                                }));
                            }
                            ordinal += 1;
                        }
                    }
                    output.layouts.push((index, layout));
                }
            }
            if pending.len() > limits.maximum_quads.min(262_144) {
                return Err(TextError::LimitExceeded);
            }
        }
        if has_images {
            let bytes = atlas.finish()?;
            let digest = Digest256(Sha256::digest(&bytes).into());
            let id = ResourceId(first_resource);
            let texture = if let Some(previous) = self
                .last_atlas
                .as_ref()
                .filter(|p| p.identity.id == id && p.identity.digest == digest)
            {
                previous.clone()
            } else {
                let revision = self.next_revision;
                let next = revision.checked_add(1).ok_or(TextError::LimitExceeded)?;
                let texture = UnicodeTexture {
                    identity: UiTexture {
                        id,
                        revision,
                        digest,
                    },
                    bytes: bytes.into(),
                };
                self.next_revision = next;
                texture
            };
            for item in pending {
                let PendingQuad::Image(image) = item else {
                    if let PendingQuad::Solid(quad) = item {
                        output.quads.push(quad);
                    }
                    continue;
                };
                if let Some(quad) = clipped_quad(
                    image.bounds,
                    image.clip,
                    scale,
                    image.paint,
                    image.part,
                    texture.identity,
                    Some(image.frame),
                    false,
                )? {
                    output.quads.push(quad);
                }
            }
            self.last_atlas = Some(texture.clone());
            output.resources.push(texture);
        } else {
            output
                .quads
                .extend(pending.into_iter().filter_map(|item| match item {
                    PendingQuad::Solid(quad) => Some(quad),
                    PendingQuad::Image(_) => None,
                }));
        }
        Ok(output)
    }

    fn raster_image(&mut self, key: CacheKey) -> Result<Option<Arc<SwashImage>>, TextError> {
        const CACHE_BYTES: usize = 16 * 1024 * 1024;
        if let Some(image) = self.raster.get(&key) {
            return Ok(Some(image.clone()));
        }
        // Font size and glyph dimensions are constrained before retaining data.
        // Swash accepts only the verified application font pack, not arbitrary
        // network fonts. Raster scratch is reset with clear_cache.
        if f32::from_bits(key.font_size_bits) > 1024.0 {
            return Err(TextError::LimitExceeded);
        }
        let font = self
            .fonts
            .get_font(key.font_id, key.font_weight)
            .ok_or(TextError::InvalidFont)?;
        // This path deliberately supports monochrome outlines. Reject color
        // tables before Swash can decode an embedded bitmap of arbitrary size.
        for tag in [*b"CBDT", *b"CBLC", *b"sbix", *b"COLR", *b"SVG "] {
            if font.as_swash().table(u32::from_be_bytes(tag)).is_some() {
                return Err(TextError::Rasterization);
            }
        }
        let Some(outline) = self
            .swash
            .get_outline_commands_uncached(&mut self.fonts, key)
        else {
            return Ok(None);
        };
        if outline.len() > 262_144 {
            return Err(TextError::LimitExceeded);
        }
        let mut low = [f32::MAX; 2];
        let mut high = [f32::MIN; 2];
        for command in &*outline {
            let points = match command {
                Command::MoveTo(a) | Command::LineTo(a) => [Some(a), None, None],
                Command::QuadTo(a, b) => [Some(a), Some(b), None],
                Command::CurveTo(a, b, c) => [Some(a), Some(b), Some(c)],
                Command::Close => [None, None, None],
            };
            for point in points.into_iter().flatten() {
                for (axis, value) in [point.x, point.y].into_iter().enumerate() {
                    if !value.is_finite() || value.abs() > 8192.0 {
                        return Err(TextError::LimitExceeded);
                    }
                    low[axis] = low[axis].min(value);
                    high[axis] = high[axis].max(value);
                }
            }
        }
        if low[0] > 8192.0 {
            return Ok(None);
        }
        let width = (high[0] - low[0] + 4.0).ceil();
        let height = (high[1] - low[1] + 4.0).ceil();
        #[allow(clippy::cast_precision_loss)]
        let budget = self.limits.maximum_raster_pixels.min(16 * 1024 * 1024) as f32;
        if width > 2048.0 || height > 8192.0 || width * height > budget {
            return Err(TextError::LimitExceeded);
        }
        let Some(image) = self.swash.get_image_uncached(&mut self.fonts, key) else {
            return Err(TextError::Rasterization);
        };
        let pixels = (image.placement.width as usize)
            .checked_mul(image.placement.height as usize)
            .ok_or(TextError::LimitExceeded)?;
        if image.placement.width > 8190
            || image.placement.height > 8190
            || pixels > self.limits.maximum_raster_pixels.min(16 * 1024 * 1024)
        {
            return Err(TextError::LimitExceeded);
        }
        if image.content != SwashContent::Mask || image.data.len() != pixels {
            return Err(TextError::Rasterization);
        }
        if pixels == 0 {
            return Ok(None);
        }
        let image = Arc::new(image);
        if image.data.len() <= CACHE_BYTES {
            while self.raster.len() >= 4096
                || self.raster_bytes.saturating_add(image.data.len()) > CACHE_BYTES
            {
                let Some(old) = self.raster_order.pop_front() else {
                    break;
                };
                if let Some(old) = self.raster.remove(&old) {
                    self.raster_bytes -= old.data.len();
                }
            }
            self.raster_bytes += image.data.len();
            self.raster_order.push_back(key);
            self.raster.insert(key, image.clone());
        }
        Ok(Some(image))
    }
}

fn claim(
    parts: &mut HashMap<SemanticId, usize>,
    id: SemanticId,
    count: usize,
) -> Result<usize, TextError> {
    let entry = parts.entry(id).or_default();
    let start = *entry;
    *entry = start
        .checked_add(count)
        .ok_or(TextError::Ui(UiError::PartOverflow))?;
    if *entry > 65_536 {
        return Err(TextError::Ui(UiError::PartOverflow));
    }
    Ok(start)
}

fn validate(
    viewport: UiViewport,
    list: UiDisplayList<'_>,
    solid: UiTexture,
    first_resource: u64,
    limits: UiLimits,
) -> Result<(), TextError> {
    if !solid.valid()
        || first_resource == 0
        || first_resource == solid.id.0
        || viewport.logical_size.contains(&0)
        || viewport.physical_size.contains(&0)
    {
        return Err(TextError::InvalidValue);
    }
    if viewport
        .logical_size
        .iter()
        .chain(&viewport.physical_size)
        .any(|side| *side > limits.maximum_dimension.min(8192))
        || u64::from(viewport.physical_size[0]) * u64::from(viewport.physical_size[1])
            > limits.maximum_pixels.min(16_777_216)
        || list.primitives.len() > limits.maximum_primitives.min(4096)
    {
        return Err(TextError::LimitExceeded);
    }
    let mut bytes = 0_usize;
    for primitive in list.primitives {
        let (paint, clip) = match *primitive {
            UiPrimitive::Solid {
                bounds,
                clip,
                paint,
                ..
            } => {
                if !bounds.valid() {
                    return Err(TextError::InvalidValue);
                }
                (paint, clip)
            }
            UiPrimitive::Text {
                origin,
                clip,
                paint,
                font,
                size,
                text,
            } => {
                if font != builtin::FONT_ID {
                    return Err(TextError::Ui(UiError::UnknownFont));
                }
                if origin
                    .iter()
                    .any(|v| !v.is_finite() || v.abs() > 16_777_216.0)
                    || !size.is_finite()
                    || !(1.0..=256.0).contains(&size)
                {
                    return Err(TextError::InvalidValue);
                }
                bytes = bytes
                    .checked_add(text.len())
                    .ok_or(TextError::LimitExceeded)?;
                if bytes > limits.maximum_text_bytes.min(1_048_576) {
                    return Err(TextError::LimitExceeded);
                }
                (paint, clip)
            }
        };
        if paint.semantic_id.0 == 0
            || !clip.valid()
            || paint
                .color
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err(TextError::InvalidValue);
        }
    }
    Ok(())
}

struct Atlas {
    rows: Vec<u8>,
    x: u32,
    y: u32,
    row_height: u32,
    width: u32,
    budget: usize,
}
impl Atlas {
    fn new(budget: usize) -> Self {
        Self {
            rows: Vec::new(),
            x: 1,
            y: 1,
            row_height: 0,
            width: 2048,
            budget,
        }
    }
    fn insert(&mut self, image: &SwashImage) -> Result<TextureRect, TextError> {
        let w = image.placement.width;
        let h = image.placement.height;
        if w + 2 > self.width {
            return Err(TextError::LimitExceeded);
        }
        if self.x + w + 1 > self.width {
            self.x = 1;
            self.y += self.row_height + 2;
            self.row_height = 0;
        }
        let height = self.y.checked_add(h + 1).ok_or(TextError::LimitExceeded)?;
        let pixels = (self.width as usize)
            .checked_mul(height as usize)
            .ok_or(TextError::LimitExceeded)?;
        if height > 8192 || pixels > self.budget {
            return Err(TextError::LimitExceeded);
        }
        if self.rows.len() < pixels * 4 {
            self.rows.resize(pixels * 4, 0);
        }
        for y in 0..h {
            for x in 0..w {
                let src = (y * w + x) as usize;
                let dst = (((self.y + y) * self.width + self.x + x) * 4) as usize;
                self.rows[dst..dst + 4].copy_from_slice(&[255, 255, 255, image.data[src]]);
            }
        }
        let frame = TextureRect {
            x: self.x,
            y: self.y,
            width: w,
            height: h,
        };
        self.x += w + 2;
        self.row_height = self.row_height.max(h);
        Ok(frame)
    }
    fn finish(self) -> Result<Vec<u8>, TextError> {
        let height = u32::try_from(self.rows.len() / 4 / self.width as usize)
            .map_err(|_| TextError::LimitExceeded)?;
        let mut bytes = Vec::with_capacity(self.rows.len() + 12);
        bytes.extend_from_slice(b"ATX1");
        bytes.extend_from_slice(&self.width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&self.rows);
        Ok(bytes)
    }
}
