#!/usr/bin/env python3
"""Prepare the separate reproducible Unicode font archive without credentials."""
# Copyright 2026 Seraphina Studios. SPDX-License-Identifier: MIT
import argparse
import json
from pathlib import Path
from font_pack import MANIFEST_SHA256, PACK_ID, archive, components, read_regular, MAX_MANIFEST

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--root', type=Path, default=Path('.'))
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
root = args.root.resolve(strict=True)
output = args.output.resolve(strict=True)
manifest = read_regular(root / 'assets/unicode/manifest.json', MAX_MANIFEST)
record = archive(root / 'assets/unicode', output / (PACK_ID + '.tar.gz'), manifest, MANIFEST_SHA256)
with (output / 'unicode-font-pack-provenance.json').open('x') as stream:
    json.dump({'schema_version': 1, **record}, stream, sort_keys=True, indent=2)
    stream.write('\n')
with (output / 'unicode-font-pack-sbom.cdx.json').open('x') as stream:
    json.dump({'bomFormat': 'CycloneDX', 'specVersion': '1.6', 'version': 1,
               'components': components(record)}, stream, sort_keys=True, indent=2)
    stream.write('\n')
print(json.dumps(record, sort_keys=True))
