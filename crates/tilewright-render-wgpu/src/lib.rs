// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

use bytemuck::{Pod, Zeroable};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    sync::{Arc, mpsc},
    time::Instant,
};
use tilewright_render_api::{
    BackendPreference, Error, FrameOutput, RecoveryMetrics, RenderRequest, Renderer,
    TargetDescriptor, TargetKind, TargetLimits,
};
use tilewright_render_resources::{
    DEFAULT_MAXIMUM_RESOURCE_BYTES, DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES,
    MAXIMUM_SCENE_TEXTURES, ResourceBlob, ResourceProvider, ResourceRequest, Rgba8Image,
    add_working_set_bytes, for_each_quad_pixel, frame_in_image, sample_quad_rgba8,
    shade_quad_rgba8, subpixel_position,
};
use tilewright_scene::{Digest256, Layer, Quad, ResourceId, Sampling, SceneBundle, TextureRect};

const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The cache holds at most one scene's distinct textures.
const MAXIMUM_CACHED_TEXTURES: usize = MAXIMUM_SCENE_TEXTURES;
type TextureKey = (ResourceId, u64, Digest256);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterDescription {
    pub name: String,
    pub driver: String,
    pub backend: String,
    pub device_type: String,
    pub maximum_texture_dimension_2d: u32,
    pub maximum_buffer_size: u64,
    pub startup_micros: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationDescription {
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub reconfigured: bool,
}

pub struct WgpuRenderer {
    target: TargetDescriptor,
    instance: wgpu::Instance,
    adapter_handle: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: Pipelines,
    surface_pipelines: HashMap<wgpu::TextureFormat, Pipelines>,
    texture_layout: wgpu::BindGroupLayout,
    /// Nearest and linear samplers shared by every texture's bindings.
    samplers: [wgpu::Sampler; 2],
    adapter: AdapterDescription,
    metrics: RecoveryMetrics,
    offscreen: Option<OffscreenTarget>,
    surface_depth: Option<SurfaceDepthTarget>,
    vertices: Option<VertexCache>,
    textures: HashMap<TextureKey, TextureCacheEntry>,
    texture_bytes: u64,
    /// Target bounds narrowed to what the device was created with.
    limits: TargetLimits,
}

struct OffscreenTarget {
    size: wgpu::Extent3d,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    unpadded_row: u32,
    padded_row: u32,
}

struct TextureCacheEntry {
    image: Arc<Rgba8Image>,
    nearest: wgpu::BindGroup,
    linear: wgpu::BindGroup,
}

struct SurfaceDepthTarget {
    size: wgpu::Extent3d,
    view: wgpu::TextureView,
}

struct VertexCache {
    buffer: wgpu::Buffer,
    capacity: u64,
}

impl WgpuRenderer {
    /// Creates an offscreen Vulkan or Direct3D 12 renderer.
    ///
    /// # Errors
    /// Returns a typed target, adapter, or device creation error.
    pub fn new(target: TargetDescriptor) -> Result<Self, Error> {
        let startup = Instant::now();
        let target = target.validate(TargetLimits::default())?;
        let (instance, adapter_handle, device, queue, adapter) =
            initialize_gpu(target.backend, startup)?;
        let limits = TargetLimits {
            maximum_dimension: device.limits().max_texture_dimension_2d,
            ..TargetLimits::default()
        };
        let target = target.validate(limits)?;
        let texture_layout = create_texture_layout(&device);
        let samplers = [
            create_sampler(&device, wgpu::FilterMode::Nearest),
            create_sampler(&device, wgpu::FilterMode::Linear),
        ];
        let pipelines = create_pipelines(&device, &texture_layout, OUTPUT_FORMAT);
        Ok(Self {
            target,
            instance,
            adapter_handle,
            device,
            queue,
            pipelines,
            surface_pipelines: HashMap::new(),
            texture_layout,
            samplers,
            adapter,
            metrics: RecoveryMetrics::default(),
            offscreen: None,
            surface_depth: None,
            vertices: None,
            textures: HashMap::new(),
            texture_bytes: 0,
            limits,
        })
    }

    #[must_use]
    pub fn adapter(&self) -> &AdapterDescription {
        &self.adapter
    }

    #[must_use]
    pub const fn metrics(&self) -> RecoveryMetrics {
        self.metrics
    }

    /// Renders the same scene/material path directly into a native surface and
    /// presents it using this renderer's existing adapter, device, and queue.
    ///
    /// # Errors
    /// Returns a typed target, surface, resource, or backend error. Timeout and
    /// occlusion are reported as unavailable frames; outdated surfaces are
    /// reconfigured and retried once.
    pub fn present_surface<'window, S>(
        &mut self,
        source: S,
        request: &RenderRequest<'_>,
    ) -> Result<PresentationDescription, Error>
    where
        S: wgpu::rwh::HasWindowHandle + wgpu::rwh::HasDisplayHandle + Send + Sync + 'window,
    {
        let started = Instant::now();
        if !matches!(self.target.kind, TargetKind::Window | TargetKind::Embedded)
            || request.scene.viewport().width != self.target.width
            || request.scene.viewport().height != self.target.height
        {
            return Err(Error::InvalidTarget);
        }
        let surface = self
            .instance
            .create_surface(source)
            .map_err(|error| Error::Internal(error.to_string()))?;
        let configuration = self.surface_configuration(&surface)?;
        surface.configure(&self.device, &configuration);
        let (frame, reconfigured) = self.acquire_surface_frame(&surface, &configuration)?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let (depth_view, depth_allocated) = self.ensure_surface_depth();
        // The device must be able to hold the vertex stream before any
        // resource is loaded or the cache changes.
        self.check_vertex_stream(request.scene)?;
        let prepared = self.prepare_sprites(request.scene, request.resources.as_ref())?;
        let vertices = vertices(request.scene, &prepared)?;
        let (vertex_buffer, vertex_allocation_bytes) = self.ensure_vertex_buffer(&vertices)?;
        let pipelines = self.ensure_surface_pipelines(configuration.format);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Tilewright surface frame encoder"),
            });
        encode_scene(
            &mut encoder,
            &pipelines,
            &view,
            &depth_view,
            &vertex_buffer,
            &prepared,
            request.scene.clear_color(),
        )?;
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        self.metrics.frames_submitted += 1;
        self.metrics.resource_requests = prepared
            .iter()
            .filter(|sprite| sprite.uploaded_bytes != 0)
            .count() as u64;
        self.metrics.uploaded_bytes = prepared
            .iter()
            .try_fold(0_u64, |total, sprite| {
                total.checked_add(sprite.uploaded_bytes)
            })
            .ok_or(Error::LimitExceeded)?;
        self.metrics.vertex_count = vertices.len() as u64;
        self.metrics.vertex_allocation_bytes = vertex_allocation_bytes;
        self.metrics.target_allocation_bytes = if depth_allocated {
            u64::from(self.target.width)
                .checked_mul(u64::from(self.target.height))
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or(Error::LimitExceeded)?
        } else {
            0
        };
        self.metrics.readback_bytes = 0;
        self.metrics.frame_cpu_micros = duration_micros(started.elapsed());
        if reconfigured {
            self.metrics.surface_reconfigurations += 1;
        }
        Ok(PresentationDescription {
            width: self.target.width,
            height: self.target.height,
            format: format!("{:?}", configuration.format),
            reconfigured,
        })
    }

    fn surface_configuration(
        &self,
        surface: &wgpu::Surface<'_>,
    ) -> Result<wgpu::SurfaceConfiguration, Error> {
        let capabilities = surface.get_capabilities(&self.adapter_handle);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .ok_or_else(|| Error::BackendUnavailable("surface exposes no formats".to_owned()))?;
        let present_mode = if capabilities
            .present_modes
            .contains(&wgpu::PresentMode::Fifo)
        {
            wgpu::PresentMode::Fifo
        } else {
            *capabilities.present_modes.first().ok_or_else(|| {
                Error::BackendUnavailable("surface exposes no presentation modes".to_owned())
            })?
        };
        let alpha_mode = *capabilities.alpha_modes.first().ok_or_else(|| {
            Error::BackendUnavailable("surface exposes no alpha modes".to_owned())
        })?;
        Ok(wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: self.target.width,
            height: self.target.height,
            present_mode,
            alpha_mode,
            view_formats: Vec::new(),
            desired_maximum_frame_latency: 2,
        })
    }

    fn acquire_surface_frame(
        &mut self,
        surface: &wgpu::Surface<'_>,
        configuration: &wgpu::SurfaceConfiguration,
    ) -> Result<(wgpu::SurfaceTexture, bool), Error> {
        match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok((frame, false)),
            wgpu::CurrentSurfaceTexture::Outdated => {
                surface.configure(&self.device, configuration);
                match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok((frame, true)),
                    _ => Err(Error::SurfaceLost),
                }
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                self.metrics.skipped_frames += 1;
                Err(Error::SurfaceUnavailable)
            }
            wgpu::CurrentSurfaceTexture::Lost => Err(Error::SurfaceLost),
            other @ wgpu::CurrentSurfaceTexture::Validation => Err(Error::Internal(format!(
                "surface acquisition failed: {other:?}"
            ))),
        }
    }

    fn ensure_surface_pipelines(&mut self, format: wgpu::TextureFormat) -> Pipelines {
        self.surface_pipelines
            .entry(format)
            .or_insert_with(|| create_pipelines(&self.device, &self.texture_layout, format))
            .clone()
    }

    fn ensure_surface_depth(&mut self) -> (wgpu::TextureView, bool) {
        let size = target_extent(self.target);
        if let Some(target) = self
            .surface_depth
            .as_ref()
            .filter(|target| target.size == size)
        {
            return (target.view.clone(), false);
        }
        let texture = create_depth_target(
            &self.device,
            size,
            "Tilewright surface depth/stencil target",
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.surface_depth = Some(SurfaceDepthTarget {
            size,
            view: view.clone(),
        });
        (view, true)
    }

    fn ensure_offscreen_target(&mut self) -> Result<bool, Error> {
        if self.offscreen.as_ref().is_some_and(|target| {
            target.size.width == self.target.width && target.size.height == self.target.height
        }) {
            return Ok(false);
        }
        let size = target_extent(self.target);
        let color = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Tilewright offscreen color target"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = create_depth_target(
            &self.device,
            size,
            "Tilewright offscreen depth/stencil target",
        );
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let unpadded_row = self
            .target
            .width
            .checked_mul(4)
            .ok_or(Error::LimitExceeded)?;
        let padded_row = unpadded_row
            .div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            .checked_mul(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            .ok_or(Error::LimitExceeded)?;
        let readback_size = u64::from(padded_row)
            .checked_mul(u64::from(self.target.height))
            .ok_or(Error::LimitExceeded)?;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tilewright offscreen readback"),
            size: readback_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.offscreen = Some(OffscreenTarget {
            size,
            color,
            color_view,
            depth_view,
            readback,
            unpadded_row,
            padded_row,
        });
        Ok(true)
    }

    fn render_frame(
        &mut self,
        scene: &SceneBundle,
        resources: &dyn ResourceProvider,
    ) -> Result<(Vec<u8>, SemanticPlanes, FrameWork), Error> {
        // The device must be able to hold the vertex stream before any
        // resource is loaded or the cache changes.
        self.check_vertex_stream(scene)?;
        let prepared = self.prepare_sprites(scene, resources)?;
        let vertices = vertices(scene, &prepared)?;
        let (vertex_buffer, vertex_allocation_bytes) = self.ensure_vertex_buffer(&vertices)?;
        let allocated = self.ensure_offscreen_target()?;
        let target = self
            .offscreen
            .as_ref()
            .ok_or_else(|| Error::Internal("offscreen target missing after creation".to_owned()))?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Tilewright frame encoder"),
            });
        encode_scene(
            &mut encoder,
            &self.pipelines,
            &target.color_view,
            &target.depth_view,
            &vertex_buffer,
            &prepared,
            scene.clear_color(),
        )?;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &target.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(target.padded_row),
                    rows_per_image: Some(self.target.height),
                },
            },
            target.size,
        );
        self.queue.submit(Some(encoder.finish()));
        let slice = target.readback.slice(..);
        let (sender, receiver) = mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ignored = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|error| Error::Internal(error.to_string()))?;
        receiver
            .recv()
            .map_err(|error| Error::Internal(error.to_string()))?
            .map_err(|error| Error::Internal(error.to_string()))?;
        let mapped = slice
            .get_mapped_range()
            .map_err(|error| Error::Internal(error.to_string()))?;
        let capacity = usize::try_from(target.unpadded_row)
            .ok()
            .and_then(|row| row.checked_mul(self.target.height as usize))
            .ok_or(Error::LimitExceeded)?;
        let mut rgba = Vec::with_capacity(capacity);
        for row in mapped.chunks_exact(target.padded_row as usize) {
            rgba.extend_from_slice(&row[..target.unpadded_row as usize]);
        }
        drop(mapped);
        target.readback.unmap();
        let planes = semantic_planes(scene, &prepared)?;
        let uploaded_bytes = prepared
            .iter()
            .try_fold(0_u64, |total, sprite| {
                total.checked_add(sprite.uploaded_bytes)
            })
            .ok_or(Error::LimitExceeded)?;
        let pixel_count = u64::from(self.target.width) * u64::from(self.target.height);
        Ok((
            rgba,
            planes,
            FrameWork {
                resource_requests: prepared
                    .iter()
                    .filter(|sprite| sprite.uploaded_bytes != 0)
                    .count() as u64,
                uploaded_bytes,
                vertex_count: vertices.len() as u64,
                vertex_allocation_bytes,
                target_allocation_bytes: if allocated {
                    pixel_count.checked_mul(12).ok_or(Error::LimitExceeded)?
                } else {
                    0
                },
                readback_bytes: pixel_count.checked_mul(4).ok_or(Error::LimitExceeded)?,
            },
        ))
    }

    /// Rejects a scene whose vertex stream the device cannot hold, before the
    /// stream is generated.
    fn check_vertex_stream(&self, scene: &SceneBundle) -> Result<(), Error> {
        let required = u64::try_from(scene.quads().len())
            .ok()
            .and_then(|quads| quads.checked_mul(6 * std::mem::size_of::<Vertex>() as u64))
            .ok_or(Error::LimitExceeded)?;
        vertex_buffer_capacity(required, self.device.limits().max_buffer_size).map(|_| ())
    }

    fn ensure_vertex_buffer(&mut self, vertices: &[Vertex]) -> Result<(wgpu::Buffer, u64), Error> {
        let contents = bytemuck::cast_slice(vertices);
        let required = u64::try_from(contents.len().max(4)).map_err(|_| Error::LimitExceeded)?;
        let allocated = if self
            .vertices
            .as_ref()
            .is_none_or(|cache| cache.capacity < required)
        {
            let capacity = vertex_buffer_capacity(required, self.device.limits().max_buffer_size)?;
            self.vertices = Some(VertexCache {
                buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Tilewright shared frame vertices"),
                    size: capacity,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                capacity,
            });
            capacity
        } else {
            0
        };
        let cache = self
            .vertices
            .as_ref()
            .ok_or_else(|| Error::Internal("vertex cache insertion failed".to_owned()))?;
        if !contents.is_empty() {
            self.queue.write_buffer(&cache.buffer, 0, contents);
        }
        Ok((cache.buffer.clone(), allocated))
    }

    fn prepare_sprites(
        &mut self,
        scene: &SceneBundle,
        resources: &dyn ResourceProvider,
    ) -> Result<Vec<PreparedSprite>, Error> {
        let key_of = |quad: &Quad| {
            (
                quad.resource_id,
                quad.resource_revision,
                quad.resource_digest,
            )
        };
        let distinct = scene.quads().iter().map(key_of).collect::<HashSet<_>>();
        if distinct.len() > MAXIMUM_CACHED_TEXTURES {
            return Err(Error::LimitExceeded);
        }
        // Frames must fit their textures before the cache changes.
        self.check_frames_before_loading(scene, resources)?;
        // Decode every missing texture before uploading any, so the scene's
        // whole working set is known and bounded first.
        let mut working_set_bytes = 0_u64;
        for key in &distinct {
            if let Some(cached) = self.textures.get(key) {
                working_set_bytes =
                    add_working_set_bytes(working_set_bytes, cached.image.pixels().len() as u64)
                        .map_err(|_| Error::LimitExceeded)?;
            }
        }
        let mut decoded = Vec::new();
        let mut requested = HashSet::new();
        let mut staged_bytes = 0_u64;
        // Bytes held by cached textures this scene does not use: the most an
        // eviction can free.
        let mut unused_bytes = self
            .textures
            .iter()
            .filter(|(key, _)| !distinct.contains(*key))
            .map(|(_, cached)| cached.image.pixels().len() as u64)
            .sum::<u64>();
        for quad in scene.quads() {
            let key = key_of(quad);
            if self.textures.contains_key(&key) || !requested.insert(key) {
                continue;
            }
            let blob = load_blob(key, resources)?;
            // Cached and staged textures share one CPU budget. A texture that
            // would not fit beside them is decoded only after evicting cached
            // textures this scene does not use, and not at all if even that
            // leaves no room. An envelope holds a 12-byte header before its
            // pixels.
            let incoming = (blob.bytes().len() as u64).saturating_sub(12);
            match decode_admission(self.texture_bytes, unused_bytes, staged_bytes, incoming) {
                Admission::Fits => {}
                Admission::EvictUnused => {
                    self.evict_unused(&distinct);
                    unused_bytes = 0;
                }
                Admission::Exceeds => return Err(Error::LimitExceeded),
            }
            let image = Arc::new(
                Rgba8Image::decode(
                    &blob,
                    self.limits.maximum_dimension,
                    self.limits.maximum_pixels,
                )
                .map_err(|error| Error::Resource(error.to_string()))?,
            );
            let bytes = image.pixels().len() as u64;
            working_set_bytes = add_working_set_bytes(working_set_bytes, bytes)
                .map_err(|_| Error::LimitExceeded)?;
            staged_bytes += bytes;
            decoded.push((key, image));
        }
        let frames = self.resolve_frames(scene, &decoded)?;
        // The entry bound works the same way.
        if texture_cache_must_evict(
            self.textures.len(),
            self.texture_bytes,
            decoded.len(),
            staged_bytes,
        ) {
            self.evict_unused(&distinct);
        }
        let mut uploads = HashMap::with_capacity(decoded.len());
        for (key, image) in decoded {
            uploads.insert(key, self.upload_texture(key, image)?);
        }
        self.prepared_sprites(scene, frames, uploads)
    }

    /// Pairs each quad with its cached texture binding, frame, and the upload
    /// its texture caused, which the first quad naming it carries.
    fn prepared_sprites(
        &self,
        scene: &SceneBundle,
        frames: Vec<TextureRect>,
        mut uploads: HashMap<TextureKey, u64>,
    ) -> Result<Vec<PreparedSprite>, Error> {
        let mut prepared = Vec::with_capacity(scene.quads().len());
        for (quad, frame) in scene.quads().iter().zip(frames) {
            let key = (
                quad.resource_id,
                quad.resource_revision,
                quad.resource_digest,
            );
            let cached = self
                .textures
                .get(&key)
                .ok_or_else(|| Error::Internal("texture cache insertion failed".to_owned()))?;
            let bind_group = match quad.sampling {
                Sampling::Nearest => cached.nearest.clone(),
                Sampling::Linear => cached.linear.clone(),
            };
            prepared.push(PreparedSprite {
                bind_group,
                image: cached.image.clone(),
                // The first quad naming a newly loaded texture carries its upload.
                uploaded_bytes: uploads.remove(&key).unwrap_or(0),
                key,
                sampling: quad.sampling,
                frame,
                clamped: frame != TextureRect::whole(cached.image.width(), cached.image.height()),
            });
        }
        Ok(prepared)
    }

    /// Checks that every frame lies inside its texture before any texture
    /// is admitted or evicted: cached textures by their decoded size, and
    /// missing textures with frames by their envelope headers. A missing
    /// framed texture is therefore loaded once here and again to decode it;
    /// keeping its bytes instead would hold every new texture beside the
    /// cache before eviction could make room.
    fn check_frames_before_loading(
        &self,
        scene: &SceneBundle,
        resources: &dyn ResourceProvider,
    ) -> Result<(), Error> {
        for (key, extent) in frame_extents(scene) {
            let (width, height) = if let Some(cached) = self.textures.get(&key) {
                (cached.image.width(), cached.image.height())
            } else {
                Rgba8Image::envelope_dimensions(
                    &load_blob(key, resources)?,
                    self.limits.maximum_dimension,
                    self.limits.maximum_pixels,
                )
                .map_err(|error| Error::Resource(error.to_string()))?
            };
            check_frame_extent(extent, width, height)?;
        }
        Ok(())
    }

    /// Resolves every quad's frame against its cached or newly decoded
    /// texture. Frames were checked against their textures while loading.
    fn resolve_frames(
        &self,
        scene: &SceneBundle,
        decoded: &[(TextureKey, Arc<Rgba8Image>)],
    ) -> Result<Vec<TextureRect>, Error> {
        let sizes = decoded
            .iter()
            .map(|(key, image)| (*key, (image.width(), image.height())))
            .collect::<HashMap<_, _>>();
        scene
            .quads()
            .iter()
            .map(|quad| {
                let key = (
                    quad.resource_id,
                    quad.resource_revision,
                    quad.resource_digest,
                );
                let (width, height) = match (sizes.get(&key), self.textures.get(&key)) {
                    (Some(size), _) => *size,
                    (None, Some(cached)) => (cached.image.width(), cached.image.height()),
                    (None, None) => {
                        return Err(Error::Internal("texture missing after loading".to_owned()));
                    }
                };
                frame_in_image(quad.frame, width, height)
                    .map_err(|error| Error::Resource(error.to_string()))
            })
            .collect()
    }

    /// Drops cached textures that `used` does not name.
    fn evict_unused(&mut self, used: &HashSet<TextureKey>) {
        self.textures.retain(|key, _| used.contains(key));
        self.texture_bytes = self
            .textures
            .values()
            .map(|cached| cached.image.pixels().len() as u64)
            .sum();
    }

    fn upload_texture(&mut self, key: TextureKey, image: Arc<Rgba8Image>) -> Result<u64, Error> {
        let size = wgpu::Extent3d {
            width: image.width(),
            height: image.height(),
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Tilewright sprite texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            image.pixels(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width() * 4),
                rows_per_image: Some(image.height()),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let [nearest, linear] = &self.samplers;
        let nearest = create_texture_binding(&self.device, &self.texture_layout, &view, nearest);
        let linear = create_texture_binding(&self.device, &self.texture_layout, &view, linear);
        let uploaded_bytes = image.pixels().len() as u64;
        self.texture_bytes = self
            .texture_bytes
            .checked_add(uploaded_bytes)
            .ok_or(Error::LimitExceeded)?;
        self.textures.insert(
            key,
            TextureCacheEntry {
                image,
                nearest,
                linear,
            },
        );
        Ok(uploaded_bytes)
    }
}

