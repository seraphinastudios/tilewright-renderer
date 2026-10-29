// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! Frame-time harness: renders testkit's crowd scene, a ground of atlas
//! tiles under walking three-layer characters, for a number of frames and
//! reports frame-time percentiles against the 60 and 30 FPS budgets.
//!
//! ```sh
//! cargo run --release -p tilewright-render-wgpu --example frame_time -- \
//!   --width 1920 --height 1080 --characters 1000 --frames 300 --sheets atlas
//! ```
//!
//! Each frame builds the scene (positions and walk frames change every
//! frame), draws it offscreen, and waits for the GPU. The frame time is that
//! whole sequence, run without overlap, so it bounds what a game loop that
//! overlaps CPU and GPU work would see. `--sheets layers` stores each layer
//! in its own sheet, as layered sheets usually ship; `--sheets atlas` packs
//! them into one atlas. `--json` prints one JSON object. Select a Vulkan
//! driver with `VK_DRIVER_FILES`, for example Mesa's lavapipe.

use std::time::Instant;
use tilewright_render_api::{BackendPreference, RenderRequest, TargetDescriptor, TargetKind};
use tilewright_render_testkit::{Crowd, CrowdSheets, crowd_provider, crowd_scene};
use tilewright_render_wgpu::WgpuRenderer;

struct Options {
    crowd: Crowd,
    frames: usize,
    warmup: usize,
    json: bool,
}

fn parse() -> Result<Options, String> {
    let mut options = Options {
        crowd: Crowd {
            width: 1_920,
            height: 1_080,
            characters: 1_000,
            sheets: CrowdSheets::Atlas,
        },
        frames: 300,
        warmup: 30,
        json: false,
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} needs a value"))
        };
        let number = |text: String| {
            text.parse::<u32>()
                .map_err(|error| format!("{argument}: {error}"))
        };
        match argument.as_str() {
            "--width" => options.crowd.width = number(value()?)?,
            "--height" => options.crowd.height = number(value()?)?,
            "--characters" => options.crowd.characters = number(value()?)?,
            "--frames" => options.frames = number(value()?)? as usize,
            "--warmup" => options.warmup = number(value()?)? as usize,
            "--sheets" => {
                options.crowd.sheets = match value()?.as_str() {
                    "atlas" => CrowdSheets::Atlas,
                    "layers" => CrowdSheets::PerLayer,
                    other => return Err(format!("unknown --sheets {other}")),
                };
            }
            "--json" => options.json = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if options.frames == 0 {
        return Err("--frames must be positive".to_owned());
    }
    Ok(options)
}

/// The value at a percentile of sorted samples, by nearest rank.
fn percentile(sorted: &[f64], percent: f64) -> f64 {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    let rank = ((percent / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

struct Summary {
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
}

fn summarise(samples: &mut [f64]) -> Summary {
    samples.sort_by(f64::total_cmp);
    Summary {
        p50: percentile(samples, 50.0),
        p95: percentile(samples, 95.0),
        p99: percentile(samples, 99.0),
        max: samples[samples.len() - 1],
    }
}

#[allow(clippy::cast_precision_loss)]
fn within(samples: &[f64], budget: f64) -> f64 {
    samples.iter().filter(|sample| **sample <= budget).count() as f64 * 100.0 / samples.len() as f64
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse()?;
    let crowd = options.crowd;
    let mut renderer = WgpuRenderer::new(TargetDescriptor {
        kind: TargetKind::Offscreen,
        width: crowd.width,
        height: crowd.height,
        backend: BackendPreference::Automatic,
    })?;
    let provider = crowd_provider(crowd.sheets)?;
    let mut build = Vec::with_capacity(options.frames);
    let mut submit = Vec::with_capacity(options.frames);
    let mut frames = Vec::with_capacity(options.frames);
    let mut quads = 0;
    for index in 0..options.warmup + options.frames {
        let started = Instant::now();
        let scene = crowd_scene(crowd, index as u64)?;
        let scene_ready = Instant::now();
        renderer.draw(&RenderRequest {
            scene: &scene,
            resources: provider.clone(),
        })?;
        let drawn = Instant::now();
        pollster::block_on(renderer.finish())?;
        let finished = Instant::now();
        if index >= options.warmup {
            let millis = |from: Instant, to: Instant| (to - from).as_secs_f64() * 1_000.0;
            build.push(millis(started, scene_ready));
            submit.push(millis(scene_ready, drawn));
            frames.push(millis(started, finished));
        }
        quads = scene.quads().len();
    }
    let frame_60 = within(&frames, 1_000.0 / 60.0);
    let frame_30 = within(&frames, 1_000.0 / 30.0);
    let cpu: Vec<f64> = build.iter().zip(&submit).map(|(a, b)| a + b).collect();
    let [frame, cpu, build, submit] =
        [frames, cpu, build, submit].map(|mut samples| summarise(&mut samples));
    let adapter = renderer.adapter();
    let sheets = match crowd.sheets {
        CrowdSheets::Atlas => "atlas",
        CrowdSheets::PerLayer => "layers",
    };
    if options.json {
        let stats = |summary: &Summary| {
            format!(
                "{{\"p50\":{:.3},\"p95\":{:.3},\"p99\":{:.3},\"max\":{:.3}}}",
                summary.p50, summary.p95, summary.p99, summary.max
            )
        };
        println!(
            "{{\"adapter\":{:?},\"backend\":{:?},\"width\":{},\"height\":{},\"characters\":{},\"sheets\":\"{sheets}\",\"quads\":{quads},\"frames\":{},\"frame_ms\":{},\"cpu_ms\":{},\"build_ms\":{},\"submit_ms\":{},\"within_60fps_percent\":{frame_60:.1},\"within_30fps_percent\":{frame_30:.1}}}",
            adapter.name,
            adapter.backend,
            crowd.width,
            crowd.height,
            crowd.characters,
            options.frames,
            stats(&frame),
            stats(&cpu),
            stats(&build),
            stats(&submit),
        );
        return Ok(());
    }
    println!(
        "adapter: {} ({}, {})",
        adapter.name, adapter.backend, adapter.device_type
    );
    println!(
        "scene: {}×{}, {} characters × 3 layers, {sheets} sheets, {quads} quads; {} frames after {} warm-up",
        crowd.width, crowd.height, crowd.characters, options.frames, options.warmup
    );
    println!("                     p50      p95      p99      max   (ms)");
    for (name, summary) in [
        ("frame (CPU + GPU)", &frame),
        ("CPU", &cpu),
        ("  scene build", &build),
        ("  draw and submit", &submit),
    ] {
        println!(
            "{name:<18} {:>8.2} {:>8.2} {:>8.2} {:>8.2}",
            summary.p50, summary.p95, summary.p99, summary.max
        );
    }
    println!(
        "frames within 16.7 ms (60 FPS): {frame_60:.1}%; within 33.3 ms (30 FPS): {frame_30:.1}%"
    );
    Ok(())
}
