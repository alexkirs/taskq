"""Pure legacy preflight: no process or provider work, no permission inference."""
import copy
import unittest
import taskq as q


class LegacyReconcile(unittest.TestCase):
    def setUp(self):
        self.raw = {'event_schema': 1, 'claim': {'runtime': 'codex', 'session': 'synthetic', 'name': 'host'},
                    'result': {'text': 'unsurrendered'}, 'answer': 'pending', 'receipts': [{'id': 1}]}
        self.issue = {'iid': 1, 'body': q.block('human prose', self.raw)}

    def test_unavailable_and_replay_preserve_all_bytes(self):
        original = copy.deepcopy(self.issue)
        first = q.legacy_reconcile_plan(self.issue, 'host')
        self.assertEqual(first, q.legacy_reconcile_plan(self.issue, 'host'))
        self.assertEqual(first['state'], 'blocked')
        self.assertFalse(first['apply_supported'])
        self.assertEqual(original, self.issue)

    def test_ambiguous_active_completed_and_dead_root_refuse(self):
        for evidence in (None, {}, {'turn_completed': True, 'pid_dead': True},
                         {'writers': False, 'inflight': False, 'resources': False, 'addressed_drain': True}):
            with self.subTest(evidence=evidence):
                plan = q.legacy_reconcile_plan(self.issue, 'host', {'codex': lambda expected: evidence})
                self.assertEqual(plan['state'], 'blocked')
        for field in ('writers', 'inflight', 'resources'):
            def observe(expected):
                return {**expected, 'addressed_drain': True, 'writers': False, 'inflight': False, 'resources': False, field: True}
            self.assertEqual(q.legacy_reconcile_plan(self.issue, 'host', {'codex': observe})['state'], 'blocked')

    def test_exact_fixture_observation_is_not_apply_authority(self):
        def observe(expected):
            return {**expected, 'addressed_drain': True, 'writers': False, 'inflight': False, 'resources': False}
        plan = q.legacy_reconcile_plan(self.issue, 'host', {'codex': observe})
        self.assertEqual(plan['owners'][0]['state'], 'observed')
        self.assertFalse(plan['apply_supported'])
        self.assertEqual(q.legacy_reconcile_plan(self.issue, 'other', {'codex': observe})['state'], 'blocked')

    def test_stale_payload_receipt_refused(self):
        old = q.legacy_reconcile_plan(self.issue, 'host')['source_sha256']
        self.raw['answer'] = 'new pending answer'
        self.issue['body'] = q.block('human prose', self.raw)
        def observe(expected):
            return {**expected, 'source_sha256': old, 'addressed_drain': True, 'writers': False, 'inflight': False, 'resources': False}
        self.assertEqual(q.legacy_reconcile_plan(self.issue, 'host', {'codex': observe})['state'], 'blocked')

    def test_malformed_identity_and_observer_failure_refuse(self):
        self.raw['claim'] = {'runtime': 'codex', 'session': 'synthetic'}
        self.issue['body'] = q.block('human prose', self.raw)
        self.assertEqual(q.legacy_reconcile_plan(self.issue, 'host')['state'], 'blocked')
        self.raw['claim']['name'] = 'host'
        self.issue['body'] = q.block('human prose', self.raw)
        def broken(expected):
            raise RuntimeError('unavailable')
        self.assertEqual(q.legacy_reconcile_plan(self.issue, 'host', {'codex': broken})['state'], 'blocked')

    def test_changed_human_acceptance_invalidates_digest(self):
        before = q.legacy_reconcile_plan(self.issue, 'host')['source_sha256']
        self.issue['body'] = q.block('changed acceptance', self.raw)
        self.assertNotEqual(before, q.legacy_reconcile_plan(self.issue, 'host')['source_sha256'])
