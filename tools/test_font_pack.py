"""Adversarial delivery tests using original synthetic font/notice bytes."""
# Copyright 2026 Seraphina Studios. SPDX-License-Identifier: MIT
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('font_pack', Path(__file__).with_name('font_pack.py'))
pack = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pack)


def fixture(base):
    source = base / 'source'
    directory = source / 'fonts/test'
    directory.mkdir(parents=True)
    font, notice = b'original synthetic font', b'original synthetic notice'
    manifest = {'schema': 1, 'id': pack.PACK_ID, 'repository': 'https://github.com/google/fonts',
                'revision': pack.REVISION, 'license': 'OFL-1.1',
                'total_font_bytes': len(font), 'total_notice_bytes': len(notice),
                'fonts': [{'family': 'Synthetic', 'path': 'fonts/test/Test.ttf',
                           'upstream_path': 'ofl/test/Test.ttf', 'bytes': len(font), 'sha256': pack.digest(font),
                           'license': 'OFL-1.1', 'notice': {'path': 'fonts/test/OFL.txt',
                           'upstream_path': 'ofl/test/OFL.txt', 'bytes': len(notice), 'sha256': pack.digest(notice)}}]}
    blob = json.dumps(manifest).encode()
    (source / 'manifest.json').write_bytes(blob)
    (directory / 'Test.ttf').write_bytes(font)
    (directory / 'OFL.txt').write_bytes(notice)
    return source, blob, pack.digest(blob)


class DeliveryTests(unittest.TestCase):
    def test_copy_and_reproducible_archive_include_only_exact_data(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, blob, expected = fixture(root)
            record = pack.copy_pack(source, root / 'copy', blob, expected)
            self.assertEqual(pack.verify(root / 'copy', blob, expected), record)
            first = pack.archive(source, root / 'first.tar.gz', blob, expected)
            second = pack.archive(source, root / 'second.tar.gz', blob, expected)
            self.assertEqual(first['archive_sha256'], second['archive_sha256'])
            with tarfile.open(root / 'first.tar.gz') as archive:
                self.assertEqual(archive.getnames(), [pack.PACK_ID + '/fonts/test/OFL.txt',
                    pack.PACK_ID + '/fonts/test/Test.ttf', pack.PACK_ID + '/manifest.json'])
                self.assertTrue(all(entry.isfile() and entry.uid == 0 and entry.mtime == 0 for entry in archive))
            with self.assertRaisesRegex(ValueError, 'must be absent'):
                pack.copy_pack(source, root / 'copy', blob, expected)

    def test_ui_crate_metadata_present_and_large_payload_excluded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, blob, expected = fixture(root)
            for mutation in ('valid', 'font-payload', 'missing', 'mismatch'):
                archive = root / 'tilewright-render-ui-0.6.0.crate'
                with tarfile.open(archive, 'w:gz') as tar:
                    files = {'src/lib.rs': b'// original synthetic fixture'}
                    if mutation != 'missing':
                        files['assets/unicode-font-pack.json'] = blob if mutation != 'mismatch' else blob + b' '
                    if mutation == 'font-payload':
                        files['assets/disguised.bin'] = (source / 'fonts/test/Test.ttf').read_bytes()
                    for name, data in files.items():
                        entry = tarfile.TarInfo('tilewright-render-ui-0.6.0/' + name)
                        entry.size = len(data)
                        tar.addfile(entry, io.BytesIO(data))
                if mutation == 'valid':
                    pack.verify_crate(archive, blob, expected)
                else:
                    with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                        pack.verify_crate(archive, blob, expected)

    def test_manifest_checksum_duplicate_key_and_unsafe_paths_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            _, blob, expected = fixture(Path(directory))
            with self.assertRaisesRegex(ValueError, 'manifest checksum'):
                pack.load_manifest(blob + b' ', expected)
            duplicate = blob[:-1] + b', "schema": 1}'
            with self.assertRaisesRegex(ValueError, 'duplicate manifest key'):
                pack.load_manifest(duplicate, pack.digest(duplicate))
            for path in ('../secret.ttf', '/fonts/test/Secret.ttf', 'fonts/test/../Secret.ttf',
                         'fonts/test\\secret.ttf', 'fonts/test//Test.ttf', 'fonts/test/C:Test.ttf'):
                value = json.loads(blob)
                value['fonts'][0]['path'] = path
                invalid = json.dumps(value).encode()
                with self.subTest(path=path), self.assertRaisesRegex(ValueError, 'unsafe font path'):
                    pack.load_manifest(invalid, pack.digest(invalid))
            value = json.loads(blob)
            value['fonts'].append({**value['fonts'][0], 'family': 'Duplicate path'})
            invalid = json.dumps(value).encode()
            with self.assertRaisesRegex(ValueError, 'duplicate font/notice path'):
                pack.load_manifest(invalid, pack.digest(invalid))

    def test_changed_font_notice_manifest_sizes_and_extra_files_refused(self):
        for mutation in ('font', 'notice', 'manifest', 'truncated', 'extra', 'empty-directory', 'missing-notice'):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                source, blob, expected = fixture(Path(directory))
                if mutation in ('font', 'truncated'):
                    (source / 'fonts/test/Test.ttf').write_bytes(b'bad' if mutation == 'font' else b'')
                elif mutation == 'notice':
                    (source / 'fonts/test/OFL.txt').write_bytes(b'bad')
                elif mutation == 'manifest':
                    (source / 'manifest.json').write_bytes(blob + b' ')
                elif mutation == 'extra':
                    (source / 'recovery.json').write_bytes(b'secret')
                elif mutation == 'empty-directory':
                    (source / 'world').mkdir()
                else:
                    (source / 'fonts/test/OFL.txt').unlink()
                with self.assertRaises(ValueError):
                    pack.verify(source, blob, expected)

    def test_file_and_directory_links_refused(self):
        for target in ('fonts/test/Test.ttf', 'fonts/test'):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source, blob, expected = fixture(root)
                path = source / target
                outside = root / 'outside'
                path.rename(outside)
                path.symlink_to(outside, target_is_directory=outside.is_dir())
                with self.assertRaisesRegex(ValueError, 'link or special'):
                    pack.verify(source, blob, expected)

    def test_download_request_is_public_pinned_and_size_hash_bounded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, blob, expected = fixture(root)
            entries = pack.load_manifest(blob, expected)[1]
            calls = []
            class Response(io.BytesIO):
                status = 200
                def geturl(self):
                    return self.url
            class Opener:
                def open(self, request, timeout):
                    calls.append(request)
                    entry = next(entry for entry in entries.values() if request.full_url.endswith(entry['upstream_path']))
                    response = Response((source / entry['path']).read_bytes())
                    response.url = request.full_url
                    return response
            pack.acquire(root / 'acquired', blob, expected, Opener())
            self.assertEqual(len(calls), 2)
            self.assertTrue(all(request.full_url.startswith('https://raw.githubusercontent.com/google/fonts/' + pack.REVISION + '/')
                                and set(request.headers) == {'User-agent'} for request in calls))
            class TooLarge(Opener):
                def open(self, request, timeout):
                    response = super().open(request, timeout)
                    response.seek(0, io.SEEK_END)
                    response.write(b'overflow')
                    response.seek(0)
                    return response
            with self.assertRaisesRegex(ValueError, 'checksum/size'):
                pack.acquire(root / 'bad-download', blob, expected, TooLarge())
            with self.assertRaisesRegex(ValueError, 'redirects forbidden'):
                pack.NoRedirect().redirect_request(None, None, 302, '', {}, 'https://example.invalid')


if __name__ == '__main__':
    unittest.main()
