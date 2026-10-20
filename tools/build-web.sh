#!/usr/bin/env bash
# Builds the browser package: both WebAssembly bundles, their wasm-bindgen
# glue, and the TypeScript wrapper. Prints each bundle's raw and gzipped
# size. Requires the wasm32-unknown-unknown target, a wasm-bindgen CLI of
# exactly the locked wasm-bindgen version, and Node with npm.
set -euo pipefail

repository=$(git rev-parse --show-toplevel)
cd "${repository}"

package=packages/tilewright-render
metadata=$(mktemp /tmp/tilewright-renderer-web-metadata.XXXXXX)
trap 'rm -f -- "${metadata}"' EXIT
cargo metadata --locked --format-version 1 >"${metadata}"
locked=$(jq -r '[.packages[] | select(.name == "wasm-bindgen") | .version] | unique | .[]' \
  "${metadata}")
installed=$(wasm-bindgen --version | cut -d' ' -f2)
if [ "${installed}" != "${locked}" ]; then
  echo "wasm-bindgen CLI ${installed} does not match the locked crate ${locked}" >&2
  exit 1
fi
target_directory=$(jq -r .target_directory "${metadata}")
wasm="${target_directory}/wasm32-unknown-unknown/web/tilewright_render_web.wasm"

# `full` draws with WebGPU or WebGL2; `webgpu` leaves WebGL2 out.
for bundle in full webgpu; do
  features=()
  if [ "${bundle}" = webgpu ]; then
    features=(--no-default-features)
  fi
  cargo build --locked --profile web --target wasm32-unknown-unknown \
    --package tilewright-render-web "${features[@]}"
  rm -rf -- "${package:?}/wasm/${bundle}"
  wasm-bindgen --target web --out-dir "${package}/wasm/${bundle}" "${wasm}"
done

(cd "${package}" && npm ci --ignore-scripts --no-audit --no-fund && npm run build)

for bundle in webgpu full; do
  file="${package}/wasm/${bundle}/tilewright_render_web_bg.wasm"
  printf 'bundle=%s wasm_bytes=%s gzip_bytes=%s\n' "${bundle}" \
    "$(wc -c <"${file}")" "$(gzip -9 --no-name --stdout "${file}" | wc -c)"
done
