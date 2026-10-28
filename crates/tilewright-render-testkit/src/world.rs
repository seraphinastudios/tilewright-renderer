// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! Procedural textures and world views for the conformance corpus. Every
//! texture is generated from constants; no game art is involved.

use crate::{FakeResourceProvider, texture_envelope};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tilewright_render_resources::Error as ResourceError;
use tilewright_render_world::{
    Anchor, Animation, Camera, GlowEffect, HeightField, Projection, Stretch, TextureRef,
    WorldObject, WorldView, compile,
};
use tilewright_scene::{Digest256, ResourceId, Sampling, SceneBundle, SceneLimits, SemanticId};

const GRASS: u64 = 101;
const SAND: u64 = 102;
const PILLAR: u64 = 103;
const ORB: u64 = 104;
const MEADOW: u64 = 105;
const STONE: u64 = 106;
const GROUND_GRID: u64 = 201;

fn atx1(width: u32, height: u32, pixels: &[[u8; 4]]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(12 + pixels.len() * 4);
    bytes.extend_from_slice(b"ATX1");
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    for pixel in pixels {
        bytes.extend_from_slice(pixel);
    }
    bytes
}

/// A 48 × 23 isometric diamond: rows widen by 4 px to 48 at row 11.
fn diamond(primary: [u8; 4], secondary: [u8; 4]) -> Vec<u8> {
    let mut pixels = vec![[0_u8; 4]; 48 * 23];
    for row in 0..23_usize {
        let half = 2 + 2 * row.min(22 - row);
        for column in (24 - half)..(24 + half) {
            let checker = (column / 6 + row / 3) % 2 == 0;
            pixels[row * 48 + column] = if checker { primary } else { secondary };
        }
    }
    atx1(48, 23, &pixels)
}

fn pillar() -> Vec<u8> {
    let mut pixels = vec![[0_u8; 4]; 16 * 40];
    for row in 2..40_usize {
        for column in 3..13_usize {
            let shade = u8::try_from(80 + row * 4).unwrap_or(u8::MAX);
            pixels[row * 16 + column] = [shade, shade / 2, 200, 255];
        }
    }
    atx1(16, 40, &pixels)
}

fn orb() -> Vec<u8> {
    let mut pixels = vec![[0_u8; 4]; 10 * 10];
    for row in 0..10_i32 {
        for column in 0..10_i32 {
            let dx = 2 * column - 9;
            let dy = 2 * row - 9;
            if dx * dx + dy * dy <= 64 {
                pixels[usize::try_from(row * 10 + column).unwrap_or(0)] = [250, 220, 90, 255];
            }
        }
    }
    atx1(10, 10, &pixels)
}

fn square(primary: [u8; 4], border: [u8; 4]) -> Vec<u8> {
    let mut pixels = vec![primary; 16 * 16];
    for index in 0..16 {
        pixels[index] = border;
        pixels[index * 16] = border;
    }
    atx1(16, 16, &pixels)
}

fn textures() -> [(u64, u32, u32, Vec<u8>); 6] {
    [
        (
            GRASS,
            48,
            23,
            diamond([70, 150, 60, 255], [60, 130, 55, 255]),
        ),
        (
            SAND,
            48,
            23,
            diamond([210, 190, 120, 255], [190, 170, 105, 255]),
        ),
        (PILLAR, 16, 40, pillar()),
        (ORB, 10, 10, orb()),
        (
            MEADOW,
            16,
            16,
            square([90, 160, 70, 255], [70, 130, 60, 255]),
        ),
        (
            STONE,
            16,
            16,
            square([130, 130, 140, 255], [100, 100, 110, 255]),
        ),
    ]
}

/// The texture identity of a corpus world texture.
#[must_use]
pub fn world_texture(id: u64) -> TextureRef {
    let (_, width, height, bytes) = textures()
        .into_iter()
        .find(|texture| texture.0 == id)
        .unwrap_or_else(|| (0, 1, 1, Vec::new()));
    TextureRef {
        id: ResourceId(id),
        revision: 1,
        digest: Digest256(Sha256::digest(&bytes).into()),
        width,
        height,
    }
}

