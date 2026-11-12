#!/usr/bin/env bash
# Packs the browser package for a release: builds both bundles without the
# browser tests' hooks, stamps the version, `npm pack`s it into OUTPUT, then
# checks the tarball's file list and manifest (tools/web_package.py) and
# installs it into an empty project and draws with it in headless Chromium
# (packages/tilewright-render/test/tarball.mjs).
#
#   tools/pack-web.sh OUTPUT VERSION
#
# VERSION is the source release version X.Y.Z, or 0.0.0-check for a dry run.
# Prints the tarball's path.
set -euo pipefail

repository=$(git rev-parse --show-toplevel)
cd "${repository}"

if [[ $# -ne 2 ]]; then
  echo "usage: tools/pack-web.sh OUTPUT VERSION" >&2
  exit 2
fi
output=$(realpath "$1")
version=$2
name=$(python3 tools/web_package.py name "${version}")
package=packages/tilewright-render

# Never ship the trap hooks, whatever the caller's environment says.
TILEWRIGHT_WEB_TEST_HOOKS=0 tools/build-web.sh >&2

# Pack a copy, so the version stamp and notices never touch the tree.
staging=$(mktemp -d "${TMPDIR:-/tmp}/tilewright-render-pack.XXXXXX")
trap 'rm -rf -- "${staging}"' EXIT
cp -R "${package}/package.json" "${package}/README.md" "${package}/dist" "${package}/wasm" \
  "${staging}/"
cp LICENSE THIRD_PARTY_NOTICES.md "${staging}/"
cp crates/tilewright-render-ui/assets/OFL.txt "${staging}/OFL-RobotoMono.txt"
jq --arg version "${version}" '.version = $version' "${package}/package.json" \
  >"${staging}/package.json"
(cd "${staging}" && npm pack --ignore-scripts --silent --pack-destination "${output}" >/dev/null)

python3 tools/web_package.py check "${output}/${name}" --version "${version}"
(cd "${package}" && node test/tarball.mjs "${output}/${name}") >&2
echo "${output}/${name}"
