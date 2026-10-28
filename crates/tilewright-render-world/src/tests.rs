// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

#![allow(clippy::float_cmp)]

use super::{
    Anchor, Animation, Camera, CornerRule, Error, GlowEffect, HeightField, Projection, Stretch,
    TextureRef, WorldObject, WorldView, compile,
};
use tilewright_scene::{Digest256, Quad, ResourceId, Sampling, SceneLimits, SemanticId};

fn texture(id: u64, width: u32, height: u32) -> TextureRef {
    TextureRef {
        id: ResourceId(id),
        revision: 1,
        digest: Digest256([7; 32]),
        width,
        height,
    }
}

fn iso_view(objects: Vec<WorldObject>, heights: HeightField) -> WorldView {
    WorldView {
        revision: 1,
        clock_millis: 0,
        camera: Camera {
            projection: Projection::Isometric {
                tile_width: 48,
                tile_height: 24,
            },
            center: [0.5, 0.5],
            zoom: 1.0,
            width: 96,
            height: 64,
            sampling: Sampling::Nearest,
        },
        clear_color: [0.0, 0.0, 0.0, 1.0],
        heights,
        stretch: Stretch::default(),
        ground_layer_ranks: 2,
        objects,
    }
}

fn ground(id: u32, tile: [i64; 2]) -> WorldObject {
    WorldObject::new(SemanticId(id), tile, 1, Anchor::Ground, texture(1, 48, 23))
}

fn quads_of(view: &WorldView, id: u32) -> Vec<Quad> {
    compile(view, SceneLimits::default())
        .unwrap()
        .quads()
        .iter()
        .filter(|quad| quad.semantic_id == SemanticId(id))
        .copied()
        .collect()
}

#[test]
fn flat_isometric_tile_maps_its_diamond_onto_the_tile() {
    let view = iso_view(vec![ground(1, [0, 0])], HeightField::empty());
    let quads = quads_of(&view, 1);
    assert_eq!(quads.len(), 1);
    let rect = quads[0].aligned_rect().expect("flat tiles are rectangles");
    // The tile centre is at the viewport centre (48, 32); the art's diamond
    // centre is at (24, 11.5) in a 48 × 23 image, so the top edge at 20.5
    // snaps up to the pixel grid.
    assert_eq!(
        (rect.left, rect.top, rect.right, rect.bottom),
        (24.0, 21.0, 72.0, 44.0)
    );
}

#[test]
fn neighbouring_flat_tiles_tessellate_on_the_half_tile_steps() {
    let view = iso_view(
        vec![ground(1, [0, 0]), ground(2, [1, 0]), ground(3, [0, 1])],
        HeightField::empty(),
    );
    let first = quads_of(&view, 1)[0].aligned_rect().unwrap();
    let east = quads_of(&view, 2)[0].aligned_rect().unwrap();
    let south = quads_of(&view, 3)[0].aligned_rect().unwrap();
    assert_eq!((east.left - first.left, east.top - first.top), (24.0, 12.0));
    assert_eq!(
        (south.left - first.left, south.top - first.top),
        (-24.0, 12.0)
    );
}

#[test]
fn stretched_tile_bends_to_shared_corner_heights() {
    let heights = HeightField {
        origin: [0, 0],
        width: 2,
        height: 1,
        heights: vec![Some(0), Some(8)],
    };
    let view = iso_view(vec![ground(1, [0, 0]), ground(2, [1, 0])], heights);
    let left = quads_of(&view, 1);
    let right = quads_of(&view, 2);
    assert_eq!(left.len(), 16, "a sloped tile becomes a 4 × 4 mesh");
    assert!(left.iter().all(|quad| quad.aligned_rect().is_none()));
    let parts: std::collections::BTreeSet<u16> = left.iter().map(|quad| quad.part).collect();
    assert_eq!(parts.len(), 16);
    // The shared edge between the two tiles takes the mean height 4 at both
    // ends, so the meshes meet without a gap.
    let right_edge_of_left = left
        .iter()
        .flat_map(|quad| quad.positions)
        .fold(f32::MIN, |best, point| best.max(point[0]));
    let left_edge_of_right = right
        .iter()
        .flat_map(|quad| quad.positions)
        .fold(f32::MAX, |best, point| best.min(point[0]));
    assert!(right_edge_of_left > left_edge_of_right);
}

#[test]
fn cliff_threshold_keeps_hard_edges() {
    let heights = HeightField {
        origin: [0, 0],
        width: 2,
        height: 1,
        heights: vec![Some(0), Some(100)],
    };
    let mut view = iso_view(vec![ground(1, [0, 0])], heights);
    view.stretch = Stretch {
        cliff_threshold: Some(12),
        ..Stretch::default()
    };
    assert_eq!(
        quads_of(&view, 1).len(),
        1,
        "the cliff neighbour does not bend the tile"
    );
    view.stretch.rule = CornerRule::Max;
    view.stretch.cliff_threshold = None;
    // Without a threshold the tile bends toward its 100-unit neighbour; parts
    // lifted above the viewport are culled.
    let bent = quads_of(&view, 1);
    assert!(bent.len() > 1 && bent.iter().all(|quad| quad.aligned_rect().is_none()));
}

