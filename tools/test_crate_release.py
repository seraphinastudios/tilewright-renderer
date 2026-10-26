#!/usr/bin/env python3
"""Negative tests for the portable exact-archive publication boundary."""
from __future__ import annotations

import copy
from contextlib import ExitStack, redirect_stdout
import email.message
import gzip
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import threading
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace
import unittest
import urllib.error
from unittest.mock import MagicMock, patch

from tools import crate_release as R
from tools import crate_prepare as P

REVISION = 'a' * 40
CONFIG = {'schema_version': 1, 'repository': 'seraphinastudios/example', 'repository_id': '123',
          'workflow': 'publish-crate.yml', 'environment': 'crates-io-release', 'toolchain': '1.97.1',
          'required_checks': ['Example validation'], 'crates': [{'name': 'example', 'path': 'crates/example'}], 'historical': []}


def files():
    return {'Cargo.toml': b'''[package]
name="example"
version="0.1.0"
publish=["crates-io"]
license="MIT"
readme="README.md"
[lib]
path="src/lib.rs"
[dependencies.alias]
package="actual"
version="^1.2"
optional=true
features=["feature"]
default-features=false
[target.'cfg(windows)'.build-dependencies]
cc="1"
[dev-dependencies]
testing="2"
[features]
default=["dep:alias"]
''', 'Cargo.toml.orig': b'[package]\n', 'Cargo.lock': b'''version = 4
[[package]]
name = "example"
version = "0.1.0"
[[package]]
name = "actual"
version = "1.2.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
''', '.cargo_vcs_info.json': R.canonical({'git': {'sha1': REVISION}, 'path_in_vcs': 'crates/example'}),
        'README.md': b'Example.', 'src/lib.rs': b'pub fn example() {}\n'}


def make_archive(members=None, special=None):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode='w:gz') as stream:
        for name, data in (files() if members is None else members).items():
            member = tarfile.TarInfo('example-0.1.0/' + name)
            member.size = len(data)
            stream.addfile(member, io.BytesIO(data))
        if special:
            stream.addfile(special)
    return output.getvalue()


def row():
    data = make_archive()
    contents = R.archive(data, 'example', '0.1.0')
    return {'name': 'example', 'version': '0.1.0', 'path': 'crates/example',
            'asset': 'example-0.1.0.crate', 'sha256': R.digest(data),
            'content_sha256': R.content_digest(contents), 'inventory': sorted(contents),
            'metadata': R.metadata(contents, 'example', '0.1.0')}, data


class Terminal(io.StringIO):
    """Captured stderr that still passes the bootstrap's interactive check."""
    def isatty(self):
        return True


class ApiEndpointTests(unittest.TestCase):
    def test_repository_root_has_no_trailing_slash(self):
        with patch.object(R, 'get', return_value=b'{}') as get:
            R.api(CONFIG, '')
            get.assert_called_once_with('https://api.github.com/repos/seraphinastudios/example')
        with patch.object(R, 'get', return_value=b'{}') as get:
            R.api(CONFIG, 'releases/123')
            get.assert_called_once_with('https://api.github.com/repos/seraphinastudios/example/releases/123')


class MetadataTests(unittest.TestCase):
    def test_full_dependency_mapping(self):
        value = R.metadata(files(), 'example', '0.1.0')
        alias, dev, build = value['deps']
        self.assertEqual((alias['name'], alias['explicit_name_in_toml']), ('actual', 'alias'))
        self.assertTrue(alias['optional']); self.assertFalse(alias['default_features'])
        self.assertEqual(alias['features'], ['feature'])
        self.assertEqual((dev['kind'], build['kind'], build['target']), ('dev', 'build', 'cfg(windows)'))
        self.assertEqual(value['features'], {'default': ['dep:alias']})

    def test_no_path_git_or_unknown_dependency_field(self):
        for addition in ('path="../sibling"', 'git="https://example.org/repo"', 'registry="other"', 'future=true'):
            with self.subTest(addition=addition):
                value = files()
                value['Cargo.toml'] = value['Cargo.toml'].replace(b'package="actual"', ('package="actual"\n' + addition).encode())
                with self.assertRaisesRegex(ValueError, 'dependency source'):
                    R.metadata(value, 'example', '0.1.0')

    def test_metadata_cannot_lie_about_archive(self):
        entry, data = row()
        entry['metadata']['deps'][0]['optional'] = False
        with self.assertRaisesRegex(ValueError, 'metadata mismatch'):
            R.verify_archive(entry, data, REVISION)


class ArchiveTests(unittest.TestCase):
    def test_valid_snapshot(self):
        entry, data = row()
        R.verify_archive(entry, data, REVISION)

    def test_revision_inventory_and_hash_must_match(self):
        entry, data = row()
        for key, value in [('sha256', '0' * 64), ('inventory', ['Cargo.toml']), ('content_sha256', '0' * 64)]:
            changed = dict(entry, **{key: value})
            with self.assertRaises(ValueError):
                R.verify_archive(changed, data, REVISION)
        with self.assertRaisesRegex(ValueError, 'source provenance'):
            R.verify_archive(entry, data, 'b' * 40)

    def test_unsafe_links_duplicates_and_paths_rejected(self):
        for name, kind in [('example-0.1.0/../outside', tarfile.REGTYPE),
                           ('example-0.1.0/src/lib.rs', tarfile.REGTYPE),
                           ('example-0.1.0/link', tarfile.SYMTYPE)]:
            with self.subTest(name=name):
                member = tarfile.TarInfo(name); member.type = kind
                with self.assertRaises(ValueError):
                    R.archive(make_archive(special=member), 'example', '0.1.0')

    def test_pax_expansion_is_bounded_before_parser(self):
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode='w', format=tarfile.PAX_FORMAT,
                          pax_headers={'comment': 'a' * 100000}) as stream:
            member = tarfile.TarInfo('example-0.1.0/Cargo.toml')
            stream.addfile(member)
        compressed = gzip.compress(output.getvalue())
        with patch.object(R, 'MAX_BYTES', 64000), self.assertRaisesRegex(ValueError, 'tar stream bound'):
            R.archive(compressed, 'example', '0.1.0')

    def test_content_hash_ignores_only_revision_metadata(self):
        value = files(); original = R.content_digest(value)
        value['.cargo_vcs_info.json'] = b'different revision'
        self.assertEqual(R.content_digest(value), original)
        value['Cargo.lock'] += b'\n# dependency graph changed\n'
        self.assertNotEqual(R.content_digest(value), original)

    def test_locked_git_dependency_rejected(self):
        entry, _ = row(); contents = files()
        contents['Cargo.lock'] = contents['Cargo.lock'].replace(b'registry+https://github.com/rust-lang/crates.io-index', b'git+https://example.com')
        data = make_archive(contents)
        entry.update(sha256=R.digest(data), content_sha256=R.content_digest(contents))
        with self.assertRaisesRegex(ValueError, 'nonregistry'):
            R.verify_archive(entry, data, REVISION)


class GraphTests(unittest.TestCase):
    def test_leaves_before_optional_target_build_dependency(self):
        leaf = {'name': 'leaf', 'metadata': {'deps': []}}
        top = {'name': 'top', 'metadata': {'deps': [{'name': 'leaf', 'kind': 'build', 'optional': True}]}}
        self.assertEqual([item['name'] for item in R.ordered([top, leaf])], ['leaf', 'top'])

    def test_cycles_fail_before_upload(self):
        values = [{'name': a, 'metadata': {'deps': [{'name': b, 'kind': 'normal'}]}}
                  for a, b in [('a', 'b'), ('b', 'a')]]
        with self.assertRaisesRegex(ValueError, 'cyclic'):
            R.ordered(values)


