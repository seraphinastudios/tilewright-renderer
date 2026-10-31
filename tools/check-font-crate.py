#!/usr/bin/env python3
"""Verify the public UI crate contains font metadata and no Unicode payloads."""
# Copyright 2026 Seraphina Studios. SPDX-License-Identifier: MIT
import argparse
from pathlib import Path
from font_pack import MAX_MANIFEST, read_regular, verify_crate

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('archive', type=Path)
parser.add_argument('--manifest', type=Path, default=Path('assets/unicode/manifest.json'))
args = parser.parse_args()
verify_crate(args.archive, read_regular(args.manifest.absolute(), MAX_MANIFEST))
