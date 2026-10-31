// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

use std::{ops::Range, sync::Arc};

use cosmic_text::{Affinity, Attrs, Buffer, Cursor, Family, Metrics, Shaping, Weight, Wrap};
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

#[derive(Debug)]
pub(super) struct LayoutData {
    pub(super) buffer: Buffer,
    text: String,
    font_size: f32,
    width: Option<f32>,
    size: [f32; 2],
    line_offsets: Vec<usize>,
    boundaries: Vec<usize>,
    clusters: Vec<UnicodeCluster>,
    missing: Vec<Range<usize>>,
    carets: Vec<CaretStop>,
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
        if let Some(cursor) = self.0.buffer.hit(point[0], point[1]) {
            let wanted = TextCursor {
                byte: self.0.line_offsets.get(cursor.line).copied().unwrap_or(0) + cursor.index,
                affinity: from_affinity(cursor.affinity),
            };
            if self.0.carets.iter().any(|stop| stop.cursor == wanted) {
                return wanted;
            }
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
        let start = self.cosmic_cursor(start);
        let end = self.cosmic_cursor(end);
        self.0
            .buffer
            .layout_runs()
            .flat_map(|run| {
                run.highlight(start, end).map(move |(x, width)| UiRect {
                    x,
                    y: run.line_top,
                    width,
                    height: run.line_height,
                })
            })
            .collect()
    }

