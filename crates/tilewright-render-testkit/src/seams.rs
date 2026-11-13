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
fn ground_under_a_pixel_samples_the_world_selected_texel() {
    // A half-pixel tile covers one centre, whose position within the art
    // changes as the camera pans. Only the selected texel is opaque, so a
    // midpoint lookup would lose both its colour and semantic identity.
    for tile in [[0, 0], [-1, -1]] {
        for (x, center_x) in [(0, 0.4), (1, 0.6)] {
            for (y, center_y) in [(0, 0.4), (1, 0.6)] {
                let camera = Camera {
                    projection: Projection::TopDown { tile_size: 2 },
                    center: [f64::from(tile[0]) + center_x, f64::from(tile[1]) + center_y],
                    zoom: 0.25,
                    width: 1,
                    height: 1,
                    sampling: Sampling::Nearest,
                };
                let mut view = ground_grid_view(camera, GroundArt::Full).unwrap();
                view.objects
                    .retain(|object| object.tile == tile.map(i64::from));
                let mut pixels = [[0; 4]; 4];
                pixels[y * 2 + x] = [170, 140, 90, 255];
                let (bytes, digest) = crate::texture_envelope(2, 2, &pixels);
                let object = &mut view.objects[0];
                object.animation.frames[0].digest = digest;
                let id = object.semantic_id.0;
                let provider = std::sync::Arc::new(crate::FakeResourceProvider::default());
                provider
                    .insert(object.animation.frames[0].id, 1, bytes)
                    .unwrap();
                let scene = compile(&view, SceneLimits::default()).unwrap();
                let frame = ReferenceRenderer::new(offscreen_target(1, 1))
                    .unwrap()
                    .render(RenderRequest {
                        scene: &scene,
                        resources: provider,
                    })
                    .unwrap();
                assert_eq!(frame.semantic_ids, [id], "texel {x},{y}");
                assert_eq!(frame.rgba8, [170, 140, 90, 255], "texel {x},{y}");
            }
        }
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
    // The bound is on the tile, not on the art: Diamond art is a texel
    // shorter than its tile, but a 1,025 px tile is still refused.
    for art in [GroundArt::Full, GroundArt::Diamond, GroundArt::Checker] {
        for (tile_width, tile_height) in [(48, 1_025), (48, 0), (1_025, 24), (0, 24)] {
            let projection = Projection::Isometric {
                tile_width,
                tile_height,
            };
            let mut tall = camera(1.0);
            tall.projection = projection;
            let case = format!("{art:?} {tile_width}x{tile_height}");
            assert!(
                matches!(
                    ground_grid_provider(projection, art),
                    Err(tilewright_render_resources::Error::LimitExceeded)
                ),
                "{case}"
            );
            assert_eq!(
                ground_grid_view(tall, art).err(),
                Some(tilewright_scene::Error::LimitExceeded),
                "{case}"
            );
            assert_eq!(
                crate::ground_grid_scene(tall, art).err(),
                Some(tilewright_scene::Error::LimitExceeded),
                "{case}"
            );
        }
        let largest = Projection::Isometric {
            tile_width: 1_024,
            tile_height: 1_024,
        };
        assert!(ground_grid_provider(largest, art).is_ok(), "{art:?}");
    }
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
    // sampling chooses, in both halves of every tile; the expected texel
    // below includes that 1/128 px move. Every `f32` zoom is a whole number of 2^-32, so the expected texel is
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

/// A deterministic run of `f64` values in `[0, 1)` on a 2^-24 grid.
fn fractions(mut state: u64) -> impl FnMut() -> f64 {
    move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        f64::from(u32::try_from(state >> 40).unwrap()) / f64::from(1_u32 << 24)
    }
}

/// The camera's world position in world pixels as the world compiler
/// computes it in `f64`.
fn camera_world(projection: Projection, center: [f64; 2]) -> [f64; 2] {
    let [u, v] = center;
    match projection {
        Projection::TopDown { tile_size } => [u, v].map(|axis| axis * f64::from(tile_size)),
        Projection::Isometric {
            tile_width,
            tile_height,
        } => [
            (u - v) * f64::from(tile_width) / 2.0,
            (u + v) * f64::from(tile_height) / 2.0,
        ],
    }
}

#[test]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::float_cmp
)]
fn whole_zoom_ground_edges_land_where_exact_arithmetic_rounds_them() {
    // At whole zooms every edge of raised flat ground lands on the pixel edge
    // nearest its unsnapped position, rounding halves up. The camera's world
    // position, as the compiler holds it in `f64`, and the rises here are
    // whole numbers of 2^-32 world px, near the origin and near the
    // coordinate bound, so the expected edges are exact in `i128`. Far from
    // the origin only the class point nearest the camera keeps that
    // precision through the zoom, and fractional rises move every edge by
    // the same amount.
    const ONE: i128 = 1 << 32;
    let exact = |value: f64| {
        let scaled = value * ONE as f64;
        assert!(
            scaled.trunc() == scaled && scaled.abs() < 2.0_f64.powi(100),
            "{value} is not a whole number of 2^-32"
        );
        scaled as i128
    };
    let far = (tilewright_render_world::MAXIMUM_TILE_COORDINATE - 50) as f64;
    let mut fraction = fractions(0x2545_f491_4f6c_dd1d);
    let mut centres = Vec::new();
    for whole in [[0.0, 0.0], [3.0, -2.0], [-7.0, 5.0], [11.0, 9.0]] {
        centres.push([whole[0] + fraction(), whole[1] - fraction()]);
    }
    for sign in [[1.0, 1.0], [1.0, -1.0], [-1.0, 1.0], [-1.0, -1.0]] {
        for _ in 0..4 {
            centres.push([sign[0] * far + fraction(), sign[1] * far - fraction()]);
        }
    }
    let rises = [(0, 1.0), (7, 0.375), (-3, 1.75), (-27, 0.25), (13, 0.0625)];
    for projection in [TOP_DOWN, ISO_48, ISO_64] {
        for (height, pixels_per_unit) in rises {
            let rise = exact(f64::from(height) * f64::from(pixels_per_unit));
            sweep_over(
                projection,
                (1_u8..=8).map(f32::from),
                &SIZES,
                &centres,
                |camera| {
                    let view = raised(
                        ground_grid_view(camera, GroundArt::Full).unwrap(),
                        height,
                        pixels_per_unit,
                    );
                    let scene = compile(&view, SceneLimits::default()).unwrap();
                    let camera_exact = camera_world(projection, camera.center).map(exact);
                    let zoom = i128::from(camera.zoom as u8);
                    let view_size = [camera.width, camera.height].map(i128::from);
                    let mut wrong = 0;
                    for quad in scene.quads() {
                        let object = &view.objects[quad.semantic_id.0 as usize - 1];
                        let (origin, art) = doubled_art_origin(projection, object.tile);
                        for axis in 0..2 {
                            let lift = if axis == 1 { rise } else { 0 };
                            for (corner, span) in [(0, 0), (2, 2 * i128::from(art[axis]))] {
                                let edge = (origin[axis] + span) * ONE / 2;
                                let expected = ((edge - lift - camera_exact[axis]) * zoom
                                    + view_size[axis] * ONE / 2
                                    + ONE / 2)
                                    .div_euclid(ONE);
                                if f64::from(quad.positions[corner][axis]) != expected as f64 {
                                    wrong += 1;
                                }
                            }
                        }
                    }
                    wrong
                },
            );
        }
    }
}

