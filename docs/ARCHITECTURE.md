# Architecture and limits

Dependencies point inward toward contracts:

```text
scene <- resources <- render-api <- testkit
  ^          ^            ^          ^
  |          |            |          |
  +----------+------------+------ render-wgpu <- tilewright-render
  +-----------------------+------ render-sdl3
  +------------------------------ render-ui
```

`tilewright-scene` owns immutable, versioned, sorted render inputs. `render-api`
owns targets, lifecycle errors, output planes, and a renderer trait. Resource
providers receive explicit content identity, revision, SHA-256 digest, and byte
budget; this grants no ambient path or URL access. The provider is deliberately
a synchronous ready-cache lookup: consumers perform filesystem/network work
before submission, so rendering never nests an async executor or waits for I/O.
`render-testkit` supplies the
fake provider and exact CPU reference. wgpu device/queue/pipeline/texture types
remain private to `render-wgpu`; SDL owns presentation only. Consumers translate
their state into these contracts through adapters in their own repositories.

Default limits are 65,536 sprites, 8,192 pixels on either axis, 16,777,216 total
pixels, 8× scale, and 16 MiB per resource request. All float inputs must be
finite; color channels are within 0–1; zero identities/revisions and all-zero
digests are rejected. Semantic IDs are unique. Sorting is stable by layer,
depth, painter order, then semantic ID. Rectangles use half-open pixel coverage.

CPU work is O(pixels + sprites), GPU vertex generation is O(sprites), and GPU
readback is O(pixels). A completed frame owns 11 bytes per pixel across RGBA,
semantic, depth, and coverage planes. The GPU path additionally owns a color
target, private depth/stencil target, at most 255 bytes of readback row padding
per row, and 240 bytes of transient vertices per sprite. Checked arithmetic
precedes every size-derived allocation. Offscreen targets/readback buffers are
reused until resize; immutable textures are reused by ID/revision/digest with a
hard 4,096-entry cache that clears as one bounded unit before admitting a new
working set. Retained texture pixels and each scene's distinct texture working
set are also capped at 256 MiB, and prepared sprites share decoded immutable
pixels instead of cloning them. The shared vertex buffer grows geometrically to the next power of
two and is reused by offscreen and surface presentation until a larger scene is
submitted; per-texture nearest/linear bind groups are created only on cache
misses.

No public dependency or type points to a client, editor, protocol, server, or
game implementation. `tools/check-architecture.sh` validates exact direct
crate edges from Cargo metadata and rejects forbidden sibling sources.

## Compatibility and cross-repository checklist

All eight crates are one compatibility set: they share one SemVer version and
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
- SDL/window/lifecycle changes require client and editor platform-shell review.
- UI/text contracts require renderer, client UI, editor UI, font asset license,
  and corpus review.
- Pure consumer session/document adaptation remains in that consumer repository
  and must not add a reverse renderer dependency.
