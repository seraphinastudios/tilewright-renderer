// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! Tiled ground covers every pixel exactly at fractional zooms, view sizes
//! and camera positions, through the reference renderer that also owns the
//! GPU backend's picking planes.

use crate::{
    GroundArt, ReferenceRenderer, ground_grid_provider, ground_grid_view, offscreen_target,
};
use tilewright_render_api::{RenderRequest, Renderer};
use tilewright_render_resources::for_each_quad_pixel;
use tilewright_render_world::{
    Anchor, Camera, HeightField, Projection, WorldObject, WorldView, compile,
};
use tilewright_scene::{Sampling, SceneBundle, SceneLimits, SemanticId};

/// Every zoom from 0.25 to 3 in steps of 0.01.
fn zooms() -> impl Iterator<Item = f32> {
    (25_u16..=300).map(|step| f32::from(step) / 100.0)
}

/// Odd and even view sizes.
const SIZES: [(u32, u32); 2] = [(61, 47), (64, 48)];

/// A tile corner at the view centre, panned centres between tiles, and a
/// centre whose edges round to within a hair of half a pixel.
const CENTRES: [[f64; 2]; 5] = [
    [0.0, 0.0],
    [3.37, 7.81],
    [-5.5, 2.25],
    [10.123, -4.987],
    [-0.749_999_999_999_999_9, -0.5],
];

const TOP_DOWN: Projection = Projection::TopDown { tile_size: 32 };
const ISO_48: Projection = Projection::Isometric {
    tile_width: 48,
    tile_height: 24,
};
const ISO_64: Projection = Projection::Isometric {
    tile_width: 64,
    tile_height: 32,
};

/// Runs `check` for every camera of the sweep and fails listing the cameras
/// it rejects, with the number of bad pixels each.
fn sweep(projection: Projection, check: impl FnMut(Camera) -> usize) {
    sweep_over(projection, zooms(), &SIZES, &CENTRES, check);
}

