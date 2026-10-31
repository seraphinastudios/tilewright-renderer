# Architecture and limits

Dependencies point inward toward contracts:

```text
scene <- resources <- render-api <- testkit
  ^          ^            ^          ^
  |          |            |          |
  +----------+------------+------ render-wgpu <- tilewright-render
  +-----------------------+------ render-sdl3
  +------------------------------ render-ui
  +------------------------------ render-world (used by testkit and the CLI)
  +------------------------------ render-web (wasm32 bindings; uses wgpu and testkit)
```

`tilewright-scene` owns immutable, versioned, sorted render inputs: textured
quads with per-quad effects, each optionally drawing one frame of an atlas or
sprite sheet (see the [scene contract](SCENE.md)). `render-api`
owns targets, lifecycle errors, output planes, and a renderer trait. Resource
providers receive explicit content identity, revision, SHA-256 digest, and byte
budget; this grants no ambient path or URL access. The provider is deliberately
a synchronous ready-cache lookup: consumers perform filesystem/network work
before submission, so rendering never nests an async executor or waits for I/O.
`render-testkit` supplies the
fake provider and exact CPU reference. wgpu device/queue/pipeline/texture types
remain private to `render-wgpu`; SDL owns presentation only. Consumers translate
their state into these contracts through adapters in their own repositories.

Default limits are 131,072 quads, 8,192 pixels on either axis, 16,777,216 total
pixels, 8× scale, 268,435,456 raster samples, and 16 MiB per resource request.
Raster samples bound overdraw: an axis-aligned quad counts its viewport-clipped
bounding box in pixels and any other quad the boxes of both its triangles, 33
times over for a glow quad with a nonzero radius. The GPU device is created with
downlevel limits except for the 2D texture dimension, which follows the adapter
up to the same 8,192 bound; targets and textures beyond the device's bound are
rejected with a typed error. All float inputs must be finite; color channels are
within 0–1; zero identities/revisions and all-zero digests are rejected. Each
(semantic ID, part) pair is unique, so one object may own several quads, and a
scene names at most 4,096 distinct textures. Sorting is stable by layer, depth,
painter order, semantic ID, then part.

## Quads, rasterisation and effects

A quad is four screen-space corners (top-left, top-right, bottom-right,
bottom-left) with matching texture coordinates, relative to the quad's
optional texture frame or else to the whole texture. It carries a color tint with
alpha, a greyscale amount, an optional glow, sampling, a pickable flag, and the
legacy effect fields. Axis-aligned quads use half-open rectangle coverage. Every
other quad is split into the triangles (0, 1, 2) and (0, 2, 3), snapped to 1/256
px, and filled with an exact integer edge test. Only pixel centres inside each
triangle's clipped bounding box are tested, and the top-left tie rule gives each
pixel on a shared edge to exactly one triangle, as GPU rasterisers do. The CPU
reference and the GPU backend's returned picking planes share this CPU
rasteriser, so semantic, depth, and coverage planes match exactly. These planes
are not independent GPU coverage measurements; GPU colour comparisons separately
check that visible sampling agrees with the contract. The GPU backend submits positions already rounded to the same 1/256 px
grid, so drivers with different snapping and rounding rules start from identical
fixed-point corners.

Texture lookups outside 0–1 are transparent, and no lookup reads a texel
outside the quad's frame: nearest and linear sampling clamp to the frame's
edge, and renderers reject a frame that does not lie inside its texture. The GPU uses two sampling pipelines for whole textures and atlas frames.
Nearest sampling loads the texel `floor(clamp(u, 0, 1 − ε) × width)` in frame
space. Linear sampling loads four frame-clamped texels and rounds the horizontal
then vertical blends to RGBA8, matching the reference. Tint, grayscale, effect
and glow staging also preserve the reference's byte rounding before blending.
Axis-aligned geometry with aligned (possibly flipped) UVs computes coordinates
from the fragment's physical pixel center and flat snapped bounds, avoiding UV
roundoff at included edges and exact nearest-texel ties. Rotated/skewed geometry
and non-aligned UV mappings retain triangle interpolation. A glow quad extends its art by
the glow radius and draws a halo from 16 fixed sample directions around the
art's alpha (a zero radius samples only the centre); it is never pickable,
and scene validation rejects a pickable glow quad. Greyscale uses Rec. 601 luma
weights before the legacy effect swap.

## World compiler

`tilewright-render-world` turns a world view (camera, height field, stretch
settings, and objects) into a scene. It depends only on `tilewright-scene` and
knows nothing about any game or content format.

- **Projection.** Isometric tiles step by half the tile size along each axis;
  top-down tiles step by the tile size. Switching projection changes only the
  camera.
- **Terrain stretching.** Each tile corner takes the mean, max, or min of the
  surrounding tile heights; neighbours beyond an optional cliff threshold keep
  a hard edge. A flat tile is one rectangle. A bent isometric tile maps its art
  onto a 4 × 4 bilinear mesh one art pixel larger than the lattice cell, so
  neighbouring meshes overlap instead of leaving hairline seams. Top-down
  ground shifts up by its tile height without bending. Isometric tiles must be
  wider than twice the seam bleed, and tile coordinates and the camera centre
  stay within ±2⁴⁰; a view whose projected corners overflow is rejected rather
  than culled.
