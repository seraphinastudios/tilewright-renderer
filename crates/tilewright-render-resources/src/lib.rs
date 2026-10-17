// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

use sha2::{Digest, Sha256};
use std::fmt;
use tilewright_scene::{Digest256, Layer, ResourceId, Sampling, Sprite};

pub const DEFAULT_MAXIMUM_RESOURCE_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES: u64 = 256 * 1024 * 1024;

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

#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn sprite_pixel_bounds(sprite: &Sprite, width: u32, height: u32) -> PixelBounds {
    PixelBounds {
        left: ((sprite.x - 0.5).ceil().max(0.0) as u32).min(width),
        top: ((sprite.y - 0.5).ceil().max(0.0) as u32).min(height),
        right: ((sprite.x + sprite.width - 0.5).ceil().max(0.0) as u32).min(width),
        bottom: ((sprite.y + sprite.height - 0.5).ceil().max(0.0) as u32).min(height),
    }
}

/// Visits each covered pixel center using the canonical half-open clipping and
/// normalized texture coordinates shared by reference and GPU semantic paths.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn for_each_sprite_pixel(
    sprite: &Sprite,
    width: u32,
    height: u32,
    mut visit: impl FnMut(u32, u32, usize, f32, f32),
) {
    let bounds = sprite_pixel_bounds(sprite, width, height);
    for y in bounds.top..bounds.bottom {
        for x in bounds.left..bounds.right {
            let index = y as usize * width as usize + x as usize;
            let u = (x as f32 + 0.5 - sprite.x) / sprite.width;
            let v = (y as f32 + 0.5 - sprite.y) / sprite.height;
            visit(x, y, index, u, v);
        }
    }
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
    use tilewright_scene::{Digest256, ResourceId};

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