class RegistryTests(unittest.TestCase):
    def observations(self, entry, data, api=True, index=True, checksum=None, yanked=False):
        checksum = checksum or entry['sha256']
        return [R.canonical({'version': {'crate': entry['name'], 'num': entry['version'], 'checksum': checksum, 'yanked': yanked}}) if api else None,
                R.canonical({'name': entry['name'], 'vers': entry['version'], 'cksum': checksum, 'yanked': yanked}) if index else None,
                data]

    def test_registry_requires_both_views_and_download(self):
        entry, data = row()
        for api, index, state in [(False, False, 'absent'), (True, False, 'indeterminate'),
                                  (False, True, 'indeterminate'), (True, True, 'present')]:
            with self.subTest(api=api, index=index), patch.object(R, 'get', side_effect=self.observations(entry, data, api, index)):
                self.assertEqual(R.registry_state(entry), state)

    def test_registry_archive_uses_exact_canonical_cdn_url(self):
        entry, data = row()
        with patch.object(R, 'get', side_effect=self.observations(entry, data)) as get:
            self.assertEqual(R.registry_state(entry), 'present')
        self.assertEqual(get.call_args_list[-1].args,
                         ('https://static.crates.io/crates/example/example-0.1.0.crate',))
        self.assertEqual(get.call_args_list[-1].kwargs, {'maximum': R.MAX_BYTES})

    def test_cdn_wrong_archive_or_json_response_cannot_confirm_publication(self):
        entry, _ = row()
        for data in (b'changed archive', R.canonical({
                'url': 'https://static.crates.io/crates/example/example-0.1.0.crate'})):
            with self.subTest(data=data), patch.object(R, 'get', side_effect=self.observations(entry, data)):
                with self.assertRaisesRegex(ValueError, 'registry download mismatch'):
                    R.registry_state(entry)

    def test_conflicting_checksum_yanked_or_download_mismatch_fail(self):
        entry, data = row()
        for observations in [self.observations(entry, data, checksum='0' * 64),
                             self.observations(entry, data, yanked=True),
                             self.observations(entry, b'changed')]:
            with patch.object(R, 'get', side_effect=observations), self.assertRaises(ValueError):
                R.registry_state(entry)

    def test_uncertain_upload_never_puts_twice_or_advances(self):
        entry, data = row()
        manifest = {'crates': [entry]}
        release = {'id': 10}
        with patch.object(R, 'boundary'), patch.object(R, 'reserve_upload'), patch.object(R, 'release_evidence', return_value=(release, manifest, {'example': data})), \
             patch.object(R, 'registry_state', return_value='absent'), patch.object(R, 'api', return_value=release), \
             patch.object(R, 'put_once', return_value=False) as put, \
             patch.object(R, 'confirm', side_effect=ValueError('unresolved')), \
             patch.dict(os.environ, {'CARGO_REGISTRY_TOKEN': 'test-token'}):
            with self.assertRaisesRegex(ValueError, 'unresolved'):
                R.upload(CONFIG, 10, REVISION, 'b' * 64)
            self.assertEqual(put.call_count, 1)

    def test_download_counts_may_change_but_asset_identity_may_not(self):
        entry, data = row()
        release = {'id': 10, 'tag_name': 'v1.0.0', 'immutable': True,
                   'assets': [{'id': 20, 'name': entry['asset'], 'size': len(data),
                               'digest': 'sha256:' + entry['sha256'], 'state': 'uploaded', 'download_count': 0}]}
        for key, value, permitted in [('download_count', 4, True), ('digest', 'sha256:' + '0' * 64, False),
                                      ('name', 'other.crate', False), ('id', 21, False), ('size', 0, False)]:
            changed = copy.deepcopy(release); changed['assets'][0][key] = value
            changed['updated_at'] = 'later'; changed['body'] = 'Edited release notes'
            with self.subTest(key=key), patch.object(R, 'boundary'), patch.object(R, 'reserve_upload'), \
                 patch.object(R, 'release_evidence', return_value=(release, {'crates': [entry]}, {'example': data})), \
                 patch.object(R, 'registry_state', return_value='absent'), patch.object(R, 'api', return_value=changed), \
                 patch.object(R, 'put_once', return_value=True) as put, patch.object(R, 'confirm'), \
                 patch.dict(os.environ, {'CARGO_REGISTRY_TOKEN': 'test-token'}):
                if permitted:
                    R.upload(CONFIG, 10, REVISION, 'b' * 64)
                    put.assert_called_once()
                else:
                    with self.assertRaisesRegex(ValueError, 'release changed'):
                        R.upload(CONFIG, 10, REVISION, 'b' * 64)
                    put.assert_not_called()

    def test_already_present_skips_upload(self):
        entry, data = row()
        with patch.object(R, 'boundary'), patch.object(R, 'reserve_upload') as reserve, patch.object(R, 'release_evidence', return_value=({}, {'crates': [entry]}, {'example': data})), \
             patch.object(R, 'registry_state', return_value='present'), patch.object(R, 'put_once') as put, \
             patch.dict(os.environ, {'CARGO_REGISTRY_TOKEN': 'test-token'}):
            R.upload(CONFIG, 10, REVISION, 'b' * 64)
            put.assert_not_called()
            reserve.assert_not_called()