#[test]
fn standing_art_sits_on_the_tile_and_rises_with_height() {
    let tree = |id| {
        WorldObject::new(
            SemanticId(id),
            [0, 0],
            3,
            Anchor::Standing,
            texture(2, 40, 60),
        )
    };
    let flat = iso_view(vec![tree(1)], HeightField::empty());
    let rect = quads_of(&flat, 1)[0].aligned_rect().unwrap();
    // Bottom centre at the diamond's front edge (48, 32 + 11.5), snapped to
    // the pixel grid like the diamond itself.
    assert_eq!(
        (rect.left, rect.bottom, rect.right - rect.left),
        (28.0, 44.0, 40.0)
    );
    let raised = iso_view(
        vec![tree(1)],
        HeightField {
            origin: [0, 0],
            width: 1,
            height: 1,
            heights: vec![Some(10)],
        },
    );
    let raised_rect = quads_of(&raised, 1)[0].aligned_rect().unwrap();
    assert!((raised_rect.bottom - (rect.bottom - 10.0)).abs() < 1e-4);
}

#[test]
fn isometric_order_interleaves_ground_and_objects_by_diagonal() {
    let objects = vec![
        WorldObject::new(
            SemanticId(10),
            [0, 0],
            3,
            Anchor::Standing,
            texture(2, 40, 60),
        ),
        ground(11, [1, 1]),
        ground(12, [0, 0]),
    ];
    let scene = compile(
        &iso_view(objects, HeightField::empty()),
        SceneLimits::default(),
    )
    .unwrap();
    let order: Vec<u32> = scene
        .quads()
        .iter()
        .map(|quad| quad.semantic_id.0)
        .collect();
    assert_eq!(order, [12, 10, 11]);
}

#[test]
fn top_down_draws_ground_first_then_rows() {
    let mut view = iso_view(Vec::new(), HeightField::empty());
    view.camera.projection = Projection::TopDown { tile_size: 32 };
    view.camera.center = [1.0, 1.0];
    view.objects = vec![
        WorldObject::new(
            SemanticId(1),
            [0, 1],
            3,
            Anchor::Standing,
            texture(2, 32, 48),
        ),
        WorldObject::new(
            SemanticId(2),
            [0, 0],
            3,
            Anchor::Standing,
            texture(2, 32, 48),
        ),
        WorldObject::new(SemanticId(3), [1, 1], 0, Anchor::Ground, texture(1, 32, 32)),
    ];
    let scene = compile(&view, SceneLimits::default()).unwrap();
    let order: Vec<u32> = scene
        .quads()
        .iter()
        .map(|quad| quad.semantic_id.0)
        .collect();
    assert_eq!(order, [3, 2, 1]);
    let tile = scene.quads()[0].aligned_rect().unwrap();
    assert_eq!(
        (tile.left, tile.top, tile.right, tile.bottom),
        (48.0, 32.0, 80.0, 64.0)
    );
}

#[test]
fn glow_adds_an_unpickable_halo_before_the_art() {
    let mut lamp = WorldObject::new(
        SemanticId(5),
        [0, 0],
        3,
        Anchor::Standing,
        texture(3, 20, 20),
    );
    lamp.glow = Some(GlowEffect {
        color: [1.0, 0.8, 0.2, 1.0],
        radius_px: 4.0,
        pulse_millis: 1000,
    });
    let mut view = iso_view(vec![lamp], HeightField::empty());
    view.clock_millis = 250;
    let quads = quads_of(&view, 5);
    assert_eq!(quads.len(), 2);
    let halo = quads[0];
    assert!(!halo.pickable);
    let glow = halo.glow.unwrap();
    assert!((glow.radius_uv[0] - 0.2).abs() < 1e-6);
    assert!(
        (glow.color[3] - 0.7).abs() < 1e-6,
        "pulse at a quarter period"
    );
    let halo_rect = halo.aligned_rect().unwrap();
    let art = quads[1].aligned_rect().unwrap();
    assert!((art.left - halo_rect.left - 4.0).abs() < 1e-4);
    assert!(quads[1].pickable && quads[1].glow.is_none());
}

#[test]
fn effects_reach_the_quads() {
    let mut object = WorldObject::new(
        SemanticId(1),
        [0, 0],
        3,
        Anchor::Standing,
        texture(2, 20, 30),
    );
    object.scale = [2.0, 0.5];
    object.alpha = 0.5;
    object.tint = [1.0, 0.0, 0.5];
    object.grayscale = 0.25;
    object.offset_px = [3.0, -2.0];
    let quad = quads_of(&iso_view(vec![object.clone()], HeightField::empty()), 1)[0];
    assert_eq!(quad.color, [1.0, 0.0, 0.5, 0.5]);
    assert!((quad.grayscale - 0.25).abs() < f32::EPSILON);
    let rect = quad.aligned_rect().unwrap();
    assert_eq!(
        (rect.right - rect.left, rect.bottom - rect.top),
        (40.0, 15.0)
    );
    assert_eq!(rect.left, 48.0 - 20.0 + 3.0);
    object.rotation_degrees = 30.0;
    let rotated = quads_of(&iso_view(vec![object], HeightField::empty()), 1)[0];
    assert!(rotated.aligned_rect().is_none());
    let center = rotated.positions.iter().fold([0.0_f32; 2], |sum, point| {
        [sum[0] + point[0] / 4.0, sum[1] + point[1] / 4.0]
    });
    assert!((center[0] - f32::midpoint(rect.left, rect.right)).abs() < 1e-3);
    assert!((center[1] - f32::midpoint(rect.top, rect.bottom)).abs() < 1e-3);
}

