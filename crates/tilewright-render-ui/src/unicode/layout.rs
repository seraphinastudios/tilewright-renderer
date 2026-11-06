// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

use std::{borrow::Cow, ops::Range, sync::Arc};

use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, FontWeight,
    LayoutContext, LineHeight, OverflowWrap, PositionedLayoutItem, StyleProperty,
};
use unicode_segmentation::UnicodeSegmentation;

use super::{TextError, TextSystem};
use crate::UiRect;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TextAffinity {
    Before,
    #[default]
    After,
}

/// Original UTF-8 byte boundary and its visual side at a bidi boundary.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TextCursor {
    pub byte: usize,
    pub affinity: TextAffinity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisualMotion {
    Left,
    Right,
    Home,
    End,
}

/// One shaped glyph's original byte cluster and logical advance rectangle.
/// Several glyphs may share one cluster; a cluster may contain many scalars.
#[derive(Clone, Debug, PartialEq)]
pub struct UnicodeCluster {
    pub bytes: Range<usize>,
    pub bounds: UiRect,
    pub right_to_left: bool,
    pub missing: bool,
}

#[derive(Clone, Copy, Debug)]
struct CaretStop {
    cursor: TextCursor,
    rect: UiRect,
}

#[derive(Clone, Debug)]
struct GraphemeSegment {
    bytes: Range<usize>,
    bounds: UiRect,
    rtl: bool,
}

/// One shaped glyph ready for rasterization, parallel to `clusters`.
#[derive(Debug)]
pub(super) struct PlacedGlyph {
    /// Index of the verified pack font that shaped this glyph.
    pub(super) font: u16,
    pub(super) glyph: u16,
    /// Pen position plus horizontal shaping offset, in layout units.
    pub(super) x: f32,
    pub(super) baseline: f32,
    /// Downward shaping offset from the baseline, in layout units.
    pub(super) y_offset: f32,
    /// Variation coordinates, for example the synthesized regular `wght`.
    pub(super) coords: Arc<[i16]>,
}

#[derive(Debug)]
pub(super) struct LayoutData {
    pub(super) glyphs: Vec<PlacedGlyph>,
    pub(super) lines: usize,
    text: String,
    font_size: f32,
    width: Option<f32>,
    size: [f32; 2],
    boundaries: Vec<usize>,
    clusters: Vec<UnicodeCluster>,
    missing: Vec<Range<usize>>,
    carets: Vec<CaretStop>,
    segments: Vec<GraphemeSegment>,
    pub(super) retained_bytes: usize,
}

/// Immutable shared layout. Cloning is cheap and never changes text or caches.
#[derive(Clone, Debug)]
pub struct UnicodeLayout(pub(super) Arc<LayoutData>);

impl UnicodeLayout {
    #[must_use]
    pub fn text(&self) -> &str {
        &self.0.text
    }
    #[must_use]
    pub fn size(&self) -> [f32; 2] {
        self.0.size
    }
    #[must_use]
    pub fn grapheme_boundaries(&self) -> &[usize] {
        &self.0.boundaries
    }
    #[must_use]
    pub fn clusters(&self) -> &[UnicodeCluster] {
        &self.0.clusters
    }
    #[must_use]
    pub fn missing_glyphs(&self) -> &[Range<usize>] {
        &self.0.missing
    }

    /// Returns a logical caret rectangle for a valid extended-grapheme boundary.
    #[must_use]
    pub fn caret(&self, cursor: TextCursor) -> Option<UiRect> {
        self.0
            .carets
            .iter()
            .find(|stop| stop.cursor == cursor)
            .or_else(|| {
                self.0
                    .carets
                    .iter()
                    .find(|stop| stop.cursor.byte == cursor.byte)
            })
            .map(|stop| stop.rect)
    }

