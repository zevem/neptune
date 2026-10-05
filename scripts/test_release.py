import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import release
import correct_release_notes as correction


class ReleaseTests(unittest.TestCase):
    def test_notes_correction_preserves_pending_acceptance_and_channel_warning(self):
        before = "### What's New\n\n- Existing feature.\n\n" + correction.OLD_STATUS + "\n" + correction.OLD_CHANNEL + "\n"
        after = correction.corrected_notes(before)
        self.assertIn("- Existing feature.", after)
        self.assertIn("Full native acceptance on every platform remains pending", after)
        self.assertIn("Automatic\n  updates never downgrade", after)
        self.assertNotIn(correction.OLD_STATUS, after)
        self.assertNotIn(correction.OLD_CHANNEL, after)
        for invalid in (after, before + correction.OLD_STATUS, before.replace(correction.OLD_CHANNEL, ""), None):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                correction.corrected_notes(invalid)

    def test_notes_repair_cannot_change_installers_source_version_or_extra_fields(self):
        before = {
            "schema": 1, "repository": release.REPO, "version": "0.1.0-rc.4", "tag": "v0.1.0-rc.4", "commit": "a" * 40,
            "notes": correction.OLD_STATUS + "\n" + correction.OLD_CHANNEL,
            "assets": [{"platform": "linux-x64-appimage", "name": "original.AppImage", "size": 123, "sha256": "b" * 64}],
        }
        after = dict(before, notes=correction.corrected_notes(before["notes"]))
        correction.only_notes_changed(before, after)
        changes = [{"version": "0.1.0-rc.5"}, {"tag": "v0.1.0-rc.5"}, {"commit": "c" * 40},
                   {"assets": []}, {"repository": "other/repo"}, {"extra": "field"}, {"notes": after["notes"] + "New feature"}]
        for change in changes:
            with self.subTest(change=change), self.assertRaisesRegex(ValueError, "Only the two"):
                correction.only_notes_changed(before, dict(after, **change))

    def test_metadata_repair_rejects_other_versions_drafts_and_immutable_releases(self):
        with patch.object(release, 'release_for_tag') as lookup:
            with self.assertRaises(ValueError): correction.snapshot('v0.1.0-rc.5')
            lookup.assert_not_called()
        for flags in ({"draft": True, "prerelease": True, "immutable": False},
                      {"draft": False, "prerelease": False, "immutable": False},
                      {"draft": False, "prerelease": True, "immutable": True}):
            with self.subTest(flags=flags), patch.object(release, 'release_for_tag', return_value=flags):
                with self.assertRaisesRegex(ValueError, "editable, published prerelease"):
                    correction.snapshot('v0.1.0-rc.4')

    def test_metadata_repair_signs_exact_new_notes_with_original_installer_checksums(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'packaging').mkdir()
            key = root / 'private.pem'
            subprocess.run(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', str(key)], check=True)
            public = subprocess.check_output(['openssl', 'pkey', '-in', str(key), '-pubout', '-outform', 'DER'])
            (root / 'packaging/update-public-key.hex').write_text(public[-32:].hex())
            directory = root / 'correction'
            before, after = directory / 'before', directory / 'after'
            before.mkdir(parents=True)
            after.mkdir()
            original = {'assets': [{'name': 'existing-installer.exe', 'sha256': 'b' * 64}],
                        'notes': correction.OLD_STATUS + '\n' + correction.OLD_CHANNEL}
            updated = dict(original, notes=correction.corrected_notes(original['notes']))
            (after / 'update-manifest.json').write_text(json.dumps(updated))
            baseline = {'release': {'tag_name': 'v0.1.0-rc.4'}}
            with patch.object(release, 'ROOT', root), patch.object(correction, 'validate_preparation', return_value=(baseline, original)), \
                 patch.object(correction, 'snapshot', return_value=baseline), patch.dict(os.environ, {'UPDATE_SIGNING_KEY': key.read_text()}):
                correction.seal(directory)
                correction.verify_signature(after)
                sums = correction.checksums(after)
                self.assertEqual(sums['existing-installer.exe'], 'b' * 64)
                self.assertEqual(sums['update-manifest.json'], release.sha256(after / 'update-manifest.json'))
                self.assertEqual(sums['update-manifest.sig'], release.sha256(after / 'update-manifest.sig'))
                # A text edit after signing is rejected by the actual Ed25519 verifier.
                (after / 'update-manifest.json').write_text(json.dumps(dict(updated, notes='tampered')))
                with self.assertRaises(subprocess.CalledProcessError): correction.verify_signature(after)

    def test_repair_upload_and_failure_recovery_can_only_touch_three_metadata_assets(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            after, before = directory / 'after', directory / 'before'
            after.mkdir()
            before.mkdir()
            for name in correction.METADATA:
                (after / name).write_bytes(b'corrected')
                (before / name).write_bytes(b'original')
            baseline = {'release': {'tag_name': 'v0.1.0-rc.4'}, 'tag': {'sha': 'a' * 40},
                        'assets': {'installer.exe': {'id': 1, 'digest': 'sha256:' + 'b' * 64}}}
            baseline['assets'].update({name: {'id': i + 2, 'digest': 'sha256:' + release.sha256(before / name)}
                                       for i, name in enumerate(correction.METADATA)})
            remote = {**baseline['release'], 'assets': [{'name': name, **asset} for name, asset in baseline['assets'].items()]}
            original = {'assets': [{'name': 'installer.exe', 'sha256': 'b' * 64}]}
            sums = {'installer.exe': 'b' * 64, **{name: release.sha256(after / name) for name in ('update-manifest.json', 'update-manifest.sig')}}
            calls = []
            def upload(command, **kwargs):
                calls.append(command)
                if len(calls) == 1: raise subprocess.CalledProcessError(1, command)
                return subprocess.CompletedProcess(command, 0)
            with patch.object(correction, 'validate_preparation', return_value=(baseline, original)), \
                 patch.object(correction, 'verify_signature'), patch.object(correction, 'checksums', return_value=sums), \
                 patch.object(correction, 'snapshot', return_value=baseline), patch.object(release, 'release_for_tag', return_value=remote), \
                 patch.object(correction, 'gh_json', return_value={'object': baseline['tag']}), \
                 patch.object(correction.subprocess, 'run', side_effect=upload):
                with self.assertRaises(subprocess.CalledProcessError): correction.publish(directory)
            self.assertEqual(len(calls), 2)
            for command, folder in zip(calls, (after, before)):
                self.assertEqual(command[:8], ['gh', 'release', 'upload', 'v0.1.0-rc.4', '--repo', release.REPO, '--clobber', str(folder / 'SHA256SUMS')])
                self.assertEqual(set(command[7:]), {str(folder / name) for name in correction.METADATA})
            (after / 'installer.exe').write_bytes(b'unexpected')
            with patch.object(correction, 'validate_preparation', return_value=(baseline, original)), \
                 patch.object(correction.subprocess, 'run') as run:
                with self.assertRaisesRegex(ValueError, "Only three metadata"):
                    correction.publish(directory)
                run.assert_not_called()
            remote['assets'][-1]['digest'] = 'sha256:' + 'c' * 64
            with patch.object(release, 'release_for_tag', return_value=remote), \
                 patch.object(correction, 'gh_json', return_value={'object': baseline['tag']}), patch.object(correction.subprocess, 'run') as run:
                with self.assertRaisesRegex(ValueError, "changed independently"):
                    correction.rollback(directory, baseline)
                run.assert_not_called()

    def test_linux_library_notice_lookup_handles_merged_usr_aliases(self):
        spec = importlib.util.spec_from_file_location('package_linux', Path(__file__).with_name('package-linux.py'))
        packaging = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(packaging)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            registered = root / 'usr/lib/libXinerama.so.1.0.0'
            registered.parent.mkdir(parents=True)
            registered.touch()
            (root / 'lib').symlink_to(root / 'usr/lib')
            alias = root / 'lib/libXinerama.so.1'
            alias.symlink_to('libXinerama.so.1.0.0')

            def lookup(command, **kwargs):
                status = 0 if command[-1] == str(registered) else 1
                return subprocess.CompletedProcess(command, status, f'libxinerama1:amd64: {registered}\n' if status == 0 else '', '')

            with patch.object(packaging.subprocess, 'run', side_effect=lookup):
                self.assertEqual(packaging.library_package(alias), 'libxinerama1')
            with patch.object(packaging.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1, '', '')):
                with self.assertRaisesRegex(ValueError, 'No installed package owns'):
                    packaging.library_package(alias)

    def test_semver_classification_and_names(self):
        for version, pre in [('0.1.0', False), ('0.2.0-beta.1', True), ('0.2.0-rc.1+build.5', True), ('0.2.0+build.5', False)]:
            self.assertEqual(release.version_info(version)[1], pre)
            self.assertEqual(set(release.artifact_names(version).values()), {f'Neptune-{version}-{suffix}' for suffix in ['macos-arm64.dmg', 'macos-x64.dmg', 'windows-x64.exe', 'linux-x64.AppImage', 'linux-x64.deb']})
        for version in ['v0.1.0', '01.2.3', '0.1', '0.1.0-', '0.1.0-beta.01', '0.1.0+','0.1.0-a..b', '0.1.0\n', '65536.0.0']:
            with self.subTest(version=version), self.assertRaises(ValueError):
                release.version_info(version)

    def root(self, directory):
        root = Path(directory)
        (root / 'packaging').mkdir()
        (root / 'packaging/update-public-key.hex').write_text('11' * 32)
        (root / 'Cargo.toml').write_text('[package]\nname = "neptune-terminal"\nversion = "0.1.0"\n')
        (root / 'Cargo.lock').write_text('version = 4\n[[package]]\nname = "neptune-terminal"\nversion = "0.1.0"\n')
        (root / 'CHANGELOG.md').write_text('# Changelog\n\n## [Unreleased]\n\n### What\'s New\n\n- Better terminal startup.\n')
        return root

    def test_prepare_validate_and_single_notes_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self.root(directory)
            release.prepare('0.2.0-beta.1', root)
            self.assertEqual(release.validate('v0.2.0-beta.1', root), {'version': '0.2.0-beta.1', 'prerelease': 'true'})
            self.assertEqual(release.notes('0.2.0-beta.1', root), "### What's New\n\n- Better terminal startup.\n")
            self.assertIn('## [Unreleased]', (root / 'CHANGELOG.md').read_text())
            with self.assertRaises(ValueError): release.validate('v0.2.0', root)
            with self.assertRaises(ValueError): release.prepare('0.2.0-beta.1', root)
            (root / 'Cargo.lock').write_text((root / 'Cargo.lock').read_text().replace('0.2.0-beta.1', '0.1.0'))
            with self.assertRaises(ValueError): release.validate('v0.2.0-beta.1', root)

    def test_incomplete_release_cannot_generate_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, 'incomplete'):
                release.manifest('0.2.0', Path(directory), 'a' * 40)

    def test_signed_manifest_and_checksums_match_exact_final_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self.root(directory)
            release.prepare('0.2.0', root)
            dist = root / 'dist'
            dist.mkdir()
            for name in release.artifact_names('0.2.0').values(): (dist / name).write_bytes(b'final installer bytes')
            key = root / 'private.pem'
            subprocess.run(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', str(key)], check=True)
            public = subprocess.check_output(['openssl', 'pkey', '-in', str(key), '-pubout', '-outform', 'DER'])
            (root / 'packaging/update-public-key.hex').write_text(public[-32:].hex())
            with patch.object(release, 'ROOT', root), patch.object(release, 'notes', return_value=release.notes('0.2.0', root)), patch.dict(os.environ, {'UPDATE_SIGNING_KEY': key.read_text()}):
                release.manifest('0.2.0', dist, 'a' * 40)
            manifest = json.loads((dist / 'update-manifest.json').read_text())
            self.assertEqual(manifest['version'], '0.2.0')
            for asset in manifest['assets']:
                self.assertEqual(asset['sha256'], release.sha256(dist / asset['name']))
                self.assertEqual(asset['size'], len(b'final installer bytes'))
            for line in (dist / 'SHA256SUMS').read_text().splitlines():
                digest, name = line.split('  ')
                self.assertEqual(digest, release.sha256(dist / name))
            signature = root / 'raw.sig'
            signature.write_bytes(bytes.fromhex((dist / 'update-manifest.sig').read_text()))
            public_pem = root / 'public.pem'
            subprocess.run(['openssl', 'pkey', '-in', str(key), '-pubout', '-out', str(public_pem)], check=True)
            subprocess.run(['openssl', 'pkeyutl', '-verify', '-pubin', '-inkey', str(public_pem), '-rawin', '-in', str(dist / 'update-manifest.json'), '-sigfile', str(signature)], check=True, stdout=subprocess.DEVNULL)

    def test_published_release_rerun_never_mutates_public_assets(self):
        with patch.object(release, 'validate', return_value={'version': '0.2.0', 'prerelease': 'false'}), patch.object(release.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, '[{"tag_name":"v0.2.0","draft":false}]', '')) as run:
            with self.assertRaisesRegex(ValueError, 'already published'):
                release.stage('v0.2.0', Path('dist'))
            self.assertEqual(run.call_count, 1)

    def test_release_lookup_errors_fail_closed(self):
        with patch.object(release, 'validate', return_value={'version': '0.2.0', 'prerelease': 'false'}), patch.object(release.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1, '', 'HTTP 403')) as run:
            with self.assertRaisesRegex(ValueError, 'safely determine'):
                release.stage('v0.2.0', Path('dist'))
            self.assertEqual(run.call_count, 1)

    def test_upload_failure_remains_a_private_draft(self):
        with tempfile.TemporaryDirectory() as directory:
            dist = Path(directory)
            (dist / 'asset').write_bytes(b'installer')
            calls = []
            def run(command, **kwargs):
                calls.append(command)
                if command[:2] == ['gh', 'api']:
                    return subprocess.CompletedProcess(command, 0, '[]', '')
                if command[:3] == ['gh', 'release', 'upload']:
                    raise subprocess.CalledProcessError(1, command)
                return subprocess.CompletedProcess(command, 0)
            with patch.object(release, 'validate', return_value={'version': '0.2.0', 'prerelease': 'false'}), patch.object(release.subprocess, 'run', side_effect=run):
                with self.assertRaises(subprocess.CalledProcessError): release.stage('v0.2.0', dist)
            self.assertTrue(all('--draft' in c for c in calls if c[:3] in [['gh', 'release', 'create'], ['gh', 'release', 'edit']]))
            self.assertFalse(any('--draft=false' in c for c in calls))

    def test_completed_draft_gets_changelog_notes_only_after_all_uploads(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self.root(directory)
            release.prepare('0.2.0-beta.1', root)
            notes = root / 'release-notes.md'
            notes.write_text(release.notes('0.2.0-beta.1', root))
            dist = root / 'dist'
            dist.mkdir()
            names = [*release.artifact_names('0.2.0-beta.1').values(), 'SHA256SUMS', 'update-manifest.json', 'update-manifest.sig']
            for name in names: (dist / name).write_bytes(b'fixture')
            calls = []
            listings = iter([[], [{'tag_name': 'v0.2.0-beta.1', 'draft': True, 'assets': [{'name': name} for name in names]}]])
            def run(command, **kwargs):
                calls.append(command)
                if command[:2] == ['gh', 'api']:
                    self.assertNotIn('/releases/tags/', command[2])
                    return subprocess.CompletedProcess(command, 0, json.dumps(next(listings)), '')
                return subprocess.CompletedProcess(command, 0)
            with patch.object(release, 'validate', return_value=release.validate('v0.2.0-beta.1', root)), patch.object(release.subprocess, 'run', side_effect=run):
                release.stage('v0.2.0-beta.1', dist)
            upload_index = next(i for i, command in enumerate(calls) if command[:3] == ['gh', 'release', 'upload'])
            self.assertTrue(all('INCOMPLETE' in ' '.join(command) for command in calls[1:upload_index]))
            final = calls[-1]
            self.assertIn('--draft', final)
            self.assertIn('--prerelease=true', final)
            self.assertEqual(final[-2:], ['--notes-file', str(notes)])
            self.assertFalse(any('--draft=false' in command for command in calls))

    def test_draft_lookup_paginates_and_rejects_ambiguous_tags(self):
        first = [{'tag_name': f'v1.0.{i}', 'draft': False} for i in range(100)]
        draft = {'tag_name': 'v0.2.0', 'draft': True, 'assets': []}
        responses = [subprocess.CompletedProcess([], 0, json.dumps(page), '') for page in [first, [draft]]]
        with patch.object(release.subprocess, 'run', side_effect=responses) as run:
            self.assertEqual(release.release_for_tag('v0.2.0'), draft)
            self.assertIn('page=2', run.call_args_list[1].args[0][2])
        with patch.object(release.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, json.dumps([draft, draft]), '')):
            with self.assertRaisesRegex(ValueError, 'Ambiguous'):
                release.release_for_tag('v0.2.0')

    def test_existing_private_draft_replaces_stale_assets_without_publishing(self):
        with tempfile.TemporaryDirectory() as directory:
            dist = Path(directory)
            (dist / 'asset').write_bytes(b'installer')
            old = {'tag_name': 'v0.2.0', 'draft': True, 'assets': [{'id': 7, 'name': 'stale'}]}
            complete = {'tag_name': 'v0.2.0', 'draft': True, 'assets': [{'name': 'asset'}]}
            listings = iter([[old], [complete]])
            calls = []
            def run(command, **kwargs):
                calls.append(command)
                if command[:2] == ['gh', 'api'] and '?per_page=' in command[2]:
                    return subprocess.CompletedProcess(command, 0, json.dumps(next(listings)), '')
                return subprocess.CompletedProcess(command, 0)
            with patch.object(release, 'validate', return_value={'version': '0.2.0', 'prerelease': 'false'}), patch.object(release.subprocess, 'run', side_effect=run):
                release.stage('v0.2.0', dist)
            self.assertFalse(any(command[:3] == ['gh', 'release', 'create'] for command in calls))
            self.assertIn(['gh', 'api', '--method', 'DELETE', 'repos/zevem/neptune/releases/assets/7'], calls)
            edits = [command for command in calls if command[:3] == ['gh', 'release', 'edit']]
            self.assertEqual(len(edits), 2)
            self.assertTrue(all('--draft' in command for command in edits))
            self.assertIn('INCOMPLETE Neptune 0.2.0', edits[0])
            self.assertIn('Neptune 0.2.0', edits[1])

    def test_macos_missing_credentials_cannot_fall_back_to_unsigned_artifacts(self):
        spec = importlib.util.spec_from_file_location('sign_macos', Path(__file__).with_name('sign-macos.py'))
        signing = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(signing)
        with patch.dict(os.environ, {}, clear=True), patch('sys.argv', ['sign-macos.py', '--version', '0.2.0', '--platform', 'macos-arm64', '--binary', 'fixture']), patch.object(signing, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'Missing macOS release credentials'):
                signing.main()
            run.assert_not_called()


if __name__ == '__main__': unittest.main()