class AttestationTests(unittest.TestCase):
    def statement(self):
        repo = 'https://github.com/' + CONFIG['repository']
        workflow = repo + '/.github/workflows/release.yml@refs/heads/main'
        return [{'verificationResult': {'signature': {'certificate': {
            'issuer': 'https://token.actions.githubusercontent.com', 'sourceRepositoryURI': repo,
            'sourceRepositoryIdentifier': '123', 'sourceRepositoryRef': 'refs/heads/main',
            'sourceRepositoryDigest': REVISION, 'buildConfigURI': workflow, 'buildConfigDigest': REVISION,
            'subjectAlternativeName': workflow, 'buildSignerURI': workflow,
            'buildSignerDigest': REVISION, 'runnerEnvironment': 'github-hosted',
            'runInvocationURI': repo + '/actions/runs/100/attempts/2'}},
            'statement': {'predicateType': 'https://slsa.dev/provenance/v1',
                'subject': [{'name': 'crate-release.json', 'digest': {'sha256': 'b' * 64}}],
                'predicate': {'runDetails': {'metadata': {'invocationId': 'untrusted'}}}}}}]

    def test_certificate_not_predicate_binds_actual_producing_run(self):
        statement = self.statement()
        self.assertEqual(R.attestation_identity(CONFIG, statement, REVISION, 'b' * 64), [{'run_id': 100, 'run_attempt': 2}])
        for field, value in [('issuer', 'https://attacker.invalid'), ('sourceRepositoryIdentifier', '999'),
                             ('sourceRepositoryRef', 'refs/pull/1/merge'), ('sourceRepositoryDigest', 'c' * 40),
                             ('buildConfigDigest', 'c' * 40), ('buildConfigURI', 'other.yml'),
                             ('buildSignerDigest', 'c' * 40), ('buildSignerURI', 'other.yml'),
                             ('subjectAlternativeName', 'other.yml'), ('runnerEnvironment', 'self-hosted'),
                             ('runInvocationURI', 'https://github.com/other/repo/actions/runs/100/attempts/2')]:
            changed = copy.deepcopy(statement)
            changed[0]['verificationResult']['signature']['certificate'][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                R.attestation_identity(CONFIG, changed, REVISION, 'b' * 64)
        with self.assertRaisesRegex(ValueError, 'subject mismatch'):
            R.attestation_identity(CONFIG, statement, REVISION, 'c' * 64)

    PRODUCER = {'run_id': 100, 'run_attempt': 2}

    def producing_run_record(self, **change):
        return dict(dict(self.PRODUCER, id=100, workflow_id=9, head_sha=REVISION, head_branch='main', event='push',
                         repository={'id': 123}, head_repository={'id': 123}, status='completed',
                         conclusion='success'), **change)

    def evidence_jobs(self, *jobs, release='success'):
        """The producing attempt's job inventory: prepare, attest and the publishing release job."""
        rows = list(jobs) if jobs else [self.job(name) for name in R.EVIDENCE_JOBS]
        rows.append(self.job('Release renderer', conclusion=release))
        return {'total_count': len(rows), 'jobs': rows}

    def job(self, name, **change):
        return dict({'name': name, 'run_id': 100, 'run_attempt': 2, 'head_sha': REVISION,
                     'status': 'completed', 'conclusion': 'success'}, **change)

    def test_only_attested_run_attempt_with_successful_evidence_is_accepted(self):
        with patch.object(R, 'api', side_effect=[{'id': 9}, self.producing_run_record(), self.evidence_jobs()]) as api:
            self.assertEqual(R.producing_run(CONFIG, [self.PRODUCER], REVISION), self.PRODUCER)
        self.assertEqual(api.call_args_list[-1].args[1], 'actions/runs/100/attempts/2/jobs?per_page=100')
        for change in [{'id': 101}, {'run_attempt': 1}, {'head_sha': 'c' * 40}, {'head_branch': '1.0.x'},
                       {'workflow_id': 8}, {'event': 'pull_request'}, {'repository': {'id': 456}},
                       {'head_repository': {'id': 456}}]:
            with self.subTest(change=change), self.assertRaises(ValueError), patch.object(R, 'api', side_effect=[
                    {'id': 9}, self.producing_run_record(**change), self.evidence_jobs()]):
                R.producing_run(CONFIG, [self.PRODUCER], REVISION)

    def test_failed_release_job_after_successful_evidence_is_accepted(self):
        # v1.14.0's shape: prepare and attest succeeded; the later dispatch step failed the run.
        for conclusion in ('failure', 'cancelled'):
            with self.subTest(conclusion=conclusion), patch.object(R, 'api', side_effect=[
                    {'id': 9}, self.producing_run_record(conclusion=conclusion), self.evidence_jobs(release=conclusion)]):
                self.assertEqual(R.producing_run(CONFIG, [self.PRODUCER], REVISION), self.PRODUCER)

    def test_failed_missing_or_foreign_evidence_job_is_refused(self):
        prepare, attest = R.EVIDENCE_JOBS
        cases = {
            'prepare failed': [self.job(prepare, conclusion='failure'), self.job(attest)],
            'attest failed': [self.job(prepare), self.job(attest, conclusion='failure')],
            'attest skipped': [self.job(prepare), self.job(attest, conclusion='skipped')],
            'attest unfinished': [self.job(prepare), self.job(attest, status='in_progress', conclusion=None)],
            'attest missing': [self.job(prepare)],
            'attest duplicated': [self.job(prepare), self.job(attest), self.job(attest)],
            'other attempt': [self.job(prepare), self.job(attest, run_attempt=1)],
            'other run': [self.job(prepare, run_id=99), self.job(attest)],
            'other revision': [self.job(prepare), self.job(attest, head_sha='c' * 40)],
        }
        for label, jobs in cases.items():
            with self.subTest(label=label), self.assertRaises(ValueError), patch.object(R, 'api', side_effect=[
                    {'id': 9}, self.producing_run_record(conclusion='failure'), self.evidence_jobs(*jobs)]):
                R.producing_run(CONFIG, [self.PRODUCER], REVISION)
        incomplete = dict(self.evidence_jobs(), total_count=101)
        with self.assertRaisesRegex(ValueError, 'job inventory'), patch.object(R, 'api', side_effect=[
                {'id': 9}, self.producing_run_record(), incomplete]):
            R.producing_run(CONFIG, [self.PRODUCER], REVISION)

    def test_running_attempt_waits_for_completion(self):
        running = self.producing_run_record(status='in_progress', conclusion=None)
        with patch.object(R, 'api', side_effect=[{'id': 9}, running, running,
                self.producing_run_record(conclusion='failure'), self.evidence_jobs(release='failure')]) as api, \
             patch.object(R.time, 'sleep') as sleep:
            self.assertEqual(R.producing_run(CONFIG, [self.PRODUCER], REVISION), self.PRODUCER)
        self.assertEqual(sleep.call_count, 2)
        # The jobs of an unfinished attempt are not consulted.
        self.assertEqual(sum('/jobs' in call.args[1] for call in api.call_args_list), 1)
        with patch.object(R, 'api', side_effect=lambda config, suffix: {'id': 9} if suffix.endswith('release.yml')
                          else running), patch.object(R.time, 'sleep') as sleep, \
             self.assertRaisesRegex(ValueError, 'completion timed out'):
            R.producing_run(CONFIG, [self.PRODUCER], REVISION)
        self.assertEqual(sleep.call_count, 23)

    def test_evidence_job_names_match_the_release_workflow(self):
        workflow = (Path(__file__).resolve().parent.parent / '.github/workflows/release.yml').read_text()
        for name in R.EVIDENCE_JOBS:
            self.assertIn(f'\n    name: {name}\n', workflow)

    def test_verifier_child_has_no_credentials_or_host_auth_config(self):
        payload = b'candidate'
        verified = self.statement()
        verified[0]['verificationResult']['statement']['subject'][0]['digest']['sha256'] = R.digest(payload)
        calls = []
        def command(argv, **kwargs):
            calls.append((argv, kwargs))
            if argv == ['gh', 'version']:
                return SimpleNamespace(returncode=0, stdout=b'gh version 2.102.0 (2026-09-30)\n')
            kwargs['stdout'].write(R.canonical(verified))
            return SimpleNamespace(returncode=0)
        with patch.dict(os.environ, {'CARGO_REGISTRY_TOKEN': 'crate-secret', 'GH_INTENT_TOKEN': 'write-secret',
                                     'GH_TOKEN': 'host-secret', 'GH_READ_TOKEN': 'read-secret'}), \
             patch.object(R, 'api', return_value={'attestations': [{'bundle': {'mediaType': 'test-only'}}]}), \
             patch.object(R.subprocess, 'run', side_effect=command):
            self.assertEqual(R.verified_attestation(CONFIG, payload, REVISION), [{'run_id': 100, 'run_attempt': 2}])
        for argv, kwargs in calls:
            self.assertFalse(any('TOKEN' in name for name in kwargs['env']))
            self.assertIn('tilewright-crate-attestation-', kwargs['env']['GH_CONFIG_DIR'])
        self.assertIn('--bundle', calls[-1][0])
        self.assertIn('--deny-self-hosted-runners', calls[-1][0])

    def test_snappy_all_copy_types_overlap_and_malformed_bounds(self):
        # Literal abc, COPY_1 adds abcabc, COPY_2 adds abc, COPY_4 adds abc.
        encoded = bytes([15, 8]) + b'abc' + bytes([9, 3, 10, 3, 0, 11, 3, 0, 0, 0])
        self.assertEqual(R.decode_snappy(encoded), b'abcabcabcabcabc')
        self.assertEqual(R.decode_snappy(bytes([70, 240, 69]) + b'a' * 70), b'a' * 70)
        for raw in [b'', bytes([129, 0, 0, 97]), bytes([128]) * 6, bytes([128, 128, 128, 1]),
                    bytes([1, 240]), bytes([1, 4, 97]), bytes([1, 1]), bytes([1, 2, 0]),
                    bytes([1, 3, 0, 0]), bytes([5, 0, 97, 1, 0]), bytes([5, 0, 97, 1, 2]),
                    bytes([4, 0, 97, 1, 1]), bytes([1, 0, 97, 0, 98]), bytes([2, 0, 97])]:
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                R.decode_snappy(raw)

    def test_current_storage_bundle_is_bounded_and_host_restricted(self):
        raw = b'{"mediaType":"test"}'
        compressed = bytes([len(raw), (len(raw) - 1) << 2]) + raw
        url = 'https://tmaproduction.blob.core.windows.net/attestations/123/2026/10/11/42.json.sn?sig=public'
        item = {'repository_id': 123, 'bundle': None, 'bundle_url': url}
        with patch.object(R, 'get', return_value=compressed) as fetch:
            self.assertEqual(R.attestation_bundle(CONFIG, item), {'mediaType': 'test'})
            fetch.assert_called_once_with(url, maximum=R.MAX_JSON)
        for other in [url.replace('https:', 'http:'), url.replace('.net/', '.net:443/'),
                      url.replace('.net/', '.net.evil/'), url.replace('//tma', '//user@tma'),
                      url.replace('/123/', '/456/'), url + '#fragment']:
            with patch.object(R, 'get') as fetch, self.assertRaises(ValueError):
                R.attestation_bundle(CONFIG, dict(item, bundle_url=other))
            fetch.assert_not_called()
        with self.assertRaises(ValueError):
            R.attestation_bundle(CONFIG, dict(item, repository_id=456))

    def test_missing_or_failed_signature_verification_fails_closed(self):
        with patch.object(R, 'api', return_value={'attestations': []}), self.assertRaises(ValueError):
            R.verified_attestation(CONFIG, b'forged-manifest', REVISION)
        with patch.object(R, 'api', return_value={'attestations': [{'bundle': {}}]}), \
             patch.object(R.subprocess, 'run', side_effect=[SimpleNamespace(returncode=0, stdout=b'gh version 2.102.0'),
                                                         SimpleNamespace(returncode=1)]), self.assertRaises(ValueError):
            R.verified_attestation(CONFIG, b'forged-manifest', REVISION)


class IntentTests(unittest.TestCase):
    def intent_fixture(self):
        entry, _ = row()
        release = {'id': 10, '_verified_producer': {'run_id': 100, 'run_attempt': 2}}
        return entry, release

    def test_two_interleaved_claimers_only_one_can_upload(self):
        entry, release = self.intent_fixture()
        barrier, lock = threading.Barrier(2), threading.Lock()
        state = {'ref': None, 'commits': 0, 'puts': 0}
        def absent(*args, **kwargs):
            barrier.wait(timeout=5)
            return None
        def api(config, suffix):
            if suffix.startswith('git/commits/'):
                return {'tree': {'sha': 'c' * 40}}
            with lock:
                return copy.deepcopy(state['ref'])
        def write(config, suffix, body, token):
            with lock:
                if suffix == 'git/commits':
                    state['commits'] += 1
                    return {'sha': str(state['commits']) * 40}
                if state['ref'] is not None:
                    raise ValueError('ref already exists')
                state['ref'] = {'ref': body['ref'], 'object': {'sha': body['sha'], 'type': 'commit'}}
                return copy.deepcopy(state['ref'])
        def caller():
            try:
                R.reserve_upload(CONFIG, release, entry, REVISION, 'b' * 64, 'github-token')
                with lock:
                    state['puts'] += 1
            except ValueError:
                pass
        with patch.object(R, 'get', side_effect=absent), patch.object(R, 'api', side_effect=api), \
             patch.object(R, 'github_write', side_effect=write), ThreadPoolExecutor(max_workers=2) as pool:
            list(pool.map(lambda _: caller(), range(2)))
        self.assertEqual(state['puts'], 1)
        self.assertEqual(state['commits'], 2)
        self.assertEqual(state['ref']['ref'], 'refs/tags/crate-upload-intent/example/0.1.0')

    def test_lost_claim_acknowledgement_and_new_source_cannot_retry(self):
        entry, release = self.intent_fixture()
        state = {'exists': False}
        def write(config, suffix, body, token):
            if suffix == 'git/commits': return {'sha': 'd' * 40}
            state['exists'] = True
            raise TimeoutError('response lost after server committed')
        with patch.object(R, 'get', side_effect=lambda *a, **k: b'existing' if state['exists'] else None), \
             patch.object(R, 'api', return_value={'tree': {'sha': 'c' * 40}}), \
             patch.object(R, 'github_write', side_effect=write) as writer:
            with self.assertRaises(TimeoutError):
                R.reserve_upload(CONFIG, release, entry, REVISION, 'b' * 64, 'github-token')
            for revision, checksum in [(REVISION, 'b' * 64), ('e' * 40, 'f' * 64)]:
                with self.assertRaisesRegex(ValueError, 'existing upload intent'):
                    R.reserve_upload(CONFIG, release, dict(entry, sha256=checksum), revision, checksum, 'github-token')
            self.assertEqual(writer.call_count, 2)

    def test_only_explicit_201_response_acknowledges_creation(self):
        for status in (200, 201, 202):
            response = MagicMock()
            response.__enter__.return_value = response
            response.status = status
            response.read.return_value = b'{"sha":"test"}'
            opener = MagicMock()
            opener.open.return_value = response
            with patch.object(R.urllib.request, 'build_opener', return_value=opener):
                if status == 201:
                    self.assertEqual(R.github_write(CONFIG, 'git/refs', {}, 'test-token'), {'sha': 'test'})
                else:
                    with self.assertRaisesRegex(ValueError, 'not acknowledged'):
                        R.github_write(CONFIG, 'git/refs', {}, 'test-token')
                self.assertEqual(opener.open.call_count, 1)

    def test_claim_readback_must_match_acknowledged_commit(self):
        entry, release = self.intent_fixture()
        ref = 'refs/tags/crate-upload-intent/example/0.1.0'
        with patch.object(R, 'get', return_value=None), \
             patch.object(R, 'api', side_effect=[{'tree': {'sha': 'c' * 40}}, {'ref': ref, 'object': {'type': 'commit', 'sha': 'e' * 40}}]), \
             patch.object(R, 'github_write', side_effect=[{'sha': 'd' * 40}, {'ref': ref, 'object': {'sha': 'd' * 40}}]), \
             self.assertRaisesRegex(ValueError, 'readback mismatch'):
            R.reserve_upload(CONFIG, release, entry, REVISION, 'b' * 64, 'github-token')


class BoundaryTests(unittest.TestCase):
    def test_no_registry_or_oidc_credentials_during_preparation(self):
        for name in ['CARGO_REGISTRY_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN', 'GH_TOKEN', 'GH_INTENT_TOKEN']:
            with patch.dict(os.environ, {name: 'secret'}, clear=True), self.assertRaises(ValueError):
                P.preparation_boundary()

    def test_upload_disallows_local_pr_fork_and_other_ref(self):
        expected = {'GITHUB_ACTIONS': 'true', 'GITHUB_REPOSITORY': CONFIG['repository'],
                    'GITHUB_REPOSITORY_ID': '123', 'GITHUB_REF': 'refs/heads/main',
                    'GITHUB_EVENT_NAME': 'workflow_dispatch',
                    'GITHUB_WORKFLOW_REF': 'seraphinastudios/example/.github/workflows/publish-crate.yml@refs/heads/main',
                    'GITHUB_SHA': REVISION, 'GITHUB_WORKFLOW_SHA': REVISION}
        for change in [{}, {'GITHUB_EVENT_NAME': 'pull_request'}, {'GITHUB_EVENT_NAME': 'workflow_run'},
                       {'GITHUB_REPOSITORY_ID': '456'}, {'GITHUB_REF': 'refs/tags/v1.0.0'}]:
            environment = {} if not change else dict(expected, **change)
            with patch.dict(os.environ, environment, clear=True), self.assertRaises(ValueError):
                R.boundary(CONFIG, upload=True)

    def test_manifest_name_allowlist_and_source_identity(self):
        entry, _ = row()
        value = {'schema_version': 1, 'repository': CONFIG['repository'], 'repository_id': '123',
                 'revision': REVISION, 'source_tag': 'v1.0.0', 'crates': [entry]}
        R.validate_manifest(CONFIG, value)
        for key, other in [('repository_id', '456'), ('revision', 'main'), ('source_tag', 'latest')]:
            with self.assertRaises(ValueError):
                R.validate_manifest(CONFIG, dict(value, **{key: other}))
        changed = copy.deepcopy(value); changed['crates'][0]['name'] = 'not-allowed'
        with self.assertRaises(ValueError):
            R.validate_manifest(CONFIG, changed)

    def test_bootstrap_cannot_run_in_actions_or_noninteractive_context(self):
        for environment in ({'GITHUB_ACTIONS': 'true'}, {'CARGO_REGISTRY_TOKEN': 'secret'}, {}):
            with patch.dict(os.environ, environment, clear=True), patch.object(R.sys.stdin, 'isatty', return_value=False):
                with self.assertRaises(ValueError):
                    R.bootstrap(CONFIG, 10, REVISION, 'b' * 64, 'owner', '123')

    def test_bootstrap_existing_name_and_uncertain_upload_fail_closed(self):
        entry, data = row()
        release = {'id': 10}
        for existing, uncertain in [(True, False), (False, True)]:
            with self.subTest(existing=existing), ExitStack() as stack:
                stack.enter_context(patch.dict(os.environ, {}, clear=True))
                stack.enter_context(patch.object(R.sys.stdin, 'isatty', return_value=True))
                stderr = stack.enter_context(patch.object(R.sys, 'stderr', Terminal()))
                stack.enter_context(patch.object(R.subprocess, 'check_output', side_effect=[b'', REVISION]))
                stack.enter_context(patch.object(R, 'api', side_effect=[{'status': 'identical'}, release]))
                stack.enter_context(patch.object(R, 'release_evidence', return_value=(release, {'crates': [entry]}, {'example': data})))
                stack.enter_context(patch.object(R, 'registry_state', return_value='absent'))
                stack.enter_context(patch.object(R, 'get', side_effect=[R.canonical({'user': {'id': 123, 'login': 'owner'}}),
                    b'existing' if existing else None, None]))
                stack.enter_context(patch('builtins.input', return_value='PUBLISH AS owner'))
                stack.enter_context(patch.object(R.getpass, 'getpass', return_value='test-only-token'))
                stack.enter_context(patch.object(R, 'host_write_authority', return_value='test-github-token'))
                stack.enter_context(patch.object(R, 'reserve_upload'))
                stack.enter_context(patch.object(R, 'confirm', side_effect=ValueError('unresolved')))
                put = stack.enter_context(patch.object(R, 'put_once', return_value=R.PutOutcome(False)))
                with self.assertRaisesRegex(ValueError, 'name already exists' if existing else 'unresolved'):
                    R.bootstrap(CONFIG, 10, REVISION, 'b' * 64, 'owner', '123')
                self.assertEqual(put.call_count, 0 if existing else 1)
                output = stderr.getvalue()
                if uncertain:
                    # Ambiguous results keep the intent until authoritative owner confirmation.
                    self.assertIn('no definite result', output)
                    self.assertIn('intent refs/tags/crate-upload-intent/example/0.1.0 stays reserved', output)
                    self.assertIn('Only after authoritative confirmation', output)
                    self.assertNotIn('429', output)
                else:
                    self.assertNotIn('intent', output)

    def test_maintenance_preparation_cannot_claim_coordinates(self):
        entry, data = row()
        def command(argv, **_):
            if argv[0] == 'cargo':
                target = Path(argv[argv.index('--target-dir') + 1]) / 'package'
                target.mkdir(parents=True)
                (target / entry['asset']).write_bytes(data)
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, {'TILEWRIGHT_RELEASE_BRANCH': '1.0.x'}, clear=True), \
             patch.object(P, 'versions', return_value={'example': '0.1.0'}), \
             patch.object(P, 'known') as known, patch.object(P, 'git', side_effect=lambda *args: REVISION if args[0] == 'rev-parse' else ''), \
             patch.object(P.subprocess, 'run', side_effect=command):
            output = Path(temporary)
            self.assertFalse(P.pending(CONFIG))
            manifest = P.prepare(CONFIG, 'v1.0.1', output, archives=output)
            self.assertEqual(manifest['crates'], [])
            self.assertFalse((output / entry['asset']).exists())
            known.assert_not_called()

    def test_version_gate_covers_policy_path_toolchain_and_cargo_config(self):
        for change in ('path', 'toolchain', 'rust-toolchain.toml', '.cargo/config.toml'):
            config = copy.deepcopy(CONFIG)
            changed = ''
            if change == 'path': config['crates'][0]['path'] = 'moved/example'
            elif change == 'toolchain': config['toolchain'] = '1.98.0'
            else: changed = change
            def git(*args):
                if args[0] == 'diff': return changed
                if args[0] == 'cat-file': return ''
                if args[1].endswith(':policy/crate-release.json'): return json.dumps(CONFIG)
                if args[1] == REVISION + ':Cargo.toml': return '[workspace.package]\nversion="0.1.0"\n'
                return '[package]\nversion="0.1.0"\n'
            with self.subTest(change=change), patch.object(P, 'git', side_effect=git), \
                 patch.object(P.subprocess, 'run') as command, \
                 patch.object(P, 'versions', return_value={'example': '0.1.0'}):
                command.return_value.returncode = 0
                with self.assertRaisesRegex(ValueError, 'without reviewed version bump'):
                    P.check_version(config, REVISION)
                with patch.object(P, 'versions', return_value={'example': '0.1.1'}):
                    P.check_version(config, REVISION)

    def test_actions_source_ref_cannot_be_overridden_and_read_token_not_in_build(self):
        with patch.dict(os.environ, {'GITHUB_ACTIONS': 'true', 'GITHUB_REF': 'refs/heads/1.0.x',
                                    'TILEWRIGHT_RELEASE_BRANCH': 'main'}, clear=True), self.assertRaises(ValueError):
            R.registry_source_branch()
        with patch.dict(os.environ, {'GH_READ_TOKEN': 'read-only-secret'}, clear=True):
            self.assertNotIn('GH_READ_TOKEN', P.clean_environment())

    def test_required_checks_exact_revision_name_app_and_latest_success(self):
        check = {'name': 'Example validation', 'head_sha': REVISION, 'app': {'slug': 'github-actions'},
                 'id': 1, 'status': 'completed', 'conclusion': 'success'}
        with patch.object(R, 'api', return_value={'total_count': 1, 'check_runs': [check]}):
            R.verify_checks(CONFIG, REVISION)
        for altered in [dict(check, head_sha='b' * 40), dict(check, name='Other validation'),
                        dict(check, app={'slug': 'other-app'}), dict(check, status='in_progress'),
                        dict(check, conclusion='failure')]:
            with patch.object(R, 'api', return_value={'total_count': 1, 'check_runs': [altered]}), self.assertRaises(ValueError):
                R.verify_checks(CONFIG, REVISION)
        newer_failure = dict(check, id=2, conclusion='failure')
        with patch.object(R, 'api', return_value={'total_count': 2, 'check_runs': [check, newer_failure]}), self.assertRaises(ValueError):
            R.verify_checks(CONFIG, REVISION)

    def test_mutable_release_refused_before_bootstrap_or_oidc_evidence(self):
        release = {'draft': False, 'prerelease': False, 'published_at': '2026-10-11T00:00:00Z',
                   'immutable': False, 'tag_name': 'v1.0.0'}
        for immutable in (False, None):
            with patch.object(R, 'api') as api, self.assertRaisesRegex(ValueError, 'public and immutable'):
                R.release_identity(CONFIG, dict(release, immutable=immutable))
            api.assert_not_called()

    def test_inert_finalizer_requires_complete_prepared_inventory(self):
        entry, data = row()
        value = {'schema_version': 1, 'repository': CONFIG['repository'], 'repository_id': '123',
                 'revision': REVISION, 'source_tag': 'v1.0.0', 'crates': [entry]}
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, {}, clear=True):
            directory = Path(temporary)
            (directory / R.MANIFEST).write_bytes(R.canonical(value))
            (directory / 'provenance.json').write_bytes(R.canonical({'revision': REVISION, 'version': '1.0.0'}))
            (directory / entry['asset']).write_bytes(data)
            sums = ''.join(f'{R.digest(path.read_bytes())}  {path.name}\n' for path in sorted(directory.iterdir()))
            (directory / 'SHA256SUMS').write_text(sums)
            with patch.object(R.subprocess, 'check_output', return_value=REVISION), \
                 patch.object(R, 'api') as settings_api:
                R.finalize(CONFIG, 'v1.0.0', directory)
                settings_api.assert_not_called()
                with patch.dict(os.environ, {'TILEWRIGHT_RELEASE_BRANCH': '1.0.x'}, clear=True), self.assertRaisesRegex(ValueError, 'maintenance release'):
                    R.finalize(CONFIG, 'v1.0.0', directory)
                (directory / 'unchecked').write_text('extra')
                with self.assertRaisesRegex(ValueError, 'inventory coverage'):
                    R.finalize(CONFIG, 'v1.0.0', directory)


