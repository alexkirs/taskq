"""Exact-SHA workflow/suite controls, without network or live queue mutations."""
import copy
import json
import importlib
from types import SimpleNamespace
import unittest
from unittest.mock import patch

doctor = importlib.import_module('taskq.doctor')


class Gate(unittest.TestCase):
    def setUp(self):
        self.sha = 'a' * 40
        self.workflow = dict(id=10, path='.github/workflows/tests.yml', state='active')
        self.runs = [dict(id=20, workflow_id=10, path=self.workflow['path'], event='push',
                          repository={'full_name': 'alexkirs/taskq'}, head_sha=self.sha,
                          check_suite_id=30, status='completed', conclusion='success')]
        self.checks = [self.check('tests', 30)]

    def check(self, name, suite, status='completed', conclusion='success', identity=1):
        return dict(id=identity, name=name, check_suite={'id': suite}, head_sha=self.sha,
                    app={'slug': 'github-actions', 'id': 15368}, status=status, conclusion=conclusion)

    def gate(self):
        self.commands = []
        def run(command, **kwargs):
            self.commands.append(command)
            endpoint = command[2]
            data = self.workflow if 'workflows/tests.yml' in endpoint else (
                self.runs if 'actions/runs?' in endpoint else self.checks)
            if 'workflows/tests.yml' not in endpoint:
                data = [{('workflow_runs' if 'actions/runs?' in endpoint else 'check_runs'): data}]
            return SimpleNamespace(returncode=0, stdout=json.dumps(data))
        with patch.object(doctor.core, 'REPO', 'https://github.com/alexkirs/taskq'), \
                patch.object(doctor.subprocess, 'run', run):
            return doctor.green(self.sha)

    def test_exact_success_and_complete_pagination(self):
        self.assertIsNone(self.gate())
        for command in self.commands[1:]:
            self.assertIn('--paginate', command)
            self.assertIn('--slurp', command)
            self.assertIn(self.sha, command[2])
        self.assertIn('filter=all', self.commands[-1][2])

    def test_required_tests_refuse_every_non_success(self):
        for conclusion in ('failure', 'cancelled', 'timed_out', 'skipped', 'neutral', None):
            with self.subTest(conclusion=conclusion):
                self.checks[0]['conclusion'] = conclusion
                self.assertEqual(self.gate(), 'CI failed: tests')
        self.checks[0].update(status='queued', conclusion=None)
        self.assertEqual(self.gate(), 'CI still running: tests')

    def test_workflow_must_complete_success(self):
        for status, conclusion in [('queued', None), ('in_progress', None), ('completed', 'skipped'),
                                   ('completed', 'neutral'), ('completed', 'failure')]:
            self.runs[0].update(status=status, conclusion=conclusion)
            self.assertIsNotNone(self.gate())

    def test_missing_or_wrong_workflow_identity_cannot_satisfy_tests(self):
        original = copy.deepcopy(self.runs[0])
        for key, value in [('head_sha', 'b' * 40), ('path', '.github/workflows/other.yml'),
                           ('workflow_id', 11), ('event', 'pull_request'),
                           ('repository', {'full_name': 'someone/taskq'})]:
            self.runs[0] = dict(original, **{key: value})
            self.assertEqual(self.gate(), 'it has no trusted exact-SHA tests run yet')
        self.runs = []
        self.assertIsNotNone(self.gate())

    def test_missing_wrong_app_or_suite_and_wrong_sha_check_refuse(self):
        original = copy.deepcopy(self.checks[0])
        for key, value in [('head_sha', 'b' * 40), ('check_suite', {'id': 99}),
                           ('app', {'id': 99, 'slug': 'github-actions'}),
                           ('app', {'id': 15368, 'slug': 'other'}), ('name', 'lint')]:
            self.checks[0] = dict(original, **{key: value})
            self.assertIsNotNone(self.gate())
        self.checks = []
        self.assertIsNotNone(self.gate())

    def test_only_identified_pages_are_excluded(self):
        pages = dict(self.runs[0], id=21, workflow_id=11, check_suite_id=31,
                     path='dynamic/pages/pages-build-deployment', event='dynamic', conclusion='failure')
        self.runs.append(pages)
        self.checks.append(self.check('build', 31, conclusion='failure'))
        self.checks.append(self.check('deploy', 31, status='queued', conclusion=None))
        self.assertIsNone(self.gate())
        pages.update(path='.github/workflows/pages.yml', event='push')
        self.assertIsNone(self.gate())
        pages['event'] = 'workflow_dispatch'
        self.assertIsNone(self.gate())
        pages['path'] = '.github/workflows/unrelated.yml'
        self.assertIn('CI failed: build', self.gate())
        pages['path'] = '.github/workflows/pages.yml'
        self.checks[-1]['app']['id'] = 99
        self.assertIn('CI still running: deploy', self.gate())

    def test_unknown_checks_retain_existing_gate_even_same_name(self):
        other = self.check('tests', 99, conclusion='failure')
        self.checks.append(other)
        self.assertEqual(self.gate(), 'CI failed: tests')
        other.update(status='in_progress', conclusion=None)
        self.assertEqual(self.gate(), 'CI still running: tests')
        for conclusion in ('success', 'skipped', 'neutral'):
            other.update(status='completed', conclusion=conclusion)
            self.assertIsNone(self.gate())

    def test_reruns_use_latest_check_and_latest_trusted_run(self):
        self.checks[0]['conclusion'] = 'failure'
        self.checks.append(self.check('tests', 30, identity=2))
        self.assertIsNone(self.gate())
        self.checks.append(self.check('tests', 30, status='in_progress', conclusion=None, identity=3))
        self.assertEqual(self.gate(), 'CI still running: tests')
        self.runs.append(dict(self.runs[0], id=22, check_suite_id=32))
        self.checks.append(self.check('tests', 32))
        self.assertIsNone(self.gate())
        self.checks.append(self.check('lint', 30, conclusion='failure'))
        self.assertEqual(self.gate(), 'CI failed: lint')
        self.runs[-1]['status'] = 'in_progress'
        self.assertEqual(self.gate(), 'CI still running: tests')

    def test_unreadable_and_malformed_ci_refuse(self):
        for data in (None, {}, [None]):
            self.checks = data
            self.assertEqual(self.gate(), 'its CI could not be read (gh api)')
        for response in (SimpleNamespace(returncode=1, stdout=''), SimpleNamespace(returncode=0, stdout='broken')):
            with patch.object(doctor.subprocess, 'run', return_value=response):
                self.assertEqual(doctor.green(self.sha), 'its CI could not be read (gh api)')
        with patch.object(doctor.subprocess, 'run', side_effect=OSError):
            self.assertEqual(doctor.green(self.sha), 'its CI could not be read (gh api)')


if __name__ == '__main__':
    unittest.main()