    fn cosmic_cursor(&self, cursor: TextCursor) -> Cursor {
        let line = self
            .0
            .line_offsets
            .partition_point(|offset| *offset <= cursor.byte)
            .saturating_sub(1);
        Cursor::new_with_affinity(
            line,
            cursor.byte - self.0.line_offsets[line],
            to_affinity(cursor.affinity),
        )
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
const fn from_affinity(value: Affinity) -> TextAffinity {
    match value {
        Affinity::Before => TextAffinity::Before,
        Affinity::After => TextAffinity::After,
    }
}
const fn to_affinity(value: TextAffinity) -> Affinity {
    match value {
        TextAffinity::Before => Affinity::Before,
        TextAffinity::After => Affinity::After,
    }
}

impl TextSystem {
    /// Shapes original Unicode text with fixed regular weight, automatic bidi
    /// direction and explicit stable locale. `None` disables wrapping.
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
        if self.shape_misses >= 256 {
            self.reset_shaping();
        }
        self.shape_misses += 1;
        let mut buffer = Buffer::new_empty(Metrics::new(size, line_height));
        buffer.set_size(width, None);
        buffer.set_wrap(if width.is_some() {
            Wrap::WordOrGlyph
        } else {
            Wrap::None
        });
        buffer.set_text(
            text,
            &Attrs::new()
                .family(Family::Name(&self.pack.fonts[0].family))
                .weight(Weight::NORMAL),
            Shaping::Advanced,
            None,
        );
        if buffer.lines.len() > self.limits.maximum_lines.min(4096) {
            return Err(TextError::LimitExceeded);
        }
        buffer.shape_until_scroll(&mut self.fonts, false);
        let mut line_offsets = Vec::with_capacity(buffer.lines.len());
        let mut offset = 0;
        for line in &buffer.lines {
            line_offsets.push(offset);
            offset += line.text().len() + line.ending().as_str().len();
        }
        if offset != text.len() {
            return Err(TextError::InvalidValue);
        }
        let mut boundaries: Vec<_> = text.grapheme_indices(true).map(|(byte, _)| byte).collect();
        boundaries.push(text.len());
        let mut clusters = Vec::new();
        let mut missing = std::collections::BTreeSet::new();
        let mut carets = Vec::new();
        let mut extent = [0.0_f32, line_height];
        let mut lines = 0;
        for run in buffer.layout_runs() {
            lines += 1;
            if lines > self.limits.maximum_lines.min(4096) {
                return Err(TextError::LimitExceeded);
            }
            extent[0] = extent[0].max(run.line_w);
            extent[1] = extent[1].max(run.line_top + run.line_height);
            for glyph in run.glyphs {
                if clusters.len() >= self.limits.maximum_glyphs.min(65_536) {
                    return Err(TextError::LimitExceeded);
                }
                let bytes =
                    line_offsets[run.line_i] + glyph.start..line_offsets[run.line_i] + glyph.end;
                if bytes.start > bytes.end
                    || !text.is_char_boundary(bytes.start)
                    || !text.is_char_boundary(bytes.end)
                {
                    return Err(TextError::InvalidValue);
                }
                let bounds = UiRect {
                    x: glyph.x,
                    y: run.line_top,
                    width: glyph.w,
                    height: run.line_height,
                };
                if !bounds.valid() {
                    return Err(TextError::InvalidValue);
                }
                // Zero glyph IDs on visible text indicate missing coverage.
                // Formatting controls and default-ignorable selectors need no ink.
                let absent =
                    glyph.glyph_id == 0 && text[bytes.clone()].chars().any(visible_character);
                if absent {
                    missing.insert((bytes.start, bytes.end));
                }
                clusters.push(UnicodeCluster {
                    bytes,
                    bounds,
                    right_to_left: glyph.level.is_rtl(),
                    missing: absent,
                });
            }
            // Map cluster geometry to extended-grapheme stops in linearithmic
            // time. Avoid a cursor_position scan for every byte (quadratic on
            // large input). This uses the layout's actual glyph advances.
            let mut groups = std::collections::BTreeMap::<(usize, usize, bool), (f32, f32)>::new();
            for glyph in run.glyphs {
                groups
                    .entry((glyph.start, glyph.end, glyph.level.is_rtl()))
                    .and_modify(|span| {
                        span.0 = span.0.min(glyph.x);
                        span.1 = span.1.max(glyph.x + glyph.w);
                    })
                    .or_insert((glyph.x, glyph.x + glyph.w));
            }
            for ((start, end, rtl), (left, right)) in groups {
                let start = start + line_offsets[run.line_i];
                let end = end + line_offsets[run.line_i];
                let first = boundaries.partition_point(|b| *b < start);
                let last = boundaries.partition_point(|b| *b <= end);
                let stops = &boundaries[first..last];
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
                        y: run.line_top,
                        width: 1.0,
                        height: run.line_height,
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
            if run.glyphs.is_empty() {
                carets.push(CaretStop {
                    cursor: TextCursor {
                        byte: line_offsets[run.line_i],
                        affinity: TextAffinity::After,
                    },
                    rect: UiRect {
                        x: 0.0,
                        y: run.line_top,
                        width: 1.0,
                        height: run.line_height,
                    },
                });
            }
        }
        if extent.iter().any(|v| !v.is_finite() || *v > 16_777_216.0) {
            return Err(TextError::LimitExceeded);
        }
        // Invisible-only lines can lack glyph stops; retain a legitimate
        // original boundary at the nearest known line position.
        let present: std::collections::BTreeSet<_> =
            carets.iter().map(|stop| stop.cursor.byte).collect();
        for byte in &boundaries {
            if !present.contains(byte) {
                let line = line_offsets
                    .partition_point(|offset| *offset <= *byte)
                    .saturating_sub(1);
                let c =
                    Cursor::new_with_affinity(line, *byte - line_offsets[line], Affinity::After);
                if let Some((x, y)) = buffer.cursor_position(&c) {
                    carets.push(CaretStop {
                        cursor: TextCursor {
                            byte: *byte,
                            affinity: TextAffinity::After,
                        },
                        rect: UiRect {
                            x,
                            y,
                            width: 1.0,
                            height: line_height,
                        },
                    });
                }
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
            buffer,
            text: text.into(),
            font_size: size,
            width,
            size: extent,
            line_offsets,
            boundaries,
            clusters,
            missing: missing.into_iter().map(|(start, end)| start..end).collect(),
            carets,
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
