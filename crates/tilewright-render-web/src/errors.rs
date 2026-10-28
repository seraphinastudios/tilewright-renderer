// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! Stable error codes for browser callers. Every binding error is a message
//! `"<code>: <detail>"`; the TypeScript wrapper turns the code into a typed
//! `TilewrightError`.

use crate::{packed::DecodeError, textures::TextureError};
use tilewright_render_api::Error as RenderError;

/// An error code and its human-readable detail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodedError {
    pub code: &'static str,
    pub detail: String,
}

impl CodedError {
    #[must_use]
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    /// The message browsers receive.
    #[must_use]
    pub fn message(&self) -> String {
        format!("{}: {}", self.code, self.detail)
    }
}

impl From<&RenderError> for CodedError {
    fn from(error: &RenderError) -> Self {
        match error {
            RenderError::InvalidTarget => Self::new(
                "invalid-target",
                "the scene does not match the target size, or the target cannot present",
            ),
            RenderError::InvalidFrame => Self::new("internal", "the frame planes are inconsistent"),
            RenderError::LimitExceeded => {
                Self::new("limit-exceeded", "a renderer limit was exceeded")
            }
            RenderError::BackendUnavailable(detail) => Self::new("backend-unavailable", detail),
            RenderError::SurfaceUnavailable => {
                Self::new("surface-unavailable", "the canvas skipped this frame")
            }
            RenderError::SurfaceLost => Self::new("surface-lost", "the canvas surface was lost"),
            RenderError::DeviceLost => Self::new("device-lost", "the GPU device was lost"),
            RenderError::Resource(detail) => Self::new("resource", detail),
            RenderError::Internal(detail) => Self::new("internal", detail),
        }
    }
}

impl From<&DecodeError> for CodedError {
    fn from(error: &DecodeError) -> Self {
        match error {
            DecodeError::Malformed(detail) => Self::new("invalid-scene", *detail),
            DecodeError::UnknownTexture { id, revision } => Self::new(
                "unknown-texture",
                format!("texture {id} revision {revision} is not uploaded"),
            ),
            DecodeError::Scene(tilewright_scene::Error::LimitExceeded) => {
                Self::new("limit-exceeded", "the scene exceeds a scene limit")
            }
            DecodeError::Scene(error) => Self::new("invalid-scene", format!("{error:?}")),
        }
    }
}

impl From<TextureError> for CodedError {
    fn from(error: TextureError) -> Self {
        match error {
            TextureError::InvalidTexture => Self::new(
                "invalid-texture",
                "identity, revision, empty size or pixel length is invalid",
            ),
            TextureError::LimitExceeded => Self::new(
                "limit-exceeded",
                "the texture or the texture store exceeds its bound",
            ),
            TextureError::Conflict => Self::new(
                "invalid-texture",
                "this identity and revision already hold different pixels; upload them as a new revision",
            ),
            TextureError::Unavailable => Self::new("internal", "the texture store is unavailable"),
        }
    }
}

/// A JavaScript number that is a safe integer: whole and between 0 and
/// 2⁵³ − 1.
///
/// # Errors
/// Returns an `invalid-argument` error naming the value otherwise.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_cmp
)]
pub fn safe_integer(value: f64, name: &str) -> Result<u64, CodedError> {
    const MAXIMUM_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
    if value.is_finite() && value.trunc() == value && (0.0..=MAXIMUM_SAFE_INTEGER).contains(&value)
    {
        Ok(value as u64)
    } else {
        Err(CodedError::new(
            "invalid-argument",
            format!("{name} must be a safe nonnegative integer"),
        ))
    }
}

/// A texture digest as `uploadTexture` returns it: 64 lowercase
/// hexadecimal digits.
///
/// # Errors
/// Returns an `invalid-argument` error otherwise.
pub fn parse_digest(value: &str) -> Result<tilewright_scene::Digest256, CodedError> {
    let invalid = || {
        CodedError::new(
            "invalid-argument",
            "texture digest must be 64 lowercase hexadecimal digits",
        )
    };
    let digits = value.as_bytes();
    if digits.len() != 64 {
        return Err(invalid());
    }
    let digit = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    let mut digest = [0_u8; 32];
    for (byte, pair) in digest.iter_mut().zip(digits.chunks_exact(2)) {
        *byte = (digit(pair[0]).ok_or_else(invalid)? << 4) | digit(pair[1]).ok_or_else(invalid)?;
    }
    Ok(tilewright_scene::Digest256(digest))
}

#[cfg(test)]
mod tests {
    use super::{CodedError, parse_digest, safe_integer};
    use crate::{packed::DecodeError, textures::TextureError};
    use tilewright_render_api::Error as RenderError;

    #[test]
    fn errors_carry_stable_codes() {
        assert_eq!(
            CodedError::from(&RenderError::Resource(
                "renderer resource error: NotFound".into()
            ))
            .message(),
            "resource: renderer resource error: NotFound"
        );
        assert_eq!(
            CodedError::from(&RenderError::SurfaceUnavailable).code,
            "surface-unavailable"
        );
        assert_eq!(
            CodedError::from(&DecodeError::UnknownTexture { id: 4, revision: 2 }).message(),
            "unknown-texture: texture 4 revision 2 is not uploaded"
        );
        assert_eq!(
            CodedError::from(&DecodeError::Scene(tilewright_scene::Error::LimitExceeded)).code,
            "limit-exceeded"
        );
        assert_eq!(
            CodedError::from(&DecodeError::Scene(
                tilewright_scene::Error::DuplicateSemanticId
            ))
            .message(),
            "invalid-scene: DuplicateSemanticId"
        );
        assert_eq!(
            CodedError::from(TextureError::InvalidTexture).code,
            "invalid-texture"
        );
    }

    #[test]
    fn identities_are_safe_integers() {
        assert_eq!(safe_integer(0.0, "id"), Ok(0));
        assert_eq!(
            safe_integer(9_007_199_254_740_991.0, "id"),
            Ok(9_007_199_254_740_991)
        );
        for invalid in [-1.0, 0.5, 9_007_199_254_740_992.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                safe_integer(invalid, "id").map_err(|error| error.code),
                Err("invalid-argument"),
                "{invalid}"
            );
        }
    }

    #[test]
    fn digests_are_lowercase_hexadecimal() {
        let hex = "00ff10".to_owned() + &"ab".repeat(29);
        let digest = parse_digest(&hex).unwrap();
        assert_eq!(digest.0[..3], [0x00, 0xff, 0x10]);
        assert_eq!(digest.0[3..], [0xab; 29]);
        for invalid in [
            String::new(),
            "ab".repeat(31),
            "ab".repeat(33),
            "AB".repeat(32),
            "g0".repeat(32),
            "é".repeat(32),
        ] {
            assert_eq!(
                parse_digest(&invalid).map_err(|error| error.code),
                Err("invalid-argument"),
                "{invalid}"
            );
        }
    }
}