fn sweep_over(
    projection: Projection,
    zooms: impl Iterator<Item = f32>,
    sizes: &[(u32, u32)],
    centres: &[[f64; 2]],
    mut check: impl FnMut(Camera) -> usize,
) {
    let mut failures = Vec::new();
    let mut cameras = 0_usize;
    for zoom in zooms {
        for &(width, height) in sizes {
            for &center in centres {
                let camera = Camera {
                    projection,
                    center,
                    zoom,
                    width,
                    height,
                    sampling: Sampling::Nearest,
                };
                cameras += 1;
                let bad = check(camera);
                if bad > 0 {
                    failures.push(format!(
                        "zoom {zoom} {width}x{height} centre {center:?}: {bad}"
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{projection:?}: {} of {cameras} cameras have bad pixels, e.g. {:?}",
        failures.len(),
        &failures[..failures.len().min(8)],
    );
}

/// Pixels the reference renderer leaves without a semantic identity.
fn uncovered(camera: Camera, art: GroundArt) -> usize {
    let view = ground_grid_view(camera, art).unwrap();
    let scene = compile(&view, SceneLimits::default()).unwrap();
    semantic_holes(&scene, camera, art)
}

/// Pixels of a ground grid scene the reference renderer leaves without a
/// semantic identity.
fn semantic_holes(scene: &SceneBundle, camera: Camera, art: GroundArt) -> usize {
    let frame = ReferenceRenderer::new(offscreen_target(camera.width, camera.height))
        .unwrap()
        .render(RenderRequest {
            scene,
            resources: ground_grid_provider(camera.projection, art).unwrap(),
        })
        .unwrap();
    frame.semantic_ids.iter().filter(|id| **id == 0).count()
}

#[test]
fn tiled_ground_covers_every_pixel_at_every_zoom() {
    for projection in [TOP_DOWN, ISO_48, ISO_64] {
        sweep(projection, |camera| uncovered(camera, GroundArt::Full));
    }
}

#[test]
fn whole_zooms_tile_without_seams_wherever_edges_round() {
    // Every quad shifts onto the pixel grid by one amount, so edges that
    // round to within a hair of half a pixel still meet.
    let centres = [
        [-0.749_999_999_999_999_9, -0.5],
        [-4.3, 8.896_340_546_624_945],
        [0.300_000_000_000_000_04, 0.1],
        [2.5, -1.25],
    ];
    let sizes = [(61, 47), (78, 103)];
    let zooms = || (1_u8..=8).map(f32::from);
    for projection in [Projection::TopDown { tile_size: 3 }, TOP_DOWN] {
        sweep_over(projection, zooms(), &sizes, &centres, |camera| {
            uncovered(camera, GroundArt::Full)
        });
    }
    for projection in [ISO_48, ISO_64] {
        for art in [GroundArt::Full, GroundArt::Diamond] {
            sweep_over(projection, zooms(), &sizes, &centres, |camera| {
                uncovered(camera, art)
            });
        }
    }
}

#[test]
fn isometric_diamonds_interlock_at_every_zoom() {
    // The art's own texels decide coverage along the diagonals, so every
    // tile must sample them at the same place in the world.
    for projection in [ISO_48, ISO_64] {
        sweep(projection, |camera| uncovered(camera, GroundArt::Diamond));
    }
}

#[test]
fn neighbouring_ground_quads_never_overlap() {
    // Top-down tiles tile the view on their own. Isometric tiles whose
    // coordinates sum to an even number abut edge to edge, as do those whose
    // coordinates sum to an odd number, so each set must cover every pixel
    // exactly once by geometry alone.
    for projection in [TOP_DOWN, ISO_48, ISO_64] {
        let isometric = matches!(projection, Projection::Isometric { .. });
        sweep(projection, |camera| {
            let view = ground_grid_view(camera, GroundArt::Full).unwrap();
            let scene = compile(&view, SceneLimits::default()).unwrap();
            let pixels = (camera.width * camera.height) as usize;
            let mut counts = [vec![0_u8; pixels], vec![0_u8; pixels]];
            for quad in scene.quads() {
                let object = &view.objects[quad.semantic_id.0 as usize - 1];
                let set = usize::from(isometric && (object.tile[0] + object.tile[1]) % 2 != 0);
                for_each_quad_pixel(quad, camera.width, camera.height, |_, _, index, _, _| {
                    counts[set][index] = counts[set][index].saturating_add(1);
                });
            }
            let sets = if isometric { &counts[..] } else { &counts[..1] };
            sets.iter().flatten().filter(|count| **count != 1).count()
        });
    }
}

#[test]
fn ground_grids_are_bounded_before_allocating() {
    let camera = |zoom| Camera {
        projection: ISO_48,
        center: [0.0, 0.0],
        zoom,
        width: 1_024,
        height: 768,
        sampling: Sampling::Nearest,
    };
    assert!(ground_grid_view(camera(0.25), GroundArt::Diamond).is_ok());
    for zoom in [0.01, 0.0] {
        assert_eq!(
            ground_grid_view(camera(zoom), GroundArt::Diamond).err(),
            Some(tilewright_scene::Error::LimitExceeded),
            "zoom {zoom}"
        );
    }
    // Tile sides the compiler rejects never build a texture: 30,000 px
    // tiles would need 3.6 GB.
    let huge = Projection::TopDown { tile_size: 30_000 };
    let mut wide = camera(1.0);
    wide.projection = huge;
    assert_eq!(
        ground_grid_view(wide, GroundArt::Full).err(),
        Some(tilewright_scene::Error::LimitExceeded)
    );
    assert!(matches!(
        ground_grid_provider(huge, GroundArt::Full),
        Err(tilewright_render_resources::Error::LimitExceeded)
    ));
    assert!(
        ground_grid_provider(Projection::TopDown { tile_size: 1_024 }, GroundArt::Full).is_ok()
    );
}

/// Twice a tile's unsnapped art left and top edges in world pixels, and the
/// art's size in texels, for the ground grid's full-size art.
fn doubled_art_origin(projection: Projection, tile: [i64; 2]) -> ([i128; 2], [u32; 2]) {
    let [x, y] = tile.map(i128::from);
    match projection {
        Projection::TopDown { tile_size } => {
            let size = i128::from(tile_size);
            ([2 * x * size, 2 * y * size], [tile_size, tile_size])
        }
        Projection::Isometric {
            tile_width,
            tile_height,
        } => {
            let (width, height) = (i128::from(tile_width), i128::from(tile_height));
            // The art's centre sits half a pixel below the diamond's.
            let left = (x - y) * width - width;
            let top = (x + y + 1) * height - height - 1;
            ([left, top], [tile_width, tile_height])
        }
    }
}

#[test]
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn ground_texels_break_ties_one_way_across_every_tile() {
    // With the camera on a tile corner or half way along a tile, texel
    // boundaries land exactly on pixel centres at these zooms, or a hair
    // from them just off zoom 1. Every pixel centre a ground quad covers must
    // sample the texel its unsnapped art puts there once all ground moves
    // 1/128 px up and left: the later texel at an exact tie, as nearest
    // sampling chooses, in both halves of every tile. In even-sized top-down
    // views, where main never moves the art, this is exactly main's choice.
    // Every `f32` zoom is a whole number of 2^-32, so the expected texel is
    // exact.
    const SCALE: i128 = 1 << 32;
    let near_one = [
        f32::from_bits(1.0_f32.to_bits() - 1),
        f32::from_bits(1.0_f32.to_bits() + 1),
    ];
    for projection in [TOP_DOWN, ISO_48, ISO_64] {
        for zoom in [0.5, 0.75, 1.25, 1.5, near_one[0], near_one[1]] {
            let ratio = (f64::from(zoom) * SCALE as f64) as i128;
            sweep_over(
                projection,
                [zoom].into_iter(),
                &SIZES,
                &[[0.0, 0.0], [0.5, 0.5], [1.0, 2.0]],
                |camera| {
                    let view = ground_grid_view(camera, GroundArt::Checker).unwrap();
                    let scene = compile(&view, SceneLimits::default()).unwrap();
                    // Twice the camera's world position: whole numbers here.
                    let [u, v] = camera.center;
                    let camera_world = match projection {
                        Projection::TopDown { tile_size } => {
                            [u, v].map(|axis| (2.0 * axis * f64::from(tile_size)) as i128)
                        }
                        Projection::Isometric {
                            tile_width,
                            tile_height,
                        } => [
                            ((u - v) * f64::from(tile_width)) as i128,
                            ((u + v) * f64::from(tile_height)) as i128,
                        ],
                    };
                    let view_size = [camera.width, camera.height].map(i128::from);
                    let mut wrong = 0;
                    for quad in scene.quads() {
                        let object = &view.objects[quad.semantic_id.0 as usize - 1];
                        let (origin, art) = doubled_art_origin(projection, object.tile);
                        let texel = |axis: usize, pixel: u32, sampled: f32| {
                            let numerator = (2 * i128::from(pixel) + 1 - view_size[axis]) * SCALE
                                + ratio * (camera_world[axis] - origin[axis])
                                + SCALE / 64;
                            let sampled = (sampled.clamp(0.0, 1.0 - f32::EPSILON)
                                * art[axis] as f32)
                                .floor() as i128;
                            sampled == numerator.div_euclid(2 * ratio)
                        };
                        for_each_quad_pixel(quad, camera.width, camera.height, |x, y, _, u, v| {
                            if !(texel(0, x, u) && texel(1, y, v)) {
                                wrong += 1;
                            }
                        });
                    }
                    wrong
                },
            );
        }
    }
}

/// Every third zoom of the sweep, for the slower variations.
fn some_zooms() -> impl Iterator<Item = f32> {
    zooms().step_by(3)
}

/// The ground grid raised to one height across a field covering it.
fn raised(mut view: WorldView, height: i32, pixels_per_unit: f32) -> WorldView {
    let along = |axis: usize| view.objects.iter().map(move |object| object.tile[axis]);
    let origin = [0, 1].map(|axis| along(axis).min().unwrap());
    let size = [0, 1].map(|axis| along(axis).max().unwrap() - origin[axis] + 1);
    let [width, rows] = size.map(|side| u32::try_from(side).unwrap());
    view.heights = HeightField {
        origin,
        width,
        height: rows,
        heights: vec![Some(height); (width * rows) as usize],
    };
    view.stretch.pixels_per_unit = pixels_per_unit;
    view
}

#[test]
fn raised_ground_covers_every_pixel() {
    // Raised flat ground subtracts its rise from every edge once, so
    // fractional pixels per unit still leave shared edges equal.
    for (height, pixels_per_unit) in [(7, 0.37), (-3, 1.7)] {
        for projection in [TOP_DOWN, ISO_48] {
            for art in [GroundArt::Full, GroundArt::Diamond] {
                sweep_over(projection, some_zooms(), &SIZES, &CENTRES, |camera| {
                    let view = raised(
                        ground_grid_view(camera, art).unwrap(),
                        height,
                        pixels_per_unit,
                    );
                    let scene = compile(&view, SceneLimits::default()).unwrap();
                    semantic_holes(&scene, camera, art)
                });
            }
        }
    }
}

#[test]
fn top_down_footprints_tile_without_gaps_or_overlaps() {
    // Two-tile-wide ground on every third row, and two-tile-tall ground on
    // the next two, cover each tile exactly once.
    sweep_over(TOP_DOWN, some_zooms(), &SIZES, &CENTRES, |camera| {
        let mut view = ground_grid_view(camera, GroundArt::Full).unwrap();
        let texture = view.objects[0].animation.frames[0];
        let tiles: Vec<[i64; 2]> = view.objects.iter().map(|object| object.tile).collect();
        view.objects = tiles
            .iter()
            .filter_map(|&[x, y]| match y.rem_euclid(3) {
                0 if x.rem_euclid(2) == 0 => Some(([x, y], [2, 1])),
                1 => Some(([x, y], [1, 2])),
                _ => None,
            })
            .zip(1..)
            .map(|((tile, footprint), id)| {
                let mut object = WorldObject::new(SemanticId(id), tile, 0, Anchor::Ground, texture);
                object.footprint = footprint;
                object
            })
            .collect();
        let scene = compile(&view, SceneLimits::default()).unwrap();
        let mut counts = vec![0_u8; (camera.width * camera.height) as usize];
        for quad in scene.quads() {
            for_each_quad_pixel(quad, camera.width, camera.height, |_, _, index, _, _| {
                counts[index] = counts[index].saturating_add(1);
            });
        }
        counts.iter().filter(|count| **count != 1).count()
    });
}

#[test]
fn far_ground_covers_every_pixel() {
    // Near the coordinate bound the camera's world position keeps only
    // about 1/128 px, but shared edges stay exact.
    #[allow(clippy::cast_precision_loss)]
    let far = (tilewright_render_world::MAXIMUM_TILE_COORDINATE - 50) as f64;
    let centres = [
        [far - 0.37, -far + 0.81],
        [far + 0.5, far - 0.25],
        [-far, -far],
    ];
    for projection in [TOP_DOWN, ISO_48] {
        for art in [GroundArt::Full, GroundArt::Diamond] {
            sweep_over(projection, some_zooms(), &SIZES, &centres, |camera| {
                uncovered(camera, art)
            });
        }
    }
}
