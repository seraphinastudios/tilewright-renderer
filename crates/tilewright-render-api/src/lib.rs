// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

use std::{fmt, sync::Arc};
use tilewright_render_resources::ResourceProvider;
use tilewright_scene::SceneBundle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendPreference {
    Automatic,
    Vulkan,
    Direct3D12,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetKind {
    Window,
    Embedded,
    Offscreen,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetDescriptor {
    pub kind: TargetKind,
    pub width: u32,
    pub height: u32,
    pub backend: BackendPreference,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetLimits {
    pub maximum_dimension: u32,
    pub maximum_pixels: u64,
}

impl Default for TargetLimits {
    fn default() -> Self {
        Self {
            maximum_dimension: 8_192,
            maximum_pixels: 16_777_216,
        }
    }
}

impl TargetDescriptor {
    /// Checks nonzero bounded target dimensions.
    ///
    /// # Errors
    /// Returns [`Error::InvalidTarget`] for dimensions outside the bound.
    pub fn validate(self, limits: TargetLimits) -> Result<Self, Error> {
        if self.width == 0
            || self.height == 0
            || self.width > limits.maximum_dimension
            || self.height > limits.maximum_dimension
            || u64::from(self.width) * u64::from(self.height) > limits.maximum_pixels
        {
            return Err(Error::InvalidTarget);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RecoveryMetrics {
    pub surface_reconfigurations: u64,
    pub device_recoveries: u64,
    pub skipped_frames: u64,
    pub frames_submitted: u64,
    pub resource_requests: u64,
    pub uploaded_bytes: u64,
    pub vertex_count: u64,
    pub vertex_allocation_bytes: u64,
    pub target_allocation_bytes: u64,
    pub readback_bytes: u64,
    pub frame_cpu_micros: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameOutput {
    pub width: u32,
    pub height: u32,
    pub rgba8: Vec<u8>,
    pub semantic_ids: Vec<u32>,
    pub depth: Vec<i16>,
    pub coverage: Vec<u8>,
    pub metrics: RecoveryMetrics,
}

impl FrameOutput {
    /// Checks that every output plane exactly matches the pixel dimensions.
    ///
    /// # Errors
    /// Returns [`Error::InvalidFrame`] or [`Error::LimitExceeded`] when the
    /// dimensions and planes cannot form a valid bounded frame.
    pub fn validate(&self) -> Result<(), Error> {
        let pixels = usize::try_from(self.width)
            .ok()
            .and_then(|width| {
                usize::try_from(self.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .ok_or(Error::LimitExceeded)?;
        if pixels == 0
            || self.rgba8.len() != pixels.checked_mul(4).ok_or(Error::LimitExceeded)?
            || self.semantic_ids.len() != pixels
            || self.depth.len() != pixels
            || self.coverage.len() != pixels
        {
            return Err(Error::InvalidFrame);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct RenderRequest<'a> {
    pub scene: &'a SceneBundle,
    pub resources: Arc<dyn ResourceProvider>,
}

pub trait Renderer {
    fn target(&self) -> TargetDescriptor;
    /// Changes logical target dimensions without changing target ownership.
    ///
    /// # Errors
    /// Returns a typed backend or target error when resizing cannot complete.
    fn resize(&mut self, width: u32, height: u32) -> Result<(), Error>;
    /// Renders exactly one immutable scene revision.
    ///
    /// # Errors
    /// Returns a typed backend, surface, resource, or validation error.
    fn render(&mut self, request: RenderRequest<'_>) -> Result<FrameOutput, Error>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidTarget,
    InvalidFrame,
    LimitExceeded,
    BackendUnavailable(String),
    SurfaceUnavailable,
    SurfaceLost,
    DeviceLost,
    Resource(String),
    Internal(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "renderer error: {self:?}")
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::{
        BackendPreference, Error, FrameOutput, RecoveryMetrics, TargetDescriptor, TargetKind,
        TargetLimits,
    };

    #[test]
    fn frame_planes_must_match_dimensions() {
        let frame = FrameOutput {
            width: 1,
            height: 1,
            rgba8: vec![0; 4],
            semantic_ids: vec![0],
            depth: vec![0],
            coverage: vec![0],
            metrics: RecoveryMetrics::default(),
        };
        assert_eq!(frame.validate(), Ok(()));
        let mut invalid = frame;
        invalid.rgba8.pop();
        assert_eq!(invalid.validate(), Err(Error::InvalidFrame));
    }

    #[test]
    fn target_pixel_budget_is_independent_of_axis_limit() {
        let target = TargetDescriptor {
            kind: TargetKind::Offscreen,
            width: 8_192,
            height: 8_192,
            backend: BackendPreference::Automatic,
        };
        assert_eq!(
            target.validate(TargetLimits::default()),
            Err(Error::InvalidTarget)
        );
    }
}