class BootstrapPacingTests(unittest.TestCase):
    """crates.io allows a burst of new names, then one per refill period."""

    def run_bootstrap(self, count, burst=R.NEW_CRATE_BURST, outcomes=(), confirm=None, present=(), interrupt=False):
        """`present` holds crates.io created_at values for the first crates, already published."""
        rows = [{'name': f'crate{index}', 'version': '0.1.0', 'sha256': 'c' * 64, 'metadata': {'deps': []}}
                for index in range(count)]
        published = {f'crate{index}': created for index, created in enumerate(present)}
        release, events, outcomes = {'id': 10}, [], list(outcomes)
        def get(url, absent=False, **_):
            if url.endswith('/api/v1/users/owner'):
                return R.canonical({'user': {'id': 123, 'login': 'owner'}})
            if url.endswith('/owners'):
                return R.canonical({'users': [{'kind': 'user', 'id': 123, 'login': 'owner'}]})
            name = url.removeprefix(R.REGISTRY + '/api/v1/crates/')
            if not absent and name in published:
                return R.canonical({'crate': {'name': name, 'created_at': published[name]}})
            self.assertTrue(absent)
            return None
        def sleep(seconds):
            events.append(('sleep', seconds))
            if interrupt:
                raise KeyboardInterrupt
        def prompt(text):
            events.append(('prompt',))
            return 'PUBLISH AS owner'
        def put(data, metadata_value, token):
            events.append(('put', data))
            return outcomes.pop(0) if outcomes else R.PutOutcome(True)
        def confirmed(items):
            events.append(('confirm', items[0]['name']))
            if confirm:
                raise confirm
        stderr, error = Terminal(), None
        with ExitStack() as stack:
            stack.enter_context(patch.dict(os.environ, {}, clear=True))
            stack.enter_context(patch.object(R.sys.stdin, 'isatty', return_value=True))
            stack.enter_context(patch.object(R.sys, 'stderr', stderr))
            stack.enter_context(patch.object(R.subprocess, 'check_output',
                side_effect=lambda argv, **_: b'' if 'status' in argv else REVISION))
            stack.enter_context(patch.object(R, 'api', side_effect=lambda config, suffix:
                {'status': 'identical'} if suffix.startswith('compare/') else release))
            stack.enter_context(patch.object(R, 'release_evidence', return_value=(
                release, {'crates': rows}, {item['name']: item['name'] for item in rows})))
            stack.enter_context(patch.object(R, 'registry_state', side_effect=lambda item:
                events.append(('check', item['name'])) or ('present' if item['name'] in published else 'absent')))
            stack.enter_context(patch.object(R, 'get', side_effect=get))
            stack.enter_context(patch('builtins.input', side_effect=prompt))
            stack.enter_context(patch.object(R.getpass, 'getpass', return_value='test-only-token'))
            stack.enter_context(patch.object(R, 'host_write_authority', return_value='test-github-token'))
            stack.enter_context(patch.object(R, 'reserve_upload',
                side_effect=lambda config, value, item, *args: events.append(('reserve', item['name']))))
            stack.enter_context(patch.object(R, 'put_once', side_effect=put))
            stack.enter_context(patch.object(R, 'confirm', side_effect=confirmed))
            stack.enter_context(patch.object(R.time, 'sleep', side_effect=sleep))
            try:
                R.bootstrap(CONFIG, 10, REVISION, 'b' * 64, 'owner', '123', burst=burst)
            except (ValueError, KeyboardInterrupt) as raised:
                error = raised
        return events, stderr.getvalue(), error

    def segments(self, events):
        """What happens between the previous upload (or the confirmation) and each reservation."""
        result, segment = [], []
        for event in events:
            if event[0] == 'reserve':
                result.append((event[1], segment))
            if event[0] in ('reserve', 'prompt', 'put', 'confirm'):
                segment = []
            else:
                segment.append(event)
        return result

    def test_first_burst_then_one_full_wait_before_each_further_intent(self):
        for count, burst in [(8, 5), (8, 0), (5, 5), (3, 2), (2, 5)]:
            with self.subTest(count=count, burst=burst):
                events, output, error = self.run_bootstrap(count, burst)
                self.assertIsNone(error)
                names = [f'crate{index}' for index in range(count)]
                self.assertEqual([event[1] for event in events if event[0] == 'put'], names)
                segments = self.segments(events)
                self.assertEqual([name for name, _ in segments], names)
                for position, (name, segment) in enumerate(segments, 1):
                    sleeps = [event for event in segment if event[0] == 'sleep']
                    # The wait finishes before the absence re-check, which precedes the intent.
                    self.assertEqual(segment, sleeps + [('check', name)])
                    if position > burst:
                        self.assertEqual(sum(value for _, value in sleeps),
                                         R.NEW_CRATE_RATE_SECONDS + R.NEW_CRATE_MARGIN_SECONDS)
                        self.assertTrue(all(0 < value <= R.PROGRESS_SECONDS for _, value in sleeps))
                    else:
                        self.assertEqual(sleeps, [])
                paced = max(0, count - burst)
                waits = sum(1 for event in events if event[0] == 'sleep')
                self.assertEqual(waits, paced * 11)
                self.assertEqual(output.count(' until reserving crate'), waits)
                if paced:
                    self.assertIn(f'rate limit: {count - paced} at once, then {paced} after a 610 s wait each', output)
                    self.assertIn(f'10m10s until reserving crate{burst}@0.1.0 ({burst + 1} of {count})', output)
                else:
                    self.assertNotIn('wait', output)

    def created(self, minutes):
        moment = R.datetime.datetime.now(R.datetime.timezone.utc) - R.datetime.timedelta(minutes=minutes)
        return moment.isoformat().replace('+00:00', 'Z')  # crates.io's created_at form

    def paced_names(self, events):
        return [name for name, segment in self.segments(events) if any(event[0] == 'sleep' for event in segment)]

    def test_resumed_run_lowers_burst_for_names_this_release_just_created(self):
        cases = [  # (created_at of the already published crates, effective burst)
            ([self.created(5)] * 5, 0),                                       # all 5 tokens just spent
            ([self.created(12), self.created(40), self.created(120)], 3),     # 2 inside the window
            ([self.created(51)] * 5, 5),                                      # all outside the window
            (['not a time', None, '2026-10-26T12:00:00'], 2),                 # unreadable counts as recent
            ([self.created(-3)], 4),                                          # clock skew counts as recent
        ]
        for present, burst in cases:
            with self.subTest(present=present):
                events, output, error = self.run_bootstrap(8, present=present)
                self.assertIsNone(error)
                remaining = [f'crate{index}' for index in range(len(present), 8)]
                self.assertEqual([event[1] for event in events if event[0] == 'put'], remaining)
                self.assertEqual(self.paced_names(events), remaining[burst:])
                lowered = [line for line in output.splitlines() if 'lowering --burst' in line]
                self.assertEqual(lowered, [] if burst == 5 else [
                    f'{5 - burst} of these crates were first published within the last 50 minutes;'
                    f' lowering --burst from 5 to {burst}.'])

    def test_resumed_run_reports_how_far_it_lowered_the_burst(self):
        _, output, _ = self.run_bootstrap(8, present=[self.created(5)] * 2, burst=4)
        self.assertIn('2 of these crates were first published within the last 50 minutes;'
                      ' lowering --burst from 4 to 2.', output)
        _, output, _ = self.run_bootstrap(8, present=[self.created(5)] * 5, burst=3)
        self.assertIn('lowering --burst from 3 to 0.', output)

    def test_every_abort_after_an_upload_advises_burst_zero(self):
        advice = 'If you re-run within the next hour, pass --burst 0.'
        # Ctrl-C during the wait after the burst.
        events, output, error = self.run_bootstrap(6, interrupt=True)
        self.assertIsInstance(error, KeyboardInterrupt)
        self.assertNotIn(('reserve', 'crate5'), events)
        self.assertIn(f'This run sent 5 new crate upload(s) to crates.io. {advice}', output)
        # An unresolved upload, and a refused one.
        for outcomes, confirm in [((R.PutOutcome(False),), ValueError('registry readiness unresolved')),
                                  ((R.PutOutcome(True), R.PutOutcome(False, True, None)), None)]:
            with self.subTest(outcomes=outcomes):
                _, output, error = self.run_bootstrap(3, outcomes=outcomes, confirm=confirm)
                self.assertIsInstance(error, ValueError)
                self.assertIn(advice, output)
        # Nothing sent yet: an interrupted first wait needs no advice.
        events, output, error = self.run_bootstrap(2, burst=0, interrupt=True)
        self.assertIsInstance(error, KeyboardInterrupt)
        self.assertNotIn('put', [event[0] for event in events])
        self.assertNotIn(advice, output)
        # A complete run warns about other bootstraps with the same account.
        _, output, error = self.run_bootstrap(2)
        self.assertIsNone(error)
        self.assertNotIn(advice, output)
        self.assertIn('crates.io counts these 2 new names for the next hour; pass --burst 0', output)

    def test_out_of_range_burst_is_refused_before_any_read(self):
        for burst in (-1, 6, True, '3', 2.0):
            with self.subTest(burst=burst), patch.dict(os.environ, {}, clear=True), \
                 patch.object(R.sys.stdin, 'isatty', return_value=True), patch.object(R.sys, 'stderr', Terminal()), \
                 patch.object(R.subprocess, 'check_output') as git, patch.object(R, 'api') as api, \
                 self.assertRaisesRegex(ValueError, 'burst out of range'):
                R.bootstrap(CONFIG, 10, REVISION, 'b' * 64, 'owner', '123', burst=burst)
            git.assert_not_called(); api.assert_not_called()

    def test_command_line_burst_bounds_and_default(self):
        for value, expected in [('0', 0), ('3', 3), ('5', 5)]:
            self.assertEqual(R.new_crate_burst(value), expected)
        for value in ('-1', '6', '10', '1.5', 'five', '', ' 3', '٣'):
            with self.subTest(value=value), self.assertRaises(R.argparse.ArgumentTypeError):
                R.new_crate_burst(value)
        for extra, expected in [([], R.NEW_CRATE_BURST), (['--burst', '2'], 2), (['--burst', '0'], 0)]:
            with self.subTest(extra=extra), patch.object(R, 'policy', return_value=CONFIG), \
                 patch.object(R, 'bootstrap') as bootstrap, redirect_stdout(io.StringIO()), \
                 patch.object(R.sys, 'argv', ['crate_release.py', 'bootstrap', *extra]):
                R.main()
                self.assertEqual(bootstrap.call_args.args[-1], expected)
        for value in ('6', '-1'):
            with self.subTest(value=value), patch.object(R, 'bootstrap') as bootstrap, \
                 patch.object(R.sys, 'stderr', io.StringIO()) as stderr, self.assertRaises(SystemExit), \
                 patch.object(R.sys, 'argv', ['crate_release.py', 'bootstrap', '--burst', value]):
                R.main()
            bootstrap.assert_not_called()
            self.assertIn('from 0 to 5', stderr.getvalue())

    def test_rate_limit_refusal_stops_with_owner_recovery_and_keeps_intent(self):
        for retry_after, when in [('2026-10-26 12:34:56 UTC', 'after 2026-10-26 12:34:56 UTC'),
                                  (None, 'gave no retry time; wait at least 10 minutes')]:
            with self.subTest(retry_after=retry_after), patch.object(R, 'github_write') as writer:
                events, output, error = self.run_bootstrap(
                    3, outcomes=[R.PutOutcome(True), R.PutOutcome(False, True, retry_after)])
                self.assertRegex(str(error), 'rate limit refused the upload; intent retained')
                # No read-only confirmation wait, no later intent, and no intent removal.
                self.assertEqual([event for event in events if event[0] in ('reserve', 'put', 'confirm')],
                                 [('reserve', 'crate0'), ('put', 'crate0'), ('confirm', 'crate0'),
                                  ('reserve', 'crate1'), ('put', 'crate1')])
                writer.assert_not_called()
                for text in ['crates.io refused crate1@0.1.0 with HTTP 429 (rate limit) before accepting it.', when,
                             'intent refs/tags/crate-upload-intent/crate1/0.1.0 is still reserved',
                             'https://crates.io/api/v1/crates/crate1 returns 404',
                             'gh api --method DELETE repos/seraphinastudios/example/git/refs/tags/crate-upload-intent/crate1/0.1.0',
                             '3. Re-run this bootstrap with --burst 0']:
                    self.assertIn(text, output)

    def test_acknowledged_but_unconfirmed_upload_keeps_its_intent(self):
        events, output, error = self.run_bootstrap(1, confirm=ValueError('registry readiness unresolved'))
        self.assertRegex(str(error), 'readiness unresolved')
        self.assertIn('crates.io acknowledged crate0@0.1.0', output)
        self.assertIn('a published coordinate keeps its reservation permanently', output)
        self.assertNotIn('DELETE', output)