impl Renderer for WgpuRenderer {
    fn target(&self) -> TargetDescriptor {
        self.target
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), Error> {
        self.target = TargetDescriptor {
            width,
            height,
            ..self.target
        }
        .validate(self.limits)?;
        self.metrics.surface_reconfigurations += 1;
        self.offscreen = None;
        self.surface_depth = None;
        Ok(())
    }

    fn render(&mut self, request: RenderRequest<'_>) -> Result<FrameOutput, Error> {
        if request.scene.viewport().width != self.target.width
            || request.scene.viewport().height != self.target.height
        {
            return Err(Error::InvalidTarget);
        }
        let started = Instant::now();
        let (rgba8, (semantic_ids, depth, coverage), work) =
            self.render_frame(request.scene, request.resources.as_ref())?;
        self.metrics.frames_submitted += 1;
        self.metrics.resource_requests = work.resource_requests;
        self.metrics.uploaded_bytes = work.uploaded_bytes;
        self.metrics.vertex_count = work.vertex_count;
        self.metrics.vertex_allocation_bytes = work.vertex_allocation_bytes;
        self.metrics.target_allocation_bytes = work.target_allocation_bytes;
        self.metrics.readback_bytes = work.readback_bytes;
        self.metrics.frame_cpu_micros = duration_micros(started.elapsed());
        let output = FrameOutput {
            width: self.target.width,
            height: self.target.height,
            rgba8,
            semantic_ids,
            depth,
            coverage,
            metrics: self.metrics,
        };
        output.validate()?;
        Ok(output)
    }
}

