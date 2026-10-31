// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

mod atlas;
mod crowd;
#[cfg(test)]
mod seams;
mod ui;
mod world;
pub use atlas::{
    ATLAS_FRAME_SIZE, ATLAS_ID, ATLAS_SHEET, atlas_digest, atlas_provider, atlas_resource,
    atlas_scene, character_sheet,
};
pub use crowd::{
    CROWD_ATLAS_ID, CROWD_FRAME, CROWD_LAYER_IDS, CROWD_SHEET, CROWD_TILE, CROWD_TILES_ID, Crowd,
    CrowdSheets, crowd_provider, crowd_scene, crowd_textures,
};
pub use ui::{UI_ATLAS_ID, UI_SOLID_ID, UI_WORLD_ID, shared_ui_provider, shared_ui_scene};
pub use world::{
    GroundArt, ground_grid_provider, ground_grid_scene, ground_grid_view, iso_world_scene,
    top_down_world_scene, world_provider, world_texture,
};

use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, RwLock},
};
use tilewright_render_api::{
    Error as RenderError, FrameOutput, RecoveryMetrics, RenderRequest, Renderer, TargetDescriptor,
    TargetKind, TargetLimits,
};
use tilewright_render_resources::{
    DEFAULT_MAXIMUM_RESOURCE_BYTES, Error as ResourceError, MAXIMUM_SCENE_TEXTURES, ResourceBlob,
    ResourceProvider, ResourceRequest, Rgba8Image, add_working_set_bytes, for_each_quad_pixel,
    glow_rgba8, quad_frame, sample_quad_rgba8, shade_quad_rgba8,
};
use tilewright_scene::{
    Digest256, Layer, Quad, ResourceId, Sampling, SceneBundle, SceneLimits, SemanticId, Sprite,
    TextureRect, Viewport,
};

/// Bytes per GPU vertex: position (3), colour (4), uv (2), effect, greyscale,
/// glow colour (4) and glow radius (2) as `f32`, a `u32` of flags, the
/// texture frame as four `u16`, and physical/UV rectangles as eight `f32`.
pub const VERTEX_BYTES: u64 = 112;

#[derive(Default)]
pub struct FakeResourceProvider {
    blobs: RwLock<BTreeMap<(ResourceId, u64), Vec<u8>>>,
}

impl FakeResourceProvider {
    /// Inserts a versioned immutable blob.
    ///
    /// # Errors
    /// Returns [`ResourceError::Unavailable`] if another test poisoned the lock.
    pub fn insert(
        &self,
        id: ResourceId,
        revision: u64,
        bytes: Vec<u8>,
    ) -> Result<(), ResourceError> {
        self.blobs
            .write()
            .map_err(|_| ResourceError::Unavailable)?
            .insert((id, revision), bytes);
        Ok(())
    }
}

impl ResourceProvider for FakeResourceProvider {
    fn load(&self, request: ResourceRequest) -> Result<ResourceBlob, ResourceError> {
        request.validate()?;
        let bytes = self
            .blobs
            .read()
            .map_err(|_| ResourceError::Unavailable)?
            .get(&(request.id, request.revision))
            .cloned()
            .ok_or(ResourceError::NotFound)?;
        ResourceBlob::for_request(request, bytes)
    }
}

/// Deterministic software reference used by contract tests. It is intentionally
/// simple: sprites are solid, half-open rectangles in logical pixel space.
pub struct ReferenceRenderer {
    target: TargetDescriptor,
}

/// Programmable renderer double for consumer lifecycle and failure tests.
pub struct FakeRenderer {
    target: TargetDescriptor,
    responses: VecDeque<Result<FrameOutput, RenderError>>,
    rendered_revisions: Vec<u64>,
}

impl FakeRenderer {
    /// Creates a fake with an explicit validated target.
    ///
    /// # Errors
    /// Returns a target error when the descriptor exceeds default limits.
    pub fn new(target: TargetDescriptor) -> Result<Self, RenderError> {
        Ok(Self {
            target: target.validate(TargetLimits::default())?,
            responses: VecDeque::new(),
            rendered_revisions: Vec::new(),
        })
    }

    pub fn push_response(&mut self, response: Result<FrameOutput, RenderError>) {
        self.responses.push_back(response);
    }

    #[must_use]
    pub fn rendered_revisions(&self) -> &[u64] {
        &self.rendered_revisions
    }
}

