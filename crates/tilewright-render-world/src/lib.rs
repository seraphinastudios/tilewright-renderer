// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

//! Compiles a game-neutral tile-world view into an ordered [`SceneBundle`].
//!
//! The compiler is pure and deterministic: it owns projection (isometric or
//! top-down), terrain stretching, draw order, animation frame selection and
//! object effects, and produces textured quads for any renderer backend.

use std::fmt;
use tilewright_scene::{
    Digest256, Glow, Layer, Quad, ResourceId, Sampling, SceneBundle, SceneLimits, SemanticId,
    Viewport,
};

/// Art pixels by which bent isometric ground overlaps its neighbours.
pub const SEAM_BLEED_PX: f64 = 1.0;

/// How tiles project to the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// A 2:1 diamond projection. `tile_width` × `tile_height` is the diamond
    /// size in pixels; tiles step by half of each per tile axis.
    Isometric { tile_width: u32, tile_height: u32 },
    /// Square tiles, `tile_size` pixels on each side.
    TopDown { tile_size: u32 },
}

/// The view's camera. `center` is in plane tile coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub projection: Projection,
    pub center: [f64; 2],
    /// Screen pixels per art pixel; integers keep pixel art crisp.
    pub zoom: f32,
    pub width: u32,
    pub height: u32,
    pub sampling: Sampling,
}

/// A texture the consumer has already resolved into a resource identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TextureRef {
    pub id: ResourceId,
    pub revision: u64,
    pub digest: Digest256,
    pub width: u32,
    pub height: u32,
}

/// How an object's art sits on its tile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Anchor {
    /// Ground art covering the tile: a diamond in isometric projection, the
    /// tile square in top-down. Isometric ground bends with terrain heights;
    /// top-down ground shifts up by its tile's height without bending.
    Ground,
    /// Art standing on the tile: its bottom centre sits at the front of the
    /// object's footprint, raised by the tile's height and the object's `z`.
    Standing,
}

/// Animation frames chosen by the view clock.
#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    pub frames: Vec<TextureRef>,
    /// Milliseconds per frame; 0 shows the first frame only.
    pub frame_millis: u32,
    pub phase_millis: u32,
}

impl Animation {
    #[must_use]
    pub fn still(texture: TextureRef) -> Self {
        Self {
            frames: vec![texture],
            frame_millis: 0,
            phase_millis: 0,
        }
    }

    fn frame(&self, clock_millis: u64) -> TextureRef {
        let count = self.frames.len() as u64;
        let index = if self.frame_millis == 0 || count <= 1 {
            0
        } else {
            (clock_millis.wrapping_add(u64::from(self.phase_millis)) / u64::from(self.frame_millis))
                % count
        };
        self.frames[usize::try_from(index).unwrap_or(0)]
    }
}

/// A coloured halo around an object's silhouette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlowEffect {
    /// Halo colour; alpha is the peak strength.
    pub color: [f32; 4],
    /// Radius in art pixels.
    pub radius_px: f32,
    /// Pulse period in milliseconds; 0 is steady.
    pub pulse_millis: u32,
}

/// One object to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldObject {
    pub semantic_id: SemanticId,
    /// Head tile in plane coordinates.
    pub tile: [i64; 2],
    /// Tiles covered along x and y; at least 1 × 1.
    pub footprint: [u16; 2],
    /// Position in the profile's render-layer order.
    pub layer_rank: u16,
    pub sub_layer: i16,
    pub anchor: Anchor,
    pub animation: Animation,
    /// Extra screen offset in art pixels.
    pub offset_px: [f32; 2],
    /// Extra elevation in height units.
    pub z: i32,
    pub scale: [f32; 2],
    /// Clockwise rotation around the art's centre.
    pub rotation_degrees: f32,
    pub alpha: f32,
    pub tint: [f32; 3],
    pub grayscale: f32,
    pub glow: Option<GlowEffect>,
    /// Draw the object a second time after the next diagonal (isometric) or
    /// row (top-down), so tall art is not cut by later neighbours.
    pub double_draw: bool,
    pub pickable: bool,
}

