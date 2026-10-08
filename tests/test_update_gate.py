"""CI-green update gate, without network or queue mutations."""
import importlib
import json
from types import SimpleNamespace
import unittest
from unittest.mock import patch

doctor = importlib.import_module('taskq.doctor')


class Gate(unittest.TestCase):
    def gate(self, runs, returncode=0):
        command = []
        def run(argv, **kwargs):
            command.extend(argv)
            return SimpleNamespace(returncode=returncode, stdout=json.dumps(runs))
        with patch.object(doctor.subprocess, 'run', run):
            found = doctor.green('a' * 40)
        return found, command

    def test_exact_sha_tests_must_succeed(self):
        found, command = self.gate([{'conclusion': 'success'}])
        self.assertIsNone(found)
        self.assertEqual(command, ['gh', 'run', 'list', '--commit', 'a' * 40,
                                   '--workflow', 'tests.yml', '--json', 'conclusion'])

    def test_missing_running_or_failed_tests_refuse(self):
        for runs, expected in (([], 'it has no exact-SHA tests run yet'),
                               ([{'conclusion': None}], 'CI still running: tests'),
                               ([{'conclusion': 'failure'}], 'CI failed: tests'),
                               ([{'conclusion': 'success'}, {'conclusion': 'cancelled'}], 'CI failed: tests')):
            with self.subTest(runs=runs):
                self.assertEqual(self.gate(runs)[0], expected)

    def test_unreadable_ci_refuses(self):
        self.assertEqual(self.gate([], returncode=1)[0], 'its CI could not be read (gh api)')
        with patch.object(doctor.subprocess, 'run', return_value=SimpleNamespace(returncode=0, stdout='broken')):
            self.assertEqual(doctor.green('a' * 40), 'its CI could not be read (gh api)')
        with patch.object(doctor.subprocess, 'run', side_effect=OSError):
            self.assertEqual(doctor.green('a' * 40), 'its CI could not be read (gh api)')


if __name__ == '__main__':
    unittest.main()
