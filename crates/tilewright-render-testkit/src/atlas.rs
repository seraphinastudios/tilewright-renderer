// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! A procedural sprite-sheet atlas and a scene of layered characters drawn
//! from its frames. Every texel is generated from constants; no game or
//! third-party art is involved.

use crate::FakeResourceProvider;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tilewright_render_resources::Error as ResourceError;
use tilewright_scene::{
    Digest256, Glow, Layer, Quad, ResourceId, Sampling, SceneBundle, SceneLimits, SemanticId,
    SpriteSheet, TextureRect, Viewport,
};

/// Resource identity of the corpus atlas.
pub const ATLAS_ID: ResourceId = ResourceId(201);
/// Width and height of every atlas frame in texels.
pub const ATLAS_FRAME_SIZE: u32 = 16;
/// The atlas layout: 4 × 4 frames of 16 × 16 texels. Rows 0–2 hold the
/// body, tunic and hair layers of a four-step walk cycle; row 3 holds four
/// opaque ground tiles whose edges touch, so any filtering that leaks past
/// a frame edge changes the picture.
pub const ATLAS_SHEET: SpriteSheet = SpriteSheet::grid(ATLAS_FRAME_SIZE, ATLAS_FRAME_SIZE, 4, 4);

const SIDE: u32 = ATLAS_FRAME_SIZE * 4;
const TUNICS: [[u8; 3]; 4] = [[60, 90, 200], [200, 60, 80], [70, 160, 90], [180, 150, 40]];
const HAIR: [[u8; 3]; 4] = [[90, 60, 30], [30, 30, 35], [200, 170, 90], [150, 70, 40]];
const GROUND: [([u8; 3], [u8; 3]); 4] = [
    ([70, 150, 60], [60, 130, 55]),
    ([210, 190, 120], [190, 170, 105]),
    ([120, 120, 130], [95, 95, 110]),
    ([60, 110, 170], [50, 95, 150]),
];

/// The texel of one frame of the atlas at a frame-relative position.
fn frame_texel(column: u32, row: u32, x: i32, y: i32) -> [u8; 4] {
    let index = usize::try_from(column).unwrap_or(0);
    let opaque = |rgb: [u8; 3]| [rgb[0], rgb[1], rgb[2], 255];
    match row {
        0 => {
            // Body: a head, a torso and legs that swing with the step.
            let phase = [0, 1, 0, -1][index];
            let head = (x - 8) * (x - 8) + (y - 4) * (y - 4) <= 9;
            let torso = (5..11).contains(&x) && (7..12).contains(&y);
            let legs = (12..16).contains(&y)
                && ((5 + phase..7 + phase).contains(&x) || (9 - phase..11 - phase).contains(&x));
            if head || torso || legs {
                opaque([224, 172, 140])
            } else {
                [0; 4]
            }
        }
        1 => {
            // Tunic over the torso, with a darker belt.
            if (4..12).contains(&x) && (7..13).contains(&y) {
                let tunic = TUNICS[index];
                if y == 11 {
                    opaque(tunic.map(|channel| channel / 3))
                } else {
                    opaque(tunic)
                }
            } else {
                [0; 4]
            }
        }
        2 => {
            // Hair over the top of the head, half translucent at its fringe.
            if (4..12).contains(&x) && (0..4).contains(&y) {
                let [red, green, blue] = HAIR[index];
                [red, green, blue, if y == 3 { 128 } else { 255 }]
            } else {
                [0; 4]
            }
        }
        _ => {
            // Ground: a two-tone checker with a bright right and bottom edge.
            let (primary, secondary) = GROUND[index];
            if x == 15 || y == 15 {
                opaque([250, 250, 250])
            } else if (x / 4 + y / 4) % 2 == 0 {
                opaque(primary)
            } else {
                opaque(secondary)
            }
        }
    }
}

/// The atlas as an `ATX1` texture envelope.
#[must_use]
pub fn atlas_resource() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(12 + (SIDE * SIDE * 4) as usize);
    bytes.extend_from_slice(b"ATX1");
    bytes.extend_from_slice(&SIDE.to_le_bytes());
    bytes.extend_from_slice(&SIDE.to_le_bytes());
    for y in 0..SIDE {
        for x in 0..SIDE {
            let texel = frame_texel(
                x / ATLAS_FRAME_SIZE,
                y / ATLAS_FRAME_SIZE,
                i32::try_from(x % ATLAS_FRAME_SIZE).unwrap_or(0),
                i32::try_from(y % ATLAS_FRAME_SIZE).unwrap_or(0),
            );
            bytes.extend_from_slice(&texel);
        }
    }
    bytes
}

/// SHA-256 of [`atlas_resource`].
#[must_use]
pub fn atlas_digest() -> Digest256 {
    Digest256(Sha256::digest(atlas_resource()).into())
}