- **Standing art** sits with its bottom centre on the front of its footprint,
  raised by the tile height and the object's `z`. Multi-tile footprints order
  by their front tile.
- **Draw order.** Isometric scenes order by diagonal (x + y), then layer rank,
  sub-layer, and column. Top-down scenes draw ground ranks first, then order
  by row. Double-drawn objects repeat after the next diagonal or row so tall art
  is not cut by later neighbours; the repeat is not pickable. Double draw is
  for opaque art: the repeat composites over the first copy, so it is
  rejected together with alpha below 1 or a glow, and translucent texels
  inside double-drawn art still composite twice by design.
- **Effects.** Offset, scale, rotation, alpha, tint, greyscale, a pulsing glow,
  and clocked animation frames are compiled into the quad fields.
- **Pixel grid.** Flat ground (top-down tiles and flat isometric tiles)
  snaps each of its four edges to whole pixels on its own. Neighbouring
  ground computes a shared world edge from the same value, so tiles that meet
  in the world meet at the same pixel edge at every zoom, view size and camera
  centre: tiled ground leaves no gap or overlap between quads, and a quad
  covers exactly the pixels whose centres lie inside its art. Its texture
  coordinates keep every texel where the unsnapped art puts it, so one world
  point samples the same texel of whichever tile draws it, and isometric
  diamonds interlock along their diagonals at fractional zoom as they do at
  zoom 1. A texture-coordinate margin of 1/65,536 keeps every sampled centre
  inside the art on every backend; a pixel centre on the boundary between
  neighbouring art, to within that margin, is drawn by both and painter order
  picks the one that shows. Art under one pixel across uses its sole covered
  centre's texture coordinate at both edges, preserving that centre's texel
  while keeping the coordinates bounded. At whole-number zooms ground art first
  moves by at most half a pixel onto the grid, rounding halves up, so pixel centres sample
  texel centres and every backend picks the same texels, as before. All ground
  whose edges lie whole art pixels apart at one height moves by the same
  amount, so where an edge falls within floating-point error of half a pixel
  the whole set rounds one way; before, neighbours there could round apart
  and leave a seam, so such views can differ from earlier output by whole
  quads moving one pixel. At other
  zooms all ground moves 1/128 px up and left instead, so where the unsnapped
  art puts a pixel centre exactly on a texel boundary, as cameras on tile
  corners do at zooms such as 0.5, 0.75, 1.25 and 1.5, the centre takes the
  later texel across the whole tile, as nearest sampling does. Such ties can
  still split a tile between the two texels when the camera sits exactly
  1/128 px off a round position, and near the edges of art more than 512 px
  across, where the margin outgrows the offset. Standing
  art snaps its top-left corner the same way and keeps its exact scaled size,
  so a sprite never gains or loses a pixel of width as it or the camera moves.
  Bent ground keeps its unsnapped mesh and overlap. Quads entirely outside the
  viewport are culled.
- **What content provides.** Geometry closes every seam between ground quads
  and keeps texels consistent between tiles, but whether art covers the
  boundary between its own texels and a neighbour's is the art's job. Ground
  that tiles exactly at zoom 1 with nearest sampling, such as opaque 2:1
  diamonds one pixel shorter than the tile, leaves no gap at fractional zooms
  either (the tests sweep 0.25× to 3× in steps of 0.01). Diamond art with soft
  or translucent edges, art that does not tile exactly, and linearly sampled
  art, whose edge texels blend with the transparent texels beside them, need
  about a texel of opaque bleed past the diamond's edge to stay closed. So
  does standing art meant to abut its neighbours, such as walls built from
  segments.
- **Validation.** Every quad the objects and their double-draw repeats will
  emit (16 for a bent isometric tile, 2 for a glowing object) is counted
  against the scene's quad budget before anything is ordered or culled,
  animations list at most 1,024 frames each and twice that budget together,
  naming at most 4,096 distinct textures across every frame, height fields
  hold at most 4,194,304 cells, and raster samples are charged for every
  object's costliest frame as if it were in view (its box widened a pixel and
  capped at the viewport, doubled when skewed). Every object and every
  animation frame is validated before culling or frame selection, so a view's
  validity never depends on the camera or clock: semantic IDs are nonzero and
  unique, frames carry full resource identities within the scene's dimension
  limit, every quad an object produces (after scale, rotation, glow padding
  and terrain bending) spans at most that limit less one pixel, and a glow
  radius stays within every frame. Ground objects reject the offset, `z`,
  scale, rotation and glow they cannot apply, isometric ground covers one
  tile, and no ground footprint covers a tile the height field marks as having
  none.

Lighting and weather are not part of this version.

## Cost and memory

