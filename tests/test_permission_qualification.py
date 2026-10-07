"""#176 qualification fixtures: typed permission observations, request dedup, claim preservation, launch policy readback."""
import argparse
import contextlib
import importlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import taskq as q

codex = importlib.import_module('taskq.codex')
worker = importlib.import_module('taskq.worker')
tick = importlib.import_module('taskq.tick')


def connection():
    client = codex.Codex.__new__(codex.Codex)
    client.counter, client.permission_requests, client.socket, client.send = 0, {}, Mock(), Mock()
    return client


def request(request_id, thread='t', at=12000):
    return {'id': request_id, 'method': 'item/commandExecution/requestApproval',
            'params': {'threadId': thread, 'turnId': 'u', 'itemId': 'i', 'startedAtMs': at}}


class PermissionQualificationTests(unittest.TestCase):
    def liveness(self, client, status, turns=()):
        item = {'claim': {'session': 't', 'runtime': 'codex'}, 'state': 'doing', 'updated': '2000-01-01T00:00:00Z'}
        with patch.object(q, 'Codex', return_value=client), \
             patch.object(q, 'codex_snapshot', return_value=(status, list(turns), None)):
            return tick.liveness(item, {}), item.get('_runtime_observation')

    def wakes(self, keys):
        with tempfile.TemporaryDirectory() as directory, patch.object(tick, 'woken', return_value=Path(directory) / 'woken'), \
             patch.object(q, 'personal', return_value={'coordinator': {'session': 'pm'}}), \
             patch.object(q, 'claude_agents', return_value={}), patch.object(q, 'claude_wake') as wake, \
             contextlib.redirect_stdout(io.StringIO()):
            for key in keys:
                tick.wake('permission', [key])
        return wake.call_count

    def test_waiting_request_notifies_once_and_new_request_notifies_again(self):
        client = connection()
        client.observe(request('r1'))
        (state, _), first = self.liveness(client, {'type': 'active', 'activeFlags': []})
        self.assertEqual((state, first['status']), ('busy', 'waiting_permission'))
        self.assertEqual(first['notify_dedup'], 'permission codex t r1')
        client.observe(request('r2', at=13000))
        second = self.liveness(client, {'type': 'active', 'activeFlags': []})[1]
        self.assertEqual(second['notify_dedup'], 'permission codex t r1,r2')
        self.assertEqual(self.wakes([first['notify_dedup']] * 2), 1)
        self.assertEqual(self.wakes([first['notify_dedup'], first['notify_dedup'], second['notify_dedup']]), 2)

    def test_flag_without_request_id_invents_none(self):
        observation = self.liveness(connection(), {'type': 'active', 'activeFlags': ['waitingOnApproval']})[1]
        self.assertEqual(observation['status'], 'waiting_permission')
        self.assertEqual(observation['permission_requests'], [])
        self.assertEqual(observation['notify_dedup'], 'permission codex t flag')

    def test_resolved_request_does_not_claim_approved_or_denied(self):
        client = connection()
        client.observe(request('r1'))
        client.observe({'method': 'serverRequest/resolved', 'params': {'threadId': 't', 'requestId': 'r1'}})
        observation = codex.codex_observation(client, 't', {'type': 'active', 'activeFlags': []}, [], None)
        self.assertEqual((observation['status'], observation['approval_visibility']), ('unknown', 'unknown'))
        self.assertIsNone(observation['notify_dedup'])
        declined = {'status': 'completed', 'entries': [{'item': {'type': 'commandExecution', 'status': 'declined'}}]}
        self.assertEqual(codex.codex_observation(client, 't', {'type': 'idle'}, [declined], 14)['status'], 'terminal')

    def test_stale_event_keeps_claim_and_separates_times(self):
        client = connection()
        client.observe(request('r1', at=1000))
        (state, _), observation = self.liveness(client, {'type': 'idle', 'activeFlags': []})
        self.assertEqual(state, 'busy')  # not idle: no nudge; not None: no 120-minute stale release
        self.assertEqual(observation['permission_requests'][0]['event_at'], 1)
        self.assertNotEqual(observation['observed_at'], observation['permission_requests'][0]['event_at'])

    def test_unreachable_is_unknown_not_idle_or_dead(self):
        with patch.object(q, 'Codex', side_effect=OSError('no socket')):
            state, activity = tick.liveness({'claim': {'session': 't', 'runtime': 'codex'}, 'state': 'doing'}, {})
        self.assertIsNone(state)
        self.assertIn('status unknown', activity)
        args = argparse.Namespace(runtime='codex', session='t', output={'actions': []})
        with patch.object(q, 'Codex', side_effect=OSError('no socket')), contextlib.redirect_stdout(io.StringIO()):
            worker.runtime_status(args)
        observation = args.output['actions'][0]
        self.assertEqual((observation['status'], observation['effective_launch_policy']), ('unknown', 'unknown'))
        self.assertIn('Status unavailable', observation['exact_blocker'])

    def test_codex_readback_keeps_metadata_time_apart(self):
        client = connection()
        client.call = Mock(side_effect=[{'thread': {'status': {'type': 'active', 'activeFlags': []}, 'updatedAt': 500}},
                                        {'data': []}])
        args = argparse.Namespace(runtime='codex', session='t', output={'actions': []})
        with patch.object(q, 'Codex', return_value=client), contextlib.redirect_stdout(io.StringIO()):
            worker.runtime_status(args)
        observation = args.output['actions'][0]
        self.assertEqual((observation['status'], observation['metadata_updated_at'], observation['event_at']), ('unknown', 500, None))
        client.send.assert_not_called()  # read-only: no approval reply

    def test_launch_policy_is_not_widened(self):
        self.assertEqual(codex.codex_access()['sandbox'], 'workspace-write')
        with patch.object(q, 'ROOT', Path.cwd()):
            policy = codex.codex_turn_policy()
        self.assertEqual((policy['approvalPolicy'], policy['sandboxPolicy']['type']), ('never', 'workspaceWrite'))
        flags = worker.CLAUDE_WORKER_TOOLS
        self.assertEqual(flags[flags.index('--permission-mode') + 1], 'dontAsk')
        self.assertFalse(any('bypass' in flag or 'skip-permissions' in flag for flag in flags))

    def test_claude_readback_and_blocked_is_not_idle(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / 'abcdefgh').mkdir()
            (Path(directory) / 'abcdefgh/state.json').write_text(json.dumps({
                'sessionId': 'abcdefgh-1', 'updatedAt': '2026-10-07T10:00:00Z', 'state': 'blocked',
                'respawnFlags': ['--permission-mode', 'dontAsk', '--tools', 'Bash']}))
            for agent, status in (({'sessionId': 'abcdefgh-1', 'pid': 1, 'status': 'idle', 'state': 'blocked'}, 'unknown'),
                                  ({'sessionId': 'abcdefgh-1', 'state': 'done'}, 'terminal')):
                args = argparse.Namespace(runtime='claude', session='abcdefgh-1', output={'actions': []})
                with self.subTest(status=status), patch.object(q, 'CLAUDE_JOBS', Path(directory)), \
                     patch.object(q, 'claude_agents', return_value={'abcdefgh-1': agent}), \
                     patch.object(q, 'claude_url', return_value=None), contextlib.redirect_stdout(io.StringIO()):
                    worker.runtime_status(args)
                    observation = args.output['actions'][0]
                    self.assertEqual(observation['status'], status)
                    self.assertEqual(observation['effective_launch_policy']['permission_mode'], 'dontAsk')
                    self.assertEqual(observation['metadata_updated_at'], '2026-10-07T10:00:00Z')
                    self.assertIsNone(observation['event_at'])
        item = {'claim': {'session': 's', 'runtime': 'claude'}, 'state': 'doing', 'updated': '2000-01-01T00:00:00Z'}
        with patch.object(q, 'age', return_value=500):
            self.assertEqual(tick.liveness(item, {'s': {'pid': 1, 'status': 'idle', 'state': 'blocked'}})[0], 'busy')
            self.assertEqual(tick.liveness(item, {'s': {'pid': 1, 'status': 'idle', 'state': 'done'}})[0], 'idle')


if __name__ == '__main__':
    unittest.main()