#[test]
fn animation_frames_follow_the_clock() {
    let mut object = WorldObject::new(
        SemanticId(1),
        [0, 0],
        3,
        Anchor::Standing,
        texture(20, 16, 16),
    );
    object.animation = Animation {
        frames: vec![
            texture(20, 16, 16),
            texture(21, 16, 16),
            texture(22, 16, 16),
        ],
        frame_millis: 100,
        phase_millis: 50,
    };
    let mut view = iso_view(vec![object], HeightField::empty());
    for (clock, expected) in [(0, 20), (49, 20), (50, 21), (200, 22), (250, 20)] {
        view.clock_millis = clock;
        assert_eq!(
            quads_of(&view, 1)[0].resource_id,
            ResourceId(expected),
            "clock {clock}"
        );
    }
}

#[test]
fn double_draw_repeats_after_the_next_diagonal_without_picking() {
    let mut tall = WorldObject::new(
        SemanticId(1),
        [0, 0],
        3,
        Anchor::Standing,
        texture(2, 20, 80),
    );
    tall.double_draw = true;
    let objects = vec![tall, ground(2, [1, 0])];
    let scene = compile(
        &iso_view(objects, HeightField::empty()),
        SceneLimits::default(),
    )
    .unwrap();
    let order: Vec<(u32, bool)> = scene
        .quads()
        .iter()
        .map(|quad| (quad.semantic_id.0, quad.pickable))
        .collect();
    assert_eq!(order, [(1, true), (2, true), (1, false)]);
}

#[test]
fn objects_outside_the_viewport_are_culled() {
    let view = iso_view(
        vec![ground(1, [0, 0]), ground(2, [40, 40])],
        HeightField::empty(),
    );
    let scene = compile(&view, SceneLimits::default()).unwrap();
    assert!(scene.quads().iter().all(|quad| quad.semantic_id.0 == 1));
}

#[test]
fn compiling_is_deterministic_and_validates_input() {
    let view = iso_view(
        vec![ground(1, [0, 0]), ground(2, [1, 0])],
        HeightField::empty(),
    );
    assert_eq!(
        compile(&view, SceneLimits::default()).unwrap(),
        compile(&view, SceneLimits::default()).unwrap()
    );
    let mut bad = view.clone();
    bad.camera.zoom = 0.0;
    assert_eq!(
        compile(&bad, SceneLimits::default()),
        Err(Error::InvalidValue)
    );
    let mut bad = view.clone();
    bad.objects[0].footprint = [0, 1];
    assert_eq!(
        compile(&bad, SceneLimits::default()),
        Err(Error::InvalidValue)
    );
    let mut bad = view;
    bad.heights = HeightField {
        origin: [0, 0],
        width: 2,
        height: 2,
        heights: vec![Some(0)],
    };
    assert_eq!(
        compile(&bad, SceneLimits::default()),
        Err(Error::InvalidValue)
    );
}

#[test]
fn top_down_ground_and_standing_art_rise_with_tile_height() {
    let mut view = iso_view(Vec::new(), HeightField::empty());
    view.camera.projection = Projection::TopDown { tile_size: 32 };
    view.camera.center = [1.0, 1.0];
    view.objects = vec![
        WorldObject::new(SemanticId(1), [1, 1], 0, Anchor::Ground, texture(1, 32, 32)),
        WorldObject::new(
            SemanticId(2),
            [1, 1],
            3,
            Anchor::Standing,
            texture(2, 32, 48),
        ),
    ];
    let flat = compile(&view, SceneLimits::default()).unwrap();
    view.heights = HeightField {
        origin: [1, 1],
        width: 1,
        height: 1,
        heights: vec![Some(10)],
    };
    let raised = compile(&view, SceneLimits::default()).unwrap();
    for (before, after) in flat.quads().iter().zip(raised.quads()) {
        let (before, after) = (
            before.aligned_rect().unwrap(),
            after.aligned_rect().unwrap(),
        );
        assert!((before.top - after.top - 10.0).abs() < 1e-4);
        assert!((before.bottom - after.bottom - 10.0).abs() < 1e-4);
    }
}

#[test]
fn coordinates_and_tile_widths_are_bounded() {
    let mut far = iso_view(vec![ground(1, [i64::MAX, 0])], HeightField::empty());
    assert_eq!(
        compile(&far, SceneLimits::default()),
        Err(crate::Error::InvalidValue)
    );
    far.objects[0].tile = [
        super::MAXIMUM_TILE_COORDINATE,
        -super::MAXIMUM_TILE_COORDINATE,
    ];
    assert!(compile(&far, SceneLimits::default()).is_ok());
    let mut narrow = iso_view(vec![ground(1, [0, 0])], HeightField::empty());
    narrow.camera.projection = Projection::Isometric {
        tile_width: 2,
        tile_height: 2,
    };
    assert_eq!(
        compile(&narrow, SceneLimits::default()),
        Err(crate::Error::InvalidValue)
    );
}

