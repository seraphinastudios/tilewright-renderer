# crates.io delivery

The public renderer family is `tilewright-scene`, `tilewright-render-resources`,
`tilewright-render-api`, `tilewright-render-ui`, `tilewright-render-world`,
`tilewright-render-testkit`, `tilewright-render-wgpu`, and
`tilewright-render-sdl3`. The policy allowlist in
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
run and attempt. In that producing attempt, the jobs that built and attested
the evidence (`Prepare renderer release without publication credentials` and
`Attest verified renderer source release manifest`) must have succeeded once
the attempt completes. The later `Release renderer` job only publishes the
release and dispatches the publisher, so its failure after publication does not
invalidate the evidence. Unrelated successful runs cannot authorize publication.

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

All eight names require an owner-run initial crates.io publication with a
narrowly scoped local API token, using the verified source-release archives.
Configure each crate's GitHub trusted publisher on crates.io with repository
owner `seraphinastudios`, repository `tilewright-renderer`, workflow
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

### Owner bootstrap

`tools/crate_release.py bootstrap` performs the initial publication from the
owner's terminal, never from Actions. It needs:

- a clean checkout of `main` that includes the reviewed bootstrap tooling;
- GitHub CLI 2.68 or newer, signed in as the owner with push access to this
  repository. The tool refuses older versions: `gh attestation verify` checks
  the manifest attestation, and the owner session reserves the upload intents;
- a crates.io API token with the `publish-new` scope from the owner account,
  typed at the hidden prompt and never placed in the environment. Its expiry
  must outlast the paced run described below.

For the v1.14.0 source release:

```sh
python3 tools/crate_release.py bootstrap \
  --release-id 418808670 \
  --revision 8f02a371066e002357b29fed94fb16a926b3a977 \
  --manifest-sha256 fadd48beb920aeaa5cca5a1d4999f719e6a8858df813ee5bbe82c9fc4db20875 \
  --expected-owner zoeyrose \
  --expected-owner-id 437663 \
  --burst 5
```

`--expected-owner` and `--expected-owner-id` are the owner's crates.io login
and numeric user ID, as shown by `https://crates.io/api/v1/users/LOGIN`. The
tool verifies the release evidence, lists the absent crates and asks the owner
to type `PUBLISH AS zoeyrose` before it reads the GitHub session or the token.
Crates already published with matching checksums and ownership are skipped, so
a later run continues where an earlier one stopped.

### New-crate rate limit

crates.io lets an account publish 5 new crate names in a burst, then one more
every 10 minutes (`LimitedAction::PublishNew` in rust-lang/crates.io
`src/rate_limiter.rs`). A refused upload would strand the intent reserved for
it, so the bootstrap paces itself before each reservation. It uploads
the first `--burst` crates (default 5) at once. Before each further crate it
waits 610 seconds, one refill period plus a 10-second margin, and prints the
remaining time. With the default burst, eight new names take a little over
30 minutes.

A resumed run accounts for its own release: for each crate of this release
that is already published, the bootstrap reads its creation time from crates.io
and lowers the burst by those created in the last 50 minutes (five refill
periods plus the margin). An unreadable creation time counts as recent. The
bootstrap cannot see other new names this account published, for example
through another repository's bootstrap. Lower `--burst` for those yourself: use
5 minus that count, or `--burst 0` to wait before every upload. If `--burst` is
too high, crates.io refuses the upload with HTTP 429 and the run stops as
described below.

Once a run has sent an upload, every stop, Ctrl-C included, ends with advice to
pass `--burst 0` to a re-run within the next hour. A completed run gives the
same advice for any other bootstrap with this account.

### Stopped bootstrap uploads

The bootstrap never removes an intent. When an upload fails it stops, keeps
the intent and prints the recovery for that crate:

