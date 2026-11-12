# @seraphinastudios/tilewright-render

The Tilewright renderer in browsers: the Rust renderer core compiled to
WebAssembly, drawing with WebGPU or WebGL2, behind a TypeScript API.

- `webgpu` bundle: WebGPU only, and small.
- `full` bundle: WebGPU and WebGL2.

`TilewrightRenderer.create` loads only the bundle it needs.

This package is not on the npm registry. Each source release of
[tilewright-renderer](https://github.com/seraphinastudios/tilewright-renderer)
carries it as the asset `seraphinastudios-tilewright-render-X.Y.Z.tgz`, built
from the tagged commit, listed in the release's `SHA256SUMS` and covered by a
build provenance attestation. The package version is the source release
version. Download and verify one release's tarball, then depend on that file:

```sh
gh release download vX.Y.Z -R seraphinastudios/tilewright-renderer \
  --pattern 'seraphinastudios-tilewright-render-X.Y.Z.tgz' --pattern SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify seraphinastudios-tilewright-render-X.Y.Z.tgz \
  -R seraphinastudios/tilewright-renderer
npm install ./seraphinastudios-tilewright-render-X.Y.Z.tgz
```

See [the browser package documentation](https://github.com/seraphinastudios/tilewright-renderer/blob/main/docs/WEB.md)
for the API, bundles, limits and verification.

## Licence

MIT; see `LICENSE`. The WebAssembly bundles link third-party Rust crates and
Unicode data under the licences given in `THIRD_PARTY_NOTICES.md`.
`OFL-RobotoMono.txt` is the SIL Open Font License notice of the renderer's
built-in UI font data.
