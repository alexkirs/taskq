"""Model-budget settlement cannot settle heavy resources or application receipts."""
import hashlib
import copy
import importlib.util
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('model_capacity_taskq', pathlib.Path(__file__).resolve().parents[1] / 'taskq.py')
q = importlib.util.module_from_spec(spec)
spec.loader.exec_module(q)


class ModelCapacity(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = pathlib.Path(directory.name).resolve()
        self.host = q.SQLiteCapacity(self.root / 'host.sqlite', 'host', 'fixture', {'model:codex': 1, 'heavy': 1})
        self.log = self.root / 'turn.log'
        self.prefix = b'{"type":"previous.fixture"}\n'
        self.runtime = {'kind': 'codex-model-turn', 'session': 'native-fixture', 'log': str(self.log),
                        'offset': len(self.prefix), 'prefix_sha256': hashlib.sha256(self.prefix).hexdigest()}
        self.entries = [{'type': 'thread.started', 'thread_id': 'native-fixture'}, {'type': 'turn.started'},
                        {'type': 'turn.completed'}]
        self.write_log()
        self.request = {'age': 1, 'priority': 0, 'demand': {'model:codex': 1}}

    def write_log(self):
        self.log.write_bytes(self.prefix + b''.join(json.dumps(entry).encode() + b'\n' for entry in self.entries))

    def bound(self, demand=None):
        request = dict(self.request, demand=demand or self.request['demand'])
        self.assertEqual(self.host.reserve('turn', request)['phase'], 'reserved')
        self.assertTrue(self.host.launch('turn', self.runtime))
        with mock.patch.object(q, 'process_identity', return_value=('running', 'exact-birth')):
            return self.host.bind_model('turn', 101, 'exact-birth', self.runtime)

    def test_model_finishes_heavy_remains_claims_and_events_are_not_involved(self):
        self.host.reserve('heavy', {'age': 1, 'priority': 0, 'demand': {'heavy': 1}})
        self.bound()
        with mock.patch.object(q, 'process_state', return_value='dead'):
            settled = self.host.settle_model('turn', self.runtime)
            self.assertEqual(settled['drain']['terminal'], 'turn.completed')
            self.assertNotIn('application', settled['drain'])
            self.assertEqual(self.host.release('turn')['phase'], 'released')
            self.assertEqual(self.host.settle_model('turn', self.runtime)['phase'], 'released')
        self.assertEqual(self.host.observe('heavy')['phase'], 'reserved')
        self.assertEqual(self.host.reserve('next', dict(self.request, age=2))['phase'], 'reserved')

    def test_mixed_model_heavy_cannot_use_model_binding(self):
        with self.assertRaises(ValueError):
            self.bound({'model:codex': 1, 'heavy': 1})
        self.assertEqual(self.host.observe('turn')['phase'], 'launching')

    def test_wrong_model_dimension_cannot_bind(self):
        other = q.SQLiteCapacity(self.root / 'other.sqlite', 'host', 'fixture', {'model:claude': 1, 'model:codex': 1})
        other.reserve('turn', dict(self.request, demand={'model:claude': 1, 'model:codex': 0}))
        other.launch('turn', self.runtime)
        with mock.patch.object(q, 'process_identity', return_value=('running', 'exact-birth')):
            with self.assertRaises(ValueError):
                other.bind_model('turn', 101, 'exact-birth', self.runtime)
        self.assertEqual(other.observe('turn')['phase'], 'launching')

    def test_unknown_launch_cannot_be_revoked_or_relaunched(self):
        self.host.reserve('turn', self.request)
        self.host.launch('turn', self.runtime)
        with self.assertRaises(ValueError):
            self.host.stop('turn')
        self.assertFalse(self.host.launch('turn', self.runtime))
        self.assertEqual(self.host.observe('turn')['phase'], 'launching')

    def test_live_unknown_and_wrong_birth_hold_slot(self):
        self.bound()
        for state in ('running', 'unknown'):
            with self.subTest(state=state), mock.patch.object(q, 'process_state', return_value=state):
                with self.assertRaises(ValueError):
                    self.host.settle_model('turn', self.runtime)
                with self.assertRaises(ValueError):
                    self.host.release('turn')
        self.assertEqual(self.host.observe('turn')['phase'], 'bound')

    def test_wrong_session_missing_terminal_multiple_turns_and_malformed_hold(self):
        self.bound()
        variants = [
            [{'type': 'thread.started', 'thread_id': 'foreign'}, {'type': 'turn.started'}, {'type': 'turn.completed'}],
            self.entries[:-1], self.entries + self.entries,
            [{'type': 'thread.started', 'thread_id': 'native-fixture'}, {'type': 'turn.completed'}],
            [{'type': 'thread.started', 'thread_id': 'native-fixture'}, {'type': 'item.completed'},
             {'type': 'turn.started'}, {'type': 'turn.completed'}],
        ]
        for entries in variants:
            self.entries = entries
            self.write_log()
            with mock.patch.object(q, 'process_state', return_value='dead'), self.assertRaises(ValueError):
                self.host.settle_model('turn', self.runtime)
        self.log.write_bytes(self.prefix + b'not-json\n')
        with mock.patch.object(q, 'process_state', return_value='dead'), self.assertRaises(ValueError):
            self.host.settle_model('turn', self.runtime)
        self.assertEqual(self.host.observe('turn')['phase'], 'bound')

    def test_failed_turn_releases_budget_but_not_application(self):
        self.entries[-1] = {'type': 'turn.failed'}
        self.write_log()
        self.bound()
        with mock.patch.object(q, 'process_state', return_value='dead'):
            proof = self.host.settle_model('turn', self.runtime)['drain']
            self.assertEqual(proof['terminal'], 'turn.failed')
            self.assertNotIn('application', proof)
            self.host.release('turn')

    def test_changed_log_after_settlement_blocks_release(self):
        self.bound()
        with mock.patch.object(q, 'process_state', return_value='dead'):
            self.host.settle_model('turn', self.runtime)
            self.entries.insert(2, {'type': 'item.completed'})
            self.write_log()
            with self.assertRaises(ValueError):
                self.host.release('turn')
        self.assertEqual(self.host.observe('turn')['phase'], 'drained')

    def test_prefix_rewrite_and_partial_line_refused(self):
        self.bound()
        original = self.log.read_bytes()
        self.prefix = b'x' * len(self.prefix)
        self.write_log()
        with mock.patch.object(q, 'process_state', return_value='dead'), self.assertRaises(ValueError):
            self.host.settle_model('turn', self.runtime)
        self.log.write_bytes(original[:-1])
        with mock.patch.object(q, 'process_state', return_value='dead'), self.assertRaises(ValueError):
            self.host.settle_model('turn', self.runtime)

    def test_real_owned_child_identity_and_terminal_release(self):
        # Finite stdlib fixture: native PID/birth proof, not a real Codex/model session.
        script = 'import sys; sys.stdin.read(); print("done")'
        child = subprocess.Popen([sys.executable, '-c', script], stdin=subprocess.PIPE,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        def cleanup():
            if child.poll() is None:
                child.kill()
            child.communicate(timeout=5)
        self.addCleanup(cleanup)
        state, birth = q.process_identity(child.pid)
        self.assertEqual(state, 'running')
        self.assertIsNotNone(birth)
        self.host.reserve('turn', self.request)
        self.host.launch('turn', self.runtime)
        self.host.bind_model('turn', child.pid, birth, self.runtime)
        with self.assertRaises(ValueError):
            self.host.settle_model('turn', self.runtime)
        child.communicate(timeout=5)
        self.assertEqual(child.returncode, 0)
        self.assertEqual(q.process_state(child.pid, birth), 'dead')
        self.host.settle_model('turn', self.runtime)
        self.assertEqual(self.host.release('turn')['phase'], 'released')

    def test_wrong_native_birth_refuses_binding(self):
        self.host.reserve('turn', self.request)
        self.host.launch('turn', self.runtime)
        with mock.patch.object(q, 'process_identity', return_value=('running', 'different-birth')):
            with self.assertRaises(ValueError):
                self.host.bind_model('turn', 101, 'expected-birth', self.runtime)
        self.assertEqual(self.host.observe('turn')['phase'], 'launching')

    def test_resource_settlement_cannot_bypass_model_receipt(self):
        self.bound()
        with mock.patch.object(q.os, 'getpid', return_value=101), \
                mock.patch.object(q, 'process_identity', return_value=('running', 'exact-birth')):
            with self.assertRaises(ValueError):
                self.host.drained('turn', {'status': 'applied'})
        value = self.host.observe('turn')
        value.update(phase='drained', drain={'kind': 'controlled-artifact'})
        with mock.patch.object(q, 'process_state', return_value='dead'):
            self.assertFalse(self.host.drain_verified(value))


class ClosedModelSettlement(unittest.TestCase):
    """Real local ledger/log + native-task-shaped closed board; no model/network."""
    bound = ModelCapacity.bound
    write_log = ModelCapacity.write_log
    def setUp(self):
        ModelCapacity.setUp(self)
        self.actor = {'runtime': 'codex', 'session': 'manager', 'name': 'fixture'}
        self.request.update(repo='o/r', task=7, role='supervisor', generation=1, owner=self.actor)
        self.project = q.SQLiteCapacity(self.root/'project.sqlite', 'project', 'o/r', {'model:codex': 1})
        self.project.reserve('turn', self.request)
        bound = self.bound()
        self.turn = {'key': 'turn', 'phase': 'bound', 'generation': 1,
                     'request': self.request, 'runtime': self.runtime, 'child': bound['child']}
        self.raw = {'event_schema': q.EVENT_SCHEMA, 'pm': self.actor,
                    'claim': {'session': 'kept-worker'}, 'supervisor': {'session': 'native-fixture'},
                    'result': {'sha': 'saved-result'}, 'events': [{'id': 9, 'acks': ['kept']}],
                    'model_turns': {'supervisor': self.turn}, 'heavy': 'unknown'}
        self.issue = {'iid': 7, 'state': 'closed', 'labels': [], 'body': q.block('human prose', self.raw)}
        self.gets = []; self.writes = []
        self.board = mock.Mock()
        self.board.get.side_effect = self.get
        self.board.metadata.side_effect = self.get
        self.board.update.side_effect = self.update
        for patch in (mock.patch.object(q, 'BOARD', self.board),
                      mock.patch.object(q, 'CONFIG', {'repo': 'o/r'}),
                      mock.patch.object(q, 'origin', return_value=self.actor),
                      mock.patch.object(q, 'model_providers', return_value=(self.project, self.host)),
                      mock.patch.object(q, 'process_state', return_value='dead')):
            patch.start(); self.addCleanup(patch.stop)
    def get(self, n):
        self.gets.append(n); self.assertEqual(n, 7)
        return copy.deepcopy(self.issue)
    def update(self, n, labels=None, body=None):
        self.assertEqual(n, 7); self.writes.append((labels, body))
        if labels is not None:
            self.issue['labels'] = labels
        if body is not None:
            self.issue['body'] = body
    def reconcile(self):
        q.model_reconcile_outstanding()
    def test_closed_before_settlement_ledger_selection_preserves_result_resources(self):
        self.host.reserve('heavy', {'age': 1, 'priority': 0, 'demand': {'heavy': 1}})
        with mock.patch.object(self.board, 'list', side_effect=AssertionError('no history scan')):
            self.reconcile()
        self.assertEqual(self.host.observe('turn')['phase'], 'released')
        self.assertEqual(self.project.observe('turn')['phase'], 'released')
        after = q.issue_data(self.issue)
        self.assertEqual(after['model_turns']['supervisor']['phase'], 'complete')
        self.assertEqual({k:v for k,v in after.items() if k!='model_turns'},
                         {k:v for k,v in self.raw.items() if k!='model_turns'})
        self.assertEqual(self.host.observe('heavy')['phase'], 'reserved')
        self.assertEqual((self.issue['state'], self.issue['labels']), ('closed', []))
        writes = self.writes[:]; self.gets.clear(); self.reconcile()
        self.assertEqual(self.writes, writes); self.assertEqual(self.gets, [])
    def test_missing_log_retains_closed_task_grants(self):
        self.log.unlink(); self.reconcile()
        self.assertEqual(self.host.observe('turn')['phase'], 'bound')
        self.assertEqual(self.project.observe('turn')['phase'], 'reserved')
        self.assertEqual(self.writes, [])
    def test_same_sid_new_active_turn_retains_grants(self):
        self.entries += [{'type': 'thread.started', 'thread_id': 'native-fixture'}, {'type':'turn.started'}]
        self.write_log(); self.reconcile()
        self.assertEqual(self.host.observe('turn')['phase'], 'bound'); self.assertEqual(self.writes, [])
    def test_unknown_birth_live_pid_and_stale_generation_cannot_release(self):
        for state in ('unknown', 'running'):
            with mock.patch.object(q, 'process_state', return_value=state):
                self.reconcile()
            self.assertEqual(self.host.observe('turn')['phase'], 'bound')
        changed = copy.deepcopy(self.raw)
        changed['model_turns']['supervisor']['child']['birth'] = 'foreign/reused-birth'
        self.issue['body'] = q.block('human prose', changed); self.reconcile()
        self.assertEqual(self.host.observe('turn')['phase'], 'bound'); self.assertEqual(self.writes, [])
    def test_foreign_pm_or_held_closed_task_not_settled(self):
        for change in ({'pm': {'session': 'foreign'}}, {'legacy_recovery': {'drain_proven': False}}):
            self.issue['body'] = q.block('human prose', {**self.raw, **change}); self.reconcile()
            self.assertEqual(self.host.observe('turn')['phase'], 'bound')
        self.assertEqual(self.writes, [])
    def test_partial_host_release_reconciles_remaining_project_grant(self):
        self.host.settle_model('turn', self.runtime); self.host.release('turn')
        self.reconcile()
        self.assertEqual(self.project.observe('turn')['phase'], 'released')
        self.assertEqual(q.issue_data(self.issue)['model_turns']['supervisor']['phase'], 'complete')
    def test_ledger_filters_history_foreign_repo_and_resource_rows(self):
        self.assertEqual(set(self.host.outstanding_models('o/r')), {'turn'})
        self.assertEqual(self.host.outstanding_models('foreign'), {})
        self.host.reserve('heavy', {'age': 1, 'priority': 0, 'demand': {'heavy': 1}})
        self.reconcile()
        self.assertEqual(self.host.outstanding_models('o/r'), {})
        self.assertEqual(self.project.outstanding_models(), {})


if __name__ == '__main__':
    unittest.main()