    /// Converts a logical point to a grapheme-safe original byte cursor.
    #[must_use]
    pub fn hit(&self, point: [f32; 2]) -> TextCursor {
        if !point.iter().all(|v| v.is_finite()) {
            return TextCursor::default();
        }
        if let Some(segment) = self.0.segments.iter().find(|segment| {
            let rect = segment.bounds;
            point[0] >= rect.x
                && point[0] < rect.x + rect.width
                && point[1] >= rect.y
                && point[1] < rect.y + rect.height
        }) {
            let left_half = point[0] < segment.bounds.x + segment.bounds.width * 0.5;
            return if left_half == segment.rtl {
                TextCursor {
                    byte: segment.bytes.end,
                    affinity: TextAffinity::Before,
                }
            } else {
                TextCursor {
                    byte: segment.bytes.start,
                    affinity: TextAffinity::After,
                }
            };
        }
        self.0
            .carets
            .iter()
            .min_by(|a, b| distance(a.rect, point).total_cmp(&distance(b.rect, point)))
            .map_or(TextCursor::default(), |stop| stop.cursor)
    }

    /// Moves by visual positions. Adjacent affinities sharing the same position
    /// do not require a redundant key press. Home/End refer to the visual line.
    #[must_use]
    #[allow(clippy::float_cmp)] // Positions are exact identities from one immutable layout.
    pub fn move_visual(&self, cursor: TextCursor, motion: VisualMotion) -> TextCursor {
        let Some(rect) = self.caret(cursor) else {
            return cursor;
        };
        let current = self
            .0
            .carets
            .iter()
            .position(|stop| stop.cursor == cursor)
            .or_else(|| {
                self.0
                    .carets
                    .iter()
                    .position(|stop| stop.cursor.byte == cursor.byte)
            });
        let Some(index) = current else {
            return cursor;
        };
        let target = match motion {
            VisualMotion::Left => self.0.carets[..index]
                .iter()
                .rev()
                .find(|s| s.rect.x != rect.x || s.rect.y != rect.y),
            VisualMotion::Right => self.0.carets[index + 1..]
                .iter()
                .find(|s| s.rect.x != rect.x || s.rect.y != rect.y),
            VisualMotion::Home => self.0.carets.iter().find(|s| s.rect.y == rect.y),
            VisualMotion::End => self.0.carets.iter().rev().find(|s| s.rect.y == rect.y),
        };
        target.map_or(cursor, |stop| stop.cursor)
    }

    /// A mixed-direction selection can have several disjoint rectangles.
    /// Invalid/interior-grapheme cursors return no geometry.
    #[must_use]
    pub fn selection_rects(&self, anchor: TextCursor, focus: TextCursor) -> Vec<UiRect> {
        if anchor.byte == focus.byte || self.caret(anchor).is_none() || self.caret(focus).is_none()
        {
            return Vec::new();
        }
        let (start, end) = if anchor.byte <= focus.byte {
            (anchor, focus)
        } else {
            (focus, anchor)
        };
        let mut rectangles: Vec<_> = self
            .0
            .segments
            .iter()
            .filter(|segment| {
                segment.bytes.start < end.byte
                    && segment.bytes.end > start.byte
                    && segment.bounds.width > 0.0
            })
            .map(|segment| segment.bounds)
            .collect();
        rectangles.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
        let mut merged: Vec<UiRect> = Vec::new();
        for rect in rectangles {
            if let Some(last) = merged.last_mut().filter(|last| {
                last.y.to_bits() == rect.y.to_bits()
                    && last.height.to_bits() == rect.height.to_bits()
                    && rect.x <= last.x + last.width
            }) {
                last.width = (rect.x + rect.width).max(last.x + last.width) - last.x;
            } else {
                merged.push(rect);
            }
        }
        merged
    }
}

fn distance(rect: UiRect, point: [f32; 2]) -> f32 {
    let vertical = if point[1] < rect.y {
        rect.y - point[1]
    } else {
        (point[1] - rect.y - rect.height).max(0.0)
    };
    vertical * 16_777_216.0 + (rect.x - point[0]).abs()
}

/// A visual line's glyph advances, in visual order, with original bytes.
struct VisualLine {
    top: f32,
    height: f32,
    width: f32,
    start: usize,
    glyphs: Vec<(Range<usize>, f32, f32, bool)>,
}