impl WorldObject {
    /// An object with neutral effects.
    #[must_use]
    pub fn new(
        semantic_id: SemanticId,
        tile: [i64; 2],
        layer_rank: u16,
        anchor: Anchor,
        texture: TextureRef,
    ) -> Self {
        Self {
            semantic_id,
            tile,
            footprint: [1, 1],
            layer_rank,
            sub_layer: 0,
            anchor,
            animation: Animation::still(texture),
            offset_px: [0.0, 0.0],
            z: 0,
            scale: [1.0, 1.0],
            rotation_degrees: 0.0,
            alpha: 1.0,
            tint: [1.0, 1.0, 1.0],
            grayscale: 0.0,
            glow: None,
            double_draw: false,
            pickable: true,
        }
    }
}

/// Terrain height per tile for the area in view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeightField {
    pub origin: [i64; 2],
    pub width: u32,
    pub height: u32,
    /// Row-major; `None` means no ground on that tile.
    pub heights: Vec<Option<i32>>,
}

impl HeightField {
    /// A field with no heights; every tile counts as height 0.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            origin: [0, 0],
            width: 0,
            height: 0,
            heights: Vec::new(),
        }
    }

    /// The height of a tile, if the field has ground there.
    #[must_use]
    pub fn get(&self, x: i64, y: i64) -> Option<i32> {
        let column = x.checked_sub(self.origin[0])?;
        let row = y.checked_sub(self.origin[1])?;
        if column < 0 || row < 0 || column >= i64::from(self.width) || row >= i64::from(self.height)
        {
            return None;
        }
        let index = usize::try_from(row * i64::from(self.width) + column).ok()?;
        self.heights.get(index).copied().flatten()
    }
}

/// How shared tile corners take their height.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CornerRule {
    Mean,
    Max,
    Min,
}

/// Terrain stretching settings from the game profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stretch {
    pub enabled: bool,
    pub rule: CornerRule,
    /// Neighbours whose height differs from a tile's by more than this do
    /// not blend into its corners, leaving a hard edge.
    pub cliff_threshold: Option<i32>,
    /// Screen pixels per height unit at zoom 1.
    pub pixels_per_unit: f32,
}

impl Default for Stretch {
    fn default() -> Self {
        Self {
            enabled: true,
            rule: CornerRule::Mean,
            cliff_threshold: None,
            pixels_per_unit: 1.0,
        }
    }
}