#[test]
fn draws_are_bounded_before_culling() {
    let limits = SceneLimits {
        maximum_quads: 2,
        ..SceneLimits::default()
    };
    // Both objects are far outside the viewport, so neither emits a quad.
    let mut view = iso_view(
        vec![ground(1, [40, 40]), ground(2, [41, 40])],
        HeightField::empty(),
    );
    assert!(compile(&view, limits).unwrap().quads().is_empty());
    view.objects[0].double_draw = true;
    assert_eq!(
        compile(&view, limits),
        Err(Error::Scene(tilewright_scene::Error::LimitExceeded))
    );
    view.objects[0].double_draw = false;
    view.objects.push(ground(3, [42, 40]));
    assert_eq!(
        compile(&view, limits),
        Err(Error::Scene(tilewright_scene::Error::LimitExceeded))
    );
}

#[test]
fn every_animation_frame_is_validated() {
    let valid = texture(2, 16, 16);
    let frames = [
        TextureRef {
            id: ResourceId(0),
            ..valid
        },
        TextureRef {
            revision: 0,
            ..valid
        },
        TextureRef {
            digest: Digest256([0; 32]),
            ..valid
        },
        TextureRef {
            width: SceneLimits::default().maximum_dimension + 1,
            ..valid
        },
    ];
    for frame in frames {
        let mut object = WorldObject::new(SemanticId(1), [0, 0], 3, Anchor::Standing, valid);
        // The clock selects only the first frame; the second is never drawn.
        object.animation = Animation {
            frames: vec![valid, frame],
            frame_millis: 100,
            phase_millis: 0,
        };
        assert_eq!(
            compile(
                &iso_view(vec![object], HeightField::empty()),
                SceneLimits::default()
            ),
            Err(Error::InvalidValue),
            "{frame:?}"
        );
    }
}

#[test]
fn semantic_ids_are_validated_whether_or_not_they_are_in_view() {
    let offscreen = ground(0, [40, 40]);
    assert_eq!(
        compile(
            &iso_view(vec![ground(1, [0, 0]), offscreen], HeightField::empty()),
            SceneLimits::default()
        ),
        Err(Error::InvalidValue)
    );
    assert_eq!(
        compile(
            &iso_view(
                vec![ground(1, [0, 0]), ground(1, [40, 40])],
                HeightField::empty()
            ),
            SceneLimits::default()
        ),
        Err(Error::Scene(tilewright_scene::Error::DuplicateSemanticId))
    );
}

#[test]
fn ground_rejects_effects_it_cannot_apply() {
    let edits: [fn(&mut WorldObject); 6] = [
        |object| object.offset_px = [1.0, 0.0],
        |object| object.z = 4,
        |object| object.scale = [2.0, 1.0],
        |object| object.rotation_degrees = 15.0,
        |object| {
            object.glow = Some(GlowEffect {
                color: [1.0; 4],
                radius_px: 2.0,
                pulse_millis: 0,
            });
        },
        |object| object.footprint = [2, 1],
    ];
    for edit in edits {
        let mut tile = ground(1, [0, 0]);
        edit(&mut tile);
        assert_eq!(
            compile(
                &iso_view(vec![tile.clone()], HeightField::empty()),
                SceneLimits::default()
            ),
            Err(Error::InvalidValue),
            "{tile:?}"
        );
    }
    // Top-down ground stretches across its footprint.
    let mut view = iso_view(Vec::new(), HeightField::empty());
    view.camera.projection = Projection::TopDown { tile_size: 32 };
    let mut wide = WorldObject::new(SemanticId(1), [0, 0], 0, Anchor::Ground, texture(1, 32, 32));
    wide.footprint = [2, 1];
    view.objects = vec![wide];
    let rect = compile(&view, SceneLimits::default()).unwrap().quads()[0]
        .aligned_rect()
        .unwrap();
    assert_eq!(rect.right - rect.left, 64.0);
}

#[test]
fn glow_radius_stays_within_every_frame() {
    let lamp = |radius_px, frames: &[TextureRef]| {
        let mut object = WorldObject::new(
            SemanticId(1),
            [0, 0],
            3,
            Anchor::Standing,
            texture(3, 20, 10),
        );
        object.animation.frames.extend_from_slice(frames);
        object.glow = Some(GlowEffect {
            color: [1.0; 4],
            radius_px,
            pulse_millis: 0,
        });
        iso_view(vec![object], HeightField::empty())
    };
    // The 20 × 10 frame's halo reaches the full 10 px on both axes.
    let halo = quads_of(&lamp(10.0, &[]), 1)[0];
    assert_eq!(halo.glow.unwrap().radius_uv, [0.5, 1.0]);
    // A radius past the art, in the shown frame or in one the clock never
    // selects, is rejected instead of being truncated.
    for view in [lamp(10.5, &[]), lamp(10.0, &[texture(4, 30, 8)])] {
        assert_eq!(
            compile(&view, SceneLimits::default()),
            Err(Error::InvalidValue)
        );
    }
}

