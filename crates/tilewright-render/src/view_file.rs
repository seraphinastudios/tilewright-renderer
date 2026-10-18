// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! Reads a `tilewright.view/1` world-view file (see `docs/WORLD-VIEW.md`)
//! into a [`WorldView`] and a resource provider holding its textures. Every
//! texture is an explicit PNG path whose SHA-256 must match before use.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, path::Path, sync::Arc};
use tilewright_render_testkit::FakeResourceProvider;
use tilewright_render_world::{
    Anchor, Animation, Camera, CornerRule, GlowEffect, HeightField, Projection, Stretch,
    TextureRef, WorldObject, WorldView,
};
use tilewright_scene::{Digest256, ResourceId, Sampling, SemanticId};

/// Largest accepted view file.
const MAXIMUM_VIEW_BYTES: u64 = 64 * 1024 * 1024;
/// Largest accepted texture file.
const MAXIMUM_TEXTURE_BYTES: u64 = 16 * 1024 * 1024;

pub struct LoadedView {
    pub view: WorldView,
    pub provider: Arc<FakeResourceProvider>,
}

type Result<T> = std::result::Result<T, String>;

fn field<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> Result<&'a Value> {
    object
        .get(key)
        .ok_or_else(|| format!("{context}: missing \"{key}\""))
}

fn as_object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| format!("{context}: expected an object"))
}

fn number(value: &Value, context: &str) -> Result<f64> {
    value
        .as_f64()
        .filter(|number| number.is_finite())
        .ok_or_else(|| format!("{context}: expected a number"))
}

#[allow(clippy::cast_possible_truncation)]
fn float(value: &Value, context: &str) -> Result<f32> {
    number(value, context).map(|number| number as f32)
}

fn integer(value: &Value, context: &str) -> Result<i64> {
    value
        .as_i64()
        .ok_or_else(|| format!("{context}: expected an integer"))
}

fn bounded<T: TryFrom<i64>>(value: &Value, context: &str) -> Result<T> {
    T::try_from(integer(value, context)?).map_err(|_| format!("{context}: out of range"))
}

fn optional<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    object.get(key).filter(|value| !value.is_null())
}

fn floats<const N: usize>(value: &Value, context: &str) -> Result<[f32; N]> {
    let items = value
        .as_array()
        .filter(|items| items.len() == N)
        .ok_or_else(|| format!("{context}: expected {N} numbers"))?;
    let mut output = [0.0; N];
    for (slot, item) in output.iter_mut().zip(items) {
        *slot = float(item, context)?;
    }
    Ok(output)
}

fn integers<const N: usize>(value: &Value, context: &str) -> Result<[i64; N]> {
    let items = value
        .as_array()
        .filter(|items| items.len() == N)
        .ok_or_else(|| format!("{context}: expected {N} integers"))?;
    let mut output = [0; N];
    for (slot, item) in output.iter_mut().zip(items) {
        *slot = integer(item, context)?;
    }
    Ok(output)
}

fn reject_unknown(object: &Map<String, Value>, allowed: &[&str], context: &str) -> Result<()> {
    match object.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(format!("{context}: unknown key \"{key}\"")),
        None => Ok(()),
    }
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(format!(
            "{} is not a regular file within {maximum} bytes",
            path.display()
        ));
    }
    fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut output, byte| {
        let _ = write!(output, "{byte:02x}");
        output
    })
}

