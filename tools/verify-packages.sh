#!/usr/bin/env bash
set -euo pipefail

repository=$(git rev-parse --show-toplevel)
cd "${repository}"

packages=${1:?usage: verify-packages.sh CRATE_DIRECTORY}
packages=$(realpath "${packages}")
version=$(cargo metadata --locked --offline --no-deps --format-version 1 \
  | jq -er '[.packages[].version] | unique | if length == 1 then .[0]
    else error("renderer crates do not share one version") end')
verification=$(mktemp -d /tmp/tilewright-renderer-crates.XXXXXX)
trap 'rm -rf -- "${verification}"' EXIT

crates=(
  tilewright-scene
  tilewright-render-resources
  tilewright-render-api
  tilewright-render-testkit
  tilewright-render-wgpu
  tilewright-render-sdl3
  tilewright-render-ui
)

for crate in "${crates[@]}"; do
  tar -xzf "${packages}/${crate}-${version}.crate" -C "${verification}"
done

patches=()
for crate in "${crates[@]}"; do
  patches+=(
    --config
    "patch.crates-io.${crate}.path='${verification}/${crate}-${version}'"
  )
done

export CARGO_TARGET_DIR="${verification}/target"
for crate in "${crates[@]}"; do
  cargo test --quiet --offline --all-features --no-run \
    --manifest-path "${verification}/${crate}-${version}/Cargo.toml" \
    "${patches[@]}"
done