#[test]
fn projection_overflow_is_rejected_rather_than_culled() {
    let tree = || {
        WorldObject::new(
            SemanticId(1),
            [0, 0],
            3,
            Anchor::Standing,
            texture(2, 16, 16),
        )
    };
    let mut view = iso_view(vec![tree()], HeightField::empty());
    view.camera.center = [f64::MAX, f64::MAX];
    assert_eq!(
        compile(&view, SceneLimits::default()),
        Err(Error::InvalidValue)
    );
    let bound = 1_099_511_627_776.0;
    assert_eq!(super::MAXIMUM_TILE_COORDINATE, 1 << 40);
    view.camera.center = [bound, -bound];
    assert!(compile(&view, SceneLimits::default()).is_ok());
    view.camera.center = [bound * 2.0, 0.0];
    assert_eq!(
        compile(&view, SceneLimits::default()),
        Err(Error::InvalidValue)
    );
    // Finite inputs whose projection overflows `f32` are invalid too.
    let mut lifted = iso_view(
        vec![tree()],
        HeightField {
            origin: [0, 0],
            width: 1,
            height: 1,
            heights: vec![Some(i32::MAX)],
        },
    );
    lifted.stretch.pixels_per_unit = f32::MAX;
    assert_eq!(
        compile(&lifted, SceneLimits::default()),
        Err(Error::InvalidValue)
    );
    let mut pushed = iso_view(vec![tree()], HeightField::empty());
    pushed.objects[0].offset_px = [f32::MAX, 0.0];
    pushed.camera.zoom = 8.0;
    assert_eq!(
        compile(&pushed, SceneLimits::default()),
        Err(Error::InvalidValue)
    );
}

#[test]
fn height_lookups_never_overflow() {
    // A malformed field whose dimensions overflow the row arithmetic.
    let field = HeightField {
        origin: [0, 0],
        width: u32::MAX,
        height: u32::MAX,
        heights: vec![Some(5)],
    };
    assert_eq!(field.get(0, 0), Some(5));
    assert_eq!(field.get(1, 3_000_000_000), None);
    assert_eq!(field.get(i64::MIN, 0), None);
}

#[test]
fn quad_spans_are_validated_before_culling() {
    let limits = SceneLimits {
        maximum_dimension: 256,
        ..SceneLimits::default()
    };
    let art = |tile, side| {
        WorldObject::new(
            SemanticId(1),
            tile,
            3,
            Anchor::Standing,
            texture(2, side, side),
        )
    };
    for tile in [[0, 0], [40, 40]] {
        let mut view = iso_view(vec![art(tile, 31)], HeightField::empty());
        view.camera.zoom = 8.0;
        // 31 px art at 8× spans 248 px, within 256 less the 1 px margin.
        assert!(compile(&view, limits).is_ok(), "{tile:?}");
        // 32 px spans 256 px; in view or culled, the view is invalid.
        view.objects = vec![art(tile, 32)];
        assert_eq!(compile(&view, limits), Err(Error::InvalidValue), "{tile:?}");
        // Rotation widens the bounding box: 248 px at 45° spans 351 px.
        view.objects = vec![art(tile, 31)];
        view.objects[0].rotation_degrees = 45.0;
        assert_eq!(compile(&view, limits), Err(Error::InvalidValue), "{tile:?}");
        // Glow padding counts too: 28 px art with a 2 px halo spans 256 px.
        view.objects = vec![art(tile, 28)];
        view.objects[0].glow = Some(GlowEffect {
            color: [1.0; 4],
            radius_px: 2.0,
            pulse_millis: 0,
        });
        assert_eq!(compile(&view, limits), Err(Error::InvalidValue), "{tile:?}");
        // So does a frame the clock does not select yet.
        view.objects = vec![art(tile, 31)];
        view.objects[0].animation = Animation {
            frames: vec![texture(2, 31, 31), texture(3, 32, 32)],
            frame_millis: 100,
            phase_millis: 0,
        };
        assert_eq!(compile(&view, limits), Err(Error::InvalidValue), "{tile:?}");
    }
}

#[test]
fn bent_ground_spans_do_not_depend_on_position() {
    let limits = SceneLimits {
        maximum_dimension: 256,
        ..SceneLimits::default()
    };
    // A 1,200-unit step stretches each quarter-tile cell past 255 px.
    for tile in [[0, 0], [40, 40]] {
        let heights = HeightField {
            origin: tile,
            width: 2,
            height: 1,
            heights: vec![Some(0), Some(1_200)],
        };
        let mut view = iso_view(vec![ground(1, tile)], heights);
        view.stretch.rule = CornerRule::Max;
        assert_eq!(compile(&view, limits), Err(Error::InvalidValue), "{tile:?}");
        view.heights.heights[1] = Some(100);
        assert!(compile(&view, limits).is_ok(), "{tile:?}");
    }
}

#[test]
fn double_draw_is_for_opaque_art() {
    let tall = || {
        let mut object = WorldObject::new(
            SemanticId(1),
            [0, 0],
            3,
            Anchor::Standing,
            texture(2, 20, 80),
        );
        object.double_draw = true;
        object
    };
    let compiles = |object: WorldObject| {
        compile(
            &iso_view(vec![object], HeightField::empty()),
            SceneLimits::default(),
        )
    };
    assert!(compiles(tall()).is_ok());
    let mut faded = tall();
    faded.alpha = 0.5;
    assert_eq!(compiles(faded.clone()), Err(Error::InvalidValue));
    let mut glowing = tall();
    glowing.glow = Some(GlowEffect {
        color: [1.0; 4],
        radius_px: 2.0,
        pulse_millis: 0,
    });
    assert_eq!(compiles(glowing.clone()), Err(Error::InvalidValue));
    // Without the repeat, the same effects are fine.
    for mut object in [faded, glowing] {
        object.double_draw = false;
        assert!(compiles(object).is_ok());
    }
}