impl Renderer for FakeRenderer {
    fn target(&self) -> TargetDescriptor {
        self.target
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), RenderError> {
        self.target = TargetDescriptor {
            width,
            height,
            ..self.target
        }
        .validate(TargetLimits::default())?;
        Ok(())
    }

    fn render(&mut self, request: RenderRequest<'_>) -> Result<FrameOutput, RenderError> {
        self.rendered_revisions.push(request.scene.revision());
        self.responses.pop_front().unwrap_or_else(|| {
            Err(RenderError::Internal(
                "fake response queue is empty".to_owned(),
            ))
        })
    }
}

impl ReferenceRenderer {
    /// Creates a bounded renderer-neutral reference target.
    ///
    /// # Errors
    /// Returns a target error when the descriptor is invalid.
    pub fn new(target: TargetDescriptor) -> Result<Self, RenderError> {
        Ok(Self {
            target: target.validate(TargetLimits::default())?,
        })
    }
}

impl Renderer for ReferenceRenderer {
    fn target(&self) -> TargetDescriptor {
        self.target
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), RenderError> {
        self.target = TargetDescriptor {
            width,
            height,
            ..self.target
        }
        .validate(TargetLimits::default())?;
        Ok(())
    }

    fn render(&mut self, request: RenderRequest<'_>) -> Result<FrameOutput, RenderError> {
        if request.scene.viewport().width != self.target.width
            || request.scene.viewport().height != self.target.height
        {
            return Err(RenderError::InvalidTarget);
        }
        let pixel_count = pixel_count(self.target.width, self.target.height)?;
        let clear = color_to_rgba8(request.scene.clear_color());
        let mut output = FrameOutput {
            width: self.target.width,
            height: self.target.height,
            rgba8: clear.repeat(pixel_count),
            semantic_ids: vec![0; pixel_count],
            depth: vec![i16::MIN; pixel_count],
            coverage: vec![0; pixel_count],
            metrics: RecoveryMetrics::default(),
        };
        // The same per-scene texture bound as every other renderer, checked
        // before any resource is loaded.
        let distinct = request
            .scene
            .quads()
            .iter()
            .map(|quad| {
                (
                    quad.resource_id,
                    quad.resource_revision,
                    quad.resource_digest,
                )
            })
            .collect::<std::collections::HashSet<_>>();
        if distinct.len() > MAXIMUM_SCENE_TEXTURES {
            return Err(RenderError::LimitExceeded);
        }
        let mut uploaded_bytes = 0_u64;
        let mut resource_requests = 0_u64;
        let mut images: HashMap<(ResourceId, u64, Digest256), Arc<Rgba8Image>> =
            HashMap::with_capacity(distinct.len());
        for quad in request.scene.quads() {
            let key = (
                quad.resource_id,
                quad.resource_revision,
                quad.resource_digest,
            );
            let image = if let Some(image) = images.get(&key) {
                image.clone()
            } else {
                let blob = request
                    .resources
                    .load(ResourceRequest {
                        id: quad.resource_id,
                        revision: quad.resource_revision,
                        digest: quad.resource_digest,
                        maximum_bytes: DEFAULT_MAXIMUM_RESOURCE_BYTES,
                    })
                    .map_err(|error| RenderError::Resource(error.to_string()))?;
                charge_before_decoding(uploaded_bytes, &blob)?;
                let image = Arc::new(
                    Rgba8Image::decode(&blob, 8_192, 16_777_216)
                        .map_err(|error| RenderError::Resource(error.to_string()))?,
                );
                uploaded_bytes = add_working_set_bytes(uploaded_bytes, image.pixels().len() as u64)
                    .map_err(|_| RenderError::LimitExceeded)?;
                resource_requests += 1;
                images.insert(key, image.clone());
                image
            };
            let frame = quad_frame(quad, &image)
                .map_err(|error| RenderError::Resource(error.to_string()))?;
            raster_quad(
                &mut output,
                quad,
                &image,
                frame,
                request.scene.clock_millis(),
            );
        }
        output.metrics.frames_submitted = 1;
        output.metrics.resource_requests = resource_requests;
        output.metrics.uploaded_bytes = uploaded_bytes;
        output.metrics.vertex_count = (request.scene.quads().len() as u64) * 6;
        output.metrics.vertex_allocation_bytes = output.metrics.vertex_count * VERTEX_BYTES;
        output.metrics.target_allocation_bytes = (pixel_count as u64) * 11;
        output.validate()?;
        Ok(output)
    }
}