type GpuInitialization = (
    wgpu::Instance,
    wgpu::Adapter,
    wgpu::Device,
    wgpu::Queue,
    AdapterDescription,
);

fn initialize_gpu(
    preference: BackendPreference,
    startup: Instant,
) -> Result<GpuInitialization, Error> {
    let backends = match preference {
        BackendPreference::Automatic => wgpu::Backends::VULKAN | wgpu::Backends::DX12,
        BackendPreference::Vulkan => wgpu::Backends::VULKAN,
        BackendPreference::Direct3D12 => wgpu::Backends::DX12,
    };
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
    descriptor.backends &= backends;
    let instance = wgpu::Instance::new(descriptor);
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .map_err(|error| {
        Error::BackendUnavailable(format!(
            "no adapter for requested {preference:?} backend policy: {error}"
        ))
    })?;
    // Downlevel limits keep the device portable; only the texture dimension is
    // raised to what the adapter offers, up to the API's own target bound.
    let required_limits = wgpu::Limits {
        max_texture_dimension_2d: adapter
            .limits()
            .max_texture_dimension_2d
            .min(TargetLimits::default().maximum_dimension),
        ..wgpu::Limits::downlevel_defaults()
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("Tilewright renderer device"),
        required_features: wgpu::Features::empty(),
        required_limits,
        memory_hints: wgpu::MemoryHints::Performance,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        trace: wgpu::Trace::Off,
    }))
    .map_err(|error| Error::Internal(error.to_string()))?;
    let info = adapter.get_info();
    let description = AdapterDescription {
        name: info.name,
        driver: info.driver,
        backend: format!("{:?}", info.backend),
        device_type: format!("{:?}", info.device_type),
        maximum_texture_dimension_2d: adapter.limits().max_texture_dimension_2d,
        maximum_buffer_size: adapter.limits().max_buffer_size,
        startup_micros: duration_micros(startup.elapsed()),
    };
    Ok((instance, adapter, device, queue, description))
}