/// Fractional zooms of a whole number of quarters, the zooms at which art
/// edges, a whole number of half world pixels apart, fall a whole number of
/// eighths of a pixel apart on screen.
const TIE_ZOOMS: [f32; 9] = [0.25, 0.5, 0.75, 1.25, 1.5, 1.75, 2.25, 2.5, 2.75];

/// A camera coordinate in world pixels that a zoom in [`TIE_ZOOMS`] puts
/// `residue / 128` px past a whole pixel, chosen so that it and its quotient
/// by `divisor` are exact in `f64`.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn tie_coordinate(zoom: f32, residue: i64, divisor: u32) -> f64 {
    // zoom = odd / 2^shift, and the coordinate is n / (128 · zoom) for some
    // n of the residue modulo 128 that both odd parts divide.
    let quarters = (zoom * 4.0) as i64;
    assert!((quarters as f32 / 4.0 - zoom).abs() == 0.0 && quarters % 4 != 0);
    let odd = quarters >> quarters.trailing_zeros();
    let shift = 2 - quarters.trailing_zeros();
    let step = odd * i64::from(divisor >> divisor.trailing_zeros());
    let multiple = (0..128)
        .find(|k| (step * k - residue).rem_euclid(128) == 0)
        .unwrap();
    (step * multiple / odd) as f64 * f64::from(1_u32 << shift) / 128.0
}

