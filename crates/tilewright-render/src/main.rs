// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

use std::{env, io::Write, path::Path, process::ExitCode};
use tilewright_render_api::{
    BackendPreference, RenderRequest, Renderer, TargetDescriptor, TargetKind,
};
use tilewright_render_testkit::{
    dense_scene, structural_scene, synthetic_provider, synthetic_scene,
};
use tilewright_render_wgpu::WgpuRenderer;

#[cfg(unix)]
use std::fs::File;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tilewright-render: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("--version") => {
            println!("tilewright-render {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("probe") if arguments.next().is_none() => {
            let renderer = WgpuRenderer::new(target(16, 16))
                .map_err(|error| format!("GPU probe failed: {error}"))?;
            let adapter = renderer.adapter();
            println!(
                "adapter={} driver={} backend={} type={} max_texture_2d={} max_buffer={} startup_micros={}",
                adapter.name,
                adapter.driver,
                adapter.backend,
                adapter.device_type,
                adapter.maximum_texture_dimension_2d,
                adapter.maximum_buffer_size,
                adapter.startup_micros,
            );
            Ok(())
        }
        Some("offscreen") => {
            let output = arguments
                .next()
                .ok_or("usage: tilewright-render offscreen OUTPUT.rgba")?;
            if arguments.next().is_some() {
                return Err("usage: tilewright-render offscreen OUTPUT.rgba".to_owned());
            }
            render_offscreen(Path::new(&output))
        }
        Some("corpus") if arguments.next().is_none() => render_corpus(),
        Some("window") if arguments.next().is_none() => render_window(),
        _ => Err(
            "usage: tilewright-render {--version|probe|corpus|window|offscreen OUTPUT.rgba}"
                .to_owned(),
        ),
    }
}

#[cfg(feature = "sdl3")]
fn render_window() -> Result<(), String> {
    let scene = synthetic_scene(64, 64).map_err(|error| error.to_string())?;
    let mut renderer = WgpuRenderer::new(TargetDescriptor {
        kind: TargetKind::Window,
        width: 64,
        height: 64,
        backend: BackendPreference::Automatic,
    })
    .map_err(|error| error.to_string())?;
    let proof = tilewright_render_sdl3::present_window(
        &mut renderer,
        RenderRequest {
            scene: &scene,
            resources: synthetic_provider().map_err(|error| error.to_string())?,
        },
    )
    .map_err(|error| error.to_string())?;
    println!(
        "window={}x{} adapter={} backend={} format={} reconfigured={} frames={} vertices={} uploads={}",
        proof.width,
        proof.height,
        proof.adapter_name,
        proof.backend,
        proof.format,
        proof.reconfigured,
        proof.metrics.frames_submitted,
        proof.metrics.vertex_count,
        proof.metrics.uploaded_bytes,
    );
    Ok(())
}

#[cfg(not(feature = "sdl3"))]
fn render_window() -> Result<(), String> {
    Err("window support is not compiled; rebuild with --features sdl3".to_owned())
}

fn render_corpus() -> Result<(), String> {
    let cases = [
        ("overlap-square", synthetic_scene(16, 16)),
        ("unaligned-wide", synthetic_scene(31, 17)),
        ("structural-fog-ui", structural_scene(48, 48)),
        ("high-density-1024", dense_scene(128, 128)),
    ];
    for (name, scene) in cases {
        let scene = scene.map_err(|error| error.to_string())?;
        let viewport = scene.viewport();
        let mut renderer = WgpuRenderer::new(target(viewport.width, viewport.height))
            .map_err(|error| error.to_string())?;
        let frame = renderer
            .render(RenderRequest {
                scene: &scene,
                resources: synthetic_provider().map_err(|error| error.to_string())?,
            })
            .map_err(|error| error.to_string())?;
        println!(
            "scenario={name} sprites={} vertices={} uploads={} readback_bytes={} frame_cpu_micros={}",
            scene.sprites().len(),
            frame.metrics.vertex_count,
            frame.metrics.uploaded_bytes,
            frame.metrics.readback_bytes,
            frame.metrics.frame_cpu_micros,
        );
    }
    Ok(())
}

fn render_offscreen(path: &Path) -> Result<(), String> {
    let scene = synthetic_scene(64, 64).map_err(|error| error.to_string())?;
    let mut renderer = WgpuRenderer::new(target(64, 64)).map_err(|error| error.to_string())?;
    let frame = renderer
        .render(RenderRequest {
            scene: &scene,
            resources: synthetic_provider().map_err(|error| error.to_string())?,
        })
        .map_err(|error| error.to_string())?;
    persist_new(path, &[b"TILEWRIGHT-RGBA8\n64 64\n", &frame.rgba8])
}

fn persist_new(path: &Path, chunks: &[&[u8]]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
        format!(
            "cannot create temporary output in {}: {error}",
            parent.display()
        )
    })?;
    for chunk in chunks {
        temporary
            .write_all(chunk)
            .map_err(|error| format!("cannot write temporary output: {error}"))?;
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("cannot sync temporary output: {error}"))?;
    let output = temporary.persist_noclobber(path).map_err(|error| {
        format!(
            "cannot create {} without overwriting: {error}",
            path.display()
        )
    })?;
    output
        .sync_all()
        .map_err(|error| format!("cannot sync {}: {error}", path.display()))?;
    sync_parent(parent)?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> Result<(), String> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("cannot sync output directory {}: {error}", parent.display()))
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> Result<(), String> {
    Ok(())
}

const fn target(width: u32, height: u32) -> TargetDescriptor {
    TargetDescriptor {
        kind: TargetKind::Offscreen,
        width,
        height,
        backend: BackendPreference::Automatic,
    }
}

#[cfg(test)]
mod tests {
    use super::persist_new;
    use std::fs;

    #[test]
    fn output_persistence_is_atomic_and_no_clobber() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("frame.rgba");
        persist_new(&output, &[b"header", b"pixels"]).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"headerpixels");
        assert!(persist_new(&output, &[b"replacement"]).is_err());
        assert_eq!(fs::read(&output).unwrap(), b"headerpixels");
        assert!(persist_new(&directory.path().join("missing/frame"), &[b"x"]).is_err());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