/// Hard paragraphs: CR, LF, CRLF and U+2029 end one. Each paragraph receives
/// its own automatic bidi base direction. A final terminator starts a last,
/// empty paragraph.
fn paragraphs(text: &str) -> Vec<Range<usize>> {
    let mut paragraphs = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        let end = match c {
            '\r' if chars.peek().is_some_and(|(_, next)| *next == '\n') => {
                chars.next();
                index + 2
            }
            '\r' | '\n' | '\u{2029}' => index + c.len_utf8(),
            _ => continue,
        };
        paragraphs.push(start..index);
        start = end;
    }
    paragraphs.push(start..text.len());
    paragraphs
}

/// Builds one paragraph with fixed regular weight, no locale, the complete
/// ordered pack stack for per-cluster fallback, and the given tab advances.
fn build(
    layout_context: &mut LayoutContext,
    fonts: &mut FontContext,
    families: &[FontFamilyName<'static>],
    text: &str,
    size: f32,
    width: Option<f32>,
    tabs: &[(Range<usize>, f32)],
) -> parley::Layout<[u8; 4]> {
    let mut builder = layout_context.ranged_builder(fonts, text, 1.0, false);
    builder.push_default(StyleProperty::FontFamily(FontFamily::List(Cow::Borrowed(
        families,
    ))));
    builder.push_default(StyleProperty::FontSize(size));
    builder.push_default(StyleProperty::FontWeight(FontWeight::NORMAL));
    builder.push_default(StyleProperty::LineHeight(LineHeight::Absolute(size * 1.5)));
    // Break inside a word only when the word alone cannot fit.
    builder.push_default(StyleProperty::OverflowWrap(OverflowWrap::Anywhere));
    for (range, extra) in tabs {
        builder.push(StyleProperty::LetterSpacing(*extra), range.clone());
    }
    let mut layout = builder.build(text);
    layout.break_all_lines(width);
    // Start alignment right-aligns right-to-left paragraphs within the wrap
    // width, or within their widest line when unwrapped.
    layout.align(Alignment::Start, AlignmentOptions::default());
    layout
}

/// Tab stops every eight space advances, measured along the paragraph in
/// logical order before wrapping. Tabs are shaped as spaces of the same byte
/// length; this returns the extra advance each tab needs to reach its stop.
fn tab_advances(layout: &parley::Layout<[u8; 4]>, text: &str) -> Vec<(Range<usize>, f32)> {
    let mut clusters: Vec<(usize, f32)> = layout
        .lines()
        .flat_map(|line| line.runs())
        .flat_map(|run| run.clusters().map(|c| (c.text_range().start, c.advance())))
        .collect();
    clusters.sort_by_key(|cluster| cluster.0);
    let mut tabs = Vec::new();
    let mut x = 0.0_f32;
    for (start, advance) in clusters {
        let stop_width = advance * 8.0;
        if text.as_bytes().get(start) == Some(&b'\t') && stop_width > 0.0 {
            let stop = ((x / stop_width).floor() + 1.0) * stop_width;
            tabs.push((start..start + 1, stop - x - advance));
            x = stop;
        } else {
            x += advance;
        }
    }
    tabs
}

impl TextSystem {
    /// Shapes original Unicode text with fixed regular weight, automatic bidi
    /// direction per paragraph and no locale. `None` disables wrapping.
    ///
    /// # Errors
    /// Rejects invalid metrics or text/glyph/line budgets without partial output.
    #[allow(clippy::too_many_lines)] // Keep validated shaping and geometry publication atomic.
    pub fn layout(
        &mut self,
        text: &str,
        size: f32,
        width: Option<f32>,
    ) -> Result<UnicodeLayout, TextError> {
        if !size.is_finite()
            || !(1.0..=256.0).contains(&size)
            || width.is_some_and(|w| !w.is_finite() || !(1.0..=8192.0).contains(&w))
        {
            return Err(TextError::InvalidValue);
        }
        if text.len() > self.limits.maximum_text_bytes.min(65_536) {
            return Err(TextError::LimitExceeded);
        }
        if let Some(index) = self.layouts.iter().position(|layout| {
            layout.0.text == text
                && layout.0.font_size.to_bits() == size.to_bits()
                && layout.0.width == width
        }) {
            let layout = self.layouts.remove(index).ok_or(TextError::InvalidValue)?;
            self.layouts.push_back(layout.clone());
            return Ok(layout);
        }
        let line_height = size * 1.5;
        let maximum_lines = self.limits.maximum_lines.min(4096);
        let maximum_glyphs = self.limits.maximum_glyphs.min(65_536);
        let ranges = paragraphs(text);
        if ranges.len() > maximum_lines {
            return Err(TextError::LimitExceeded);
        }
        if self.shape_misses >= 256 {
            self.reset_shaping();
        }
        self.shape_misses += 1;
        let mut lines = Vec::<VisualLine>::new();
        let mut glyphs = Vec::<PlacedGlyph>::new();
        let mut top = 0.0_f32;
        for range in ranges {
            let offset = range.start;
            let original = &text[range];
            if original.is_empty() {
                if lines.len() >= maximum_lines {
                    return Err(TextError::LimitExceeded);
                }
                lines.push(VisualLine {
                    top,
                    height: line_height,
                    width: 0.0,
                    start: offset,
                    glyphs: Vec::new(),
                });
                top += line_height;
                continue;
            }
            // A tab is shaped as a space; both are one byte, so every
            // original byte offset is preserved.
            let shaped: Cow<'_, str> = if original.contains('\t') {
                Cow::Owned(original.replace('\t', " "))
            } else {
                Cow::Borrowed(original)
            };
            let tabs = if original.contains('\t') {
                let unwrapped = build(
                    &mut self.layout_context,
                    &mut self.fonts,
                    &self.families,
                    &shaped,
                    size,
                    None,
                    &[],
                );
                tab_advances(&unwrapped, original)
            } else {
                Vec::new()
            };
            let paragraph = build(
                &mut self.layout_context,
                &mut self.fonts,
                &self.families,
                &shaped,
                size,
                width,
                &tabs,
            );
            for line in paragraph.lines() {
                if lines.len() >= maximum_lines {
                    return Err(TextError::LimitExceeded);
                }
                let metrics = line.metrics();
                let baseline = top + metrics.baseline;
                let mut visual = VisualLine {
                    top: top + metrics.block_min_coord,
                    height: metrics.line_height,
                    width: metrics.advance - metrics.hanging_advance,
                    start: offset + line.text_range().start,
                    glyphs: Vec::new(),
                };
                let line_first = glyphs.len();
                for item in line.items() {
                    let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                        continue;
                    };
                    let run = glyph_run.run();
                    let font = self.pack_font(run.font().data.id())?;
                    let coords: Arc<[i16]> = run
                        .normalized_coords()
                        .iter()
                        .map(|coord| coord.to_bits())
                        .collect();
                    let rtl = run.is_rtl();
                    // Group graphemes into shaped clusters: a ligature's
                    // continuations join the grapheme that owns its glyphs.
                    let mut shaped_clusters: Vec<(Range<usize>, parley::Cluster<'_, [u8; 4]>)> =
                        Vec::new();
                    for cluster in run.clusters() {
                        let bytes = cluster.text_range();
                        match shaped_clusters.last_mut() {
                            Some(last) if cluster.is_ligature_continuation() => {
                                last.0.end = last.0.end.max(bytes.end);
                            }
                            _ => shaped_clusters.push((bytes, cluster)),
                        }
                    }
                    if rtl {
                        shaped_clusters.reverse();
                    }
                    let mut pen = glyph_run.offset();
                    for (bytes, cluster) in shaped_clusters {
                        let bytes = offset + bytes.start..offset + bytes.end;
                        for glyph in cluster.glyphs() {
                            if glyphs.len() >= maximum_glyphs {
                                return Err(TextError::LimitExceeded);
                            }
                            let id = u16::try_from(glyph.id).map_err(|_| TextError::InvalidFont)?;
                            visual.glyphs.push((bytes.clone(), pen, glyph.advance, rtl));
                            glyphs.push(PlacedGlyph {
                                font,
                                glyph: id,
                                x: pen + glyph.x,
                                baseline,
                                y_offset: glyph.y,
                                coords: coords.clone(),
                            });
                            pen += glyph.advance;
                        }
                    }
                }
                // Publish glyphs in the paragraph's visual direction, which
                // fixes cluster and quad part numbering for consumers.
                if paragraph.is_rtl() {
                    glyphs[line_first..].reverse();
                    visual.glyphs.reverse();
                }
                lines.push(visual);
            }
            top += paragraph.height();
        }
        let mut boundaries: Vec<_> = text.grapheme_indices(true).map(|(byte, _)| byte).collect();
        boundaries.push(text.len());
        let mut clusters = Vec::with_capacity(glyphs.len());
        let mut missing = std::collections::BTreeSet::new();
        let mut carets = Vec::new();
        let mut segments = Vec::new();
        let mut extent = [0.0_f32, line_height];
        let mut placed = glyphs.iter();
        for line in &lines {
            extent[0] = extent[0].max(line.width);
            extent[1] = extent[1].max(line.top + line.height);
            for (bytes, x, advance, rtl) in &line.glyphs {
                let glyph = placed.next().ok_or(TextError::InvalidValue)?;
                if bytes.start > bytes.end
                    || !text.is_char_boundary(bytes.start)
                    || !text.is_char_boundary(bytes.end)
                {
                    return Err(TextError::InvalidValue);
                }
                let bounds = UiRect {
                    x: *x,
                    y: line.top,
                    width: *advance,
                    height: line.height,
                };
                if !bounds.valid() {
                    return Err(TextError::InvalidValue);
                }
                // Zero glyph IDs on visible text indicate missing coverage.
                // Formatting controls and default-ignorable selectors need no ink.
                let absent = glyph.glyph == 0 && text[bytes.clone()].chars().any(visible_character);
                if absent {
                    missing.insert((bytes.start, bytes.end));
                }
                clusters.push(UnicodeCluster {
                    bytes: bytes.clone(),
                    bounds,
                    right_to_left: *rtl,
                    missing: absent,
                });
            }
            // Map cluster geometry to extended-grapheme stops in linearithmic
            // time, using the layout's actual glyph advances.
            let mut groups = std::collections::BTreeMap::<(usize, usize, bool), (f32, f32)>::new();
            for (bytes, x, advance, rtl) in &line.glyphs {
                groups
                    .entry((bytes.start, bytes.end, *rtl))
                    .and_modify(|span| {
                        span.0 = span.0.min(*x);
                        span.1 = span.1.max(x + advance);
                    })
                    .or_insert((*x, x + advance));
            }
            for ((start, end, rtl), (left, right)) in groups {
                let first = boundaries.partition_point(|b| *b < start);
                let last = boundaries.partition_point(|b| *b <= end);
                let stops = &boundaries[first..last];
                let segment_first = boundaries
                    .partition_point(|byte| *byte <= start)
                    .saturating_sub(1);
                let segment_last = boundaries.partition_point(|byte| *byte < end);
                let count = segment_last.saturating_sub(segment_first);
                for index in segment_first..segment_last {
                    #[allow(clippy::cast_precision_loss)]
                    let fraction = (index - segment_first) as f32 / count as f32;
                    #[allow(clippy::cast_precision_loss)]
                    let width = (right - left) / count as f32;
                    let x = if rtl {
                        right - (right - left) * fraction - width
                    } else {
                        left + (right - left) * fraction
                    };
                    segments.push(GraphemeSegment {
                        bytes: boundaries[index]..boundaries[index + 1],
                        bounds: UiRect {
                            x,
                            y: line.top,
                            width,
                            height: line.height,
                        },
                        rtl,
                    });
                }
                let local_boundaries: Vec<_> = text[start..end]
                    .grapheme_indices(true)
                    .map(|(byte, _)| start + byte)
                    .collect();
                let units = local_boundaries.len().max(1);
                for byte in stops {
                    let before = local_boundaries.partition_point(|offset| offset < byte);
                    #[allow(clippy::cast_precision_loss)]
                    let fraction = before as f32 / units as f32;
                    let x = if rtl {
                        right - (right - left) * fraction
                    } else {
                        left + (right - left) * fraction
                    };
                    let rect = UiRect {
                        x,
                        y: line.top,
                        width: 1.0,
                        height: line.height,
                    };
                    if *byte != end {
                        carets.push(CaretStop {
                            cursor: TextCursor {
                                byte: *byte,
                                affinity: TextAffinity::After,
                            },
                            rect,
                        });
                    }
                    if *byte != start {
                        carets.push(CaretStop {
                            cursor: TextCursor {
                                byte: *byte,
                                affinity: TextAffinity::Before,
                            },
                            rect,
                        });
                    }
                }
            }
            if line.glyphs.is_empty() {
                carets.push(CaretStop {
                    cursor: TextCursor {
                        byte: line.start,
                        affinity: TextAffinity::After,
                    },
                    rect: UiRect {
                        x: 0.0,
                        y: line.top,
                        width: 1.0,
                        height: line.height,
                    },
                });
            }
        }
        if placed.next().is_some() {
            return Err(TextError::InvalidValue);
        }
        if extent.iter().any(|v| !v.is_finite() || *v > 16_777_216.0) {
            return Err(TextError::LimitExceeded);
        }
        // Invisible-only content can lack glyph stops; retain a legitimate
        // original boundary at the start of the visual line that holds it.
        let present: std::collections::BTreeSet<_> =
            carets.iter().map(|stop| stop.cursor.byte).collect();
        for byte in &boundaries {
            if !present.contains(byte)
                && let Some(line) = lines.iter().rev().find(|line| line.start <= *byte)
            {
                carets.push(CaretStop {
                    cursor: TextCursor {
                        byte: *byte,
                        affinity: TextAffinity::After,
                    },
                    rect: UiRect {
                        x: 0.0,
                        y: line.top,
                        width: 1.0,
                        height: line.height,
                    },
                });
            }
        }
        carets.sort_by(|a, b| {
            a.rect
                .y
                .total_cmp(&b.rect.y)
                .then(a.rect.x.total_cmp(&b.rect.x))
                .then(a.cursor.byte.cmp(&b.cursor.byte))
                .then((a.cursor.affinity as u8).cmp(&(b.cursor.affinity as u8)))
        });
        carets.dedup_by(|a, b| a.cursor == b.cursor && a.rect == b.rect);
        let retained_bytes = text
            .len()
            .saturating_mul(512)
            .saturating_add(clusters.len().saturating_mul(512));
        let layout = UnicodeLayout(Arc::new(LayoutData {
            glyphs,
            lines: lines.len(),
            text: text.into(),
            font_size: size,
            width,
            size: extent,
            boundaries,
            clusters,
            missing: missing.into_iter().map(|(start, end)| start..end).collect(),
            carets,
            segments,
            retained_bytes,
        }));
        let budget = self.limits.maximum_layout_cache_bytes.min(8 * 1024 * 1024);
        let count = self.limits.maximum_cached_layouts.min(256);
        if retained_bytes <= budget && count > 0 {
            while self.layouts.len() >= count
                || self.layout_bytes.saturating_add(retained_bytes) > budget
            {
                let Some(old) = self.layouts.pop_front() else {
                    break;
                };
                self.layout_bytes -= old.0.retained_bytes;
            }
            self.layouts.push_back(layout.clone());
            self.layout_bytes += retained_bytes;
        }
        Ok(layout)
    }
}

fn visible_character(c: char) -> bool {
    !c.is_control()
        && !c.is_whitespace()
        && !matches!(c, '\u{ad}'|'\u{34f}'|'\u{61c}'|'\u{180e}'|'\u{200b}'..='\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2060}'..='\u{206f}'|'\u{fe00}'..='\u{fe0f}'|'\u{feff}'|'\u{e0100}'..='\u{e01ef}')
}
