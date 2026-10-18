#!/usr/bin/env python3
"""Credential-free candidate packaging. Never requests OIDC or uploads."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib

# Set before importing local modules: even a read-only command must not dirty source.
sys.dont_write_bytecode = True

if __package__:
    from . import crate_release as R
else:
    import crate_release as R

ROOT = R.ROOT


def git(*args):
    return subprocess.check_output(['git', *args], cwd=ROOT, text=True).strip()


def clean_environment():
    environment = os.environ.copy()
    for name in tuple(environment):
        if name in ('GH_TOKEN', 'GITHUB_TOKEN', 'GH_READ_TOKEN', 'GH_INTENT_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_URL',
                    'ACTIONS_ID_TOKEN_REQUEST_TOKEN') or name.startswith('CARGO_REGISTR'):
            environment.pop(name)
    return environment


def preparation_boundary():
    R.require(not any(os.environ.get(key) for key in ('CARGO_REGISTRY_TOKEN',
        'CARGO_REGISTRIES_CRATES_IO_TOKEN', 'CARGO_REGISTRY_BOOTSTRAP_TOKEN',
        'ACTIONS_ID_TOKEN_REQUEST_URL', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN', 'GH_TOKEN', 'GITHUB_TOKEN', 'GH_INTENT_TOKEN')),
        'preparation requires a credential-free job')


def versions(config, root=ROOT):
    workspace = tomllib.loads((root / 'Cargo.toml').read_text()).get('workspace', {}).get('package', {})
    result = {}
    for crate in config['crates']:
        package = tomllib.loads((root / crate['path'] / 'Cargo.toml').read_text())['package']
        R.require(package['name'] == crate['name'] and package.get('publish') == ['crates-io'], 'publishable crate policy')
        version = package['version']
        if isinstance(version, dict):
            R.require(version == {'workspace': True}, 'unsupported version inheritance')
            version = workspace['version']
        R.require(isinstance(version, str) and R.VERSION.fullmatch(version), 'crate version')
        result[crate['name']] = version
    return result


def known(config):
    entries = R.catalog(config)
    for old in config['historical']:
        key = (old['name'], old['version'])
        R.require(key not in entries, 'historical version has duplicate automatic lineage')
        entries[key] = old
    return entries


def pending(config):
    if not R.registry_source_branch():
        return False
    existing = known(config)
    return any((name, version) not in existing for name, version in versions(config).items())


def check_version(config, base):
    R.require(R.SHA.fullmatch(base), 'base must be immutable revision')
    git('cat-file', '-e', base + '^{commit}')
    # First onboarding has no previous public-package policy. Real preparation
    # still compares every coordinate against public release/registry history.
    if subprocess.run(['git', 'cat-file', '-e', base + ':policy/crate-release.json'],
                      cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
        return
    previous = json.loads(git('show', base + ':policy/crate-release.json'))
    previous_names = {row['name']: row for row in previous['crates']}
    current_versions = versions(config)
    changed = set(git('diff', '--name-only', base, '--').splitlines())
    old_workspace = tomllib.loads(git('show', base + ':Cargo.toml')).get('workspace', {}).get('package', {})
    for row in config['crates']:
        if row['name'] not in previous_names:
            continue
        prior = previous_names[row['name']]
        old_package = tomllib.loads(git('show', base + ':' + prior['path'] + '/Cargo.toml'))['package']
        old_version = old_package['version']
        if isinstance(old_version, dict):
            old_version = old_workspace['version']
        version = current_versions[row['name']]
        R.require(tuple(map(int, version.split('.'))) >= tuple(map(int, old_version.split('.'))), 'crate version moved backward')
        package_changed = (row['path'] != prior['path'] or config['toolchain'] != previous['toolchain']
            or any(path.startswith((row['path'] + '/', prior['path'] + '/', '.cargo/')) for path in changed)
            or bool(changed & {'Cargo.toml', 'Cargo.lock', 'rust-toolchain', 'rust-toolchain.toml'}))
        R.require(not package_changed or version != old_version, 'package content changed without reviewed version bump')


def write_exact(path, data):
    if path.exists():
        R.require(path.is_file() and not path.is_symlink() and path.read_bytes() == data, 'prepared output collision')
    else:
        path.write_bytes(data)


def package_row(crate, version, data):
    files = R.archive(data, crate['name'], version)
    return {'name': crate['name'], 'version': version, 'path': crate['path'],
            'asset': f"{crate['name']}-{version}.crate", 'sha256': R.digest(data),
            'content_sha256': R.content_digest(files), 'inventory': sorted(files),
            'metadata': R.metadata(files, crate['name'], version)}


def original_package(config, crate, version, entry):
    """Authenticate the owning source bytes and require their registry readiness."""
    if isinstance(entry, dict):
        release = R.api(config, 'releases/tags/' + entry['source_tag'])
        data = R.asset_bytes(config, release, entry['asset'])
        R.require(R.digest(data) == entry['sha256'], 'historical artifact changed')
        row = package_row(crate, version, data)
        revision = entry['revision']
    else:
        release, manifest, row, _ = entry
        data = R.asset_bytes(config, release, row['asset'])
        revision = manifest['revision']
    R.require((row['name'], row['version'], row['path']) ==
              (crate['name'], version, crate['path']), 'owned package identity changed')
    R.verify_archive(row, data, revision)
    R.require(R.registry_state(row) == 'present', 'owned crate registry readiness unresolved')
    return row, data


def source_only_packages(config, names):
    """Explicit nonpublishable workspace packages are source assets, never registry rows."""
    R.require(len(names) == len(set(names)) and all(R.NAME.fullmatch(name) for name in names),
              'invalid source-only package selection')
    R.require(not set(names) & {crate['name'] for crate in config['crates']}, 'source/public package overlap')
    if not names:
        return {}
    value = json.loads(subprocess.check_output(['cargo', '+' + config['toolchain'], 'metadata',
        '--locked', '--offline', '--no-deps', '--format-version', '1'], cwd=ROOT,
        env=clean_environment(), timeout=120))
    members = {row['name']: row for row in value['packages'] if row['id'] in value['workspace_members']}
    result = {}
    for name in names:
        R.require(name in members and members[name]['publish'] == [], 'source-only package must have publish=false')
        row = members[name]
        path = Path(row['manifest_path']).resolve().parent.relative_to(ROOT.resolve()).as_posix()
        R.require(R.VERSION.fullmatch(row['version']), 'source-only package version')
        result[name] = {'name': name, 'version': row['version'], 'path': path}
    return result


def package_twice(config, names, current, target, dry_run):
    R.require(names, 'empty package selection')
    sets = []
    for number in (1, 2):
        destination = target / str(number)
        command = ['cargo', '+' + config['toolchain'], 'package', '--locked', '--no-verify',
                   '--registry', 'crates-io', '--target-dir', str(destination)]
        for name in names:
            command += ['--package', name]
        if dry_run:
            command += ['--allow-dirty', '--offline']
        subprocess.run(command, cwd=ROOT, env=clean_environment(), check=True, timeout=1200)
        sets.append(destination / 'package')
    result = {}
    for name in names:
        asset = f'{name}-{current[name]}.crate'
        first = (sets[0] / asset).read_bytes()
        R.require(first == (sets[1] / asset).read_bytes(), 'package reproduction mismatch')
        result[name] = first
    return result


def stage_packages(config, current, previous, revision, destination, dry_run=False, source_packages=None):
    """Published coordinates stay out of Cargo's new-package registry overlay."""
    source_packages = source_packages or {}
    originals = {}
    for crate in config['crates']:
        key = (crate['name'], current[crate['name']])
        if key in previous:
            originals[crate['name']] = original_package(config, crate, key[1], previous[key])
    # Resolve ALL original provenance/readiness before building or staging anything.
    rows, data = {}, {}
    for crate in config['crates']:
        name = crate['name']
        if name not in originals:
            continue
        candidate = package_twice(config, [name], current, destination / ('old-' + name), dry_run)[name]
        candidate_row = package_row(crate, current[name], candidate)
        R.verify_archive(candidate_row, candidate, revision)
        original_row, original_bytes = originals[name]
        R.require(candidate_row['inventory'] == original_row['inventory'] and
                  candidate_row['content_sha256'] == original_row['content_sha256'],
                  'owned crate version has changed package contents')
        rows[name], data[name] = original_row, original_bytes
    selected = [crate['name'] for crate in config['crates'] if crate['name'] not in originals]
    selected += list(source_packages)
    if selected:
        data.update(package_twice(config, selected,
            dict(current, **{name: row['version'] for name, row in source_packages.items()}),
            destination / 'new', dry_run))
    for crate in config['crates']:
        name = crate['name']
        if name not in originals:
            rows[name] = package_row(crate, current[name], data[name])
            if not dry_run:
                R.verify_archive(rows[name], data[name], revision)
    for name, crate in source_packages.items():
        files = R.archive(data[name], name, crate['version'])
        if not dry_run:
            R.require(json.loads(files['.cargo_vcs_info.json']) ==
                {'git': {'sha1': revision}, 'path_in_vcs': crate['path']}, 'source-only archive provenance')
    staged = destination / 'verified-graph'
    staged.mkdir()
    for name, archive in data.items():
        version = current[name] if name in current else source_packages[name]['version']
        (staged / f'{name}-{version}.crate').write_bytes(archive)
    verifier = ROOT / 'tools/verify-packages.sh'
    R.require(verifier.is_file(), 'independent packaged graph verifier required')
    subprocess.run(['bash', str(verifier), str(staged)], cwd=ROOT,
                   env=clean_environment(), check=True, timeout=1800)
    # A verifier may compile extracted archives, but cannot change the proven input bytes.
    R.require({path.name for path in staged.glob('*.crate')} ==
        {f"{name}-{current[name] if name in current else source_packages[name]['version']}.crate" for name in data},
        'staged archive inventory changed')
    for name, archive in data.items():
        version = current[name] if name in current else source_packages[name]['version']
        R.require((staged / f'{name}-{version}.crate').read_bytes() == archive, 'staged archive changed')
    return rows, data