class PutOnceTests(unittest.TestCase):
    def put(self, outcome):
        opener = MagicMock()
        if isinstance(outcome, BaseException):
            opener.open.side_effect = outcome
        else:
            opener.open.return_value = outcome
        with patch.object(R.urllib.request, 'build_opener', return_value=opener):
            result = R.put_once(b'archive', {'name': 'example'}, 'test-token')
        self.assertEqual(opener.open.call_count, 1)
        return result

    def refusal(self, code, retry_after=None, body=b''):
        headers = email.message.Message()
        if retry_after is not None:
            headers['Retry-After'] = retry_after
        return urllib.error.HTTPError(R.REGISTRY + '/api/v1/crates/new', code, 'refused', headers, io.BytesIO(body))

    def test_rate_limit_is_definite_and_reports_only_a_parsed_retry_time(self):
        # The body crates.io sends with its Retry-After header (src/util/errors/json.rs).
        detail = R.canonical({'errors': [{'detail': 'You have published too many new crates in a short period'
            ' of time. Please try again after Tue, 27 Oct 2026 01:02:03 GMT and see'
            ' https://crates.io/docs/rate-limits for more details.'}]})
        for retry_after, body, expected in [
                ('Mon, 26 Oct 2026 12:34:56 GMT', detail, '2026-10-26 12:34:56 UTC'),
                (None, detail, '2026-10-27 01:02:03 UTC'),
                ('\x1b]8;;https://attacker.invalid\x07', detail, '2026-10-27 01:02:03 UTC'),
                ('Mon, 26 Oct 2026 12:34:56 GMT\x1b[2J', b'try again after \x1b[2J', None),
                ('Mon, 31 Feb 2026 12:34:56 GMT', b'', None),
                (None, b'not json', None)]:
            with self.subTest(retry_after=retry_after, body=body):
                outcome = self.put(self.refusal(429, retry_after, body))
                self.assertEqual(outcome, R.PutOutcome(False, True, expected))
                self.assertFalse(outcome)
        self.assertRegex(self.put(self.refusal(429, '120')).retry_after,
                         r'[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2} UTC')

    def test_unreadable_rate_limit_details_are_still_a_refusal(self):
        error = self.refusal(429, 'Mon, 26 Oct 2026 12:34:56 GMT')
        with patch.object(error, 'read', side_effect=ConnectionResetError('reset')):
            self.assertEqual(self.put(error), R.PutOutcome(False, True, None))

    def test_other_failures_stay_ambiguous(self):
        for failure in (self.refusal(500, 'Mon, 26 Oct 2026 12:34:56 GMT'), self.refusal(403),
                        TimeoutError('lost'), urllib.error.URLError('reset'), ValueError('redirect refused')):
            with self.subTest(failure=failure):
                self.assertEqual(self.put(failure), R.PutOutcome(False))

    def test_only_acknowledged_success_is_accepted(self):
        for body, accepted in [(b'{"warnings":{}}', True), (b'{"errors":[{"detail":"refused"}]}', False)]:
            response = MagicMock()
            response.__enter__.return_value = response
            response.status = 200
            response.read.return_value = body
            response.geturl.return_value = R.REGISTRY + '/api/v1/crates/new'
            outcome = self.put(response)
            self.assertEqual(outcome, R.PutOutcome(accepted))
            self.assertEqual(bool(outcome), accepted)


