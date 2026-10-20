#!/usr/bin/env bash
set -euo pipefail

repository=$(git rev-parse --show-toplevel)
cd "${repository}"

test -s PROVENANCE.md
test -s THIRD_PARTY_NOTICES.md
jq -e '
  .schema_version == 1
  and (.provenance | contains("synthetic MIT"))
  and (.failure_cases | length == 5)
  and all(.failure_cases[]; .name != "" and .expected != "" and .owner == "renderer")
  and (.resources | length == 8)
  and (.resources | map(.name) | length == (unique | length))
  and (.resources | map(.resource_id) | length == (unique | length))
  and all(.resources[];
    (.name | type == "string" and length > 0)
    and (.resource_id | type == "number" and . > 0 and . == floor)
    and (.revision | type == "number" and . > 0 and . == floor)
    and (.width | type == "number" and . > 0 and . == floor)
    and (.height | type == "number" and . > 0 and . == floor)
    and (.sha256 | type == "string" and test("^[0-9a-f]{64}$"))
    and (.source | type == "string" and startswith("tilewright-render-testkit "))
    and .author == "Seraphina Studios"
    and .license == "MIT"
    and (.transformation | type == "string" and length > 0)
    and (.notice | type == "string" and length > 0))
  and (.cases | length == 7)
  and (.cases | map(.name) | length == (unique | length))
  and all(.cases[];
    .width > 0 and .height > 0
    and (.clock_millis | type == "number" and . >= 0 and . == floor)
    and .maximum_rgba_channel_difference == 1
    and .performance.maximum_cpu_micros > 0
    and .performance.quad_count > 0
    and .performance.resource_requests > 0
    and .performance.uploaded_bytes > 0
    and .performance.vertex_count == (.performance.quad_count * 6)
    and .performance.target_allocation_bytes > 0
    and all(.digests[]; test("^[0-9a-f]{64}$")))
' corpus/manifest.json >/dev/null

if rg -ni '(classic[/ -](client|editor|renderer)|\b(a?gpl)-[123])' \
  crates corpus --glob '!*.lock'; then
  echo "unexpected classic or incompatible-license reference in implementation" >&2
  exit 1
fi
