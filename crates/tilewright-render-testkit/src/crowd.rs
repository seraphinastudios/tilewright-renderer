// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! A large synthetic scene for frame-time measurement: a top-down ground of
//! atlas tiles under a crowd of walking, layered characters drawn from
//! character sheets laid out like common open sprite sheets (13 × 21 frames
//! of 64 × 64 texels, with walk cycles in rows 8–11). Every texel is
//! generated from constants; no game or third-party art is involved.

use crate::{FakeResourceProvider, atlas::frame_texel, texture_envelope};
use std::sync::Arc;
use tilewright_render_resources::Error as ResourceError;
use tilewright_scene::{
    Digest256, Layer, Quad, ResourceId, Sampling, SceneBundle, SceneLimits, SemanticId,
    SpriteSheet, TextureRect, Viewport,
};

/// Width and height of a character frame in texels.
pub const CROWD_FRAME: u32 = 64;
/// One layer's character sheet: 13 × 21 frames.
pub const CROWD_SHEET: SpriteSheet = SpriteSheet::grid(CROWD_FRAME, CROWD_FRAME, 13, 21);
/// Width and height of a ground tile in texels and pixels.
pub const CROWD_TILE: u32 = 32;
/// The ground tile atlas: four tiles in a row.
pub const CROWD_TILES_ID: ResourceId = ResourceId(301);
/// The body, tunic and hair sheets when each layer has its own texture.
pub const CROWD_LAYER_IDS: [ResourceId; 3] = [ResourceId(302), ResourceId(303), ResourceId(304)];
/// The atlas holding all three layer sheets, stacked top to bottom.
pub const CROWD_ATLAS_ID: ResourceId = ResourceId(305);
/// The first walk row; rows 8–11 walk up, left, down and right.
const WALK_ROW: u32 = 8;
/// Frames in a walk cycle.
const WALK_FRAMES: u32 = 9;

/// How the character layers are stored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CrowdSheets {
    /// One texture per layer, as layered sheets are usually distributed. A
    /// character's three layers then bind three textures.
    PerLayer,
    /// One atlas holding every layer's sheet, so consecutive layers share a
    /// texture and a draw call.
    Atlas,
}

/// A crowd scene's size and layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Crowd {
    pub width: u32,
    pub height: u32,
    /// Characters drawn, each as three layer quads.
    pub characters: u32,
    pub sheets: CrowdSheets,
}

/// A ground tile at a tile-relative position: a checker with a bright edge.
fn tile_texel(tile: u32, x: u32, y: u32) -> [u8; 4] {
    frame_texel(
        tile,
        3,
        i32::try_from(x / 2).unwrap_or(0),
        i32::try_from(y / 2).unwrap_or(0),
    )
}

/// A character layer's texel at a position in one 64 × 64 frame of its
/// sheet: the 16 × 16 corpus design scaled 4×, with the walk step taken
/// from the frame's column and a tint from its row.
fn layer_texel(layer: u32, row: u32, column: u32, x: u32, y: u32) -> [u8; 4] {
    let [red, green, blue, alpha] = frame_texel(
        column % 4,
        layer,
        i32::try_from(x / 4).unwrap_or(0),
        i32::try_from(y / 4).unwrap_or(0),
    );
    let shade = |channel: u8| {
        u8::try_from((u32::from(channel) * (200 + row * 2) / 242).min(255)).unwrap_or(u8::MAX)
    };
    [shade(red), shade(green), shade(blue), alpha]
}

fn layer_sheet(layer: u32) -> Vec<[u8; 4]> {
    let width = CROWD_SHEET.columns * CROWD_FRAME;
    let height = CROWD_SHEET.rows * CROWD_FRAME;
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            pixels.push(layer_texel(
                layer,
                y / CROWD_FRAME,
                x / CROWD_FRAME,
                x % CROWD_FRAME,
                y % CROWD_FRAME,
            ));
        }
    }
    pixels
}

/// Every texture of a crowd as `(identity, width, height, envelope,
/// digest)`.
#[must_use]
pub fn crowd_textures(sheets: CrowdSheets) -> Vec<(ResourceId, u32, u32, Vec<u8>, Digest256)> {
    let mut textures = Vec::new();
    let tiles: Vec<[u8; 4]> = (0..CROWD_TILE)
        .flat_map(|y| {
            (0..CROWD_TILE * 4).map(move |x| tile_texel(x / CROWD_TILE, x % CROWD_TILE, y))
        })
        .collect();
    let (bytes, digest) = texture_envelope(CROWD_TILE * 4, CROWD_TILE, &tiles);
    textures.push((CROWD_TILES_ID, CROWD_TILE * 4, CROWD_TILE, bytes, digest));
    let width = CROWD_SHEET.columns * CROWD_FRAME;
    let height = CROWD_SHEET.rows * CROWD_FRAME;
    match sheets {
        CrowdSheets::PerLayer => {
            for (layer, id) in (0_u32..).zip(CROWD_LAYER_IDS) {
                let (bytes, digest) = texture_envelope(width, height, &layer_sheet(layer));
                textures.push((id, width, height, bytes, digest));
            }
        }
        CrowdSheets::Atlas => {
            let pixels: Vec<[u8; 4]> = (0..3).flat_map(layer_sheet).collect();
            let (bytes, digest) = texture_envelope(width, height * 3, &pixels);
            textures.push((CROWD_ATLAS_ID, width, height * 3, bytes, digest));
        }
    }
    textures
}

