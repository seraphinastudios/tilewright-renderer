#!/usr/bin/env python3
# Copyright 2026 Seraphina Studios
# SPDX-License-Identifier: MIT
"""Opt-in public pinned font acquisition; never called by a build or runtime."""
import argparse
from pathlib import Path
import tempfile
import urllib.parse
import urllib.request
from check import ROOT, REVISION, check, digest, load_manifest
from sfnt import require


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError('font acquisition redirects are forbidden')


def acquire(output):
    manifest_path = ROOT / 'assets/unicode/manifest.json'
    manifest = load_manifest(manifest_path)
    require(not output.exists() and not output.is_symlink(), 'output must not exist')
    require(output.parent.is_dir(), 'output parent must exist')
    # Explicitly omit cookies, authentication, environment proxies and credentials.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect)
    files = []
    for font in manifest['fonts']:
        files.append(font)
        files.append(font['notice'])
    with tempfile.TemporaryDirectory(prefix='.unicode-fonts-', dir=output.parent) as directory:
        staged = Path(directory) / 'pack'
        staged.mkdir()
        (staged / 'manifest.json').write_bytes(manifest_path.read_bytes())
        for entry in files:
            source = entry['upstream_path']
            require(source.startswith('ofl/') and '..' not in source.split('/'), 'upstream path')
            url = 'https://raw.githubusercontent.com/google/fonts/' + REVISION + '/' + urllib.parse.quote(source, safe='/')
            parsed = urllib.parse.urlsplit(url)
            require(parsed.scheme == 'https' and parsed.netloc == 'raw.githubusercontent.com', 'source host allowlist')
            with opener.open(urllib.request.Request(url, headers={'User-Agent': 'Tilewright-pinned-font-acquisition/1'}), timeout=60) as response:
                data = response.read(entry['bytes'] + 1)
            require(len(data) == entry['bytes'] and digest(data) == entry['sha256'], 'download checksum: ' + source)
            target = staged / entry['path']
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        result = check(staged)
        # A fresh output is the only supported destination. No existing pack is
        # removed or merged; failed acquisition cleans only its private staging.
        require(not output.exists() and not output.is_symlink(), 'output appeared during acquisition')
        staged.rename(output)
        return result


if __name__ == '__main__':
    import json
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', required=True, type=Path)
    print(json.dumps(acquire(parser.parse_args().output_dir), sort_keys=True))
