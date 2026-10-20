// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! The packed quad records pages submit. A scene crosses from JavaScript
//! into WebAssembly as one byte buffer of fixed-size little-endian records,
//! so a frame of thousands of quads costs one copy rather than one object
//! per quad. `docs/WEB.md` documents the layout; the TypeScript
//! `SceneBuilder` writes it.

use crate::textures::WebTextures;
use tilewright_scene::{
    Glow, Layer, Quad, ResourceId, Sampling, SceneBundle, SceneLimits, SemanticId, TextureRect,
    Viewport,
};

/// Bytes per packed quad: 39 little-endian 32-bit words.
pub const QUAD_RECORD_BYTES: usize = 156;

/// The quad writes the semantic, depth and coverage planes.
pub const FLAG_PICKABLE: u32 = 1;
/// The quad filters linearly.
pub const FLAG_LINEAR: u32 = 1 << 1;
/// The quad draws a glow halo.
pub const FLAG_GLOW: u32 = 1 << 2;
/// The quad samples a texture frame.
pub const FLAG_FRAME: u32 = 1 << 3;
/// Bits 8–9 hold the layer: 0 world, 1 effect, 2 interface.
pub const LAYER_SHIFT: u32 = 8;
const KNOWN_FLAGS: u32 = FLAG_PICKABLE | FLAG_LINEAR | FLAG_GLOW | FLAG_FRAME | (3 << LAYER_SHIFT);

/// Scene-wide values sent beside the records.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneHeader {
    pub revision: u64,
    pub clock_millis: u64,
    pub viewport: Viewport,
    pub clear_color: [f32; 4],
}

/// Why a packed scene was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    /// The buffer is not whole records, or a record holds an unknown flag,
    /// layer, or a value its flags say must be zero.
    Malformed(&'static str),
    /// A quad names a texture the store does not hold.
    UnknownTexture { id: u64, revision: u64 },
    /// The records decode but the scene is invalid.
    Scene(tilewright_scene::Error),
}

/// Decodes packed records into a validated scene, resolving each texture's
/// digest from the store.
///
/// # Errors
/// Returns [`DecodeError`] for malformed records, an unknown texture, or a
/// scene the scene contract rejects (including more quads than
/// `limits.maximum_quads`, checked before decoding).
pub fn decode_scene(
    header: SceneHeader,
    records: &[u8],
    textures: &WebTextures,
    limits: SceneLimits,
) -> Result<SceneBundle, DecodeError> {
    if !records.len().is_multiple_of(QUAD_RECORD_BYTES) {
        return Err(DecodeError::Malformed("buffer is not whole quad records"));
    }
    let count = records.len() / QUAD_RECORD_BYTES;
    if count > limits.maximum_quads {
        return Err(DecodeError::Scene(tilewright_scene::Error::LimitExceeded));
    }
    let mut quads = Vec::with_capacity(count);
    let mut last_texture = None;
    for record in records.chunks_exact(QUAD_RECORD_BYTES) {
        quads.push(decode_quad(record, textures, &mut last_texture)?);
    }
    SceneBundle::from_quads(
        header.revision,
        header.clock_millis,
        header.viewport,
        header.clear_color,
        quads,
        limits,
    )
    .map_err(DecodeError::Scene)
}

type CachedTexture = Option<((ResourceId, u64), tilewright_scene::Digest256)>;