/// A provider holding a crowd's textures.
///
/// # Errors
/// Returns a provider availability error if its internal lock is poisoned.
pub fn crowd_provider(sheets: CrowdSheets) -> Result<Arc<FakeResourceProvider>, ResourceError> {
    let provider = Arc::new(FakeResourceProvider::default());
    for (id, _, _, bytes, _) in crowd_textures(sheets) {
        provider.insert(id, 1, bytes)?;
    }
    Ok(provider)
}

/// A deterministic 64-bit mix of an index (the `SplitMix64` finaliser).
const fn mix(index: u64) -> u64 {
    let mut value = index.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

type Texture = (ResourceId, Digest256);

/// A pickable, nearest-sampled world quad drawing one frame over
/// `[left, top, right, bottom]`.
fn crowd_quad(
    semantic: u32,
    part: u16,
    texture: Texture,
    frame: TextureRect,
    [left, top, right, bottom]: [f32; 4],
) -> Quad {
    Quad {
        semantic_id: SemanticId(semantic),
        part,
        resource_id: texture.0,
        resource_revision: 1,
        resource_digest: texture.1,
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

/// Ground tiles covering a `columns` × `rows` grid, below everything else.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn ground(columns: u32, rows: u32, tiles: Texture, quads: &mut Vec<Quad>) {
    for row in 0..rows {
        for column in 0..columns {
            let index = row * columns + column;
            let tile = (mix(u64::from(index)) % 4) as u32;
            let left = (column * CROWD_TILE) as f32;
            let top = (row * CROWD_TILE) as f32;
            let frame = TextureRect {
                x: tile * CROWD_TILE,
                y: 0,
                width: CROWD_TILE,
                height: CROWD_TILE,
            };
            let size = CROWD_TILE as f32;
            quads.push(Quad {
                depth: -1,
                painter_order: index,
                pickable: false,
                ..crowd_quad(
                    1 + index,
                    0,
                    tiles,
                    frame,
                    [left, top, left + size, top + size],
                )
            });
        }
    }
}

/// One character's position and walk-cycle frame at `frame`.
#[allow(clippy::cast_possible_truncation)]
fn walker(crowd: Crowd, character: u32, frame: u64) -> (i64, i64, u32, u32) {
    let seed = mix(u64::from(character) + 0x1000_0000);
    let direction = (seed % 4) as u32;
    let (step_x, step_y): (i64, i64) = match direction {
        0 => (0, -1),
        1 => (-1, 0),
        2 => (0, 1),
        _ => (1, 0),
    };
    let span_x = i64::from(crowd.width + CROWD_FRAME);
    let span_y = i64::from(crowd.height + CROWD_FRAME);
    let travel = i64::try_from(frame % 1_000_000).unwrap_or(0);
    let start_x = i64::try_from((seed >> 8) & 0xffff).unwrap_or(0);
    let start_y = i64::try_from((seed >> 24) & 0xffff).unwrap_or(0);
    let x = (start_x + step_x * travel).rem_euclid(span_x) - i64::from(CROWD_FRAME);
    let y = (start_y + step_y * travel).rem_euclid(span_y) - i64::from(CROWD_FRAME);
    let column = ((frame / 4 + (seed >> 40)) % u64::from(WALK_FRAMES)) as u32;
    (x, y, column, WALK_ROW + direction)
}

/// The crowd at `frame`: the ground fills the viewport, and each character
/// walks one pixel per frame in its direction, wrapping at the edges, with
/// its walk cycle advancing every four frames. Characters are drawn in rows
/// from the top down, each as body, tunic and hair. The texture identities
/// match [`crowd_provider`] for the same `sheets`.
///
/// # Errors
/// Returns a scene validation error, for example for a viewport outside the
/// scene limits or more quads than they allow.
#[allow(clippy::cast_precision_loss)]
pub fn crowd_scene(crowd: Crowd, frame: u64) -> Result<SceneBundle, tilewright_scene::Error> {
    let textures = crowd_textures_identities(crowd.sheets);
    let columns = crowd.width.div_ceil(CROWD_TILE);
    let rows = crowd.height.div_ceil(CROWD_TILE);
    let tiles = columns.saturating_mul(rows);
    let capacity = usize::try_from(u64::from(tiles) + u64::from(crowd.characters) * 3)
        .map_err(|_| tilewright_scene::Error::LimitExceeded)?;
    if capacity > SceneLimits::default().maximum_quads {
        return Err(tilewright_scene::Error::LimitExceeded);
    }
    let mut quads = Vec::with_capacity(capacity);
    ground(columns, rows, textures[0], &mut quads);
    for character in 0..crowd.characters {
        let (x, y, column, row) = walker(crowd, character, frame);
        let size = i64::from(CROWD_FRAME);
        let bounds = [x as f32, y as f32, (x + size) as f32, (y + size) as f32];
        // Lower characters are drawn later; layers keep their order.
        let order = u32::try_from(y + size).unwrap_or(0) * 4;
        for layer in 0..3_u16 {
            let (sheet, texture) = match crowd.sheets {
                CrowdSheets::PerLayer => (CROWD_SHEET, textures[1 + usize::from(layer)]),
                CrowdSheets::Atlas => (
                    SpriteSheet {
                        origin: [0, u32::from(layer) * CROWD_SHEET.rows * CROWD_FRAME],
                        ..CROWD_SHEET
                    },
                    textures[1],
                ),
            };
            let frame_rect = sheet
                .frame_at(column, row)
                .ok_or(tilewright_scene::Error::InvalidValue)?;
            quads.push(Quad {
                painter_order: order + u32::from(layer),
                ..crowd_quad(tiles + 1 + character, layer, texture, frame_rect, bounds)
            });
        }
    }
    SceneBundle::from_quads(
        frame + 1,
        frame.saturating_mul(16),
        Viewport {
            width: crowd.width,
            height: crowd.height,
            scale_milli: 1_000,
        },
        [0.05, 0.05, 0.08, 1.0],
        quads,
        SceneLimits::default(),
    )
}

/// The identities of a crowd's textures, in [`crowd_textures`] order,
/// computed once per process.
fn crowd_textures_identities(sheets: CrowdSheets) -> Vec<(ResourceId, Digest256)> {
    use std::sync::OnceLock;
    static PER_LAYER: OnceLock<Vec<(ResourceId, Digest256)>> = OnceLock::new();
    static ATLAS: OnceLock<Vec<(ResourceId, Digest256)>> = OnceLock::new();
    let cell = match sheets {
        CrowdSheets::PerLayer => &PER_LAYER,
        CrowdSheets::Atlas => &ATLAS,
    };
    cell.get_or_init(|| {
        crowd_textures(sheets)
            .into_iter()
            .map(|(id, _, _, _, digest)| (id, digest))
            .collect()
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::{
        CROWD_ATLAS_ID, CROWD_LAYER_IDS, CROWD_TILES_ID, Crowd, CrowdSheets, crowd_provider,
        crowd_scene, crowd_textures,
    };
    use crate::{ReferenceRenderer, offscreen_target};
    use tilewright_render_api::{RenderRequest, Renderer};

    #[test]
    fn crowds_fill_the_view_with_layered_walking_characters() {
        let crowd = Crowd {
            width: 320,
            height: 192,
            characters: 40,
            sheets: CrowdSheets::Atlas,
        };
        let scene = crowd_scene(crowd, 0).unwrap();
        // 10 × 6 ground tiles and three layers per character.
        assert_eq!(scene.quads().len(), 60 + 120);
        assert!(
            scene
                .quads()
                .iter()
                .all(|quad| [CROWD_TILES_ID, CROWD_ATLAS_ID].contains(&quad.resource_id))
        );
        let frame = ReferenceRenderer::new(offscreen_target(320, 192))
            .unwrap()
            .render(RenderRequest {
                scene: &scene,
                resources: crowd_provider(CrowdSheets::Atlas).unwrap(),
            })
            .unwrap();
        // Ground tiles are not pickable; characters are.
        assert!(frame.semantic_ids.iter().any(|id| *id > 60));
        // Characters move and animate from frame to frame.
        let later = crowd_scene(crowd, 8).unwrap();
        assert_ne!(scene.quads()[60].positions, later.quads()[60].positions);
        assert_eq!(later.revision(), 9);
    }

    #[test]
    fn per_layer_crowds_bind_one_sheet_per_layer() {
        let textures = crowd_textures(CrowdSheets::PerLayer);
        assert_eq!(
            textures.iter().map(|texture| texture.0).collect::<Vec<_>>(),
            [
                CROWD_TILES_ID,
                CROWD_LAYER_IDS[0],
                CROWD_LAYER_IDS[1],
                CROWD_LAYER_IDS[2]
            ]
        );
        // Each sheet is 832 × 1,344 texels, within one resource request.
        assert!(
            textures[1..]
                .iter()
                .all(|texture| (texture.1, texture.2) == (832, 1_344))
        );
        let atlas = crowd_textures(CrowdSheets::Atlas);
        assert_eq!((atlas[1].1, atlas[1].2), (832, 4_032));
        assert!(atlas[1].3.len() <= tilewright_render_resources::DEFAULT_MAXIMUM_RESOURCE_BYTES);
        let scene = crowd_scene(
            Crowd {
                width: 64,
                height: 64,
                characters: 3,
                sheets: CrowdSheets::PerLayer,
            },
            5,
        )
        .unwrap();
        let character: Vec<_> = scene
            .quads()
            .iter()
            .filter(|quad| quad.resource_id != CROWD_TILES_ID)
            .map(|quad| quad.resource_id)
            .collect();
        assert_eq!(character.len(), 9);
        assert!(CROWD_LAYER_IDS.iter().all(|id| character.contains(id)));
    }
}
