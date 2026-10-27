# Tilewright renderer repository guide

## Ownership and architecture

- This repository owns the MIT shared Rust GPU renderer for isometric and
  top-down tile worlds: game clients, the Tilewright editor, deterministic
  offscreen tools, and world imagery.
- Renderer APIs consume bounded renderer-owned scenes, resources, targets,
  cameras, clocks, and materials; they remain source-neutral. `scene` and
  `render-api` must not depend on SDL3, wgpu, protocol, consumers, or content
  syntax. Backend GPU objects remain private.
- The SDL3 bridge owns only surface/raw-window integration, not application
  events, input, audio, client lifecycle, or editor workflows. Headless crates
  must not acquire native/GPU dependencies accidentally.
- Never depend on client/editor/protocol or content parsers. Consumers own
  state/document-to-scene adapters and authorization; rendering never decides
  gameplay visibility, collision, targeting, or disclosure.

## Rendering, resource, and lifecycle invariants

- Use one device/frame-graph/material/resource architecture for windows,
  embedded viewports, and offscreen targets. Do not create consumer-specific or
  scenery-only renderers.
- Preserve deterministic ordered scenes across neighboring tiles/depths,
  including tall/multipart objects, structural geometry, transparency,
  lighting, effects, overlays, clipping, and pixel-art sampling.
- Color snapshots may use measured backend tolerances. Semantic identity,
  painter/depth, coverage, and visibility masks are exact and own picking,
  selection, disclosure, and robust cross-backend tests.
- Resource providers accept immutable IDs/digests/revisions, declared limits,
  and bounded asynchronous bytes. They never grant ambient filesystem/network
  access. Keep CPU, staging, and GPU caches separately budgeted and precisely
  invalidated.
- Treat scene/resource/material/shader data as untrusted. Validate revisions,
  dimensions, counts, coordinates, dependency graphs, and byte sizes before
  publishing frame state. A server can never provide executable shaders/code.
- Return typed outcomes for device/surface loss, resize, suspend, cancellation,
  target destruction, readback, and output failure. Recovery creates one
  coherent generation; stale handles/partial outputs cannot survive.
- Keep CLI, consumer integration, previews, imagery, and golden tests on the
  same library paths. Offscreen tools use explicit digest-verified input, no
  ambient discovery/network, and atomic output.
- Isolate wgpu/SDL unsafe code to the smallest bridge with documented lifetime
  invariants and tests; logical crates should forbid unsafe code.

## Licensing, performance, and validation

- New Rust/WGSL/tests/docs/fixtures are MIT. Do not link, bundle, depend on,
  copy, or adapt GPL/AGPL source. Independent implementation is the default;
  any reuse of outside material needs an MIT-compatible license and an exact
  record (source, revision, destination, transformation, notice) in
  [PROVENANCE.md](PROVENANCE.md) and `THIRD_PARTY_NOTICES.md`.
- Authored inputs retain exact licenses. Fixture/package manifests record
  source, author, license, digest, transformation, and notice. Shaders are
  executable renderer code authored/reviewed/versioned here, never delivered
  arbitrarily by a server.
- Pin Rust/MSRV, `Cargo.lock`, wgpu/SDL acquisition, supported backends, and
  fallback policy. Keep compatible registry requirements in `Cargo.toml`; the
  locked graph remains the exact build input. Every performance change defines
  representative before/after budgets for CPU/GPU time, uploads, allocations,
  caches, queues, and recovery. Bound metrics and keep disabled instrumentation cheap.
- Local issues and milestones own delivery.
- Run the real aggregate contract:

  ```sh
  tools/validate.sh
  git diff --check
  ```

  `Renderer validation` owns formatting, strict Clippy, tests/docs,
  architecture/shader/generated drift, dependency/license/security gates,
  consumer smoke builds, and supported platform/rendering proofs. Record
  adapter/backend/driver/limits for GPU evidence; never imply absent GPU
  coverage passed.
- `Dependency review` provides nonblocking advisory feedback on eligible ordinary
  PR runs where this workflow exists on the relevant revision, with no target-branch
  filter, across runtime, development, and unknown scopes at every severity.
  Implementing agents must read the latest job summary for the current PR head,
  even when a PR comment exists, because comments may be stale. If that summary
  is unavailable, treat results as unknown and inspect annotations and logs.
  Fix actionable introduced risks, or record an explicit maintainer acceptance
  of existing risk, before merge. This adds no required human approval gate;
  preserve all other validation and security checks.
- Use repository validation and released consumer contracts, not unaudited
  source copies. Commits/PR titles use Conventional Commits; semantic-release
  owns coherent releases/tags.

## Registry delivery

The eight public libraries are selected by `policy/crate-release.json`; the
CLI is not a registry package. Run the packaged-content version gate and
credential-free preparation before source-release publication. Trusted publishing
uploads verified release archives without builds. The inert signer attests the
manifest; publication requires the exact attested `release.yml` workflow
attempt on `main`, completed with its prepare and attest jobs successful, and
a durable atomic upload intent before any registry PUT. Source releases in the
policy's reviewed `excluded` list never publish. See
[crate delivery](docs/CRATE-RELEASE.md); Cargo and source-release versions are
independent, and initial registry ownership requires owner activation.
