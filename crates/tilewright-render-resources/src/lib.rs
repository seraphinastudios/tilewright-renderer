// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

use sha2::{Digest, Sha256};
use std::fmt;
use tilewright_scene::{Digest256, Glow, Layer, Quad, ResourceId, Sampling};

pub const DEFAULT_MAXIMUM_RESOURCE_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES: u64 = 256 * 1024 * 1024;
/// Most distinct texture identities one scene may name, on every renderer.
pub use tilewright_scene::MAXIMUM_SCENE_TEXTURES;

/// Adds one distinct decoded texture to the default scene working-set budget.
///
/// # Errors
/// Returns [`Error::LimitExceeded`] on arithmetic overflow or above 256 MiB.
pub fn add_working_set_bytes(total: u64, additional: u64) -> Result<u64, Error> {
    let updated = total.checked_add(additional).ok_or(Error::LimitExceeded)?;
    if updated > DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES {
        return Err(Error::LimitExceeded);
    }
    Ok(updated)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceRequest {
    pub id: ResourceId,
    pub revision: u64,
    pub digest: Digest256,
    pub maximum_bytes: usize,
}

impl ResourceRequest {
    /// Checks identity and allocation bounds.
    ///
    /// # Errors
    /// Returns [`Error::InvalidRequest`] for an empty identity or unsafe bound.
    pub fn validate(self) -> Result<Self, Error> {
        if self.id.0 == 0
            || self.revision == 0
            || self.digest.0.iter().all(|byte| *byte == 0)
            || self.maximum_bytes == 0
            || self.maximum_bytes > DEFAULT_MAXIMUM_RESOURCE_BYTES
        {
            return Err(Error::InvalidRequest);
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceBlob {
    bytes: Vec<u8>,
}

impl ResourceBlob {
    /// Verifies and constructs a blob for the exact request identity.
    ///
    /// # Errors
    /// Returns [`Error::LimitExceeded`] when the byte count is zero or too large.
    pub fn for_request(request: ResourceRequest, bytes: Vec<u8>) -> Result<Self, Error> {
        let request = request.validate()?;
        if bytes.is_empty() || bytes.len() > request.maximum_bytes {
            return Err(Error::LimitExceeded);
        }
        if Sha256::digest(&bytes).as_slice() != request.digest.0 {
            return Err(Error::DigestMismatch);
        }
        Ok(Self { bytes })
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rgba8Image {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Rgba8Image {
    /// Decodes the bounded `ATX1` texture envelope.
    ///
    /// The encoding is `ATX1`, little-endian `u32` width and height, followed
    /// by tightly packed row-major RGBA8 pixels.
    ///
    /// # Errors
    /// Returns a format or limit error for malformed dimensions or byte counts.
    pub fn decode(
        blob: &ResourceBlob,
        maximum_dimension: u32,
        maximum_pixels: u64,
    ) -> Result<Self, Error> {
        let bytes = blob.bytes();
        if bytes.len() < 12 || &bytes[..4] != b"ATX1" {
            return Err(Error::InvalidFormat);
        }
        let width = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let height = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let pixels = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or(Error::LimitExceeded)?;
        let byte_count = pixels.checked_mul(4).ok_or(Error::LimitExceeded)?;
        if width == 0
            || height == 0
            || width > maximum_dimension
            || height > maximum_dimension
            || pixels > maximum_pixels
            || usize::try_from(byte_count)
                .ok()
                .and_then(|count| count.checked_add(12))
                != Some(bytes.len())
        {
            return Err(Error::InvalidFormat);
        }
        Ok(Self {
            width,
            height,
            pixels: bytes[12..].to_vec(),
        })
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PixelBounds {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

/// Fixed-point subpixel resolution used for general quads: positions snap
/// to 1/256 px so every renderer path makes identical coverage decisions.
pub const SUBPIXEL_STEPS: i64 = 256;
const SUBPIXEL_SCALE: f64 = 256.0;

/// The quad with every corner on the shared subpixel grid. Every renderer
/// path rasterises this geometry.
fn snapped(quad: &Quad) -> Quad {
    Quad {
        positions: quad.positions.map(|point| point.map(subpixel_position)),
        ..*quad
    }
}

/// Pixel bounds of a quad on the shared subpixel grid, clipped to the target.
#[must_use]
pub fn quad_pixel_bounds(quad: &Quad, width: u32, height: u32) -> PixelBounds {
    snapped_bounds(&snapped(quad), width, height)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn snapped_bounds(quad: &Quad, width: u32, height: u32) -> PixelBounds {
    let (min_x, max_x, min_y, max_y) = quad.positions.iter().fold(
        (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
        |(min_x, max_x, min_y, max_y), point| {
            (
                min_x.min(point[0]),
                max_x.max(point[0]),
                min_y.min(point[1]),
                max_y.max(point[1]),
            )
        },
    );
    PixelBounds {
        left: ((min_x - 0.5).ceil().max(0.0) as u32).min(width),
        top: ((min_y - 0.5).ceil().max(0.0) as u32).min(height),
        right: ((max_x - 0.5).ceil().max(0.0) as u32).min(width),
        bottom: ((max_y - 0.5).ceil().max(0.0) as u32).min(height),
    }
}

/// Visits each covered pixel centre of a quad with its interpolated texture
/// coordinates. Corners are first snapped to [`SUBPIXEL_STEPS`], exactly as
/// GPU backends submit them. Axis-aligned quads then use half-open rectangle
/// coverage. Other quads are split into the triangles (0, 1, 2) and
/// (0, 2, 3) and filled with an exact integer edge test. Pixel centres on an
/// edge follow the GPU's top-left rule: they belong to top and left edges
/// only, so every pixel on a shared edge goes to exactly one triangle and
/// matches what the hardware colours. The reference renderer and the GPU
/// picking planes both use this function.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn for_each_quad_pixel(
    quad: &Quad,
    width: u32,
    height: u32,
    mut visit: impl FnMut(u32, u32, usize, f32, f32),
) {
    let quad = &snapped(quad);
    if let Some(rect) = quad.aligned_rect() {
        let rect_width = rect.right - rect.left;
        let rect_height = rect.bottom - rect.top;
        let bounds = snapped_bounds(quad, width, height);
        for y in bounds.top..bounds.bottom {
            for x in bounds.left..bounds.right {
                let index = y as usize * width as usize + x as usize;
                let horizontal = (x as f32 + 0.5 - rect.left) / rect_width;
                let vertical = (y as f32 + 0.5 - rect.top) / rect_height;
                let u = rect.uv_min[0] + horizontal * (rect.uv_max[0] - rect.uv_min[0]);
                let v = rect.uv_min[1] + vertical * (rect.uv_max[1] - rect.uv_min[1]);
                visit(x, y, index, u, v);
            }
        }
        return;
    }
    for triangle in [[0_usize, 1, 2], [0, 2, 3]] {
        let mut points = triangle.map(|corner| {
            let [x, y] = quad.positions[corner];
            (snap(x), snap(y))
        });
        let mut uvs = triangle.map(|corner| quad.uvs[corner]);
        let mut area = edge(points[0], points[1], points[2]);
        if area == 0 {
            continue;
        }
        if area < 0 {
            points.swap(1, 2);
            uvs.swap(1, 2);
            area = -area;
        }
        let min_x = points.iter().map(|point| point.0).min().unwrap_or(0);
        let max_x = points.iter().map(|point| point.0).max().unwrap_or(0);
        let min_y = points.iter().map(|point| point.1).min().unwrap_or(0);
        let max_y = points.iter().map(|point| point.1).max().unwrap_or(0);
        // Only pixels whose centres fall inside the triangle's box can be
        // covered; a triangle wholly outside the target scans nothing.
        let (Some((first_x, last_x)), Some((first_y, last_y))) = (
            centre_range(min_x, max_x, width),
            centre_range(min_y, max_y, height),
        ) else {
            continue;
        };
        let half = SUBPIXEL_STEPS / 2;
        let area_f = area as f64;
        for y in first_y..=last_y {
            let center_y = y * SUBPIXEL_STEPS + half;
            for x in first_x..=last_x {
                let center = (x * SUBPIXEL_STEPS + half, center_y);
                let w0 = edge(points[1], points[2], center);
                let w1 = edge(points[2], points[0], center);
                let w2 = edge(points[0], points[1], center);
                if covers(w0, points[1], points[2])
                    && covers(w1, points[2], points[0])
                    && covers(w2, points[0], points[1])
                {
                    let (w0, w1, w2) = (w0 as f64, w1 as f64, w2 as f64);
                    let interpolate = |channel: usize| {
                        ((w0 * f64::from(uvs[0][channel])
                            + w1 * f64::from(uvs[1][channel])
                            + w2 * f64::from(uvs[2][channel]))
                            / area_f) as f32
                    };
                    let index = y as usize * width as usize + x as usize;
                    visit(x as u32, y as u32, index, interpolate(0), interpolate(1));
                }
            }
        }
    }
}

/// The pixels, within a target `size` pixels long, whose centres lie in the
/// fixed-point interval `[low, high]`.
fn centre_range(low: i64, high: i64, size: u32) -> Option<(i64, i64)> {
    let half = SUBPIXEL_STEPS / 2;
    // The first centre at or after `low` and the last at or before `high`.
    let first = (low - half + SUBPIXEL_STEPS - 1)
        .div_euclid(SUBPIXEL_STEPS)
        .max(0);
    let last = (high - half)
        .div_euclid(SUBPIXEL_STEPS)
        .min(i64::from(size) - 1);
    (first <= last).then_some((first, last))
}

#[allow(clippy::cast_possible_truncation)]
fn snap(value: f32) -> i64 {
    (f64::from(value) * SUBPIXEL_SCALE).round() as i64
}

/// A position rounded to the shared subpixel grid. GPU backends submit these
/// so the hardware's own snapping starts from the exact fixed-point corners
/// the reference rasteriser uses, whatever its rounding mode.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub fn subpixel_position(value: f32) -> f32 {
    (snap(value) as f64 / SUBPIXEL_SCALE) as f32
}

/// The edge function in `i128`: fixed-point coordinate differences can
/// exceed 2³² under permissive scene limits, so their products need more
/// than 64 bits.
fn edge(a: (i64, i64), b: (i64, i64), point: (i64, i64)) -> i128 {
    let difference = |to: i64, from: i64| i128::from(to) - i128::from(from);
    difference(b.0, a.0) * difference(point.1, a.1)
        - difference(b.1, a.1) * difference(point.0, a.0)
}

/// Whether a pixel centre with this edge weight is inside. Triangles are
/// wound so inside weights are positive in y-down screen space; there a top
/// edge runs rightward and a left edge runs upward, and only those edges keep
/// the centres lying exactly on them.
const fn covers(weight: i128, a: (i64, i64), b: (i64, i64)) -> bool {
    weight > 0 || (weight == 0 && (b.1 < a.1 || (b.1 == a.1 && b.0 > a.0)))
}

#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn sample_rgba8(image: &Rgba8Image, u: f32, v: f32, sampling: Sampling) -> [u8; 4] {
    match sampling {
        Sampling::Nearest => {
            let x = (u.clamp(0.0, 1.0 - f32::EPSILON) * image.width() as f32).floor() as u32;
            let y = (v.clamp(0.0, 1.0 - f32::EPSILON) * image.height() as f32).floor() as u32;
            texel(image, x, y)
        }
        Sampling::Linear => {
            let x = u.mul_add(image.width() as f32, -0.5);
            let y = v.mul_add(image.height() as f32, -0.5);
            let x0 = x.floor().max(0.0) as u32;
            let y0 = y.floor().max(0.0) as u32;
            let x1 = x0.saturating_add(1).min(image.width() - 1);
            let y1 = y0.saturating_add(1).min(image.height() - 1);
            let fx = x.fract().max(0.0);
            let fy = y.fract().max(0.0);
            let top = mix(texel(image, x0, y0), texel(image, x1, y0), fx);
            let bottom = mix(texel(image, x0, y1), texel(image, x1, y1), fx);
            mix(top, bottom, fy)
        }
    }
}

#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn shade_rgba8(texel: [u8; 4], color: [f32; 4], layer: Layer, clock_millis: u64) -> [u8; 4] {
    let mut output =
        std::array::from_fn(|channel| (f32::from(texel[channel]) * color[channel]).round() as u8);
    if layer == Layer::Effect {
        let amount = (clock_millis % 1_000) as f32 / 1_000.0 * 0.15;
        let original = output;
        for channel in 0..3 {
            output[channel] = f32::from(original[channel])
                .mul_add(1.0 - amount, f32::from(original[2 - channel]) * amount)
                .round() as u8;
        }
    }
    output
}

/// Samples a quad texture; coordinates outside 0–1 are transparent.
#[must_use]
pub fn sample_quad_rgba8(image: &Rgba8Image, u: f32, v: f32, sampling: Sampling) -> [u8; 4] {
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return [0; 4];
    }
    sample_rgba8(image, u, v, sampling)
}

/// Luma weights used for greyscale.
pub const GRAYSCALE_WEIGHTS: [f32; 3] = [0.299, 0.587, 0.114];

/// Applies a quad's colour, greyscale and layer effect to a texel.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn shade_quad_rgba8(texel: [u8; 4], quad: &Quad, clock_millis: u64) -> [u8; 4] {
    if quad.grayscale <= 0.0 {
        return shade_rgba8(texel, quad.color, quad.layer, clock_millis);
    }
    let mut channels: [f32; 4] =
        std::array::from_fn(|channel| f32::from(texel[channel]) * quad.color[channel]);
    let gray = channels[0] * GRAYSCALE_WEIGHTS[0]
        + channels[1] * GRAYSCALE_WEIGHTS[1]
        + channels[2] * GRAYSCALE_WEIGHTS[2];
    for channel in channels.iter_mut().take(3) {
        *channel += (gray - *channel) * quad.grayscale;
    }
    let shaded = channels.map(|channel| channel.round().clamp(0.0, 255.0) as u8);
    shade_rgba8(shaded, [1.0; 4], quad.layer, clock_millis)
}

/// The sixteen unit directions a glow halo samples, at 22.5° steps.
pub const GLOW_DIRECTIONS: [[f32; 2]; 16] = [
    [1.0, 0.0],
    [0.923_879_5, 0.382_683_43],
    [0.707_106_77, 0.707_106_77],
    [0.382_683_43, 0.923_879_5],
    [0.0, 1.0],
    [-0.382_683_43, 0.923_879_5],
    [-0.707_106_77, 0.707_106_77],
    [-0.923_879_5, 0.382_683_43],
    [-1.0, 0.0],
    [-0.923_879_5, -0.382_683_43],
    [-0.707_106_77, -0.707_106_77],
    [-0.382_683_43, -0.923_879_5],
    [0.0, -1.0],
    [0.382_683_43, -0.923_879_5],
    [0.707_106_77, -0.707_106_77],
    [0.923_879_5, -0.382_683_43],
];

/// The halo colour at a texture coordinate: the glow colour with alpha set
/// by the strongest texture alpha at the centre and at full and half radius
/// along [`GLOW_DIRECTIONS`]. A zero radius samples only the centre.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn glow_rgba8(image: &Rgba8Image, u: f32, v: f32, glow: &Glow, sampling: Sampling) -> [u8; 4] {
    let mut alpha = sample_quad_rgba8(image, u, v, sampling)[3];
    // A zero radius would sample the centre 32 more times for the same alpha.
    if glow.radius_uv.iter().any(|radius| *radius > 0.0) {
        for direction in GLOW_DIRECTIONS {
            for scale in [1.0_f32, 0.5] {
                let sample_u = u + direction[0] * glow.radius_uv[0] * scale;
                let sample_v = v + direction[1] * glow.radius_uv[1] * scale;
                alpha = alpha.max(sample_quad_rgba8(image, sample_u, sample_v, sampling)[3]);
            }
        }
    }
    let channel = |value: f32| (value * 255.0).round().clamp(0.0, 255.0) as u8;
    [
        channel(glow.color[0]),
        channel(glow.color[1]),
        channel(glow.color[2]),
        (f32::from(alpha) * glow.color[3]).round().clamp(0.0, 255.0) as u8,
    ]
}

fn texel(image: &Rgba8Image, x: u32, y: u32) -> [u8; 4] {
    let index = (y as usize * image.width() as usize + x as usize) * 4;
    image.pixels()[index..index + 4]
        .try_into()
        .expect("validated RGBA image index")
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn mix(left: [u8; 4], right: [u8; 4], amount: f32) -> [u8; 4] {
    std::array::from_fn(|channel| {
        f32::from(left[channel])
            .mul_add(1.0 - amount, f32::from(right[channel]) * amount)
            .round() as u8
    })
}

/// Supplies immutable, content-addressed bytes from a caller-owned cache.
/// Implementations must not perform filesystem, network, or executor-dependent
/// work; consumers acquire and verify external resources before frame
/// submission.
pub trait ResourceProvider: Send + Sync {
    /// Resolves one bounded identity from the ready cache.
    ///
    /// # Errors
    /// Returns a typed missing, unavailable, identity, or size error.
    fn load(&self, request: ResourceRequest) -> Result<ResourceBlob, Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidRequest,
    NotFound,
    DigestMismatch,
    LimitExceeded,
    InvalidFormat,
    Unavailable,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "renderer resource error: {self:?}")
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES, Error, ResourceBlob, ResourceRequest,
        Rgba8Image, add_working_set_bytes,
    };
    use sha2::{Digest, Sha256};
    use tilewright_scene::{Digest256, Layer, Quad, ResourceId, Sampling, SemanticId};

    fn rect_quad(left: f32, right: f32) -> Quad {
        Quad {
            semantic_id: SemanticId(1),
            part: 0,
            resource_id: ResourceId(1),
            resource_revision: 1,
            resource_digest: Digest256([1; 32]),
            positions: [[left, 0.0], [right, 0.0], [right, 1.0], [left, 1.0]],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            depth: 0,
            painter_order: 0,
            color: [1.0; 4],
            grayscale: 0.0,
            glow: None,
            pickable: true,
            sampling: Sampling::Nearest,
            layer: Layer::World,
        }
    }

    #[test]
    fn aligned_quads_cover_their_snapped_geometry() {
        // 0.5 + 1/1024 rounds to the pixel centre 0.5 on the 1/256 grid, so
        // the snapped left edge covers pixel 0 exactly as the GPU does.
        let mut covered = Vec::new();
        super::for_each_quad_pixel(&rect_quad(0.500_976_56, 2.0), 4, 1, |x, _, _, _, _| {
            covered.push(x);
        });
        assert_eq!(covered, [0, 1]);
        let bounds = super::quad_pixel_bounds(&rect_quad(0.500_976_56, 2.0), 4, 1);
        assert_eq!((bounds.left, bounds.right), (0, 2));
    }

    #[test]
    fn skewed_quads_keep_top_and_left_edge_pixels_only() {
        // Pixel centres lie exactly on the vertical left (x = 0.5) and right
        // (x = 3.5) edges, and on the horizontal top (y = 0.5) and bottom
        // (y = 4.5) edges. The top-left rule keeps column 0 and row 0 and
        // drops column 3 and row 4, as GPUs do.
        let mut vertical = rect_quad(0.0, 1.0);
        vertical.positions = [[0.5, 0.0], [3.5, 1.0], [3.5, 6.0], [0.5, 5.0]];
        let mut horizontal = rect_quad(0.0, 1.0);
        horizontal.positions = [[4.0, 0.5], [7.0, 0.5], [8.0, 4.5], [5.0, 4.5]];
        let mut rows = [[b'.'; 8]; 6];
        for quad in [vertical, horizontal] {
            assert!(quad.aligned_rect().is_none());
            super::for_each_quad_pixel(&quad, 8, 6, |x, y, _, _, _| {
                rows[y as usize][x as usize] = b'#';
            });
        }
        let rows: Vec<&str> = rows
            .iter()
            .map(|row| std::str::from_utf8(row).unwrap())
            .collect();
        assert_eq!(
            rows,
            [
                "##..###.", "###.###.", "###.###.", "###..###", "###.....", "..#.....",
            ]
        );
    }

    #[test]
    fn triangles_scan_only_pixel_centres_inside_their_box() {
        let step = super::SUBPIXEL_STEPS;
        // A box at x ∈ [-1, 0] holds no pixel centre of the target.
        assert_eq!(super::centre_range(-step, 0, 1), None);
        assert_eq!(super::centre_range(-step, 0, 8_192), None);
        // Centres lying exactly on either end are included.
        assert_eq!(
            super::centre_range(step / 2, step * 3 + step / 2, 8),
            Some((0, 3))
        );
        assert_eq!(super::centre_range(step / 2 + 1, step * 3, 8), Some((1, 2)));
        // Ranges clip to the target.
        assert_eq!(super::centre_range(-step * 10, step * 100, 4), Some((0, 3)));
        assert_eq!(super::centre_range(step * 9, step * 10, 4), None);
    }

    #[test]
    fn huge_skewed_quads_rasterise_without_overflow() {
        // Corners billions of pixels apart, as permissive custom limits allow,
        // make 64-bit edge products overflow.
        let mut quad = rect_quad(0.0, 1.0);
        quad.positions = [
            [-4.0e9, -4.0e9],
            [4.0e9, -3.0e9],
            [4.0e9, 4.0e9],
            [-4.0e9, 4.0e9],
        ];
        let mut covered = 0;
        super::for_each_quad_pixel(&quad, 4, 4, |_, _, _, _, _| covered += 1);
        assert_eq!(covered, 16);
    }

    #[test]
    fn rejects_empty_and_oversized_blobs() {
        let bytes = vec![1, 2];
        let request = ResourceRequest {
            id: ResourceId(1),
            revision: 1,
            digest: Digest256(Sha256::digest(&bytes).into()),
            maximum_bytes: 1,
        };
        assert_eq!(
            ResourceBlob::for_request(request, bytes),
            Err(Error::LimitExceeded)
        );
    }

    #[test]
    fn decodes_exact_texture_envelope() {
        let bytes = [
            b"ATX1".as_slice(),
            &1_u32.to_le_bytes(),
            &1_u32.to_le_bytes(),
            &[1, 2, 3, 4],
        ]
        .concat();
        let request = ResourceRequest {
            id: ResourceId(1),
            revision: 1,
            digest: Digest256(Sha256::digest(&bytes).into()),
            maximum_bytes: bytes.len(),
        };
        let blob = ResourceBlob::for_request(request, bytes).unwrap();
        let image = Rgba8Image::decode(&blob, 16, 256).unwrap();
        assert_eq!(image.pixels(), [1, 2, 3, 4]);
    }

    #[test]
    fn rejects_digest_valid_but_corrupt_texture() {
        let bytes = b"not-an-atx1-texture".to_vec();
        let request = ResourceRequest {
            id: ResourceId(1),
            revision: 1,
            digest: Digest256(Sha256::digest(&bytes).into()),
            maximum_bytes: bytes.len(),
        };
        let blob = ResourceBlob::for_request(request, bytes).unwrap();
        assert_eq!(
            Rgba8Image::decode(&blob, 16, 256),
            Err(Error::InvalidFormat)
        );
    }

    #[test]
    fn working_set_budget_is_checked_without_allocation() {
        assert_eq!(
            add_working_set_bytes(DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES - 1, 1),
            Ok(DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES)
        );
        assert_eq!(
            add_working_set_bytes(DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES, 1),
            Err(Error::LimitExceeded)
        );
        assert_eq!(
            add_working_set_bytes(u64::MAX, 1),
            Err(Error::LimitExceeded)
        );
    }
}
