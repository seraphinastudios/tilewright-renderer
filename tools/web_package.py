#!/usr/bin/env python3
"""Checks the browser package's release tarball and its in-tree manifest.

Standard library only: the release's attest job runs it on downloaded bytes
without building, installing or running package code.

- `check TARBALL --version X.Y.Z` checks an `npm pack` tarball: its name, its
  exact file list, its manifest, and that no test hook was built in.
- `check-release DIRECTORY --version X.Y.Z` checks that a prepared release
  directory holds exactly one browser package tarball, this release's, that
  `SHA256SUMS` lists it with its digest, and checks the tarball.
- `check-source` checks that `packages/tilewright-render/package.json` keeps
  the placeholder version, which only the release replaces, and the `files`
  allowlist that produces exactly the tarball's file list.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import posixpath
import re
import sys
import tarfile

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_DIRECTORY = ROOT / 'packages' / 'tilewright-render'
NAME = '@seraphinastudios/tilewright-render'
TARBALL_PREFIX = 'seraphinastudios-tilewright-render-'
# The in-tree version. Only the release stamps a real one, the source release
# version, so the committed manifest never claims a stale version.
SOURCE_VERSION = '0.0.0-development'
# Validation's dry run packs under this version, which no release can have.
CHECK_VERSION = '0.0.0-check'
RELEASE_VERSION = re.compile(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)')
REPOSITORY = 'git+https://github.com/seraphinastudios/tilewright-renderer.git'

# The TypeScript modules in src/; each ships as .js and .d.ts.
MODULES = ('bindings', 'errors', 'index', 'scene')
BUNDLES = ('full', 'webgpu')
BUNDLE_FILES = ('tilewright_render_web.js', 'tilewright_render_web.d.ts',
                'tilewright_render_web_bg.wasm', 'tilewright_render_web_bg.wasm.d.ts')
NOTICES = ('LICENSE', 'THIRD_PARTY_NOTICES.md', 'OFL-RobotoMono.txt')
# Every file the tarball holds, and nothing else.
FILES = frozenset(
    ['package.json', 'README.md', *NOTICES]
    + [f'dist/{module}.{suffix}' for module in MODULES for suffix in ('js', 'd.ts')]
    + [f'wasm/{bundle}/{file}' for bundle in BUNDLES for file in BUNDLE_FILES]
)
# The `files` field that selects FILES (npm adds package.json and README.md).
FILES_FIELD = [
    'dist/*.js',
    'dist/*.d.ts',
    *[f'wasm/{bundle}/{file}' for bundle in BUNDLES for file in BUNDLE_FILES],
    *NOTICES,
]
# The `exports` map: the entry, the manifest, and the bundles' files by
# subpath, for consumers that pass `wasm` URLs to `create` themselves.
EXPORTS = {
    '.': {'types': './dist/index.d.ts', 'default': './dist/index.js'},
    './package.json': './package.json',
    './wasm/*': './wasm/*',
}
SOURCE_MAPPING_URL = re.compile(rb'^//# sourceMappingURL=(\S+)\s*$', re.MULTILINE)
# Exports of the browser tests' trap hooks (crates/tilewright-render-web,
# feature `test-hooks`). A shipped bundle never carries them.
TEST_HOOK_MARKERS = (b'panicForTest', b'panic_for_test', b'test panic at')
LIFECYCLE_SCRIPTS = ('preinstall', 'install', 'postinstall', 'prepare', 'prepack', 'postpack',
                     'preuninstall', 'uninstall', 'postuninstall', 'dependencies')
MAX_TARBALL_BYTES = 64 * 1024 * 1024
MAX_FILE_BYTES = 32 * 1024 * 1024


class PackageError(ValueError):
    pass


def require(ok: bool, message: str) -> None:
    if not ok:
        raise PackageError(message)


def tarball_name(version: str) -> str:
    return f'{TARBALL_PREFIX}{version}.tgz'


def valid_version(version: str) -> bool:
    return version == CHECK_VERSION or RELEASE_VERSION.fullmatch(version) is not None


def check_manifest(manifest: object, version: str) -> None:
    """The packed package.json: the release version and nothing that runs on install."""
    require(isinstance(manifest, dict), 'package.json is not an object')
    require(manifest.get('name') == NAME, f'package name is {manifest.get("name")!r}, not {NAME}')
    require(manifest.get('version') == version, f'package version is {manifest.get("version")!r}, not {version}')
    require(manifest.get('license') == 'MIT', 'package licence is not MIT')
    repository = manifest.get('repository')
    require(isinstance(repository, dict) and repository.get('url') == REPOSITORY,
            'package repository is not this repository')
    require(manifest.get('type') == 'module', 'package is not an ES module package')
    require(manifest.get('exports') == EXPORTS, 'package exports differ from tools/web_package.py EXPORTS')
    for field in ('dependencies', 'optionalDependencies', 'peerDependencies', 'bundleDependencies',
                  'bundledDependencies'):
        require(not manifest.get(field), f'package declares {field}; the asset has no runtime dependencies')
    scripts = manifest.get('scripts', {})
    require(isinstance(scripts, dict), 'package scripts are not an object')
    for script in LIFECYCLE_SCRIPTS:
        require(script not in scripts, f'package declares the lifecycle script {script}')


def check_tarball(path: Path, version: str) -> dict:
    """Checks an `npm pack` tarball of this release; returns its package.json."""
    require(valid_version(version), f'invalid package version: {version}')
    require(path.name == tarball_name(version), f'unexpected tarball name: {path.name}')
    require(path.is_file() and not path.is_symlink(), f'{path.name} is not a regular file')
    data = path.read_bytes()
    require(len(data) <= MAX_TARBALL_BYTES, f'{path.name} exceeds {MAX_TARBALL_BYTES} bytes')
    try:
        raw = gzip.decompress(data)
    except (OSError, EOFError) as error:
        raise PackageError(f'{path.name} is not gzip: {error}') from None
    contents: dict[str, bytes] = {}
    with tarfile.open(fileobj=io.BytesIO(raw), mode='r:') as archive:
        for member in archive:
            require(member.isfile(), f'{member.name} is not a regular file')
            require(member.name.startswith('package/'), f'{member.name} is outside package/')
            name = member.name[len('package/'):]
            require(name not in contents, f'{member.name} appears twice')
            require(member.size <= MAX_FILE_BYTES, f'{member.name} exceeds {MAX_FILE_BYTES} bytes')
            extracted = archive.extractfile(member)
            require(extracted is not None, f'{member.name} cannot be read')
            contents[name] = extracted.read()
    unexpected = sorted(set(contents) - FILES)
    missing = sorted(FILES - set(contents))
    require(not unexpected, f'files outside the allowlist: {", ".join(unexpected)}')
    require(not missing, f'missing files: {", ".join(missing)}')
    for name, body in contents.items():
        require(len(body) > 0, f'{name} is empty')
        for marker in TEST_HOOK_MARKERS:
            require(marker not in body, f'{name} contains the test hook {marker.decode()}; '
                    'build without TILEWRIGHT_WEB_TEST_HOOKS')
    for name, body in contents.items():
        if not name.endswith('.js'):
            continue
        for match in SOURCE_MAPPING_URL.finditer(body):
            target = match[1].decode(errors='replace')
            if target.startswith('data:'):
                continue
            resolved = posixpath.normpath(str(PurePosixPath(name).parent / target))
            require(resolved in contents, f'{name} names the source map {target}, which the tarball does not hold')
    for bundle in BUNDLES:
        wasm = contents[f'wasm/{bundle}/tilewright_render_web_bg.wasm']
        require(wasm.startswith(b'\0asm'), f'wasm/{bundle} is not WebAssembly')
    try:
        manifest = json.loads(contents['package.json'])
    except ValueError as error:
        raise PackageError(f'package.json is not JSON: {error}') from None
    check_manifest(manifest, version)
    require(contents['LICENSE'] == (ROOT / 'LICENSE').read_bytes(), 'LICENSE differs from the repository licence')
    return manifest


def check_release(directory: Path, version: str) -> Path:
    """A prepared release holds this release's tarball once, listed in SHA256SUMS."""
    require(RELEASE_VERSION.fullmatch(version) is not None, f'invalid release version: {version}')
    tarballs = sorted(path.name for path in directory.iterdir() if path.name.startswith(TARBALL_PREFIX))
    require(tarballs == [tarball_name(version)],
            f'expected exactly {tarball_name(version)} in the release, found {tarballs}')
    path = directory / tarballs[0]
    listed = [line for line in (directory / 'SHA256SUMS').read_text().splitlines()
              if re.fullmatch(rf'[0-9a-f]{{64}}  (\./)?{re.escape(path.name)}', line)]
    require(len(listed) == 1, f'SHA256SUMS does not list {path.name} once')
    require(listed[0][:64] == hashlib.sha256(path.read_bytes()).hexdigest(),
            f'SHA256SUMS disagrees with {path.name}')
    check_tarball(path, version)
    return path


