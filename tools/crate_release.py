#!/usr/bin/env python3
"""Bounded, standard-library-only release evidence and exact-byte publication.

No Cargo, package code, generated executables, or artifact-supplied helpers run
here. The same schema is copied into each physical producer repository.
"""
from __future__ import annotations

import argparse
import datetime
import email.utils
import hashlib
import gzip
import getpass
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
import typing
import secrets
import resource
import urllib.error
import urllib.parse
import urllib.request

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
MAX_BYTES = 32 * 1024 * 1024
MAX_JSON = 2 * 1024 * 1024
MAX_FILES = 4096
MANIFEST = 'crate-release.json'
REGISTRY = 'https://crates.io'
INDEX = 'https://index.crates.io'
NAME = re.compile(r'[a-z][a-z0-9_-]{0,63}')
VERSION = re.compile(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)')
SHA = re.compile(r'[0-9a-f]{40}')
DIGEST = re.compile(r'[0-9a-f]{64}')
# crates.io gives each account a token bucket for new crate names: a burst of
# 5, then one more every 600 s (LimitedAction::PublishNew in rust-lang/crates.io
# src/rate_limiter.rs). Versions of existing crates use a separate bucket.
NEW_CRATE_BURST = 5
NEW_CRATE_RATE_SECONDS = 600
# Covers request latency and clock differences between this terminal and crates.io.
NEW_CRATE_MARGIN_SECONDS = 10
PROGRESS_SECONDS = 60
# Window in which earlier new names may still hold tokens: 5 refill periods, plus margin.
NEW_CRATE_WINDOW_SECONDS = NEW_CRATE_BURST * NEW_CRATE_RATE_SECONDS + NEW_CRATE_MARGIN_SECONDS
# Job names in .github/workflows/release.yml that build and attest the evidence.
EVIDENCE_JOBS = ('Prepare renderer release without publication credentials',
                 'Attest verified renderer source release manifest')
# The Retry-After format crates.io writes (TooManyRequests in src/util/errors/json.rs).
HTTP_DATE = (r'(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun), [0-9]{2} (?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)'
             r' [0-9]{4} [0-9]{2}:[0-9]{2}:[0-9]{2} GMT')


def require(ok, message):
    if not ok:
        raise ValueError(message)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True).encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def safe_path(value):
    require(isinstance(value, str) and 0 < len(value) < 512 and '\\' not in value
            and '\x00' not in value and '\n' not in value and '\r' not in value,
            'invalid relative path')
    path = PurePosixPath(value)
    require(not path.is_absolute() and '..' not in path.parts and str(path) == value
            and value != '.', 'unsafe relative path')
    return value


def policy(root=ROOT):
    value = json.loads((root / 'policy/crate-release.json').read_text())
    require(set(value) == {'schema_version', 'repository', 'repository_id', 'workflow', 'environment',
                           'toolchain', 'crates', 'historical', 'excluded', 'required_checks'}, 'policy fields')
    require(value['schema_version'] == 1 and re.fullmatch(r'seraphinastudios/[a-z0-9-]+', value['repository'])
            and re.fullmatch(r'[1-9][0-9]*', value['repository_id'])
            and value['workflow'] == 'publish-crate.yml' and value['environment'] == 'crates-io-release'
            and VERSION.fullmatch(value['toolchain']), 'policy identity')
    require(isinstance(value['required_checks'], list) and 0 < len(value['required_checks']) <= 16
            and len(set(value['required_checks'])) == len(value['required_checks'])
            and all(isinstance(name, str) and 0 < len(name) <= 128 for name in value['required_checks']), 'required check policy')
    require(isinstance(value['crates'], list) and 0 < len(value['crates']) <= 32, 'crate inventory bound')
    names, paths = set(), set()
    for crate in value['crates']:
        require(set(crate) == {'name', 'path'} and NAME.fullmatch(crate['name']), 'crate policy fields')
        safe_path(crate['path'])
        require(crate['name'] not in names and crate['path'] not in paths, 'duplicate crate policy')
        names.add(crate['name']); paths.add(crate['path'])
    require(isinstance(value['historical'], list) and len(value['historical']) <= 128, 'history bound')
    seen = set()
    for row in value['historical']:
        require(set(row) == {'name', 'version', 'source_tag', 'revision', 'asset', 'sha256'}, 'history fields')
        validate_coordinate(row)
        require(row['name'] in names and SHA.fullmatch(row['revision']) and
                re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+', row['source_tag']), 'history identity')
        coordinate = (row['name'], row['version'])
        require(coordinate not in seen, 'duplicate history')
        seen.add(coordinate)
    # Source releases whose crate versions must never reach the registry.
    require(isinstance(value['excluded'], list) and len(value['excluded']) <= 32, 'exclusion bound')
    tags, ids, versions = set(), set(), set()
    for row in value['excluded']:
        require(isinstance(row, dict) and set(row) == {'source_tag', 'release_id', 'version'}
                and all(isinstance(row[key], str) for key in row)
                and re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+', row['source_tag'])
                and re.fullmatch(r'[1-9][0-9]*', row['release_id']) and VERSION.fullmatch(row['version']),
                'exclusion fields')
        require(row['source_tag'] not in tags and row['release_id'] not in ids and row['version'] not in versions,
                'duplicate exclusion')
        tags.add(row['source_tag']); ids.add(row['release_id']); versions.add(row['version'])
    require(not any(row['version'] in versions for row in value['historical']), 'historical coordinate excluded')
    return value


def excluded_coordinate(config, name, version):
    """Family crates at an excluded release's version never reach the registry."""
    return (any(row['version'] == version for row in config['excluded'])
            and any(crate['name'] == name for crate in config['crates']))


def excluded_release(config, release, manifest=None):
    """Whether policy permanently excludes this source release from registry publication.

    Its tag and release ID must both match one entry, and its crates must all be
    at that entry's version. A partial match is a policy or identity error.
    """
    matches = [row for row in config['excluded']
               if row['source_tag'] == release.get('tag_name') or row['release_id'] == str(release.get('id'))]
    if not matches:
        return False
    require(len(matches) == 1 and matches[0]['source_tag'] == release.get('tag_name')
            and matches[0]['release_id'] == str(release.get('id')), 'excluded release identity mismatch')
    require(manifest is None or all(row['version'] == matches[0]['version'] for row in manifest['crates']),
            'excluded release version mismatch')
    return True


def validate_coordinate(row):
    require(isinstance(row.get('name'), str) and NAME.fullmatch(row['name'])
            and isinstance(row.get('version'), str) and VERSION.fullmatch(row['version'])
            and row.get('asset') == f"{row['name']}-{row['version']}.crate"
            and isinstance(row.get('sha256'), str) and DIGEST.fullmatch(row['sha256']), 'crate coordinate')


def boundary(config, upload=False):
    require(not os.environ.get('CARGO_REGISTRIES_CRATES_IO_TOKEN') and
            not os.environ.get('CARGO_REGISTRY_BOOTSTRAP_TOKEN'), 'unexpected registry credential')
    if not upload:
        require(not os.environ.get('CARGO_REGISTRY_TOKEN'), 'registry credential outside upload')
    if upload or os.environ.get('GITHUB_ACTIONS') == 'true':
        expected = {'GITHUB_ACTIONS': 'true', 'GITHUB_REPOSITORY': config['repository'],
                    'GITHUB_REPOSITORY_ID': config['repository_id'], 'GITHUB_REF': 'refs/heads/main',
                    'GITHUB_EVENT_NAME': 'workflow_dispatch',
                    'GITHUB_WORKFLOW_REF': f"{config['repository']}/.github/workflows/{config['workflow']}@refs/heads/main"}
        require(all(os.environ.get(key) == value for key, value in expected.items()), 'publication workflow boundary')
        require(SHA.fullmatch(os.environ.get('GITHUB_SHA', '')) and
                os.environ.get('GITHUB_WORKFLOW_SHA') == os.environ['GITHUB_SHA'], 'workflow revision boundary')
        require(subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
                == os.environ['GITHUB_SHA'], 'checkout revision boundary')
        require(not subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT), 'dirty publication checkout')


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError('redirect refused')


class PublicRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        parsed = urllib.parse.urlparse(newurl)
        require(parsed.scheme == 'https' and parsed.hostname in {
            'github.com', 'release-assets.githubusercontent.com', 'objects.githubusercontent.com',
            'crates.io', 'static.crates.io', 'index.crates.io'}, 'download redirect refused')
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def get(url, absent=False, maximum=MAX_JSON, token=None):
    headers = {'User-Agent': 'tilewright-crate-release/1 (https://github.com/seraphinastudios)', 'Accept': 'application/json'}
    token = token or os.environ.get('GH_READ_TOKEN')
    authenticated = url.startswith('https://api.github.com/') and bool(token)
    if authenticated:
        headers['Authorization'] = 'Bearer ' + token
    request = urllib.request.Request(url, headers=headers)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect() if authenticated else PublicRedirect())
    try:
        with opener.open(request, timeout=30) as response:
            data = response.read(maximum + 1)
            require(len(data) <= maximum, 'response bound exceeded')
            return data
    except urllib.error.HTTPError as error:
        code = error.code
        error.close()
        if absent and code == 404:
            return None
        raise ValueError('public request failed') from None


def api(config, suffix):
    root = f"https://api.github.com/repos/{config['repository']}"
    return json.loads(get(root + ('/' + suffix if suffix else '')))


def github_intent_read(config, suffix, token, *, absent=False, readback=False):
    """Fresh intent-authorized reads; only transient reads may be retried."""
    require(isinstance(token, str) and 0 < len(token) <= 8192
            and all(33 <= ord(char) <= 126 for char in token), 'missing GitHub intent authority')
    url = f"https://api.github.com/repos/{config['repository']}/{suffix}"
    request = urllib.request.Request(url, headers={'Authorization': 'Bearer ' + token,
        'Accept': 'application/vnd.github+json', 'User-Agent': 'tilewright-crate-release/1',
        'Cache-Control': 'no-cache', 'Pragma': 'no-cache'})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    for attempt in range(3):
        try:
            with opener.open(request, timeout=30) as response:
                require(response.status == 200 and response.geturl() == url, 'GitHub intent read response')
                raw = response.read(MAX_JSON + 1)
                require(len(raw) <= MAX_JSON, 'GitHub intent read bound')
                value = json.loads(raw)
                require(isinstance(value, dict), 'GitHub intent read object')
                return value
        except urllib.error.HTTPError as error:
            code = error.code
            error.close()
            if absent and code == 404:
                return None
            require(code in (502, 503, 504) or (readback and code == 404), 'GitHub intent read failed')
        except (urllib.error.URLError, TimeoutError):
            pass
        require(attempt < 2, 'GitHub intent read unresolved')
        time.sleep(2)


def github_write(config, suffix, body, token):
    """One bounded API mutation. Ambiguous responses are never retried."""
    require(isinstance(token, str) and token and '\n' not in token, 'missing GitHub intent authority')
    request = urllib.request.Request(f"https://api.github.com/repos/{config['repository']}/{suffix}",
        data=canonical(body), method='POST', headers={'Authorization': 'Bearer ' + token,
        'Accept': 'application/vnd.github+json', 'Content-Type': 'application/json',
        'User-Agent': 'tilewright-crate-release/1'})
    with urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect()).open(request, timeout=30) as response:
        require(response.status == 201, 'GitHub intent creation not acknowledged')
        raw = response.read(MAX_JSON + 1)
        require(len(raw) <= MAX_JSON, 'GitHub intent response bound')
        return json.loads(raw)


def verifier_environment(directory):
    # No registry or GitHub write credential reaches gh or its config files.
    return {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'HOME': str(directory),
            'GH_CONFIG_DIR': str(directory / 'gh-config'), 'GH_PROMPT_DISABLED': '1',
            'GH_NO_UPDATE_NOTIFIER': '1', 'NO_COLOR': '1', 'LANG': 'C.UTF-8'}