/// Cameras whose position, at `zoom`, puts art edges exactly 1/128 px after
/// a pixel centre along one axis or both, so the world compiler's 1/128 px
/// tie offset puts the centre exactly on the edge. That offset is private to
/// `tilewright-render-world`; its own test
/// `ground_tie_offset_and_margin_keep_their_reviewed_values` pins it to the
/// 1/128 these residues assume.
fn tie_centres(projection: Projection, zoom: f32) -> Vec<[f64; 2]> {
    let divisors = match projection {
        Projection::TopDown { tile_size } => [tile_size; 2],
        Projection::Isometric {
            tile_width,
            tile_height,
        } => [tile_width / 2, tile_height / 2],
    };
    // An edge at a whole number of eighths of a pixel, minus 1/128 px,
    // lies 1/128 px after a pixel centre for one of these residues.
    let residue = |eighths: i64| 16 * eighths - 1;
    let mut centres = Vec::new();
    for eighths in 0..8 {
        let x = tie_coordinate(zoom, residue(eighths), divisors[0]);
        let y = tie_coordinate(zoom, residue(eighths), divisors[1]);
        let other = tie_coordinate(zoom, residue(eighths * 3 % 8), divisors[1]);
        for world in [[x, 0.0], [0.0, y], [x, other]] {
            let center = match projection {
                Projection::TopDown { tile_size } => world.map(|axis| axis / f64::from(tile_size)),
                Projection::Isometric { .. } => {
                    let across = world[0] / f64::from(divisors[0]);
                    let down = world[1] / f64::from(divisors[1]);
                    [f64::midpoint(across, down), (down - across) / 2.0]
                }
            };
            assert_eq!(
                camera_world(projection, center).map(f64::to_bits),
                world.map(f64::to_bits),
                "{projection:?} zoom {zoom}"
            );
            centres.push(center);
        }
    }
    centres
}

#[test]
fn ground_covers_every_pixel_at_tie_cameras() {
    // The offset leaves exact ties only at cameras exactly 1/128 px off a
    // round position, where a pixel centre lies on the edge between
    // neighbouring art; the UV margin keeps such centres inside both.
    for (projection, art) in [
        (TOP_DOWN, GroundArt::Full),
        (ISO_48, GroundArt::Full),
        (ISO_48, GroundArt::Diamond),
        (ISO_64, GroundArt::Full),
        (ISO_64, GroundArt::Diamond),
    ] {
        for zoom in TIE_ZOOMS {
            let centres = tie_centres(projection, zoom);
            sweep_over(projection, [zoom].into_iter(), &SIZES, &centres, |camera| {
                uncovered(camera, art)
            });
        }
    }
}

