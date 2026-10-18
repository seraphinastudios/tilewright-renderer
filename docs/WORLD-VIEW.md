# World-view files

`tilewright-render world VIEW.json OUTPUT.png [--reference]` renders one frame
of a tile world from a `tilewright.view/1` file. The file is plain JSON holding
the same data as `tilewright_render_world::WorldView`. Consumers such as the
world toolkit or a game's content tools write it; the renderer never reads
game content directly.

The command compiles the view with `tilewright-render-world`, renders it with
the wgpu backend (or the CPU reference with `--reference`), and writes a PNG.
The output path must not exist; the write is atomic and never overwrites.

## Inputs and trust

- Every texture is listed explicitly with a path relative to the view file
  and the SHA-256 of the file's bytes. Paths are at most 4,096 bytes,
  `/`-separated, with no empty, `.` or `..` components. On Unix the view's directory is opened once, and the
  view file and each texture are opened through that handle, one directory at
  a time without following symbolic links and without waiting on FIFOs, so a
  path can neither leave the directory nor stall even if the directory or its
  ancestors change while the view loads.
  Elsewhere a path must stay inside the directory once symbolic links are
  resolved. A mismatch fails before anything is drawn. Nothing else is read
  from disk and nothing is fetched.
- Unknown and repeated keys are errors, so a misspelled or duplicated field
  fails rather than being silently ignored or overridden. A camera takes only
  its own projection's tile keys.
- View files are capped at 64 MiB, and texture files at 16 MiB each and
  256 MiB together; reads stop at the cap even if a file grows while it is
  read. A view holds at most 2,097,152 JSON values, counted before the file is
  parsed, and lists at most 4,096 textures, 131,072 objects, 1,024 frames per
  animation and 262,144 animation frames in total, 4,194,304 height-field
  cells, and 131,072 quads: every quad the objects and their double-drawn
  repeats emit (16 for a bent isometric tile, 2 for a glowing object),
  counted before off-screen objects are culled. Counts are checked before
  their entries are read. Each image is at most 8,192 px per side and decodes
  to at most 16 MiB; all textures together decode to at most 256 MiB. Sizes
  are checked from the PNG header before decoding. The GPU device may impose
  a lower bound, which is reported as an error. The compiled
  scene must also fit the default scene limits, including 268,435,456 raster
  samples of overdraw and glow work, charged for every object as if it were
  in view.
- PNG input may be RGBA, RGB, greyscale, greyscale with alpha, or indexed;
  16-bit channels are reduced to 8 bits. Text and ICC metadata chunks are
  skipped unread, so no compressed metadata is inflated.
- Every object is validated whether or not it is in view, and every animation
  frame whether or not the clock selects it, so panning or advancing the clock
  never turns a valid view into an invalid one. Object IDs are nonzero and
  unique, and each object names exactly one of `texture` and `animation`.
  Ground objects take no `offset`, `z`, `scale`, `rotation` or `glow`,
  isometric ground keeps the 1 × 1 footprint, and no ground footprint covers
  a tile whose height is `null`. A glow radius is at most the width and height
  of every frame of its object. `double_draw` is for opaque art: it is
  rejected together with an `alpha` below 1 or a `glow`, and translucent
  texels inside double-drawn art still composite twice by design.
  Every quad an object produces, after zoom, scale, rotation, glow padding and
  terrain bending, spans at most 8,191 px on each axis.

## Fields

```jsonc
{
  "format": "tilewright.view/1",      // required
  "revision": 1,                      // default 1
  "clock_millis": 0,                  // drives animation and glow pulses
  "camera": {
    "projection": "isometric",        // or "top_down"
    "tile_width": 48,                 // isometric only
    "tile_height": 24,                // isometric only
    "tile_size": 32,                  // top_down only
    "center": [24.0, 24.0],           // tile coordinates at the viewport centre
    "zoom": 1.0,                      // default 1
    "width": 1024,                    // output size in pixels
    "height": 768,
    "sampling": "nearest"             // or "linear"
  },
  "clear_color": [0.0, 0.0, 0.0, 1.0],
  "stretch": {
    "enabled": true,                  // default true
    "rule": "mean",                   // "mean", "max" or "min"
    "cliff_threshold": 24,            // optional
    "pixels_per_unit": 1.0            // screen pixels per height unit
  },
  "ground_layer_ranks": 3,            // ranks below this are ground layers
  "heights": {                        // optional
    "origin": [0, 0],
    "width": 48,
    "height": 48,
    "values": [0, 0, 8, null]         // row-major; null means no ground there
  },
  "textures": [
    {"id": 1, "path": "art/grass.png", "sha256": "…64 hex digits…"}
  ],
  "objects": [
    {
      "id": 1,                        // semantic ID reported in picking
      "tile": [3, 4],                 // head tile
      "rank": 1,                      // render-layer rank
      "anchor": "ground",             // "ground" or "standing"
      "texture": 1                    // or "animation" below
    },
    {
      "id": 2, "tile": [5, 5], "rank": 3, "anchor": "standing",
      "animation": {"frames": [1, 2], "frame_millis": 200, "phase_millis": 0},
      "footprint": [2, 1],            // default [1, 1]
      "sub_layer": 0,
      "offset": [0.0, -4.0],          // art pixels
      "z": 6,                         // extra height units
      "scale": [1.0, 1.0],
      "rotation": 0.0,                // degrees clockwise
      "alpha": 1.0,
      "tint": [1.0, 1.0, 1.0],
      "grayscale": 0.0,
      "glow": {"color": [1.0, 0.6, 0.1, 0.9], "radius": 3.0, "pulse_millis": 1000},
      "double_draw": false,
      "pickable": true
    }
  ]
}
```

The comments above are for this document only; view files are strict JSON.
See [architecture](ARCHITECTURE.md#world-compiler) for how projection,
stretching, draw order, and effects are applied.
