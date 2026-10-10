"""Model-budget settlement cannot settle heavy resources or application receipts."""
import hashlib
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


if __name__ == '__main__':
    unittest.main()
