// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! Fill-rate benchmark: renders full-target layers offscreen and reports
//! the median and 90th-percentile frame time, readback included.
//!
//! ```sh
//! cargo run --release -p tilewright-render-wgpu --example fill_rate -- MODE
//! ```
//!
//! `MODE` is `empty` (clear and readback only), `nearest`, `linear`,
//! `glow-nearest` or `glow-linear`, optionally prefixed with `framed-` to
//! sample half of the texture as an atlas frame. Plain modes draw 32
//! layers of 1,024 × 1,024 pixels; glow modes draw 7 halo layers, the most
//! the default raster-sample budget admits. Select a Vulkan driver with
//! `VK_DRIVER_FILES`, for example Mesa's lavapipe.

use std::time::Instant;
use tilewright_render_api::{RenderRequest, Renderer};
use tilewright_render_testkit::{offscreen_target, synthetic_digest, synthetic_provider};
use tilewright_render_wgpu::WgpuRenderer;
use tilewright_scene::{
    Glow, Layer, Quad, ResourceId, Sampling, SceneBundle, SceneLimits, SemanticId, TextureRect,
    Viewport,
};

const SIDE: u32 = 1_024;
const WARMUP_FRAMES: usize = 3;
const MEASURED_FRAMES: usize = 25;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "nearest".to_owned());
    let (framed, base) = mode
        .strip_prefix("framed-")
        .map_or((false, mode.as_str()), |base| (true, base));
    let (layers, sampling, glow) = match base {
        "empty" => (0, Sampling::Nearest, false),
        "nearest" => (32, Sampling::Nearest, false),
        "linear" => (32, Sampling::Linear, false),
        "glow-nearest" => (7, Sampling::Nearest, true),
        "glow-linear" => (7, Sampling::Linear, true),
        _ => return Err(format!("unknown mode {mode}").into()),
    };
    #[allow(clippy::cast_precision_loss)]
    let side = SIDE as f32;
    let quads = (0..layers).map(|index| Quad {
        semantic_id: SemanticId(index + 1),
        part: 0,
        resource_id: ResourceId(1),
        resource_revision: 1,
        resource_digest: synthetic_digest(),
        frame: framed.then_some(TextureRect {
            x: 0,
            y: 0,
            width: 1,
            height: 2,
        }),
        positions: [[0.0, 0.0], [side, 0.0], [side, side], [0.0, side]],
        uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        depth: 0,
        painter_order: index,
        color: [1.0, 1.0, 1.0, 0.5],
        grayscale: 0.0,
        glow: glow.then_some(Glow {
            color: [1.0, 1.0, 0.0, 1.0],
            radius_uv: [0.2, 0.2],
        }),
        // Picking planes are CPU work; leave them out of a fill benchmark.
        pickable: false,
        sampling,
        layer: Layer::World,
    });
    let scene = SceneBundle::from_quads(
        1,
        0,
        Viewport {
            width: SIDE,
            height: SIDE,
            scale_milli: 1_000,
        },
        [0.0, 0.0, 0.0, 1.0],
        quads,
        SceneLimits::default(),
    )?;
    let mut renderer = WgpuRenderer::new(offscreen_target(SIDE, SIDE))?;
    let provider = synthetic_provider()?;
    let mut frame = || {
        renderer.render(RenderRequest {
            scene: &scene,
            resources: provider.clone(),
        })
    };
    for _ in 0..WARMUP_FRAMES {
        frame()?;
    }
    let mut millis = Vec::with_capacity(MEASURED_FRAMES);
    for _ in 0..MEASURED_FRAMES {
        let started = Instant::now();
        frame()?;
        millis.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    millis.sort_by(f64::total_cmp);
    println!(
        "adapter={:?} mode={mode} median_ms={:.2} p90_ms={:.2}",
        renderer.adapter().name,
        millis[MEASURED_FRAMES / 2],
        millis[MEASURED_FRAMES * 9 / 10],
    );
    Ok(())
}
