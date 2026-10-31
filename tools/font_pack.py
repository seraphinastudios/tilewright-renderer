"""Exact, bounded delivery of the separate immutable Unicode font pack.

This MIT delivery helper verifies bytes and inventories, not font syntax. The
renderer tools/unicode-fonts checker owns cmap, name and license interpretation.
"""
# Copyright 2026 Seraphina Studios. SPDX-License-Identifier: MIT
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import tarfile
import urllib.parse
import urllib.request

MANIFEST_SHA256 = '94b81a712fb41a66eb744c88ae2ed312cc5d0ec1a491e9821fcc278d99410f13'
REVISION = '6c1dbe27dcf7d817d4b62cec033ba77fa8e2a5ec'
PACK_ID = 'tilewright-unicode-noto-v1'
MAX_MANIFEST = 1024 * 1024
MAX_TOTAL = 64 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'duplicate manifest key')
        result[key] = value
    return result


def read_regular(path, maximum):
    require(path.is_absolute() and path.resolve() == path and not path.is_symlink(),
            'font path contains a link')
    descriptor = os.open(path, os.O_RDONLY | getattr(os, 'O_NOFOLLOW', 0) | getattr(os, 'O_NONBLOCK', 0))
    with os.fdopen(descriptor, 'rb') as stream:
        value = os.fstat(stream.fileno())
        require(stat.S_ISREG(value.st_mode) and value.st_size <= maximum, 'font file type/size bound')
        data = stream.read(maximum + 1)
    require(len(data) <= maximum, 'font read size bound')
    return data


def load_manifest(blob, expected_sha=MANIFEST_SHA256):
    require(len(blob) <= MAX_MANIFEST and digest(blob) == expected_sha, 'font manifest checksum/bounds')
    manifest = json.loads(blob, object_pairs_hook=unique_object)
    require(manifest['schema'] == 1 and manifest['id'] == PACK_ID
            and manifest['revision'] == REVISION and manifest['repository'] == 'https://github.com/google/fonts'
            and manifest['license'] == 'OFL-1.1', 'font manifest identity/license')
    require(isinstance(manifest['fonts'], list) and 0 < len(manifest['fonts']) <= 32, 'font count bound')
    entries = {}
    folded = set()
    total_font = total_notice = 0
    families = set()
    for font in manifest['fonts']:
        require(font['family'] not in families and font['license'] == 'OFL-1.1', 'font family/license')
        families.add(font['family'])
        for entry, kind in ((font, 'font'), (font['notice'], 'notice')):
            name = entry['path']
            require(isinstance(name, str) and '\\' not in name and ':' not in name
                    and all(ord(character) >= 32 for character in name), 'unsafe font path')
            path = PurePosixPath(name)
            require(len(path.parts) == 3 and path.parts[0] == 'fonts'
                    and re.fullmatch('[a-z0-9]+', path.parts[1]) is not None
                    and str(path) == name and path.name not in ('.', '..')
                    and (path.suffix == '.ttf' if kind == 'font' else path.name == 'OFL.txt'), 'unsafe font path')
            require(name.casefold() not in folded, 'duplicate font/notice path')
            folded.add(name.casefold())
            require(entry['upstream_path'] == 'ofl/' + '/'.join(path.parts[1:]), 'font upstream path mismatch')
            require(type(entry['bytes']) is int and 0 < entry['bytes'] <= (24 * 1024 * 1024 if kind == 'font' else 8192),
                    'font byte bound')
            require(re.fullmatch('[0-9a-f]{64}', entry['sha256']) is not None, 'font digest syntax')
            entries[name] = entry
            if kind == 'font':
                total_font += entry['bytes']
            else:
                total_notice += entry['bytes']
        require(PurePosixPath(font['path']).parent == PurePosixPath(font['notice']['path']).parent,
                'font notice family mismatch')
    require(total_font == manifest['total_font_bytes'] and total_notice == manifest['total_notice_bytes']
            and total_font + total_notice <= MAX_TOTAL, 'font total byte bound')
    return manifest, entries


def tree_files(root):
    require(root.is_absolute() and root.resolve() == root and root.is_dir() and not root.is_symlink(),
            'font pack root contains a link')
    files, directories = set(), set()
    for parent, children, names in os.walk(root, followlinks=False):
        for name in children + names:
            path = Path(parent) / name
            value = path.lstat()
            relative = path.relative_to(root).as_posix()
            if stat.S_ISDIR(value.st_mode):
                directories.add(relative)
            else:
                require(stat.S_ISREG(value.st_mode), 'font pack contains a link or special file')
                files.add(relative)
    return files, directories


def verify(root, manifest_blob, expected_sha=MANIFEST_SHA256):
    manifest, entries = load_manifest(manifest_blob, expected_sha)
    files, directories = tree_files(root)
    expected_directories = {str(parent) for name in entries for parent in PurePosixPath(name).parents if str(parent) != '.'}
    require(files == {'manifest.json', *entries} and directories == expected_directories, 'exact font pack file/directory set')
    require(read_regular(root / 'manifest.json', MAX_MANIFEST) == manifest_blob, 'font pack manifest mismatch')
    records = []
    for name, entry in entries.items():
        blob = read_regular(root / name, entry['bytes'])
        require(len(blob) == entry['bytes'] and digest(blob) == entry['sha256'], 'font pack file checksum/size: ' + name)
        records.append({'path': name, 'bytes': len(blob), 'sha256': digest(blob), 'license': 'OFL-1.1'})
    return {'id': manifest['id'], 'revision': manifest['revision'], 'manifest_sha256': digest(manifest_blob),
            'font_bytes': manifest['total_font_bytes'], 'notice_bytes': manifest['total_notice_bytes'],
            'files': sorted(records, key=lambda entry: entry['path'])}