def decode_snappy(raw):
    """Bounded Snappy raw block, as served by GitHub's attestation storage."""
    require(0 < len(raw) <= MAX_JSON, 'compressed attestation bound')
    size, shift, position = 0, 0, 0
    while True:
        require(position < len(raw) and shift <= 28, 'Snappy length header')
        byte = raw[position]; position += 1
        size |= (byte & 127) << shift
        if byte < 128:
            break
        shift += 7
    require(0 < size <= MAX_JSON and position == (size.bit_length() + 6) // 7, 'expanded attestation bound')
    output = bytearray()
    while position < len(raw):
        tag = raw[position]; position += 1
        kind, length = tag & 3, tag >> 2
        if kind == 0:
            if length >= 60:
                count = length - 59
                require(position + count <= len(raw), 'Snappy literal length')
                length = int.from_bytes(raw[position:position + count], 'little')
                position += count
            length += 1
            require(position + length <= len(raw) and len(output) + length <= size, 'Snappy literal bound')
            output.extend(raw[position:position + length]); position += length
        else:
            count = {1: 1, 2: 2, 3: 4}[kind]
            require(position + count <= len(raw), 'Snappy copy offset')
            offset = int.from_bytes(raw[position:position + count], 'little')
            position += count
            if kind == 1:
                offset |= (tag & 224) << 3
                length = 4 + ((tag >> 2) & 7)
            else:
                length += 1
            require(0 < offset <= len(output) and len(output) + length <= size, 'Snappy copy bound')
            while length:
                chunk = min(offset, length)
                output.extend(output[len(output) - offset:len(output) - offset + chunk])
                length -= chunk
    require(len(output) == size, 'Snappy decoded length mismatch')
    return bytes(output)


def attestation_bundle(config, item):
    require(str(item.get('repository_id', config['repository_id'])) == config['repository_id'],
            'attestation repository mismatch')
    if item.get('bundle_url'):
        url = urllib.parse.urlparse(item['bundle_url'])
        require(url.scheme == 'https' and url.netloc == 'tmaproduction.blob.core.windows.net'
            and not url.fragment and re.fullmatch(r'/attestations/' + config['repository_id']
                + r'/[0-9]{4}/[0-9]{2}/[0-9]{2}/[1-9][0-9]*\.json\.sn', url.path),
            'attestation storage URL refused')
        # GitHub credentials are sent only to api.github.com by get().
        value = json.loads(decode_snappy(get(item['bundle_url'], maximum=MAX_JSON)))
    else:
        value = item.get('bundle')
    require(isinstance(value, dict) and 0 < len(canonical(value)) <= MAX_JSON, 'attestation bundle bound')
    return value


def verified_attestation(config, raw, revision):
    """Verify signatures with the preinstalled official CLI, then certificate policy."""
    envelope = api(config, 'attestations/sha256:' + digest(raw)
                   + '?per_page=30&predicate_type=https%3A%2F%2Fslsa.dev%2Fprovenance%2Fv1')
    bundles = envelope.get('attestations', [])
    require(isinstance(bundles, list) and 0 < len(bundles) <= 30, 'attestation inventory bound')
    with tempfile.TemporaryDirectory(prefix='tilewright-crate-attestation-') as temporary:
        directory = Path(temporary)
        manifest_path, bundle_path, output_path = (directory / name for name in ('crate-release.json', 'bundles.jsonl', 'verified.json'))
        manifest_path.write_bytes(raw)
        encoded_bundles = bytearray()
        for item in bundles:
            encoded = canonical(attestation_bundle(config, item)) + b'\n'
            require(len(encoded_bundles) + len(encoded) <= MAX_JSON, 'combined attestation bundle bound')
            encoded_bundles.extend(encoded)
        bundle_path.write_bytes(encoded_bundles)
        version = subprocess.run(['gh', 'version'], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            env=verifier_environment(directory), timeout=10)
        parsed_version = re.match(rb'gh version ([0-9]+)\.([0-9]+)\.([0-9]+)', version.stdout)
        require(version.returncode == 0 and parsed_version is not None
            and tuple(map(int, parsed_version.groups())) >= (2, 68, 0), 'GitHub CLI 2.68 or later required')
        def bounded_output():
            resource.setrlimit(resource.RLIMIT_FSIZE, (MAX_JSON, MAX_JSON))
        command = ['gh', 'attestation', 'verify', str(manifest_path), '--bundle', str(bundle_path),
                   '--repo', config['repository'], '--signer-workflow', config['repository'] + '/.github/workflows/release.yml',
                   '--source-ref', 'refs/heads/main', '--source-digest', revision, '--signer-digest', revision,
                   '--deny-self-hosted-runners', '--format', 'json']
        with output_path.open('wb') as output:
            result = subprocess.run(command, stdout=output, stderr=subprocess.DEVNULL,
                env=verifier_environment(directory), timeout=180, preexec_fn=bounded_output)
        require(result.returncode == 0 and output_path.stat().st_size <= MAX_JSON, 'attestation signature verification failed')
        verified = json.loads(output_path.read_bytes())
    return attestation_identity(config, verified, revision, digest(raw))


def attestation_identity(config, verified, revision, manifest_sha256):
    require(isinstance(verified, list) and 0 < len(verified) <= 30, 'verified attestation response')
    expected_repo = 'https://github.com/' + config['repository']
    expected_workflow = expected_repo + '/.github/workflows/release.yml@refs/heads/main'
    producers = set()
    for item in verified:
        value = item['verificationResult']
        cert = value['signature']['certificate']
        statement = value['statement']
        require(cert['issuer'] == 'https://token.actions.githubusercontent.com'
            and cert['sourceRepositoryURI'] == expected_repo
            and str(cert['sourceRepositoryIdentifier']) == config['repository_id']
            and cert['sourceRepositoryRef'] == 'refs/heads/main'
            and cert['sourceRepositoryDigest'] == revision
            and cert['buildConfigURI'] == expected_workflow
            and cert['buildConfigDigest'] == revision
            and cert['subjectAlternativeName'] == expected_workflow
            and cert['buildSignerURI'] == expected_workflow
            and cert['buildSignerDigest'] == revision
            and cert['runnerEnvironment'] == 'github-hosted', 'attested certificate identity mismatch')
        require(statement['predicateType'] == 'https://slsa.dev/provenance/v1'
            and any(subject.get('digest') == {'sha256': manifest_sha256} for subject in statement['subject']),
            'attestation subject mismatch')
        invocation = re.fullmatch(re.escape(expected_repo) + r'/actions/runs/([1-9][0-9]*)/attempts/([1-9][0-9]*)', cert['runInvocationURI'])
        require(invocation is not None, 'attested run invocation mismatch')
        producers.add((int(invocation[1]), int(invocation[2])))
    return [{'run_id': run_id, 'run_attempt': attempt} for run_id, attempt in sorted(producers, reverse=True)]


def evidence_jobs_succeeded(config, producer, revision):
    """The attempt's prepare and attest jobs, which built and signed the evidence.

    The later release job only publishes the release and dispatches the
    publisher, so its failure (for example while the publisher workflow is
    disabled) leaves the attested evidence intact.
    """
    suffix = f"actions/runs/{producer['run_id']}/attempts/{producer['run_attempt']}/jobs?per_page=100"
    inventory = api(config, suffix)
    jobs = inventory.get('jobs')
    require(isinstance(jobs, list) and inventory.get('total_count') == len(jobs) <= 100, 'producing job inventory')
    for name in EVIDENCE_JOBS:
        matching = [job for job in jobs if job.get('name') == name]
        require(len(matching) == 1, 'attested evidence job missing or duplicated')
        job = matching[0]
        require(job.get('run_id') == producer['run_id'] and job.get('run_attempt') == producer['run_attempt']
                and job.get('head_sha') == revision, 'attested evidence job identity mismatch')
        if job.get('status') != 'completed' or job.get('conclusion') != 'success':
            return False
    return True


def producing_run(config, producers, revision):
    workflow_id = api(config, 'actions/workflows/release.yml')['id']
    for count in range(24):
        active = False
        for producer in producers:
            suffix = f"actions/runs/{producer['run_id']}/attempts/{producer['run_attempt']}"
            run = api(config, suffix)
            require(run['id'] == producer['run_id'] and run['run_attempt'] == producer['run_attempt']
                and run['workflow_id'] == workflow_id and run['head_sha'] == revision
                and run['head_branch'] == 'main' and run['event'] in ('push', 'workflow_dispatch')
                and str(run['repository']['id']) == config['repository_id']
                and str(run['head_repository']['id']) == config['repository_id'], 'attested producing run identity mismatch')
            # A finished attempt counts when its evidence jobs succeeded, whatever its later jobs did.
            if run['status'] == 'completed' and evidence_jobs_succeeded(config, producer, revision):
                return producer
            active = active or run['status'] != 'completed'
        require(active, 'no successful attested producing run')
        require(count < 23, 'attested producing run completion timed out')
        time.sleep(5)


def reserve_upload(config, release, row, revision, manifest_sha256, token):
    """Coordinate-global, atomic and persistent intent, before any registry PUT."""
    validate_coordinate(row)
    require(not excluded_coordinate(config, row['name'], row['version']), 'excluded crate coordinate')
    ref = f"refs/tags/crate-upload-intent/{row['name']}/{row['version']}"
    suffix = ref.removeprefix('refs/')
    stage = 'authenticated precheck'
    try:
        require(github_intent_read(config, 'git/ref/' + suffix, token, absent=True) is None,
                'existing upload intent requires explicit owner recovery')
        producer = release['_verified_producer']
        intent = {'schema_version': 1, 'kind': 'tilewright-crate-upload-intent', 'repository_id': config['repository_id'],
            'name': row['name'], 'version': row['version'], 'archive_sha256': row['sha256'],
            'manifest_sha256': manifest_sha256, 'source_revision': revision, 'source_release_id': release['id'],
            'producer': producer, 'nonce': secrets.token_hex(32)}
        stage = 'authenticated source tree read'
        tree = github_intent_read(config, f'git/commits/{revision}', token)['tree']['sha']
        require(SHA.fullmatch(tree), 'intent tree identity')
        stage = 'commit creation'
        commit = github_write(config, 'git/commits', {'message': canonical(intent).decode(), 'tree': tree, 'parents': [revision]}, token)
        require(SHA.fullmatch(commit.get('sha', '')), 'intent commit identity')
        stage = 'atomic ref creation'
        created = github_write(config, 'git/refs', {'ref': ref, 'sha': commit['sha']}, token)
        require(created.get('ref') == ref and created.get('object', {}).get('sha') == commit['sha'], 'intent acknowledgement mismatch')
        stage = 'authenticated ref readback'
        confirmed = github_intent_read(config, 'git/ref/' + suffix, token, readback=True)
        require(confirmed.get('ref') == ref and confirmed.get('object', {}).get('type') == 'commit'
            and confirmed['object']['sha'] == commit['sha'], 'intent readback mismatch')
        return commit['sha']
    except BaseException:
        # Only local stage labels are printed, never remote bodies or exceptions.
        print(f'Upload intent reservation stopped during {stage}. No registry PUT was attempted for this'
              ' coordinate in this run. Any created intent remains reserved; explicit owner recovery is required.',
              file=sys.stderr)
        raise


def host_write_authority(config, expected_owner):
    """Use the separately authenticated owner GitHub session, never a crate token."""
    environment = {name: value for name, value in os.environ.items()
        if not name.startswith(('CARGO_REGISTR', 'ACTIONS_ID_TOKEN')) and name != 'GH_INTENT_TOKEN'}
    result = subprocess.run(['gh', 'auth', 'token', '--hostname', 'github.com'],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=environment, timeout=30)
    require(result.returncode == 0 and 0 < len(result.stdout) <= 8192, 'owner GitHub authentication required')
    token = result.stdout.decode().strip()
    owner = json.loads(get('https://api.github.com/user', token=token))
    repo = json.loads(get('https://api.github.com/repos/' + config['repository'], token=token))
    require(owner['login'] == expected_owner and str(repo['id']) == config['repository_id']
        and repo['full_name'] == config['repository'] and repo['permissions']['push'] is True,
        'owner GitHub identity or write authority mismatch')
    return token


def asset_bytes(config, release, name, maximum=MAX_BYTES):
    rows = [asset for asset in release['assets'] if asset['name'] == name]
    require(len(rows) == 1, 'missing or duplicate release asset')
    row = rows[0]
    expected = f"https://github.com/{config['repository']}/releases/download/{release['tag_name']}/{name}"
    require(row['state'] == 'uploaded' and row['browser_download_url'] == expected and
            isinstance(row['size'], int) and 0 < row['size'] <= maximum, 'release asset identity')
    data = get(expected, maximum=maximum)
    require(len(data) == row['size'] and row.get('digest') == 'sha256:' + digest(data), 'release asset checksum')
    return data


def archive(data, name, version):
    require(len(data) <= MAX_BYTES, 'archive bound')
    files = {}
    prefix = f'{name}-{version}/'
    total = 0
    # Bound decompression BEFORE tarfile consumes PAX/GNU extension headers.
    with gzip.GzipFile(fileobj=io.BytesIO(data)) as compressed:
        expanded = compressed.read(MAX_BYTES + 1)
    require(len(expanded) <= MAX_BYTES, 'expanded tar stream bound')
    with tarfile.open(fileobj=io.BytesIO(expanded), mode='r:') as stream:
        for member in stream:
            require(len(files) < MAX_FILES and member.isfile() and member.name.startswith(prefix), 'archive member type')
            path = safe_path(member.name[len(prefix):])
            require(path not in files, 'duplicate archive member')
            total += member.size
            require(0 <= member.size <= MAX_BYTES and total <= MAX_BYTES, 'expanded archive bound')
            files[path] = stream.extractfile(member).read()
    require({'Cargo.toml', 'Cargo.toml.orig', 'Cargo.lock', '.cargo_vcs_info.json'} <= files.keys(), 'incomplete package')
    return files


def content_digest(files):
    return digest(canonical([[name, len(data), digest(data)] for name, data in sorted(files.items())
                             if name != '.cargo_vcs_info.json']))


def string(value, nullable=False):
    require((nullable and value is None) or (isinstance(value, str) and len(value) <= MAX_JSON), 'metadata string')
    return value


def metadata(files, name, version):
    """Cargo Web API metadata from a normalized manifest, including target deps."""
    manifest = tomllib.loads(files['Cargo.toml'].decode())
    require(set(manifest) <= {'package', 'lib', 'bin', 'example', 'test', 'bench', 'dependencies',
            'dev-dependencies', 'build-dependencies', 'features', 'target', 'badges', 'lints', 'profile'},
            'unsupported normalized manifest section')
    package = manifest['package']
    require(package['name'] == name and package['version'] == version and package.get('publish') == ['crates-io'],
            'normalized package identity')
    allowed = {'name', 'version', 'edition', 'rust-version', 'build', 'publish', 'autolib', 'autobins',
               'autoexamples', 'autotests', 'autobenches', 'description', 'homepage', 'documentation',
               'readme', 'keywords', 'license', 'license-file', 'repository', 'authors', 'categories',
               'links', 'include', 'exclude', 'metadata', 'default-run', 'resolver'}
    require(set(package) <= allowed, 'unsupported package field')
    dependencies = []
    def add(table, target=None):
        for section, kind in [('dependencies', 'normal'), ('dev-dependencies', 'dev'), ('build-dependencies', 'build')]:
            for alias, dependency in sorted(table.get(section, {}).items()):
                if isinstance(dependency, str):
                    dependency = {'version': dependency}
                require(isinstance(dependency, dict) and set(dependency) <= {
                    'version', 'features', 'optional', 'default-features', 'package'}, 'unsupported dependency source/field')
                requirement = string(dependency.get('version'))
                require(requirement and NAME.fullmatch(alias), 'dependency version/name')
                features = dependency.get('features', [])
                require(isinstance(features, list) and all(isinstance(f, str) for f in features), 'dependency features')
                require(isinstance(dependency.get('optional', False), bool) and
                        isinstance(dependency.get('default-features', True), bool), 'dependency flags')
                dependencies.append({'name': dependency.get('package', alias), 'version_req': requirement,
                    'features': features, 'optional': dependency.get('optional', False),
                    'default_features': dependency.get('default-features', True), 'target': target,
                    'kind': kind, 'registry': None,
                    'explicit_name_in_toml': alias if 'package' in dependency else None})
    add(manifest)
    for target, table in sorted(manifest.get('target', {}).items()):
        require(set(table) <= {'dependencies', 'dev-dependencies', 'build-dependencies'}, 'unsupported target section')
        add(table, target)
    features = manifest.get('features', {})
    require(isinstance(features, dict) and all(isinstance(k, str) and isinstance(v, list)
            and all(isinstance(item, str) for item in v) for k, v in features.items()), 'package features')
    readme_path = package.get('readme')
    if readme_path is False or readme_path is None:
        readme_path = None
        readme = None
    else:
        safe_path(readme_path)
        require(readme_path in files, 'missing package readme')
        readme = files[readme_path].decode()
    license_file = package.get('license-file')
    if license_file:
        require(safe_path(license_file) in files, 'missing package license')
    result = {'name': name, 'vers': version, 'deps': dependencies, 'features': features,
              'readme': readme, 'readme_file': readme_path, 'license_file': license_file,
              'badges': manifest.get('badges', {}), 'rust_version': package.get('rust-version')}
    for key in ('description', 'documentation', 'homepage', 'license', 'repository', 'links'):
        result[key] = string(package.get(key), nullable=True)
    for key in ('authors', 'keywords', 'categories'):
        value = package.get(key, [])
        require(isinstance(value, list) and all(isinstance(item, str) for item in value), 'metadata list')
        result[key] = value
    require(len(canonical(result)) <= MAX_JSON, 'metadata bound')
    return result


def validate_manifest(config, value):
    require(set(value) == {'schema_version', 'repository', 'repository_id', 'revision', 'source_tag', 'crates'}
            and value['schema_version'] == 1 and value['repository'] == config['repository']
            and value['repository_id'] == config['repository_id'] and SHA.fullmatch(value['revision'])
            and re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+', value['source_tag']), 'manifest identity')
    require(isinstance(value['crates'], list) and len(value['crates']) <= len(config['crates']), 'manifest inventory bound')
    allowed = {row['name']: row['path'] for row in config['crates']}
    seen = set()
    for row in value['crates']:
        require(set(row) == {'name', 'version', 'path', 'asset', 'sha256', 'content_sha256', 'inventory', 'metadata'}, 'manifest crate fields')
        validate_coordinate(row)
        require(row['name'] in allowed and row['path'] == allowed[row['name']] and row['name'] not in seen, 'manifest allowlist')
        require(DIGEST.fullmatch(row['content_sha256']) and isinstance(row['inventory'], list)
                and 0 < len(row['inventory']) <= MAX_FILES, 'manifest content inventory')
        require(row['inventory'] == sorted(set(row['inventory'])), 'manifest inventory ordering')
        for name in row['inventory']:
            safe_path(name)
        require(row['metadata']['name'] == row['name'] and row['metadata']['vers'] == row['version'], 'metadata identity')
        seen.add(row['name'])
    ordered(value['crates'])
    return value


def verify_archive(row, data, revision):
    require(digest(data) == row['sha256'], 'archive digest mismatch')
    files = archive(data, row['name'], row['version'])
    require(sorted(files) == row['inventory'] and content_digest(files) == row['content_sha256'], 'archive inventory/content mismatch')
    require(json.loads(files['.cargo_vcs_info.json']) == {'git': {'sha1': revision}, 'path_in_vcs': row['path']}, 'archive source provenance')
    require(metadata(files, row['name'], row['version']) == row['metadata'], 'archive metadata mismatch')
    lock = tomllib.loads(files['Cargo.lock'].decode())
    for dependency in lock['package']:
        if (dependency['name'], dependency['version']) == (row['name'], row['version']):
            require('source' not in dependency, 'root lock source')
        else:
            require(dependency.get('source') == 'registry+https://github.com/rust-lang/crates.io-index', 'nonregistry locked dependency')


def ordered(rows):
    """Internal normal/build/optional/target dependencies precede their consumers."""
    by_name = {row['name']: row for row in rows}
    require(len(by_name) == len(rows), 'duplicate crate')
    edges = {name: {dep['name'] for dep in row['metadata']['deps']
                    if dep['kind'] != 'dev' and dep['name'] in by_name} for name, row in by_name.items()}
    output = []
    while edges:
        ready = sorted(name for name, dependencies in edges.items() if not dependencies)
        require(ready, 'cyclic publication dependencies')
        for name in ready:
            output.append(by_name[name]); del edges[name]
        for dependencies in edges.values():
            dependencies.difference_update(ready)
    return output


def index_path(name):
    return '1/' + name if len(name) == 1 else ('2/' + name if len(name) == 2 else
           ('3/' + name[0] + '/' + name if len(name) == 3 else name[:2] + '/' + name[2:4] + '/' + name))


def registry_state(row):
    raw_api = get(f"{REGISTRY}/api/v1/crates/{row['name']}/{row['version']}", absent=True)
    raw_index = get(f"{INDEX}/{index_path(row['name'])}", absent=True, maximum=MAX_BYTES)
    api_row = None if raw_api is None else json.loads(raw_api)['version']
    require(api_row is None or (api_row.get('crate') == row['name'] and api_row.get('num') == row['version']),
            'registry API coordinate mismatch')
    matches = [] if raw_index is None else [json.loads(line) for line in raw_index.splitlines()
                if line and json.loads(line)['vers'] == row['version']]
    require(len(matches) <= 1, 'duplicate index version')
    index_row = matches[0] if matches else None
    require(index_row is None or index_row.get('name') == row['name'], 'registry index coordinate mismatch')
    for value, key in ((api_row, 'checksum'), (index_row, 'cksum')):
        if value is not None:
            require(value[key] == row['sha256'] and not value.get('yanked', False), 'registry checksum/yank conflict')
    if api_row is None and index_row is None:
        return 'absent'
    if api_row is None or index_row is None:
        return 'indeterminate'
    data = get(f"https://static.crates.io/crates/{row['name']}/{row['name']}-{row['version']}.crate", maximum=MAX_BYTES)
    require(digest(data) == row['sha256'], 'registry download mismatch')
    return 'present'


def confirm(rows, attempts=12):
    for attempt in range(attempts):
        if all(registry_state(row) == 'present' for row in rows):
            return
        if attempt + 1 < attempts:
            time.sleep(5)
    raise ValueError('registry readiness unresolved; no upload retry')


def release_identity(config, release):
    require(not release['draft'] and not release['prerelease'] and release.get('published_at')
            and release.get('immutable') is True, 'source release must be public and immutable')
    require(re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+', release['tag_name']), 'source tag format')
    repo = api(config, '')
    require(str(repo['id']) == config['repository_id'] and repo['full_name'] == config['repository']
            and repo['default_branch'] == 'main', 'repository identity changed')
    branch = api(config, 'branches/main')
    require(branch['protected'] is True, 'main is not protected')


def verify_checks(config, revision):
    check_runs = api(config, f'commits/{revision}/check-runs?per_page=100&filter=latest')
    require(check_runs['total_count'] <= 100, 'source check inventory exceeds bound')
    for name in config['required_checks']:
        matching = [check for check in check_runs['check_runs'] if check['name'] == name
                    and check['head_sha'] == revision and check['app']['slug'] == 'github-actions']
        require(matching, 'required source check absent')
        latest = max(matching, key=lambda check: check['id'])
        require(latest['status'] == 'completed' and latest['conclusion'] == 'success', 'required source check not successful')


def release_evidence(config, release_id, revision, manifest_sha256):
    require(str(release_id).isdigit() and SHA.fullmatch(revision) and DIGEST.fullmatch(manifest_sha256), 'release selectors')
    release = api(config, f'releases/{release_id}')
    require(str(release['id']) == str(release_id), 'release ID mismatch')
    require(not excluded_release(config, release), 'source release excluded from registry publication')
    release_identity(config, release)
    raw = asset_bytes(config, release, MANIFEST, MAX_JSON)
    require(digest(raw) == manifest_sha256, 'manifest digest mismatch')
    manifest = validate_manifest(config, json.loads(raw))
    require(manifest['revision'] == revision and manifest['source_tag'] == release['tag_name'], 'manifest source mismatch')
    require(not any(excluded_coordinate(config, row['name'], row['version']) for row in manifest['crates']),
            'excluded crate coordinate')
    ref = api(config, 'git/ref/tags/' + release['tag_name'])['object']
    for _ in range(4):
        if ref['type'] != 'tag':
            break
        ref = api(config, 'git/tags/' + ref['sha'])['object']
    require(ref['type'] == 'commit' and ref['sha'] == revision, 'tag revision mismatch')
    comparison = api(config, f'compare/{revision}...main')
    require(comparison['status'] in ('identical', 'ahead'), 'source revision is not on main')
    producer = producing_run(config, verified_attestation(config, raw, revision), revision)
    release['_verified_producer'] = producer
    verify_checks(config, revision)
    sums = asset_bytes(config, release, 'SHA256SUMS', MAX_JSON).decode()
    checksums = {}
    for line in sums.splitlines():
        match = re.fullmatch(r'([0-9a-f]{64})  ([A-Za-z0-9_.+-]+)', line)
        require(match is not None and match[2] not in checksums, 'release checksum inventory invalid')
        checksums[match[2]] = match[1]
    assets = release['assets']
    require(len({row['name'] for row in assets}) == len(assets) and
            set(checksums) == {row['name'] for row in assets if row['name'] != 'SHA256SUMS'}, 'incomplete source checksum inventory')
    for asset in assets:
        if asset['name'] != 'SHA256SUMS':
            require(asset.get('state') == 'uploaded' and asset.get('digest') == 'sha256:' + checksums[asset['name']],
                    'source asset digest disagrees with checksum inventory')
    require(checksums[MANIFEST] == manifest_sha256, 'manifest absent from source checksums')
    snapshots = {}
    for row in manifest['crates']:
        require(checksums.get(row['asset']) == row['sha256'], 'crate absent from source checksums')
        data = asset_bytes(config, release, row['asset'])
        verify_archive(row, data, revision)
        snapshots[row['name']] = data
    return release, manifest, snapshots


class PutOutcome(typing.NamedTuple):
    """One registry PUT. Only HTTP 429 is a definite refusal: crates.io checks its
    rate limits before it reads the archive, and commits nothing for a 429."""
    accepted: bool
    rate_limited: bool = False
    retry_after: str | None = None  # normalized UTC time, never registry text
    http_status: int | None = None
    category: str = 'unknown'  # fixed local label, never exception/response text

    def __bool__(self):
        # Keeps the former boolean meaning: true only for an acknowledged upload.
        return self.accepted


def http_date(value):
    if not re.fullmatch(HTTP_DATE, value):
        return None
    try:
        return email.utils.parsedate_to_datetime(value)
    except (TypeError, ValueError, OverflowError):
        return None


def retry_time(headers, body):
    """crates.io's retry time from Retry-After or the error detail's "try again
    after" text. Only a strictly parsed time is reformatted for the terminal."""
    value = (headers.get('Retry-After') or '').strip() if headers is not None else ''
    if re.fullmatch(r'[0-9]{1,6}', value):
        moment = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(seconds=int(value))
    else:
        moment = http_date(value)
        if moment is None:
            found = re.search('try again after (' + HTTP_DATE + ')', body.decode('utf-8', 'replace'))
            moment = found and http_date(found[1])
    return moment.astimezone(datetime.timezone.utc).strftime('%Y-%m-%d %H:%M:%S UTC') if moment else None


class PutNoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # urllib raises HTTPError with the original status when this returns None.
        return None


def put_once(data, metadata_value, token):
    encoded = canonical(metadata_value)
    payload = struct.pack('<I', len(encoded)) + encoded + struct.pack('<I', len(data)) + data
    endpoint = REGISTRY + '/api/v1/crates/new'
    request = urllib.request.Request(endpoint, data=payload, method='PUT', headers={
        'Authorization': token, 'Content-Type': 'application/octet-stream', 'Accept': 'application/json',
        'User-Agent': 'tilewright-crate-release/1 (https://github.com/seraphinastudios)'})
    status = None
    try:
        with urllib.request.build_opener(urllib.request.ProxyHandler({}), PutNoRedirect()).open(request, timeout=30) as response:
            status = response.status
            raw = response.read(65537)
            if response.geturl() != endpoint or not 200 <= status < 300 or len(raw) > 65536:
                return PutOutcome(False, http_status=status, category='invalid_response')
            value = json.loads(raw)
            accepted = isinstance(value, dict) and 'errors' not in value
            return PutOutcome(accepted, http_status=status,
                              category='acknowledged' if accepted else 'invalid_response')
    except urllib.error.HTTPError as error:
        retry_after = None
        if error.code == 429:
            try:
                retry_after = retry_time(error.headers, error.read(65536))
            except Exception:
                pass  # The status alone is definite; its retry time is optional.
        try:
            error.close()
        except Exception:
            pass  # Cleanup cannot erase the observed HTTP status.
        return PutOutcome(False, error.code == 429, retry_after, error.code, 'http_error')
    except (json.JSONDecodeError, UnicodeDecodeError):
        category = 'invalid_response'
    except TimeoutError:
        category = 'timeout'
    except ConnectionError:
        category = 'connection_error'
    except urllib.error.URLError as error:
        category = ('timeout' if isinstance(error.reason, TimeoutError) else
                    'connection_error' if isinstance(error.reason, ConnectionError) else 'transport_error')
    except Exception:
        category = 'unexpected_error'
    return PutOutcome(False, http_status=status, category=category)


def publication_receipts():
    """Validate local receipt configuration before reserving or uploading."""
    attempt = {}
    for key, variable in (('run_id', 'GITHUB_RUN_ID'), ('run_attempt', 'GITHUB_RUN_ATTEMPT')):
        value = os.environ.get(variable, '')
        require(re.fullmatch(r'[1-9][0-9]{0,19}', value), 'publication run identity')
        attempt[key] = int(value)
    revision = os.environ.get('GITHUB_SHA', '')
    require(SHA.fullmatch(revision), 'publication workflow revision')
    attempt['workflow_revision'] = revision
    directory = os.environ.get('TILEWRIGHT_PUBLISH_RECEIPTS', '')
    require(directory and Path(directory).is_absolute(), 'publication receipt directory required')
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    return directory, attempt


def record_put_outcome(directory, attempt, config, release, row, revision, manifest_sha256, intent, outcome):
    """Diagnostic evidence only: never permission to retry or retire an intent."""
    receipt = {'schema_version': 1, 'kind': 'tilewright-crate-put-outcome',
        'repository_id': config['repository_id'], 'name': row['name'], 'version': row['version'],
        'source_release_id': release['id'], 'source_revision': revision,
        'manifest_sha256': manifest_sha256, 'archive_sha256': row['sha256'],
        'intent_ref': f"refs/tags/crate-upload-intent/{row['name']}/{row['version']}",
        'intent_commit': intent, 'producer': release['_verified_producer'], 'publisher': attempt,
        'http_status': outcome.http_status, 'category': outcome.category,
        'result': 'acknowledged' if outcome.accepted else 'rate_limited' if outcome.rate_limited else 'uncertain',
        'retry_after': outcome.retry_after}
    try:
        encoded = canonical(receipt).decode('ascii') + '\n'
        # No raw server bodies, headers, URLs or exception strings enter the receipt.
        print(encoded, end='', file=sys.stderr, flush=True)
        path = directory / f"{row['name']}-{row['version']}.json"
        with path.open('x', encoding='ascii') as stream:
            stream.write(encoded)
            stream.flush()
            os.fsync(stream.fileno())
    except Exception:
        print('Publication receipt persistence failed after a PUT. Its outcome remains unresolved;'
              ' keep the upload intent reserved.', file=sys.stderr)
        raise ValueError('publication receipt persistence failed; intent retained') from None


def release_snapshot(release):
    """Immutable identity only; GitHub download counters and notes can change."""
    identity = {key: release.get(key) for key in ('id', 'node_id', 'url', 'html_url',
        'tag_name', 'target_commitish', 'draft', 'prerelease', 'immutable', 'published_at')}
    identity['assets'] = sorted(({key: asset.get(key) for key in ('id', 'node_id',
        'name', 'size', 'digest', 'state', 'content_type', 'url', 'browser_download_url')}
        for asset in release.get('assets', [])), key=lambda asset: (str(asset['id']), str(asset['name'])))
    return identity


def registry_source_branch():
    if os.environ.get('GITHUB_ACTIONS') == 'true':
        ref = os.environ.get('GITHUB_REF', '')
        require(ref.startswith('refs/heads/'), 'source release must use a branch')
        branch = ref.removeprefix('refs/heads/')
        require(os.environ.get('TILEWRIGHT_RELEASE_BRANCH', branch) == branch, 'source branch identity drift')
    else:
        branch = os.environ.get('TILEWRIGHT_RELEASE_BRANCH', 'main')
    require(branch == 'main' or re.fullmatch(r'[0-9]+\.[0-9]+\.x', branch), 'unsupported source release branch')
    return branch == 'main'


def upload(config, release_id, revision, manifest_sha256):
    token = os.environ.pop('CARGO_REGISTRY_TOKEN', None)
    intent_token = os.environ.pop('GH_INTENT_TOKEN', None)
    boundary(config, upload=True)
    require(isinstance(token, str) and 0 < len(token) <= 8192 and all(33 <= ord(c) <= 126 for c in token), 'missing upload credential')
    release, manifest, snapshots = release_evidence(config, release_id, revision, manifest_sha256)
    receipts, attempt = publication_receipts()
    for row in ordered(manifest['crates']):
        state = registry_state(row)
        if state == 'present':
            continue
        require(state == 'absent', 'registry state indeterminate; no write')
        # No source execution, registry credential subprocess or repackaging.
        require(release_snapshot(api(config, f'releases/{release_id}')) == release_snapshot(release),
                'release changed during upload preflight')
        intent = reserve_upload(config, release, row, revision, manifest_sha256, intent_token)
        outcome = put_once(snapshots[row['name']], row['metadata'], token)
        record_put_outcome(receipts, attempt, config, release, row, revision, manifest_sha256, intent, outcome)
        # Both acknowledged and uncertain PUTs are reconciled by reads only.
        # A timeout aborts this graph; never perform a second PUT in this run.
        confirm([row])
    return manifest


def legacy_manifest(config, value, tag):
    """A manifest this repository published under an earlier name.

    GitHub keeps a repository's ID across renames and transfers, so a manifest
    with the same ID, a different well-formed name, and a crate family disjoint
    from every current and historical name belongs to an earlier identity.
    Such a release cannot own any coordinate this policy manages and is
    skipped. Anything else must pass `validate_manifest`.
    """
    if not isinstance(value, dict) or value.get('repository') == config['repository']:
        return False
    require(set(value) == {'schema_version', 'repository', 'repository_id', 'revision', 'source_tag', 'crates'}
            and value['schema_version'] == 1 and value['repository_id'] == config['repository_id']
            and isinstance(value['repository'], str)
            and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9._-]+', value['repository'])
            and SHA.fullmatch(value['revision']) and value['source_tag'] == tag
            and isinstance(value['crates'], list), 'legacy manifest identity')
    managed = {row['name'] for row in config['crates']} | {row['name'] for row in config['historical']}
    for row in value['crates']:
        require(isinstance(row, dict) and isinstance(row.get('name'), str)
                and row['name'] not in managed, 'legacy manifest overlaps managed crates')
    return True


def catalog(config):
    found = {}
    for page in range(1, 11):
        releases = api(config, f'releases?per_page=100&page={page}')
        require(isinstance(releases, list), 'release inventory response')
        for release in releases:
            if release['draft'] or release['prerelease']:
                continue
            if not any(row['name'] == MANIFEST for row in release['assets']):
                continue
            raw = asset_bytes(config, release, MANIFEST, MAX_JSON)
            value = json.loads(raw)
            if legacy_manifest(config, value, release['tag_name']):
                continue
            manifest = validate_manifest(config, value)
            require(manifest['source_tag'] == release['tag_name'], 'catalog source tag')
            if manifest['crates']:
                require(release.get('immutable') is True, 'mutable automatic source release')
            for row in manifest['crates']:
                key = (row['name'], row['version'])
                require(key not in found, 'duplicate crate source lineage')
                found[key] = (release, manifest, row, digest(raw))
        if len(releases) < 100:
            return found
    raise ValueError('release inventory exceeds pagination bound')


def reconcile(config):
    """Dispatch the newest eligible release with absent crates; excluded releases never."""
    seen = set()
    for release, manifest, row, manifest_sha256 in catalog(config).values():
        if release['id'] in seen:
            continue
        seen.add(release['id'])
        if excluded_release(config, release, manifest):
            continue
        require(not any(excluded_coordinate(config, item['name'], item['version']) for item in manifest['crates']),
                'excluded crate coordinate')
        if any(registry_state(item) != 'present' for item in manifest['crates']):
            # Each dispatch is an explicit side effect; no publication credential here.
            dispatch(config, release, manifest, manifest_sha256)
            # One release per run keeps failure/retry and concurrency bounded.
            break


def dispatch(config, release, manifest, manifest_sha256):
    token = os.environ.pop('GH_TOKEN', None)
    require(token and os.environ.get('GITHUB_REPOSITORY') == config['repository']
            and os.environ.get('GITHUB_REPOSITORY_ID') == config['repository_id']
            and os.environ.get('GITHUB_REF') == 'refs/heads/main', 'dispatch identity')
    body = canonical({'ref': 'main', 'inputs': {'operation': 'automatic', 'source_release_id': str(release['id']),
                      'source_revision': manifest['revision'], 'manifest_sha256': manifest_sha256}})
    request = urllib.request.Request(f"https://api.github.com/repos/{config['repository']}/actions/workflows/{config['workflow']}/dispatches",
        data=body, method='POST', headers={'Authorization': 'Bearer ' + token, 'Accept': 'application/vnd.github+json',
                                        'Content-Type': 'application/json', 'User-Agent': 'tilewright-crate-release/1'})
    with urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect()).open(request, timeout=30) as response:
        require(response.status == 204, 'dispatch response unresolved')


def finalize(config, source_tag, directory):
    """Inert pre-tag check of same-run artifacts, before semantic-release writes."""
    require(re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+', source_tag or ''), 'final source tag')
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    require(os.environ.get('EXPECTED_SOURCE_REVISION', revision) == revision and
            os.environ.get('EXPECTED_SOURCE_VERSION', source_tag[1:]) == source_tag[1:], 'source plan drift')
    manifest = validate_manifest(config, json.loads((directory / MANIFEST).read_text()))
    require(manifest['revision'] == revision and manifest['source_tag'] == source_tag, 'prepared source identity')
    require(registry_source_branch() or not manifest['crates'], 'maintenance release cannot own registry coordinates')
    provenance = json.loads((directory / 'provenance.json').read_text())
    require(provenance['revision'] == revision and provenance['version'] == source_tag[1:], 'prepared provenance')
    checksums = {}
    for line in (directory / 'SHA256SUMS').read_text().splitlines():
        match = re.fullmatch(r'([0-9a-f]{64})  (?:\./)?(.+)', line)
        require(match is not None, 'prepared checksum syntax')
        path = safe_path(match[2])
        require(path not in checksums, 'duplicate prepared checksum')
        checksums[path] = match[1]
    actual = {str(path.relative_to(directory)) for path in directory.rglob('*') if path.is_file()
              and path.name != 'SHA256SUMS'}
    require(set(checksums) == actual, 'prepared inventory coverage')
    for path, expected in checksums.items():
        file = directory / path
        require(not file.is_symlink() and file.stat().st_size <= 1024 * MAX_BYTES, 'prepared file boundary')
        with file.open('rb') as stream:
            actual_digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        require(actual_digest == expected, 'prepared asset digest')
    for row in manifest['crates']:
        path = directory / row['asset']
        if not path.is_file():
            path = directory / 'crates' / row['asset']
        verify_archive(row, path.read_bytes(), revision)
    return manifest


def new_crate_burst(value):
    """argparse type for --burst: from 0 to crates.io's new-crate burst."""
    if not re.fullmatch(r'[0-9]{1,2}', value) or int(value) > NEW_CRATE_BURST:
        raise argparse.ArgumentTypeError(f'expected an integer from 0 to {NEW_CRATE_BURST}')
    return int(value)


def wait_for_new_crate_token(row, position, total):
    """One full refill period plus margin, so the next new name has a token."""
    remaining = NEW_CRATE_RATE_SECONDS + NEW_CRATE_MARGIN_SECONDS
    while remaining > 0:
        print(f"crates.io new-crate rate limit: {remaining // 60}m{remaining % 60:02d}s until reserving "
              f"{row['name']}@{row['version']} ({position} of {total}).", file=sys.stderr)
        step = min(PROGRESS_SECONDS, remaining)
        time.sleep(step)
        remaining -= step


def report_put_failure(config, row, outcome):
    """Owner recovery for a stopped upload. Intents are never removed here."""
    ref = f"refs/tags/crate-upload-intent/{row['name']}/{row['version']}"
    remove = f"gh api --method DELETE repos/{config['repository']}/git/{ref}"
    rerun = 'Re-run this bootstrap with --burst 0; it skips crates that are already published.'
    coordinate = f"{row['name']}@{row['version']}"
    if outcome.rate_limited:
        when = (f'crates.io accepts another new crate name after {outcome.retry_after}.' if outcome.retry_after
                else 'crates.io gave no retry time; wait at least 10 minutes.')
        lines = [f'crates.io refused {coordinate} with HTTP 429 (rate limit) before accepting it.', when,
                 f'Its intent {ref} is still reserved and blocks another attempt. After that time:',
                 f"  1. Confirm {row['name']} is still absent: {REGISTRY}/api/v1/crates/{row['name']} returns 404"
                 ' and your crates.io dashboard does not list it.',
                 f'  2. Remove the intent with your owner GitHub session: {remove}', f'  3. {rerun}']
    elif outcome.accepted:
        lines = [f'crates.io acknowledged {coordinate}, but registry reads did not confirm it in time.',
                 f'Keep its intent {ref}: a published coordinate keeps its reservation permanently.',
                 'Re-run this bootstrap with --burst 0 once the crate is visible; it skips published crates.']
    else:
        lines = [f'The upload of {coordinate} has no definite result, and registry reads did not confirm it.',
                 f'Its intent {ref} stays reserved: crates.io may still have accepted the write,'
                 ' and public absence alone does not prove that it did not.',
                 f"If {coordinate} appears with sha256 {row['sha256']}, keep the intent and re-run this bootstrap.",
                 'Only after authoritative confirmation that crates.io accepted no write, for example from'
                 f' crates.io support, remove the intent: {remove}', rerun]
    for line in lines:
        print(line, file=sys.stderr)


def recently_created(name, now):
    """Whether crates.io created NAME inside the new-crate window; an unreadable time counts as recent."""
    crate = json.loads(get(f'{REGISTRY}/api/v1/crates/{name}'))['crate']
    try:
        return (now - datetime.datetime.fromisoformat(crate['created_at'])).total_seconds() < NEW_CRATE_WINDOW_SECONDS
    except (KeyError, TypeError, ValueError, OverflowError):
        return True


def bootstrap(config, release_id, revision, manifest_sha256, expected_owner, expected_owner_id,
              burst=NEW_CRATE_BURST):
    """One-time owner-terminal bootstrap; never an Actions fallback credential."""
    require(not os.environ.get('GITHUB_ACTIONS') and not os.environ.get('CI') and
            not os.environ.get('ACTIONS_ID_TOKEN_REQUEST_URL') and
            not any(os.environ.get(key) for key in ('CARGO_REGISTRY_TOKEN',
                'CARGO_REGISTRIES_CRATES_IO_TOKEN', 'CARGO_REGISTRY_BOOTSTRAP_TOKEN')),
            'bootstrap requires a local owner terminal without environment credentials')
    require(sys.stdin.isatty() and sys.stderr.isatty(), 'bootstrap requires interactive terminal')
    require(re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9-]{0,63}', expected_owner or '')
            and str(expected_owner_id).isdigit(), 'explicit owner identity required')
    require(type(burst) is int and 0 <= burst <= NEW_CRATE_BURST, 'new-crate burst out of range')
    require(not subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT), 'dirty bootstrap checkout')
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    require(api(config, f'compare/{head}...main')['status'] in ('identical', 'ahead'), 'bootstrap tooling must be reviewed on main')
    release, manifest, snapshots = release_evidence(config, release_id, revision, manifest_sha256)
    historical = {row['name'] for row in config['historical']}
    require(manifest['crates'] and all(row['name'] not in historical for row in manifest['crates']),
            'bootstrap cannot publish established historical crate families')
    public_owner = json.loads(get(f'{REGISTRY}/api/v1/users/{expected_owner}'))['user']
    require(str(public_owner['id']) == str(expected_owner_id) and public_owner['login'] == expected_owner,
            'bootstrap owner identity mismatch')
    def owned(name):
        users = json.loads(get(f'{REGISTRY}/api/v1/crates/{name}/owners'))['users']
        require(any(user.get('kind') == 'user' and str(user.get('id')) == str(expected_owner_id)
                    and user.get('login') == expected_owner for user in users), 'bootstrap owner mismatch')
    to_publish, recent = [], 0
    now = datetime.datetime.now(datetime.timezone.utc)
    for row in ordered(manifest['crates']):
        if registry_state(row) == 'present':
            owned(row['name'])
            # A resumed run: names published here moments ago still hold crates.io tokens.
            recent += recently_created(row['name'], now)
            continue
        # A name that exists at any other coordinate is not a first publication.
        require(get(f"{REGISTRY}/api/v1/crates/{row['name']}", absent=True) is None,
                'bootstrap name already exists; configure Trusted Publishing instead')
        to_publish.append(row)
    if not to_publish:
        return
    if recent:
        print(f'{recent} of these crates were first published within the last {NEW_CRATE_WINDOW_SECONDS // 60}'
              f' minutes; lowering --burst from {burst} to {max(0, burst - recent)}.', file=sys.stderr)
        burst = max(0, burst - recent)
    coordinates = ', '.join(f"{row['name']}@{row['version']}" for row in to_publish)
    print(f'First publication of {coordinates} as {expected_owner} ({expected_owner_id}).', file=sys.stderr)
    paced = max(0, len(to_publish) - burst)
    if paced:
        wait = NEW_CRATE_RATE_SECONDS + NEW_CRATE_MARGIN_SECONDS
        print(f'Pacing for the crates.io new-crate rate limit: {len(to_publish) - paced} at once, then {paced}'
              f' after a {wait} s wait each (at least {paced * wait // 60} minutes).', file=sys.stderr)
    print('Use a short-lived publish-new token created in this owner account; no token identity API is available.', file=sys.stderr)
    require(input(f'Type PUBLISH AS {expected_owner}: ') == f'PUBLISH AS {expected_owner}', 'bootstrap not confirmed')
    github_token = host_write_authority(config, expected_owner)
    token = getpass.getpass('One-use crates.io token (hidden): ')
    require(0 < len(token) <= 8192 and all(33 <= ord(char) <= 126 for char in token), 'invalid bootstrap token')
    sent = 0
    attempted = False
    stage = 'pre-upload checks'
    try:
        for position, row in enumerate(to_publish, 1):
            attempted = False
            stage = 'pre-upload checks'
            if position > burst:
                # Pace before the checks and intent: a refused PUT would strand its reservation.
                wait_for_new_crate_token(row, position, len(to_publish))
            require(release_snapshot(api(config, f'releases/{release_id}')) == release_snapshot(release),
                    'bootstrap source changed')
            require(registry_state(row) == 'absent' and
                    get(f"{REGISTRY}/api/v1/crates/{row['name']}", absent=True) is None,
                    'bootstrap coordinate is no longer absent')
            stage = 'upload intent reservation'
            reserve_upload(config, release, row, revision, manifest_sha256, github_token)
            stage = 'registry PUT attempt'
            attempted = True
            sent += 1  # Counted before the PUT: an interrupted request may still have reached crates.io.
            outcome = put_once(snapshots[row['name']], row['metadata'], token)
            if outcome.rate_limited:
                report_put_failure(config, row, outcome)
                raise ValueError('crates.io rate limit refused the upload; intent retained')
            if not outcome.accepted:
                print(f"crates.io did not acknowledge {row['name']}@{row['version']}; checking registry reads.",
                      file=sys.stderr)
            stage = 'registry confirmation'
            try:
                confirm([row])
            except Exception:
                report_put_failure(config, row, outcome)
                raise
            stage = 'owner verification'
            owned(row['name'])
    except BaseException:
        print(f'Bootstrap stopped during {stage}. ' +
              ('A registry PUT was attempted for the current crate; its outcome may be unresolved.' if attempted else
               'No registry PUT was attempted for the current crate in this run.'), file=sys.stderr)
        if sent:
            print(f'This run sent {sent} new crate upload(s) to crates.io. If you re-run within the next hour,'
                  ' pass --burst 0.', file=sys.stderr)
        raise
    print(f'crates.io counts these {sent} new names for the next hour; pass --burst 0 to any other'
          ' bootstrap with this account before then.', file=sys.stderr)
    print('Bootstrap confirmed; revoke this one-use token and register Trusted Publishing.', file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['verify', 'upload', 'confirm', 'dispatch', 'reconcile', 'finalize', 'bootstrap'])
    parser.add_argument('--release-id'); parser.add_argument('--revision'); parser.add_argument('--manifest-sha256')
    parser.add_argument('--source-tag'); parser.add_argument('--archives', type=Path)
    parser.add_argument('--expected-owner'); parser.add_argument('--expected-owner-id')
    parser.add_argument('--burst', type=new_crate_burst, default=NEW_CRATE_BURST, help=(
        f'bootstrap: new crate names to upload before pacing to one per {NEW_CRATE_RATE_SECONDS // 60} minutes'
        f' (0-{NEW_CRATE_BURST}, default {NEW_CRATE_BURST}). Lower it if this crates.io account published'
        f' other new crate names in the last hour: {NEW_CRATE_BURST} minus that count, or 0 when unsure.'
        " This release's own recently published crates are counted automatically."))
    args = parser.parse_args()
    config = policy()
    if args.operation == 'bootstrap':
        bootstrap(config, args.release_id, args.revision, args.manifest_sha256, args.expected_owner,
                  args.expected_owner_id, args.burst)
    elif args.operation == 'finalize':
        require(args.archives is not None, 'prepared directory required')
        finalize(config, args.source_tag, args.archives)
    elif args.operation in ('verify', 'confirm'):
        boundary(config)
        _, manifest, _ = release_evidence(config, args.release_id, args.revision, args.manifest_sha256)
        if args.operation == 'confirm':
            confirm(manifest['crates'])
        else:
            for row in manifest['crates']:
                require(registry_state(row) != 'indeterminate', 'registry state unresolved')
    elif args.operation == 'upload':
        upload(config, args.release_id, args.revision, args.manifest_sha256)
    elif args.operation == 'dispatch':
        if os.environ.get('GITHUB_REF') != 'refs/heads/main':
            return
        require(re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+', args.source_tag or ''), 'dispatch source tag')
        release = api(config, 'releases/tags/' + args.source_tag)
        raw = asset_bytes(config, release, MANIFEST, MAX_JSON)
        manifest = validate_manifest(config, json.loads(raw))
        if manifest['crates']:
            dispatch(config, release, manifest, digest(raw))
    else:
        reconcile(config)
    print(json.dumps({'operation': args.operation, 'status': 'complete'}))


if __name__ == '__main__':
    try:
        main()
    except Exception:
        # Never emit credentials, registry responses or attacker-controlled metadata.
        print('Crate release failed validation or remains unresolved.', file=sys.stderr)
        raise SystemExit(1)