/// Diamond ground on terraces: flat plateaus joined by ramps one tile wide,
/// `step` height units of 1 px per terrace, repeating every eight tiles on
/// both axes, with the default corner stretch. Plateau tiles stay flat and
/// ramp tiles bend. The grid reaches far enough below the view that raised
/// tiles still cover its bottom edge.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn terraced(camera: Camera, step: i32) -> WorldView {
    let level = |tile: i64| [0, 0, 0, 1, 2, 2, 2, 1][usize::try_from(tile.rem_euclid(8)).unwrap()];
    let highest = f64::from(4 * step) * f64::from(camera.zoom);
    let mut padded = camera;
    padded.height += 2 * highest.ceil() as u32;
    let mut view = raised(
        ground_grid_view(padded, GroundArt::Diamond).unwrap(),
        0,
        1.0,
    );
    view.camera = camera;
    let field = &mut view.heights;
    for row in 0..field.height {
        for column in 0..field.width {
            let [x, y] = [
                field.origin[0] + i64::from(column),
                field.origin[1] + i64::from(row),
            ];
            field.heights[(row * field.width + column) as usize] =
                Some(step * (level(x) + level(y)));
        }
    }
    view
}

#[test]
fn terraced_ground_leaves_only_rare_holes_where_flat_meets_bent() {
    // Bent tiles stretch their art one art pixel past the lattice cell, but
    // flat art beside them stops half an art pixel short of its tips, so at
    // some zooms a few pixels where the two meet stay uncovered. That
    // residual is known and not fixed; this pins how many pixels it leaves
    // over this sweep, at fractional zooms and separately at whole zooms, so
    // any change that opens more fails. Each case runs on its own thread.
    let fractional: Vec<f32> = (5_u16..=59)
        .step_by(2)
        .map(|step| f32::from(step) / 20.0)
        .collect();
    let fractional = &fractional;
    // Whole zooms see fewer distinct alignments, so they get more centres.
    let mut fraction = fractions(0x9e37_79b9_7f4a_7c15);
    let mut whole_centres = CENTRES.to_vec();
    for whole in -6_i32..6 {
        let whole = f64::from(whole);
        whole_centres.push([whole + fraction(), 2.0 * whole - fraction()]);
    }
    let whole_centres = &whole_centres;
    // Projection, height units per terrace, and the most holes measured on
    // the reference renderer at the fractional zooms 0.25 to 2.95 and at
    // zooms 1, 2 and 3. At these cameras terraces of 2 or 4 units leave none
    // at whole zooms and terraces of 8 units leave a few. Only 2-unit
    // terraces stay clean at every whole-zoom camera; 4-unit terraces can
    // leave rare holes at cameras this sweep does not visit. Every measured
    // hole lies within 3 px of both flat and bent ground.
    let cases = [
        (ISO_48, 2, 1, 0),
        (ISO_48, 4, 3, 0),
        (ISO_48, 8, 17, 5),
        (ISO_64, 2, 0, 0),
        (ISO_64, 4, 0, 0),
        (ISO_64, 8, 1, 1),
    ];
    let results: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = cases
            .map(|(projection, step, _, _)| {
                scope.spawn(move || {
                    let holes = |zooms: &[f32], sizes, centres| {
                        let mut holes = 0;
                        sweep_over(
                            projection,
                            zooms.iter().copied(),
                            sizes,
                            centres,
                            |camera| {
                                let scene =
                                    compile(&terraced(camera, step), SceneLimits::default())
                                        .unwrap();
                                holes += semantic_holes(&scene, camera, GroundArt::Diamond);
                                0
                            },
                        );
                        holes
                    };
                    (
                        holes(fractional, &[(121, 87)], &CENTRES),
                        holes(&[1.0, 2.0, 3.0], &[(121, 87), (120, 86)], whole_centres),
                    )
                })
            })
            .into_iter()
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    for ((projection, step, most, most_whole), (holes, whole)) in cases.into_iter().zip(results) {
        assert!(
            holes <= most && whole <= most_whole,
            "{projection:?} step {step}: {holes} holes, {whole} at whole zooms; at most {most} and {most_whole}"
        );
    }
}
