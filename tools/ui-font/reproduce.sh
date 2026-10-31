#!/bin/sh
# Copyright 2026 Seraphina Studios
# SPDX-License-Identifier: MIT
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
output=$(mktemp -d "${TMPDIR:-/tmp}/tilewright-ui-font.XXXXXXXX")
trap 'rm -rf -- "$output"' EXIT HUP INT TERM
image=sha256:7abe7338937bc6609ebae586eb66e7a79ff8667d19f62cb1ed212cc1fbf7e13f
# No implicit pull, packages, ambient fonts, credentials, network or Docker socket.
docker image inspect "$image" >/dev/null
mkdir "$output/one" "$output/two"
for attempt in one two; do
    docker run --rm --pull never --network none --read-only --cpus 1 --memory 2g \
        --pids-limit 64 --user "$(id -u):$(id -g)" --cap-drop ALL \
        --security-opt no-new-privileges --entrypoint python3 \
        --mount "type=bind,src=$root/tools/ui-font,dst=/generator,readonly" \
        --mount "type=bind,src=$root/crates/tilewright-render-ui/assets,dst=/source,readonly" \
        --mount "type=bind,src=$output/$attempt,dst=/output" \
        "$image" /generator/generate.py --source-dir /source --output-dir /output
done
for file in atlas.atx solid.atx metrics.json glyphs.rs source-cmap.json manifest.json; do
    cmp "$output/one/$file" "$output/two/$file"
    cmp "$output/one/$file" "$root/crates/tilewright-render-ui/assets/$file"
done
PYTHONDONTWRITEBYTECODE=1 python3 "$root/tools/ui-font/check.py"
printf '%s\n' 'Two independent offline generations match each other and the checked-in assets byte for byte.'
