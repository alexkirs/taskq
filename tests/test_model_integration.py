"""Ordinary wiring with disposable persistence/native finite children; no model calls."""
import argparse
import contextlib
import hashlib
import importlib.util
import io
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('model_fixture', pathlib.Path(__file__).with_name('test_single.py'))
f = importlib.util.module_from_spec(spec); spec.loader.exec_module(f)
q = f.taskq


class ModelIntegration(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(); self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name).resolve()
        prior = q.CONFIG, q.BOARD
        self.addCleanup(lambda: self.restore(prior))
        q.CONFIG = {'root': self.root, 'board': 'fixture.py', 'repo': 'fixture/model', 'publish': 'direct',
                    'limits': {'codex': 1}, 'hosts': {}, 'capacity': {'project_caps': {'model:codex': 1},
                    'host_caps': {'model:codex': 1, 'heavy': 1}, 'host_path': str(self.root / 'host.sqlite')}}
        self.board = q.BOARD = f.FakeBoard()
        self.project = q.SQLiteCapacity(self.root / 'project.sqlite', 'project', 'fixture/model', {'model:codex': 1})
        self.board.capacity_provider = lambda caps: self.project
        self.actor = {'runtime': 'codex', 'session': 'fixture-controller', 'name': q.machine()}
        self.identity = dict(self.actor)
        for patcher in (mock.patch.object(q, 'origin', side_effect=lambda: self.identity),
                        mock.patch.object(q, 'session', side_effect=lambda: self.identity),
                        mock.patch.object(q, 'local_limits', return_value={'codex': 1}),
                        mock.patch.object(q, 'dispatch'), mock.patch.object(q, 'GUARD_WAIT', 0)):
            patcher.start(); self.addCleanup(patcher.stop)
        self.raw = {'event_schema': q.EVENT_SCHEMA, 'event_seq': 0, 'events': [], 'scope': ['artifact.txt'],
                    'deps': [], 'claim': {'runtime': 'codex', 'session': None, 'name': q.machine()},
                    'supervisor': self.actor, 'pm': self.actor, 'order': 'run', 'result': None}
        self.board.add('model fixture', q.block('synthetic', self.raw), ['q-doing'])
        self.kind = q.Codex(); self.children = []; self.launches = 0
        self.addCleanup(self.cleanup)

    def restore(self, prior):
        q.CONFIG, q.BOARD = prior

    def cleanup(self):
        for child in self.children:
            if child.poll() is None: child.kill()
            child.communicate(timeout=5)

    def host(self):
        return q.SQLiteCapacity(self.root / 'host.sqlite', 'host', q.machine(), q.CONFIG['capacity']['host_caps'])

    def item(self):
        return q.parse(self.board.get(1))

    def launch(self, *args):
        self.launches += 1
        child = subprocess.Popen([sys.executable, '-c', 'import sys;sys.stdin.read()'], stdin=subprocess.PIPE,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.children.append(child); child.taskq_birth = q.process_identity(child.pid)[1]
        log = self.kind.folder() / 'T1.log'
        q.CONFIG['_model_launch'](child, log)
        with log.open('ab') as stream:
            for event in ({'type': 'thread.started', 'thread_id': 'fixture-native'}, {'type': 'turn.started'}):
                stream.write(json.dumps(event).encode() + b'\n')
        return 'fixture-native'

    def complete(self):
        with (self.kind.folder() / 'T1.log').open('ab') as stream:
            stream.write(b'{"type":"turn.completed"}\n')
        self.children[-1].communicate(timeout=5)
        with q.coordination(): q.model_reconcile(self.item())

    def start(self):
        with q.coordination(), mock.patch.object(self.kind, 'spawn', side_effect=self.launch):
            sid = q.owned_model_turn(self.item(), self.kind, 'worker', 'first question')
            item = self.item()
            q.move(item, 'doing', 'spawn', 'worker '+sid, claim={**item['claim'], 'session': sid}, order=None)
        return sid

    def test_same_worker_two_turns_separate_budget_and_no_delivery_ack(self):
        sid = self.start(); first = self.item()['raw']['model_turns']['worker']
        self.assertEqual(first['phase'], 'bound')
        with q.coordination(), mock.patch.object(self.kind, 'spawn', side_effect=AssertionError('duplicate')):
            self.assertIsNone(q.owned_model_turn(self.item(), self.kind, 'worker', 'first question'))
        self.complete()
        self.assertEqual(self.item()['claim']['session'], sid)
        self.assertEqual(self.host().observe(first['key'])['phase'], 'released')
        with q.coordination():
            q.move(self.item(), 'ask', 'ask', 'choose synthetic value')
            q.cmd_answer(argparse.Namespace(command='answer', n=['1'], text='chosen value'))
        raw = self.item()['raw']; answer = next(e for e in raw['events'] if e['action'] == 'answer')
        self.assertEqual(answer['acks'], [])
        with q.coordination(), mock.patch.object(self.kind, 'send', side_effect=self.launch):
            q.follow(self.item(), self.kind, self.item()['claim'], True)
        current = self.item()['raw']['model_turns']['worker']
        self.assertEqual(current['generation'], 2)
        self.assertEqual(current['runtime']['session'], sid)
        self.assertEqual(next(e for e in self.item()['raw']['events'] if e['id'] == answer['id'])['acks'], [])
        self.complete()
        before = json.dumps(self.board.issues, sort_keys=True)
        with q.coordination():
            q.cmd_answer(argparse.Namespace(command='answer', n=['1'], text='chosen value'))
        self.assertEqual(json.dumps(self.board.issues, sort_keys=True), before)
        self.assertEqual(self.launches, 2)

    def test_launch_binding_failure_is_not_retried(self):
        def unknown(*args): raise RuntimeError('transport failed after launch intent')
        with self.assertRaises(RuntimeError), q.coordination(), mock.patch.object(self.kind, 'spawn', side_effect=unknown):
            q.owned_model_turn(self.item(), self.kind, 'worker', 'first question')
        self.assertEqual(self.item()['raw']['model_turns']['worker']['phase'], 'launching')
        # Independently invoked primitive must refuse; do not remove a retained board guard.
        with mock.patch.object(self.kind, 'spawn', side_effect=AssertionError('duplicate')):
            self.assertIsNone(q.owned_model_turn(self.item(), self.kind, 'worker', 'first question'))

    def test_model_worker_requeue_preserves_unapplied_answer_and_blocks_replacement(self):
        sid = self.start(); self.complete()
        with q.coordination():
            q.move(self.item(), 'ask', 'ask', 'choose')
            q.cmd_answer(argparse.Namespace(command='answer', n=['1'], text='chosen value'))
        with q.coordination(), mock.patch.object(self.kind, 'send', side_effect=self.launch):
            q.follow(self.item(), self.kind, self.item()['claim'], True)
        self.identity = dict(self.item()['claim'])
        with q.coordination():
            q.cmd_requeue(argparse.Namespace(command='requeue', n=1, text='native Git permission denied'))
        self.complete()
        item = self.item()
        self.assertEqual(item['claim']['session'], sid)
        answer = next(e for e in item['raw']['events'] if e['action'] == 'answer')
        self.assertEqual(answer['acks'], [])
        self.assertNotIn('cancelled', answer)
        self.assertEqual(item['raw']['model_recovery']['session'], sid)
        self.assertEqual(q.model_role_state(item, 'worker'), 'unknown')
        self.identity = dict(self.actor)
        with q.coordination(), mock.patch.object(self.kind, 'send', side_effect=AssertionError('retry')):
            q.follow(item, self.kind, item['claim'], True)
            self.assertIsNone(q.owned_model_turn(item, self.kind, 'worker', 'retry', [answer], resume=sid))
            with self.assertRaises(SystemExit):
                q.cmd_requeue(argparse.Namespace(command='requeue', n=1, text='replacement'))
        self.assertEqual(self.launches, 2)
        self.assertEqual(self.item()['claim']['session'], sid)
        before = json.dumps(self.board.issues, sort_keys=True)
        with self.assertRaises(SystemExit), q.coordination():
            q.cmd_move(argparse.Namespace(command='later', n=1, text='defer'))
        self.assertEqual(json.dumps(self.board.issues, sort_keys=True), before)

    def test_unsupervised_model_worker_failure_keeps_claim(self):
        sid = self.start(); self.complete()
        issue = self.board.get(1); raw = q.issue_data(issue)
        raw['supervisor'] = None
        issue['body'] = q.block('synthetic', raw); self.board.issues[1] = issue
        self.identity = dict(self.item()['claim'])
        with q.coordination():
            q.cmd_requeue(argparse.Namespace(command='requeue', n=1, text='permission denied'))
        self.assertEqual(self.item()['claim']['session'], sid)
        self.assertEqual(self.item()['raw']['model_recovery']['session'], sid)

    def test_non_codex_adapter_refused_before_effect(self):
        with self.assertRaises(SystemExit), mock.patch.object(q.Claude, 'spawn', side_effect=AssertionError('spawn')):
            q.owned_model_turn(self.item(), q.Claude(), 'worker', 'synthetic')

    def rejected_fixture(self):
        sid = self.start(); self.complete()
        def supervisor(*args):
            child = subprocess.Popen([sys.executable, '-c', 'import sys;sys.stdin.read()'], stdin=subprocess.PIPE,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            self.children.append(child); child.taskq_birth = q.process_identity(child.pid)[1]
            log = self.kind.folder() / 'S1.log'; q.CONFIG['_model_launch'](child, log)
            log.write_bytes(b'{"type":"thread.started","thread_id":"fixture-supervisor"}\n{"type":"turn.started"}\n')
            return 'fixture-supervisor'
        with q.coordination(), mock.patch.object(self.kind, 'spawn', side_effect=supervisor):
            boss = q.owned_model_turn(self.item(), self.kind, 'supervisor', 'review')
            q.move(self.item(), 'doing', 'spawn', 'supervisor '+boss,
                   supervisor={**self.actor, 'session': boss})
        with (self.kind.folder()/'S1.log').open('ab') as stream:
            stream.write(b'{"type":"turn.completed"}\n')
        self.children[-1].communicate(timeout=5)
        with q.coordination():
            q.model_reconcile(self.item())
            self.identity = dict(self.item()['claim'])
            q.move(self.item(), 'review', 'result', 'submitted', result={'sha': 'a'*40, 'checks': 'fixture'})
            self.identity = dict(self.item()['supervisor'])
            q.move(self.item(), 'ask', 'ask', 'review rejected: correct shared runtime',
                   decision={'summary': 'rejected', 'options': ['correct'], 'recommend': 1})
        self.identity = dict(self.actor)
        item = self.item()
        return argparse.Namespace(command='answer', n=['1'], text='correct approved scope',
                                  command_id='rework-fixture', question_revision=q.question_revision(item),
                                  rework_rejection=item['raw']['event_seq']), sid

    def test_explicit_rejected_result_rework_same_identity_and_idempotent_application(self):
        args, sid = self.rejected_fixture(); old = self.item()['raw']; boss = old['supervisor']
        with q.coordination(): q.cmd_answer(args)
        raw = self.item()['raw']
        self.assertEqual(raw['rejected_results'][0]['result'], old['result'])
        self.assertEqual((raw['claim'], raw['supervisor']), (old['claim'], boss))
        self.assertIsNone(raw['result'])
        before = json.dumps(self.board.issues, sort_keys=True)
        with q.coordination(): q.cmd_answer(args)
        self.assertEqual(json.dumps(self.board.issues, sort_keys=True), before)
        with q.coordination(), mock.patch.object(self.kind, 'send', side_effect=self.launch):
            q.follow(self.item(), self.kind, self.item()['claim'], True)
        turn = self.item()['raw']['model_turns']['worker']
        self.assertEqual(turn['runtime']['session'], sid)
        self.assertEqual(turn['event_ids'], [raw['event_seq']])
        self.assertEqual(self.item()['raw']['events'][-2]['acks'], [])
        git = ['git', '-C', str(self.root)]
        for argv in (['init', '-q'], ['config', 'user.name', 'Fixture'], ['config', 'user.email', 'fixture@example.invalid']):
            subprocess.run(git+argv, check=True, capture_output=True)
        (self.root/'artifact.txt').write_text('corrected')
        subprocess.run(git+['add', 'artifact.txt'], check=True)
        subprocess.run(git+['commit', '-qm', 'corrected fixture'], check=True)
        sha = subprocess.check_output(git+['rev-parse', 'HEAD'], text=True).strip()
        self.identity = dict(self.item()['claim'])
        applied = argparse.Namespace(event=f'1:{turn["event_ids"][0]}', artifact='artifact.txt', sha=sha)
        with q.coordination(): q.cmd_applied(applied)
        before = json.dumps(self.board.issues, sort_keys=True)
        with q.coordination(): q.cmd_applied(applied)
        self.assertEqual(json.dumps(self.board.issues, sort_keys=True), before)
        self.complete()

    def test_rework_refuses_unknown_identity_unsettled_grants_and_accepted_result(self):
        args, _ = self.rejected_fixture(); issue = self.board.get(1); raw = q.issue_data(issue)
        cases = []
        changed = json.loads(json.dumps(raw)); changed['model_turns']['worker']['runtime']['session'] = 'foreign'; cases.append(changed)
        changed = json.loads(json.dumps(raw)); changed['model_turns']['worker']['phase'] = 'bound'; cases.append(changed)
        changed = json.loads(json.dumps(raw)); changed['action_payloads']['close'] = {'id': 10}; cases.append(changed)
        changed = json.loads(json.dumps(raw)); changed['acceptance_receipts'] = {'native': {'status': 'accepted'}}; cases.append(changed)
        changed = json.loads(json.dumps(raw)); changed['events'].append({'id': 10, 'action': 'close', 'text': 'accepted', 'by': 'supervisor', 'recipients': [], 'acks': []}); cases.append(changed)
        changed = json.loads(json.dumps(raw)); changed['events'][-1]['by'] = 'codex:foreign'; cases.append(changed)
        for changed in cases:
            with self.subTest(changed=changed):
                self.board.issues[1]['body'] = q.block('synthetic', changed)
                before = json.dumps(self.board.issues, sort_keys=True)
                with self.assertRaises(SystemExit), q.coordination(): q.cmd_answer(args)
                self.assertEqual(json.dumps(self.board.issues, sort_keys=True), before)
        self.board.issues[1] = issue
        with q.coordination(), mock.patch.object(self.project, 'observe', return_value={'phase': 'reserved'}):
            with self.assertRaises(SystemExit): q.cmd_answer(args)
        with q.coordination(), mock.patch.object(self.kind, 'send', side_effect=AssertionError('unsafe resume')):
            with self.assertRaises(SystemExit): q.owned_model_turn(self.item(), self.kind, 'worker', 'without order', resume=self.item()['claim']['session'])

    def test_larger_provider_cannot_bypass_local_model_limit(self):
        q.CONFIG['capacity']['host_caps']['model:codex'] = 2
        with self.assertRaises(SystemExit), mock.patch.object(self.kind, 'spawn', side_effect=AssertionError('spawn')):
            q.owned_model_turn(self.item(), self.kind, 'worker', 'synthetic')
        self.assertNotIn('model_turns', self.item()['raw'])

    def test_schema1_repair_preserves_events_and_ack(self):
        raw = dict(self.raw, event_schema=1, events=[{'id': 1, 'action': 'answer', 'text': 'value',
            'by': self.actor, 'recipients': ['worker:codex:legacy'], 'acks': []}], event_seq=1, supervisor=None, claim=None)
        issue = self.board.get(1); issue['body'] = q.block('preserve', raw)
        self.board.issues[1] = issue
        converted = q.repair_plan()[0][1]
        self.assertEqual(converted['events'], raw['events'])
        self.assertEqual(converted['event_schema'], 3)

    def test_applied_requires_native_identity_exact_commit_and_repeat_is_no_write(self):
        self.start(); self.complete()
        with q.coordination():
            q.move(self.item(), 'ask', 'ask', 'choose')
            q.cmd_answer(argparse.Namespace(command='answer', n=['1'], text='chosen value'))
        with q.coordination(), mock.patch.object(self.kind, 'send', side_effect=self.launch):
            q.follow(self.item(), self.kind, self.item()['claim'], True)
        event_id = self.item()['raw']['model_turns']['worker']['event_ids'][0]
        git = ['git', '-C', str(self.root)]
        for argv in (['init', '-q'], ['config', 'user.name', 'Fixture'], ['config', 'user.email', 'fixture@example.invalid']):
            subprocess.run(git+argv, check=True, capture_output=True)
        (self.root/'artifact.txt').write_text('chosen value')
        subprocess.run(git+['add', 'artifact.txt'], check=True)
        subprocess.run(git+['commit', '-qm', 'synthetic artifact'], check=True)
        sha = subprocess.check_output(git+['rev-parse', 'HEAD'], text=True).strip()
        args = argparse.Namespace(event=f'1:{event_id}', artifact='artifact.txt', sha=sha)
        with self.assertRaises(SystemExit): q.cmd_applied(args)  # manager is not worker
        self.identity = self.item()['claim']
        issue = self.board.get(1); changed = q.issue_data(issue)
        answer = next(e for e in changed['events'] if e['id'] == event_id)
        answer['text'] = 'changed under same ID'
        self.board.issues[1]['body'] = q.block('synthetic', changed)
        with self.assertRaises(SystemExit): q.cmd_applied(args)
        self.board.issues[1] = issue
        with q.coordination(): q.cmd_applied(args)
        before = json.dumps(self.board.issues, sort_keys=True)
        with q.coordination(): q.cmd_applied(args)
        self.assertEqual(json.dumps(self.board.issues, sort_keys=True), before)
        (self.root/'artifact.txt').write_text('changed')
        with self.assertRaises(SystemExit): q.cmd_applied(args)
        self.complete()


if __name__ == '__main__': unittest.main()