/// Decodes a PNG into an `ATX1` texture envelope.
///
/// # Errors
/// Returns a message for malformed or oversized images.
pub fn png_to_atx1(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("invalid PNG: {error}"))?;
    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };
    if width == 0 || height == 0 || width > 8_192 || height > 8_192 {
        return Err("PNG dimensions out of range".to_owned());
    }
    let buffer_size = reader
        .output_buffer_size()
        .ok_or("PNG output size overflows")?;
    let mut buffer = vec![0; buffer_size];
    let frame = reader
        .next_frame(&mut buffer)
        .map_err(|error| format!("invalid PNG: {error}"))?;
    let data = &buffer[..frame.buffer_size()];
    let pixels = usize::try_from(u64::from(width) * u64::from(height))
        .map_err(|_| "PNG too large".to_owned())?;
    let mut output = Vec::with_capacity(12 + pixels * 4);
    output.extend_from_slice(b"ATX1");
    output.extend_from_slice(&width.to_le_bytes());
    output.extend_from_slice(&height.to_le_bytes());
    match frame.color_type {
        png::ColorType::Rgba => output.extend_from_slice(data),
        png::ColorType::Rgb => {
            for pixel in data.chunks_exact(3) {
                output.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for pixel in data.chunks_exact(2) {
                output.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        png::ColorType::Grayscale => {
            for value in data {
                output.extend_from_slice(&[*value, *value, *value, 255]);
            }
        }
        png::ColorType::Indexed => return Err("indexed PNG was not expanded".to_owned()),
    }
    if output.len() != 12 + pixels * 4 {
        return Err("PNG pixel data has an unexpected size".to_owned());
    }
    Ok((width, height, output))
}

/// Encodes RGBA8 pixels as a PNG.
///
/// # Errors
/// Returns a message if encoding fails.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| format!("cannot encode PNG: {error}"))?;
        writer
            .write_image_data(rgba)
            .map_err(|error| format!("cannot encode PNG: {error}"))?;
    }
    Ok(output)
}