CPU work is O(pixels + quads), GPU vertex generation is O(quads), and GPU
readback is O(pixels). A completed frame owns 11 bytes per pixel across RGBA,
semantic, depth, and coverage planes. The GPU path additionally owns a color
target, private depth/stencil target, at most 255 bytes of readback row padding
per row, and 672 bytes of transient vertices per quad (six 112-byte vertices).
Checked arithmetic precedes every size-derived allocation. Offscreen
targets/readback buffers are reused until resize; immutable textures are reused
by ID/revision/digest with a hard 4,096-entry cache. A scene's missing textures
are decoded before the cache changes; cached and staged textures share the
256 MiB CPU budget, and textures the scene does not use are evicted before a
decode or admission would exceed it or the entry bound, and a texture that still
does not fit is refused before it is decoded, so the cache never holds more than
one bounded working set. Scene validation and every renderer, the CPU reference
included, reject a scene naming more than 4,096 distinct textures before loading
any. Retained texture pixels and each scene's distinct texture working set are
also capped at 256 MiB, and prepared quads share decoded immutable pixels
instead of cloning them. The shared vertex buffer grows geometrically to the
next power of two, capped at the device's largest buffer (larger streams are
rejected before they are generated), and is reused by offscreen and surface
presentation until a larger scene is submitted; per-texture nearest/linear bind
groups are created only on cache misses, and consecutive quads of one texture
with the same sampling, such as layered characters drawn from one atlas, share
a draw call whether they draw the whole texture or different frames.

The following historical fill-cost measurements describe the 0.3 sampling
implementation, before staged RGBA8 sampling and physical-coordinate UVs.
Current performance evidence must be measured for the new implementation.

Fill cost is measured with the `fill_rate` example of `tilewright-render-wgpu`
(32 translucent 1,024 × 1,024 layers, or 7 glow layers, readback included;
median of three interleaved runs of the release build, Rust 1.97.1). All three
devices ran on one Ryzen 9 9950X3D host under Linux 7.0, each selected with
`VK_DRIVER_FILES`, through wgpu 30.0.1's Vulkan backend with the renderer's
downlevel limits (2D textures up to 8,192 px):

| Column | Adapter | Driver | Vulkan | Adapter limits |
| --- | --- | --- | --- | --- |
| lavapipe | llvmpipe (LLVM 21.1.8, 256 bits), CPU | Mesa 26.0.8 | 1.4.335 | 16,384 px textures, 2 GiB buffers |
| RADV | AMD Ryzen 9 9950X3D iGPU (RAPHAEL_MENDOCINO), integrated | Mesa 26.0.8 | 1.4.335 | 16,384 px textures, 2 GiB buffers |
| RTX 3090 | NVIDIA GeForce RTX 3090, discrete | NVIDIA 595.91.07 | 1.4.329 | 32,768 px textures, 4 GiB buffers |

`tilewright-render probe` prints the adapter, driver, backend and limits for
another machine.

| Mode | lavapipe | RADV | RTX 3090 |
| --- | --- | --- | --- |
| whole texture, nearest | 11.0 → 10.9 ms | 7.1 → 7.0 ms | 1.8 → 1.4 ms |
| whole texture, linear | 12.6 → 11.9 ms | 6.0 → 5.9 ms | 1.4 → 1.3 ms |
| whole texture, glow | 13.9 → 13.3 ms | 12.6 → 13.7 ms | 1.4 → 1.4 ms |
| frame, nearest | 13.0 ms | 8.5 ms | 1.4 ms |
| frame, linear | 16.0 ms | 8.8 ms | 1.5 ms |
| frame, glow | 29.1 ms | 21.1 ms | 1.6 ms |

Arrows run from version 0.2 to 0.3. The budget is: whole-texture quads stay
within 10% of 0.2 apart from run-to-run noise, and a frame smaller than its
texture costs at most 1.7× a whole-texture quad, except glow halos on
lavapipe, a software rasteriser, at up to 2.2×. The integrated GPU's frame
rows are medians of six runs.

No public dependency or type points to a client, editor, protocol, server, or
game implementation. `tools/check-architecture.sh` validates exact direct
crate edges from Cargo metadata and rejects forbidden sibling sources.

## Compatibility and cross-repository checklist

All ten crates are one compatibility set: they share one SemVer version and
are packaged/released from one tag. Consumers pin the same exact version of
every renderer crate. `SceneBundle::version` is the serialized logical-scene
contract (version 3; see [scene contract](SCENE.md)); the embedded shader ships
in the matching `render-wgpu` crate.

- A scene field, identity, ordering, semantic mask, or serialized bundle change
  requires renderer, protocol producer, client adapter, editor adapter, and
  corpus review.
- A resource envelope/digest/revision change requires renderer, world toolkit,
  content packaging, client/editor providers, and corpus/provenance review.
- A shader/material/sampling/depth change requires renderer and visual corpus
  review; exact masks must not change silently.
- A world-compiler change to projection, stretching, ordering, or effects
  requires world toolkit, client and editor adapter, and corpus review.
- SDL/window/lifecycle changes require client and editor platform-shell review.
- UI/text contracts require renderer, client UI, editor UI, font asset license,
  and corpus review.
- Pure consumer session/document adaptation remains in that consumer repository
  and must not add a reverse renderer dependency.