fn decode_quad(
    record: &[u8],
    textures: &WebTextures,
    last_texture: &mut CachedTexture,
) -> Result<Quad, DecodeError> {
    let word = |index: usize| {
        let start = index * 4;
        u32::from_le_bytes([
            record[start],
            record[start + 1],
            record[start + 2],
            record[start + 3],
        ])
    };
    let float = |index: usize| f32::from_bits(word(index));
    let floats = |start: usize| -> [f32; 2] { [float(start), float(start + 1)] };
    let flags = word(3);
    if flags & !KNOWN_FLAGS != 0 {
        return Err(DecodeError::Malformed("unknown quad flag"));
    }
    let layer = match (flags >> LAYER_SHIFT) & 3 {
        0 => Layer::World,
        1 => Layer::Effect,
        2 => Layer::Interface,
        _ => return Err(DecodeError::Malformed("unknown layer")),
    };
    let part = u16::from_le_bytes([record[4], record[5]]);
    let depth = i16::from_le_bytes([record[6], record[7]]);
    let id = u64::from(word(4)) | (u64::from(word(5)) << 32);
    let revision = u64::from(word(6)) | (u64::from(word(7)) << 32);
    let key = (ResourceId(id), revision);
    let digest = match *last_texture {
        Some((cached, digest)) if cached == key => digest,
        _ => {
            let digest = textures
                .digest(key.0, key.1)
                .ok_or(DecodeError::UnknownTexture { id, revision })?;
            *last_texture = Some((key, digest));
            digest
        }
    };
    let frame_words = [word(8), word(9), word(10), word(11)];
    let frame = if flags & FLAG_FRAME == 0 {
        if frame_words != [0; 4] {
            return Err(DecodeError::Malformed(
                "frame values without the frame flag",
            ));
        }
        None
    } else {
        Some(TextureRect {
            x: frame_words[0],
            y: frame_words[1],
            width: frame_words[2],
            height: frame_words[3],
        })
    };
    let glow_words = [word(33), word(34), word(35), word(36), word(37), word(38)];
    let glow = if flags & FLAG_GLOW == 0 {
        if glow_words != [0; 6] {
            return Err(DecodeError::Malformed("glow values without the glow flag"));
        }
        None
    } else {
        Some(Glow {
            color: [float(33), float(34), float(35), float(36)],
            radius_uv: [float(37), float(38)],
        })
    };
    Ok(Quad {
        semantic_id: SemanticId(word(0)),
        part,
        resource_id: key.0,
        resource_revision: revision,
        resource_digest: digest,
        frame,
        positions: [floats(12), floats(14), floats(16), floats(18)],
        uvs: [floats(20), floats(22), floats(24), floats(26)],
        depth,
        painter_order: word(2),
        color: [float(28), float(29), float(30), float(31)],
        grayscale: float(32),
        glow,
        pickable: flags & FLAG_PICKABLE != 0,
        sampling: if flags & FLAG_LINEAR == 0 {
            Sampling::Nearest
        } else {
            Sampling::Linear
        },
        layer,
    })
}

