// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! Reads a `tilewright.view/1` world-view file (see `docs/WORLD-VIEW.md`)
//! into a [`WorldView`] and a resource provider holding its textures. Every
//! texture is an explicit PNG path whose SHA-256 must match before use.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, io::Read as _, path::Path, sync::Arc};
use tilewright_render_resources::{
    DEFAULT_MAXIMUM_RESOURCE_BYTES, DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES,
};
use tilewright_render_testkit::FakeResourceProvider;
use tilewright_render_world::{
    Anchor, Animation, Camera, CornerRule, GlowEffect, HeightField, MAXIMUM_ANIMATION_FRAMES,
    Projection, Stretch, TextureRef, WorldObject, WorldView,
};
use tilewright_scene::{Digest256, ResourceId, Sampling, SemanticId};

/// Largest accepted view file.
const MAXIMUM_VIEW_BYTES: u64 = 64 * 1024 * 1024;
/// Largest accepted texture file.
const MAXIMUM_TEXTURE_BYTES: u64 = 16 * 1024 * 1024;
/// Most bytes all of a view's texture files may hold together.
const MAXIMUM_TEXTURE_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// Most JSON values (containers, object members and array items) a view may
/// hold. It bounds the parsed document's memory before the document is built.
const MAXIMUM_JSON_VALUES: usize = 2_097_152;
/// Longest texture path a view may write.
const MAXIMUM_TEXTURE_PATH_BYTES: usize = 4_096;
/// Most textures one view may list, matching the renderer's texture cache.
const MAXIMUM_TEXTURES: usize = 4_096;
/// Most objects one view may list, matching the default scene's draw budget.
const MAXIMUM_OBJECTS: usize = 131_072;
/// Most frame references all of a view's animations may list together:
/// twice the default draw budget, as the compiler allows.
const MAXIMUM_FRAME_REFERENCES: usize = 2 * MAXIMUM_OBJECTS;

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

/// An integer of type `T`, read across the full signed and unsigned 64-bit
/// range so `u64` identities above `i64::MAX` are accepted.
fn bounded<T: TryFrom<i64> + TryFrom<u64>>(value: &Value, context: &str) -> Result<T> {
    let out_of_range = || format!("{context}: out of range");
    match value.as_u64() {
        Some(unsigned) => T::try_from(unsigned).map_err(|_| out_of_range()),
        None => T::try_from(integer(value, context)?).map_err(|_| out_of_range()),
    }
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

/// Reads a regular file of at most `maximum` bytes. The file is opened once,
/// without blocking on FIFOs or devices; its type and size are checked on
/// that handle and the read stops after `maximum + 1` bytes, so the limit
/// holds even if the file is replaced or grows while it is read.
#[cfg(any(test, not(unix)))]
pub(crate) fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let file = open_nonblocking(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    read_handle(file, maximum, &path.display().to_string())
}

fn read_handle(file: fs::File, maximum: u64, name: &str) -> Result<Vec<u8>> {
    let failure = |error: std::io::Error| format!("cannot read {name}: {error}");
    let metadata = file.metadata().map_err(failure)?;
    let rejected = || format!("{name} is not a regular file within {maximum} bytes");
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(rejected());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(failure)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum {
        return Err(rejected());
    }
    Ok(bytes)
}

/// Opens a path for reading without waiting for a FIFO writer.
#[cfg(all(test, unix))]
fn open_nonblocking(path: &Path) -> std::io::Result<fs::File> {
    use rustix::fs::{Mode, OFlags};
    let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC;
    Ok(fs::File::from(rustix::fs::open(
        path,
        flags,
        Mode::empty(),
    )?))
}

#[cfg(not(unix))]
fn open_nonblocking(path: &Path) -> std::io::Result<fs::File> {
    fs::File::open(path)
}

/// An upper bound on the values in a JSON document, found without parsing
/// it: outside strings, every value but the first follows a `,` or opens a
/// container. Malformed documents are left for the parser to reject.
pub(crate) fn json_values(bytes: &[u8]) -> usize {
    let mut count = 1_usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in bytes {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b',' | b'[' | b'{' => count += 1,
                _ => {}
            }
        }
    }
    count
}

/// A JSON document parsed as a [`Value`], except that an object repeating a
/// key is an error instead of silently keeping the last occurrence.
struct Strict(Value);

impl<'de> serde_core::Deserialize<'de> for Strict {
    fn deserialize<D: serde_core::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor).map(Self)
    }
}

struct StrictVisitor;