/// A provider holding every corpus world texture.
///
/// # Errors
/// Returns a provider availability error if its internal lock is poisoned.
pub fn world_provider() -> Result<Arc<FakeResourceProvider>, ResourceError> {
    let provider = Arc::new(FakeResourceProvider::default());
    for (id, _, _, bytes) in textures() {
        provider.insert(ResourceId(id), 1, bytes)?;
    }
    Ok(provider)
}

fn compiled(view: &WorldView) -> Result<SceneBundle, tilewright_scene::Error> {
    compile(view, SceneLimits::default()).map_err(|error| match error {
        tilewright_render_world::Error::Scene(error) => error,
        tilewright_render_world::Error::InvalidValue => tilewright_scene::Error::InvalidValue,
    })
}

/// An isometric 6 × 6 hill with stretched terrain, a tall pillar drawn twice,
/// a pulsing glow, tint and greyscale, and a rotated object.
///
/// # Errors
/// Returns a scene validation error for dimensions outside scene bounds.
pub fn iso_world_scene(width: u32, height: u32) -> Result<SceneBundle, tilewright_scene::Error> {
    let size = 6_i64;
    let mut heights = Vec::new();
    for row in 0..size {
        for column in 0..size {
            let distance = (row - 2).abs().max((column - 3).abs());
            heights.push(Some(i32::try_from((3 - distance).max(0) * 8).unwrap_or(0)));
        }
    }
    let mut objects = Vec::new();
    let mut next = 1_u32;
    for row in 0..size {
        for column in 0..size {
            let texture = if (row + column) % 3 == 0 { SAND } else { GRASS };
            objects.push(WorldObject::new(
                SemanticId(next),
                [column, row],
                1,
                Anchor::Ground,
                world_texture(texture),
            ));
            next += 1;
        }
    }
    let mut tall = WorldObject::new(
        SemanticId(next),
        [1, 4],
        3,
        Anchor::Standing,
        world_texture(PILLAR),
    );
    tall.double_draw = true;
    let mut lamp = WorldObject::new(
        SemanticId(next + 1),
        [4, 1],
        3,
        Anchor::Standing,
        world_texture(ORB),
    );
    lamp.z = 6;
    lamp.glow = Some(GlowEffect {
        color: [1.0, 0.6, 0.1, 0.9],
        radius_px: 3.0,
        pulse_millis: 1_000,
    });
    let mut summit = WorldObject::new(
        SemanticId(next + 2),
        [3, 2],
        3,
        Anchor::Standing,
        world_texture(PILLAR),
    );
    summit.tint = [0.6, 1.0, 0.8];
    summit.grayscale = 0.5;
    summit.alpha = 0.8;
    let mut spun = WorldObject::new(
        SemanticId(next + 3),
        [2, 4],
        3,
        Anchor::Standing,
        world_texture(ORB),
    );
    spun.rotation_degrees = 30.0;
    spun.scale = [2.0, 1.5];
    objects.extend([tall, lamp, summit, spun]);
    compiled(&WorldView {
        revision: 1,
        clock_millis: 250,
        camera: Camera {
            projection: Projection::Isometric {
                tile_width: 48,
                tile_height: 24,
            },
            center: [3.0, 3.0],
            zoom: 1.0,
            width,
            height,
            sampling: Sampling::Nearest,
        },
        clear_color: [0.05, 0.05, 0.08, 1.0],
        heights: HeightField {
            origin: [0, 0],
            width: 6,
            height: 6,
            heights,
        },
        stretch: Stretch::default(),
        ground_layer_ranks: 3,
        objects,
    })
}