/// Packs one quad. The TypeScript `SceneBuilder` writes the same bytes;
/// Rust callers and tests use this to build buffers.
#[must_use]
pub fn encode_quad(quad: &Quad) -> [u8; QUAD_RECORD_BYTES] {
    let mut words = [0_u32; QUAD_RECORD_BYTES / 4];
    let layer = match quad.layer {
        Layer::World => 0,
        Layer::Effect => 1,
        Layer::Interface => 2,
    };
    words[0] = quad.semantic_id.0;
    words[1] = u32::from(quad.part) | (u32::from(quad.depth.cast_unsigned()) << 16);
    words[2] = quad.painter_order;
    words[3] = (layer << LAYER_SHIFT)
        | if quad.pickable { FLAG_PICKABLE } else { 0 }
        | match quad.sampling {
            Sampling::Nearest => 0,
            Sampling::Linear => FLAG_LINEAR,
        }
        | if quad.glow.is_some() { FLAG_GLOW } else { 0 }
        | if quad.frame.is_some() { FLAG_FRAME } else { 0 };
    let split = |value: u64| {
        [
            u32::try_from(value & 0xffff_ffff).unwrap_or(0),
            u32::try_from(value >> 32).unwrap_or(0),
        ]
    };
    words[4..6].copy_from_slice(&split(quad.resource_id.0));
    words[6..8].copy_from_slice(&split(quad.resource_revision));
    if let Some(frame) = quad.frame {
        words[8..12].copy_from_slice(&[frame.x, frame.y, frame.width, frame.height]);
    }
    for (index, value) in quad
        .positions
        .iter()
        .chain(&quad.uvs)
        .flatten()
        .chain(&quad.color)
        .chain([&quad.grayscale])
        .enumerate()
    {
        words[12 + index] = value.to_bits();
    }
    if let Some(glow) = quad.glow {
        for (index, value) in glow.color.iter().chain(&glow.radius_uv).enumerate() {
            words[33 + index] = value.to_bits();
        }
    }
    let mut bytes = [0_u8; QUAD_RECORD_BYTES];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(words) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::{DecodeError, QUAD_RECORD_BYTES, SceneHeader, decode_scene, encode_quad};
    use crate::textures::WebTextures;
    use tilewright_scene::{
        Glow, Layer, Quad, ResourceId, Sampling, SceneLimits, SemanticId, TextureRect, Viewport,
    };

    fn header() -> SceneHeader {
        SceneHeader {
            revision: 3,
            clock_millis: 250,
            viewport: Viewport {
                width: 32,
                height: 16,
                scale_milli: 1_000,
            },
            clear_color: [0.1, 0.2, 0.3, 1.0],
        }
    }

    fn store() -> (WebTextures, tilewright_scene::Digest256) {
        let textures = WebTextures::default();
        let id = ResourceId((1 << 40) + 5);
        let digest = textures.insert(id, 1 << 33, 2, 2, &[255; 16]).unwrap();
        (textures, digest)
    }

    fn quad(digest: tilewright_scene::Digest256) -> Quad {
        Quad {
            semantic_id: SemanticId(9),
            part: 2,
            resource_id: ResourceId((1 << 40) + 5),
            resource_revision: 1 << 33,
            resource_digest: digest,
            frame: Some(TextureRect {
                x: 1,
                y: 0,
                width: 1,
                height: 2,
            }),
            positions: [[1.5, 2.0], [9.0, 2.5], [8.0, 12.25], [1.0, 11.0]],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [-0.25, 1.0]],
            depth: -3,
            painter_order: 70_000,
            color: [0.5, 0.25, 1.0, 0.75],
            grayscale: 0.5,
            glow: Some(Glow {
                color: [1.0, 0.5, 0.0, 0.5],
                radius_uv: [0.1, 0.2],
            }),
            pickable: false,
            sampling: Sampling::Linear,
            layer: Layer::Effect,
        }
    }

    #[test]
    fn records_round_trip_every_quad_field() {
        let (textures, digest) = store();
        let plain = Quad {
            semantic_id: SemanticId(10),
            part: 0,
            frame: None,
            glow: None,
            pickable: true,
            sampling: Sampling::Nearest,
            layer: Layer::Interface,
            depth: i16::MAX,
            ..quad(digest)
        };
        let records = [encode_quad(&quad(digest)), encode_quad(&plain)].concat();
        let scene = decode_scene(header(), &records, &textures, SceneLimits::default()).unwrap();
        assert_eq!(scene.revision(), 3);
        assert_eq!(scene.clock_millis(), 250);
        assert_eq!(scene.viewport(), header().viewport);
        // Effect sorts before interface.
        assert_eq!(scene.quads(), [quad(digest), plain]);
    }

    #[test]
    fn malformed_records_are_refused() {
        let (textures, digest) = store();
        let decode = |bytes: &[u8]| {
            decode_scene(header(), bytes, &textures, SceneLimits::default()).map(|_| ())
        };
        let record = encode_quad(&quad(digest));
        assert_eq!(
            decode(&record[..QUAD_RECORD_BYTES - 1]),
            Err(DecodeError::Malformed("buffer is not whole quad records"))
        );
        let with_word = |index: usize, value: u32| {
            let mut bytes = record;
            bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
            bytes
        };
        let flags = u32::from_le_bytes([record[12], record[13], record[14], record[15]]);
        assert_eq!(
            decode(&with_word(3, flags | 1 << 4)),
            Err(DecodeError::Malformed("unknown quad flag"))
        );
        assert_eq!(
            decode(&with_word(3, flags | 3 << 8)),
            Err(DecodeError::Malformed("unknown layer"))
        );
        let plain = encode_quad(&Quad {
            frame: None,
            glow: None,
            ..quad(digest)
        });
        let mut framed = plain;
        framed[32] = 1;
        assert_eq!(
            decode(&framed),
            Err(DecodeError::Malformed(
                "frame values without the frame flag"
            ))
        );
        let mut glowing = plain;
        glowing[33 * 4 + 3] = 0x3f;
        assert_eq!(
            decode(&glowing),
            Err(DecodeError::Malformed("glow values without the glow flag"))
        );
        // Scene validation still applies: a pickable glow is invalid.
        assert_eq!(
            decode(&with_word(3, flags | 1)),
            Err(DecodeError::Scene(tilewright_scene::Error::InvalidValue))
        );
    }

    #[test]
    fn unknown_textures_and_oversized_buffers_are_refused() {
        let (textures, digest) = store();
        let missing = encode_quad(&Quad {
            resource_revision: 2,
            ..quad(digest)
        });
        assert_eq!(
            decode_scene(header(), &missing, &textures, SceneLimits::default()),
            Err(DecodeError::UnknownTexture {
                id: (1 << 40) + 5,
                revision: 2
            })
        );
        let limits = SceneLimits {
            maximum_quads: 1,
            ..SceneLimits::default()
        };
        let two = [encode_quad(&quad(digest)), encode_quad(&quad(digest))].concat();
        assert_eq!(
            decode_scene(header(), &two, &textures, limits),
            Err(DecodeError::Scene(tilewright_scene::Error::LimitExceeded))
        );
        // An empty buffer is an empty scene.
        assert!(decode_scene(header(), &[], &textures, SceneLimits::default()).is_ok());
    }
}
