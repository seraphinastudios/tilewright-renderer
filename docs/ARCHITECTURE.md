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
```

`tilewright-scene` owns immutable, versioned, sorted render inputs: textured
quads with per-quad effects. `render-api`
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
times over for a glow quad with a nonzero radius. The GPU device is created
with downlevel limits except for the 2D texture dimension, which follows the
adapter up to the same 8,192 bound; targets and textures beyond the device's
bound are rejected with a typed error. All float inputs must be finite; color
channels are within 0–1; zero identities/revisions and all-zero digests are
rejected. Each (semantic ID, part) pair is unique, so one object may own several
quads. Sorting is stable by layer, depth, painter order, semantic ID, then part.

## Quads, rasterisation and effects

A quad is four screen-space corners (top-left, top-right, bottom-right,
bottom-left) with matching texture coordinates. It carries a color tint with
alpha, a greyscale amount, an optional glow, sampling, a pickable flag, and the
legacy effect fields. Axis-aligned quads use half-open rectangle coverage.
Every other quad is split into the triangles (0, 1, 2) and (0, 2, 3), snapped
to 1/256 px, and filled with an exact integer edge test whose top-left tie rule
gives each pixel on a shared edge to exactly one triangle, as GPU rasterisers
do. The CPU reference and the GPU picking planes share this rasteriser, so
semantic, depth, and coverage planes match exactly on every backend and cover
the pixels the GPU colours. The GPU backend submits positions already rounded
to the same 1/256 px grid, so drivers with different snapping and rounding
rules start from identical fixed-point corners.

Texture lookups outside 0–1 are transparent. A glow quad extends its art by
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
  is not cut by later neighbours; the repeat is not pickable.
- **Effects.** Offset, scale, rotation, alpha, tint, greyscale, a pulsing glow,
  and clocked animation frames are compiled into the quad fields.
- **Pixel grid.** Axis-aligned art snaps its top-left corner to whole pixels,
  rounding halves up, so pixel centres sample texel centres and every backend
  picks the same texels. Quads entirely outside the viewport are culled.
- **Validation.** Draws (objects plus double-draw repeats) are counted against
  the scene's quad budget before anything is ordered. Every object and every
  animation frame is validated before culling or frame selection, so a view's
  validity never depends on the camera or clock: semantic IDs are nonzero and
  unique, frames carry full resource identities within the scene's dimension
  limit, every quad an object produces (after scale, rotation, glow padding
  and terrain bending) spans at most that limit less one pixel, and a glow
  radius stays within every frame. Ground objects reject the
  offset, `z`, scale, rotation and glow they cannot apply, and isometric
  ground covers one tile.

Lighting and weather are not part of this version.

## Cost and memory

CPU work is O(pixels + quads), GPU vertex generation is O(quads), and GPU
readback is O(pixels). A completed frame owns 11 bytes per pixel across RGBA,
semantic, depth, and coverage planes. The GPU path additionally owns a color
target, private depth/stencil target, at most 255 bytes of readback row padding
per row, and 432 bytes of transient vertices per quad (six 72-byte vertices). Checked arithmetic
precedes every size-derived allocation. Offscreen targets/readback buffers are
reused until resize; immutable textures are reused by ID/revision/digest with a
hard 4,096-entry cache. A scene's missing textures are decoded before the cache
changes; when admitting them would exceed the entry or byte bound, textures the
scene does not use are evicted first, so the cache never holds more than one
bounded working set. A scene naming more than 4,096 distinct textures is
rejected. Retained texture pixels and each scene's distinct texture working set
are also capped at 256 MiB, and prepared quads share decoded immutable pixels
instead of cloning them. The shared vertex buffer grows geometrically to the
next power of two, capped at the device's largest buffer (larger streams are
rejected before they are generated), and is reused by offscreen and surface
presentation until a larger scene is submitted; per-texture nearest/linear
bind groups are created only on cache misses.

No public dependency or type points to a client, editor, protocol, server, or
game implementation. `tools/check-architecture.sh` validates exact direct
crate edges from Cargo metadata and rejects forbidden sibling sources.

## Compatibility and cross-repository checklist

All nine crates are one compatibility set: they share one SemVer version and
are packaged/released from one tag. Consumers pin the same exact version of
every renderer crate. `SceneBundle::version` is the serialized logical-scene
contract; the embedded shader ships in the matching `render-wgpu` crate.

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