fn create_texture_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Tilewright sprite texture layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

fn create_sampler(device: &wgpu::Device, filter: wgpu::FilterMode) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Tilewright sprite sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    })
}

/// One texture with one of the shared samplers.
fn create_texture_binding(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Tilewright sprite texture binding"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn target_extent(target: TargetDescriptor) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: target.width,
        height: target.height,
        depth_or_array_layers: 1,
    }
}

fn create_depth_target(device: &wgpu::Device, size: wgpu::Extent3d, label: &str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth24PlusStencil8,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

/// The scene pipelines for one target format: one for quads sampling a
/// whole texture and one that clamps samples inside a smaller frame.
#[derive(Clone)]
struct Pipelines {
    whole: wgpu::RenderPipeline,
    framed: wgpu::RenderPipeline,
}

impl Pipelines {
    const fn get(&self, clamped: bool) -> &wgpu::RenderPipeline {
        if clamped { &self.framed } else { &self.whole }
    }
}

fn create_pipelines(
    device: &wgpu::Device,
    texture_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> Pipelines {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Tilewright sprite shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("shader.wgsl"))),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Tilewright sprite pipeline layout"),
        bind_group_layouts: &[Some(texture_layout)],
        immediate_size: 0,
    });
    Pipelines {
        whole: create_pipeline(device, &shader, &layout, format, false),
        framed: create_pipeline(device, &shader, &layout, format, true),
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    clamp_to_frame: bool,
) -> wgpu::RenderPipeline {
    let constants = [("CLAMP_TO_FRAME", if clamp_to_frame { 1.0 } else { 0.0 })];
    let compilation_options = wgpu::PipelineCompilationOptions {
        constants: &constants,
        ..wgpu::PipelineCompilationOptions::default()
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Tilewright sprite pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex_main"),
            compilation_options: compilation_options.clone(),
            buffers: &[Some(Vertex::layout())],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fragment_main"),
            compilation_options,
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth24PlusStencil8,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn encode_scene(
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &Pipelines,
    color_view: &wgpu::TextureView,
    depth_view: &wgpu::TextureView,
    vertex_buffer: &wgpu::Buffer,
    sprites: &[PreparedSprite],
    clear: [f32; 4],
) -> Result<(), Error> {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("Tilewright scene pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: f64::from(clear[0]),
                    g: f64::from(clear[1]),
                    b: f64::from(clear[2]),
                    a: f64::from(clear[3]),
                }),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth_view,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(0.0),
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(0),
                store: wgpu::StoreOp::Store,
            }),
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_vertex_buffer(0, vertex_buffer.slice(..));
    // Consecutive quads that sample the same texture the same way share one
    // draw call; order is unchanged.
    let mut current = None;
    let mut start = 0_usize;
    while start < sprites.len() {
        let mut end = start + 1;
        while end < sprites.len()
            && sprites[end].key == sprites[start].key
            && sprites[end].sampling == sprites[start].sampling
            && sprites[end].clamped == sprites[start].clamped
        {
            end += 1;
        }
        if current != Some(sprites[start].clamped) {
            pass.set_pipeline(pipelines.get(sprites[start].clamped));
            current = Some(sprites[start].clamped);
        }
        let first = u32::try_from(start)
            .ok()
            .and_then(|value| value.checked_mul(6))
            .ok_or(Error::LimitExceeded)?;
        let last = u32::try_from(end)
            .ok()
            .and_then(|value| value.checked_mul(6))
            .ok_or(Error::LimitExceeded)?;
        pass.set_bind_group(0, &sprites[start].bind_group, &[]);
        pass.draw(first..last, 0..1);
        start = end;
    }
    Ok(())
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    color: [f32; 4],
    uv: [f32; 2],
    effect: f32,
    grayscale: f32,
    glow_color: [f32; 4],
    glow_radius: [f32; 2],
    /// [`FLAG_GLOW`].
    flags: u32,
    /// The sampled frame's texel x, y, width and height.
    frame: [u16; 4],
    /// The texture's width and height in texels.
    texture_size: [u16; 2],
}

/// The quad draws only its glow halo.
const FLAG_GLOW: u32 = 1;

impl Vertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 10] = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x4, 2 => Float32x2, 3 => Float32,
        4 => Float32, 5 => Float32x4, 6 => Float32x2, 7 => Uint32,
        8 => Uint16x4, 9 => Uint16x2
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

