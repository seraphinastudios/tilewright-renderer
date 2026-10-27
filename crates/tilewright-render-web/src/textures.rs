// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! The browser's texture store: the resource provider behind every web
//! renderer. Pages upload decoded RGBA8 pixels; the store wraps them in the
//! `ATX1` envelope and records their SHA-256, so the renderer loads them
//! through the same verified resource contract as every other consumer.

use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, hash_map::Entry},
    sync::{Arc, RwLock},
};
use tilewright_render_resources::{
    DEFAULT_MAXIMUM_RESOURCE_BYTES, DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES,
    Error as ResourceError, MAXIMUM_SCENE_TEXTURES, ResourceBlob, ResourceProvider,
    ResourceRequest,
};
use tilewright_scene::{Digest256, ResourceId};

/// Largest texture side any store accepts: the renderer's bound on the web.
/// A store for one device accepts at most that device's limit.
pub const MAXIMUM_TEXTURE_DIMENSION: u32 = 8_192;
/// Most textures the store holds at once.
pub const MAXIMUM_STORED_TEXTURES: usize = MAXIMUM_SCENE_TEXTURES * 4;
/// Most envelope bytes the store holds at once.
pub const MAXIMUM_STORED_BYTES: u64 = DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES;
const HEADER_BYTES: usize = 12;

/// Why an upload was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureError {
    /// A zero identity or revision, an empty side, or a pixel buffer whose
    /// length is not `width × height × 4`.
    InvalidTexture,
    /// A side exceeds the store's texture side (the device's limit, at most
    /// 8,192), or the texture, or the store with it, would exceed its byte
    /// or count bound.
    LimitExceeded,
    /// The identity and revision already hold different pixels. A stored
    /// texture never changes; upload changed pixels as a new revision.
    Conflict,
    /// The store's lock was poisoned by a panic.
    Unavailable,
}

struct StoredTexture {
    digest: Digest256,
    envelope: Arc<[u8]>,
}

#[derive(Default)]
struct Store {
    textures: HashMap<(ResourceId, u64), StoredTexture>,
    bytes: u64,
}

/// Uploaded textures by identity and revision.
pub struct WebTextures {
    store: RwLock<Store>,
    maximum_bytes: u64,
    maximum_textures: usize,
    maximum_dimension: u32,
}

impl Default for WebTextures {
    fn default() -> Self {
        Self::with_limits(MAXIMUM_STORED_BYTES, MAXIMUM_STORED_TEXTURES)
    }
}

impl WebTextures {
    /// A store holding at most `maximum_bytes` of envelopes and
    /// `maximum_textures` textures.
    #[must_use]
    pub fn with_limits(maximum_bytes: u64, maximum_textures: usize) -> Self {
        Self {
            store: RwLock::default(),
            maximum_bytes,
            maximum_textures,
            maximum_dimension: MAXIMUM_TEXTURE_DIMENSION,
        }
    }

    /// The same store for a device whose textures are at most `side` texels
    /// on a side, so a texture the device cannot hold is refused on upload
    /// rather than failing every frame.
    #[must_use]
    pub fn with_maximum_dimension(mut self, side: u32) -> Self {
        self.maximum_dimension = side.min(MAXIMUM_TEXTURE_DIMENSION);
        self
    }

    /// The largest texture side this store accepts.
    #[must_use]
    pub const fn maximum_dimension(&self) -> u32 {
        self.maximum_dimension
    }