#[test]
fn ground_cannot_fill_height_field_holes() {
    let field = HeightField {
        origin: [0, 0],
        width: 2,
        height: 1,
        heights: vec![Some(4), None],
    };
    // The explicit hole at [1, 0] has no ground to draw.
    let holed = iso_view(vec![ground(1, [0, 0]), ground(2, [1, 0])], field.clone());
    assert_eq!(
        compile(&holed, SceneLimits::default()),
        Err(Error::InvalidValue)
    );
    // Standing art may still stand there, and tiles outside the field count
    // as height 0, like a view with no field at all.
    let mut tree = WorldObject::new(
        SemanticId(2),
        [1, 0],
        3,
        Anchor::Standing,
        texture(2, 16, 16),
    );
    let standing = iso_view(vec![ground(1, [0, 0]), tree.clone()], field.clone());
    assert!(compile(&standing, SceneLimits::default()).is_ok());
    tree.tile = [5, 5];
    let outside = iso_view(vec![ground(1, [0, 0]), ground(3, [5, 5]), tree], field);
    assert!(compile(&outside, SceneLimits::default()).is_ok());
}

#[test]
fn ground_footprints_cannot_cover_holes() {
    let mut view = iso_view(Vec::new(), HeightField::empty());
    view.camera.projection = Projection::TopDown { tile_size: 32 };
    view.heights = HeightField {
        origin: [0, 0],
        width: 3,
        height: 2,
        heights: vec![Some(0), None, Some(0), Some(0), Some(0), Some(0)],
    };
    let wide = |tile, footprint| {
        let mut object =
            WorldObject::new(SemanticId(1), tile, 0, Anchor::Ground, texture(1, 32, 32));
        object.footprint = footprint;
        object
    };
    // The head tile [0, 0] has ground, but the footprint reaches the hole.
    view.objects = vec![wide([0, 0], [2, 1])];
    assert_eq!(
        compile(&view, SceneLimits::default()),
        Err(Error::InvalidValue)
    );
    view.objects = vec![wide([-5, -5], [7, 6])];
    assert_eq!(
        compile(&view, SceneLimits::default()),
        Err(Error::InvalidValue)
    );
    // Footprints that miss the hole, or lie outside the field, are fine.
    for (tile, footprint) in [([0, 1], [3, 1]), ([2, 0], [1, 2]), ([5, 5], [9, 9])] {
        view.objects = vec![wide(tile, footprint)];
        assert!(
            compile(&view, SceneLimits::default()).is_ok(),
            "{tile:?} {footprint:?}"
        );
    }
}

#[test]
fn animation_frames_are_bounded_before_traversal() {
    let animated = |id, frames: usize| {
        let mut object = WorldObject::new(
            SemanticId(id),
            [40, 40],
            3,
            Anchor::Standing,
            texture(2, 16, 16),
        );
        object.animation.frames = vec![texture(2, 16, 16); frames];
        object
    };
    let limit = Err(Error::Scene(tilewright_scene::Error::LimitExceeded));
    let view = |objects| iso_view(objects, HeightField::empty());
    assert!(compile(&view(vec![animated(1, 1_024)]), SceneLimits::default()).is_ok());
    assert_eq!(
        compile(&view(vec![animated(1, 1_025)]), SceneLimits::default()),
        limit
    );
    // Together, a view's animations list at most twice the quad budget.
    let small = SceneLimits {
        maximum_quads: 2,
        ..SceneLimits::default()
    };
    assert!(compile(&view(vec![animated(1, 2), animated(2, 2)]), small).is_ok());
    assert_eq!(
        compile(&view(vec![animated(1, 3), animated(2, 2)]), small),
        limit
    );
}

#[test]
fn height_fields_are_bounded_before_scanning() {
    let side = 2_049_u32;
    let mut view = iso_view(
        Vec::new(),
        HeightField {
            origin: [0, 0],
            width: side,
            height: side,
            heights: vec![None; (side * side) as usize],
        },
    );
    assert_eq!(
        compile(&view, SceneLimits::default()),
        Err(Error::Scene(tilewright_scene::Error::LimitExceeded))
    );
    view.heights.width = 2_048;
    view.heights.height = 2_048;
    view.heights.heights.truncate(2_048 * 2_048);
    assert!(compile(&view, SceneLimits::default()).is_ok());
}

#[test]
fn every_emitted_quad_is_counted_before_culling() {
    let limits = |maximum_quads| SceneLimits {
        maximum_quads,
        ..SceneLimits::default()
    };
    // A glowing object emits a halo and its art, in view or not.
    for tile in [[0, 0], [40, 40]] {
        let mut lamp =
            WorldObject::new(SemanticId(1), tile, 3, Anchor::Standing, texture(3, 20, 20));
        lamp.glow = Some(GlowEffect {
            color: [1.0; 4],
            radius_px: 2.0,
            pulse_millis: 0,
        });
        let view = iso_view(vec![lamp], HeightField::empty());
        assert_eq!(
            compile(&view, limits(1)),
            Err(Error::Scene(tilewright_scene::Error::LimitExceeded)),
            "{tile:?}"
        );
        assert!(compile(&view, limits(2)).is_ok(), "{tile:?}");
    }
    // A bent isometric tile emits its whole mesh; a flat one a single quad.
    for tile in [[0, 0], [40, 40]] {
        let heights = HeightField {
            origin: tile,
            width: 2,
            height: 1,
            heights: vec![Some(0), Some(8)],
        };
        let bent = iso_view(vec![ground(1, tile)], heights);
        assert_eq!(
            compile(&bent, limits(15)),
            Err(Error::Scene(tilewright_scene::Error::LimitExceeded)),
            "{tile:?}"
        );
        assert!(compile(&bent, limits(16)).is_ok(), "{tile:?}");
        let flat = iso_view(vec![ground(1, tile)], HeightField::empty());
        assert!(compile(&flat, limits(1)).is_ok(), "{tile:?}");
    }
}