class CatalogTests(unittest.TestCase):
    def releases(self):
        return [{'id': 7, 'tag_name': 'v1.0.0', 'draft': False, 'prerelease': False,
                 'immutable': True, 'assets': [{'name': R.MANIFEST}]}]

    def run_catalog(self, manifest):
        with patch.object(R, 'api', return_value=self.releases()), \
                patch.object(R, 'asset_bytes', return_value=json.dumps(manifest).encode()):
            return R.catalog(CONFIG)

    def legacy(self, **changes):
        manifest = {'schema_version': 1, 'repository': 'previous-owner/example', 'repository_id': '123',
                    'revision': REVISION, 'source_tag': 'v1.0.0',
                    'crates': [{'name': 'previous-example', 'version': '0.1.0'}]}
        manifest.update(changes)
        return manifest

    def test_earlier_identity_of_the_same_repository_is_skipped(self):
        self.assertEqual(self.run_catalog(self.legacy()), {})
        self.assertEqual(self.run_catalog(self.legacy(crates=[])), {})

    def test_foreign_or_overlapping_or_malformed_legacy_manifests_fail(self):
        for manifest in (self.legacy(repository_id='999'),
                         self.legacy(crates=[{'name': 'example', 'version': '0.1.0'}]),
                         self.legacy(source_tag='v9.9.9'),
                         self.legacy(revision='not-a-sha'),
                         dict(self.legacy(), extra=True)):
            with self.subTest(manifest=manifest), self.assertRaises(ValueError):
                self.run_catalog(manifest)

    def test_historical_names_are_managed_too(self):
        config = dict(CONFIG, historical=[{'name': 'previous-example'}])
        with patch.object(R, 'api', return_value=self.releases()), \
                patch.object(R, 'asset_bytes', return_value=json.dumps(self.legacy()).encode()), \
                self.assertRaises(ValueError):
            R.catalog(config)


