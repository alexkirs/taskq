"""Typed permission observations and local execution ACKs, without live workers or credentials."""
import argparse
import contextlib
import importlib
import io
import json
import subprocess
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

import taskq as q

codex = importlib.import_module('taskq.codex')
worker = importlib.import_module('taskq.worker')
tick = importlib.import_module('taskq.tick')


class RuntimeObservationTests(unittest.TestCase):
    def connection(self):
        client = codex.Codex.__new__(codex.Codex)
        client.counter, client.permission_requests = 0, {}
        client.send = Mock()
        return client

    def test_supported_request_survives_rpc_without_approval_reply(self):
        client = self.connection()
        request = {'id': 'r1', 'method': 'item/commandExecution/requestApproval',
                   'params': {'threadId': 't1', 'turnId': 'u1', 'itemId': 'i1',
                              'startedAtMs': 12000, 'command': 'private args'}}
        client.receive = Mock(side_effect=[request, {'id': 1, 'result': {'ok': True}}])
        self.assertEqual(client.call('thread/read', {}), {'ok': True})
        client.send.assert_called_once_with({'id': 1, 'method': 'thread/read', 'params': {}})
        status = codex.codex_observation(client, 't1', {'type': 'active', 'activeFlags': []}, [], None)
        self.assertEqual(status['status'], 'waiting_permission')
        self.assertEqual(status['permission_requests'][0]['event_at'], 12)
        self.assertNotIn('private args', json.dumps(status))
        client.observe({'method': 'serverRequest/resolved', 'params': {'threadId': 't1', 'requestId': 'r1'}})
        self.assertEqual(codex.codex_observation(client, 't1', {'type': 'active'}, [], None)['status'], 'unknown')

    def test_empty_flags_are_unknown_and_link_is_preserved(self):
        observation = codex.codex_observation(self.connection(), 't1', {'type': 'active', 'activeFlags': []},
                                             [{'status': 'inProgress', 'entries': []}], None)
        self.assertEqual(observation['status'], 'unknown')
        self.assertEqual(observation['approval_visibility'], 'unknown')
        self.assertTrue(observation['session_link'].endswith('#codex://threads/t1'))
        self.assertIsNone(observation['event_at'])

    def test_running_item_and_terminal_are_distinct(self):
        turn = {'status': 'inProgress', 'entries': [{'item': {'type': 'commandExecution', 'status': 'inProgress'}}]}
        self.assertEqual(codex.codex_observation(self.connection(), 't', {'type': 'active'}, [turn], 12)['status'], 'active')
        turn['status'] = 'completed'
        self.assertEqual(codex.codex_observation(self.connection(), 't', {'type': 'active'}, [turn], 13)['status'], 'unknown')
        self.assertEqual(codex.codex_observation(self.connection(), 't', {'type': 'idle'}, [turn], 13)['status'], 'terminal')

    def test_permission_wait_is_not_idle_and_wake_is_deduplicated(self):
        client = self.connection()
        client.socket = Mock()
        item = {'claim': {'session': 't', 'runtime': 'codex'}, 'state': 'doing'}
        with patch.object(q, 'Codex', return_value=client), patch.object(q, 'codex_snapshot',
                return_value=({'type': 'idle', 'activeFlags': ['waitingOnApproval']}, [], None)):
            state, _ = tick.liveness(item, {})
        self.assertEqual(state, 'busy')
        key = item['_runtime_observation']['notify_dedup']
        with tempfile.TemporaryDirectory() as directory, patch.object(tick, 'woken', return_value=Path(directory) / 'woken'), \
             patch.object(q, 'personal', return_value={'coordinator': {'session': 'pm'}}), \
             patch.object(q, 'claude_agents', return_value={}), patch.object(q, 'claude_wake') as wake, \
             contextlib.redirect_stdout(io.StringIO()):
            tick.wake('owner approval link', [key])
            tick.wake('owner approval link', [key])
        wake.assert_called_once()

    def test_unknown_and_active_not_loaded_are_not_nudged(self):
        client = self.connection()
        client.socket = Mock()
        item = {'claim': {'session': 't', 'runtime': 'codex'}, 'state': 'doing'}
        for entries in ([], [{'item': {'type': 'commandExecution', 'status': 'inProgress'}}]):
            with patch.object(q, 'Codex', return_value=client), patch.object(q, 'codex_snapshot',
                    return_value=({'type': 'notLoaded', 'activeFlags': []}, [{'status': 'inProgress', 'entries': entries}], None)):
                self.assertEqual(tick.liveness(item, {})[0], 'busy')

    def test_real_local_preflight_and_failure_ack(self):
        args = argparse.Namespace(output={'actions': []})
        with tempfile.TemporaryDirectory() as directory, patch.object(q, 'ROOT', Path(directory)), \
             contextlib.redirect_stdout(io.StringIO()):
            worker.preflight(args)
        ack = args.output['actions'][0]
        self.assertEqual((ack['status'], ack['exit_code']), ('ready', 0))
        self.assertEqual(ack['stdout'].strip(), ack['cwd'])
        self.assertEqual(ack['runtime_capability'], 'unknown')
        with patch.object(q, 'ROOT', Path.cwd()), \
             patch.object(worker.subprocess, 'run', return_value=SimpleNamespace(returncode=1, stdout='', stderr='denied')), \
             contextlib.redirect_stdout(io.StringIO()), self.assertRaises(SystemExit):
            worker.preflight(argparse.Namespace())

    def test_claude_without_pid_or_status_is_unknown(self):
        args = argparse.Namespace(runtime='claude', session='s', output={'actions': []})
        with patch.object(q, 'claude_agents', return_value={'s': {'sessionId': 's'}}), \
             patch.object(q, 'claude_url', return_value=None), contextlib.redirect_stdout(io.StringIO()):
            worker.runtime_status(args)
        self.assertEqual(args.output['actions'][0]['status'], 'unknown')

    def test_preflight_timeout_and_oserror_return_failure_ack(self):
        errors = [(subprocess.TimeoutExpired('probe', 30, output=b'partial output', stderr=b'partial error'),
                   124, 'partial output', 'partial error'),
                  (OSError('permission denied'), 126, '', 'permission denied')]
        for error, code, stdout, stderr in errors:
            args = argparse.Namespace(output={'actions': []})
            with self.subTest(error=error), patch.object(q, 'ROOT', Path.cwd()), \
                 patch.object(worker.subprocess, 'run', side_effect=error), \
                 contextlib.redirect_stdout(io.StringIO()) as printed, self.assertRaises(SystemExit):
                worker.preflight(args)
            ack = json.loads(printed.getvalue())
            self.assertEqual((ack['status'], ack['exit_code']), ('unknown', code))
            self.assertEqual((ack['stdout'], ack['stderr']), (stdout, stderr))
            self.assertTrue(ack['exact_blocker'])
            self.assertEqual(args.output['actions'][0]['status'], 'unknown')
            command = argparse.Namespace(action='preflight', function=worker.preflight)
            with patch.object(q, 'ROOT', Path.cwd()), patch.object(q, 'PROJECT', 'test'), \
                 patch.object(worker.subprocess, 'run', side_effect=error), \
                 contextlib.redirect_stdout(io.StringIO()) as printed, self.assertRaises(SystemExit) as stopped:
                q.json_command(command)
            envelope = json.loads(printed.getvalue())
            self.assertNotEqual(stopped.exception.code, 0)
            self.assertEqual(envelope['outcome'], 'failure')
            self.assertEqual(envelope['actions'][0]['exit_code'], code)


if __name__ == '__main__':
    unittest.main()