def prepare(config, tag, output, archives=None, dry_run=False, source_packages=()):
    preparation_boundary()
    R.require(dry_run or not git('status', '--porcelain'), 'source packaging requires clean revision')
    revision = git('rev-parse', 'HEAD')
    if dry_run:
        tag = 'v0.0.0'
    R.require(__import__('re').fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+', tag), 'source version')
    output.mkdir(parents=True, exist_ok=True)
    current = versions(config)
    extras = source_only_packages(config, source_packages)
    eligible = dry_run or R.registry_source_branch()
    previous = {} if dry_run or not eligible else known(config)
    manifest = {'schema_version': 1, 'repository': config['repository'],
                'repository_id': config['repository_id'], 'revision': revision, 'source_tag': tag, 'crates': []}
    # Existing caller-generated .crate files are not authoritative graph inputs.
    # Reject them rather than let an all-selected overlay replace published bytes.
    for directory in {output, archives} - {None}:
        R.require(not any(directory.glob('*.crate')), 'preexisting package output collision')
    with tempfile.TemporaryDirectory(prefix='tilewright-crate-prepare-') as temporary:
        rows, data = stage_packages(config, current, previous, revision, Path(temporary), dry_run, extras)
        for crate in config['crates']:
            name, version = crate['name'], current[crate['name']]
            if not eligible or (name, version) in previous:
                continue
            if not dry_run:
                R.require(R.registry_state(rows[name]) == 'absent', 'new crate coordinate already exists or is unresolved')
            manifest['crates'].append(rows[name])
        R.validate_manifest(config, manifest)
        manifest['crates'] = R.ordered(manifest['crates'])
        # Only after the complete graph proof may selected assets leave staging.
        for row in manifest['crates']:
            write_exact(output / row['asset'], data[row['name']])
            if archives is not None and archives != output:
                write_exact(archives / row['asset'], data[row['name']])
        for name, crate in extras.items():
            write_exact(output / f"{name}-{crate['version']}.crate", data[name])
    write_exact(output / R.MANIFEST, R.canonical(manifest) + b'\n')
    R.require(dry_run or not git('status', '--porcelain'), 'preparation changed source')
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['check-version', 'prepare', 'pending'])
    parser.add_argument('--base'); parser.add_argument('--source-tag')
    parser.add_argument('--output', type=Path); parser.add_argument('--archives', type=Path)
    parser.add_argument('--dry-run', action='store_true')
    parser.add_argument('--source-package', action='append', default=[])
    args = parser.parse_args()
    config = R.policy()
    if args.operation == 'check-version':
        check_version(config, args.base)
    elif args.operation == 'pending':
        print(json.dumps({'pending': pending(config)}))
    else:
        R.require(args.output is not None, 'output required')
        prepare(config, args.source_tag, args.output, args.archives, args.dry_run, args.source_package)


if __name__ == '__main__':
    main()