impl<'de> serde_core::de::Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E: serde_core::de::Error>(self, value: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: serde_core::de::Error>(self, value: i64) -> std::result::Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_u64<E: serde_core::de::Error>(self, value: u64) -> std::result::Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_f64<E: serde_core::de::Error>(self, value: f64) -> std::result::Result<Value, E> {
        Ok(serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number))
    }

    fn visit_str<E: serde_core::de::Error>(self, value: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: serde_core::de::Error>(
        self,
        value: String,
    ) -> std::result::Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_unit<E: serde_core::de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: serde_core::de::SeqAccess<'de>>(
        self,
        mut sequence: A,
    ) -> std::result::Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(Strict(item)) = sequence.next_element()? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: serde_core::de::MapAccess<'de>>(
        self,
        mut entries: A,
    ) -> std::result::Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = entries.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(serde_core::de::Error::custom(format_args!(
                    "duplicate key \"{key}\""
                )));
            }
            let Strict(value) = entries.next_value()?;
            object.insert(key, value);
        }
        Ok(Value::Object(object))
    }
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut output, byte| {
        let _ = write!(output, "{byte:02x}");
        output
    })
}

/// A PNG decoder for view textures. Text and ICC metadata are unused, so
/// they are skipped: no compressed metadata is ever inflated or stored.
pub(crate) fn png_decoder(bytes: &[u8]) -> png::Decoder<std::io::Cursor<&[u8]>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    decoder
}

