// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

use std::{
    collections::{HashMap, HashSet},
    fmt,
};

use tilewright_scene::{Digest256, Layer, Quad, ResourceId, Sampling, SemanticId, TextureRect};

use crate::{BitmapFont, BitmapFontSet, FontId};

const MAX_PRIMITIVES: usize = 4096;
const MAX_TEXT_BYTES: usize = 1_048_576;
const MAX_GLYPHS: usize = 262_144;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiViewport {
    pub logical_size: [u32; 2],
    pub physical_size: [u32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl UiRect {
    pub(crate) fn valid(self) -> bool {
        [
            self.x,
            self.y,
            self.width,
            self.height,
            self.x + self.width,
            self.y + self.height,
        ]
        .iter()
        .all(|value| value.is_finite() && value.abs() <= 16_777_216.0)
            && self.width >= 0.0
            && self.height >= 0.0
    }

    pub(crate) fn intersect(self, other: Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = (self.x + self.width).min(other.x + other.width);
        let bottom = (self.y + self.height).min(other.y + other.height);
        (right > x && bottom > y).then_some(Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiTexture {
    pub id: ResourceId,
    pub revision: u64,
    pub digest: Digest256,
}

impl UiTexture {
    pub(crate) fn valid(self) -> bool {
        self.id.0 != 0 && self.revision != 0 && self.digest.0.iter().any(|byte| *byte != 0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiPaint {
    pub semantic_id: SemanticId,
    pub painter_order: u32,
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UiPrimitive<'a> {
    Solid {
        bounds: UiRect,
        clip: UiRect,
        paint: UiPaint,
        pickable: bool,
    },
    /// Origin is the top-left of the first line box in logical units. Size is
    /// the font's nominal pixel size in logical units; no shaping is performed.
    Text {
        origin: [f32; 2],
        clip: UiRect,
        paint: UiPaint,
        font: FontId,
        size: f32,
        text: &'a str,
    },
}

impl UiPrimitive<'_> {
    fn paint(&self) -> UiPaint {
        match *self {
            Self::Solid { paint, .. } | Self::Text { paint, .. } => paint,
        }
    }
    fn clip(&self) -> UiRect {
        match *self {
            Self::Solid { clip, .. } | Self::Text { clip, .. } => clip,
        }
    }
    fn parts(&self) -> usize {
        match *self {
            Self::Solid { .. } => 1,
            Self::Text { text, .. } => text.chars().count(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct UiDisplayList<'a> {
    pub primitives: &'a [UiPrimitive<'a>],
}

/// Caller budgets can tighten the absolute caps: 4096 primitives, 1 MiB UTF-8,
/// 262144 scalars/quads, 8192 pixels per side, 16 Mi pixels, 4096 lines/run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiLimits {
    pub maximum_primitives: usize,
    pub maximum_text_bytes: usize,
    pub maximum_glyphs: usize,
    pub maximum_quads: usize,
    pub maximum_lines: usize,
    pub maximum_dimension: u32,
    pub maximum_pixels: u64,
}

impl Default for UiLimits {
    fn default() -> Self {
        Self {
            maximum_primitives: MAX_PRIMITIVES,
            maximum_text_bytes: MAX_TEXT_BYTES,
            maximum_glyphs: MAX_GLYPHS,
            maximum_quads: MAX_GLYPHS,
            maximum_lines: 4096,
            maximum_dimension: 8192,
            maximum_pixels: 16_777_216,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedReason {
    MissingGlyph,
    RequiresShaping,
}

/// Scalar and byte offsets always refer to the original unmodified input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnsupportedText {
    pub primitive_index: usize,
    pub byte_start: usize,
    pub byte_end: usize,
    pub character: char,
    pub reason: UnsupportedReason,
}

/// Logical coordinates, before DPI scaling; includes spaces and line breaks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphLayout {
    pub byte_start: usize,
    pub byte_end: usize,
    pub character: char,
    pub baseline: [f32; 2],
    pub advance: f32,
    pub supported: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextLayout {
    pub primitive_index: usize,
    pub size: [f32; 2],
    pub lines: usize,
    pub glyphs: Vec<GlyphLayout>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompiledUi {
    pub quads: Vec<Quad>,
    pub text_layouts: Vec<TextLayout>,
    pub unsupported: Vec<UnsupportedText>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiError {
    InvalidValue,
    InvalidFont,
    UnknownFont,
    LimitExceeded,
    PartOverflow,
    IdentityCollision,
    ResourceCollision,
}

impl fmt::Display for UiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid UI display list: {self:?}")
    }
}
impl std::error::Error for UiError {}

/// A deliberately bounded unshaped repertoire. Glyph availability alone does
/// not imply support for combining sequences, bidirectional or complex scripts.
pub(crate) fn simple_scalar(character: char) -> bool {
    character != '\u{ad}'
        && matches!(character, ' '..='~' | '\u{a0}'..='\u{24f}' | '\u{1e00}'..='\u{1eff}'
        | '\u{2010}'..='\u{2027}' | '\u{2030}'..='\u{205e}'
        | '\u{20a0}'..='\u{20cf}' | '\u{2122}' | '\u{2212}' | '\u{fffd}')
}

/// Compiles positioned logical UI into existing Interface-layer scene quads.
/// Rectangular clipping crops UVs relative to each original atlas frame.
/// Input counts, font/geometry validity and part capacity are checked before
/// allocating output. A dry pass also verifies derived coordinates and bounds.
///
/// # Errors
/// Returns a typed error for invalid values, missing fonts, conflicting
/// resources, exceeded budgets or more than 65536 parts of one semantic ID.
pub fn compile_ui(
    viewport: UiViewport,
    list: UiDisplayList<'_>,
    fonts: &BitmapFontSet<'_>,
    solid: UiTexture,
    limits: UiLimits,
) -> Result<CompiledUi, UiError> {
    validate_inputs(viewport, list, fonts, solid, limits)?;
    compile_pass(viewport, list, fonts, solid, limits, None)?;
    let mut output = CompiledUi::default();
    compile_pass(viewport, list, fonts, solid, limits, Some(&mut output))?;
    Ok(output)
}

fn validate_inputs(
    viewport: UiViewport,
    list: UiDisplayList<'_>,
    fonts: &BitmapFontSet<'_>,
    solid: UiTexture,
    limits: UiLimits,
) -> Result<(), UiError> {
    if !solid.valid() || viewport.logical_size.contains(&0) || viewport.physical_size.contains(&0) {
        return Err(UiError::InvalidValue);
    }
    let dimension = limits.maximum_dimension.min(8192);
    if viewport
        .logical_size
        .iter()
        .chain(viewport.physical_size.iter())
        .any(|side| *side > dimension)
        || u64::from(viewport.physical_size[0]) * u64::from(viewport.physical_size[1])
            > limits.maximum_pixels.min(16_777_216)
        || list.primitives.len() > limits.maximum_primitives.min(MAX_PRIMITIVES)
    {
        return Err(UiError::LimitExceeded);
    }
    fonts.check_solid(solid)?;
    let mut bytes = 0_usize;
    let mut glyphs = 0_usize;
    let mut parts = PartLedger::new();
    for primitive in list.primitives {
        let paint = primitive.paint();
        if paint.semantic_id.0 == 0
            || !primitive.clip().valid()
            || paint
                .color
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err(UiError::InvalidValue);
        }
        match *primitive {
            UiPrimitive::Solid { bounds, .. } => {
                if !bounds.valid() {
                    return Err(UiError::InvalidValue);
                }
            }
            UiPrimitive::Text {
                origin,
                font,
                size,
                text,
                ..
            } => {
                if origin
                    .iter()
                    .any(|v| !v.is_finite() || v.abs() > 16_777_216.0)
                    || !size.is_finite()
                    || size <= 0.0
                    || size > 4096.0
                {
                    return Err(UiError::InvalidValue);
                }
                if fonts.get(font).is_none() {
                    return Err(UiError::UnknownFont);
                }
                bytes = bytes
                    .checked_add(text.len())
                    .ok_or(UiError::LimitExceeded)?;
                if bytes > limits.maximum_text_bytes.min(MAX_TEXT_BYTES) {
                    return Err(UiError::LimitExceeded);
                }
                glyphs = glyphs
                    .checked_add(text.chars().count())
                    .ok_or(UiError::LimitExceeded)?;
                if glyphs > limits.maximum_glyphs.min(MAX_GLYPHS)
                    || text.bytes().filter(|byte| *byte == b'\n').count() + 1
                        > limits.maximum_lines.min(4096)
                {
                    return Err(UiError::LimitExceeded);
                }
            }
        }
        parts.claim(paint.semantic_id, primitive.parts())?;
    }
    Ok(())
}

struct PartLedger {
    ids: [SemanticId; MAX_PRIMITIVES],
    counts: [u32; MAX_PRIMITIVES],
    used: usize,
}
impl PartLedger {
    fn new() -> Self {
        Self {
            ids: [SemanticId(0); MAX_PRIMITIVES],
            counts: [0; MAX_PRIMITIVES],
            used: 0,
        }
    }
    fn claim(&mut self, id: SemanticId, count: usize) -> Result<usize, UiError> {
        let index = if let Some(index) = self.ids[..self.used].iter().position(|entry| *entry == id)
        {
            index
        } else {
            if self.used == MAX_PRIMITIVES {
                return Err(UiError::LimitExceeded);
            }
            let index = self.used;
            self.ids[index] = id;
            self.used += 1;
            index
        };
        let start = usize::try_from(self.counts[index]).map_err(|_| UiError::PartOverflow)?;
        let end = start.checked_add(count).ok_or(UiError::PartOverflow)?;
        if end > 65_536 {
            return Err(UiError::PartOverflow);
        }
        self.counts[index] = u32::try_from(end).map_err(|_| UiError::PartOverflow)?;
        Ok(start)
    }
}

// Keep dry and emitting passes on the same geometry/layout code. All integer
// casts here are exact: validated dimensions/frames <=8192 and lines <=4096.
#[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
fn compile_pass(
    viewport: UiViewport,
    list: UiDisplayList<'_>,
    fonts: &BitmapFontSet<'_>,
    solid: UiTexture,
    limits: UiLimits,
    mut output: Option<&mut CompiledUi>,
) -> Result<(), UiError> {
    let scale = [
        viewport.physical_size[0] as f32 / viewport.logical_size[0] as f32,
        viewport.physical_size[1] as f32 / viewport.logical_size[1] as f32,
    ];
    let target = UiRect {
        x: 0.0,
        y: 0.0,
        width: viewport.logical_size[0] as f32,
        height: viewport.logical_size[1] as f32,
    };
    let mut parts = PartLedger::new();
    let mut quads = 0_usize;
    for (index, primitive) in list.primitives.iter().enumerate() {
        let paint = primitive.paint();
        let first_part = parts.claim(paint.semantic_id, primitive.parts())?;
        let clip = primitive.clip().intersect(target);
        match *primitive {
            UiPrimitive::Solid {
                bounds, pickable, ..
            } => {
                if let Some(quad) = clipped_quad(
                    bounds, clip, scale, paint, first_part, solid, None, pickable,
                )? {
                    emit_quad(quad, &mut quads, limits, &mut output)?;
                }
            }
            UiPrimitive::Text {
                origin,
                font,
                size,
                text,
                ..
            } => {
                let font = fonts.get(font).ok_or(UiError::UnknownFont)?;
                let mut layout = TextLayout {
                    primitive_index: index,
                    size: [0.0; 2],
                    lines: 1,
                    glyphs: Vec::new(),
                };
                let factor = size / font.metrics().native_size;
                let line_height =
                    (font.metrics().ascent + font.metrics().descent + font.metrics().line_gap)
                        * factor;
                let mut pen = [origin[0], origin[1] + font.metrics().ascent * factor];
                if !pen.iter().all(|v| v.is_finite() && v.abs() <= 16_777_216.0) {
                    return Err(UiError::InvalidValue);
                }
                let mut width = 0.0_f32;
                for (ordinal, (byte_start, character)) in text.char_indices().enumerate() {
                    let byte_end = byte_start + character.len_utf8();
                    let baseline = pen;
                    let reason = unsupported_reason(font, character);
                    let advance;
                    if character == '\n' {
                        advance = 0.0;
                        width = width.max(pen[0] - origin[0]);
                        pen[0] = origin[0];
                        pen[1] += line_height;
                        layout.lines += 1;
                    } else {
                        let glyph = font
                            .glyph(if reason.is_some() {
                                font.fallback()
                            } else {
                                character
                            })
                            .ok_or(UiError::InvalidFont)?;
                        advance = glyph.advance * factor;
                        if let Some(frame) = glyph.frame {
                            let bounds = UiRect {
                                x: pen[0] + glyph.bearing[0] * factor,
                                y: pen[1] + glyph.bearing[1] * factor,
                                width: frame.width as f32 * factor,
                                height: frame.height as f32 * factor,
                            };
                            if let Some(quad) = clipped_quad(
                                bounds,
                                clip,
                                scale,
                                paint,
                                first_part + ordinal,
                                font.texture(),
                                Some(frame),
                                false,
                            )? {
                                emit_quad(quad, &mut quads, limits, &mut output)?;
                            }
                        }
                        pen[0] += advance;
                    }
                    if !pen.iter().all(|v| v.is_finite() && v.abs() <= 16_777_216.0)
                        || !advance.is_finite()
                    {
                        return Err(UiError::InvalidValue);
                    }
                    if let Some(result) = output.as_deref_mut() {
                        layout.glyphs.push(GlyphLayout {
                            byte_start,
                            byte_end,
                            character,
                            baseline,
                            advance,
                            supported: reason.is_none(),
                        });
                        if let Some(reason) = reason {
                            result.unsupported.push(UnsupportedText {
                                primitive_index: index,
                                byte_start,
                                byte_end,
                                character,
                                reason,
                            });
                        }
                    }
                }
                width = width.max(pen[0] - origin[0]);
                layout.size = [
                    width,
                    (font.metrics().ascent + font.metrics().descent) * factor
                        + (layout.lines - 1) as f32 * line_height,
                ];
                if layout
                    .size
                    .iter()
                    .any(|v| !v.is_finite() || *v > 16_777_216.0)
                {
                    return Err(UiError::InvalidValue);
                }
                if let Some(result) = output.as_deref_mut() {
                    result.text_layouts.push(layout);
                }
            }
        }
    }
    Ok(())
}

fn unsupported_reason(font: &BitmapFont<'_>, character: char) -> Option<UnsupportedReason> {
    if character == '\n' {
        None
    } else if !simple_scalar(character) {
        Some(UnsupportedReason::RequiresShaping)
    } else if font.glyph(character).is_none() {
        Some(UnsupportedReason::MissingGlyph)
    } else {
        None
    }
}

fn emit_quad(
    quad: Quad,
    count: &mut usize,
    limits: UiLimits,
    output: &mut Option<&mut CompiledUi>,
) -> Result<(), UiError> {
    *count += 1;
    if *count > limits.maximum_quads.min(MAX_GLYPHS) {
        return Err(UiError::LimitExceeded);
    }
    if let Some(result) = output.as_deref_mut() {
        result.quads.push(quad);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn clipped_quad(
    bounds: UiRect,
    clip: Option<UiRect>,
    scale: [f32; 2],
    paint: UiPaint,
    part: usize,
    texture: UiTexture,
    frame: Option<TextureRect>,
    pickable: bool,
) -> Result<Option<Quad>, UiError> {
    if !bounds.valid() {
        return Err(UiError::InvalidValue);
    }
    let Some(crop) = clip.and_then(|clip| bounds.intersect(clip)) else {
        return Ok(None);
    };
    let left = crop.x * scale[0];
    let top = crop.y * scale[1];
    let right = (crop.x + crop.width) * scale[0];
    let bottom = (crop.y + crop.height) * scale[1];
    // Finite f32 geometry can still collapse after scaling/rounding.
    if ![left, top, right, bottom].iter().all(|v| v.is_finite()) || left >= right || top >= bottom {
        return Err(UiError::InvalidValue);
    }
    let u0 = (crop.x - bounds.x) / bounds.width;
    let v0 = (crop.y - bounds.y) / bounds.height;
    let u1 = (crop.x + crop.width - bounds.x) / bounds.width;
    let v1 = (crop.y + crop.height - bounds.y) / bounds.height;
    Ok(Some(Quad {
        semantic_id: paint.semantic_id,
        part: u16::try_from(part).map_err(|_| UiError::PartOverflow)?,
        resource_id: texture.id,
        resource_revision: texture.revision,
        resource_digest: texture.digest,
        frame,
        positions: [[left, top], [right, top], [right, bottom], [left, bottom]],
        uvs: [[u0, v0], [u1, v0], [u1, v1], [u0, v1]],
        depth: 0,
        painter_order: paint.painter_order,
        color: paint.color,
        grayscale: 0.0,
        glow: None,
        pickable,
        sampling: Sampling::Linear,
        layer: Layer::Interface,
    }))
}

/// Validates semantic/resource namespaces before consumer scene composition.
/// Distinct semantic IDs are required even if UI/world parts would not overlap.
/// This does not replace scene or resource-provider validation.
///
/// # Errors
/// Rejects shared semantic IDs or any resource ID whose revision/digest differ.
pub fn validate_ui_composition(world: &[Quad], ui: &CompiledUi) -> Result<(), UiError> {
    if world.len() > 131_072 || ui.quads.len() > MAX_GLYPHS {
        return Err(UiError::LimitExceeded);
    }
    let identities: HashSet<_> = world.iter().map(|quad| quad.semantic_id).collect();
    if ui
        .quads
        .iter()
        .any(|quad| identities.contains(&quad.semantic_id))
    {
        return Err(UiError::IdentityCollision);
    }
    let mut resources = HashMap::new();
    for quad in world.iter().chain(ui.quads.iter()) {
        let value = (quad.resource_revision, quad.resource_digest);
        if resources
            .insert(quad.resource_id, value)
            .is_some_and(|old| old != value)
        {
            return Err(UiError::ResourceCollision);
        }
    }
    Ok(())
}