/// A top-down 6 × 4 field with standing art, an animated object and a glow.
///
/// # Errors
/// Returns a scene validation error for dimensions outside scene bounds.
pub fn top_down_world_scene(
    width: u32,
    height: u32,
) -> Result<SceneBundle, tilewright_scene::Error> {
    let mut objects = Vec::new();
    let mut next = 1_u32;
    for row in 0..4_i64 {
        for column in 0..6_i64 {
            let texture = if row == 2 { STONE } else { MEADOW };
            objects.push(WorldObject::new(
                SemanticId(next),
                [column, row],
                0,
                Anchor::Ground,
                world_texture(texture),
            ));
            next += 1;
        }
    }
    let mut walker = WorldObject::new(
        SemanticId(next),
        [2, 1],
        3,
        Anchor::Standing,
        world_texture(PILLAR),
    );
    walker.animation = Animation {
        frames: vec![world_texture(PILLAR), world_texture(ORB)],
        frame_millis: 200,
        phase_millis: 0,
    };
    let mut lamp = WorldObject::new(
        SemanticId(next + 1),
        [4, 3],
        3,
        Anchor::Standing,
        world_texture(ORB),
    );
    lamp.glow = Some(GlowEffect {
        color: [0.3, 0.6, 1.0, 1.0],
        radius_px: 2.0,
        pulse_millis: 0,
    });
    let behind = WorldObject::new(
        SemanticId(next + 2),
        [2, 2],
        3,
        Anchor::Standing,
        world_texture(PILLAR),
    );
    objects.extend([walker, lamp, behind]);
    compiled(&WorldView {
        revision: 1,
        clock_millis: 300,
        camera: Camera {
            projection: Projection::TopDown { tile_size: 16 },
            center: [3.0, 2.0],
            zoom: 1.0,
            width,
            height,
            sampling: Sampling::Nearest,
        },
        clear_color: [0.0, 0.0, 0.0, 1.0],
        heights: HeightField::empty(),
        stretch: Stretch {
            enabled: false,
            ..Stretch::default()
        },
        ground_layer_ranks: 3,
        objects,
    })
}

/// Art for [`ground_grid_view`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroundArt {
    /// Opaque art filling its whole quad: the tile square in top-down
    /// projection, or the isometric tile's bounding rectangle.
    Full,
    /// An opaque isometric diamond one pixel shorter than its tile, the usual
    /// shape of isometric ground art. Rows widen by four texels from the
    /// tips to the full width at the middle, so the diamonds of tiles twice
    /// as wide as they are tall interlock exactly. Top-down tiles draw
    /// [`GroundArt::Full`] instead.
    Diamond,
}

/// The ground grid's texture: its size and `ATX1` envelope with digest.
#[allow(clippy::cast_possible_truncation)]
fn ground_grid_texture(projection: Projection, art: GroundArt) -> (u32, u32, Vec<u8>, Digest256) {
    let (width, height, diamond) = match projection {
        Projection::TopDown { tile_size } => (tile_size, tile_size, false),
        Projection::Isometric {
            tile_width,
            tile_height,
        } => match art {
            GroundArt::Full => (tile_width, tile_height, false),
            GroundArt::Diamond => (tile_width, tile_height.saturating_sub(1).max(1), true),
        },
    };
    let (columns, rows) = (width as usize, height as usize);
    let mut pixels = vec![[0_u8; 4]; columns * rows];
    for row in 0..rows {
        let half = if diamond {
            (2 + 2 * row.min(rows - 1 - row)).min(columns / 2)
        } else {
            columns
        };
        for column in 0..columns {
            if column + half >= columns / 2 && column < columns / 2 + half {
                pixels[row * columns + column] = [170, 140, 90, 255];
            }
        }
    }
    let (bytes, digest) = texture_envelope(width, height, &pixels);
    (width, height, bytes, digest)
}