    /// Stores RGBA8 pixels as one texture identity and revision and returns
    /// its digest. Uploading the same pixels again is accepted; different
    /// pixels under a stored pair are refused.
    ///
    /// # Errors
    /// Returns [`TextureError::InvalidTexture`] for a malformed texture,
    /// [`TextureError::Conflict`] for different pixels under a stored pair,
    /// and [`TextureError::LimitExceeded`] above the device's texture side
    /// or the size or store bounds.
    pub fn insert(
        &self,
        id: ResourceId,
        revision: u64,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<Digest256, TextureError> {
        if id.0 == 0 || revision == 0 || width == 0 || height == 0 {
            return Err(TextureError::InvalidTexture);
        }
        if width > self.maximum_dimension || height > self.maximum_dimension {
            return Err(TextureError::LimitExceeded);
        }
        let expected = u64::from(width) * u64::from(height) * 4;
        if u64::try_from(rgba.len()).ok() != Some(expected) {
            return Err(TextureError::InvalidTexture);
        }
        let length = rgba.len() + HEADER_BYTES;
        if length > DEFAULT_MAXIMUM_RESOURCE_BYTES {
            return Err(TextureError::LimitExceeded);
        }
        let mut envelope = Vec::with_capacity(length);
        envelope.extend_from_slice(b"ATX1");
        envelope.extend_from_slice(&width.to_le_bytes());
        envelope.extend_from_slice(&height.to_le_bytes());
        envelope.extend_from_slice(rgba);
        let digest = Digest256(Sha256::digest(&envelope).into());
        let mut store = self.store.write().map_err(|_| TextureError::Unavailable)?;
        if let Some(stored) = store.textures.get(&(id, revision)) {
            return if stored.digest == digest {
                Ok(digest)
            } else {
                Err(TextureError::Conflict)
            };
        }
        let bytes = store.bytes + length as u64;
        if bytes > self.maximum_bytes || store.textures.len() >= self.maximum_textures {
            return Err(TextureError::LimitExceeded);
        }
        store.bytes = bytes;
        store.textures.insert(
            (id, revision),
            StoredTexture {
                digest,
                envelope: envelope.into(),
            },
        );
        Ok(digest)
    }

    /// Forgets one texture if it is stored under its full identity, digest
    /// included; returns whether it was. A stale or foreign identity whose
    /// ID and revision match another stored texture removes nothing.
    /// Renderers keep their own decoded copy until their cache evicts it.
    pub fn remove(&self, id: ResourceId, revision: u64, digest: Digest256) -> bool {
        let Ok(mut store) = self.store.write() else {
            return false;
        };
        let Entry::Occupied(entry) = store.textures.entry((id, revision)) else {
            return false;
        };
        if entry.get().digest != digest {
            return false;
        }
        let texture = entry.remove();
        store.bytes -= texture.envelope.len() as u64;
        true
    }

    /// Forgets every texture.
    pub fn clear(&self) {
        if let Ok(mut store) = self.store.write() {
            *store = Store::default();
        }
    }

    /// The digest of a stored texture.
    #[must_use]
    pub fn digest(&self, id: ResourceId, revision: u64) -> Option<Digest256> {
        let store = self.store.read().ok()?;
        store
            .textures
            .get(&(id, revision))
            .map(|texture| texture.digest)
    }

    /// Envelope bytes currently stored.
    #[must_use]
    pub fn stored_bytes(&self) -> u64 {
        self.store.read().map_or(0, |store| store.bytes)
    }
}

impl ResourceProvider for WebTextures {
    fn load(&self, request: ResourceRequest) -> Result<ResourceBlob, ResourceError> {
        let request = request.validate()?;
        let envelope = {
            let store = self.store.read().map_err(|_| ResourceError::Unavailable)?;
            let texture = store
                .textures
                .get(&(request.id, request.revision))
                .ok_or(ResourceError::NotFound)?;
            if texture.digest != request.digest {
                return Err(ResourceError::DigestMismatch);
            }
            Arc::clone(&texture.envelope)
        };
        ResourceBlob::for_request(request, envelope.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::{MAXIMUM_STORED_BYTES, TextureError, WebTextures};
    use tilewright_render_resources::{
        DEFAULT_MAXIMUM_RESOURCE_BYTES, Error, ResourceProvider, ResourceRequest, Rgba8Image,
    };
    use tilewright_scene::{Digest256, ResourceId};

    fn request(id: u64, revision: u64, digest: Digest256) -> ResourceRequest {
        ResourceRequest {
            id: ResourceId(id),
            revision,
            digest,
            maximum_bytes: DEFAULT_MAXIMUM_RESOURCE_BYTES,
        }
    }

    #[test]
    fn uploads_load_as_verified_envelopes() {
        let textures = WebTextures::default();
        let pixels = [1, 2, 3, 4, 5, 6, 7, 8];
        let digest = textures.insert(ResourceId(7), 2, 2, 1, &pixels).unwrap();
        assert_eq!(textures.digest(ResourceId(7), 2), Some(digest));
        assert_eq!(textures.stored_bytes(), 20);
        let blob = textures.load(request(7, 2, digest)).unwrap();
        let image = Rgba8Image::decode(&blob, 8_192, 1 << 24).unwrap();
        assert_eq!((image.width(), image.height()), (2, 1));
        assert_eq!(image.pixels(), pixels);
        // Another revision, or a stale digest, is not served.
        assert_eq!(textures.load(request(7, 1, digest)), Err(Error::NotFound));
        assert_eq!(
            textures.load(request(7, 2, Digest256([9; 32]))),
            Err(Error::DigestMismatch)
        );
    }

    #[test]
    fn malformed_uploads_are_refused() {
        let textures = WebTextures::default();
        for (id, revision, width, height, length) in [
            (0, 1, 1, 1, 4),
            (1, 0, 1, 1, 4),
            (1, 1, 0, 1, 0),
            (1, 1, 1, 1, 3),
            (1, 1, 1, 1, 5),
        ] {
            assert_eq!(
                textures.insert(ResourceId(id), revision, width, height, &vec![0; length]),
                Err(TextureError::InvalidTexture),
                "{id} {revision} {width}×{height} {length}"
            );
        }
        // A side past 8,192 is refused before its length is checked.
        assert_eq!(
            textures.insert(ResourceId(1), 1, 8_193, 1, &[]),
            Err(TextureError::LimitExceeded)
        );
        // An envelope larger than one resource request is refused.
        let side = 2_048;
        assert_eq!(
            textures.insert(
                ResourceId(1),
                1,
                side,
                side,
                &vec![0; (side * side * 4) as usize]
            ),
            Err(TextureError::LimitExceeded)
        );
        assert_eq!(textures.stored_bytes(), 0);
    }

    #[test]
    fn the_store_is_bounded_and_removal_frees_bytes() {
        // Room for three 2 × 2 envelopes of 28 bytes each.
        let textures = WebTextures::with_limits(3 * 28, 16);
        let pixels = [0; 16];
        for id in 1..=3 {
            textures.insert(ResourceId(id), 1, 2, 2, &pixels).unwrap();
        }
        assert_eq!(
            textures.insert(ResourceId(4), 1, 2, 2, &pixels),
            Err(TextureError::LimitExceeded)
        );
        // A stored texture never changes: the same pixels are accepted again,
        // different ones are refused.
        assert!(textures.insert(ResourceId(1), 1, 2, 2, &pixels).is_ok());
        assert_eq!(
            textures.insert(ResourceId(1), 1, 2, 2, &[1; 16]),
            Err(TextureError::Conflict)
        );
        assert!(textures.insert(ResourceId(1), 2, 2, 2, &[1; 16]).is_err());
        let digest = textures.digest(ResourceId(1), 1).unwrap();
        // Removal needs the digest too: another one removes nothing.
        assert!(!textures.remove(ResourceId(1), 1, Digest256([7; 32])));
        assert_eq!(textures.digest(ResourceId(1), 1), Some(digest));
        assert!(textures.remove(ResourceId(1), 1, digest));
        assert!(!textures.remove(ResourceId(1), 1, digest));
        assert_eq!(textures.stored_bytes(), 2 * 28);
        assert!(textures.insert(ResourceId(4), 1, 2, 2, &pixels).is_ok());
        // A pair reused for other pixels after removal is a new identity: the
        // old handle can neither remove nor load it.
        assert!(textures.remove(ResourceId(2), 1, digest));
        let reused = textures.insert(ResourceId(2), 1, 2, 2, &[3; 16]).unwrap();
        assert_ne!(reused, digest);
        assert!(!textures.remove(ResourceId(2), 1, digest));
        assert_eq!(
            textures.load(request(2, 1, digest)),
            Err(Error::DigestMismatch)
        );
        assert!(textures.remove(ResourceId(2), 1, reused));
        textures.clear();
        assert_eq!(textures.stored_bytes(), 0);
        assert_eq!(textures.digest(ResourceId(3), 1), None);
        // The default bound is the renderer's texture working set.
        assert_eq!(
            WebTextures::default().maximum_bytes,
            super::MAXIMUM_STORED_BYTES
        );
    }

    #[test]
    fn a_device_texture_limit_is_checked_on_upload() {
        let textures = WebTextures::default().with_maximum_dimension(4);
        assert_eq!(textures.maximum_dimension(), 4);
        assert!(textures.insert(ResourceId(1), 1, 4, 1, &[0; 16]).is_ok());
        assert_eq!(
            textures.insert(ResourceId(2), 1, 5, 1, &[0; 20]),
            Err(TextureError::LimitExceeded)
        );
        assert_eq!(
            textures.insert(ResourceId(2), 1, 1, 5, &[0; 20]),
            Err(TextureError::LimitExceeded)
        );
        // No device raises the store's own bound.
        assert_eq!(
            WebTextures::default()
                .with_maximum_dimension(16_384)
                .maximum_dimension(),
            super::MAXIMUM_TEXTURE_DIMENSION
        );
    }

    #[test]
    fn the_store_holds_a_bounded_number_of_textures() {
        let textures = WebTextures::with_limits(MAXIMUM_STORED_BYTES, 2);
        for id in 1..=2 {
            textures.insert(ResourceId(id), 1, 1, 1, &[0; 4]).unwrap();
        }
        assert_eq!(
            textures.insert(ResourceId(3), 1, 1, 1, &[0; 4]),
            Err(TextureError::LimitExceeded)
        );
        // Uploading a stored texture again does not add one.
        assert!(textures.insert(ResourceId(1), 1, 1, 1, &[0; 4]).is_ok());
        assert_eq!(
            WebTextures::default().maximum_textures,
            super::MAXIMUM_STORED_TEXTURES
        );
    }
}
