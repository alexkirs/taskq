"""Selection must fail conservatively; legacy subTest failures must remain failures."""
import unittest
import json
from pathlib import Path
import subprocess
from tests.portfolio import CONTRACTS, affected, contracts_for, fingerprint

pytest_plugins = ['pytester']


def test_known_contracts_and_conservative_shared_changes():
    assert contracts_for('tests/test_host_service.py::ServiceTests::test_resume') == {'restart'}
    assert contracts_for('tests/test_single.py::TestStateKernel::runTest') >= {'lifecycle', 'capacity', 'recovery'}
    assert affected(['tests/test_host_service.py']) == {'restart'}
    for path in ('taskq.py', 'host_service.py', 'taskq.md', 'tests/conftest.py', '../unknown', 'new.py'):
        assert affected([path]) == set(CONTRACTS)
    assert affected(['tests/test_host_service.py', 'taskq.py']) == set(CONTRACTS)
    assert affected(['experiments/state_replay.py']) == {'lifecycle', 'recovery'}


class LegacySubtests(unittest.TestCase):
    def test_all_subtests_run(self):
        seen = []
        for n in (1, 2, 3):
            with self.subTest(case=n):
                seen.append(n)
                self.assertGreater(n, 0)
        self.assertEqual(seen, [1, 2, 3])


def test_failed_subtest_fails_runner_and_finishes_remaining_cases(pytester):
    pytester.makepyfile('''
import unittest
from pathlib import Path
class Legacy(unittest.TestCase):
    def test_cases(self):
        seen = []
        for n in (1, 2, 3):
            with self.subTest(case=n):
                seen.append(n)
                self.assertNotEqual(n, 2)
        Path('seen.txt').write_text(str(seen))
''')
    result = pytester.runpytest_subprocess('-q')
    assert result.ret == 1
    assert (pytester.path / 'seen.txt').read_text() == '[1, 2, 3]'
    result.stdout.fnmatch_lines(['*case=2*'])


def test_fingerprint_binds_bytes_without_rewriting_commit(tmp_path, monkeypatch):
    def git(*args):
        subprocess.run(['git', '-C', str(tmp_path), *args], check=True, capture_output=True)
    git('init')
    source = tmp_path / 'taskq.py'
    source.write_text('first\n', encoding='utf-8')
    git('add', 'taskq.py')
    git('-c', 'user.name=fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-m', 'fixture')
    first = fingerprint(tmp_path)
    monkeypatch.setenv('TASKQ_TOKEN', 'secret-never-in-receipt')
    assert fingerprint(tmp_path) == first
    source.write_text('second\n', encoding='utf-8')
    changed = fingerprint(tmp_path)
    assert changed['commit'] == first['commit'] and changed['tree'] == first['tree']
    assert changed['fingerprint'] != first['fingerprint']
    (tmp_path / 'new.py').write_text('new artifact\n', encoding='utf-8')
    assert fingerprint(tmp_path)['fingerprint'] != changed['fingerprint']


def test_receipt_scope_redaction_and_output_do_not_change_artifact(pytester):
    root = Path(__file__).resolve().parent.parent
    pytester.makeconftest(f'import sys\nsys.path.insert(0, {str(root)!r})\nfrom tests.conftest import *\n')
    pytester.makeini('[pytest]\ntestpaths = tests\n')
    folder = pytester.path / 'tests'
    folder.mkdir()
    (folder / 'test_one.py').write_text('def test_one(): assert True\n', encoding='utf-8')
    (folder / 'test_two.py').write_text('def test_two(): assert True\n', encoding='utf-8')
    (pytester.path / '.gitignore').write_text('test-receipt.json\n__pycache__/\nstdout\nstderr\nrunpytest-*\n', encoding='utf-8')
    def git(*args):
        subprocess.run(['git', '-C', str(pytester.path), *args], check=True, capture_output=True)
    git('init')
    git('add', '.')
    git('-c', 'user.name=fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-m', 'fixture')
    receipt = pytester.path / 'test-receipt.json'
    assert pytester.runpytest_subprocess('-q', '--receipt', str(receipt)).ret == 0
    full = json.loads(receipt.read_text(encoding='utf-8'))
    assert full['full_suite'] and full['selected'] == 2 and full['artifact_unchanged']
    assert pytester.runpytest_subprocess('tests', '--ignore=tests/test_two.py', '-q', '--receipt', str(receipt)).ret == 0
    subset = json.loads(receipt.read_text(encoding='utf-8'))
    assert not subset['full_suite'] and subset['selected'] == 1 and subset['artifact_unchanged']
    assert full['fingerprint'] == subset['fingerprint']
    assert full['selected_nodes_sha256'] != subset['selected_nodes_sha256']
    (folder / 'test_one.py').write_text('def test_one(): assert False, "SECRET-FIXTURE-TEXT"\n', encoding='utf-8')
    assert pytester.runpytest_subprocess('tests/test_one.py', '-q', '--receipt', str(receipt)).ret == 1
    failed = json.loads(receipt.read_text(encoding='utf-8'))
    assert failed['failures'] == [{'node': 'tests/test_one.py::test_one', 'phase': 'call', 'contracts': []}]
    assert 'SECRET-FIXTURE-TEXT' not in receipt.read_text(encoding='utf-8')
