#!/usr/bin/env bash
set -euo pipefail

repository=$(git rev-parse --show-toplevel)
cd "${repository}"

test "$(rustc --version | cut -d' ' -f2)" = 1.97.1
python3 -m unittest tools.test_crate_release
python3 tools/crate_prepare.py check-version --base "${CRATE_RELEASE_BASE:-$(git rev-parse HEAD)}"
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets --all-features
cargo test --locked --workspace --doc
cargo build --locked --release --package tilewright-render --features sdl3
cargo run --locked --quiet --package tilewright-render -- --version

tools/check-architecture.sh
tools/check-dependencies.sh
tools/test-check-dependencies.sh
tools/check-provenance.sh
jq empty corpus/*.json policy/*.json

corpus_actual=$(mktemp /tmp/tilewright-renderer-corpus.XXXXXX)
proof_output=$(mktemp /tmp/tilewright-renderer-proof.XXXXXX)
proof_error=$(mktemp /tmp/tilewright-renderer-proof-error.XXXXXX)
rm -f -- "${proof_output}"
trap 'rm -f -- "${corpus_actual}" "${proof_output}" "${proof_error}"' EXIT
cargo run --locked --quiet --package tilewright-render-testkit --example corpus \
  >"${corpus_actual}"
diff -u \
  <(jq -S '{schema_version,cases}' corpus/manifest.json) \
  <(jq -S . "${corpus_actual}")

cargo run --locked --quiet --package tilewright-render -- probe
cargo run --locked --quiet --package tilewright-render -- corpus
SDL_VIDEO_DRIVER=x11 xvfb-run -a -s '-screen 0 1024x768x24' \
  cargo run --locked --quiet --package tilewright-render --features sdl3 -- window
cargo run --locked --quiet --package tilewright-render -- offscreen "${proof_output}"
test "$(wc -c <"${proof_output}")" -eq 16407
if cargo run --locked --quiet --package tilewright-render -- offscreen "${proof_output}" \
  2>"${proof_error}"; then
  echo "offscreen proof overwrote an existing output" >&2
  exit 1
fi
grep -F "without overwriting" "${proof_error}" >/dev/null

cargo check --locked --workspace --target x86_64-pc-windows-gnu \
  --exclude tilewright-render-sdl3

release_output=$(mktemp -d /tmp/tilewright-renderer-release.XXXXXX)
rmdir "${release_output}"
tools/package-release.sh "${release_output}"
test -s "${release_output}/SHA256SUMS"
rm -rf -- "${release_output}"

git diff --check
