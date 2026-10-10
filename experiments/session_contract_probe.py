"""Disposable memory-only failure model; does not import or invoke production TaskQ."""
import copy
import os
import unittest
from dataclasses import dataclass, field

FAULT = os.environ.get('PROBE_MUTATION', '')


@dataclass
class Fixture:
    guard: bool = True
    writer: bool = False
    stopped: bool = True
    authorized: bool = True
    explicit_stop: bool = False
    question: int = 7
    ops: dict = field(default_factory=dict)
    effects: list = field(default_factory=list)
    applied: set = field(default_factory=set)
    acks: set = field(default_factory=set)
    results: dict = field(default_factory=dict)
    waits: set = field(default_factory=set)
    artifact: str = 'candidate-sha'

    def act(self, op, crash=None, replace=False, question=None):
        if not self.guard and FAULT != 'guard':
            return 'blocked:guard'
        if not self.authorized or self.explicit_stop:
            return 'blocked:authority'
        if question is not None and question != self.question and FAULT != 'question':
            return 'blocked:stale-question'
        if replace and (not self.stopped or self.writer) and FAULT != 'writer':
            return 'blocked:predecessor'
        if op in self.ops and FAULT != 'retry':
            return self.ops[op]
        self.ops[op] = 'reserved'
        if crash == 'reservation':
            self.ops[op] = 'unknown'
            return 'unknown'
        self.effects.append(op)
        self.ops[op] = 'admitted'
        if FAULT == 'receipt':
            self.applied.add(op)
        if crash == 'effect':
            self.ops[op] = 'unknown'
            return 'unknown'
        return 'admitted'

    def reconcile(self, op, exact_receipt=False):
        if not exact_receipt:
            return 'unknown'
        if op not in self.effects:
            return 'blocked:no-effect-evidence'
        self.applied.add(op)
        self.ops[op] = 'applied'
        return 'applied'

    def submit(self, op, sha):
        if op not in self.applied or not self.guard:
            return 'blocked:result'
        self.results[op] = sha
        return 'result-recorded'

    def ack(self, event, received=False):
        if not self.guard:
            return 'blocked:guard'
        if not received:
            return 'blocked:no-receipt'
        self.acks.add(event)
        return 'acknowledged'

    def wait(self, project, absent_verified=False):
        if self.explicit_stop:
            return 'blocked:owner-stop'
        if project in self.waits:
            return 'existing-wait'
        if not absent_verified:
            return 'blocked:unknown-old-wait'
        self.waits.add(project)
        return 'running-wait'


INCIDENTS = {
    'unreadable-controller': ('unknown', 'platform', 'addressed-stop/drain'),
    'active-writer': ('blocked', 'owning-runtime', 'targeted-unload/readback'),
    'surface-auth': ('blocked', 'owning-runtime', 'compare-effective-context'),
    'after-hook': ('unknown', 'runtime', 'verify-after-pass-receipt'),
    'pong-timeout': ('unknown', 'ARM', 'reconcile-delivery-and-wait'),
    'missing-order': ('blocked', 'supervisor', 'decide-native-order'),
    'unsubmitted-result': ('blocked', 'worker', 'submit-preserved-candidate'),
    'ack-contention': ('blocked', 'ack-actor', 'reconcile-exact-event-receipt'),
    'stale-question': ('blocked', 'PM', 'refresh-question-version'),
    'native-windows-conflict': ('blocked', 'supervisor', 'diagnose-native-tooling'),
}


