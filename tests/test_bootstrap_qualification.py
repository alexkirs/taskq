"""#177 bounded qualification: a real local-command ACK, negative ACKs, network versus login, and no worker,
tick or claim after a failed bootstrap. Fixtures only for failures; no live worker, credential or network write."""
import argparse
import contextlib
import importlib
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

import taskq as q

worker = importlib.import_module('taskq.worker')
doctor = importlib.import_module('taskq.doctor')


def preflight(run=None):
    """(ACK, exit code, stdout envelope) of `taskq preflight --json`; `run` replaces the subprocess call."""
    command = argparse.Namespace(action='preflight', function=worker.preflight)
    with contextlib.ExitStack() as stack:
        if run is not None:
            stack.enter_context(patch.object(worker.subprocess, 'run', run))
        printed = stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
        try:
            q.json_command(command)
            code = 0
        except SystemExit as error:
            code = error.code
    envelope = json.loads(printed.getvalue())
    return envelope['actions'][0], code, envelope


class LocalAck(unittest.TestCase):
    def setUp(self):
        self.enterContext(patch.object(q, 'PROJECT', 'test'))
        self.enterContext(patch.object(q, 'ROOT', Path.cwd()))

    def test_real_ack_has_stdout_stderr_exit_time_cwd_host(self):
        ack, code, envelope = preflight()
        self.assertEqual((code, envelope['outcome'], ack['status'], ack['exit_code']), (0, 'ok', 'ready', 0))
        self.assertEqual((ack['stdout'].strip(), ack['stderr'], ack['source']), (str(Path.cwd().resolve()), '', 'local subprocess'))
        self.assertRegex(ack['observed_at'], r'^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$')
        self.assertEqual(ack['host'], q.machine())
        # Ready proves local execution only: capability and launch policy stay unknown, never inferred.
        self.assertEqual((ack['runtime_capability'], ack['effective_launch_policy']), ('unknown', 'unknown'))

    def test_negative_acks_fail_closed(self):
        cases = {'exit 0 but another cwd': lambda *a, **k: SimpleNamespace(returncode=0, stdout='/elsewhere\n', stderr=''),
                 'denied': lambda *a, **k: SimpleNamespace(returncode=1, stdout='', stderr='Operation not permitted'),
                 'no interpreter': Mock(side_effect=FileNotFoundError(2, 'No such file or directory')),
                 'no capability to run': Mock(side_effect=PermissionError(13, 'Permission denied')),
                 'no reply': Mock(side_effect=subprocess.TimeoutExpired('probe', 30))}
        for name, run in cases.items():
            with self.subTest(name):
                ack, code, envelope = preflight(run)
                self.assertNotEqual(code, 0)
                self.assertEqual((envelope['outcome'], ack['status']), ('failure', 'unknown'))
                self.assertTrue(ack['exact_blocker'])
                self.assertEqual((ack['runtime_capability'], ack['effective_launch_policy']), ('unknown', 'unknown'))

    def test_failed_bootstrap_starts_no_worker_tick_or_claim(self):
        store = Mock(side_effect=AssertionError('the queue was touched'))
        tick = importlib.import_module('taskq.tick')
        launches = [self.enterContext(patch.object(module, name)) for module, name in
                    ((worker, 'spawn'), (worker, 'claude_spawn'), (worker, 'executor_run'), (worker, 'claude_wake'), (worker, 'take'),
                     (q, 'codex_spawn'), (q, 'claude_spawn'), (tick, 'tick'), (tick, 'queue_pass'))]
        with patch.object(q, 'api', store), patch.object(q, 'STORE', store):
            _, code, envelope = preflight(lambda *a, **k: SimpleNamespace(returncode=1, stdout='', stderr='denied'))
        self.assertNotEqual(code, 0)
        self.assertEqual([action['action'] for action in envelope['actions']], ['local_command_ack'])
        self.assertEqual(envelope['tasks'], [])
        for mock in [store, *launches]:
            mock.assert_not_called()


class NetworkOrLogin(unittest.TestCase):
    """A failed authorized read is shown exactly; nothing is written."""
    def gap(self, reply):
        with patch.object(doctor, 'api_read', Mock(return_value=reply)) as read:
            found = doctor.login_gap('gh', None)
        read.assert_called_once_with(['gh', 'api', 'user'])
        return found

    def test_read_succeeds_no_gap(self):
        self.assertIsNone(self.gap((0, '')))  # `auth status` alone is not a diagnosis

    def test_failure_is_printed_as_is(self):
        for stderr in ('error connecting to api.github.com\ncheck your internet connection or https://githubstatus.com',
                       'gh: Bad credentials (HTTP 401)',
                       'gh: Resource not accessible by integration (HTTP 403)'):
            with self.subTest(stderr=stderr):
                what, fix = self.gap((1, stderr))
                self.assertEqual((what, fix), (stderr.splitlines()[-1], 'gh api user'))
        self.assertEqual(self.gap((1, ''))[0], 'gh api user failed without error output')

    def test_api_read_is_read_only_and_survives_a_missing_cli(self):
        self.assertEqual(doctor.api_read(['taskq-no-such-cli-177', 'api', 'user'])[0], 1)
        self.assertEqual(doctor.api_read([sys.executable, '-c', 'import sys; sys.exit(0)']), (0, ''))


if __name__ == '__main__':
    unittest.main()
