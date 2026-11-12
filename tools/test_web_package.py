#!/usr/bin/env python3
"""Tests for the browser package tarball checks in tools/web_package.py."""
from __future__ import annotations

import gzip
import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

from tools import web_package as W

VERSION = '1.25.0'


def manifest(**changes):
    value = {'name': W.NAME, 'version': VERSION, 'license': 'MIT', 'type': 'module',
             'repository': {'type': 'git', 'url': W.REPOSITORY, 'directory': 'packages/tilewright-render'},
             'files': W.FILES_FIELD, 'scripts': {'test': 'node --test'}}
    value.update(changes)
    return value


def contents(changes=None):
    """A complete package's files; `changes` replaces files, or drops them with None."""
    files = {name: b'x' for name in W.FILES}
    files['LICENSE'] = (W.ROOT / 'LICENSE').read_bytes()
    files['package.json'] = json.dumps(manifest()).encode()
    for bundle in W.BUNDLES:
        files[f'wasm/{bundle}/tilewright_render_web_bg.wasm'] = b'\0asm\1\0\0\0'
    for name, body in (changes or {}).items():
        if body is None:
            files.pop(name)
        else:
            files[name] = body
    return files


def tarball(directory: Path, files: dict, name: str | None = None, members=None) -> Path:
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode='w') as archive:
        for path, body in files.items():
            info = tarfile.TarInfo(f'package/{path}')
            info.size = len(body)
            archive.addfile(info, io.BytesIO(body))
        for info in members or ():
            archive.addfile(info)
    path = directory / (name or W.tarball_name(VERSION))
    path.write_bytes(gzip.compress(raw.getvalue()))
    return path


class TarballTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.directory = Path(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def refused(self, path, message, version=VERSION):
        with self.assertRaisesRegex(W.PackageError, message):
            W.check_tarball(path, version)

    def test_complete_tarball_passes(self):
        self.assertEqual(W.check_tarball(tarball(self.directory, contents()), VERSION)['version'], VERSION)

    def test_check_version_is_never_a_release_version(self):
        self.assertTrue(W.valid_version(W.CHECK_VERSION))
        self.assertIsNone(W.RELEASE_VERSION.fullmatch(W.CHECK_VERSION))
        for invalid in ('1.2', 'v1.2.3', '01.2.3', '1.2.3-rc.1', W.SOURCE_VERSION):
            self.assertFalse(W.valid_version(invalid), invalid)

    def test_name_and_version_must_be_this_release(self):
        path = tarball(self.directory, contents())
        self.refused(path, 'unexpected tarball name', version='1.25.1')
        other = tarball(self.directory, contents(), name=W.tarball_name('1.25.1'))
        self.refused(other, 'package version', version='1.25.1')

    def test_file_list_is_exact(self):
        self.refused(tarball(self.directory, contents({'dist/index.js.map': b'{}'})), 'outside the allowlist')
        self.refused(tarball(self.directory, contents({'dist/index.js': None})), 'missing files')
        self.refused(tarball(self.directory, contents({'THIRD_PARTY_NOTICES.md': None})), 'missing files')

    def test_only_regular_files_under_package(self):
        link = tarfile.TarInfo('package/dist/link.js')
        link.type = tarfile.SYMTYPE
        link.linkname = '../../outside'
        self.refused(tarball(self.directory, contents(), members=[link]), 'not a regular file')
        outside = tarfile.TarInfo('other/README.md')
        self.refused(tarball(self.directory, contents(), members=[outside]), 'outside package/')

    def test_test_hooks_are_refused(self):
        for marker in W.TEST_HOOK_MARKERS:
            body = b'export function ' + marker + b'() {}'
            path = tarball(self.directory, contents({'wasm/full/tilewright_render_web.js': body}))
            self.refused(path, 'test hook')

    def test_wasm_must_be_webassembly(self):
        path = tarball(self.directory, contents({'wasm/webgpu/tilewright_render_web_bg.wasm': b'text'}))
        self.refused(path, 'not WebAssembly')

    def test_manifest_cannot_run_or_pull_code_on_install(self):
        for script in ('preinstall', 'install', 'postinstall', 'prepare'):
            body = json.dumps(manifest(scripts={script: 'node evil.js'})).encode()
            self.refused(tarball(self.directory, contents({'package.json': body})), 'lifecycle script')
        body = json.dumps(manifest(dependencies={'left-pad': '1.0.0'})).encode()
        self.refused(tarball(self.directory, contents({'package.json': body})), 'dependencies')
        body = json.dumps(manifest(license='GPL-3.0')).encode()
        self.refused(tarball(self.directory, contents({'package.json': body})), 'licence')

    def test_licence_is_the_repository_licence(self):
        self.refused(tarball(self.directory, contents({'LICENSE': b'other'})), 'LICENSE differs')

    def test_release_directory_lists_exactly_one_tarball(self):
        path = tarball(self.directory, contents())
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        sums = self.directory / 'SHA256SUMS'
        sums.write_text(f'{digest}  {path.name}\n')
        self.assertEqual(W.check_release(self.directory, VERSION), path)
        with self.assertRaisesRegex(W.PackageError, 'expected exactly'):
            W.check_release(self.directory, '1.25.1')
        sums.write_text(f'{"0" * 64}  {path.name}\n')
        with self.assertRaisesRegex(W.PackageError, 'disagrees'):
            W.check_release(self.directory, VERSION)
        sums.write_text('')
        with self.assertRaisesRegex(W.PackageError, 'does not list'):
            W.check_release(self.directory, VERSION)
        sums.write_text(f'{digest}  {path.name}\n')
        tarball(self.directory, contents(), name=W.tarball_name('1.24.0'))
        with self.assertRaisesRegex(W.PackageError, 'expected exactly'):
            W.check_release(self.directory, VERSION)


class SourceTests(unittest.TestCase):
    def test_committed_manifest_keeps_the_placeholder_and_allowlist(self):
        W.check_source()

    def test_a_real_version_in_the_tree_is_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / 'src').mkdir()
            for module in W.MODULES:
                (directory / 'src' / f'{module}.ts').write_text('')
            lock = json.loads((W.PACKAGE_DIRECTORY / 'package-lock.json').read_text())
            (directory / 'package-lock.json').write_text(json.dumps(lock))
            stale = json.loads((W.PACKAGE_DIRECTORY / 'package.json').read_text())
            stale['version'] = '0.5.0'
            (directory / 'package.json').write_text(json.dumps(stale))
            with self.assertRaisesRegex(W.PackageError, 'release stamps'):
                W.check_source(directory)
            stale['version'] = W.SOURCE_VERSION
            stale['files'] = ['dist', 'wasm']
            (directory / 'package.json').write_text(json.dumps(stale))
            with self.assertRaisesRegex(W.PackageError, 'files differs'):
                W.check_source(directory)
            stale['files'] = W.FILES_FIELD
            (directory / 'package.json').write_text(json.dumps(stale))
            (directory / 'src' / 'extra.ts').write_text('')
            with self.assertRaisesRegex(W.PackageError, 'update MODULES'):
                W.check_source(directory)


if __name__ == '__main__':
    unittest.main()