class FailureExperiments(unittest.TestCase):
    def test_guard_orphan_deadline_never_authorizes_effect(self):
        f = Fixture(guard=False)
        for _ in range(8):
            self.assertEqual(f.act('resume'), 'blocked:guard')
        self.assertFalse(f.effects)

    def test_authorized_replacement_requires_app_writer_release(self):
        f = Fixture(writer=True)
        self.assertEqual(f.act('replace', replace=True), 'blocked:predecessor')
        self.assertEqual(f.artifact, 'candidate-sha')
        self.assertFalse(f.effects)
        f.writer = False
        self.assertEqual(f.act('replace', replace=True), 'admitted')

    def test_each_crash_boundary_preserves_bound_through_restart(self):
        for crash in ('reservation', 'effect'):
            with self.subTest(crash=crash):
                f = Fixture()
                self.assertEqual(f.act('handoff:349:8', crash), 'unknown')
                restored = copy.deepcopy(f)
                for turn in range(4):
                    self.assertEqual(restored.act('handoff:349:8'), 'unknown')
                self.assertLessEqual(len(restored.effects), 1)
                self.assertEqual(restored.artifact, f.artifact)

    def test_submission_is_not_application_or_result(self):
        f = Fixture()
        self.assertEqual(f.act('submit'), 'admitted')
        self.assertNotIn('submit', f.applied)
        self.assertEqual(f.submit('submit', f.artifact), 'blocked:result')

    def test_exact_reconciliation_recovers_without_second_effect(self):
        f = Fixture()
        f.act('resume', 'effect')
        self.assertEqual(f.reconcile('resume'), 'unknown')
        self.assertEqual(f.reconcile('resume', True), 'applied')
        self.assertEqual(f.submit('resume', f.artifact), 'result-recorded')
        self.assertEqual(f.act('resume'), 'applied')
        self.assertEqual(f.effects, ['resume'])

    def test_stale_question_cannot_authorize_new_action(self):
        f = Fixture(question=9)
        self.assertEqual(f.act('answer', question=7), 'blocked:stale-question')
        self.assertFalse(f.effects)

    def test_competing_ack_known_refusal_preserves_event(self):
        f = Fixture(guard=False)
        self.assertEqual(f.ack('352:9', True), 'blocked:guard')
        self.assertFalse(f.acks)
        f.guard = True
        self.assertEqual(f.ack('352:9'), 'blocked:no-receipt')
        self.assertEqual(f.ack('352:9', True), 'acknowledged')
        f.ack('352:9', True)
        self.assertEqual(f.acks, {'352:9'})

    def test_interrupted_wait_needs_absence_proof_not_new_sender(self):
        f = Fixture()
        self.assertEqual(f.wait('consumer-project'), 'blocked:unknown-old-wait')
        self.assertEqual(f.wait('consumer-project', True), 'running-wait')
        self.assertEqual(f.wait('consumer-project', True), 'existing-wait')
        self.assertEqual(f.waits, {'consumer-project'})

    def test_explicit_stop_blocks_but_generic_abort_preserves_authority(self):
        f = Fixture(explicit_stop=True)
        self.assertEqual(f.act('resume'), 'blocked:authority')
        self.assertEqual(f.wait('consumer-project', True), 'blocked:owner-stop')
        g = Fixture()  # generic abort: unknown attempt, no invented explicit stop
        g.act('resume', 'reservation')
        self.assertTrue(g.authorized)
        self.assertEqual(g.act('resume'), 'unknown')

    def test_all_incidents_surface_responsible_actor_and_next_action(self):
        self.assertEqual(len(INCIDENTS), 10)
        for name, (state, actor, action) in INCIDENTS.items():
            with self.subTest(name=name):
                self.assertIn(state, ('blocked', 'unknown'))
                self.assertTrue(actor and action)
                self.assertNotEqual(action, 'success')

    def test_bounded_end_to_end_repeated_and_interrupted_flow(self):
        f = Fixture()
        trace = ['idle', f.wait('taskq', True), 'event-received', 'tick',
                 INCIDENTS['unsubmitted-result'][2]]
        trace.append(f.act('worker:obligation:result', 'effect'))
        self.assertEqual(f.act('worker:obligation:result'), 'unknown')
        trace.append(f.reconcile('worker:obligation:result', True))
        trace.append(f.submit('worker:obligation:result', f.artifact))
        trace.append(f.ack('result:1', True))
        f.waits.remove('taskq')  # disposable wait observed completed
        trace.append(f.wait('taskq', True))
        self.assertEqual(trace[-4:], ['applied', 'result-recorded',
                                     'acknowledged', 'running-wait'])
        self.assertEqual(len(f.effects), 1)
        self.assertEqual(f.results, {'worker:obligation:result': 'candidate-sha'})


if __name__ == '__main__':
    unittest.main(verbosity=2)
