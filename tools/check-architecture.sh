#!/usr/bin/env bash
set -euo pipefail

repository=$(git rev-parse --show-toplevel)
cd "${repository}"

metadata=$(mktemp /tmp/tilewright-renderer-architecture.XXXXXX)
actual=$(mktemp /tmp/tilewright-renderer-dependencies.XXXXXX)
trap 'rm -f -- "${metadata}" "${actual}"' EXIT
cargo metadata --locked --offline --all-features --format-version 1 >"${metadata}"

jq -S '
  . as $metadata
  | [$metadata.resolve.nodes[]
    | select(.id | startswith("path+file://"))
    | . as $node
    | ($metadata.packages[] | select(.id == $node.id) | .name) as $name
    | select($name | startswith("tilewright-"))
    | {key: $name,
       value: [$node.deps[].pkg as $id
         | $metadata.packages[] | select(.id == $id) | .name] | sort}]
  | from_entries
' "${metadata}" >"${actual}"

diff -u \
  <(jq -S '.crate_dependencies' policy/architecture.json) \
  "${actual}"

jq -e --slurpfile architecture policy/architecture.json '
  all(.packages[];
    (.source // "") as $source
    | all($architecture[0].forbidden_source_patterns[];
        . as $pattern | ($source | contains($pattern) | not)))
' "${metadata}" >/dev/null

if rg -n '\b(wgpu|sdl3)::' \
  crates/tilewright-scene crates/tilewright-render-api \
  crates/tilewright-render-resources crates/tilewright-render-ui; then
  echo "renderer-neutral crates expose a backend symbol" >&2
  exit 1
fi
