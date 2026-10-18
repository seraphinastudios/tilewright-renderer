// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

//! Renderer-neutral, bounded, ordered scene contracts.
//!
//! A [`SceneBundle`] is an immutable, validated list of textured [`Quad`]s in
//! logical pixel space. [`Sprite`] is a convenience for axis-aligned quads.

use std::{collections::HashSet, fmt};

/// Version of the logical scene contract. Version 2 replaced sprites with
/// arbitrary textured quads carrying effect parameters.
pub const SCENE_BUNDLE_VERSION: u32 = 2;

/// Largest accepted texture-coordinate magnitude. Coordinates outside 0–1
/// are allowed so geometry can extend past an image (glow halos, bilinear
/// ground meshes); those samples are transparent.
pub const MAXIMUM_UV_MAGNITUDE: f32 = 16.0;

/// Texture samples a glow quad with a nonzero radius takes per covered
/// pixel: its centre plus 16 directions at full and half radius. Other quads,
/// including zero-radius glows, take one.
pub const GLOW_SAMPLES_PER_PIXEL: u64 = 33;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ResourceId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Digest256(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sampling {
    Nearest,
    Linear,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Layer {
    World,
    Effect,
    Interface,
}

/// An axis-aligned textured rectangle. Converted into a [`Quad`] whose
/// corners run top-left, top-right, bottom-right, bottom-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprite {
    pub semantic_id: SemanticId,
    pub resource_id: ResourceId,
    pub resource_revision: u64,
    pub resource_digest: Digest256,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub depth: i16,
    pub painter_order: u32,
    pub color: [f32; 4],
    pub sampling: Sampling,
    pub layer: Layer,
}

/// A coloured halo around the texture's silhouette. A quad with a glow draws
/// only the halo; its texture supplies the silhouette. Halos are never
/// pickable, so a glow quad must have `pickable: false`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glow {
    /// Halo colour; alpha is the halo strength.
    pub color: [f32; 4],
    /// Halo radius in texture coordinates, per axis.
    pub radius_uv: [f32; 2],
}

/// A textured quadrilateral split into the triangles (0, 1, 2) and (0, 2, 3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    /// Picking identity. Several quads of one object share it.
    pub semantic_id: SemanticId,
    /// Distinguishes quads that share a semantic identity.
    pub part: u16,
    pub resource_id: ResourceId,
    pub resource_revision: u64,
    pub resource_digest: Digest256,
    /// Corner positions in logical pixels.
    pub positions: [[f32; 2]; 4],
    /// Corner texture coordinates.
    pub uvs: [[f32; 2]; 4],
    pub depth: i16,
    pub painter_order: u32,
    /// Multiplied with the texture; alpha scales opacity.
    pub color: [f32; 4],
    /// 0 keeps colour, 1 is fully grey.
    pub grayscale: f32,
    pub glow: Option<Glow>,
    /// Whether this quad writes the semantic, depth and coverage planes.
    /// Glow quads must not be pickable.
    pub pickable: bool,
    pub sampling: Sampling,
    pub layer: Layer,
}

impl From<Sprite> for Quad {
    fn from(sprite: Sprite) -> Self {
        let left = sprite.x;
        let top = sprite.y;
        let right = sprite.x + sprite.width;
        let bottom = sprite.y + sprite.height;
        Self {
            semantic_id: sprite.semantic_id,
            part: 0,
            resource_id: sprite.resource_id,
            resource_revision: sprite.resource_revision,
            resource_digest: sprite.resource_digest,
            positions: [[left, top], [right, top], [right, bottom], [left, bottom]],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            depth: sprite.depth,
            painter_order: sprite.painter_order,
            color: sprite.color,
            grayscale: 0.0,
            glow: None,
            pickable: true,
            sampling: sprite.sampling,
            layer: sprite.layer,
        }
    }
}

/// An axis-aligned rectangle with linearly mapped texture coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlignedRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
}