/// A provider holding the texture [`ground_grid_view`] draws for this
/// projection and art.
///
/// # Errors
/// Returns a provider availability error if its internal lock is poisoned.
pub fn ground_grid_provider(
    projection: Projection,
    art: GroundArt,
) -> Result<Arc<FakeResourceProvider>, ResourceError> {
    let provider = Arc::new(FakeResourceProvider::default());
    let (_, _, bytes, _) = ground_grid_texture(projection, art);
    provider.insert(ResourceId(GROUND_GRID), 1, bytes)?;
    Ok(provider)
}

/// Flat ground tiles covering the camera's whole view and two tiles past
/// it, all showing one opaque texture: a surface with no holes, for seam
/// and coverage checks. Semantic IDs count from 1 along each row of tiles,
/// rows from the lowest `y`.
///
/// # Errors
/// Returns [`tilewright_scene::Error::LimitExceeded`], before allocating
/// anything, when the view needs more tiles than the default scene quad
/// budget, or for a zoom at which the view has no finite extent.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
pub fn ground_grid_view(
    camera: Camera,
    art: GroundArt,
) -> Result<WorldView, tilewright_scene::Error> {
    let (width, height, _, digest) = ground_grid_texture(camera.projection, art);
    let texture = TextureRef {
        id: ResourceId(GROUND_GRID),
        revision: 1,
        digest,
        width,
        height,
    };
    // The view's corners in tile coordinates bound the tiles in view.
    let zoom = f64::from(camera.zoom);
    let tile = |x: u32, y: u32| -> [f64; 2] {
        let dx = (f64::from(x) - f64::from(camera.width) / 2.0) / zoom;
        let dy = (f64::from(y) - f64::from(camera.height) / 2.0) / zoom;
        let [across, down] = match camera.projection {
            Projection::TopDown { tile_size } => {
                let size = f64::from(tile_size);
                return [camera.center[0] + dx / size, camera.center[1] + dy / size];
            }
            Projection::Isometric {
                tile_width,
                tile_height,
            } => [
                2.0 * dx / f64::from(tile_width),
                2.0 * dy / f64::from(tile_height),
            ],
        };
        [
            camera.center[0] + f64::midpoint(across, down),
            camera.center[1] + (down - across) / 2.0,
        ]
    };
    let corners = [
        tile(0, 0),
        tile(camera.width, 0),
        tile(0, camera.height),
        tile(camera.width, camera.height),
    ];
    let along = |axis: usize| corners.iter().map(move |corner| corner[axis]);
    let low = |axis: usize| along(axis).fold(f64::INFINITY, f64::min).floor() - 2.0;
    let high = |axis: usize| along(axis).fold(f64::NEG_INFINITY, f64::max).ceil() + 2.0;
    let [left, top, right, bottom] = [low(0), low(1), high(0), high(1)];
    let tiles = (right - left + 1.0) * (bottom - top + 1.0);
    if !(tiles.is_finite() && tiles <= SceneLimits::default().maximum_quads as f64) {
        return Err(tilewright_scene::Error::LimitExceeded);
    }
    let mut objects = Vec::with_capacity(tiles as usize);
    for y in top as i64..=bottom as i64 {
        for x in left as i64..=right as i64 {
            let id = u32::try_from(objects.len() + 1).unwrap_or(u32::MAX);
            objects.push(WorldObject::new(
                SemanticId(id),
                [x, y],
                0,
                Anchor::Ground,
                texture,
            ));
        }
    }
    Ok(WorldView {
        revision: 1,
        clock_millis: 0,
        camera,
        clear_color: [0.0, 0.0, 0.0, 1.0],
        heights: HeightField::empty(),
        stretch: Stretch::default(),
        ground_layer_ranks: 1,
        objects,
    })
}

/// [`ground_grid_view`] compiled into a scene.
///
/// # Errors
/// Returns the view's error, or a scene validation error for a camera
/// outside scene bounds.
pub fn ground_grid_scene(
    camera: Camera,
    art: GroundArt,
) -> Result<SceneBundle, tilewright_scene::Error> {
    compiled(&ground_grid_view(camera, art)?)
}
