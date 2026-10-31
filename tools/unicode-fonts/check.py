#!/usr/bin/env python3
# Copyright 2026 Seraphina Studios
# SPDX-License-Identifier: MIT
"""Offline exact-byte, provenance and scalar-coverage check of the Noto pack."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
from sfnt import inventory, require

REVISION = '6c1dbe27dcf7d817d4b62cec033ba77fa8e2a5ec'
MANIFEST_SHA256 = 'a332b35dcb2874f8517552a9ee6e3a6509e6728dfdbe71b1efbde270069aa2d8'
ROOT = Path(__file__).resolve().parents[2]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def checked_file(root, name, size, expected):
    path = PurePosixPath(name)
    require(not path.is_absolute() and path.parts and all(p not in ('.', '..') for p in path.parts), 'unsafe asset path')
    target = root
    require(not target.is_symlink(), 'symlink asset root')
    for component in path.parts:
        target = target / component
        require(not target.is_symlink(), 'symlink asset path')
    require(target.is_file() and target.stat().st_size == size, 'asset size: ' + name)
    with target.open('rb') as stream:
        data = stream.read(size + 1)
    require(len(data) == size and digest(data) == expected, 'asset checksum: ' + name)
    return data


def load_manifest(path):
    require(not path.is_symlink() and path.stat().st_size <= 256 * 1024, 'manifest bounds/symlink')
    data = path.read_bytes()
    require(digest(data) == MANIFEST_SHA256, 'pinned manifest checksum')
    manifest = json.loads(data)
    require(manifest['schema'] == 1 and manifest['revision'] == REVISION, 'manifest schema/revision')
    return manifest


def check(root):
    require(not root.is_symlink(), 'symlink asset root')
    manifest = load_manifest(root / 'manifest.json')
    fonts = manifest['fonts']
    require(len(fonts) == manifest['limits']['fonts'] == 16, 'exact font count')
    expected_files = set()
    total = notices = 0
    union = set()
    for font in fonts:
        require(font['license'] == 'OFL-1.1' and font['author'] and font['copyright'], 'font provenance')
        require(font['transformation'] == 'none; original upstream font bytes retained', 'unmodified source')
        require(font['bytes'] <= manifest['limits']['font_bytes'], 'font byte limit')
        expected_files.add(font['path'])
        data = checked_file(root, font['path'], font['bytes'], font['sha256'])
        actual = inventory(data)
        require(actual == font['inventory'], 'exact cmap and name inventory: ' + font['family'])
        require(any('SIL Open Font License, Version 1.1' in text for text in actual['name_records'].get('13', [])), 'embedded OFL license')
        scalars = {scalar for first, last in actual['ranges'] for scalar in range(first, last + 1)}
        require(all(ord(c) in scalars for c in font['coverage_sample']), 'representative script scalar coverage')
        union.update(scalars)
        notice = font['notice']
        expected_files.add(notice['path'])
        text = checked_file(root, notice['path'], notice['bytes'], notice['sha256']).decode('utf-8')
        require(text.splitlines()[0] == font['copyright'], 'notice copyright')
        header = text.split('This Font Software is licensed', 1)[0]
        require(font['reserved_font_names'] == (['Source'] if "Reserved Font Name 'Source'" in header else []), 'reserved font names')
        require('SIL OPEN FONT LICENSE Version 1.1' in text, 'full OFL notice')
        total += len(data)
        notices += notice['bytes']
    actual_files = set()
    for path in (root / 'fonts').rglob('*'):
        require(not path.is_symlink(), 'symlink in font tree')
        if path.is_file():
            actual_files.add(path.relative_to(root).as_posix())
    require(actual_files == expected_files, 'exact font/notice file set')
    require(total == manifest['total_font_bytes'] == 29197208 and total <= manifest['limits']['total_font_bytes'], 'total font byte bound')
    require(notices == manifest['total_notice_bytes'], 'total notice bytes')
    require(all(s in union for s in (0x0301, 0x0627, 0x05E9, 0x0915, 0x4E2D, 0x304B, 0x1F600, 0xFFFD)), 'core scalar coverage')
    # Demonstrate the documented finite repertoire, rather than universal coverage.
    require(0x10FFFF not in union, 'finite repertoire contract')
    return {'font_count': len(fonts), 'font_bytes': total, 'notice_bytes': notices,
            'union_scalar_count': len(union), 'manifest_sha256': MANIFEST_SHA256,
            'revision': REVISION}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--assets', type=Path, default=ROOT / 'assets/unicode')
    print(json.dumps(check(parser.parse_args().assets), sort_keys=True))