/// Charges a blob's pixel payload (the envelope after its 12-byte header)
/// to the working set before any pixels are decoded.
fn charge_before_decoding(uploaded_bytes: u64, blob: &ResourceBlob) -> Result<(), RenderError> {
    let payload = (blob.bytes().len() as u64).saturating_sub(12);
    add_working_set_bytes(uploaded_bytes, payload)
        .map(|_| ())
        .map_err(|_| RenderError::LimitExceeded)
}

fn pixel_count(width: u32, height: u32) -> Result<usize, RenderError> {
    usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(RenderError::LimitExceeded)
}

fn raster_quad(
    output: &mut FrameOutput,
    quad: &Quad,
    image: &Rgba8Image,
    frame: TextureRect,
    clock_millis: u64,
) {
    for_each_quad_pixel(quad, output.width, output.height, |_x, _y, index, u, v| {
        let source = match quad.glow {
            Some(glow) => glow_rgba8(image, frame, u, v, &glow, quad.sampling),
            None => shade_quad_rgba8(
                sample_quad_rgba8(image, frame, u, v, quad.sampling),
                quad,
                clock_millis,
            ),
        };
        if source[3] != 0 {
            blend(&mut output.rgba8[index * 4..index * 4 + 4], source);
            if quad.pickable {
                output.semantic_ids[index] = quad.semantic_id.0;
                output.depth[index] = quad.depth;
                output.coverage[index] = source[3];
            }
        }
    });
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn color_to_rgba8(color: [f32; 4]) -> [u8; 4] {
    color.map(|channel| (channel.mul_add(255.0, 0.5)).floor() as u8)
}

fn blend(destination: &mut [u8], source: [u8; 4]) {
    let alpha = u32::from(source[3]);
    let inverse = 255 - alpha;
    for channel in 0..3 {
        destination[channel] = u8::try_from(
            (u32::from(source[channel]) * alpha + u32::from(destination[channel]) * inverse + 127)
                / 255,
        )
        .expect("8-bit blend channel remains in range");
    }
    destination[3] = u8::try_from(alpha + (u32::from(destination[3]) * inverse + 127) / 255)
        .expect("8-bit alpha remains in range");
}

/// Wraps row-major RGBA8 pixels in the `ATX1` texture envelope and returns
/// the envelope with its SHA-256 digest.
#[must_use]
pub fn texture_envelope(width: u32, height: u32, pixels: &[[u8; 4]]) -> (Vec<u8>, Digest256) {
    let mut bytes = Vec::with_capacity(12 + pixels.len() * 4);
    bytes.extend_from_slice(b"ATX1");
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    for pixel in pixels {
        bytes.extend_from_slice(pixel);
    }
    let digest = Digest256(Sha256::digest(&bytes).into());
    (bytes, digest)
}

#[must_use]
pub fn synthetic_digest() -> Digest256 {
    Digest256(Sha256::digest(synthetic_resource()).into())
}

#[must_use]
pub fn synthetic_resource() -> &'static [u8] {
    b"ATX1\x02\x00\x00\x00\x02\x00\x00\x00\xff\xff\xff\xff\xff\x40\x20\xff\x20\xff\x80\xc0\x20\x60\xff\x40"
}

/// Creates a provider containing every resource used by synthetic scenes.
///
/// # Errors
/// Returns a provider availability error if its internal test lock is poisoned.
pub fn synthetic_provider() -> Result<Arc<FakeResourceProvider>, ResourceError> {
    let provider = Arc::new(FakeResourceProvider::default());
    provider.insert(ResourceId(1), 1, synthetic_resource().to_vec())?;
    Ok(provider)
}