struct PreparedSprite {
    bind_group: wgpu::BindGroup,
    image: Arc<Rgba8Image>,
    uploaded_bytes: u64,
    key: TextureKey,
    sampling: Sampling,
    /// The quad's frame, checked against its texture.
    frame: TextureRect,
    /// The frame is smaller than its texture, so samples are clamped inside it.
    clamped: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct FrameWork {
    resource_requests: u64,
    uploaded_bytes: u64,
    vertex_count: u64,
    vertex_allocation_bytes: u64,
    target_allocation_bytes: u64,
    readback_bytes: u64,
}

fn duration_micros(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

#[allow(clippy::cast_precision_loss)]
fn vertices(scene: &SceneBundle, prepared: &[PreparedSprite]) -> Result<Vec<Vertex>, Error> {
    let viewport = scene.viewport();
    let mut output = Vec::with_capacity(scene.quads().len().saturating_mul(6));
    for (quad, prepared) in scene.quads().iter().zip(prepared) {
        let z = depth_value(quad);
        let effect = effect_amount(quad, scene.clock_millis());
        let (glow_color, glow_radius, flags) = quad.glow.map_or(([0.0; 4], [0.0; 2], 0), |glow| {
            (glow.color, glow.radius_uv, FLAG_GLOW)
        });
        // Frames lie inside textures no larger than the device's 2D limit,
        // which never exceeds 16 bits.
        let frame = [
            prepared.frame.x,
            prepared.frame.y,
            prepared.frame.width,
            prepared.frame.height,
            prepared.image.width(),
            prepared.image.height(),
        ]
        .map(u16::try_from);
        let [
            Ok(left),
            Ok(top),
            Ok(width),
            Ok(height),
            Ok(texture_width),
            Ok(texture_height),
        ] = frame
        else {
            return Err(Error::LimitExceeded);
        };
        let frame = [left, top, width, height];
        let texture_size = [texture_width, texture_height];
        let vertex = |corner: usize| {
            let [x, y] = quad.positions[corner].map(subpixel_position);
            Vertex {
                position: [
                    x.mul_add(2.0 / viewport.width as f32, -1.0),
                    1.0 - y * 2.0 / viewport.height as f32,
                    z,
                ],
                color: quad.color,
                uv: quad.uvs[corner],
                effect,
                grayscale: quad.grayscale,
                glow_color,
                glow_radius,
                flags,
                frame,
                texture_size,
            }
        };
        output.extend_from_slice(&[
            vertex(0),
            vertex(1),
            vertex(2),
            vertex(0),
            vertex(2),
            vertex(3),
        ]);
    }
    Ok(output)
}

/// The furthest right and bottom texel edge any frame reaches in each
/// texture a scene names, in the order the scene first names them.
fn frame_extents(scene: &SceneBundle) -> Vec<(TextureKey, (u64, u64))> {
    let mut extents = Vec::<(TextureKey, (u64, u64))>::new();
    let mut positions = HashMap::<TextureKey, usize>::new();
    for quad in scene.quads() {
        let Some(frame) = quad.frame else {
            continue;
        };
        let key = (
            quad.resource_id,
            quad.resource_revision,
            quad.resource_digest,
        );
        let position = *positions.entry(key).or_insert_with(|| {
            extents.push((key, (0, 0)));
            extents.len() - 1
        });
        let extent = &mut extents[position].1;
        extent.0 = extent.0.max(u64::from(frame.x) + u64::from(frame.width));
        extent.1 = extent.1.max(u64::from(frame.y) + u64::from(frame.height));
    }
    extents
}

fn check_frame_extent((right, bottom): (u64, u64), width: u32, height: u32) -> Result<(), Error> {
    if right > u64::from(width) || bottom > u64::from(height) {
        return Err(Error::Resource(
            tilewright_render_resources::Error::FrameOutOfBounds.to_string(),
        ));
    }
    Ok(())
}

/// Whether decoding `incoming` more bytes beside `cached` and `staged` ones
/// would pass the CPU texture budget.
fn decoding_needs_room(cached: u64, staged: u64, incoming: u64) -> bool {
    cached.saturating_add(staged).saturating_add(incoming)
        > DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES
}

#[derive(Debug, Eq, PartialEq)]
enum Admission {
    Fits,
    EvictUnused,
    Exceeds,
}

/// Whether a texture of `incoming` decoded bytes fits beside the `cached`
/// and `staged` ones, fits once the `unused` cached bytes are evicted, or
/// cannot fit at all.
fn decode_admission(cached: u64, unused: u64, staged: u64, incoming: u64) -> Admission {
    if !decoding_needs_room(cached, staged, incoming) {
        Admission::Fits
    } else if unused > 0 && !decoding_needs_room(cached.saturating_sub(unused), staged, incoming) {
        Admission::EvictUnused
    } else {
        Admission::Exceeds
    }
}

fn load_blob(key: TextureKey, resources: &dyn ResourceProvider) -> Result<ResourceBlob, Error> {
    resources
        .load(ResourceRequest {
            id: key.0,
            revision: key.1,
            digest: key.2,
            maximum_bytes: DEFAULT_MAXIMUM_RESOURCE_BYTES,
        })
        .map_err(|error| Error::Resource(error.to_string()))
}

/// Bytes to allocate for a vertex stream of `required` bytes: the next power
/// of two, capped at the device's largest buffer (kept 4-byte aligned).
/// Streams larger than that buffer are rejected.
fn vertex_buffer_capacity(required: u64, maximum: u64) -> Result<u64, Error> {
    let maximum = maximum & !3;
    if required > maximum {
        return Err(Error::LimitExceeded);
    }
    Ok(required
        .checked_next_power_of_two()
        .map_or(maximum, |capacity| capacity.min(maximum)))
}

/// Whether admitting `missing` textures of `missing_bytes` to a cache that
/// holds `cached` textures of `cached_bytes` would exceed its entry or byte
/// bound, so textures the next scene does not use must be evicted first.
fn texture_cache_must_evict(
    cached: usize,
    cached_bytes: u64,
    missing: usize,
    missing_bytes: u64,
) -> bool {
    cached.saturating_add(missing) > MAXIMUM_CACHED_TEXTURES
        || cached_bytes.saturating_add(missing_bytes) > DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES
}

fn depth_value(quad: &Quad) -> f32 {
    let layer = match quad.layer {
        Layer::World => 0.0,
        Layer::Effect => 1.0,
        Layer::Interface => 2.0,
    };
    let depth = (f32::from(quad.depth) - f32::from(i16::MIN)) / f32::from(u16::MAX);
    (layer + depth) / 3.0
}

#[allow(clippy::cast_precision_loss)]
fn effect_amount(quad: &Quad, clock_millis: u64) -> f32 {
    if quad.layer == Layer::Effect {
        (clock_millis % 1_000) as f32 / 1_000.0 * 0.15
    } else {
        0.0
    }
}

type SemanticPlanes = (Vec<u32>, Vec<i16>, Vec<u8>);

fn semantic_planes(
    scene: &SceneBundle,
    prepared: &[PreparedSprite],
) -> Result<SemanticPlanes, Error> {
    let viewport = scene.viewport();
    let pixels = (viewport.width as usize)
        .checked_mul(viewport.height as usize)
        .ok_or(Error::LimitExceeded)?;
    let mut semantic = vec![0; pixels];
    let mut depth = vec![i16::MIN; pixels];
    let mut coverage = vec![0; pixels];
    for (quad, prepared) in scene.quads().iter().zip(prepared) {
        // Scene validation rejects pickable glow quads, so `pickable` alone
        // selects the quads that mark these planes, as in the reference.
        if !quad.pickable {
            continue;
        }
        mark_quad(
            viewport.width,
            viewport.height,
            quad,
            &mut semantic,
            &mut depth,
            &mut coverage,
            &prepared.image,
            prepared.frame,
        );
    }
    Ok((semantic, depth, coverage))
}

#[allow(clippy::too_many_arguments)]
fn mark_quad(
    width: u32,
    height: u32,
    quad: &Quad,
    semantic: &mut [u32],
    depth: &mut [i16],
    coverage: &mut [u8],
    image: &Rgba8Image,
    frame: TextureRect,
) {
    for_each_quad_pixel(quad, width, height, |_x, _y, index, u, v| {
        let alpha = shade_quad_rgba8(
            sample_quad_rgba8(image, frame, u, v, quad.sampling),
            quad,
            0,
        )[3];
        if alpha != 0 {
            semantic[index] = quad.semantic_id.0;
            depth[index] = quad.depth;
            coverage[index] = alpha;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::WgpuRenderer;
    use tilewright_render_api::{RenderRequest, Renderer};
    use tilewright_render_testkit::{
        ReferenceRenderer, atlas_provider, atlas_scene, iso_world_scene, offscreen_target,
        synthetic_provider, synthetic_scene, top_down_world_scene, world_provider,
    };

    /// Renders a scene on the GPU and the reference, requiring exact
    /// semantic, depth and coverage planes and RGBA within one step.
    fn assert_matches_reference(
        scene: &tilewright_scene::SceneBundle,
        resources: &std::sync::Arc<tilewright_render_testkit::FakeResourceProvider>,
    ) -> Option<tilewright_render_api::FrameOutput> {
        let viewport = scene.viewport();
        let target = offscreen_target(viewport.width, viewport.height);
        let Ok(mut renderer) = WgpuRenderer::new(target) else {
            eprintln!("skipping GPU comparison: no supported adapter");
            return None;
        };
        let request = RenderRequest {
            scene,
            resources: resources.clone(),
        };
        let frame = renderer.render(request.clone()).unwrap();
        let reference = ReferenceRenderer::new(target)
            .unwrap()
            .render(request)
            .unwrap();
        assert_eq!(frame.semantic_ids, reference.semantic_ids);
        assert_eq!(frame.depth, reference.depth);
        assert_eq!(frame.coverage, reference.coverage);
        let (index, difference) = frame
            .rgba8
            .iter()
            .zip(&reference.rgba8)
            .map(|(actual, expected)| actual.abs_diff(*expected))
            .enumerate()
            .max_by_key(|(_, difference)| *difference)
            .unwrap_or((0, 0));
        assert!(
            difference <= 1,
            "{}x{} scene differs from the reference by {difference} at byte {index}: GPU={}, reference={}",
            viewport.width,
            viewport.height,
            frame.rgba8[index],
            reference.rgba8[index],
        );
        Some(frame)
    }

    #[test]
    fn atlas_frames_match_the_reference_when_a_gpu_adapter_is_available() {
        let scene = atlas_scene(112, 64).unwrap();
        let Some(frame) = assert_matches_reference(&scene, &atlas_provider().unwrap()) else {
            return;
        };
        // One atlas texture is requested once for every layer and tile.
        assert_eq!(frame.metrics.resource_requests, 1);
        assert_eq!(frame.metrics.uploaded_bytes, 64 * 64 * 4);
    }

    #[test]
    fn frames_outside_their_texture_fail_before_any_upload() {
        use tilewright_scene::{SceneBundle, SceneLimits, TextureRect};
        let Ok(mut renderer) = WgpuRenderer::new(offscreen_target(112, 64)) else {
            eprintln!("skipping GPU frame test: no supported adapter");
            return;
        };
        let scene = atlas_scene(112, 64).unwrap();
        let mut quads = scene.quads().to_vec();
        quads[3].frame = Some(TextureRect {
            x: 63,
            y: 63,
            width: 2,
            height: 1,
        });
        let outside = SceneBundle::from_quads(
            scene.revision(),
            scene.clock_millis(),
            scene.viewport(),
            scene.clear_color(),
            quads,
            SceneLimits::default(),
        )
        .unwrap();
        let error = renderer
            .render(RenderRequest {
                scene: &outside,
                resources: atlas_provider().unwrap(),
            })
            .unwrap_err();
        assert!(
            matches!(&error, tilewright_render_api::Error::Resource(message) if message.contains("FrameOutOfBounds")),
            "{error:?}"
        );
        assert!(renderer.textures.is_empty());
        // The valid scene still renders on the same renderer.
        let frame = renderer
            .render(RenderRequest {
                scene: &scene,
                resources: atlas_provider().unwrap(),
            })
            .unwrap();
        assert_eq!(frame.metrics.resource_requests, 1);
    }

    #[test]
    fn oversized_vertex_streams_fail_before_loading_resources() {
        use tilewright_render_resources::{
            Error as ResourceError, ResourceBlob, ResourceProvider, ResourceRequest,
        };
        use tilewright_scene::{
            Digest256, Layer, Quad, ResourceId, Sampling, SceneBundle, SceneLimits, SemanticId,
            Viewport,
        };
        struct Refusing;
        impl ResourceProvider for Refusing {
            fn load(&self, _request: ResourceRequest) -> Result<ResourceBlob, ResourceError> {
                Err(ResourceError::NotFound)
            }
        }
        let Ok(mut renderer) = WgpuRenderer::new(offscreen_target(4, 4)) else {
            eprintln!("skipping GPU vertex limit test: no supported adapter");
            return;
        };
        // One quad more than a 256 MiB buffer of 504-byte quads holds.
        let count = (256_u32 << 20) / 504 + 1;
        let quads = (0..count).map(|index| Quad {
            semantic_id: SemanticId(index + 1),
            part: 0,
            resource_id: ResourceId(1),
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
        });
        let viewport = Viewport {
            width: 4,
            height: 4,
            scale_milli: 1_000,
        };
        let limits = SceneLimits {
            maximum_quads: usize::MAX,
            maximum_raster_samples: u64::MAX,
            ..SceneLimits::default()
        };
        let scene = SceneBundle::from_quads(1, 0, viewport, [0.0; 4], quads, limits).unwrap();
        // The refusing provider would fail any load; the limit comes first.
        let result = renderer.render(RenderRequest {
            scene: &scene,
            resources: std::sync::Arc::new(Refusing),
        });
        assert_eq!(
            result.err(),
            Some(tilewright_render_api::Error::LimitExceeded)
        );
    }

    #[test]
    fn vertex_buffers_stay_within_the_device_buffer_limit() {
        use super::vertex_buffer_capacity;
        let limit = 256 << 20;
        // 600,000 quads of six 84-byte vertices need about 288 MiB.
        assert!(vertex_buffer_capacity(600_000 * 504, limit).is_err());
        // Rounding up to a power of two never passes the device limit.
        assert_eq!(vertex_buffer_capacity(150 << 20, limit), Ok(limit));
        assert_eq!(vertex_buffer_capacity(100 << 20, limit), Ok(128 << 20));
        assert_eq!(vertex_buffer_capacity(504, limit), Ok(512));
        assert_eq!(vertex_buffer_capacity(limit, limit + 3), Ok(limit));
    }

    #[test]
    fn texture_cache_evicts_before_exceeding_either_bound() {
        use super::{
            DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES as BYTES, MAXIMUM_CACHED_TEXTURES as CAP,
            texture_cache_must_evict,
        };
        assert!(!texture_cache_must_evict(10, 1 << 20, 10, 1 << 20));
        assert!(!texture_cache_must_evict(CAP - 1, 0, 1, 0));
        assert!(texture_cache_must_evict(CAP, 0, 1, 0));
        assert!(!texture_cache_must_evict(1, BYTES - 1, 1, 1));
        assert!(texture_cache_must_evict(1, BYTES - 1, 1, 2));
        assert!(texture_cache_must_evict(1, u64::MAX, 1, 1));
    }

    #[test]
    fn staged_decodes_share_the_budget_with_the_cache() {
        use super::{
            Admission, DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES as BYTES, decode_admission,
            decoding_needs_room,
        };
        // A nearly full cache must make room before a disjoint scene decodes
        // its own nearly full working set.
        assert!(decoding_needs_room(BYTES - 1, 0, 16));
        assert!(decoding_needs_room(BYTES / 2, BYTES / 2, 1));
        assert!(!decoding_needs_room(BYTES / 2, BYTES / 4, BYTES / 4));
        assert!(decoding_needs_room(u64::MAX, 1, 1));
        assert_eq!(decode_admission(BYTES / 2, 0, 0, 16), Admission::Fits);
        assert_eq!(
            decode_admission(BYTES - 8, BYTES - 8, 0, 16),
            Admission::EvictUnused
        );
        // When the scene reuses every cached texture, eviction frees nothing
        // and the texture is refused before it is decoded.
        assert_eq!(decode_admission(BYTES - 8, 0, 0, 16), Admission::Exceeds);
        assert_eq!(decode_admission(BYTES - 8, 4, 0, 16), Admission::Exceeds);
    }

    #[test]
    fn scenes_reuse_and_evict_cached_textures_by_working_set() {
        let Ok(mut renderer) = WgpuRenderer::new(offscreen_target(16, 16)) else {
            eprintln!("skipping GPU cache test: no supported adapter");
            return;
        };
        let scene = synthetic_scene(16, 16).unwrap();
        let render = |renderer: &mut WgpuRenderer| {
            renderer
                .render(RenderRequest {
                    scene: &scene,
                    resources: synthetic_provider().unwrap(),
                })
                .unwrap()
                .metrics
        };
        assert_eq!(render(&mut renderer).resource_requests, 1);
        assert_eq!(render(&mut renderer).resource_requests, 0);
        // A full cache of unused textures is evicted, but the scene's own
        // texture stays cached and is not requested again.
        let unused = renderer.textures.values().next().map(|entry| {
            (
                entry.image.clone(),
                entry.nearest.clone(),
                entry.linear.clone(),
            )
        });
        let (image, nearest, linear) = unused.unwrap();
        for index in 0..super::MAXIMUM_CACHED_TEXTURES as u64 {
            renderer.textures.insert(
                (
                    tilewright_scene::ResourceId(1_000 + index),
                    1,
                    tilewright_scene::Digest256([9; 32]),
                ),
                super::TextureCacheEntry {
                    image: image.clone(),
                    nearest: nearest.clone(),
                    linear: linear.clone(),
                },
            );
        }
        assert_eq!(render(&mut renderer).resource_requests, 0);
        assert!(renderer.textures.len() <= super::MAXIMUM_CACHED_TEXTURES);
    }

    #[test]
    fn vertex_layout_matches_the_reference_accounting() {
        assert_eq!(
            std::mem::size_of::<super::Vertex>() as u64,
            tilewright_render_testkit::VERTEX_BYTES
        );
    }

    #[test]
    fn renders_offscreen_when_a_gpu_adapter_is_available() {
        let Ok(mut renderer) = WgpuRenderer::new(offscreen_target(16, 16)) else {
            eprintln!("skipping GPU smoke test: no supported adapter");
            return;
        };
        let scene = synthetic_scene(16, 16).unwrap();
        let request = RenderRequest {
            scene: &scene,
            resources: synthetic_provider().unwrap(),
        };
        let frame = renderer.render(request.clone()).unwrap();
        let reference = ReferenceRenderer::new(offscreen_target(16, 16))
            .unwrap()
            .render(request)
            .unwrap();
        assert_eq!(frame.rgba8.len(), 16 * 16 * 4);
        assert!(frame.rgba8.windows(4).any(|pixel| pixel != [5, 8, 13, 255]));
        assert_eq!(frame.semantic_ids, reference.semantic_ids);
        assert_eq!(frame.depth, reference.depth);
        assert_eq!(frame.coverage, reference.coverage);
        let (maximum_index, maximum_channel_difference) = frame
            .rgba8
            .iter()
            .zip(reference.rgba8.iter())
            .enumerate()
            .map(|(index, (actual, expected))| (index, actual.abs_diff(*expected)))
            .max_by_key(|(_, difference)| *difference)
            .unwrap();
        assert!(
            maximum_channel_difference <= 1,
            "maximum RGBA difference: {maximum_channel_difference} at byte {maximum_index}; GPU={}, reference={}",
            frame.rgba8[maximum_index],
            reference.rgba8[maximum_index],
        );
        assert_eq!(frame.metrics.frames_submitted, 1);
        assert_eq!(frame.metrics.resource_requests, 1);
        assert_eq!(frame.metrics.uploaded_bytes, 16);
        assert_eq!(frame.metrics.vertex_count, 18);
        assert_eq!(frame.metrics.vertex_allocation_bytes, 2_048);
        assert_eq!(frame.metrics.target_allocation_bytes, 16 * 16 * 12);

        let reused = renderer
            .render(RenderRequest {
                scene: &scene,
                resources: synthetic_provider().unwrap(),
            })
            .unwrap();
        assert_eq!(reused.metrics.frames_submitted, 2);
        assert_eq!(reused.metrics.target_allocation_bytes, 0);
        assert_eq!(reused.metrics.vertex_allocation_bytes, 0);
        assert_eq!(reused.metrics.resource_requests, 0);
        assert_eq!(reused.metrics.uploaded_bytes, 0);

        renderer.resize(31, 17).unwrap();
        let resized_scene = synthetic_scene(31, 17).unwrap();
        let resized = renderer
            .render(RenderRequest {
                scene: &resized_scene,
                resources: synthetic_provider().unwrap(),
            })
            .unwrap();
        assert_eq!(resized.metrics.frames_submitted, 3);
        assert_eq!(resized.metrics.surface_reconfigurations, 1);
        assert_eq!(resized.metrics.vertex_allocation_bytes, 0);

        for _ in 0..3 {
            let mut recreated = WgpuRenderer::new(offscreen_target(8, 8)).unwrap();
            let recreated_scene = synthetic_scene(8, 8).unwrap();
            recreated
                .render(RenderRequest {
                    scene: &recreated_scene,
                    resources: synthetic_provider().unwrap(),
                })
                .unwrap();
        }
    }

    /// A renderer holding only the atlas in its cache, a scene of the given
    /// quads, and a quad on the uncached synthetic texture with a frame
    /// outside it.
    fn cached_atlas_case() -> Option<(
        WgpuRenderer,
        impl Fn(Vec<tilewright_scene::Quad>) -> tilewright_scene::SceneBundle,
        tilewright_scene::Quad,
    )> {
        use tilewright_render_testkit::synthetic_digest;
        use tilewright_scene::{
            Layer, Quad, ResourceId, Sampling, SceneBundle, SceneLimits, SemanticId, TextureRect,
            Viewport,
        };
        let Ok(mut renderer) = WgpuRenderer::new(offscreen_target(112, 64)) else {
            eprintln!("skipping GPU frame-order test: no supported adapter");
            return None;
        };
        renderer
            .render(RenderRequest {
                scene: &atlas_scene(112, 64).unwrap(),
                resources: atlas_provider().unwrap(),
            })
            .unwrap();
        let rebuild = |quads: Vec<Quad>| {
            let viewport = Viewport {
                width: 112,
                height: 64,
                scale_milli: 1_000,
            };
            SceneBundle::from_quads(9, 0, viewport, [0.0; 4], quads, SceneLimits::default())
                .unwrap()
        };
        let outside = Quad {
            semantic_id: SemanticId(900),
            part: 0,
            resource_id: ResourceId(1),
            resource_revision: 1,
            resource_digest: synthetic_digest(),
            frame: Some(TextureRect {
                x: 1,
                y: 1,
                width: 2,
                height: 2,
            }),
            positions: [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            depth: 0,
            painter_order: 0,
            color: [1.0; 4],
            grayscale: 0.0,
            glow: None,
            pickable: true,
            sampling: Sampling::Nearest,
            layer: Layer::World,
        };
        Some((renderer, rebuild, outside))
    }

    fn is_out_of_bounds(
        result: Result<tilewright_render_api::FrameOutput, tilewright_render_api::Error>,
    ) -> bool {
        matches!(result, Err(tilewright_render_api::Error::Resource(message)) if message.contains("FrameOutOfBounds"))
    }

    #[test]
    fn cached_frames_are_checked_before_any_texture_loads() {
        use tilewright_render_resources::{
            Error as ResourceError, ResourceBlob, ResourceProvider, ResourceRequest,
        };
        use tilewright_scene::{Quad, TextureRect};
        struct Refusing;
        impl ResourceProvider for Refusing {
            fn load(&self, _request: ResourceRequest) -> Result<ResourceBlob, ResourceError> {
                Err(ResourceError::NotFound)
            }
        }
        let Some((mut renderer, rebuild, outside)) = cached_atlas_case() else {
            return;
        };
        // A bad frame on the cached atlas fails before the scene's missing
        // texture is requested, which the provider would refuse.
        let mut quads = atlas_scene(112, 64).unwrap().quads().to_vec();
        quads[0].frame = Some(TextureRect {
            x: 60,
            y: 60,
            width: 8,
            height: 1,
        });
        quads.push(Quad {
            frame: None,
            ..outside
        });
        assert!(is_out_of_bounds(renderer.render(RenderRequest {
            scene: &rebuild(quads),
            resources: std::sync::Arc::new(Refusing),
        })));
    }

    #[test]
    fn missing_frames_are_checked_before_the_cache_evicts() {
        use tilewright_render_resources::{
            DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES, ResourceProvider as _,
        };
        use tilewright_scene::{Quad, TextureRect};
        let Some((mut renderer, rebuild, outside)) = cached_atlas_case() else {
            return;
        };
        // A bad frame on a missing texture fails from its envelope header,
        // before a nearly full cache evicts the unused atlas to admit it.
        let bytes = renderer.texture_bytes;
        renderer.texture_bytes = DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES - 8;
        assert!(is_out_of_bounds(renderer.render(RenderRequest {
            scene: &rebuild(vec![outside]),
            resources: synthetic_provider().unwrap(),
        })));
        assert_eq!(renderer.textures.len(), 1);
        renderer.texture_bytes = bytes;
        // The same holds when an earlier, valid missing texture would make
        // the cache evict before the bad frame is reached.
        let provider = synthetic_provider().unwrap();
        let meadow = tilewright_render_testkit::world_texture(105);
        let meadow_bytes = world_provider()
            .unwrap()
            .load(tilewright_render_resources::ResourceRequest {
                id: meadow.id,
                revision: meadow.revision,
                digest: meadow.digest,
                maximum_bytes: tilewright_render_resources::DEFAULT_MAXIMUM_RESOURCE_BYTES,
            })
            .unwrap()
            .bytes()
            .to_vec();
        provider
            .insert(meadow.id, meadow.revision, meadow_bytes)
            .unwrap();
        let valid_first = Quad {
            semantic_id: tilewright_scene::SemanticId(901),
            resource_id: meadow.id,
            resource_revision: meadow.revision,
            resource_digest: meadow.digest,
            frame: None,
            painter_order: 0,
            ..outside
        };
        let bad_second = Quad {
            painter_order: 1,
            ..outside
        };
        renderer.texture_bytes = DEFAULT_MAXIMUM_TEXTURE_WORKING_SET_BYTES - 8;
        assert!(is_out_of_bounds(renderer.render(RenderRequest {
            scene: &rebuild(vec![valid_first, bad_second]),
            resources: provider,
        })));
        assert_eq!(renderer.textures.len(), 1);
        renderer.texture_bytes = bytes;
        // The same texture with a fitting frame loads beside the atlas.
        let fitting = Quad {
            frame: Some(TextureRect {
                x: 1,
                y: 0,
                width: 1,
                height: 2,
            }),
            ..outside
        };
        let frame = renderer
            .render(RenderRequest {
                scene: &rebuild(vec![fitting]),
                resources: synthetic_provider().unwrap(),
            })
            .unwrap();
        assert_eq!(frame.metrics.resource_requests, 1);
        assert_eq!(renderer.textures.len(), 2);
    }

    #[test]
    fn coordinates_just_outside_a_distant_frame_stay_transparent() {
        use tilewright_render_testkit::{FakeResourceProvider, texture_envelope};
        use tilewright_scene::{
            Layer, Quad, ResourceId, Sampling, SceneBundle, SceneLimits, SemanticId, TextureRect,
            Viewport,
        };
        // A one-texel red frame halfway along an 8,192-texel green row:
        // mapped onto the texture, a coordinate 0.0001 before the frame
        // rounds onto its edge, yet it must stay transparent as in the
        // reference.
        let row: Vec<[u8; 4]> = (0..8_192)
            .map(|texel| {
                if texel == 4_096 {
                    [255, 0, 0, 255]
                } else {
                    [0, 255, 0, 255]
                }
            })
            .collect();
        let (bytes, digest) = texture_envelope(8_192, 1, &row);
        let provider = std::sync::Arc::new(FakeResourceProvider::default());
        provider.insert(ResourceId(5), 1, bytes).unwrap();
        // Pixel centres sample u = -0.0001, 0.2499, 0.4999 and 0.7499.
        let quad = |id: u32, top: f32, sampling| Quad {
            semantic_id: SemanticId(id),
            part: 0,
            resource_id: ResourceId(5),
            resource_revision: 1,
            resource_digest: digest,
            frame: Some(TextureRect {
                x: 4_096,
                y: 0,
                width: 1,
                height: 1,
            }),
            positions: [[0.0, top], [4.0, top], [4.0, top + 2.0], [0.0, top + 2.0]],
            uvs: [[-0.1251, 0.0], [0.8749, 0.0], [0.8749, 1.0], [-0.1251, 1.0]],
            depth: 0,
            painter_order: id,
            color: [1.0; 4],
            grayscale: 0.0,
            glow: None,
            pickable: true,
            sampling,
            layer: Layer::World,
        };
        let scene = SceneBundle::from_quads(
            1,
            0,
            Viewport {
                width: 4,
                height: 4,
                scale_milli: 1_000,
            },
            [0.0, 0.0, 0.0, 1.0],
            [
                quad(1, 0.0, Sampling::Nearest),
                quad(2, 2.0, Sampling::Linear),
            ],
            SceneLimits::default(),
        )
        .unwrap();
        let Some(frame) = assert_matches_reference(&scene, &provider) else {
            return;
        };
        for row in 0..4 {
            assert_eq!(frame.semantic_ids[row * 4], 0, "row {row}");
            assert_eq!(&frame.rgba8[row * 16..row * 16 + 4], [0, 0, 0, 255]);
            assert_eq!(&frame.rgba8[row * 16 + 4..row * 16 + 8], [255, 0, 0, 255]);
        }
    }

    #[test]
    fn world_scenes_match_the_reference_when_a_gpu_adapter_is_available() {
        for scene in [
            iso_world_scene(192, 128).unwrap(),
            top_down_world_scene(96, 64).unwrap(),
        ] {
            if assert_matches_reference(&scene, &world_provider().unwrap()).is_none() {
                return;
            }
        }
    }

    #[test]
    fn skewed_edges_through_pixel_centres_match_the_reference_coverage() {
        use tilewright_render_testkit::synthetic_digest;
        use tilewright_scene::{
            Layer, Quad, ResourceId, Sampling, SceneBundle, SceneLimits, SemanticId, Viewport,
        };
        // Pixel centres lie exactly on every edge of these skewed quads; the
        // GPU's top-left rule must colour the same pixels the shared
        // rasteriser marks. Inset texture coordinates keep edge samples
        // inside the texture.
        let vertical = Quad {
            semantic_id: SemanticId(1),
            part: 0,
            resource_id: ResourceId(1),
            resource_revision: 1,
            resource_digest: synthetic_digest(),
            frame: None,
            positions: [[0.5, 0.0], [3.5, 1.0], [3.5, 6.0], [0.5, 5.0]],
            uvs: [[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]],
            depth: 0,
            painter_order: 0,
            color: [1.0; 4],
            grayscale: 0.0,
            glow: None,
            pickable: true,
            sampling: Sampling::Nearest,
            layer: Layer::World,
        };
        let horizontal = Quad {
            semantic_id: SemanticId(2),
            positions: [[4.0, 0.5], [7.0, 0.5], [8.0, 4.5], [5.0, 4.5]],
            ..vertical
        };
        let scene = SceneBundle::from_quads(
            1,
            0,
            Viewport {
                width: 8,
                height: 8,
                scale_milli: 1_000,
            },
            [0.0, 0.0, 0.0, 1.0],
            [vertical, horizontal],
            SceneLimits::default(),
        )
        .unwrap();
        let target = offscreen_target(8, 8);
        let Ok(mut renderer) = WgpuRenderer::new(target) else {
            eprintln!("skipping GPU edge test: no supported adapter");
            return;
        };
        let request = RenderRequest {
            scene: &scene,
            resources: synthetic_provider().unwrap(),
        };
        let frame = renderer.render(request.clone()).unwrap();
        let reference = ReferenceRenderer::new(target)
            .unwrap()
            .render(request)
            .unwrap();
        let coloured = |rgba8: &[u8]| -> Vec<bool> {
            rgba8
                .chunks_exact(4)
                .map(|pixel| pixel[..3] != [0, 0, 0])
                .collect()
        };
        assert_eq!(coloured(&frame.rgba8), coloured(&reference.rgba8));
        assert_eq!(
            coloured(&frame.rgba8),
            frame
                .semantic_ids
                .iter()
                .map(|id| *id != 0)
                .collect::<Vec<_>>()
        );
        assert_eq!(frame.semantic_ids, reference.semantic_ids);
    }

    #[test]
    fn shader_is_valid_portable_wgsl() {
        let source = include_str!("shader.wgsl");
        let module = naga::front::wgsl::parse_str(source).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }
}
