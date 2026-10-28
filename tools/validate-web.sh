#!/usr/bin/env bash
# Validates the browser package: strict Clippy for both wasm32 bundles and
# the test hooks, the package build, the headless Chromium tests for WebGL2
# and WebGPU, context loss and traps, and a short run of the frame-time
# harness. Set TILEWRIGHT_REQUIRE_WEBGPU=0 to skip, rather than fail, the
# WebGPU test and the WebGPU harness run where the browser offers no WebGPU
# adapter.
set -euo pipefail

repository=$(git rev-parse --show-toplevel)
cd "${repository}"

for features in "" "--no-default-features" "--features test-hooks"; do
  # shellcheck disable=SC2086 # an empty feature set adds no argument
  cargo clippy --locked --target wasm32-unknown-unknown --lib \
    --package tilewright-render-wgpu --package tilewright-render-web ${features} \
    -- -D warnings
done

# The trap tests need the test hooks, and fail without them when this is
# set. The package directory must never keep that build, so the bundles are
# rebuilt without the hooks on the way out, pass or fail.
rebuild_without_hooks() {
  if ! TILEWRIGHT_WEB_TEST_HOOKS=0 tools/build-web.sh >/dev/null; then
    echo "could not rebuild the bundles without test hooks; run tools/build-web.sh" >&2
    exit 1
  fi
}
trap rebuild_without_hooks EXIT
export TILEWRIGHT_WEB_TEST_HOOKS=1
tools/build-web.sh
(cd packages/tilewright-render && TILEWRIGHT_REQUIRE_WEBGPU="${TILEWRIGHT_REQUIRE_WEBGPU:-1}" npm test)

# Keep the browser frame-time harness working; its numbers are not checked.
# Without a WebGPU requirement it runs on WebGL2 only, so a browser without
# WebGPU passes here too.
bench_backend=all
if [ "${TILEWRIGHT_REQUIRE_WEBGPU:-1}" = 0 ]; then
  bench_backend=webgl2
fi
(cd packages/tilewright-render \
  && npm run bench -- --backend "${bench_backend}" --width 320 --height 180 --characters 50 \
    --frames 5 --warmup 1)