/// A provider holding the atlas.
///
/// # Errors
/// Returns a provider availability error if its internal lock is poisoned.
pub fn atlas_provider() -> Result<Arc<FakeResourceProvider>, ResourceError> {
    let provider = Arc::new(FakeResourceProvider::default());
    provider.insert(ATLAS_ID, 1, atlas_resource())?;
    Ok(provider)
}

fn frame(column: u32, row: u32) -> TextureRect {
    ATLAS_SHEET
        .frame_at(column, row)
        .unwrap_or(TextureRect::whole(1, 1))
}

/// An atlas quad covering `[left, top, right, bottom]` with full-frame
/// texture coordinates.
fn atlas_quad(semantic: u32, part: u16, frame: TextureRect, bounds: [f32; 4]) -> Quad {
    let [left, top, right, bottom] = bounds;
    Quad {
        semantic_id: SemanticId(semantic),
        part,
        resource_id: ATLAS_ID,
        resource_revision: 1,
        resource_digest: atlas_digest(),
        frame: Some(frame),
        positions: [[left, top], [right, top], [right, bottom], [left, bottom]],
        uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        depth: 0,
        painter_order: 0,
        color: [1.0; 4],
        grayscale: 0.0,
        glow: None,
        pickable: true,
        sampling: Sampling::Nearest,
        layer: Layer::World,
    }
}

/// The body, tunic and hair quads of one character at a walk step, drawn
/// in that order at `[left, top]` and `scale` art pixels per texel.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn character_sheet(
    semantic: u32,
    step: u32,
    origin: [f32; 2],
    scale: f32,
    painter_order: u32,
    sampling: Sampling,
) -> [Quad; 3] {
    let size = ATLAS_FRAME_SIZE as f32 * scale;
    let bounds = [origin[0], origin[1], origin[0] + size, origin[1] + size];
    [0_u16, 1, 2].map(|layer| Quad {
        painter_order: painter_order + u32::from(layer),
        sampling,
        ..atlas_quad(semantic, layer, frame(step % 4, u32::from(layer)), bounds)
    })
}

