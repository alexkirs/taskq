"""Actual subprocess outcomes survive qualification/cleanliness failures."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

GATE = Path(__file__).with_name('native_gate.py')


class NativeGate(unittest.TestCase):
    def invoke(self, body):
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder); root = base / 'repo'; root.mkdir(); (root / 'tests').mkdir()
            source = 'import unittest,os\nfrom pathlib import Path\nclass One(unittest.TestCase):\n def test_one(self):\n  ' + body + '\n'
            (root / 'tests' / 'test_one.py').write_bytes(source.encode('utf-8'))
            def git(*args):
                subprocess.run(['git', '-C', str(root), *args], check=True, capture_output=True)
            git('init'); git('config', 'core.autocrlf', 'false')
            git('config', 'user.email', 'fixture@example.invalid'); git('config', 'user.name', 'Fixture')
            git('add', '.'); git('commit', '-m', 'fixture')
            env = dict(os.environ)
            for key in ('PYTHONUTF8', 'PYTHONIOENCODING'):
                env.pop(key, None)
            done = subprocess.run([sys.executable, '-B', str(GATE), '--root', str(root),
                                   '--output', str(base / 'evidence')], env=env, capture_output=True)
            receipt = json.loads((base / 'evidence' / 'receipt.json').read_text('utf-8'))
            receipt['hypothesis_cache_retained'] = (base / 'evidence' / 'hypothesis' / 'cache').exists()
            return done.returncode, receipt

    def test_clean_pass(self):
        code, receipt = self.invoke('self.assertTrue(True)')
        self.assertEqual((code, receipt['test_exit'], receipt['error']), (0, 0, None))

    def test_test_failure_stays_distinct_from_clean_postcheck(self):
        code, receipt = self.invoke('self.fail("expected")')
        self.assertEqual((code, receipt['test_exit'], receipt['error']), (1, 1, None))
        self.assertEqual(receipt['after']['status'], '')

    def test_success_preserved_when_untracked_artifact_refuses_gate(self):
        code, receipt = self.invoke('Path("unexpected").write_bytes(b"fixture")')
        self.assertEqual((code, receipt['test_exit']), (1, 0))
        self.assertIn('post-gate', receipt['error'])
        self.assertIn('unexpected', receipt['after']['status'])
        self.assertIn('log_sha256', receipt)

    def test_success_preserved_when_tracked_source_changes(self):
        code, receipt = self.invoke('Path(__file__).write_bytes(b"changed")')
        self.assertEqual((code, receipt['test_exit']), (1, 0))
        self.assertEqual(receipt['after']['mismatches'], ['tests/test_one.py'])

    def test_hypothesis_storage_is_retained_outside_clean_checkout(self):
        code, receipt = self.invoke('cache=Path(os.environ["HYPOTHESIS_STORAGE_DIRECTORY"]); cache.mkdir(); (cache/"cache").write_bytes(b"fixture")')
        self.assertEqual((code, receipt['test_exit'], receipt['after']['status']), (0, 0, ''))
        self.assertTrue(receipt['hypothesis_cache_retained'])
