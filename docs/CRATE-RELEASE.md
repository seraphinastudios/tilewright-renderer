# crates.io delivery

The public renderer family is `tilewright-scene`, `tilewright-render-resources`,
`tilewright-render-api`, `tilewright-render-ui`, `tilewright-render-testkit`,
`tilewright-render-wgpu`, and `tilewright-render-sdl3`. The policy allowlist in
`policy/crate-release.json` is authoritative. `tilewright-render` stays a GitHub
application artifact and has `publish = false`.

Only source releases from `main` may own registry coordinates. Maintenance
branches still prepare and verify the complete package graph, then omit the
selected crate archives and emit an empty publication manifest. Their source
and application releases continue independently without reserving Cargo
versions for the main-only publisher.

Cargo packages share the workspace version, initially `0.1.0`. This version is
independent of the semantic-release source tag. Change the workspace version and
all exact internal dependency requirements together whenever packaged content
changes after publication. Required validation compares packaged content with
the accepted base and rejects changed content at an unchanged published version.
For already published coordinates, preparation packages each candidate separately
against the production registry twice. It compares the complete inventory and
canonical contents, including `Cargo.lock`, with the original owning archive;
only the candidate's VCS revision is excluded from that content comparison.
The original archive must match its owning source provenance and the registry
API, index and downloaded bytes before reuse. Missing, conflicting or unresolved
registry state stops preparation.

The standalone graph verifier receives those exact original archives alongside
twice-reproduced new packages. Only new coordinates enter Cargo's packaging
overlay, so a source-only commit cannot replace published dependency checksums
with checksums of regenerated archives. Existing coordinates remain owned by
the original release and are omitted from the new manifest and assets. A genuine
content or lockfile change at an existing version still requires a version bump.
Explicit `--source-package NAME` selections require `publish = false`; they join
the new package graph for verification and remain source assets without registry
ownership. Maintenance and offline validation continue to verify their complete
current graph without consulting main's coordinate ownership.

`tools/crate_prepare.py` runs without registry credentials. It prepares the
allowlisted Cargo archives, checks their normalized manifests and dependency
order, verifies standalone packages, and writes `crate-release.json` with the
source revision, source tag, package inventory and exact SHA-256 checksums.
The source release attaches each newly owned package archive, this manifest,
source provenance and checksums before dispatching the publisher. The manifest selects only new
crate versions; unchanged versions retain their original owning release. Preserve the CLI, corpus, SBOM and
source archive assets in that release.

`publish-crate.yml` runs from protected `main`. Its `automatic` dispatch accepts
the exact source release ID, full source revision and manifest digest. The
publisher checks release/tag/repository identity, verifies every asset and
normalized dependency record, exchanges GitHub OIDC in `crates-io-release`, and
uploads the stored archives directly to crates.io. It runs no Cargo build or
package hook. Source and application builds run first in a separate read-only
job without publication credentials; the source-release finalizer verifies the
prepared revision, source version and checksums before creating a tag. Dependencies must become visible with matching
checksums in the sparse index before dependent uploads proceed. Existing exact
packages are idempotent; a conflicting checksum stops publication. Hourly
reconciliation revisits eligible releases to recover interrupted dispatches or
index propagation delays.

## Immutable source-release ownership

Repository governance enables immutable GitHub releases before activation. The
publisher and owner bootstrap inspect the public source release and require
`immutable: true` before any registry write. The source finalizer verifies the
prepared assets without querying administrator-only repository settings. If
immutable releases were misconfigured, a mutable source release can exist, but
its crate publication fails closed. Preserve that release and its evidence for
explicit owner review; do not automatically delete, rewrite or replace assets.

## Attested evidence and durable upload intent

A separate signer job verifies the complete prepared payload, then uses the
pinned official `actions/attest-build-provenance` action to attest the exact
`crate-release.json` bytes. It runs no Cargo or package hook and holds no release
write token. The source finalizer waits for this signer before creating the
source release.

Before publication, the helper retrieves bounded official attestation bundles,
including GitHub's compressed storage responses, without forwarding credentials
to storage. The preinstalled official GitHub CLI (2.68 or newer) verifies their
signatures in a fresh configuration without registry or GitHub write credentials.
Certificate checks bind the subject digest, repository ID, protected `main`,
source and workflow revisions, GitHub-hosted runner, and exact source workflow
run and attempt. That producing invocation must succeed; unrelated successful
runs cannot authorize publication.

Before each registry PUT, the publisher atomically creates and reads back
`refs/tags/crate-upload-intent/NAME/VERSION`, recording the archive and manifest
digests, source release, revision and verified producer. A preexisting intent
blocks another automatic attempt even when public registry reads still report
absence. The publisher's contents-write permission serves this reservation;
`GH_INTENT_TOKEN` is exposed only to the upload step and is stripped before
Cargo or signature-verifier subprocesses.

Retain an intent after ambiguous upload results. The owner must establish whether
the registry accepted the write; public absence alone is insufficient. Only
explicit owner recovery after authoritative confirmation of no accepted write
may remove the reservation and authorize another attempt. A matching published
coordinate retains its reservation permanently. Bootstrap uses the same gate and
a separate authenticated owner GitHub session for the intent; the crate token
never reaches the GitHub CLI.

## Initial ownership and activation

All seven names require an owner-run initial crates.io publication with a
narrowly scoped local API token, using the verified source-release archives.
Configure each crate's GitHub trusted publisher with this repository ID,
`publish-crate.yml` and environment `crates-io-release`. Keep the token out of
GitHub Actions, logs and committed files. Repository code prepares this rollout;
merging a candidate does not itself grant registry ownership or configure a
trusted publisher. The coordinator must verify initial versions and exact
checksums before activating automatic publication.

For a local preparation, run the pinned credential-free build image and use an
isolated Cargo cache:

```sh
python3 tools/crate_prepare.py check-version --base ACCEPTED_BASE_SHA
python3 tools/crate_prepare.py prepare --source-tag vSOURCE_VERSION --output /tmp/renderer-crates
```

For recovery, dispatch the existing eligible release through the same workflow
with its exact release ID, revision and manifest checksum. Never rebuild or
replace release archives to recover a failed upload. Correct conflicting
packages through a new reviewed Cargo version and source release.
