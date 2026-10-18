// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

mod view_file;

use std::{env, io::Write, path::Path, process::ExitCode};
use tilewright_render_api::{
    BackendPreference, FrameOutput, RenderRequest, Renderer, TargetDescriptor, TargetKind,
};
use tilewright_render_testkit::{
    ReferenceRenderer, dense_scene, iso_world_scene, structural_scene, synthetic_provider,
    synthetic_scene, top_down_world_scene, world_provider,
};
use tilewright_render_wgpu::WgpuRenderer;
use tilewright_scene::SceneLimits;

const USAGE: &str = "usage: tilewright-render {--version|probe|corpus|window|offscreen OUTPUT.rgba|world VIEW.json OUTPUT.png [--reference]}";

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
        Some("world") => {
            let (Some(view), Some(output)) = (arguments.next(), arguments.next()) else {
                return Err(USAGE.to_owned());
            };
            let reference = match arguments.next().as_deref() {
                None => false,
                Some("--reference") if arguments.next().is_none() => true,
                Some(_) => return Err(USAGE.to_owned()),
            };
            render_world(Path::new(&view), Path::new(&output), reference)
        }
        _ => Err(USAGE.to_owned()),
    }
}

/// Renders a `tilewright.view/1` file to a PNG through the same compiler and
/// renderer paths the corpus uses. `--reference` selects the CPU reference.
fn render_world(view: &Path, output: &Path, reference: bool) -> Result<(), String> {
    let loaded = view_file::load(view)?;
    let scene = tilewright_render_world::compile(&loaded.view, SceneLimits::default())
        .map_err(|error| format!("cannot compile view: {error}"))?;
    let viewport = scene.viewport();
    let request = RenderRequest {
        scene: &scene,
        resources: loaded.provider,
    };
    let frame: FrameOutput = if reference {
        ReferenceRenderer::new(target(viewport.width, viewport.height))
            .and_then(|mut renderer| renderer.render(request))
    } else {
        WgpuRenderer::new(target(viewport.width, viewport.height))
            .and_then(|mut renderer| renderer.render(request))
    }
    .map_err(|error| error.to_string())?;
    let png = view_file::encode_png(frame.width, frame.height, &frame.rgba8)?;
    persist_new(output, &[&png])?;
    println!(
        "world={} quads={} size={}x{} renderer={}",
        output.display(),
        scene.quads().len(),
        frame.width,
        frame.height,
        if reference { "reference" } else { "wgpu" },
    );
    Ok(())
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
        ("overlap-square", synthetic_scene(16, 16), false),
        ("unaligned-wide", synthetic_scene(31, 17), false),
        ("structural-fog-ui", structural_scene(48, 48), false),
        ("high-density-1024", dense_scene(128, 128), false),
        ("world-iso-terrain", iso_world_scene(192, 128), true),
        ("world-top-down", top_down_world_scene(96, 64), true),
    ];
    for (name, scene, world) in cases {
        let scene = scene.map_err(|error| error.to_string())?;
        let viewport = scene.viewport();
        let mut renderer = WgpuRenderer::new(target(viewport.width, viewport.height))
            .map_err(|error| error.to_string())?;
        let frame = renderer
            .render(RenderRequest {
                scene: &scene,
                resources: if world {
                    world_provider()
                } else {
                    synthetic_provider()
                }
                .map_err(|error| error.to_string())?,
            })
            .map_err(|error| error.to_string())?;
        println!(
            "scenario={name} quads={} vertices={} uploads={} readback_bytes={} frame_cpu_micros={}",
            scene.quads().len(),
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
    use super::{persist_new, render_world, view_file};
    use sha2::{Digest, Sha256};
    use std::fs;

    /// Writes a 4 × 4 opaque square PNG and a view naming it.
    fn write_view(directory: &std::path::Path, digest: Option<&str>) -> std::path::PathBuf {
        let pixels: Vec<u8> = (0..16).flat_map(|_| [200, 40, 40, 255]).collect();
        let png = view_file::encode_png(4, 4, &pixels).unwrap();
        fs::write(directory.join("square.png"), &png).unwrap();
        let sha = digest.map_or_else(|| view_file::hex(&Sha256::digest(&png)), str::to_owned);
        let view = format!(
            r#"{{
  "format": "tilewright.view/1",
  "camera": {{"projection": "top_down", "tile_size": 4, "center": [1.0, 1.0],
              "width": 8, "height": 8}},
  "clear_color": [0.0, 0.0, 1.0, 1.0],
  "stretch": {{"enabled": false}},
  "textures": [{{"id": 7, "path": "square.png", "sha256": "{sha}"}}],
  "objects": [{{"id": 1, "tile": [0, 0], "rank": 0, "anchor": "ground", "texture": 7}}]
}}"#
        );
        let path = directory.join("view.json");
        fs::write(&path, view).unwrap();
        path
    }

    #[test]
    fn world_views_render_through_the_reference_path() {
        let directory = tempfile::tempdir().unwrap();
        let view = write_view(directory.path(), None);
        let output = directory.path().join("frame.png");
        render_world(&view, &output, true).unwrap();
        let (width, height, atx1) = view_file::png_to_atx1(&fs::read(&output).unwrap()).unwrap();
        assert_eq!((width, height), (8, 8));
        let pixel = |x: usize, y: usize| &atx1[12 + (y * 8 + x) * 4..][..4];
        assert_eq!(pixel(1, 1), [200, 40, 40, 255]);
        assert_eq!(pixel(6, 6), [0, 0, 255, 255]);
        assert!(render_world(&view, &output, true).is_err());
    }

    #[test]
    fn world_views_reject_mismatched_digests_and_unknown_keys() {
        let directory = tempfile::tempdir().unwrap();
        let view = write_view(directory.path(), Some(&"0".repeat(64)));
        let error = view_file::load(&view).err().unwrap();
        assert!(error.contains("sha256 does not match"), "{error}");
        let text = fs::read_to_string(write_view(directory.path(), None)).unwrap();
        fs::write(&view, text.replace("\"clear_color\"", "\"colour\"")).unwrap();
        let error = view_file::load(&view).err().unwrap();
        assert!(error.contains("unknown key \"colour\""), "{error}");
    }

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