def copy_pack(source, destination, manifest_blob, expected_sha=MANIFEST_SHA256):
    record = verify(source, manifest_blob, expected_sha)
    require(not destination.exists() and not destination.is_symlink(), 'font destination must be absent')
    require(destination.parent.resolve() == destination.parent, 'font destination contains a link')
    destination.mkdir(mode=0o700)
    for name in ['manifest.json', *(entry['path'] for entry in record['files'])]:
        maximum = MAX_MANIFEST if name == 'manifest.json' else next(entry['bytes'] for entry in record['files'] if entry['path'] == name)
        data = read_regular(source / name, maximum)
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open('xb') as output:
            output.write(data)
    verify(destination, manifest_blob, expected_sha)
    return record


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError('font acquisition redirects forbidden')


def acquire(destination, manifest_blob, expected_sha=MANIFEST_SHA256, opener=None):
    """Explicit preparation only. Public fixed-revision HTTPS; no ambient auth."""
    _, entries = load_manifest(manifest_blob, expected_sha)
    require(not destination.exists() and not destination.is_symlink(), 'font destination must be absent')
    require(destination.parent.resolve() == destination.parent, 'font destination contains a link')
    if opener is None:
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect)
    destination.mkdir(mode=0o700)
    (destination / 'manifest.json').write_bytes(manifest_blob)
    for entry in entries.values():
        url = 'https://raw.githubusercontent.com/google/fonts/' + REVISION + '/' + urllib.parse.quote(entry['upstream_path'], safe='/')
        request = urllib.request.Request(url, headers={'User-Agent': 'Tilewright-public-font-preparation/1'})
        with opener.open(request, timeout=30) as response:
            require(response.status == 200 and response.geturl() == url, 'font response identity/status')
            data = response.read(entry['bytes'] + 1)
        require(len(data) == entry['bytes'] and digest(data) == entry['sha256'], 'font download checksum/size')
        target = destination / entry['path']
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open('xb') as output:
            output.write(data)
    return verify(destination, manifest_blob, expected_sha)


def components(record):
    files = [{'path': 'manifest.json', 'sha256': record['manifest_sha256'], 'license': 'OFL-1.1'}, *record['files']]
    return [{'type': 'file', 'name': '/font-pack/' + entry['path'],
             'bom-ref': 'font-pack:' + record['manifest_sha256'] + ':' + entry['path'],
             'hashes': [{'alg': 'SHA-256', 'content': entry['sha256']}],
             'licenses': [{'license': {'id': entry['license']}}]} for entry in files]


def payload(root, name, manifest_blob, record):
    if name == 'manifest.json':
        blob = read_regular(root / name, MAX_MANIFEST)
        require(blob == manifest_blob, 'font manifest changed during delivery')
    else:
        entry = next(entry for entry in record['files'] if entry['path'] == name)
        blob = read_regular(root / name, entry['bytes'])
        require(len(blob) == entry['bytes'] and digest(blob) == entry['sha256'], 'font changed during delivery')
    return blob


def archive(root, destination, manifest_blob, expected_sha=MANIFEST_SHA256):
    record = verify(root, manifest_blob, expected_sha)
    with destination.open('xb') as output:
        with gzip.GzipFile(fileobj=output, mode='wb', filename='', mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode='w|', format=tarfile.USTAR_FORMAT) as tar:
                for name in sorted(['manifest.json', *(entry['path'] for entry in record['files'])]):
                    blob = payload(root, name, manifest_blob, record)
                    info = tarfile.TarInfo(PACK_ID + '/' + name)
                    info.size, info.mode, info.mtime = len(blob), 0o644, 0
                    tar.addfile(info, io.BytesIO(blob))
    return {**record, 'archive': destination.name, 'archive_sha256': digest(destination.read_bytes())}


def verify_crate(archive_path, manifest_blob, expected_sha=MANIFEST_SHA256):
    """Keep the large separately delivered source fonts out of the UI crate."""
    _, entries = load_manifest(manifest_blob, expected_sha)
    font_hashes = {entry['sha256'] for name, entry in entries.items() if name.endswith('.ttf')}
    require(archive_path.stat().st_size <= 10 * 1024 * 1024, 'UI crate archive size bound')
    total = 0
    found = False
    seen = set()
    with tarfile.open(archive_path, mode='r:gz') as tar:
        for entry in tar:
            path = PurePosixPath(entry.name)
            require(entry.isfile() and not path.is_absolute() and '..' not in path.parts
                    and len(path.parts) >= 2 and entry.name not in seen
                    and path.parts[0] == archive_path.name.removesuffix('.crate'), 'unsafe/duplicate UI crate member')
            seen.add(entry.name)
            require(len(seen) <= 4096, 'UI crate file count bound')
            total += entry.size
            require(total <= 10 * 1024 * 1024 and entry.size <= MAX_MANIFEST * 2, 'UI crate unpacked size bound')
            relative = str(PurePosixPath(*path.parts[1:]))
            require(not relative.startswith('assets/unicode/')
                    and (not relative.endswith('.ttf') or relative == 'assets/RobotoMono-Regular.ttf'),
                    'Unicode font payload forbidden inside public UI crate')
            blob = tar.extractfile(entry).read(entry.size + 1)
            require(digest(blob) not in font_hashes, 'Unicode font payload forbidden inside public UI crate')
            if relative == 'assets/unicode-font-pack.json':
                require(blob == manifest_blob, 'UI crate Unicode metadata mismatch')
                found = True
    require(found, 'UI crate Unicode metadata missing')