/// Builds the repository-owned visual contract scene at an explicit size.
///
/// # Errors
/// Returns a scene validation error for dimensions outside scene bounds.
#[must_use = "synthetic scene construction can fail"]
#[allow(clippy::cast_precision_loss)]
pub fn synthetic_scene(width: u32, height: u32) -> Result<SceneBundle, tilewright_scene::Error> {
    SceneBundle::new(
        1,
        375,
        Viewport {
            width,
            height,
            scale_milli: 1_000,
        },
        [0.02, 0.03, 0.05, 1.0],
        [
            Sprite {
                semantic_id: SemanticId(1),
                resource_id: ResourceId(1),
                resource_revision: 1,
                resource_digest: synthetic_digest(),
                frame: None,
                x: 1.0,
                y: 1.0,
                width: (width / 2).max(1) as f32,
                height: (height / 2).max(1) as f32,
                depth: 0,
                painter_order: 0,
                color: [0.85, 0.12, 0.18, 1.0],
                sampling: Sampling::Nearest,
                layer: Layer::World,
            },
            Sprite {
                semantic_id: SemanticId(2),
                resource_id: ResourceId(1),
                resource_revision: 1,
                resource_digest: synthetic_digest(),
                frame: None,
                x: (width / 3) as f32,
                y: (height / 3) as f32,
                width: (width / 2).max(1) as f32,
                height: (height / 2).max(1) as f32,
                depth: 1,
                painter_order: 1,
                color: [0.1, 0.55, 0.95, 0.75],
                sampling: Sampling::Linear,
                layer: Layer::Effect,
            },
            Sprite {
                semantic_id: SemanticId(3),
                resource_id: ResourceId(1),
                resource_revision: 1,
                resource_digest: synthetic_digest(),
                frame: None,
                x: -1.25,
                y: height.saturating_sub(3) as f32,
                width: width.saturating_add(2) as f32,
                height: 2.5,
                depth: 0,
                painter_order: 0,
                color: [0.95, 0.95, 0.75, 0.8],
                sampling: Sampling::Nearest,
                layer: Layer::Interface,
            },
        ],
        SceneLimits::default(),
    )
}

/// Builds a bounded high-density batching and allocation scenario.
///
/// # Errors
/// Returns a scene validation error if requested dimensions cannot contain the
/// fixed 1,024-sprite scenario.
#[allow(clippy::cast_precision_loss)]
pub fn dense_scene(width: u32, height: u32) -> Result<SceneBundle, tilewright_scene::Error> {
    let mut sprites = Vec::with_capacity(1_024);
    for index in 0..1_024_u32 {
        let column = index % 32;
        let row = index / 32;
        let depth = i16::try_from(index % 17).map_err(|_| tilewright_scene::Error::InvalidValue)?;
        sprites.push(Sprite {
            semantic_id: SemanticId(index + 1),
            resource_id: ResourceId(1),
            resource_revision: 1,
            resource_digest: synthetic_digest(),
            frame: None,
            x: (column * 4) as f32,
            y: (row * 4) as f32,
            width: 4.0,
            height: 4.0,
            depth,
            painter_order: index,
            color: [0.55, 0.7, 0.9, 0.9],
            sampling: if index % 2 == 0 {
                Sampling::Nearest
            } else {
                Sampling::Linear
            },
            layer: if index % 11 == 0 {
                Layer::Effect
            } else {
                Layer::World
            },
        });
    }
    SceneBundle::new(
        2,
        375,
        Viewport {
            width,
            height,
            scale_milli: 1_000,
        },
        [0.01, 0.01, 0.02, 1.0],
        sprites,
        SceneLimits::default(),
    )
}

/// Builds a synthetic structural/tall/multipart/fog/cutaway/UI contract scene.
///
/// # Errors
/// Returns a scene validation error for dimensions below the fixture contract.
pub fn structural_scene(width: u32, height: u32) -> Result<SceneBundle, tilewright_scene::Error> {
    if width < 32 || height < 32 {
        return Err(tilewright_scene::Error::InvalidValue);
    }
    let digest = synthetic_digest();
    SceneBundle::new(
        3,
        375,
        Viewport {
            width,
            height,
            scale_milli: 1_500,
        },
        [0.04, 0.05, 0.08, 1.0],
        [
            fixture_sprite(1, [3.0, 20.0, 26.0, 5.0], -2, 0, Layer::World, digest),
            fixture_sprite(2, [8.0, 7.0, 5.0, 20.0], -1, 1, Layer::World, digest),
            fixture_sprite(3, [13.0, 11.0, 5.0, 16.0], 0, 2, Layer::World, digest),
            fixture_sprite(4, [18.0, 15.0, 5.0, 12.0], 1, 3, Layer::World, digest),
            fixture_sprite(5, [23.0, 19.0, 5.0, 8.0], 2, 4, Layer::World, digest),
            Sprite {
                color: [0.35, 0.55, 0.8, 0.28],
                sampling: Sampling::Linear,
                ..fixture_sprite(6, [-2.0, 9.5, 36.0, 14.0], 3, 0, Layer::Effect, digest)
            },
            Sprite {
                color: [1.0, 0.92, 0.55, 0.65],
                ..fixture_sprite(7, [10.0, 4.0, 12.0, 6.0], 4, 1, Layer::Effect, digest)
            },
            Sprite {
                color: [0.12, 0.15, 0.2, 0.92],
                ..fixture_sprite(8, [0.0, 27.0, 32.0, 5.0], 0, 0, Layer::Interface, digest)
            },
            Sprite {
                color: [0.95, 0.95, 0.9, 1.0],
                ..fixture_sprite(9, [35.0, 26.0, 2.0, 6.0], 1, 1, Layer::Interface, digest)
            },
            Sprite {
                color: [0.95, 0.95, 0.9, 1.0],
                ..fixture_sprite(10, [41.0, 26.0, 2.0, 6.0], 1, 2, Layer::Interface, digest)
            },
            Sprite {
                color: [0.95, 0.95, 0.9, 1.0],
                ..fixture_sprite(11, [35.0, 28.0, 8.0, 2.0], 1, 3, Layer::Interface, digest)
            },
        ],
        SceneLimits::default(),
    )
}