#[test]
fn animation_phase_holds_across_the_whole_clock() {
    let mut object = WorldObject::new(
        SemanticId(1),
        [0, 0],
        3,
        Anchor::Standing,
        texture(20, 16, 16),
    );
    object.animation = Animation {
        frames: vec![texture(20, 16, 16), texture(21, 16, 16)],
        frame_millis: 3,
        phase_millis: 1,
    };
    let mut view = iso_view(vec![object], HeightField::empty());
    // (2⁶⁴ − 1 + 1) / 3 is odd, so the second frame shows; a wrapped sum
    // would give 0 and the first.
    view.clock_millis = u64::MAX;
    assert_eq!(quads_of(&view, 1)[0].resource_id, ResourceId(21));
}

#[test]
fn texture_identities_are_bounded_across_every_frame() {
    // Off-screen objects and unselected frames count toward the bound.
    let named = |count: u64| {
        let mut object = WorldObject::new(
            SemanticId(1),
            [40, 40],
            3,
            Anchor::Standing,
            texture(1, 16, 16),
        );
        object.animation.frames = (1..=count).map(|id| texture(id, 16, 16)).collect();
        object.animation.frames.truncate(1_024);
        object
    };
    let objects = |total: u64| -> Vec<WorldObject> {
        (0..total.div_ceil(1_024))
            .map(|index| {
                let mut object = named(1_024);
                object.semantic_id = SemanticId(u32::try_from(index).unwrap() + 1);
                for (offset, frame) in object.animation.frames.iter_mut().enumerate() {
                    frame.id = ResourceId(index * 1_024 + offset as u64 + 1);
                }
                object
                    .animation
                    .frames
                    .truncate(usize::try_from((total - index * 1_024).min(1_024)).unwrap());
                object
            })
            .collect()
    };
    let view = |total| iso_view(objects(total), HeightField::empty());
    assert!(compile(&view(4_096), SceneLimits::default()).is_ok());
    assert_eq!(
        compile(&view(4_097), SceneLimits::default()),
        Err(Error::Scene(tilewright_scene::Error::LimitExceeded))
    );
}

#[test]
fn raster_work_is_charged_before_culling() {
    let limits = SceneLimits {
        maximum_raster_samples: 1_000,
        ..SceneLimits::default()
    };
    let art = |id, tile, side| {
        WorldObject::new(
            SemanticId(id),
            tile,
            3,
            Anchor::Standing,
            texture(2, side, side),
        )
    };
    for tile in [[0, 0], [40, 40]] {
        // Each 20 px object costs at most 21 × 21 samples wherever it lies.
        let two = iso_view(
            vec![art(1, tile, 20), art(2, tile, 20)],
            HeightField::empty(),
        );
        assert!(compile(&two, limits).is_ok(), "{tile:?}");
        let three = iso_view(
            vec![art(1, tile, 20), art(2, tile, 20), art(3, tile, 20)],
            HeightField::empty(),
        );
        assert_eq!(
            compile(&three, limits),
            Err(Error::Scene(tilewright_scene::Error::LimitExceeded)),
            "{tile:?}"
        );
        // A frame the clock does not select yet counts at its own size.
        let mut animated = art(1, tile, 10);
        animated.animation = Animation {
            frames: vec![texture(2, 10, 10), texture(3, 40, 40)],
            frame_millis: 100,
            phase_millis: 0,
        };
        assert_eq!(
            compile(&iso_view(vec![animated], HeightField::empty()), limits),
            Err(Error::Scene(tilewright_scene::Error::LimitExceeded)),
            "{tile:?}"
        );
    }
}

/// Edges and texture-coordinate ranges of an axis-aligned quad.
fn edges(quad: &Quad) -> ([f32; 4], [f32; 4]) {
    let rect = quad.aligned_rect().expect("ground is a rectangle");
    (
        [rect.left, rect.top, rect.right, rect.bottom],
        [
            rect.uv_min[0],
            rect.uv_min[1],
            rect.uv_max[0],
            rect.uv_max[1],
        ],
    )
}

#[test]
fn neighbouring_ground_shares_pixel_edges_at_fractional_zoom() {
    let top_down = |zoom| {
        let mut view = iso_view(Vec::new(), HeightField::empty());
        view.camera.projection = Projection::TopDown { tile_size: 32 };
        view.camera.center = [1.37, 0.81];
        (view.camera.width, view.camera.height) = (197, 129);
        view.camera.zoom = zoom;
        view.objects = [[0, 0], [1, 0], [0, 1]]
            .iter()
            .zip(1..)
            .map(|(tile, id)| {
                WorldObject::new(SemanticId(id), *tile, 0, Anchor::Ground, texture(1, 32, 32))
            })
            .collect();
        view
    };
    // Full-size isometric art abuts its neighbours across the row and down
    // the column.
    let isometric = |zoom| {
        let mut view = iso_view(
            [[0, 0], [1, -1], [1, 1]]
                .iter()
                .zip(1..)
                .map(|(tile, id)| {
                    WorldObject::new(SemanticId(id), *tile, 1, Anchor::Ground, texture(1, 48, 24))
                })
                .collect(),
            HeightField::empty(),
        );
        view.camera.center = [0.63, 0.29];
        (view.camera.width, view.camera.height) = (197, 129);
        view.camera.zoom = zoom;
        view
    };
    for zoom in [0.3, 0.7, 1.1, 1.3, 2.6] {
        for view in [top_down(zoom), isometric(zoom)] {
            let (first, _) = edges(&quads_of(&view, 1)[0]);
            let (across, _) = edges(&quads_of(&view, 2)[0]);
            let (down, _) = edges(&quads_of(&view, 3)[0]);
            assert_eq!(first[2], across[0], "zoom {zoom}: right edge");
            assert_eq!(first[3], down[1], "zoom {zoom}: bottom edge");
            assert!(first.iter().all(|edge| edge.fract() == 0.0), "{first:?}");
        }
    }
}

