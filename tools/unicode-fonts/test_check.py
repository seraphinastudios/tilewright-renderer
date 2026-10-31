# Copyright 2026 Seraphina Studios
# SPDX-License-Identifier: MIT
import hashlib
from pathlib import Path
import struct
import tempfile
import unittest
from check import ROOT, check, checked_file, load_manifest
from sfnt import inventory


class FontPackTests(unittest.TestCase):
    def test_complete_offline_inventory(self):
        result = check(ROOT / 'assets/unicode')
        self.assertEqual(result['font_count'], 16)
        self.assertEqual(result['union_scalar_count'], 37236)

    def test_altered_manifest_rejected_before_paths_are_used(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'manifest.json'
            path.write_text('{"fonts":[{"path":"../../outside"}]}')
            with self.assertRaisesRegex(ValueError, 'pinned manifest checksum'):
                load_manifest(path)

    def test_corrupt_truncated_and_lfs_pointer_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / 'font.ttf'
            expected = b'trusted font'
            sha = hashlib.sha256(expected).hexdigest()
            for data in (b'trusted fonT', b'trusted', b'version https://git-lfs.github.com/spec/v1\n'):
                target.write_bytes(data)
                with self.assertRaises(ValueError):
                    checked_file(root, 'font.ttf', len(expected), sha)

    def test_path_traversal_and_symlink_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / 'outside'
            target.write_bytes(b'a')
            (root / 'alias').symlink_to(target)
            for name in ('../outside', '/outside', 'alias'):
                with self.assertRaises(ValueError):
                    checked_file(root, name, 1, hashlib.sha256(b'a').hexdigest())

    def test_unbounded_or_truncated_sfnt_rejected(self):
        for data in (b'', b'OTTO', b'\x00\x01\x00\x00' + b'\xff' * 12,
                     b'OTTO' + struct.pack('>H', 1) + b'\x00' * 6 + struct.pack('>4sIII', b'cmap', 0, 99999, 99999)):
            with self.assertRaises(ValueError):
                inventory(data)

    def test_altered_font_table_offset_rejected(self):
        path = next((ROOT / 'assets/unicode/fonts/notosans').glob('*.ttf'))
        data = bytearray(path.read_bytes())
        struct.pack_into('>I', data, 20, len(data) + 1)
        with self.assertRaisesRegex(ValueError, 'bounds'):
            inventory(data)


if __name__ == '__main__':
    unittest.main()
