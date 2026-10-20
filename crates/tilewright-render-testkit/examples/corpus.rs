// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

use sha2::{Digest, Sha256};
use std::{fmt::Write as _, time::Instant};
use tilewright_render_api::{RenderRequest, Renderer};
use tilewright_render_testkit::{
    ReferenceRenderer, atlas_provider, atlas_scene, dense_scene, iso_world_scene, offscreen_target,
    structural_scene, synthetic_provider, synthetic_scene, top_down_world_scene, world_provider,
};

/// The resources a corpus scenario draws from.
#[derive(Clone, Copy)]
enum Resources {
    Synthetic,
    World,
    Atlas,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("{{\"schema_version\":1,\"cases\":[");
    let cases = [
        (
            "overlap-square",
            synthetic_scene(16, 16)?,
            50_000_u64,
            Resources::Synthetic,
        ),
        (
            "unaligned-wide",
            synthetic_scene(31, 17)?,
            50_000,
            Resources::Synthetic,
        ),
        (
            "structural-fog-ui",
            structural_scene(48, 48)?,
            75_000,
            Resources::Synthetic,
        ),
        (
            "high-density-1024",
            dense_scene(128, 128)?,
            250_000,
            Resources::Synthetic,
        ),
        (
            "world-iso-terrain",
            iso_world_scene(192, 128)?,
            400_000,
            Resources::World,
        ),
        (
            "world-top-down",
            top_down_world_scene(96, 64)?,
            200_000,
            Resources::World,
        ),
        (
            "atlas-layered-characters",
            atlas_scene(112, 64)?,
            200_000,
            Resources::Atlas,
        ),
    ];
    for (index, (name, scene, maximum_cpu_micros, resources)) in cases.into_iter().enumerate() {
        let viewport = scene.viewport();
        let started = Instant::now();
        let frame = ReferenceRenderer::new(offscreen_target(viewport.width, viewport.height))?
            .render(RenderRequest {
                scene: &scene,
                resources: match resources {
                    Resources::Synthetic => synthetic_provider()?,
                    Resources::World => world_provider()?,
                    Resources::Atlas => atlas_provider()?,
                },
            })?;
        let elapsed_micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        if elapsed_micros > maximum_cpu_micros {
            return Err(format!(
                "scenario {name} exceeded CPU budget: {elapsed_micros} > {maximum_cpu_micros} microseconds"
            )
            .into());
        }
        let semantic = frame
            .semantic_ids
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        let depth = frame
            .depth
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        println!(
            "{}{{\"name\":\"{}\",\"width\":{},\"height\":{},\"clock_millis\":{},\"maximum_rgba_channel_difference\":1,\"performance\":{{\"maximum_cpu_micros\":{},\"quad_count\":{},\"resource_requests\":{},\"uploaded_bytes\":{},\"vertex_count\":{},\"vertex_allocation_bytes\":{},\"target_allocation_bytes\":{}}},\"digests\":{{\"rgba8\":\"{}\",\"semantic_u32le\":\"{}\",\"depth_i16le\":\"{}\",\"coverage_u8\":\"{}\"}}}}",
            if index == 0 { "" } else { "," },
            name,
            viewport.width,
            viewport.height,
            scene.clock_millis(),
            maximum_cpu_micros,
            scene.quads().len(),
            frame.metrics.resource_requests,
            frame.metrics.uploaded_bytes,
            frame.metrics.vertex_count,
            frame.metrics.vertex_allocation_bytes,
            frame.metrics.target_allocation_bytes,
            digest(&frame.rgba8),
            digest(&semantic),
            digest(&depth),
            digest(&frame.coverage),
        );
    }
    println!("]}}");
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}