#[allow(clippy::too_many_lines)]
pub fn load(path: &Path) -> Result<LoadedView> {
    let bytes = read_bounded(path, MAXIMUM_VIEW_BYTES)?;
    let document: Value =
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid view JSON: {error}"))?;
    let root = as_object(&document, "view")?;
    reject_unknown(
        root,
        &[
            "format",
            "revision",
            "clock_millis",
            "camera",
            "clear_color",
            "stretch",
            "ground_layer_ranks",
            "heights",
            "textures",
            "objects",
        ],
        "view",
    )?;
    if field(root, "format", "view")?.as_str() != Some("tilewright.view/1") {
        return Err("view: format must be \"tilewright.view/1\"".to_owned());
    }
    let base = path.parent().unwrap_or(Path::new("."));

    let camera_object = as_object(field(root, "camera", "view")?, "camera")?;
    reject_unknown(
        camera_object,
        &[
            "projection",
            "tile_width",
            "tile_height",
            "tile_size",
            "center",
            "zoom",
            "width",
            "height",
            "sampling",
        ],
        "camera",
    )?;
    let projection = match field(camera_object, "projection", "camera")?.as_str() {
        Some("isometric") => Projection::Isometric {
            tile_width: bounded(field(camera_object, "tile_width", "camera")?, "tile_width")?,
            tile_height: bounded(
                field(camera_object, "tile_height", "camera")?,
                "tile_height",
            )?,
        },
        Some("top_down") => Projection::TopDown {
            tile_size: bounded(field(camera_object, "tile_size", "camera")?, "tile_size")?,
        },
        _ => return Err("camera: projection must be \"isometric\" or \"top_down\"".to_owned()),
    };
    let center = field(camera_object, "center", "camera")?
        .as_array()
        .filter(|items| items.len() == 2)
        .ok_or("camera: center must hold two numbers")?;
    let camera = Camera {
        projection,
        center: [
            number(&center[0], "camera.center")?,
            number(&center[1], "camera.center")?,
        ],
        zoom: optional(camera_object, "zoom")
            .map_or(Ok(1.0), |value| float(value, "camera.zoom"))?,
        width: bounded(field(camera_object, "width", "camera")?, "camera.width")?,
        height: bounded(field(camera_object, "height", "camera")?, "camera.height")?,
        sampling: match optional(camera_object, "sampling").and_then(Value::as_str) {
            None | Some("nearest") => Sampling::Nearest,
            Some("linear") => Sampling::Linear,
            Some(_) => return Err("camera: sampling must be \"nearest\" or \"linear\"".to_owned()),
        },
    };

    let stretch = match optional(root, "stretch") {
        None => Stretch::default(),
        Some(value) => {
            let object = as_object(value, "stretch")?;
            reject_unknown(
                object,
                &["enabled", "rule", "cliff_threshold", "pixels_per_unit"],
                "stretch",
            )?;
            Stretch {
                enabled: optional(object, "enabled")
                    .map_or(Some(true), Value::as_bool)
                    .ok_or("stretch.enabled must be a boolean")?,
                rule: match optional(object, "rule").and_then(Value::as_str) {
                    None | Some("mean") => CornerRule::Mean,
                    Some("max") => CornerRule::Max,
                    Some("min") => CornerRule::Min,
                    Some(_) => return Err("stretch.rule must be mean, max or min".to_owned()),
                },
                cliff_threshold: optional(object, "cliff_threshold")
                    .map(|value| bounded::<i32>(value, "stretch.cliff_threshold"))
                    .transpose()?,
                pixels_per_unit: optional(object, "pixels_per_unit")
                    .map_or(Ok(1.0), |value| float(value, "stretch.pixels_per_unit"))?,
            }
        }
    };

    let heights = match optional(root, "heights") {
        None => HeightField::empty(),
        Some(value) => {
            let object = as_object(value, "heights")?;
            reject_unknown(object, &["origin", "width", "height", "values"], "heights")?;
            let values = field(object, "values", "heights")?
                .as_array()
                .ok_or("heights.values must be an array")?;
            HeightField {
                origin: integers::<2>(field(object, "origin", "heights")?, "heights.origin")?,
                width: bounded(field(object, "width", "heights")?, "heights.width")?,
                height: bounded(field(object, "height", "heights")?, "heights.height")?,
                heights: values
                    .iter()
                    .map(|value| {
                        if value.is_null() {
                            Ok(None)
                        } else {
                            bounded::<i32>(value, "heights.values").map(Some)
                        }
                    })
                    .collect::<Result<Vec<_>>>()?,
            }
        }
    };

    let provider = Arc::new(FakeResourceProvider::default());
    let mut textures: HashMap<u64, TextureRef> = HashMap::new();
    for (index, entry) in field(root, "textures", "view")?
        .as_array()
        .ok_or("textures must be an array")?
        .iter()
        .enumerate()
    {
        let context = format!("textures[{index}]");
        let object = as_object(entry, &context)?;
        reject_unknown(object, &["id", "path", "sha256"], &context)?;
        let id: u64 = bounded(field(object, "id", &context)?, &context)?;
        let relative = field(object, "path", &context)?
            .as_str()
            .ok_or_else(|| format!("{context}: path must be a string"))?;
        let expected = field(object, "sha256", &context)?
            .as_str()
            .ok_or_else(|| format!("{context}: sha256 must be a string"))?;
        let file = read_bounded(&base.join(relative), MAXIMUM_TEXTURE_BYTES)?;
        if hex(&Sha256::digest(&file)) != expected.to_ascii_lowercase() {
            return Err(format!("{context}: sha256 does not match {relative}"));
        }
        let (width, height, atx1) =
            png_to_atx1(&file).map_err(|error| format!("{context}: {error}"))?;
        let digest = Digest256(Sha256::digest(&atx1).into());
        if id == 0 || textures.contains_key(&id) {
            return Err(format!("{context}: id must be unique and nonzero"));
        }
        provider
            .insert(ResourceId(id), 1, atx1)
            .map_err(|error| error.to_string())?;
        textures.insert(
            id,
            TextureRef {
                id: ResourceId(id),
                revision: 1,
                digest,
                width,
                height,
            },
        );
    }
    let texture = |value: &Value, context: &str| -> Result<TextureRef> {
        let id: u64 = bounded(value, context)?;
        textures
            .get(&id)
            .copied()
            .ok_or_else(|| format!("{context}: unknown texture {id}"))
    };

    let mut objects = Vec::new();
    for (index, entry) in field(root, "objects", "view")?
        .as_array()
        .ok_or("objects must be an array")?
        .iter()
        .enumerate()
    {
        let context = format!("objects[{index}]");
        let object = as_object(entry, &context)?;
        reject_unknown(
            object,
            &[
                "id",
                "tile",
                "footprint",
                "rank",
                "sub_layer",
                "anchor",
                "texture",
                "animation",
                "offset",
                "z",
                "scale",
                "rotation",
                "alpha",
                "tint",
                "grayscale",
                "glow",
                "double_draw",
                "pickable",
            ],
            &context,
        )?;
        let animation = if let Some(value) = optional(object, "animation") {
            let animation_object = as_object(value, &context)?;
            reject_unknown(
                animation_object,
                &["frames", "frame_millis", "phase_millis"],
                &context,
            )?;
            Animation {
                frames: field(animation_object, "frames", &context)?
                    .as_array()
                    .ok_or_else(|| format!("{context}: frames must be an array"))?
                    .iter()
                    .map(|frame| texture(frame, &context))
                    .collect::<Result<Vec<_>>>()?,
                frame_millis: optional(animation_object, "frame_millis")
                    .map_or(Ok(0), |value| bounded(value, &context))?,
                phase_millis: optional(animation_object, "phase_millis")
                    .map_or(Ok(0), |value| bounded(value, &context))?,
            }
        } else {
            Animation::still(texture(field(object, "texture", &context)?, &context)?)
        };
        let anchor = match field(object, "anchor", &context)?.as_str() {
            Some("ground") => Anchor::Ground,
            Some("standing") => Anchor::Standing,
            _ => {
                return Err(format!(
                    "{context}: anchor must be \"ground\" or \"standing\""
                ));
            }
        };
        let footprint = optional(object, "footprint")
            .map_or(Ok([1, 1]), |value| integers::<2>(value, &context))?;
        let glow = optional(object, "glow")
            .map(|value| -> Result<GlowEffect> {
                let glow_object = as_object(value, &context)?;
                reject_unknown(glow_object, &["color", "radius", "pulse_millis"], &context)?;
                Ok(GlowEffect {
                    color: floats::<4>(field(glow_object, "color", &context)?, &context)?,
                    radius_px: float(field(glow_object, "radius", &context)?, &context)?,
                    pulse_millis: optional(glow_object, "pulse_millis")
                        .map_or(Ok(0), |value| bounded(value, &context))?,
                })
            })
            .transpose()?;
        objects.push(WorldObject {
            semantic_id: SemanticId(bounded(field(object, "id", &context)?, &context)?),
            tile: integers::<2>(field(object, "tile", &context)?, &context)?,
            footprint: [
                u16::try_from(footprint[0]).map_err(|_| format!("{context}: footprint"))?,
                u16::try_from(footprint[1]).map_err(|_| format!("{context}: footprint"))?,
            ],
            layer_rank: bounded(field(object, "rank", &context)?, &context)?,
            sub_layer: optional(object, "sub_layer")
                .map_or(Ok(0), |value| bounded(value, &context))?,
            anchor,
            animation,
            offset_px: optional(object, "offset")
                .map_or(Ok([0.0, 0.0]), |value| floats::<2>(value, &context))?,
            z: optional(object, "z").map_or(Ok(0), |value| bounded(value, &context))?,
            scale: optional(object, "scale")
                .map_or(Ok([1.0, 1.0]), |value| floats::<2>(value, &context))?,
            rotation_degrees: optional(object, "rotation")
                .map_or(Ok(0.0), |value| float(value, &context))?,
            alpha: optional(object, "alpha").map_or(Ok(1.0), |value| float(value, &context))?,
            tint: optional(object, "tint")
                .map_or(Ok([1.0, 1.0, 1.0]), |value| floats::<3>(value, &context))?,
            grayscale: optional(object, "grayscale")
                .map_or(Ok(0.0), |value| float(value, &context))?,
            glow,
            double_draw: optional(object, "double_draw")
                .map_or(Some(false), Value::as_bool)
                .ok_or_else(|| format!("{context}: double_draw must be a boolean"))?,
            pickable: optional(object, "pickable")
                .map_or(Some(true), Value::as_bool)
                .ok_or_else(|| format!("{context}: pickable must be a boolean"))?,
        });
    }

    Ok(LoadedView {
        view: WorldView {
            revision: optional(root, "revision")
                .map_or(Ok(1), |value| bounded(value, "revision"))?,
            clock_millis: optional(root, "clock_millis")
                .map_or(Ok(0), |value| bounded(value, "clock_millis"))?,
            camera,
            clear_color: optional(root, "clear_color")
                .map_or(Ok([0.0, 0.0, 0.0, 1.0]), |value| {
                    floats::<4>(value, "clear_color")
                })?,
            heights,
            stretch,
            ground_layer_ranks: optional(root, "ground_layer_ranks")
                .map_or(Ok(0), |value| bounded(value, "ground_layer_ranks"))?,
            objects,
        },
        provider,
    })
}
