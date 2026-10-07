"""#177 bounded qualification: a real local-command ACK, negative ACKs, network versus login, and no worker,
tick or claim after a failed bootstrap. Fixtures only for failures; no live worker, credential or network write."""
import argparse
import contextlib
import importlib
import io
import json
from pathlib import Path
import re
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
                     (q, 'codex_spawn'), (q, 'claude_spawn'), (tick, 'tick'), (tick, 'tick_pass'))]
        with patch.object(q, 'api', store), patch.object(q, 'STORE', store):
            _, code, envelope = preflight(lambda *a, **k: SimpleNamespace(returncode=1, stdout='', stderr='denied'))
        self.assertNotEqual(code, 0)
        self.assertEqual([action['action'] for action in envelope['actions']], ['local_command_ack'])
        self.assertEqual((envelope['tasks'], envelope['sessions']), ([], []))
        for mock in [store, *launches]:
            mock.assert_not_called()


class NetworkOrLogin(unittest.TestCase):
    """A failing `gh auth status` is told apart by one authorized read, `gh api user`; nothing is written."""
    def gap(self, reply):
        with patch.object(doctor, 'api_read', Mock(return_value=reply)) as read:
            found = doctor.login_gap('gh', None)
        read.assert_called_once_with(['gh', 'api', 'user'])
        return found

    def test_read_succeeds_no_gap(self):
        self.assertIsNone(self.gap((0, '')))  # `auth status` alone is not a diagnosis

    def test_network_denial_is_not_a_login_gap(self):
        for stderr in ('error connecting to api.github.com\ncheck your internet connection or https://githubstatus.com',
                       'Get "https://api.github.com/user": dial tcp: lookup api.github.com: no such host',
                       "Command '['gh', 'api', 'user']' timed out after 60 seconds"):
            with self.subTest(stderr=stderr):
                what, fix = self.gap((1, stderr))
                self.assertIn('network denied or offline, not a proven login gap', what)
                self.assertNotIn('auth login', fix.split('  (')[0])

    def test_server_permission_tls_and_unknown_are_not_login_gaps(self):
        """PM review of c94fa28: each of these was reported as «not logged in»."""
        cases = {'gh: Internal Server Error (HTTP 500)': 'server error (HTTP 5xx)',
                 'gh: Resource not accessible by integration (HTTP 403)': 'permission denied for this token (HTTP 403)',
                 'Get "https://api.github.com/user": tls: failed to verify certificate: x509: certificate signed by unknown authority':
                     'TLS/certificate failure',
                 '': 'unknown failure'}
        for stderr, kind in cases.items():
            with self.subTest(stderr=stderr):
                what, fix = self.gap((1, stderr))
                self.assertIn(f'failed: {kind}, not a proven login gap', what)
                self.assertNotIn('not logged in', what)
                self.assertEqual(fix.split('  (')[0], 'gh api user')
        self.assertIn('(no error output)', self.gap((1, ''))[0])

    def test_real_auth_failure_gives_owner_login_step(self):
        for stderr in ('gh: Bad credentials (HTTP 401)', 'gh: Requires authentication (HTTP 401)'):
            with self.subTest(stderr=stderr):
                what, fix = self.gap((1, stderr))
                self.assertEqual((what, fix.split('  (')[0]), ('`gh` is not logged in', 'gh auth login'))

    def test_api_read_is_read_only_and_survives_a_missing_cli(self):
        self.assertEqual(doctor.api_read(['taskq-no-such-cli-177', 'api', 'user'])[0], 1)
        self.assertEqual(doctor.api_read([sys.executable, '-c', 'import sys; sys.exit(0)']), (0, ''))
        self.assertTrue(re.search(dict(doctor.FAILURES)['network denied or offline'], str(subprocess.TimeoutExpired(['gh'], 60))))


if __name__ == '__main__':
    unittest.main()