impl Quad {
    /// Returns the rectangle when the quad is axis aligned, in top-left,
    /// top-right, bottom-right, bottom-left order, with texture coordinates
    /// aligned the same way.
    #[must_use]
    #[allow(clippy::float_cmp)]
    pub fn aligned_rect(&self) -> Option<AlignedRect> {
        let [p0, p1, p2, p3] = self.positions;
        let [t0, t1, t2, t3] = self.uvs;
        let aligned = p0[1] == p1[1]
            && p1[0] == p2[0]
            && p2[1] == p3[1]
            && p3[0] == p0[0]
            && p0[0] < p1[0]
            && p0[1] < p3[1]
            && t0[1] == t1[1]
            && t1[0] == t2[0]
            && t2[1] == t3[1]
            && t3[0] == t0[0];
        aligned.then_some(AlignedRect {
            left: p0[0],
            top: p0[1],
            right: p2[0],
            bottom: p2[1],
            uv_min: t0,
            uv_max: t2,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub scale_milli: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SceneLimits {
    pub maximum_quads: usize,
    pub maximum_dimension: u32,
    pub maximum_pixels: u64,
    /// Most texture samples drawing the scene may take: the sum over quads of
    /// each quad's viewport-clipped bounding box in pixels, times
    /// [`GLOW_SAMPLES_PER_PIXEL`] for glow quads with a nonzero radius. It
    /// bounds overdraw and halo work on every backend.
    pub maximum_raster_samples: u64,
}

impl Default for SceneLimits {
    fn default() -> Self {
        Self {
            maximum_quads: 131_072,
            maximum_dimension: 8_192,
            maximum_pixels: 16_777_216,
            // Sixteen layers of the largest viewport.
            maximum_raster_samples: 268_435_456,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneBundle {
    version: u32,
    revision: u64,
    clock_millis: u64,
    viewport: Viewport,
    clear_color: [f32; 4],
    quads: Vec<Quad>,
}

impl SceneBundle {
    /// Builds a scene from axis-aligned sprites.
    ///
    /// # Errors
    /// Returns a validation error as [`SceneBundle::from_quads`] does.
    pub fn new(
        revision: u64,
        clock_millis: u64,
        viewport: Viewport,
        clear_color: [f32; 4],
        sprites: impl IntoIterator<Item = Sprite>,
        limits: SceneLimits,
    ) -> Result<Self, Error> {
        Self::from_quads(
            revision,
            clock_millis,
            viewport,
            clear_color,
            sprites.into_iter().map(Quad::from),
            limits,
        )
    }

    /// Builds a scene from quads, validating every value and ordering them
    /// stably by layer, depth, painter order, semantic identity and part.
    ///
    /// # Errors
    /// Returns [`Error::InvalidValue`], [`Error::LimitExceeded`] (also when
    /// the quads need more than [`SceneLimits::maximum_raster_samples`]) or
    /// [`Error::DuplicateSemanticId`] for a repeated identity and part.
    pub fn from_quads(
        revision: u64,
        clock_millis: u64,
        viewport: Viewport,
        clear_color: [f32; 4],
        quads: impl IntoIterator<Item = Quad>,
        limits: SceneLimits,
    ) -> Result<Self, Error> {
        if revision == 0
            || viewport.width == 0
            || viewport.height == 0
            || viewport.width > limits.maximum_dimension
            || viewport.height > limits.maximum_dimension
            || u64::from(viewport.width) * u64::from(viewport.height) > limits.maximum_pixels
            || viewport.scale_milli == 0
            || viewport.scale_milli > 8000
            || !valid_color(clear_color)
        {
            return Err(Error::InvalidValue);
        }
        let mut values = Vec::new();
        let mut samples = 0_u64;
        for quad in quads {
            if values.len() >= limits.maximum_quads {
                return Err(Error::LimitExceeded);
            }
            validate_quad(&quad, limits.maximum_dimension)?;
            samples = samples.saturating_add(raster_samples(&quad, viewport));
            if samples > limits.maximum_raster_samples {
                return Err(Error::LimitExceeded);
            }
            values.push(quad);
        }
        let mut identities = HashSet::with_capacity(values.len());
        if values
            .iter()
            .any(|quad| !identities.insert((quad.semantic_id, quad.part)))
        {
            return Err(Error::DuplicateSemanticId);
        }
        values.sort_by_key(|quad| {
            (
                quad.layer,
                quad.depth,
                quad.painter_order,
                quad.semantic_id,
                quad.part,
            )
        });
        Ok(Self {
            version: SCENE_BUNDLE_VERSION,
            revision,
            clock_millis,
            viewport,
            clear_color,
            quads: values,
        })
    }

    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn clock_millis(&self) -> u64 {
        self.clock_millis
    }

    #[must_use]
    pub const fn viewport(&self) -> Viewport {
        self.viewport
    }

    #[must_use]
    pub const fn clear_color(&self) -> [f32; 4] {
        self.clear_color
    }

    #[must_use]
    pub fn quads(&self) -> &[Quad] {
        &self.quads
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidValue,
    LimitExceeded,
    DuplicateSemanticId,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid renderer scene: {self:?}")
    }
}

impl std::error::Error for Error {}

fn validate_quad(quad: &Quad, maximum_dimension: u32) -> Result<(), Error> {
    let maximum = maximum_dimension as f32;
    let position_ok = |point: &[f32; 2]| {
        point
            .iter()
            .all(|value| value.is_finite() && value.abs() <= maximum * 2.0)
    };
    let uv_ok = |point: &[f32; 2]| {
        point
            .iter()
            .all(|value| value.is_finite() && value.abs() <= MAXIMUM_UV_MAGNITUDE)
    };
    let glow_ok = quad.glow.is_none_or(|glow| {
        !quad.pickable
            && valid_color(glow.color)
            && glow
                .radius_uv
                .iter()
                .all(|radius| radius.is_finite() && (0.0..=1.0).contains(radius))
    });
    if quad.semantic_id.0 == 0
        || quad.resource_id.0 == 0
        || quad.resource_revision == 0
        || quad.resource_digest.0.iter().all(|byte| *byte == 0)
        || !quad.positions.iter().all(position_ok)
        || !quad.uvs.iter().all(uv_ok)
        || !valid_color(quad.color)
        || !quad.grayscale.is_finite()
        || !(0.0..=1.0).contains(&quad.grayscale)
        || !glow_ok
    {
        return Err(Error::InvalidValue);
    }
    let (min_x, max_x, min_y, max_y) = bounds(quad);
    if max_x - min_x <= 0.0
        || max_y - min_y <= 0.0
        || max_x - min_x > maximum
        || max_y - min_y > maximum
    {
        return Err(Error::InvalidValue);
    }
    Ok(())
}

fn bounds(quad: &Quad) -> (f32, f32, f32, f32) {
    quad.positions.iter().fold(
        (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
        |(min_x, max_x, min_y, max_y), point| {
            (
                min_x.min(point[0]),
                max_x.max(point[0]),
                min_y.min(point[1]),
                max_y.max(point[1]),
            )
        },
    )
}

/// Texture samples a validated quad can take: its bounding box clipped to
/// the viewport in whole pixels, times its samples per pixel.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn raster_samples(quad: &Quad, viewport: Viewport) -> u64 {
    let (min_x, max_x, min_y, max_y) = bounds(quad);
    let span = |low: f32, high: f32, size: u32| {
        let low = f64::from(low).floor().max(0.0);
        let high = f64::from(high).ceil().min(f64::from(size));
        if high > low { (high - low) as u64 } else { 0 }
    };
    let per_pixel = match quad.glow {
        Some(glow) if glow.radius_uv.iter().any(|radius| *radius > 0.0) => GLOW_SAMPLES_PER_PIXEL,
        _ => 1,
    };
    span(min_x, max_x, viewport.width) * span(min_y, max_y, viewport.height) * per_pixel
}

fn valid_color(color: [f32; 4]) -> bool {
    color
        .iter()
        .all(|channel| channel.is_finite() && (0.0..=1.0).contains(channel))
}

#[cfg(test)]
mod tests {
    use super::{
        Digest256, Glow, Layer, Quad, ResourceId, Sampling, SceneBundle, SceneLimits, SemanticId,
        Sprite, Viewport,
    };

    fn sprite(order: u32, depth: i16) -> Sprite {
        Sprite {
            semantic_id: SemanticId(order + 1),
            resource_id: ResourceId(1),
            resource_revision: 1,
            resource_digest: Digest256([1; 32]),
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            depth,
            painter_order: order,
            color: [1.0; 4],
            sampling: Sampling::Nearest,
            layer: Layer::World,
        }
    }

    fn viewport() -> Viewport {
        Viewport {
            width: 64,
            height: 64,
            scale_milli: 1000,
        }
    }

    #[test]
    fn validates_bounds_and_orders_scene_deterministically() {
        let scene = SceneBundle::new(
            1,
            0,
            viewport(),
            [0.0, 0.0, 0.0, 1.0],
            [sprite(2, 1), sprite(1, 0)],
            SceneLimits::default(),
        )
        .unwrap();
        assert_eq!(scene.version(), 2);
        assert_eq!(scene.quads()[0].painter_order, 1);
        assert!(scene.quads()[0].aligned_rect().is_some());
    }

    #[test]
    fn rejects_duplicate_semantic_identity_and_part() {
        let value = sprite(0, 0);
        let error = SceneBundle::new(
            1,
            0,
            viewport(),
            [0.0, 0.0, 0.0, 1.0],
            [value, value],
            SceneLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error, super::Error::DuplicateSemanticId);
        let first = Quad::from(value);
        let second = Quad { part: 1, ..first };
        assert!(
            SceneBundle::from_quads(
                1,
                0,
                viewport(),
                [0.0; 4],
                [first, second],
                SceneLimits::default()
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_viewport_above_total_pixel_budget() {
        let error = SceneBundle::new(
            1,
            0,
            Viewport {
                width: 8_192,
                height: 8_192,
                scale_milli: 1000,
            },
            [0.0, 0.0, 0.0, 1.0],
            [],
            SceneLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error, super::Error::InvalidValue);
    }

    #[test]
    fn rejects_invalid_quad_values() {
        let base = Quad::from(sprite(0, 0));
        let cases = [
            Quad {
                grayscale: 1.5,
                ..base
            },
            Quad {
                uvs: [[0.0, 0.0], [17.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                ..base
            },
            Quad {
                positions: [[0.0, 0.0]; 4],
                ..base
            },
            Quad {
                glow: Some(Glow {
                    color: [1.0; 4],
                    radius_uv: [2.0, 0.1],
                }),
                ..base
            },
            Quad {
                positions: [[f32::NAN, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                ..base
            },
            // Halos are never pickable on any backend.
            Quad {
                glow: Some(Glow {
                    color: [1.0; 4],
                    radius_uv: [0.1, 0.1],
                }),
                pickable: true,
                ..base
            },
        ];
        for quad in cases {
            assert_eq!(
                SceneBundle::from_quads(1, 0, viewport(), [0.0; 4], [quad], SceneLimits::default()),
                Err(super::Error::InvalidValue)
            );
        }
        let halo = Quad {
            glow: Some(Glow {
                color: [1.0; 4],
                radius_uv: [0.1, 0.1],
            }),
            pickable: false,
            ..base
        };
        assert!(
            SceneBundle::from_quads(1, 0, viewport(), [0.0; 4], [halo], SceneLimits::default())
                .is_ok()
        );
    }

    #[test]
    fn raster_samples_bound_clipped_overdraw_and_halos() {
        let limits = SceneLimits {
            maximum_raster_samples: 64 * 64 * 4,
            ..SceneLimits::default()
        };
        let full = |id: u32| {
            Quad::from(Sprite {
                x: -32.0,
                y: -32.0,
                width: 128.0,
                height: 128.0,
                ..sprite(id, 0)
            })
        };
        // Each quad counts only the 64 × 64 pixels inside the viewport.
        let layers: Vec<Quad> = (0..4).map(full).collect();
        assert!(
            SceneBundle::from_quads(1, 0, viewport(), [0.0; 4], layers.clone(), limits).is_ok()
        );
        let mut more = layers;
        more.push(full(4));
        assert_eq!(
            SceneBundle::from_quads(1, 0, viewport(), [0.0; 4], more, limits),
            Err(super::Error::LimitExceeded)
        );
        // A halo samples 33 times per pixel unless its radius is zero.
        let halo = |radius: f32| Quad {
            glow: Some(Glow {
                color: [1.0; 4],
                radius_uv: [radius, radius],
            }),
            pickable: false,
            positions: [[0.0, 0.0], [8.0, 0.0], [8.0, 8.0], [0.0, 8.0]],
            ..full(1)
        };
        let budget = |samples| SceneLimits {
            maximum_raster_samples: samples,
            ..SceneLimits::default()
        };
        let render = |quad, samples| {
            SceneBundle::from_quads(1, 0, viewport(), [0.0; 4], [quad], budget(samples))
        };
        assert!(render(halo(0.1), 64 * 33).is_ok());
        assert_eq!(
            render(halo(0.1), 64 * 33 - 1),
            Err(super::Error::LimitExceeded)
        );
        assert!(render(halo(0.0), 64).is_ok());
    }

    #[test]
    fn skewed_quads_are_not_aligned_rectangles() {
        let mut quad = Quad::from(sprite(0, 0));
        quad.positions[1][1] = 0.5;
        assert!(quad.aligned_rect().is_none());
    }
}
