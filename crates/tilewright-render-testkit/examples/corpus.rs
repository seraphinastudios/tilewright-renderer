// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

use sha2::{Digest, Sha256};
use std::{fmt::Write as _, time::Instant};
use tilewright_render_api::{RenderRequest, Renderer};
use tilewright_render_testkit::{
    ATLAS_ID, ReferenceRenderer, atlas_digest, atlas_provider, atlas_scene, dense_scene,
    iso_world_scene, offscreen_target, structural_scene, synthetic_digest, synthetic_provider,
    synthetic_scene, top_down_world_scene, world_provider, world_texture,
};
use tilewright_scene::Digest256;

/// The resources a corpus scenario draws from.
#[derive(Clone, Copy)]
enum Resources {
    Synthetic,
    World,
    Atlas,
}

/// Every fixture texture the scenarios draw: its name, identity, size and
/// digest. The manifest records each one's provenance beside these.
fn resources() -> Vec<(&'static str, u64, u32, u32, Digest256)> {
    let mut resources = vec![("synthetic-2x2", 1, 2, 2, synthetic_digest())];
    for (name, id) in [
        ("world-grass", 101),
        ("world-sand", 102),
        ("world-pillar", 103),
        ("world-orb", 104),
        ("world-meadow", 105),
        ("world-stone", 106),
    ] {
        let texture = world_texture(id);
        resources.push((name, id, texture.width, texture.height, texture.digest));
    }
    resources.push(("atlas-character-sheet", ATLAS_ID.0, 64, 64, atlas_digest()));
    resources
}

fn print_resources() {
    for (index, (name, id, width, height, sha256)) in resources().into_iter().enumerate() {
        println!(
            "{}{{\"name\":\"{name}\",\"resource_id\":{id},\"revision\":1,\"width\":{width},\"height\":{height},\"sha256\":\"{}\"}}",
            if index == 0 { "" } else { "," },
            hex(&sha256.0),
        );
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("{{\"schema_version\":1,\"resources\":[");
    print_resources();
    println!("],\"cases\":[");
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
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}