fn fixture_sprite(
    semantic_id: u32,
    bounds: [f32; 4],
    depth: i16,
    painter_order: u32,
    layer: Layer,
    digest: Digest256,
) -> Sprite {
    Sprite {
        semantic_id: SemanticId(semantic_id),
        resource_id: ResourceId(1),
        resource_revision: 1,
        resource_digest: digest,
        frame: None,
        x: bounds[0],
        y: bounds[1],
        width: bounds[2],
        height: bounds[3],
        depth,
        painter_order,
        color: [0.85, 0.75, 0.6, 1.0],
        sampling: Sampling::Nearest,
        layer,
    }
}

#[must_use]
pub const fn offscreen_target(width: u32, height: u32) -> TargetDescriptor {
    TargetDescriptor {
        kind: TargetKind::Offscreen,
        width,
        height,
        backend: tilewright_render_api::BackendPreference::Automatic,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FakeRenderer, FakeResourceProvider, ReferenceRenderer, offscreen_target,
        synthetic_provider, synthetic_scene,
    };
    use sha2::{Digest, Sha256};
    use std::sync::Arc;
    use tilewright_render_api::{Error, RenderRequest, Renderer};
    use tilewright_render_resources::{
        Error as ResourceError, ResourceBlob, ResourceProvider, ResourceRequest,
    };
    use tilewright_scene::{Digest256, ResourceId, SceneBundle, SceneLimits};

    struct FailingProvider(ResourceError);

    impl ResourceProvider for FailingProvider {
        fn load(&self, _request: ResourceRequest) -> Result<ResourceBlob, ResourceError> {
            Err(self.0)
        }
    }

    #[test]
    fn scenes_naming_too_many_textures_never_reach_a_renderer() {
        use tilewright_scene::{Layer, Quad, Sampling, SemanticId, Viewport};
        let quads = |count: u32| {
            (0..count).map(|index| Quad {
                semantic_id: SemanticId(index + 1),
                part: 0,
                resource_id: ResourceId(u64::from(index) + 1),
                resource_revision: 1,
                resource_digest: Digest256([1; 32]),
                frame: None,
                positions: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                depth: 0,
                painter_order: index,
                color: [1.0; 4],
                grayscale: 0.0,
                glow: None,
                pickable: true,
                sampling: Sampling::Nearest,
                layer: Layer::World,
            })
        };
        let viewport = Viewport {
            width: 4,
            height: 4,
            scale_milli: 1_000,
        };
        // The scene contract itself enforces the bound the renderers share.
        assert_eq!(
            SceneBundle::from_quads(
                1,
                0,
                viewport,
                [0.0; 4],
                quads(4_097),
                SceneLimits::default()
            ),
            Err(tilewright_scene::Error::LimitExceeded)
        );
        // At the bound, the reference requests each texture once (here the
        // provider refuses the first).
        let scene = SceneBundle::from_quads(
            1,
            0,
            viewport,
            [0.0; 4],
            quads(4_096),
            SceneLimits::default(),
        )
        .unwrap();
        let result = ReferenceRenderer::new(offscreen_target(4, 4))
            .unwrap()
            .render(RenderRequest {
                scene: &scene,
                resources: Arc::new(FailingProvider(ResourceError::NotFound)),
            });
        assert!(matches!(result, Err(Error::Resource(_))));
    }

    #[test]
    fn the_working_set_is_charged_before_decoding() {
        use tilewright_render_resources::DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES as BUDGET;
        // A 2 × 2 envelope carries 16 bytes of pixels after its header.
        let bytes = super::synthetic_resource().to_vec();
        let blob = ResourceBlob::for_request(
            ResourceRequest {
                id: ResourceId(1),
                revision: 1,
                digest: Digest256(Sha256::digest(&bytes).into()),
                maximum_bytes: bytes.len(),
            },
            bytes,
        )
        .unwrap();
        assert!(super::charge_before_decoding(BUDGET - 16, &blob).is_ok());
        assert_eq!(
            super::charge_before_decoding(BUDGET - 15, &blob),
            Err(Error::LimitExceeded)
        );
    }

    #[test]
    fn reference_output_is_exact_and_repeatable() {
        let scene = synthetic_scene(16, 16).unwrap();
        let request = RenderRequest {
            scene: &scene,
            resources: synthetic_provider().unwrap(),
        };
        let first = ReferenceRenderer::new(offscreen_target(16, 16))
            .unwrap()
            .render(request.clone())
            .unwrap();
        let second = ReferenceRenderer::new(offscreen_target(16, 16))
            .unwrap()
            .render(request)
            .unwrap();
        assert_eq!(first, second);
        assert!(first.semantic_ids.contains(&1));
        assert!(first.semantic_ids.contains(&2));
    }

    #[test]
    fn missing_and_digest_mismatched_resources_fail_closed() {
        let scene = synthetic_scene(16, 16).unwrap();
        let missing = ReferenceRenderer::new(offscreen_target(16, 16))
            .unwrap()
            .render(RenderRequest {
                scene: &scene,
                resources: Arc::new(FakeResourceProvider::default()),
            })
            .unwrap_err();
        assert!(matches!(missing, Error::Resource(message) if message.contains("NotFound")));

        let provider = Arc::new(FakeResourceProvider::default());
        provider
            .insert(ResourceId(1), 1, b"wrong bytes".to_vec())
            .unwrap();
        let mismatch = ReferenceRenderer::new(offscreen_target(16, 16))
            .unwrap()
            .render(RenderRequest {
                scene: &scene,
                resources: provider,
            })
            .unwrap_err();
        assert!(matches!(mismatch, Error::Resource(message) if message.contains("DigestMismatch")));

        let corrupt_bytes = b"not-an-atx1-texture".to_vec();
        let digest = Digest256(Sha256::digest(&corrupt_bytes).into());
        let mut quads = scene.quads().to_vec();
        for quad in &mut quads {
            quad.resource_digest = digest;
        }
        let corrupt_scene = SceneBundle::from_quads(
            scene.revision(),
            scene.clock_millis(),
            scene.viewport(),
            scene.clear_color(),
            quads,
            SceneLimits::default(),
        )
        .unwrap();
        let corrupt_provider = Arc::new(FakeResourceProvider::default());
        corrupt_provider
            .insert(ResourceId(1), 1, corrupt_bytes)
            .unwrap();
        let corrupt = ReferenceRenderer::new(offscreen_target(16, 16))
            .unwrap()
            .render(RenderRequest {
                scene: &corrupt_scene,
                resources: corrupt_provider,
            })
            .unwrap_err();
        assert!(matches!(corrupt, Error::Resource(message) if message.contains("InvalidFormat")));

        let oversized = ReferenceRenderer::new(offscreen_target(16, 16))
            .unwrap()
            .render(RenderRequest {
                scene: &scene,
                resources: Arc::new(FailingProvider(ResourceError::LimitExceeded)),
            })
            .unwrap_err();
        assert!(matches!(oversized, Error::Resource(message) if message.contains("LimitExceeded")));
    }

    #[test]
    fn programmable_fake_records_requests_and_failures() {
        let scene = synthetic_scene(4, 4).unwrap();
        let request = RenderRequest {
            scene: &scene,
            resources: synthetic_provider().unwrap(),
        };
        let recovered = ReferenceRenderer::new(offscreen_target(4, 4))
            .unwrap()
            .render(request.clone())
            .unwrap();
        let mut fake = FakeRenderer::new(offscreen_target(4, 4)).unwrap();
        fake.push_response(Err(Error::SurfaceUnavailable));
        fake.push_response(Err(Error::SurfaceLost));
        fake.push_response(Err(Error::DeviceLost));
        fake.push_response(Ok(recovered.clone()));
        assert_eq!(fake.render(request.clone()), Err(Error::SurfaceUnavailable));
        assert_eq!(fake.render(request.clone()), Err(Error::SurfaceLost));
        assert_eq!(fake.render(request.clone()), Err(Error::DeviceLost));
        assert_eq!(fake.render(request), Ok(recovered));
        assert_eq!(fake.rendered_revisions(), [1, 1, 1, 1]);
    }
}
