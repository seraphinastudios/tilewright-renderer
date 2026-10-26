#!/usr/bin/env bash
# Validates the browser package: strict Clippy for both wasm32 bundles and
# the test hooks, the package build, and the headless Chromium tests for
# WebGL2 and WebGPU, context loss and traps. Set TILEWRIGHT_REQUIRE_WEBGPU=0
# to skip, rather than fail, the WebGPU test where the browser offers no
# WebGPU adapter.
set -euo pipefail

repository=$(git rev-parse --show-toplevel)
cd "${repository}"

for features in "" "--no-default-features" "--features test-hooks"; do
  # shellcheck disable=SC2086 # an empty feature set adds no argument
  cargo clippy --locked --target wasm32-unknown-unknown --lib \
    --package tilewright-render-wgpu --package tilewright-render-web ${features} \
    -- -D warnings
done

# The trap tests need the test hooks, and fail without them when this is set.
export TILEWRIGHT_WEB_TEST_HOOKS=1
tools/build-web.sh
(cd packages/tilewright-render && TILEWRIGHT_REQUIRE_WEBGPU="${TILEWRIGHT_REQUIRE_WEBGPU:-1}" npm test)
