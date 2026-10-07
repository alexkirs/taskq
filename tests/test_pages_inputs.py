"""Real Git deletion, rename and multi-commit controls for conditional Pages."""
import importlib.util
import os
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
        self.assertEqual(self.reason('0' * 40), 'initial full build')
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