def check_source(directory: Path = PACKAGE_DIRECTORY) -> None:
    """The committed manifest keeps the placeholder version and the allowlist."""
    manifest = json.loads((directory / 'package.json').read_text())
    require(manifest.get('version') == SOURCE_VERSION,
            f'packages/tilewright-render/package.json version is {manifest.get("version")!r}; keep '
            f'{SOURCE_VERSION}: the release stamps the source release version')
    lock = json.loads((directory / 'package-lock.json').read_text())
    require(lock.get('version') == SOURCE_VERSION and lock.get('packages', {}).get('', {}).get('version')
            == SOURCE_VERSION, f'package-lock.json must also say {SOURCE_VERSION}')
    require(manifest.get('files') == FILES_FIELD, 'package.json files differs from tools/web_package.py FILES_FIELD')
    sources = sorted(path.stem for path in (directory / 'src').glob('*.ts'))
    require(sources == sorted(MODULES), f'src/ holds {sources}; update MODULES in tools/web_package.py')
    check_manifest(manifest, SOURCE_VERSION)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest='command', required=True)
    check = commands.add_parser('check', help='check a packed tarball')
    check.add_argument('tarball', type=Path)
    check.add_argument('--version', required=True)
    release = commands.add_parser('check-release', help='check a prepared release directory')
    release.add_argument('directory', type=Path)
    release.add_argument('--version', required=True)
    commands.add_parser('check-source', help='check the committed package manifest')
    commands.add_parser('name', help='print the tarball name for a version').add_argument('version')
    options = parser.parse_args(argv)
    try:
        if options.command == 'check':
            check_tarball(options.tarball, options.version)
        elif options.command == 'check-release':
            check_release(options.directory, options.version)
        elif options.command == 'check-source':
            check_source()
        else:
            require(valid_version(options.version), f'invalid package version: {options.version}')
            print(tarball_name(options.version))
    except (PackageError, OSError) as error:
        print(f'web package: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