/// Everything needed to draw one frame of a world.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldView {
    pub revision: u64,
    pub clock_millis: u64,
    pub camera: Camera,
    pub clear_color: [f32; 4],
    pub heights: HeightField,
    pub stretch: Stretch,
    /// Render-layer ranks below this are ground layers. Top-down projection
    /// draws them before every other layer.
    pub ground_layer_ranks: u16,
    pub objects: Vec<WorldObject>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidValue,
    Scene(tilewright_scene::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidValue => write!(formatter, "invalid world view"),
            Self::Scene(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for Error {}

/// Compiles a view into an ordered scene.
///
/// # Errors
/// Returns [`Error::InvalidValue`] for an invalid view and [`Error::Scene`]
/// when the produced scene exceeds its limits.
pub fn compile(view: &WorldView, limits: SceneLimits) -> Result<SceneBundle, Error> {
    validate(view)?;
    let projector = Projector::new(view);
    let mut entries: Vec<Entry<'_>> = Vec::with_capacity(view.objects.len());
    for (index, object) in view.objects.iter().enumerate() {
        let sequence = u32::try_from(index).map_err(|_| Error::InvalidValue)?;
        entries.push(Entry {
            key: projector.order_key(object, false, view.ground_layer_ranks),
            object,
            sequence,
            repeat: false,
        });
        if object.double_draw {
            entries.push(Entry {
                key: projector.order_key(object, true, view.ground_layer_ranks),
                object,
                sequence,
                repeat: true,
            });
        }
    }
    entries.sort_by(|left, right| {
        left.key
            .cmp(&right.key)
            .then(left.sequence.cmp(&right.sequence))
            .then(left.repeat.cmp(&right.repeat))
    });
    let mut quads = Vec::new();
    for (order, entry) in entries.iter().enumerate() {
        let painter_order = u32::try_from(order).map_err(|_| Error::InvalidValue)?;
        let part_base: u16 = if entry.repeat { 0x8000 } else { 0 };
        projector.emit(
            view,
            entry.object,
            painter_order,
            part_base,
            entry.repeat,
            &mut quads,
        );
        if quads.len() > limits.maximum_quads {
            return Err(Error::Scene(tilewright_scene::Error::LimitExceeded));
        }
    }
    let scale_milli = (view.camera.zoom * 1000.0).round().clamp(1.0, 8000.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let viewport = Viewport {
        width: view.camera.width,
        height: view.camera.height,
        scale_milli: scale_milli as u32,
    };
    SceneBundle::from_quads(
        view.revision,
        view.clock_millis,
        viewport,
        view.clear_color,
        quads,
        limits,
    )
    .map_err(Error::Scene)
}

struct Entry<'a> {
    key: OrderKey,
    object: &'a WorldObject,
    sequence: u32,
    repeat: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct OrderKey {
    group: u8,
    primary: i64,
    rank: u32,
    sub_layer: i16,
    row: i64,
    column: i64,
}

fn finite(values: &[f32]) -> bool {
    values.iter().all(|value| value.is_finite())
}

fn unit(values: &[f32]) -> bool {
    values
        .iter()
        .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
}

/// Largest tile coordinate magnitude a view may use. It keeps footprint and
/// diagonal arithmetic far from `i64` overflow and pixel positions exact in
/// `f64`.
pub const MAXIMUM_TILE_COORDINATE: i64 = 1 << 40;

fn validate(view: &WorldView) -> Result<(), Error> {
    let camera = &view.camera;
    let coordinate =
        |value: i64| (-MAXIMUM_TILE_COORDINATE..=MAXIMUM_TILE_COORDINATE).contains(&value);
    // Bent isometric tiles divide by half the tile width less the seam bleed,
    // so that must stay positive.
    let projection_ok = match camera.projection {
        Projection::Isometric {
            tile_width,
            tile_height,
        } => {
            (2..=1024).contains(&tile_width)
                && f64::from(tile_width) / 2.0 > SEAM_BLEED_PX
                && (2..=1024).contains(&tile_height)
        }
        Projection::TopDown { tile_size } => (1..=1024).contains(&tile_size),
    };
    let heights = &view.heights;
    let cells = u64::from(heights.width) * u64::from(heights.height);
    let valid = projection_ok
        && camera.center.iter().all(|value| value.is_finite())
        && camera.zoom.is_finite()
        && camera.zoom > 0.0
        && camera.zoom <= 8.0
        && view.stretch.pixels_per_unit.is_finite()
        && view.stretch.pixels_per_unit >= 0.0
        && view.stretch.cliff_threshold.is_none_or(|value| value >= 0)
        && usize::try_from(cells).is_ok_and(|cells| cells == heights.heights.len())
        && heights.origin.iter().all(|value| coordinate(*value))
        && view.objects.iter().all(|object| {
            object.tile.iter().all(|value| coordinate(*value))
                && object.footprint[0] >= 1
                && object.footprint[1] >= 1
                && !object.animation.frames.is_empty()
                && object
                    .animation
                    .frames
                    .iter()
                    .all(|frame| frame.width > 0 && frame.height > 0)
                && finite(&object.offset_px)
                && object
                    .scale
                    .iter()
                    .all(|value| value.is_finite() && *value > 0.0 && *value <= 16.0)
                && object.rotation_degrees.is_finite()
                && unit(&[object.alpha, object.grayscale])
                && unit(&object.tint)
                && object.glow.is_none_or(|glow| {
                    unit(&glow.color) && glow.radius_px.is_finite() && glow.radius_px >= 0.0
                })
        });
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidValue)
    }
}

struct Projector {
    projection: Projection,
    camera_world: [f64; 2],
    zoom: f64,
    half_viewport: [f64; 2],
    viewport: [f64; 2],
    stretch: Stretch,
    clock_millis: u64,
    sampling: Sampling,
}

impl Projector {
    fn new(view: &WorldView) -> Self {
        let projection = view.camera.projection;
        let mut projector = Self {
            projection,
            camera_world: [0.0, 0.0],
            zoom: f64::from(view.camera.zoom),
            half_viewport: [
                f64::from(view.camera.width) / 2.0,
                f64::from(view.camera.height) / 2.0,
            ],
            viewport: [f64::from(view.camera.width), f64::from(view.camera.height)],
            stretch: view.stretch,
            clock_millis: view.clock_millis,
            sampling: view.camera.sampling,
        };
        projector.camera_world = projector.world(view.camera.center[0], view.camera.center[1], 0.0);
        projector
    }

    /// Unscaled world pixels of a plane point at a height.
    fn world(&self, u: f64, v: f64, height: f64) -> [f64; 2] {
        let lift = height * f64::from(self.stretch.pixels_per_unit);
        match self.projection {
            Projection::Isometric {
                tile_width,
                tile_height,
            } => [
                (u - v) * f64::from(tile_width) / 2.0,
                (u + v) * f64::from(tile_height) / 2.0 - lift,
            ],
            Projection::TopDown { tile_size } => {
                [u * f64::from(tile_size), v * f64::from(tile_size) - lift]
            }
        }
    }

    fn screen(&self, world: [f64; 2]) -> [f64; 2] {
        [
            (world[0] - self.camera_world[0]) * self.zoom + self.half_viewport[0],
            (world[1] - self.camera_world[1]) * self.zoom + self.half_viewport[1],
        ]
    }

    fn order_key(&self, object: &WorldObject, repeat: bool, ground_ranks: u16) -> OrderKey {
        let front_x = object.tile[0] + i64::from(object.footprint[0]) - 1;
        let front_y = object.tile[1] + i64::from(object.footprint[1]) - 1;
        let bump = i64::from(repeat);
        let rank = if repeat {
            u32::from(u16::MAX) + 1
        } else {
            u32::from(object.layer_rank)
        };
        match self.projection {
            Projection::Isometric { .. } => OrderKey {
                group: 1,
                primary: front_x + front_y + bump,
                rank,
                sub_layer: object.sub_layer,
                row: 0,
                column: front_x,
            },
            Projection::TopDown { .. } if object.layer_rank < ground_ranks && !repeat => OrderKey {
                group: 0,
                primary: i64::from(object.layer_rank),
                rank: 0,
                sub_layer: object.sub_layer,
                row: object.tile[1],
                column: object.tile[0],
            },
            Projection::TopDown { .. } => OrderKey {
                group: 1,
                primary: front_y + bump,
                rank,
                sub_layer: object.sub_layer,
                row: 0,
                column: front_x,
            },
        }
    }

    fn corner_height(&self, heights: &HeightField, corner: [i64; 2], own: i32) -> f64 {
        if !self.stretch.enabled {
            return f64::from(own);
        }
        let mut samples = [0_i32; 4];
        let mut count = 0;
        for (dx, dy) in [(-1, -1), (0, -1), (-1, 0), (0, 0)] {
            if let Some(height) = heights.get(corner[0] + dx, corner[1] + dy) {
                let within = self.stretch.cliff_threshold.is_none_or(|threshold| {
                    (i64::from(height) - i64::from(own)).abs() <= i64::from(threshold)
                });
                if within {
                    samples[count] = height;
                    count += 1;
                }
            }
        }
        if count == 0 {
            return f64::from(own);
        }
        let samples = &samples[..count];
        match self.stretch.rule {
            CornerRule::Mean => {
                samples.iter().map(|value| f64::from(*value)).sum::<f64>()
                    / f64::from(u32::try_from(samples.len()).unwrap_or(1))
            }
            CornerRule::Max => f64::from(*samples.iter().max().unwrap_or(&own)),
            CornerRule::Min => f64::from(*samples.iter().min().unwrap_or(&own)),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn emit(
        &self,
        view: &WorldView,
        object: &WorldObject,
        painter_order: u32,
        part_base: u16,
        repeat: bool,
        quads: &mut Vec<Quad>,
    ) {
        let texture = object.animation.frame(self.clock_millis);
        let tile_height = view
            .heights
            .get(object.tile[0], object.tile[1])
            .unwrap_or(0);
        let base = Quad {
            semantic_id: object.semantic_id,
            part: part_base,
            resource_id: texture.id,
            resource_revision: texture.revision,
            resource_digest: texture.digest,
            positions: [[0.0; 2]; 4],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            depth: 0,
            painter_order,
            color: [object.tint[0], object.tint[1], object.tint[2], object.alpha],
            grayscale: object.grayscale,
            glow: None,
            pickable: object.pickable && !repeat,
            sampling: self.sampling,
            layer: Layer::World,
        };
        let art_w = f64::from(texture.width);
        let art_h = f64::from(texture.height);
        let mut push = |quad: Quad| {
            if self.visible(&quad) {
                quads.push(quad);
            }
        };
        if object.anchor == Anchor::Ground {
            match self.projection {
                Projection::Isometric {
                    tile_width,
                    tile_height: diamond_height,
                } => {
                    let [tile_x, tile_y] = object.tile;
                    let heights = [
                        self.corner_height(&view.heights, [tile_x, tile_y], tile_height),
                        self.corner_height(&view.heights, [tile_x + 1, tile_y], tile_height),
                        self.corner_height(&view.heights, [tile_x + 1, tile_y + 1], tile_height),
                        self.corner_height(&view.heights, [tile_x, tile_y + 1], tile_height),
                    ];
                    #[allow(clippy::cast_precision_loss)]
                    let lattice = [
                        (tile_x as f64, tile_y as f64),
                        ((tile_x + 1) as f64, tile_y as f64),
                        ((tile_x + 1) as f64, (tile_y + 1) as f64),
                        (tile_x as f64, (tile_y + 1) as f64),
                    ];
                    let corners: [[f64; 2]; 4] = std::array::from_fn(|index| {
                        let (u, v) = lattice[index];
                        self.screen(self.world(u, v, heights[index]))
                    });
                    let center_x = art_w / 2.0;
                    let center_y = art_h - (f64::from(diamond_height) - 1.0) / 2.0;
                    // Bent tiles map their art onto a diamond one art pixel larger than the
                    // lattice cell, so neighbouring meshes overlap instead of leaving
                    // hairline gaps where the art's stepped edges no longer interlock.
                    let half_w = f64::from(tile_width) / 2.0 - SEAM_BLEED_PX;
                    let half_h = f64::from(diamond_height) / 2.0 - SEAM_BLEED_PX / 2.0;
                    let map = |image: [f64; 2]| {
                        let across = (image[0] - center_x) / half_w;
                        let down = (image[1] - center_y) / half_h + 1.0;
                        let east = f64::midpoint(across, down);
                        let south = (down - across) / 2.0;
                        let weights = [
                            (1.0 - east) * (1.0 - south),
                            east * (1.0 - south),
                            east * south,
                            (1.0 - east) * south,
                        ];
                        let mut point = [0.0; 2];
                        for (weight, corner) in weights.iter().zip(corners) {
                            point[0] += weight * corner[0];
                            point[1] += weight * corner[1];
                        }
                        point
                    };
                    #[allow(clippy::float_cmp)]
                    let flat = heights.iter().all(|value| *value == heights[0]);
                    if flat {
                        let center = self.screen(self.world(
                            lattice[0].0 + 0.5,
                            lattice[0].1 + 0.5,
                            heights[0],
                        ));
                        let left = snap(center[0] - center_x * self.zoom);
                        let top = snap(center[1] - center_y * self.zoom);
                        push(Quad {
                            positions: rect(left, top, art_w * self.zoom, art_h * self.zoom),
                            ..base
                        });
                    } else {
                        let steps = 4_u16;
                        for row in 0..steps {
                            for column in 0..steps {
                                let image = |c: u16, r: u16| {
                                    [
                                        art_w * f64::from(c) / f64::from(steps),
                                        art_h * f64::from(r) / f64::from(steps),
                                    ]
                                };
                                let cell = [
                                    image(column, row),
                                    image(column + 1, row),
                                    image(column + 1, row + 1),
                                    image(column, row + 1),
                                ];
                                #[allow(clippy::cast_possible_truncation)]
                                push(Quad {
                                    part: part_base + row * steps + column,
                                    positions: cell.map(|point| {
                                        let screen = map(point);
                                        [screen[0] as f32, screen[1] as f32]
                                    }),
                                    uvs: cell.map(|point| {
                                        [(point[0] / art_w) as f32, (point[1] / art_h) as f32]
                                    }),
                                    ..base
                                });
                            }
                        }
                    }
                }
                Projection::TopDown { tile_size } => {
                    #[allow(clippy::cast_precision_loss)]
                    let origin = self.screen(self.world(
                        object.tile[0] as f64,
                        object.tile[1] as f64,
                        f64::from(tile_height),
                    ));
                    let size = f64::from(tile_size) * self.zoom;
                    push(Quad {
                        positions: rect(
                            snap(origin[0]),
                            snap(origin[1]),
                            size * f64::from(object.footprint[0]),
                            size * f64::from(object.footprint[1]),
                        ),
                        ..base
                    });
                }
            }
            return;
        }
        // Standing art: bottom centre at the front of the footprint.
        let footprint = [
            f64::from(object.footprint[0]),
            f64::from(object.footprint[1]),
        ];
        #[allow(clippy::cast_precision_loss)]
        let (head_x, head_y) = (object.tile[0] as f64, object.tile[1] as f64);
        let elevation = f64::from(tile_height) + f64::from(object.z);
        let anchor = match self.projection {
            Projection::Isometric { .. } => {
                let front = self.screen(self.world(
                    head_x + footprint[0],
                    head_y + footprint[1],
                    elevation,
                ));
                [front[0], front[1] - 0.5 * self.zoom]
            }
            Projection::TopDown { .. } => self.screen(self.world(
                head_x + footprint[0] / 2.0,
                head_y + footprint[1],
                elevation,
            )),
        };
        let width = art_w * f64::from(object.scale[0]) * self.zoom;
        let height = art_h * f64::from(object.scale[1]) * self.zoom;
        let left = snap(anchor[0] - width / 2.0 + f64::from(object.offset_px[0]) * self.zoom);
        let top = snap(anchor[1] + f64::from(object.offset_px[1]) * self.zoom - height);
        let rotation = f64::from(object.rotation_degrees).to_radians();
        let rotate = |corners: [[f32; 2]; 4]| -> [[f32; 2]; 4] {
            if rotation == 0.0 {
                return corners;
            }
            let center = [left + width / 2.0, top + height / 2.0];
            let (sin, cos) = rotation.sin_cos();
            #[allow(clippy::cast_possible_truncation)]
            corners.map(|point| {
                let dx = f64::from(point[0]) - center[0];
                let dy = f64::from(point[1]) - center[1];
                [
                    (center[0] + dx * cos - dy * sin) as f32,
                    (center[1] + dx * sin + dy * cos) as f32,
                ]
            })
        };
        if let Some(glow) = object.glow {
            let strength = if glow.pulse_millis == 0 {
                1.0
            } else {
                let period = u64::from(glow.pulse_millis);
                #[allow(clippy::cast_precision_loss)]
                let phase = (self.clock_millis % period) as f64 / period as f64;
                0.4 + 0.6 * (1.0 - (2.0 * phase - 1.0).abs())
            };
            let radius = f64::from(glow.radius_px);
            let pad_x = radius * f64::from(object.scale[0]) * self.zoom;
            let pad_y = radius * f64::from(object.scale[1]) * self.zoom;
            let extend_u = radius / art_w;
            let extend_v = radius / art_h;
            #[allow(clippy::cast_possible_truncation)]
            let glow_quad = Quad {
                part: part_base + 1,
                positions: rotate(rect(
                    left - pad_x,
                    top - pad_y,
                    width + 2.0 * pad_x,
                    height + 2.0 * pad_y,
                )),
                uvs: [
                    [-extend_u as f32, -extend_v as f32],
                    [(1.0 + extend_u) as f32, -extend_v as f32],
                    [(1.0 + extend_u) as f32, (1.0 + extend_v) as f32],
                    [-extend_u as f32, (1.0 + extend_v) as f32],
                ],
                color: [1.0; 4],
                grayscale: 0.0,
                glow: Some(Glow {
                    color: [
                        glow.color[0],
                        glow.color[1],
                        glow.color[2],
                        (f64::from(glow.color[3]) * strength * f64::from(object.alpha)) as f32,
                    ],
                    radius_uv: [extend_u.min(1.0) as f32, extend_v.min(1.0) as f32],
                }),
                pickable: false,
                ..base
            };
            push(glow_quad);
        }
        push(Quad {
            part: part_base + 2,
            positions: rotate(rect(left, top, width, height)),
            ..base
        });
    }

    fn visible(&self, quad: &Quad) -> bool {
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
        f64::from(max_x) > 0.0
            && f64::from(max_y) > 0.0
            && f64::from(min_x) < self.viewport[0]
            && f64::from(min_y) < self.viewport[1]
    }
}

/// Snaps an axis-aligned edge to the pixel grid, rounding halves up, so pixel
/// centres fall on texel centres and every backend samples the same texels.
fn snap(value: f64) -> f64 {
    (value + 0.5).floor()
}

#[allow(clippy::cast_possible_truncation)]
fn rect(left: f64, top: f64, width: f64, height: f64) -> [[f32; 2]; 4] {
    let right = left + width;
    let bottom = top + height;
    [
        [left as f32, top as f32],
        [right as f32, top as f32],
        [right as f32, bottom as f32],
        [left as f32, bottom as f32],
    ]
}

#[cfg(test)]
mod tests;
