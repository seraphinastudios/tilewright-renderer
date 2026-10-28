// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

//! Compiles a game-neutral tile-world view into an ordered [`SceneBundle`].
//!
//! The compiler is pure and deterministic: it owns projection (isometric or
//! top-down), terrain stretching, draw order, animation frame selection and
//! object effects, and produces textured quads for any renderer backend.

use std::{collections::HashSet, fmt};
use tilewright_scene::{
    Digest256, GLOW_SAMPLES_PER_PIXEL, Glow, Layer, MAXIMUM_SCENE_TEXTURES, Quad, ResourceId,
    Sampling, SceneBundle, SceneLimits, SemanticId, Viewport,
};

/// Most frames one animation may list.
pub const MAXIMUM_ANIMATION_FRAMES: usize = 1_024;

/// Most cells a view's height field may hold (2,048 × 2,048).
pub const MAXIMUM_HEIGHT_FIELD_CELLS: usize = 1 << 22;

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
///
/// The ID, revision and digest are nonzero, and each side is between 1 and
/// [`SceneLimits::maximum_dimension`] pixels. [`compile`] checks every
/// animation frame, whether or not the clock selects it.
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
    ///
    /// Ground keeps the neutral offset, `z`, scale and rotation and has no
    /// glow; isometric ground covers exactly one tile, and no ground footprint
    /// covers a tile the height field marks as having none. [`compile`] rejects
    /// other values rather than ignoring them.
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
            // Widened so a clock near `u64::MAX` keeps its phase.
            (u128::from(clock_millis) + u128::from(self.phase_millis))
                / u128::from(self.frame_millis)
                % u128::from(count)
        };
        self.frames[usize::try_from(index).unwrap_or(0)]
    }
}

/// A coloured halo around an object's silhouette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlowEffect {
    /// Halo colour; alpha is the peak strength.
    pub color: [f32; 4],
    /// Radius in art pixels, at most the width and height of every
    /// animation frame.
    pub radius_px: f32,
    /// Pulse period in milliseconds; 0 is steady.
    pub pulse_millis: u32,
}

/// One object to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldObject {
    /// Picking identity: nonzero and unique within a view.
    pub semantic_id: SemanticId,
    /// Head tile in plane coordinates.
    pub tile: [i64; 2],
    /// Tiles covered along x and y; at least 1 × 1, and exactly 1 × 1 for
    /// isometric ground.
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
    ///
    /// Double draw is for opaque art. The repeat composites over the first
    /// copy, so [`compile`] rejects it together with `alpha < 1` or a glow.
    /// Translucent texels inside double-drawn art still composite twice by
    /// design.
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
    /// Row-major; `None` means no ground on that tile, so no ground
    /// footprint may cover it. Tiles outside the field count as height 0.
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
        self.index(x, y)
            .and_then(|index| self.heights.get(index).copied().flatten())
    }

    fn index(&self, x: i64, y: i64) -> Option<usize> {
        let column = x.checked_sub(self.origin[0])?;
        let row = y.checked_sub(self.origin[1])?;
        if column < 0 || row < 0 || column >= i64::from(self.width) || row >= i64::from(self.height)
        {
            return None;
        }
        row.checked_mul(i64::from(self.width))?
            .checked_add(column)
            .and_then(|index| usize::try_from(index).ok())
    }
}

/// Prefix sums of a height field's holes, so whether a rectangle of tiles
/// covers one is answered in constant time.
struct Holes {
    origin: [i64; 2],
    width: usize,
    height: usize,
    /// `(width + 1) × (height + 1)` running counts; empty when the field has
    /// no holes. Fields hold at most [`MAXIMUM_HEIGHT_FIELD_CELLS`] cells, so
    /// the counts fit `u32`.
    sums: Vec<u32>,
}

impl Holes {
    /// Counts the holes of a field whose cell count is already validated.
    fn new(field: &HeightField) -> Option<Self> {
        let width = usize::try_from(field.width).ok()?;
        let height = usize::try_from(field.height).ok()?;
        let mut holes = Self {
            origin: field.origin,
            width,
            height,
            sums: Vec::new(),
        };
        if !field.heights.iter().any(Option::is_none) {
            return Some(holes);
        }
        let stride = width.checked_add(1)?;
        holes.sums = vec![0; stride.checked_mul(height.checked_add(1)?)?];
        for (row, cells) in field.heights.chunks_exact(width).enumerate() {
            let mut line = 0;
            for (column, cell) in cells.iter().enumerate() {
                line += u32::from(cell.is_none());
                holes.sums[(row + 1) * stride + column + 1] =
                    holes.sums[row * stride + column + 1] + line;
            }
        }
        Some(holes)
    }