/// A scene drawn entirely from atlas frames: a ground of abutting tiles,
/// layered characters at each walk step (one magnified 1.5× at a fractional
/// position), a body frame magnified with linear filtering, a glowing hair
/// frame whose halo would catch the opaque ground row below it if sampling
/// leaked, a ground tile magnified with linear filtering, a rotated tile,
/// and a tile whose texture coordinates run past its frame. At least
/// 112 × 64 pixels.
///
/// Two rules keep every backend within one 8-bit step of the reference.
/// No quad edge and no texel boundary falls exactly on a pixel centre, where
/// interpolated texture coordinates may round to either side. And each pixel
/// blends at most one translucent layer, because some drivers (lavapipe
/// among them) round 8-bit blends to within one step, and stacked
/// translucent layers would add those steps up.
///
/// # Errors
/// Returns a scene validation error for dimensions below the fixture
/// contract.
#[allow(clippy::cast_precision_loss)]
pub fn atlas_scene(width: u32, height: u32) -> Result<SceneBundle, tilewright_scene::Error> {
    if width < 112 || height < 64 {
        return Err(tilewright_scene::Error::InvalidValue);
    }
    let mut quads = Vec::new();
    // Ground: 4 × 4 tiles cycling through the four ground frames.
    for row in 0..4_u32 {
        for column in 0..4_u32 {
            let index = row * 4 + column;
            let left = (column * ATLAS_FRAME_SIZE) as f32;
            let top = (row * ATLAS_FRAME_SIZE) as f32;
            quads.push(Quad {
                painter_order: index,
                depth: -1,
                ..atlas_quad(
                    1 + index,
                    0,
                    frame((row + column) % 4, 3),
                    [left, top, left + 16.0, top + 16.0],
                )
            });
        }
    }
    // Four layered characters, one per walk step; the last is magnified
    // 1.5× at a fractional position.
    for step in 0..4_u32 {
        let (origin, scale) = if step == 3 {
            ([40.25, 36.25], 1.5)
        } else {
            ([(8 + step * 16) as f32, (8 + step * 5) as f32], 1.0)
        };
        quads.extend(character_sheet(
            101 + step,
            step,
            origin,
            scale,
            10 + step * 3,
            Sampling::Nearest,
        ));
    }
    // A body frame magnified 1.75× with linear filtering.
    quads.push(Quad {
        sampling: Sampling::Linear,
        painter_order: 30,
        ..atlas_quad(105, 0, frame(1, 0), [2.125, 34.125, 30.125, 62.125])
    });
    // A ground tile magnified 2× with linear filtering: its bright edge must
    // not blend with the neighbouring tile's texels.
    quads.push(Quad {
        sampling: Sampling::Linear,
        layer: Layer::Effect,
        ..atlas_quad(200, 0, frame(1, 3), [66.25, 1.25, 98.25, 33.25])
    });
    // A tile whose coordinates run a quarter frame past each edge: the
    // margin is transparent rather than showing the neighbouring frames.
    quads.push(Quad {
        uvs: [[-0.25, -0.25], [1.25, -0.25], [1.25, 1.25], [-0.25, 1.25]],
        sampling: Sampling::Linear,
        layer: Layer::Effect,
        painter_order: 1,
        color: [1.0, 1.0, 1.0, 0.85],
        ..atlas_quad(201, 0, frame(3, 3), [99.25, 2.25, 111.25, 14.25])
    });
    // A tile rotated by 30 degrees about (80, 48).
    quads.push(Quad {
        positions: [
            [77.07, 37.07],
            [90.93, 45.07],
            [82.93, 58.93],
            [69.07, 50.93],
        ],
        layer: Layer::Effect,
        painter_order: 2,
        ..atlas_quad(202, 0, frame(2, 3), [0.0, 0.0, 1.0, 1.0])
    });
    // A glowing hair frame: the halo quad extends two texels past the frame
    // on each side, and the frame below it is opaque ground, so a halo that
    // sampled past the frame's bottom edge would glow there.
    quads.push(Quad {
        uvs: [
            [-0.125, -0.125],
            [1.125, -0.125],
            [1.125, 1.125],
            [-0.125, 1.125],
        ],
        glow: Some(Glow {
            color: [1.0, 0.85, 0.3, 0.9],
            radius_uv: [0.2, 0.2],
        }),
        pickable: false,
        painter_order: 3,
        ..atlas_quad(203, 0, frame(2, 2), [92.0, 38.0, 112.0, 58.0])
    });
    quads.push(Quad {
        painter_order: 4,
        ..atlas_quad(204, 0, frame(2, 2), [94.0, 40.0, 110.0, 56.0])
    });
    SceneBundle::from_quads(
        4,
        375,
        Viewport {
            width,
            height,
            scale_milli: 1_000,
        },
        [0.03, 0.02, 0.05, 1.0],
        quads,
        SceneLimits::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::{ATLAS_SHEET, atlas_provider, atlas_resource, atlas_scene, character_sheet};
    use crate::{ReferenceRenderer, offscreen_target};
    use tilewright_render_api::{RenderRequest, Renderer};
    use tilewright_scene::{Sampling, SceneBundle, SceneLimits};

    #[test]
    fn the_atlas_is_a_valid_four_by_four_sheet() {
        assert_eq!(atlas_resource().len(), 12 + 64 * 64 * 4);
        assert_eq!(ATLAS_SHEET.frame_count(), Some(16));
        let layers = character_sheet(7, 5, [1.0, 2.0], 2.0, 3, Sampling::Nearest);
        assert_eq!(layers.map(|quad| quad.part), [0, 1, 2]);
        assert_eq!(layers.map(|quad| quad.painter_order), [3, 4, 5]);
        // Step 5 wraps to the second column of each layer row.
        assert_eq!(
            layers.map(|quad| quad.frame),
            [
                ATLAS_SHEET.frame(1),
                ATLAS_SHEET.frame(5),
                ATLAS_SHEET.frame(9),
            ]
        );
        assert_eq!(
            layers[0].positions[2].map(f32::to_bits),
            [33.0_f32.to_bits(), 34.0_f32.to_bits()]
        );
    }

    #[test]
    fn the_atlas_scene_draws_every_layer_and_rejects_small_targets() {
        assert!(atlas_scene(111, 64).is_err());
        let scene = atlas_scene(112, 64).unwrap();
        let frame = ReferenceRenderer::new(offscreen_target(112, 64))
            .unwrap()
            .render(RenderRequest {
                scene: &scene,
                resources: atlas_provider().unwrap(),
            })
            .unwrap();
        assert_eq!(frame.metrics.resource_requests, 1);
        for id in [1, 16, 101, 104, 105, 200, 201, 202, 204] {
            assert!(frame.semantic_ids.contains(&id), "semantic {id} missing");
        }
        // The halo is never pickable.
        assert!(!frame.semantic_ids.contains(&203));
    }

    #[test]
    fn frames_outside_the_atlas_fail_closed() {
        let scene = atlas_scene(112, 64).unwrap();
        let mut quads = scene.quads().to_vec();
        quads[0].frame = Some(tilewright_scene::TextureRect {
            x: 60,
            y: 0,
            width: 8,
            height: 8,
        });
        let outside = SceneBundle::from_quads(
            scene.revision(),
            scene.clock_millis(),
            scene.viewport(),
            scene.clear_color(),
            quads,
            SceneLimits::default(),
        )
        .unwrap();
        let error = ReferenceRenderer::new(offscreen_target(112, 64))
            .unwrap()
            .render(RenderRequest {
                scene: &outside,
                resources: atlas_provider().unwrap(),
            })
            .unwrap_err();
        assert!(
            matches!(&error, tilewright_render_api::Error::Resource(message) if message.contains("FrameOutOfBounds")),
            "{error:?}"
        );
    }
}