#[test]
fn ground_texels_keep_their_world_positions_at_fractional_zoom() {
    // Diagonal neighbours overlap by half a tile, so one screen point lies
    // in both: it must fall 24 texels further right and 12 further down in
    // the western tile's art than in the eastern's, as it does unsnapped.
    for zoom in [0.3_f32, 0.7, 1.1, 1.3, 2.6] {
        let mut view = iso_view(
            vec![ground(1, [0, 0]), ground(2, [1, 0])],
            HeightField::empty(),
        );
        view.camera.center = [0.63, 0.29];
        view.camera.zoom = zoom;
        let texel = |id, axis: usize, at: f32| {
            let (edges, uvs) = edges(&quads_of(&view, id)[0]);
            let size = [48.0, 23.0][axis];
            let along = (at - edges[axis]) / (edges[axis + 2] - edges[axis]);
            (uvs[axis] + along * (uvs[axis + 2] - uvs[axis])) * size
        };
        let point = [52.5, 38.5];
        for (axis, offset) in [(0, 24.0), (1, 12.0)] {
            let shift = texel(1, axis, point[axis]) - texel(2, axis, point[axis]);
            assert!(
                (shift - offset).abs() < 0.01,
                "zoom {zoom} axis {axis}: {shift}"
            );
        }
        // Every pixel centre the art covers samples inside it.
        for id in [1, 2] {
            let (_, uvs) = edges(&quads_of(&view, id)[0]);
            assert!(uvs.iter().all(|uv| uv.abs() < 1.5), "{uvs:?}");
        }
    }
}

#[test]
fn whole_zooms_keep_ground_art_on_the_pixel_grid() {
    // The art moves onto the pixel grid as one piece and keeps its whole
    // texture, so pixel centres sample texel centres.
    for zoom in [1.0_f32, 2.0, 3.0] {
        let mut view = iso_view(
            vec![ground(1, [0, 0]), ground(2, [1, 0])],
            HeightField::empty(),
        );
        view.camera.center = [0.63, 0.29];
        view.camera.zoom = zoom;
        for id in [1, 2] {
            let (edges, uvs) = edges(&quads_of(&view, id)[0]);
            assert_eq!(uvs, [0.0, 0.0, 1.0, 1.0]);
            assert!(edges.iter().all(|edge| edge.fract() == 0.0), "{edges:?}");
            assert_eq!(
                [edges[2] - edges[0], edges[3] - edges[1]],
                [48.0 * zoom, 23.0 * zoom]
            );
        }
    }
}

#[test]
fn standing_art_keeps_its_exact_size_at_fractional_zoom() {
    // Standing art moves freely rather than tiling, so it keeps its scaled
    // size wherever it lies instead of gaining or losing a pixel.
    for center in [[0.5, 0.5], [0.61, 0.47], [0.83, 0.12]] {
        let mut view = iso_view(
            vec![WorldObject::new(
                SemanticId(1),
                [0, 0],
                3,
                Anchor::Standing,
                texture(2, 40, 60),
            )],
            HeightField::empty(),
        );
        view.camera.center = center;
        view.camera.zoom = 0.7;
        let (edges, uvs) = edges(&quads_of(&view, 1)[0]);
        assert_eq!(uvs, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!([edges[0].fract(), edges[1].fract()], [0.0, 0.0]);
        assert!((edges[2] - edges[0] - 28.0).abs() < 1e-4, "{edges:?}");
        assert!((edges[3] - edges[1] - 42.0).abs() < 1e-4, "{edges:?}");
    }
}

#[test]
fn ground_under_a_pixel_keeps_texture_coordinates_in_bounds() {
    // Tiles a tenth of a pixel across either cover one pixel or none.
    let mut view = iso_view(Vec::new(), HeightField::empty());
    view.camera.projection = Projection::TopDown { tile_size: 2 };
    view.camera.zoom = 0.05;
    view.objects = (0..400)
        .map(|index| {
            WorldObject::new(
                SemanticId(index + 1),
                [i64::from(index % 20) - 10, i64::from(index / 20) - 10],
                0,
                Anchor::Ground,
                texture(1, 2, 2),
            )
        })
        .collect();
    let scene = compile(&view, SceneLimits::default()).unwrap();
    assert!(!scene.quads().is_empty());
    for quad in scene.quads() {
        let (edges, uvs) = edges(quad);
        assert_eq!(uvs, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!([edges[2] - edges[0], edges[3] - edges[1]], [1.0, 1.0]);
    }
}
