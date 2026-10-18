// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

use std::{collections::HashSet, fmt};

pub const SCENE_BUNDLE_VERSION: u32 = 1;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub scale_milli: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SceneLimits {
    pub maximum_sprites: usize,
    pub maximum_dimension: u32,
    pub maximum_pixels: u64,
}

impl Default for SceneLimits {
    fn default() -> Self {
        Self {
            maximum_sprites: 65_536,
            maximum_dimension: 8_192,
            maximum_pixels: 16_777_216,
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
    sprites: Vec<Sprite>,
}

impl SceneBundle {
    pub fn new(
        revision: u64,
        clock_millis: u64,
        viewport: Viewport,
        clear_color: [f32; 4],
        sprites: impl IntoIterator<Item = Sprite>,
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
        for sprite in sprites {
            if values.len() >= limits.maximum_sprites {
                return Err(Error::LimitExceeded);
            }
            validate_sprite(&sprite, limits.maximum_dimension)?;
            values.push(sprite);
        }
        let mut semantic_ids = HashSet::with_capacity(values.len());
        if values
            .iter()
            .any(|sprite| !semantic_ids.insert(sprite.semantic_id))
        {
            return Err(Error::DuplicateSemanticId);
        }
        values.sort_by_key(|sprite| {
            (
                sprite.layer,
                sprite.depth,
                sprite.painter_order,
                sprite.semantic_id,
            )
        });
        Ok(Self {
            version: SCENE_BUNDLE_VERSION,
            revision,
            clock_millis,
            viewport,
            clear_color,
            sprites: values,
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
    pub fn sprites(&self) -> &[Sprite] {
        &self.sprites
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

fn validate_sprite(sprite: &Sprite, maximum_dimension: u32) -> Result<(), Error> {
    let maximum = maximum_dimension as f32;
    if sprite.semantic_id.0 == 0
        || sprite.resource_id.0 == 0
        || sprite.resource_revision == 0
        || sprite.resource_digest.0.iter().all(|byte| *byte == 0)
        || !sprite.x.is_finite()
        || !sprite.y.is_finite()
        || !sprite.width.is_finite()
        || !sprite.height.is_finite()
        || sprite.width <= 0.0
        || sprite.height <= 0.0
        || sprite.width > maximum
        || sprite.height > maximum
        || sprite.x.abs() > maximum * 2.0
        || sprite.y.abs() > maximum * 2.0
        || !valid_color(sprite.color)
    {
        return Err(Error::InvalidValue);
    }
    Ok(())
}

fn valid_color(color: [f32; 4]) -> bool {
    color
        .iter()
        .all(|channel| channel.is_finite() && (0.0..=1.0).contains(channel))
}

#[cfg(test)]
mod tests {
    use super::{
        Digest256, Layer, ResourceId, Sampling, SceneBundle, SceneLimits, SemanticId, Sprite,
        Viewport,
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

    #[test]
    fn validates_bounds_and_orders_scene_deterministically() {
        let scene = SceneBundle::new(
            1,
            0,
            Viewport {
                width: 64,
                height: 64,
                scale_milli: 1000,
            },
            [0.0, 0.0, 0.0, 1.0],
            [sprite(2, 1), sprite(1, 0)],
            SceneLimits::default(),
        )
        .unwrap();
        assert_eq!(scene.sprites()[0].painter_order, 1);
    }

    #[test]
    fn rejects_duplicate_semantic_identity() {
        let value = sprite(0, 0);
        let error = SceneBundle::new(
            1,
            0,
            Viewport {
                width: 64,
                height: 64,
                scale_milli: 1000,
            },
            [0.0, 0.0, 0.0, 1.0],
            [value, value],
            SceneLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error, super::Error::DuplicateSemanticId);
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
}
