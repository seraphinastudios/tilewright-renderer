# Scene contract

`tilewright-scene` defines what every renderer draws: a `SceneBundle`, an
immutable, validated, sorted list of textured quads in logical pixel space.
`SceneBundle::version()` reports the contract version below. Consumers build
scenes through the Rust API; the renderer never parses a scene file.

## Versions

| Version | Change |
| --- | --- |
| 1 | Axis-aligned sprites. |
| 2 | Arbitrary textured quads with tint, greyscale, glow, pickability and sampling. |
| 3 | Optional texture frames: a quad can draw one rectangle of an atlas or sprite sheet. |

Version 3 is a superset of version 2. A quad without a frame renders exactly
as it did in version 2, byte for byte, on the CPU reference, and the corpus
digests of every version 2 scenario are unchanged. Rust callers that build
`Quad` or `Sprite` literals add `frame: None` to keep the old behaviour.

## Quads

A quad names its texture by resource ID, revision and SHA-256 digest, and
carries:

- `positions`: four corners in logical pixels (top-left, top-right,
  bottom-right, bottom-left);
- `uvs`: the matching texture coordinates;
- `frame`: an optional `TextureRect` of the texture (version 3);
- depth, painter order, layer, tint colour with alpha, greyscale amount, an
  optional glow, sampling (`Nearest` or `Linear`), and a pickable flag.

See [architecture](ARCHITECTURE.md#quads-rasterisation-and-effects) for
rasterisation, ordering and limits.

## Texture frames

A `TextureRect { x, y, width, height }` is a rectangle of texels inside the
quad's texture: one frame of an atlas or sprite sheet. With a frame:

- **Coordinates are frame-relative.** UV (0, 0) is the frame's top-left
  corner and (1, 1) its bottom-right corner, exactly as they span the whole
  texture without a frame. Code that draws a frame changes only the frame,
  not its UVs.
- **Nothing outside the frame is ever sampled.** Coordinates outside 0–1 are
  transparent, as before. Nearest sampling reads texel
  `floor(clamp(u, 0, 1 − ε) × width)` of the frame. Linear sampling blends
  the two nearest texel columns at `u × width − 0.5`, then the two rows, with
  both clamped to the frame's edge, so neighbouring frames never bleed in.
  Glow halos sample around the frame's own silhouette only. Bilinear ground
  meshes and glow padding that extend UVs past 0–1 keep working.
- **Atlas and separate textures are interchangeable.** Drawing a frame of an
  atlas gives exactly the pixels of drawing the same texels cut out into a
  texture of their own, with either sampling and with glow. The resources
  crate tests this property for every frame of a fixture atlas.

Validation:

- The scene rejects an empty frame and one that does not fit inside
  `SceneLimits::maximum_dimension` on both axes.
- Every renderer rejects a frame that does not lie inside its decoded texture
  with the resource error `FrameOutOfBounds`. Like the reference, the GPU
  backend takes quads in scene order: a texture is charged to the scene's
  working set at its first quad, before its envelope is inspected, and each
  quad's frame is checked before the next quad touches anything, so the
  first failure is the one the reference reaches, whatever the cache holds.
  A texture's first frame is checked against its envelope header before the
  texture is admitted to the cache. No texture is
  uploaded and nothing is drawn until every frame has been checked. As with
  any other resource failure, admitting an earlier texture of a failing
  scene may already have evicted cached textures the scene does not name;
  that costs a later re-upload and never changes a frame.

### Sprite sheets

`SpriteSheet` numbers the frames of a regular grid, row by row from the
top-left frame:

```rust
use tilewright_scene::{SpriteSheet, TextureRect};

// A sheet of 64 × 64 frames, 13 columns by 21 rows.
let sheet = SpriteSheet::grid(64, 64, 13, 21);
assert_eq!(sheet.frame(14), Some(TextureRect { x: 64, y: 64, width: 64, height: 64 }));
assert_eq!(sheet.frame_at(12, 20), sheet.frame(272));
assert_eq!(sheet.frame(273), None);
```

A sheet may start at an `origin` and leave `spacing` texels between frames.
Sheets with irregular frames list their `TextureRect`s directly.

### Layered characters

A layered character draws one quad per layer from the same frame position
of each layer's sheet: body, then clothing, then hair, each a `part` of one
semantic ID with increasing painter order. When all layers share one atlas
texture, consecutive quads of the same texture and sampling that all draw
frames smaller than the texture are drawn in one GPU batch, whatever their
frames; a quad drawing the whole texture can share that batch too. The corpus scenario
`atlas-layered-characters` draws such characters from a procedural atlas.

## Cost

Frames cost nothing on the CPU reference. The GPU vertex carries the frame
as four 16-bit integers, plus flat physical/UV rectangles for stable sampling;
a vertex is 112 bytes and a quad 672 bytes. Eleven vertex attributes and ten
inter-stage variables fit the WebGL2 downlevel limits. The fragment shader
checks frame-relative coordinates and directly loads texels inside that frame.
The nearest and linear pipelines serve both atlas frames and whole textures.
Linear sampling rounds horizontal then vertical blends to RGBA8, like the
reference, rather than depending on hardware filtering precision. See
[architecture](ARCHITECTURE.md#cost-and-memory) for cost and sampling details.