- **HTTP 429 (definite rejection).** crates.io checks its rate limits before
  it reads the archive, and it publishes nothing for a 429. The bootstrap prints
  the retry time from crates.io's `Retry-After` header or its "try again
  after" detail. After that time, confirm the crate is still absent:
  `https://crates.io/api/v1/crates/NAME` returns 404 and the crates.io
  dashboard does not list it. Then remove the intent with the owner's GitHub
  session and re-run with `--burst 0`:

  ```sh
  gh api --method DELETE \
    repos/seraphinastudios/tilewright-renderer/git/refs/tags/crate-upload-intent/NAME/VERSION
  ```

- **No definite result** (timeout, connection loss, another error status or
  an unacknowledged response). The bootstrap checks registry reads for about a
  minute and continues if they confirm the crate. Otherwise the ambiguous-result
  rule above applies: keep the intent until crates.io authoritatively confirms
  that it accepted no write, for example through crates.io support.
- **Acknowledged but not yet visible.** Keep the intent, because a published
  coordinate keeps its reservation permanently, and re-run once the crate
  appears.

### Activation order

`publish-crate.yml` and `reconcile-crates.yml` have been disabled since
2026-10-17. While the publisher is disabled, the final `dispatch` step of every
release that owns new coordinates fails after the release is published, as it
did for v1.12.0, v1.13.0 and v1.14.0. Their prepare and attest jobs succeeded,
so their evidence remains valid. Trusted Publishing cannot create crate names,
so the publisher cannot publish the first versions anyway. Activate in this
order:

1. Merge the bootstrap tooling and the exclusions below to `main`. Every merge
   to `main` starts a minor source release. Until v1.14.0's 0.3.0 crates exist
   on crates.io, its prepare job stops with "owned crate registry readiness
   unresolved". That job has read-only repository permissions and stops
   before attestation, tagging and release, so the failed run publishes
   nothing.
2. The owner runs the bootstrap for v1.14.0, as shown above, from a clean
   checkout of that `main`.
3. The owner configures each crate's GitHub trusted publisher on crates.io:
   repository owner `seraphinastudios`, repository `tilewright-renderer`,
   workflow `publish-crate.yml` and environment `crates-io-release`. crates.io
   records the repository owner's GitHub ID along with the owner and
   repository names, so renaming or transferring the repository or the
   organization requires reconfiguring the publishers.
4. The owner re-enables `publish-crate.yml` and `reconcile-crates.yml`. This
   is a repository Actions setting that only the owner changes; no code or
   agent re-enables them.

From then on, merges release normally. A release that changes no packaged
crate content reuses the published archives, owns no new coordinates and
dispatches nothing. A release that changes packaged content bumps the Cargo
version and owns new coordinates; v1.15.0's 0.4.0 is the first after v1.14.0.
While the publisher is disabled, such a release's dispatch fails after the
release is published, and once it is active, reconciliation publishes those
releases, one per run. Later Cargo version bumps publish automatically.
crates.io's first version, 0.3.0, and 0.4.0 keep wgpu's default Direct3D 12
shader compiler, which prefers `dxcompiler.dll`; the renderer selects FXC from
0.4.1 (see the [security policy](../SECURITY.md#native-libraries-on-windows)).

### Excluded source releases

The owner decided that crates.io starts at 0.3.0. v1.12.0 (0.2.0) and v1.13.0
(0.2.1) own crate versions that were never published and must never be. The
`excluded` list in `policy/crate-release.json` records each such release by
source tag, release ID and Cargo version, and changes only through review.

- Reconciliation skips an excluded release and never dispatches it.
- Release evidence, and therefore `verify`, `confirm`, `upload` and the owner
  bootstrap, refuses an excluded release before reading anything else. It also
  refuses any manifest that contains an excluded coordinate: a policy crate at
  an excluded version.
- Every intent reservation refuses an excluded coordinate, so neither the
  publisher nor the bootstrap can reach a registry PUT for one.
- Anything inconsistent fails closed. That covers a tag or release ID that
  matches an entry only partly, an excluded release whose crates are at
  another version, and an excluded coordinate owned by another release.

Excluded releases stay in the source catalog, so their coordinates remain
owned and no later release can claim them.
