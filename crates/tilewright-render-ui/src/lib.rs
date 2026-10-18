// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

use tilewright_scene::SemanticId;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiQuad {
    pub semantic_id: SemanticId,
    pub bounds: [f32; 4],
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextRun<'a> {
    pub semantic_id: SemanticId,
    pub text: &'a str,
    pub font_role: &'a str,
    pub size_milli: u32,
}

/// Consumer-owned layout output. This crate deliberately performs no font,
/// filesystem, window, or GPU discovery.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiFrame<'a> {
    pub quads: Vec<UiQuad>,
    pub text: Vec<TextRun<'a>>,
}

impl UiFrame<'_> {
    /// Checks item bounds and scalar layout invariants.
    ///
    /// # Errors
    /// Returns [`Error::InvalidLayout`] for an unsafe or malformed frame.
    pub fn validate(&self, maximum_items: usize) -> Result<(), Error> {
        if self.quads.len().saturating_add(self.text.len()) > maximum_items
            || self.quads.iter().any(|quad| {
                quad.semantic_id.0 == 0
                    || quad.bounds.iter().any(|value| !value.is_finite())
                    || quad.bounds[2] <= 0.0
                    || quad.bounds[3] <= 0.0
                    || quad
                        .color
                        .iter()
                        .any(|channel| !channel.is_finite() || !(0.0..=1.0).contains(channel))
            })
            || self.text.iter().any(|run| {
                run.semantic_id.0 == 0
                    || run.text.is_empty()
                    || run.font_role.is_empty()
                    || run.size_milli == 0
            })
        {
            return Err(Error::InvalidLayout);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidLayout,
}
