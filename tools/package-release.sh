#!/usr/bin/env bash
set -euo pipefail

repository=$(git rev-parse --show-toplevel)
cd "${repository}"

output=${1:-dist}
version=${2:-$(git describe --tags --always --dirty)}
if [[ ! ${version} =~ ^[0-9A-Za-z][0-9A-Za-z.+-]*$ ]]; then
  echo "invalid release version: ${version}" >&2
  exit 2
fi
if [[ -e ${output} ]]; then
  echo "release output already exists: ${output}" >&2
  exit 1
fi
install -d "${output}"
target_directory=$(cargo metadata --locked --offline --no-deps --format-version 1 \
  | jq -r '.target_directory')

archive="tilewright-renderer-${version}.tar.gz"
git archive --format=tar --prefix="tilewright-renderer-${version}/" HEAD \
  | gzip -n >"${output}/${archive}"

prepare_options=()
if [[ $# -lt 2 ]]; then
  prepare_options+=(--dry-run)
  source_tag=v0.0.0
else
  source_tag="v${version}"
fi
python3 tools/crate_prepare.py prepare "${prepare_options[@]}" \
  --source-tag "${source_tag}" --output "$(realpath "${output}")" \
  --archives "$(realpath "${output}")"
cargo build --locked --release --package tilewright-render --features sdl3
cp "${target_directory}/release/tilewright-render" "${output}/"
tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner \
  -cf - corpus | gzip -n >"${output}/corpus.tar.gz"
cp LICENSE PROVENANCE.md THIRD_PARTY_NOTICES.md "${output}/"

SYFT_CHECK_FOR_APP_UPDATE=false syft dir:. \
  --source-name tilewright-renderer --source-version "${version}" \
  --output "cyclonedx-json=${output}/sbom.cdx.json"

jq -n \
  --arg version "${version}" \
  --arg revision "$(git rev-parse HEAD)" \
  --arg rust "$(rustc --version)" \
  '{schema_version:1,version:$version,revision:$revision,
    tools:{rust:$rust},backends:["Vulkan","D3D12"],sdl_major:3}' \
  >"${output}/provenance.json"

checksums=$(mktemp /tmp/tilewright-renderer-checksums.XXXXXX)
trap 'rm -f -- "${checksums}"' EXIT
(
  cd "${output}"
  find . -maxdepth 1 -type f ! -name SHA256SUMS -printf '%f\0' \
    | sort -z | xargs -0 sha256sum
) >"${checksums}"
mv "${checksums}" "${output}/SHA256SUMS"

crate_version=$(cargo metadata --locked --offline --no-deps --format-version 1 \
  | jq -er '.packages[] | select(.name == "tilewright-render-wgpu") | .version')
if [[ -f ${output}/tilewright-render-wgpu-${crate_version}.crate ]]; then
  tar -tf "${output}/tilewright-render-wgpu-${crate_version}.crate" \
    | grep -Fx "tilewright-render-wgpu-${crate_version}/src/shader.wgsl" >/dev/null
fi
