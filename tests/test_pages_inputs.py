"""Real Git deletion, rename and multi-commit controls for conditional Pages."""
import importlib.util
import os
import io
import json
from unittest.mock import patch
from pathlib import Path
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('pages_inputs', Path(__file__).resolve().parents[1] / '.github/scripts/pages_inputs.py')
inputs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inputs)


class PagesInputs(unittest.TestCase):
    def setUp(self):
        self.directory = self.enterContext(tempfile.TemporaryDirectory())
        before = Path.cwd()
        os.chdir(self.directory)
        self.addCleanup(os.chdir, before)
        self.git('init', '-q')
        self.write('.github/workflows/pages.yml', 'workflow')
        self.write('docs/open.html', 'bridge')
        self.write('README.md', 'cli')
        self.base = self.commit()

    def git(self, *args):
        return subprocess.check_output(['git', '-c', 'user.name=test', '-c', 'user.email=test@example.com', *args],
                                       stderr=subprocess.DEVNULL, text=True).strip()

    def write(self, path, text):
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def commit(self):
        self.git('add', '-A')
        self.git('commit', '-qm', 'test')
        return self.git('rev-parse', 'HEAD')

    def reason(self, before=None):
        return inputs.publication_reason(self.base if before is None else before, self.git('rev-parse', 'HEAD'), 'push')

    def test_cli_only_skips_but_manual_initial_unknown_build(self):
        self.write('README.md', 'changed cli')
        self.commit()
        self.assertEqual(self.reason(), 'site inputs unchanged')
        self.assertEqual(self.reason('0' * 40), 'unknown deployment baseline: full build')
        self.assertEqual(self.reason('f' * 40), 'unknown or pre-migration baseline: full build')
        self.assertEqual(inputs.publication_reason(self.base, self.base, 'workflow_dispatch'), 'manual full build')

    def test_source_config_dependency_workflow_and_detector_build(self):
        for path in ('docs/open.html', 'docs/_config.yml', 'docs/Gemfile', 'docs/Gemfile.lock',
                     '.github/workflows/pages.yml', '.github/scripts/pages_inputs.py'):
            with self.subTest(path=path):
                self.write(path, 'new site input')
                self.commit()
                self.assertEqual(self.reason(), 'site inputs changed')
                self.base = self.git('rev-parse', 'HEAD')

    def test_deletion_and_rename_outside_docs_build(self):
        Path('docs/open.html').rename('bridge.html')
        self.commit()
        self.assertEqual(self.reason(), 'site inputs changed')
        self.write('docs/old.md', 'old')
        self.base = self.commit()
        Path('docs/old.md').unlink()
        self.commit()
        self.assertEqual(self.reason(), 'site inputs changed')

    def test_multi_commit_push_and_more_than_300_files(self):
        self.write('docs/new.md', 'site')
        self.commit()
        for i in range(301):
            self.write(f'cli/{i}', 'cli')
        self.commit()
        self.assertEqual(self.reason(), 'site inputs changed')

    def metadata(self, state, sha=None):
        deployment = [{'id': 1, 'sha': sha or self.base, 'environment': 'github-pages'}]
        responses = [io.BytesIO(json.dumps(data).encode()) for data in (deployment, [{'state': state}])]
        return patch.object(inputs.urllib.request, 'urlopen', side_effect=responses)

    def test_pending_replacement_compares_deployed_a_to_c(self):
        deployed_a = self.base
        self.write('docs/open.html', 'unpublished B')
        pending_b = self.commit()
        self.write('README.md', 'CLI-only C replaces pending B')
        candidate_c = self.commit()
        self.assertEqual(inputs.publication_reason(pending_b, candidate_c, 'push'), 'site inputs unchanged')
        with self.metadata('success', deployed_a):
            baseline = inputs.deployed_baseline('alexkirs/taskq')
        self.assertEqual(baseline, deployed_a)
        self.assertEqual(inputs.publication_reason(baseline, candidate_c, 'push'), 'site inputs changed')

    def test_failed_or_pending_deployment_forces_recovery(self):
        self.write('docs/open.html', 'B failed to deploy')
        failed_b = self.commit()
        self.write('README.md', 'CLI-only C')
        candidate_c = self.commit()
        for state in ('failure', 'error', 'pending', 'in_progress', 'queued', 'inactive'):
            with self.subTest(state=state), self.metadata(state, failed_b):
                baseline = inputs.deployed_baseline('alexkirs/taskq')
            self.assertEqual(baseline, '')
            self.assertEqual(inputs.publication_reason(baseline, candidate_c, 'push'),
                             'unknown deployment baseline: full build')
        with self.metadata('success', failed_b):
            self.assertEqual(inputs.publication_reason(inputs.deployed_baseline('alexkirs/taskq'), candidate_c, 'push'),
                             'site inputs unchanged')

    def test_unreadable_missing_or_malformed_metadata_builds_fully(self):
        for data in ([], {}, [{'id': 1, 'sha': 'bad', 'environment': 'github-pages'}]):
            with patch.object(inputs.urllib.request, 'urlopen', return_value=io.BytesIO(json.dumps(data).encode())):
                self.assertEqual(inputs.deployed_baseline('alexkirs/taskq'), '')
        with patch.object(inputs.urllib.request, 'urlopen', side_effect=OSError):
            self.assertEqual(inputs.deployed_baseline('alexkirs/taskq'), '')
        with patch.object(inputs.urllib.request, 'urlopen') as read:
            self.assertEqual(inputs.deployed_baseline('../bad'), '')
            read.assert_not_called()

    def test_pre_migration_baseline_builds_fully(self):
        Path('.github/workflows/pages.yml').unlink()
        before = self.commit()
        self.write('.github/workflows/pages.yml', 'workflow')
        self.commit()
        self.assertEqual(self.reason(before), 'unknown or pre-migration baseline: full build')

    def test_unreadable_diff_builds_fully(self):
        self.assertEqual(inputs.publication_reason(self.base, 'f' * 40, 'push'), 'unreadable diff: full build')


if __name__ == '__main__':
    unittest.main()
