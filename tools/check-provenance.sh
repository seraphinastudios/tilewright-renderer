#!/usr/bin/env bash
set -euo pipefail

repository=$(git rev-parse --show-toplevel)
cd "${repository}"

test -s PROVENANCE.md
test -s THIRD_PARTY_NOTICES.md
python3 tools/ui-font/check.py
python3 tools/unicode-fonts/check.py
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tools/unicode-fonts -p 'test_*.py'
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest tools.test_font_pack
# Registry consumers receive the same immutable manifest, never font payloads.
cmp assets/unicode/manifest.json crates/tilewright-render-ui/assets/unicode-font-pack.json
jq -e --slurpfile font crates/tilewright-render-ui/assets/manifest.json '
  .schema_version == 1
  and (.provenance | contains("synthetic MIT"))
  and (.failure_cases | length == 5)
  and all(.failure_cases[]; .name != "" and .expected != "" and .owner == "renderer")
  and (.resources | length == 10)
  and ([.resources[] | select(.resource_id != 301 and .resource_id != 302)] | length == 8)
  and ([.resources[] | select(.resource_id != 301 and .resource_id != 302) | .resource_id] | sort == [1,101,102,103,104,105,106,201])
  and (.resources | map(.name) | length == (unique | length))
  and (.resources | map(.resource_id) | length == (unique | length))
  and all(.resources[] | select(.resource_id != 301 and .resource_id != 302);
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
  and ([.resources[] | select(.resource_id == 301)] | length == 1)
  and ([.resources[] | select(.resource_id == 302)] | length == 1)
  and all(.resources[] | select(.resource_id == 301 or .resource_id == 302);
    . as $resource
    | (if .resource_id == 301 then "atlas.atx" else "solid.atx" end) as $file
    | $font[0].files[$file] as $asset
    | .revision == 1
    and (if .resource_id == 301
      then .name == "ui-roboto-mono-atlas" and .width == 512 and .height == 256 and .license == "OFL-1.1"
      else .name == "ui-solid" and .width == 1 and .height == 1 and .license == "MIT" end)
    and all(["sha256", "source", "author", "license", "transformation", "notice"][];
      . as $key | $resource[$key] == $asset[$key]))
  and (.cases | length == 9)
  and (.cases | map(.name) | length == (unique | length))
  and all(.cases[];
    .width > 0 and .height > 0
    and (.clock_millis | type == "number" and . >= 0 and . == floor)
    and .maximum_rgba_channel_difference ==
      (if .name == "structural-fog-ui" or .name == "shared-ui-dpi-125" then 2 else 1 end)
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