/// Decodes a PNG into an `ATX1` texture envelope of at most `maximum_bytes`.
/// The size is checked from the header before any pixel buffer is allocated.
///
/// # Errors
/// Returns a message for malformed or oversized images.
pub fn png_to_atx1(bytes: &[u8], maximum_bytes: usize) -> Result<(u32, u32, Vec<u8>)> {
    let mut reader = png_decoder(bytes)
        .read_info()
        .map_err(|error| format!("invalid PNG: {error}"))?;
    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };
    if width == 0 || height == 0 || width > 8_192 || height > 8_192 {
        return Err("PNG dimensions out of range".to_owned());
    }
    let envelope = (u64::from(width) * u64::from(height))
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_add(12))
        .ok_or("PNG too large")?;
    if envelope > u64::try_from(maximum_bytes).unwrap_or(u64::MAX) {
        return Err(format!(
            "a {width}x{height} texture decodes to {envelope} bytes, over the {maximum_bytes}-byte limit"
        ));
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

/// Splits a texture path written in a view into its components: relative,
/// `/`-separated, with no empty, `.` or `..` components.
fn components(relative: &str) -> Result<Vec<&str>> {
    // The length is bounded before the path is split, so a huge path cannot
    // allocate an entry per component.
    if relative.len() > MAXIMUM_TEXTURE_PATH_BYTES {
        return Err(format!(
            "texture paths are at most {MAXIMUM_TEXTURE_PATH_BYTES} bytes"
        ));
    }
    let components: Vec<&str> = relative.split('/').collect();
    if relative.is_empty()
        || relative.contains('\\')
        || relative.contains(':')
        || components
            .iter()
            .any(|component| component.is_empty() || *component == "." || *component == "..")
    {
        return Err(format!(
            "texture path \"{relative}\" must be relative to the view, without . or .. components"
        ));
    }
    Ok(components)
}

/// The directory holding a view, opened once. The view file and every
/// texture are resolved relative to this one handle, so replacing the
/// directory or any of its ancestors while the view loads cannot redirect
/// them elsewhere.
#[cfg(unix)]
struct ViewDirectory(std::os::fd::OwnedFd);

/// The directory holding a view, resolved once.
#[cfg(not(unix))]
struct ViewDirectory(std::path::PathBuf);

#[cfg(unix)]
impl ViewDirectory {
    fn open(parent: &Path) -> Result<Self> {
        use rustix::fs::{Mode, OFlags, open};
        open(
            parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(Self)
        .map_err(|error| {
            format!(
                "cannot open {}: {}",
                parent.display(),
                std::io::Error::from(error)
            )
        })
    }

    /// Reads the view file `name` in this directory without blocking or
    /// following a symbolic link.
    fn read_view(&self, path: &Path, name: &std::ffi::OsStr) -> Result<Vec<u8>> {
        use rustix::{
            fs::{Mode, OFlags, openat},
            io::Errno,
        };
        let file = openat(
            &self.0,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            // `O_NOFOLLOW` reports a symbolic link as ELOOP (EMLINK on BSDs).
            if error == Errno::LOOP || error == Errno::MLINK {
                format!("{} may not be a symbolic link", path.display())
            } else {
                format!(
                    "cannot read {}: {}",
                    path.display(),
                    std::io::Error::from(error)
                )
            }
        })?;
        read_handle(
            fs::File::from(file),
            MAXIMUM_VIEW_BYTES,
            &path.display().to_string(),
        )
    }

    /// Opens a texture beneath this directory: each component relative to
    /// the previous directory handle without following symbolic links, and
    /// the file without blocking, so the path cannot leave the directory or
    /// stall even if the directory changes concurrently.
    fn open_texture(&self, relative: &str) -> Result<fs::File> {
        use rustix::{
            fs::{Mode, OFlags, openat},
            io::Errno,
        };
        let components = components(relative)?;
        let failure = |error: Errno| {
            // `O_NOFOLLOW` reports a symbolic link as ELOOP (EMLINK on BSDs),
            // and as ENOTDIR together with `O_DIRECTORY`.
            if error == Errno::LOOP || error == Errno::MLINK || error == Errno::NOTDIR {
                format!(
                    "texture path \"{relative}\" may not pass through a symbolic link or a non-directory"
                )
            } else {
                format!(
                    "cannot open \"{relative}\": {}",
                    std::io::Error::from(error)
                )
            }
        };
        let (file, parents) = components
            .split_last()
            .ok_or_else(|| failure(Errno::INVAL))?;
        let mut directory = None;
        for parent in parents {
            directory = Some(
                openat(
                    directory.as_ref().unwrap_or(&self.0),
                    *parent,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(failure)?,
            );
        }
        let file = openat(
            directory.as_ref().unwrap_or(&self.0),
            *file,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(failure)?;
        Ok(fs::File::from(file))
    }
}

#[cfg(not(unix))]
impl ViewDirectory {
    fn open(parent: &Path) -> Result<Self> {
        fs::canonicalize(parent)
            .map(Self)
            .map_err(|error| format!("cannot resolve {}: {error}", parent.display()))
    }

    fn read_view(&self, path: &Path, _name: &std::ffi::OsStr) -> Result<Vec<u8>> {
        read_bounded(path, MAXIMUM_VIEW_BYTES)
    }

    /// Opens a texture that, once symbolic links are resolved, stays inside
    /// the view's directory.
    fn open_texture(&self, relative: &str) -> Result<fs::File> {
        let joined = components(relative)?
            .iter()
            .fold(self.0.clone(), |path, component| path.join(component));
        let resolved = fs::canonicalize(&joined)
            .map_err(|error| format!("cannot resolve {}: {error}", joined.display()))?;
        if !resolved.starts_with(&self.0) {
            return Err(format!(
                "texture path \"{relative}\" leaves the view's directory"
            ));
        }
        fs::File::open(&resolved).map_err(|error| format!("cannot open \"{relative}\": {error}"))
    }
}

/// Reads a view and its textures with the renderer's default budgets.
pub fn load(path: &Path) -> Result<LoadedView> {
    load_within(
        path,
        usize::try_from(DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES).unwrap_or(usize::MAX),
        MAXIMUM_TEXTURE_FILE_BYTES,
    )
}

/// Reads a view whose decoded textures together fit in `texture_budget`
/// bytes and whose texture files together hold at most `file_budget` bytes.
#[allow(clippy::too_many_lines)]
pub(crate) fn load_within(
    path: &Path,
    texture_budget: usize,
    file_budget: u64,
) -> Result<LoadedView> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} does not name a view file", path.display()))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let directory = ViewDirectory::open(parent)?;
    let bytes = directory.read_view(path, name)?;
    if json_values(&bytes) > MAXIMUM_JSON_VALUES {
        return Err(format!(
            "a view holds at most {MAXIMUM_JSON_VALUES} JSON values"
        ));
    }
    let Strict(document) =
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
    // Each projection takes only its own tile keys, so a stale key from the
    // other one fails instead of being silently ignored.
    let foreign: &[&str] = match field(camera_object, "projection", "camera")?.as_str() {
        Some("isometric") => &["tile_size"],
        Some("top_down") => &["tile_width", "tile_height"],
        _ => &[],
    };
    if let Some(key) = foreign.iter().find(|key| camera_object.contains_key(**key)) {
        return Err(format!(
            "camera: \"{key}\" does not apply to this projection"
        ));
    }
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
    let entries = field(root, "textures", "view")?
        .as_array()
        .ok_or("textures must be an array")?;
    if entries.len() > MAXIMUM_TEXTURES {
        return Err(format!("a view lists at most {MAXIMUM_TEXTURES} textures"));
    }
    // Decoded textures share the renderer's working-set budget.
    let mut remaining = texture_budget;
    let mut encoded_remaining = file_budget;
    for (index, entry) in entries.iter().enumerate() {
        let context = format!("textures[{index}]");
        let object = as_object(entry, &context)?;
        reject_unknown(object, &["id", "path", "sha256"], &context)?;
        let id: u64 = bounded(field(object, "id", &context)?, &context)?;
        let relative = field(object, "path", &context)?
            .as_str()
            .ok_or_else(|| format!("{context}: path must be a string"))?;
        // Checked before anything is opened, and compared without copying.
        let expected = field(object, "sha256", &context)?
            .as_str()
            .filter(|digest| {
                digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or_else(|| format!("{context}: sha256 must be 64 hexadecimal digits"))?;
        let handle = directory
            .open_texture(relative)
            .map_err(|error| format!("{context}: {error}"))?;
        // Texture files share one encoded-byte budget, so repeated or padded
        // files cannot make the loader read and hash without bound.
        let file = read_handle(
            handle,
            MAXIMUM_TEXTURE_BYTES.min(encoded_remaining),
            &format!("{context} ({relative})"),
        )
        .map_err(|error| {
            format!("{error}; texture files together may hold at most {file_budget} bytes")
        })?;
        encoded_remaining -= u64::try_from(file.len()).unwrap_or(u64::MAX);
        if !hex(&Sha256::digest(&file)).eq_ignore_ascii_case(expected) {
            return Err(format!("{context}: sha256 does not match {relative}"));
        }
        let (width, height, atx1) =
            png_to_atx1(&file, DEFAULT_MAXIMUM_RESOURCE_BYTES.min(remaining)).map_err(|error| {
                format!("{context}: {error} (all textures together may decode to at most {texture_budget} bytes)")
            })?;
        remaining -= atx1.len();
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

    let entries = field(root, "objects", "view")?
        .as_array()
        .ok_or("objects must be an array")?;
    // Counts are bounded before anything is collected from them.
    if entries.len() > MAXIMUM_OBJECTS {
        return Err(format!("a view lists at most {MAXIMUM_OBJECTS} objects"));
    }
    let mut frame_references = 0_usize;
    let mut objects = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
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
        if optional(object, "animation").is_some() && optional(object, "texture").is_some() {
            return Err(format!(
                "{context}: give either \"texture\" or \"animation\", not both"
            ));
        }
        let animation = if let Some(value) = optional(object, "animation") {
            let animation_object = as_object(value, &context)?;
            reject_unknown(
                animation_object,
                &["frames", "frame_millis", "phase_millis"],
                &context,
            )?;
            let frames = field(animation_object, "frames", &context)?
                .as_array()
                .ok_or_else(|| format!("{context}: frames must be an array"))?;
            frame_references = frame_references.saturating_add(frames.len());
            if frames.len() > MAXIMUM_ANIMATION_FRAMES {
                return Err(format!(
                    "{context}: an animation lists at most {MAXIMUM_ANIMATION_FRAMES} frames"
                ));
            }
            if frame_references > MAXIMUM_FRAME_REFERENCES {
                return Err(format!(
                    "{context}: a view's animations list at most {MAXIMUM_FRAME_REFERENCES} frames together"
                ));
            }
            Animation {
                frames: frames
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

#[cfg(all(test, unix))]
mod tests {
    use super::ViewDirectory;
    use std::{fs, io::Read as _};

    #[test]
    fn view_files_are_not_followed_through_links() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("real.json"), b"{}").unwrap();
        std::os::unix::fs::symlink("real.json", directory.path().join("view.json")).unwrap();
        let error = super::load(&directory.path().join("view.json"))
            .err()
            .unwrap_or_default();
        assert!(error.contains("may not be a symbolic link"), "{error}");
    }

    #[test]
    fn textures_resolve_beneath_the_directory_opened_first() {
        let outer = tempfile::tempdir().unwrap();
        let view = outer.path().join("view");
        let other = outer.path().join("other");
        fs::create_dir(&view).unwrap();
        fs::create_dir(&other).unwrap();
        fs::write(view.join("art.png"), b"inside").unwrap();
        fs::write(other.join("art.png"), b"outside").unwrap();
        let directory = ViewDirectory::open(&view).unwrap();
        // Swap the view's directory for a link elsewhere after it is opened.
        fs::rename(&view, outer.path().join("moved")).unwrap();
        std::os::unix::fs::symlink(&other, &view).unwrap();
        let mut bytes = Vec::new();
        directory
            .open_texture("art.png")
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"inside");
    }
}