class StablePackageTests(unittest.TestCase):
    def old_fixture(self):
        old_files = files()
        old_files['.cargo_vcs_info.json'] = R.canonical({'git': {'sha1': 'b' * 40}, 'path_in_vcs': 'crates/example'})
        data = make_archive(old_files)
        entry = P.package_row(CONFIG['crates'][0], '0.1.0', data)
        return entry, data, ({'id': 1}, {'revision': 'b' * 40}, entry, 'c' * 64)

    def commands(self, data, observed):
        def command(argv, **_):
            if argv[0] == 'cargo':
                observed.append(argv)
                target = Path(argv[argv.index('--target-dir') + 1]) / 'package'
                target.mkdir(parents=True)
                for name, value in data.items():
                    if name in argv:
                        (target / (name + '-0.1.0.crate')).write_bytes(value)
            else:
                observed.append({p.name: p.read_bytes() for p in Path(argv[-1]).glob('*.crate')})
        return command

    def named_archive(self, name, publish=True):
        contents = {key: value.replace(b'example', name.encode()) for key, value in files().items()}
        if not publish:
            contents['Cargo.toml'] = contents['Cargo.toml'].replace(b'publish=["crates-io"]', b'publish=false')
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode='w:gz') as stream:
            for key, data in contents.items():
                member = tarfile.TarInfo(f'{name}-0.1.0/{key}'); member.size = len(data)
                stream.addfile(member, io.BytesIO(data))
        return output.getvalue()

    def test_mixed_graph_is_partitioned_and_cli_stays_source_only(self):
        old, old_bytes, lineage = self.old_fixture()
        config = copy.deepcopy(CONFIG)
        config['crates'].append({'name': 'fresh', 'path': 'crates/fresh'})
        extras = {'cli': {'name': 'cli', 'version': '0.1.0', 'path': 'crates/cli'}}
        data = {'example': make_archive(), 'fresh': self.named_archive('fresh'),
                'cli': self.named_archive('cli', publish=False)}
        calls = []
        with tempfile.TemporaryDirectory() as temporary, patch.object(R, 'asset_bytes', return_value=old_bytes), \
             patch.object(R, 'registry_state', return_value='present'), \
             patch.object(P.subprocess, 'run', side_effect=self.commands(data, calls)):
            rows, staged = P.stage_packages(config, {'example': '0.1.0', 'fresh': '0.1.0'},
                {('example', '0.1.0'): lineage}, REVISION, Path(temporary), source_packages=extras)
            self.assertEqual(set(rows), {'example', 'fresh'})
            self.assertEqual(staged['example'], old_bytes)
            self.assertEqual(set(calls[-1]), {'example-0.1.0.crate', 'fresh-0.1.0.crate', 'cli-0.1.0.crate'})
            self.assertEqual([arg for arg in calls[0] if arg in data], ['example'])
            self.assertEqual([arg for arg in calls[2] if arg in data], ['fresh', 'cli'])
            self.assertEqual(len(calls), 5)

    def test_maintenance_retains_cli_without_looking_up_main_ownership(self):
        calls = []; data = {'example': make_archive(), 'cli': self.named_archive('cli', publish=False)}
        with tempfile.TemporaryDirectory() as temporary, ExitStack() as stack:
            stack.enter_context(patch.dict(os.environ, {'TILEWRIGHT_RELEASE_BRANCH': '1.0.x'}, clear=True))
            stack.enter_context(patch.object(P, 'versions', return_value={'example': '0.1.0'}))
            stack.enter_context(patch.object(P, 'source_only_packages', return_value={
                'cli': {'name': 'cli', 'version': '0.1.0', 'path': 'crates/cli'}}))
            known = stack.enter_context(patch.object(P, 'known'))
            registry = stack.enter_context(patch.object(R, 'registry_state'))
            stack.enter_context(patch.object(P, 'git', side_effect=lambda *a: REVISION if a[0] == 'rev-parse' else ''))
            stack.enter_context(patch.object(P.subprocess, 'run', side_effect=self.commands(data, calls)))
            output = Path(temporary)
            manifest = P.prepare(CONFIG, 'v1.0.1', output, archives=output, source_packages=['cli'])
            self.assertEqual(manifest['crates'], [])
            self.assertEqual({p.name for p in output.glob('*.crate')}, {'cli-0.1.0.crate'})
            self.assertEqual(set(calls[-1]), {'example-0.1.0.crate', 'cli-0.1.0.crate'})
            known.assert_not_called(); registry.assert_not_called()

    def test_old_source_only_commit_stages_original_and_emits_no_new_lineage(self):
        original, old_bytes, lineage = self.old_fixture()
        candidate = make_archive()
        calls = []
        with tempfile.TemporaryDirectory() as temporary, ExitStack() as stack:
            stack.enter_context(patch.dict(os.environ, {}, clear=True))
            stack.enter_context(patch.object(P, 'versions', return_value={'example': '0.1.0'}))
            stack.enter_context(patch.object(P, 'known', return_value={('example', '0.1.0'): lineage}))
            stack.enter_context(patch.object(P, 'git', side_effect=lambda *a: REVISION if a[0] == 'rev-parse' else ''))
            stack.enter_context(patch.object(R, 'asset_bytes', return_value=old_bytes))
            stack.enter_context(patch.object(R, 'registry_state', return_value='present'))
            stack.enter_context(patch.object(P.subprocess, 'run', side_effect=self.commands({'example': candidate}, calls)))
            output = Path(temporary)
            result = P.prepare(CONFIG, 'v1.0.0', output, archives=output)
            self.assertEqual(result['crates'], [])
            self.assertEqual(list(output.glob('*.crate')), [])
            self.assertEqual(calls[-1], {original['asset']: old_bytes})
            self.assertEqual(len(calls), 3)  # exactly two individual runs, then full graph proof
            self.assertNotEqual(calls[0][calls[0].index('--target-dir') + 1],
                                calls[1][calls[1].index('--target-dir') + 1])

    def test_old_content_inventory_and_current_revision_cannot_change(self):
        _, old_bytes, lineage = self.old_fixture()
        mutations = {'code': ('src/lib.rs', b'changed'), 'added-file': ('new.txt', b'new'),
                     'lock': ('Cargo.lock', files()['Cargo.lock'] + b'# changed\n'),
                     'metadata': ('Cargo.toml', files()['Cargo.toml'].replace(b'license="MIT"', b'license="Apache-2.0"')),
                     'revision': ('.cargo_vcs_info.json', b'{}')}
        for label, (key, value) in mutations.items():
            changed = files(); changed[key] = value
            with self.subTest(label=label), tempfile.TemporaryDirectory() as temporary, \
                 patch.object(R, 'asset_bytes', return_value=old_bytes), \
                 patch.object(R, 'registry_state', return_value='present'), \
                 patch.object(P.subprocess, 'run', side_effect=self.commands({'example': make_archive(changed)}, [])):
                with self.assertRaises(ValueError):
                    P.stage_packages(CONFIG, {'example': '0.1.0'}, {('example', '0.1.0'): lineage}, REVISION, Path(temporary))

    def test_unready_or_conflicting_original_blocks_before_any_cargo(self):
        _, old_bytes, lineage = self.old_fixture()
        for state in ('absent', 'indeterminate', ValueError('checksum/yank conflict')):
            with self.subTest(state=state), tempfile.TemporaryDirectory() as temporary, \
                 patch.object(R, 'asset_bytes', return_value=old_bytes), \
                 patch.object(R, 'registry_state', side_effect=state if isinstance(state, Exception) else None,
                              return_value=state), patch.object(P.subprocess, 'run') as run:
                with self.assertRaises(ValueError):
                    P.stage_packages(CONFIG, {'example': '0.1.0'}, {('example', '0.1.0'): lineage}, REVISION, Path(temporary))
                run.assert_not_called()

    def test_original_digest_inventory_and_provenance_must_match(self):
        entry, old_bytes, lineage = self.old_fixture()
        for field, value in [('sha256', '0' * 64), ('inventory', []), ('path', 'other')]:
            changed = copy.deepcopy(lineage); changed[2][field] = value
            with self.subTest(field=field), patch.object(R, 'asset_bytes', return_value=old_bytes), \
                 patch.object(R, 'registry_state') as registry:
                with self.assertRaises(ValueError):
                    P.original_package(CONFIG, CONFIG['crates'][0], '0.1.0', changed)
                registry.assert_not_called()
        historical = dict(entry, source_tag='v0.1.0', revision='b' * 40)
        with patch.object(R, 'api'), patch.object(R, 'asset_bytes', return_value=old_bytes), \
             patch.object(R, 'registry_state', return_value='present'):
            self.assertEqual(P.original_package(CONFIG, CONFIG['crates'][0], '0.1.0', historical), (entry, old_bytes))
            historical['revision'] = REVISION
            with self.assertRaisesRegex(ValueError, 'source provenance'):
                P.original_package(CONFIG, CONFIG['crates'][0], '0.1.0', historical)

    def test_preexisting_all_selected_outputs_are_not_adopted(self):
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, {}, clear=True), \
             patch.object(P, 'versions', return_value={'example': '0.1.0'}), \
             patch.object(P, 'known', return_value={}), patch.object(P, 'git', return_value=REVISION), \
             patch.object(P, 'stage_packages') as stage:
            output = Path(temporary); (output / 'example-0.1.0.crate').write_bytes(b'old overlay')
            with self.assertRaisesRegex(ValueError, 'preexisting package output collision'):
                P.prepare(CONFIG, 'v1.0.0', output, dry_run=True)
            stage.assert_not_called()

    def test_reproduction_and_verifier_input_tampering_rejected(self):
        _, data = row()
        for tamper in ('repeat', 'verifier'):
            calls = []
            ordinary = self.commands({'example': data}, calls)
            def command(argv, **kwargs):
                ordinary(argv, **kwargs)
                if tamper == 'repeat' and argv[0] == 'cargo' and len(calls) == 2:
                    (Path(argv[argv.index('--target-dir') + 1]) / 'package/example-0.1.0.crate').write_bytes(b'changed')
                if tamper == 'verifier' and argv[0] == 'bash':
                    (Path(argv[-1]) / 'example-0.1.0.crate').write_bytes(b'changed')
            with self.subTest(tamper=tamper), tempfile.TemporaryDirectory() as temporary, \
                 patch.object(P.subprocess, 'run', side_effect=command):
                with self.assertRaisesRegex(ValueError, 'reproduction mismatch|staged archive changed'):
                    P.stage_packages(CONFIG, {'example': '0.1.0'}, {}, REVISION, Path(temporary))

    def test_source_only_selection_must_be_explicit_nonpublishable_workspace_member(self):
        metadata = {'workspace_members': ['cli-id'], 'packages': [
            {'id': 'cli-id', 'name': 'example-cli', 'version': '0.1.0', 'publish': [],
             'manifest_path': str(P.ROOT / 'cli/Cargo.toml')} ]}
        with patch.object(P.subprocess, 'check_output', return_value=R.canonical(metadata)):
            self.assertEqual(P.source_only_packages(CONFIG, ['example-cli'])['example-cli']['path'], 'cli')
            for names in (['example'], ['example-cli', 'example-cli'], ['unknown'], ['--workspace']):
                with self.subTest(names=names), self.assertRaises(ValueError):
                    P.source_only_packages(CONFIG, names)
            metadata['packages'][0]['publish'] = ['crates-io']
        with patch.object(P.subprocess, 'check_output', return_value=R.canonical(metadata)):
            with self.assertRaisesRegex(ValueError, 'publish=false'):
                P.source_only_packages(CONFIG, ['example-cli'])


class PackageVerifierTests(unittest.TestCase):
    def test_unreadable_or_empty_policy_fails_before_verifying_nothing(self):
        script = Path(__file__).resolve().parent / 'verify-packages.sh'
        for policy in ('{"crates": []}', '{}', 'not json'):
            with self.subTest(policy=policy), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                (root / 'policy').mkdir()
                (root / 'policy/crate-release.json').write_text(policy)
                (root / 'bin').mkdir()
                cargo = root / 'bin/cargo'
                cargo.write_text('#!/usr/bin/env bash\n'
                                 'if [ "$1" = metadata ]; then echo \'{"packages":[{"version":"0.1.0"}]}\'; '
                                 'else echo "$*" >>"$0.log"; fi\n')
                cargo.chmod(0o755)
                subprocess.run(['git', 'init', '-q', str(root)], check=True)
                result = subprocess.run(['bash', str(script), str(root)], cwd=root, capture_output=True,
                                        env={**os.environ, 'PATH': f'{root / "bin"}:{os.environ["PATH"]}'})
                self.assertNotEqual(result.returncode, 0, result.stderr)
                self.assertFalse((root / 'bin/cargo.log').exists())


if __name__ == '__main__':
    unittest.main()