    /// Whether any tile of the footprint starting at `tile` is a hole.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn cover(&self, tile: [i64; 2], footprint: [u16; 2]) -> bool {
        if self.sums.is_empty() {
            return false;
        }
        // Tile coordinates are bounded to ±2⁴⁰, so this cannot overflow.
        let clip = |start: i64, extent: u16, origin: i64, size: usize| {
            let size = i64::try_from(size).unwrap_or(i64::MAX);
            let low = (start - origin).clamp(0, size);
            let high = (start - origin + i64::from(extent)).clamp(0, size);
            (low as usize, high as usize)
        };
        let (left, right) = clip(tile[0], footprint[0], self.origin[0], self.width);
        let (top, bottom) = clip(tile[1], footprint[1], self.origin[1], self.height);
        if left >= right || top >= bottom {
            return false;
        }
        let stride = self.width + 1;
        let at = |x: usize, y: usize| self.sums[y * stride + x];
        at(right, bottom) + at(left, top) > at(left, bottom) + at(right, top)
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
/// Every object is validated, including objects the camera culls and
/// animation frames the clock does not select, so whether a view compiles
/// never depends on what is in view. Every quad an object produces must span
/// at most [`SceneLimits::maximum_dimension`] less one pixel on each axis,
/// measured from its own geometry before culling.
///
/// # Errors
/// Returns [`Error::InvalidValue`] for an invalid view and [`Error::Scene`]
/// for a repeated semantic ID or when the view or the produced scene exceeds
/// its limits, or its height field holds more than
/// [`MAXIMUM_HEIGHT_FIELD_CELLS`] cells. Every quad the view's objects and
/// double-draw repeats would emit, counted before culling (a bent isometric
/// tile emits 16 and a glowing object 2), must fit
/// [`SceneLimits::maximum_quads`]. An animation lists at most
/// [`MAXIMUM_ANIMATION_FRAMES`] frames, and a view's animations at most twice
/// [`SceneLimits::maximum_quads`] together, naming at most
/// [`MAXIMUM_SCENE_TEXTURES`] distinct textures across every frame.
pub fn compile(view: &WorldView, limits: SceneLimits) -> Result<SceneBundle, Error> {
    // Bound ordering and frame work by the scene budget before allocating or
    // traversing anything.
    let (draws, _) = view
        .objects
        .iter()
        .try_fold((0_usize, 0_usize), |(draws, frames), object| {
            let count = object.animation.frames.len();
            let draws = draws
                .checked_add(1 + usize::from(object.double_draw))
                .filter(|draws| *draws <= limits.maximum_quads)?;
            let frames = frames
                .checked_add(count)
                .filter(|frames| *frames <= limits.maximum_quads.saturating_mul(2))?;
            (count <= MAXIMUM_ANIMATION_FRAMES).then_some((draws, frames))
        })
        .ok_or(Error::Scene(tilewright_scene::Error::LimitExceeded))?;
    validate(view, limits)?;
    let projector = Projector::new(view, limits);
    check_emission(view, &projector, limits)?;
    let mut entries: Vec<Entry<'_>> = Vec::with_capacity(draws);
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
    // Raster work is charged for every object as if it were in view, for its
    // costliest frame, so panning or the clock cannot push a view over the
    // scene's sample budget.
    let mut raster_samples = 0_u64;
    for (order, entry) in entries.iter().enumerate() {
        let painter_order = u32::try_from(order).map_err(|_| Error::InvalidValue)?;
        let animation = &entry.object.animation;
        let shown = animation.frame(view.clock_millis);
        let mut costliest = projector.emit(
            view,
            entry.object,
            shown,
            painter_order,
            entry.repeat,
            Some(&mut quads),
        )?;
        if !entry.repeat {
            // Frames of other sizes give other quads at another clock, so
            // check those quads too, without drawing them.
            let mut sizes = HashSet::from([(shown.width, shown.height)]);
            for frame in &animation.frames {
                if sizes.insert((frame.width, frame.height)) {
                    let samples =
                        projector.emit(view, entry.object, *frame, painter_order, false, None)?;
                    costliest = costliest.max(samples);
                }
            }
            let draws = 1 + u64::from(entry.object.double_draw);
            raster_samples = raster_samples.saturating_add(costliest.saturating_mul(draws));
            if raster_samples > limits.maximum_raster_samples {
                return Err(Error::Scene(tilewright_scene::Error::LimitExceeded));
            }
        }
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

/// Checks, before culling, that every quad the view's draws will emit fits
/// the quad budget and that all its frames name few enough textures.
fn check_emission(
    view: &WorldView,
    projector: &Projector,
    limits: SceneLimits,
) -> Result<(), Error> {
    // Every quad each draw will emit is counted before culling, from the
    // view's own geometry, so the budget does not depend on the camera.
    view.objects
        .iter()
        .try_fold(0_usize, |total, object| {
            let repeats = 1 + usize::from(object.double_draw);
            total
                .checked_add(projector.quads_per_draw(view, object) * repeats)
                .filter(|total| *total <= limits.maximum_quads)
        })
        .ok_or(Error::Scene(tilewright_scene::Error::LimitExceeded))?;
    // Every texture any frame may show counts, so which ones survive culling
    // or the clock cannot change whether the scene fits every renderer.
    let mut textures = HashSet::new();
    for frame in view
        .objects
        .iter()
        .flat_map(|object| &object.animation.frames)
    {
        textures.insert((frame.id, frame.revision, frame.digest));
        if textures.len() > MAXIMUM_SCENE_TEXTURES {
            return Err(Error::Scene(tilewright_scene::Error::LimitExceeded));
        }
    }
    Ok(())
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

/// Largest tile coordinate magnitude a view may use, for object tiles, the
/// height field origin and the camera centre. It keeps footprint and
/// diagonal arithmetic far from `i64` overflow and pixel positions exact in
/// `f64`.
pub const MAXIMUM_TILE_COORDINATE: i64 = 1 << 40;

#[allow(clippy::float_cmp)]
fn validate(view: &WorldView, limits: SceneLimits) -> Result<(), Error> {
    let camera = &view.camera;
    let isometric = matches!(camera.projection, Projection::Isometric { .. });
    let texture_ok = |frame: &TextureRef| {
        frame.id.0 != 0
            && frame.revision != 0
            && frame.digest.0.iter().any(|byte| *byte != 0)
            && (1..=limits.maximum_dimension).contains(&frame.width)
            && (1..=limits.maximum_dimension).contains(&frame.height)
    };
    let heights = &view.heights;
    let cells = u64::from(heights.width) * u64::from(heights.height);
    if !usize::try_from(cells).is_ok_and(|cells| cells == heights.heights.len()) {
        return Err(Error::InvalidValue);
    }
    // Bounded before the field is scanned for holes.
    if heights.heights.len() > MAXIMUM_HEIGHT_FIELD_CELLS {
        return Err(Error::Scene(tilewright_scene::Error::LimitExceeded));
    }
    let holes = Holes::new(heights).ok_or(Error::InvalidValue)?;
    // Ground art ignores placement effects, so it must not request them, and
    // no tile it covers may be one the height field marks as having no ground.
    let ground_ok = |object: &WorldObject| {
        object.anchor != Anchor::Ground
            || (!holes.cover(object.tile, object.footprint)
                && object.offset_px == [0.0, 0.0]
                && object.z == 0
                && object.scale == [1.0, 1.0]
                && object.rotation_degrees == 0.0
                && object.glow.is_none()
                && (!isometric || object.footprint == [1, 1]))
    };
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
    #[allow(clippy::cast_precision_loss)]
    let center_bound = MAXIMUM_TILE_COORDINATE as f64;
    let valid = projection_ok
        && camera
            .center
            .iter()
            .all(|value| value.is_finite() && value.abs() <= center_bound)
        && camera.zoom.is_finite()
        && camera.zoom > 0.0
        && camera.zoom <= 8.0
        && view.stretch.pixels_per_unit.is_finite()
        && view.stretch.pixels_per_unit >= 0.0
        && view.stretch.cliff_threshold.is_none_or(|value| value >= 0)
        && heights.origin.iter().all(|value| coordinate(*value))
        && view.objects.iter().all(|object| {
            let frames = &object.animation.frames;
            object.semantic_id.0 != 0
                && object.tile.iter().all(|value| coordinate(*value))
                && object.footprint[0] >= 1
                && object.footprint[1] >= 1
                && !frames.is_empty()
                && frames.iter().all(texture_ok)
                && finite(&object.offset_px)
                && object
                    .scale
                    .iter()
                    .all(|value| value.is_finite() && *value > 0.0 && *value <= 16.0)
                && object.rotation_degrees.is_finite()
                && unit(&[object.alpha, object.grayscale])
                && unit(&object.tint)
                && object.glow.is_none_or(|glow| {
                    unit(&glow.color)
                        && glow.radius_px.is_finite()
                        && glow.radius_px >= 0.0
                        && frames.iter().all(|frame| {
                            f64::from(glow.radius_px) <= f64::from(frame.width.min(frame.height))
                        })
                })
                && ground_ok(object)
                // The repeat would composite translucency over the first copy.
                && (!object.double_draw || (object.alpha >= 1.0 && object.glow.is_none()))
        });
    if !valid {
        return Err(Error::InvalidValue);
    }
    let mut identities = HashSet::with_capacity(view.objects.len());
    if view
        .objects
        .iter()
        .all(|object| identities.insert(object.semantic_id))
    {
        Ok(())
    } else {
        Err(Error::Scene(tilewright_scene::Error::DuplicateSemanticId))
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
    /// Largest span a quad may have on either axis, checked before culling.
    maximum_span: f64,
}

/// Pixels a quad's span must stay below the scene's dimension limit, so the
/// `f32` rounding of its corners in view cannot push it over that limit.
const SPAN_MARGIN_PX: f64 = 1.0;

/// The bounding-box span of a `width` × `height` rectangle rotated by
/// `rotation` radians.
fn rotated_span(width: f64, height: f64, rotation: f64) -> [f64; 2] {
    if rotation == 0.0 {
        return [width, height];
    }
    let (sin, cos) = rotation.sin_cos();
    [
        (width * cos).abs() + (height * sin).abs(),
        (width * sin).abs() + (height * cos).abs(),
    ]
}

fn span_of(points: &[[f64; 2]; 4]) -> [f64; 2] {
    let (min_x, max_x, min_y, max_y) = points.iter().fold(
        (f64::MAX, f64::MIN, f64::MAX, f64::MIN),
        |(min_x, max_x, min_y, max_y), point| {
            (
                min_x.min(point[0]),
                max_x.max(point[0]),
                min_y.min(point[1]),
                max_y.max(point[1]),
            )
        },
    );
    [max_x - min_x, max_y - min_y]
}

impl Projector {
    fn new(view: &WorldView, limits: SceneLimits) -> Self {
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
            maximum_span: f64::from(limits.maximum_dimension) - SPAN_MARGIN_PX,
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

    /// The corners and texture coordinates of ground art spanning
    /// `top_left` to `bottom_right` in world pixels before the tile's
    /// `rise`.
    ///
    /// Each edge is projected and snapped to the pixel grid on its own, and
    /// the quad covers exactly the pixels whose centres lie inside the art.
    /// Ground that meets its neighbour at a world edge computes that edge
    /// from the same value, so the two share one pixel edge at every zoom,
    /// view size and camera position: tiled ground leaves no gap and no
    /// overlap between quads. The texture coordinates keep every texel where
    /// the unsnapped art puts it, so art that interlocks inside its quads,
    /// such as isometric diamonds, interlocks the same way in every tile.
    ///
    /// At whole-number zooms the art first moves by at most half a pixel
    /// onto the pixel grid, rounding halves up as it always has, so pixel
    /// centres fall on texel centres and the art keeps its whole texture.
    /// The shift is taken from one reference point per edge class: art whose
    /// edges lie a whole number of art pixels apart at the same rise, which
    /// includes every neighbour sharing an edge, shares that point and so
    /// moves by exactly the same amount. No shift can put every texel
    /// boundary on a pixel edge at other zooms, so there the art stays where
    /// it is.
    fn ground_quad(
        &self,
        top_left: [f64; 2],
        bottom_right: [f64; 2],
        rise: f64,
    ) -> ([[f64; 2]; 4], [[f32; 2]; 4]) {
        let lifted = |point: [f64; 2]| self.screen([point[0], point[1] - rise]);
        let (start, end) = (lifted(top_left), lifted(bottom_right));
        let whole = self.zoom.fract() == 0.0;
        // The class's point nearest the camera, so it keeps its precision far
        // from the world origin.
        let reference = lifted(std::array::from_fn(|index| {
            self.camera_world[index].floor() + top_left[index].rem_euclid(1.0)
        }));
        let axis = |index: usize| {
            let (shift, margin) = if whole {
                (snap(reference[index]) - reference[index], 0.0)
            } else {
                (0.0, GROUND_UV_MARGIN)
            };
            let (start, end) = (start[index] + shift, end[index] + shift);
            let edges = [start, end].map(centre_edge);
            let extent = end - start;
            // Whole-zoom art spans whole pixels and keeps every pixel centre
            // half a pixel inside its edges. Art under a pixel across covers
            // at most one pixel centre, which samples its middle; this keeps
            // its coordinates small.
            let uvs = if whole || extent < 1.0 {
                [0.0, 1.0]
            } else {
                edges.map(|edge| margin + (edge - start) / extent * (1.0 - 2.0 * margin))
            };
            #[allow(clippy::cast_possible_truncation)]
            (edges, uvs.map(|uv| uv as f32))
        };
        let ([left, right], [u0, u1]) = axis(0);
        let ([top, bottom], [v0, v1]) = axis(1);
        (
            [[left, top], [right, top], [right, bottom], [left, bottom]],
            [[u0, v0], [u1, v0], [u1, v1], [u0, v1]],
        )
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

    fn tile_height(view: &WorldView, object: &WorldObject) -> i32 {
        view.heights
            .get(object.tile[0], object.tile[1])
            .unwrap_or(0)
    }

    /// The heights of a ground tile's four corners, from the field alone.
    fn ground_corners(&self, view: &WorldView, object: &WorldObject) -> [f64; 4] {
        let [x, y] = object.tile;
        let own = Self::tile_height(view, object);
        [[x, y], [x + 1, y], [x + 1, y + 1], [x, y + 1]]
            .map(|corner| self.corner_height(&view.heights, corner, own))
    }

    /// Quads one draw of an object emits before culling: a bent isometric
    /// tile is a mesh, and a glow adds a halo.
    fn quads_per_draw(&self, view: &WorldView, object: &WorldObject) -> usize {
        match (object.anchor, self.projection) {
            (Anchor::Ground, Projection::Isometric { .. }) => {
                if flat(&self.ground_corners(view, object)) {
                    1
                } else {
                    usize::from(MESH_STEPS * MESH_STEPS)
                }
            }
            (Anchor::Ground, Projection::TopDown { .. }) => 1,
            (Anchor::Standing, _) => 1 + usize::from(object.glow.is_some()),
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
    /// Emits the object's quads for one animation frame into `quads`, or
    /// only checks them when `quads` is `None`. Returns the raster samples
    /// those quads may cost wherever they lie in view.
    fn emit(
        &self,
        view: &WorldView,
        object: &WorldObject,
        texture: TextureRef,
        painter_order: u32,
        repeat: bool,
        mut quads: Option<&mut Vec<Quad>>,
    ) -> Result<u64, Error> {
        let part_base: u16 = if repeat { 0x8000 } else { 0 };
        let tile_height = Self::tile_height(view, object);
        let base = Quad {
            semantic_id: object.semantic_id,
            part: part_base,
            resource_id: texture.id,
            resource_revision: texture.revision,
            resource_digest: texture.digest,
            frame: None,
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
        // Every quad is checked before culling, so whether a view compiles
        // never depends on what is in view. `span` is the quad's size from
        // its local geometry, independent of where it lies; spans over the
        // scene's dimension limit and corners that overflow to infinity are
        // an invalid view, not quads to cull. A quad whose corners collapse
        // onto a line covers no pixels and is dropped.
        let mut samples = 0_u64;
        let mut push = |quad: Quad, corners: [[f64; 2]; 4], span: [f64; 2], skewed: bool| {
            if !span.iter().all(|extent| *extent <= self.maximum_span) {
                return Err(Error::InvalidValue);
            }
            samples = samples.saturating_add(self.raster_estimate(span, skewed, quad.glow));
            #[allow(clippy::cast_possible_truncation)]
            let quad = Quad {
                positions: corners.map(|point| point.map(|value| value as f32)),
                ..quad
            };
            if !quad
                .positions
                .iter()
                .flatten()
                .all(|value| value.is_finite())
            {
                return Err(Error::InvalidValue);
            }
            if let Some(quads) = quads.as_deref_mut()
                && self.visible(&quad)
                && !collapsed(&quad)
            {
                quads.push(quad);
            }
            Ok(())
        };
        if object.anchor == Anchor::Ground {
            match self.projection {
                Projection::Isometric {
                    tile_width,
                    tile_height: diamond_height,
                } => {
                    let [tile_x, tile_y] = object.tile;
                    let heights = self.ground_corners(view, object);
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
                    // The same corners relative to the first, from the
                    // tile's own geometry, so spans do not depend on where
                    // the tile lies.
                    let ppu = f64::from(self.stretch.pixels_per_unit);
                    let local: [[f64; 2]; 4] = std::array::from_fn(|index| {
                        let (u, v) = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)][index];
                        [
                            (u - v) * f64::from(tile_width) / 2.0 * self.zoom,
                            ((u + v) * f64::from(diamond_height) / 2.0
                                - (heights[index] - heights[0]) * ppu)
                                * self.zoom,
                        ]
                    });
                    let center_x = art_w / 2.0;
                    let center_y = art_h - (f64::from(diamond_height) - 1.0) / 2.0;
                    // Bent tiles map their art onto a diamond one art pixel larger than the
                    // lattice cell, so neighbouring meshes overlap instead of leaving
                    // hairline gaps where the art's stepped edges no longer interlock.
                    let half_w = f64::from(tile_width) / 2.0 - SEAM_BLEED_PX;
                    let half_h = f64::from(diamond_height) / 2.0 - SEAM_BLEED_PX / 2.0;
                    let map = |corners: [[f64; 2]; 4], image: [f64; 2]| {
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
                    if flat(&heights) {
                        // The art's edges in world pixels before the tile's
                        // rise are multiples of half a pixel, so exact and
                        // equal wherever neighbouring art meets; each edge
                        // then subtracts the same rise once.
                        #[allow(clippy::cast_precision_loss)]
                        let (across, down) =
                            ((tile_x - tile_y) as f64, (tile_x + tile_y + 1) as f64);
                        let left = across * f64::from(tile_width) / 2.0 - center_x;
                        let top = down * f64::from(diamond_height) / 2.0 - center_y;
                        let rise = heights[0] * ppu;
                        let (corners, uvs) =
                            self.ground_quad([left, top], [left + art_w, top + art_h], rise);
                        push(
                            Quad { uvs, ..base },
                            corners,
                            [art_w * self.zoom, art_h * self.zoom],
                            false,
                        )?;
                    } else {
                        let steps = MESH_STEPS;
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
                                push(
                                    Quad {
                                        part: part_base + row * steps + column,
                                        uvs: cell.map(|point| {
                                            [(point[0] / art_w) as f32, (point[1] / art_h) as f32]
                                        }),
                                        ..base
                                    },
                                    cell.map(|point| map(corners, point)),
                                    span_of(&cell.map(|point| map(local, point))),
                                    true,
                                )?;
                            }
                        }
                    }
                }
                Projection::TopDown { tile_size } => {
                    #[allow(clippy::cast_precision_loss)]
                    let [x, y] = object.tile.map(|value| value as f64);
                    let [across, down] = object.footprint.map(f64::from);
                    let rise = f64::from(tile_height) * f64::from(self.stretch.pixels_per_unit);
                    let size = f64::from(tile_size) * self.zoom;
                    let (corners, uvs) = self.ground_quad(
                        self.world(x, y, 0.0),
                        self.world(x + across, y + down, 0.0),
                        rise,
                    );
                    push(
                        Quad { uvs, ..base },
                        corners,
                        [size * across, size * down],
                        false,
                    )?;
                }
            }
            return Ok(samples);
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
        let rotate = |corners: [[f64; 2]; 4]| -> [[f64; 2]; 4] {
            if rotation == 0.0 {
                return corners;
            }
            let center = [left + width / 2.0, top + height / 2.0];
            let (sin, cos) = rotation.sin_cos();
            // Rotate the corners as `f32` would hold them, about the exact
            // centre.
            #[allow(clippy::cast_possible_truncation)]
            corners.map(|point| {
                let dx = f64::from(point[0] as f32) - center[0];
                let dy = f64::from(point[1] as f32) - center[1];
                [
                    center[0] + dx * cos - dy * sin,
                    center[1] + dx * sin + dy * cos,
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
            let (glow_width, glow_height) = (width + 2.0 * pad_x, height + 2.0 * pad_y);
            #[allow(clippy::cast_possible_truncation)]
            let glow_quad = Quad {
                part: part_base + 1,
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
                    // Validation keeps the radius within the art, so this
                    // is the full radius.
                    radius_uv: [extend_u as f32, extend_v as f32],
                }),
                pickable: false,
                ..base
            };
            push(
                glow_quad,
                rotate(rect(left - pad_x, top - pad_y, glow_width, glow_height)),
                rotated_span(glow_width, glow_height, rotation),
                rotation != 0.0,
            )?;
        }
        push(
            Quad {
                part: part_base + 2,
                ..base
            },
            rotate(rect(left, top, width, height)),
            rotated_span(width, height, rotation),
            rotation != 0.0,
        )?;
        Ok(samples)
    }

    /// Raster samples a quad of this span may cost wherever it lies in view:
    /// its box widened by a pixel for sub-pixel placement and capped at the
    /// viewport, twice over for a skewed quad (one box per triangle), times
    /// its samples per pixel. It never undercounts the scene's own charge.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn raster_estimate(&self, span: [f64; 2], skewed: bool, glow: Option<Glow>) -> u64 {
        let axis = |extent: f64, size: f64| (extent.ceil() + 1.0).clamp(0.0, size) as u64;
        let pixels = axis(span[0], self.viewport[0]) * axis(span[1], self.viewport[1]);
        let per_pixel = match glow {
            Some(glow) if glow.radius_uv.iter().any(|radius| *radius > 0.0) => {
                GLOW_SAMPLES_PER_PIXEL
            }
            _ => 1,
        };
        pixels
            .saturating_mul(1 + u64::from(skewed))
            .saturating_mul(per_pixel)
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

/// The pixel edge before the first pixel whose centre lies at or after
/// `value`. Edges snapped this way cover the pixel centres from a start
/// (inclusive) to an end (exclusive), the same rule by which nearest sampling
/// gives a centre on a texel boundary to the later texel.
fn centre_edge(value: f64) -> f64 {
    (value - 0.5).ceil()
}

fn rect(left: f64, top: f64, width: f64, height: f64) -> [[f64; 2]; 4] {
    let right = left + width;
    let bottom = top + height;
    [[left, top], [right, top], [right, bottom], [left, bottom]]
}

/// How far inside the texture's edges ground art maps its own edges, in
/// texture coordinates, at zooms that are not whole numbers. Every pixel
/// centre a ground quad covers lies inside its unsnapped art, but `f32`
/// interpolation on any backend could carry a centre on the art's edge just
/// outside it, where texels read as transparent. The margin keeps such centres inside. It grows the art by
/// 1/65,536 of its size each way, so a pixel centre lying on the boundary
/// between neighbouring art, to within that much, is drawn by both rather
/// than by neither; painter order picks the one that shows.
const GROUND_UV_MARGIN: f64 = 1.0 / 65_536.0;

/// Rows and columns of the mesh a bent isometric tile becomes.
const MESH_STEPS: u16 = 4;

#[allow(clippy::float_cmp)]
fn flat(heights: &[f64; 4]) -> bool {
    heights.iter().all(|value| *value == heights[0])
}

/// Whether a quad's corners lie on one line, so it covers no pixels.
fn collapsed(quad: &Quad) -> bool {
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
    max_x - min_x <= 0.0 || max_y - min_y <= 0.0
}

#[cfg(test)]
mod tests;
