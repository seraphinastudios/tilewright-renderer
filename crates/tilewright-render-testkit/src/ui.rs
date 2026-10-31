// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! Shared compiler/font conformance, over a world with explicit UI blockers.

use std::sync::Arc;
use tilewright_render_resources::Error as ResourceError;
use tilewright_render_ui::{
    BitmapFontSet, CompiledUi, UiDisplayList, UiLimits, UiPaint, UiPrimitive, UiRect, UiTexture,
    UiViewport, builtin, compile_ui, validate_ui_composition,
};
use tilewright_scene::{
    Digest256, Layer, Quad, ResourceId, Sampling, SceneBundle, SceneLimits, SemanticId, Viewport,
};

use crate::{FakeResourceProvider, world_provider, world_texture};

/// Reserved independently of all eight original corpus resource identities.
pub const UI_ATLAS_ID: ResourceId = ResourceId(301);
pub const UI_SOLID_ID: ResourceId = ResourceId(302);
pub const UI_WORLD_ID: SemanticId = SemanticId(9000);
const LOGICAL: [u32; 2] = [320, 192];

fn texture(id: ResourceId, digest: [u8; 32]) -> UiTexture {
    UiTexture {
        id,
        revision: 1,
        digest: Digest256(digest),
    }
}

/// Supplies the actual immutable builtin atlas/solid envelopes and world art.
///
/// # Errors
/// Returns a provider error if its internal test lock is poisoned.
pub fn shared_ui_provider() -> Result<Arc<FakeResourceProvider>, ResourceError> {
    let provider = world_provider()?;
    provider.insert(UI_ATLAS_ID, 1, builtin::ATLAS_BYTES.to_vec())?;
    provider.insert(UI_SOLID_ID, 1, builtin::SOLID_BYTES.to_vec())?;
    Ok(provider)
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> UiRect {
    UiRect {
        x,
        y,
        width,
        height,
    }
}

// Semantic IDs and painter order are explicit. Every solid is opaque and
// pickable: even blank panel space blocks the world identity behind it.
fn solids() -> [(u32, UiRect, [f32; 4]); 9] {
    [
        (9100, rect(0.0, 0.0, 320.0, 28.0), [0.10, 0.13, 0.18, 1.0]),
        (9101, rect(0.0, 28.0, 72.0, 164.0), [0.12, 0.16, 0.21, 1.0]),
        (
            9102,
            rect(176.0, 28.0, 144.0, 164.0),
            [0.09, 0.12, 0.16, 1.0],
        ),
        (9103, rect(4.0, 4.0, 52.0, 20.0), [0.24, 0.35, 0.49, 1.0]),
        (
            9104,
            rect(180.0, 76.0, 136.0, 24.0),
            [0.16, 0.30, 0.47, 1.0],
        ),
        (9105, rect(244.0, 80.0, 0.8, 16.0), [1.0, 0.83, 0.30, 1.0]),
        (9106, rect(8.0, 64.0, 24.0, 24.0), [0.23, 0.61, 0.27, 1.0]),
        (9107, rect(40.0, 64.0, 24.0, 24.0), [0.65, 0.56, 0.35, 1.0]),
        (9108, rect(8.0, 96.0, 24.0, 24.0), [0.48, 0.50, 0.56, 1.0]),
    ]
}

fn compiled(width: u32, height: u32) -> Result<CompiledUi, tilewright_render_ui::UiError> {
    let target = rect(0.0, 0.0, 320.0, 192.0);
    let mut primitives: Vec<_> = solids()
        .into_iter()
        .enumerate()
        .map(|(order, (id, bounds, color))| UiPrimitive::Solid {
            bounds,
            clip: target,
            paint: UiPaint {
                semantic_id: SemanticId(id),
                painter_order: u32::try_from(order).unwrap_or(0),
                color,
            },
            pickable: true,
        })
        .collect();
    for (index, (origin, text, clip)) in [
        ([8.0, 4.0], "Save", target),
        ([64.0, 4.0], "Undo  Redo  Paint", target),
        ([8.0, 32.0], "Palette", target),
        ([184.0, 32.0], "Inspector", target),
        ([184.0, 52.0], "Name: Caf\u{e9}", target),
        ([184.0, 80.0], "Stone", target),
        ([184.0, 104.0], "Value: 42", target),
        ([184.0, 128.0], "Unknown: \u{1f680}", target),
        // A scrolled row is cut through real glyph ink at fractional edges.
        (
            [182.0, 152.0],
            "clipped scrolling label",
            rect(185.25, 157.25, 126.5, 9.5),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        primitives.push(UiPrimitive::Text {
            origin,
            clip,
            paint: UiPaint {
                semantic_id: SemanticId(9200 + u32::try_from(index).unwrap_or(0)),
                painter_order: 20 + u32::try_from(index).unwrap_or(0),
                color: [0.94, 0.96, 1.0, 1.0],
            },
            font: builtin::FONT_ID,
            size: 12.0,
            text,
        });
    }
    let font = builtin::builtin_font(texture(UI_ATLAS_ID, builtin::ATLAS_SHA256))?;
    let fonts = [font];
    compile_ui(
        UiViewport {
            logical_size: LOGICAL,
            physical_size: [width, height],
        },
        UiDisplayList {
            primitives: &primitives,
        },
        &BitmapFontSet::new(&fonts)?,
        texture(UI_SOLID_ID, builtin::SOLID_SHA256),
        UiLimits::default(),
    )
}

/// Compiles real panel/text/caret/palette primitives at DPI 1.25 or 1.5.
///
/// # Errors
/// Returns a scene validation error for targets outside these bounded cases,
/// or an invalid compiler/composition result.
#[allow(clippy::cast_precision_loss)]
pub fn shared_ui_scene(width: u32, height: u32) -> Result<SceneBundle, tilewright_scene::Error> {
    if !matches!((width, height), (400, 240) | (480, 288)) {
        return Err(tilewright_scene::Error::InvalidValue);
    }
    let mut world = vec![Quad {
        semantic_id: UI_WORLD_ID,
        part: 0,
        resource_id: ResourceId(105),
        resource_revision: 1,
        resource_digest: world_texture(105).digest,
        frame: None,
        positions: [
            [0.0, 0.0],
            [width as f32, 0.0],
            [width as f32, height as f32],
            [0.0, height as f32],
        ],
        uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        depth: -7,
        painter_order: 0,
        color: [1.0; 4],
        grayscale: 0.0,
        glow: None,
        pickable: true,
        sampling: Sampling::Nearest,
        layer: Layer::World,
    }];
    let ui = compiled(width, height).map_err(|_| tilewright_scene::Error::InvalidValue)?;
    validate_ui_composition(&world, &ui).map_err(|_| tilewright_scene::Error::InvalidValue)?;
    world.extend(ui.quads);
    SceneBundle::from_quads(
        8,
        375,
        Viewport {
            width,
            height,
            scale_milli: width * 1000 / LOGICAL[0],
        },
        [0.0, 0.0, 0.0, 1.0],
        world,
        SceneLimits::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReferenceRenderer, offscreen_target};
    use std::collections::HashSet;
    use tilewright_render_api::{RenderRequest, Renderer};

    #[test]
    fn opaque_nonpickable_panel_preserves_world_mask_and_requires_application_routing() {
        let original = shared_ui_scene(400, 240).unwrap();
        let quads = original.quads().iter().copied().map(|quad| Quad {
            pickable: quad.pickable && quad.semantic_id != SemanticId(9102),
            ..quad
        });
        let scene = SceneBundle::from_quads(
            original.revision(),
            original.clock_millis(),
            original.viewport(),
            original.clear_color(),
            quads,
            SceneLimits::default(),
        )
        .unwrap();
        let frame = ReferenceRenderer::new(offscreen_target(400, 240))
            .unwrap()
            .render(RenderRequest {
                scene: &scene,
                resources: shared_ui_provider().unwrap(),
            })
            .unwrap();
        let blank = 236 * 400 + 396;
        assert_eq!(frame.semantic_ids[blank], UI_WORLD_ID.0);
        assert_eq!(frame.depth[blank], -7);
        assert_eq!(frame.coverage[blank], 255);
        assert_eq!(&frame.rgba8[blank * 4..blank * 4 + 4], &[23, 31, 41, 255]);
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn compiled_font_panels_have_exact_full_semantic_depth_and_coverage_planes() {
        for (width, height) in [(400, 240), (480, 288)] {
            let scene = shared_ui_scene(width, height).unwrap();
            let ui = compiled(width, height).unwrap();
            assert_eq!(ui.unsupported.len(), 1);
            assert_eq!(ui.unsupported[0].character, '\u{1f680}');
            let unsupported = ui
                .text_layouts
                .iter()
                .flat_map(|layout| &layout.glyphs)
                .find(|glyph| !glyph.supported)
                .unwrap();
            assert_eq!((unsupported.byte_start, unsupported.byte_end), (9, 13));
            assert!(ui.quads.iter().any(
                |quad| quad.resource_id == UI_ATLAS_ID && quad.semantic_id == SemanticId(9207)
            ));
            let keys: HashSet<_> = scene
                .quads()
                .iter()
                .map(|quad| (quad.semantic_id, quad.part))
                .collect();
            assert_eq!(keys.len(), scene.quads().len());
            assert_eq!(
                ui.quads
                    .iter()
                    .filter(|quad| quad.semantic_id == SemanticId(9208))
                    .map(|quad| quad.positions[0][1])
                    .fold(f32::INFINITY, f32::min)
                    .to_bits(),
                (157.25 * width as f32 / 320.0).to_bits()
            );
            let frame = ReferenceRenderer::new(offscreen_target(width, height))
                .unwrap()
                .render(RenderRequest {
                    scene: &scene,
                    resources: shared_ui_provider().unwrap(),
                })
                .unwrap();
            let scale = width as f32 / 320.0;
            for y in 0..height {
                for x in 0..width {
                    let p = [(x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale];
                    let mut expected = UI_WORLD_ID.0;
                    for (id, bounds, _) in solids() {
                        if p[0] >= bounds.x
                            && p[0] < bounds.x + bounds.width
                            && p[1] >= bounds.y
                            && p[1] < bounds.y + bounds.height
                        {
                            expected = id;
                        }
                    }
                    let index = (y * width + x) as usize;
                    assert_eq!(
                        frame.semantic_ids[index], expected,
                        "{width}x{height} pixel {x},{y}"
                    );
                    assert_eq!(
                        frame.depth[index],
                        if expected == UI_WORLD_ID.0 { -7 } else { 0 }
                    );
                    assert_eq!(frame.coverage[index], 255);
                }
            }
            // Blank inspector space still has a UI blocker, never world ID.
            let index = ((height - 4) * width + width - 4) as usize;
            assert_eq!(frame.semantic_ids[index], 9102);
            assert_eq!(frame.metrics.resource_requests, 3);
            assert!(
                frame
                    .rgba8
                    .chunks_exact(4)
                    .any(|pixel| pixel[0] > 180 && pixel[1] > 180 && pixel[2] > 180)
            );
        }
        assert!(shared_ui_scene(320, 192).is_err());
    }
}
