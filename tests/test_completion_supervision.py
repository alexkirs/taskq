"""#223: completion supervision. The tracker is the transport; the tick correlates what it delivered with what is
still pending (state, claim, revision) and never reads an acknowledgement from a delivery. Run with `discover -s tests`."""
import contextlib
import importlib
import io
import json
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch

import taskq as q
import test_taskq as base
from test_taskq import CLAUDE, CODEX, COORDINATOR, link

codex = importlib.import_module('taskq.codex')
tick = importlib.import_module('taskq.tick')


class Supervision(unittest.TestCase):
    do, refused, add, state = base.Cycle.do, base.Cycle.refused, base.Cycle.add, base.Cycle.state

    def setUp(self):
        base.Cycle.setUp(self)
        q.LOCAL.write_text('[coordinator]\nsession = "coordinator-session"\n')
        self.addCleanup(q.LOCAL.unlink, missing_ok=True)
        self.enterContext(patch.object(tick, 'woken', return_value=self.directory / 'woken'))
        self.enterContext(patch.object(q, 'spawn', lambda args: None))
        self.woken = []
        self.enterContext(patch.object(q, 'claude_wake', lambda session, prompt: self.woken.append((session, prompt))))
        self.agents['coordinator-session'] = {'id': 'coordina', 'sessionId': 'coordinator-session', 'kind': 'background',
                                              'cwd': str(q.ROOT), 'name': 'PM (mac-1)', 'state': 'done'}

    def act(self):
        """One `tick --act --wake`: its printed output; the wake list and report actions tell what it delivered."""
        self.report = []
        with contextlib.redirect_stdout(io.StringIO()) as out, contextlib.redirect_stderr(io.StringIO()), \
                patch.dict(os.environ, COORDINATOR), patch.object(q, 'record', lambda args, action, **values: self.report.append({'action': action, **values})):
            with contextlib.suppress(SystemExit):
                q.main(['tick', '--act', '--wake'])
        return out.getvalue()

    def wakes(self):
        return [event for event in self.report if event['action'] == 'wake']

    def test_resubmitted_result_and_second_question_are_new_pending_sets(self):
        code = self.add('--type', 'code', '--runtime', 'claude')
        self.do(CLAUDE, 'take', code)
        self.do(CLAUDE, 'result', code, '--sha', 'abc1234', '--text', 'first', '--checks', 'one')
        self.act()
        self.assertEqual(len(self.woken), 1)
        self.assertIn('already woken', self.act())
        self.assertEqual(self.wakes()[-1]['status'], 'already delivered')
        self.assertEqual(len(self.woken), 1)
        # The same SHA with new checks after a reject in the worker's session: a new result note, a new revision.
        self.do(CLAUDE, 'reject', code, '--text', 'more checks')
        self.do(CLAUDE, 'result', code, '--sha', 'abc1234', '--text', 'second', '--checks', 'two')
        self.act()
        self.assertEqual(len(self.woken), 2)
        self.assertTrue(self.wakes()[-1]['pending'][0].startswith(f'review {code} claude:claude-session result '))
        self.assertEqual(self.wakes()[-1]['acknowledged'], 'unknown')
        # A research result has no SHA: its judgement line is the same `review N None` each time, its revision is not.
        research = self.add('--type', 'research', '--runtime', 'codex')
        self.do(CODEX, 'take', research)
        self.do(CODEX, 'result', research, '--text', 'found', '--checks', 'read')
        self.act()
        self.assertEqual(len(self.woken), 3)
        self.do(CODEX, 'reject', research, '--text', 'look again')
        self.do(CODEX, 'result', research, '--text', 'found more', '--checks', 'read')
        self.act()
        self.assertEqual(len(self.woken), 4)
        # A second question after an in-session answer: the ask line `ask N` repeats, the ask note does not.
        asked = self.add('--type', 'asset', '--runtime', 'claude')
        self.do(CLAUDE, 'take', asked)
        self.do(CLAUDE, 'ask', asked, '--text', 'A or B?')
        self.act()
        self.assertEqual(len(self.woken), 5)
        self.do(CLAUDE, 'answer', asked, '--text', 'A')
        self.do(CLAUDE, 'ask', asked, '--text', 'C or D?')
        self.act()
        self.assertEqual(len(self.woken), 6)

    def test_every_open_question_is_pending_even_when_the_summary_hides_it(self):
        code = self.add('--type', 'code', '--runtime', 'claude')
        self.do(CLAUDE, 'take', code)
        self.do(CLAUDE, 'result', code, '--sha', 'abc1234', '--text', 'x', '--checks', 'x')
        asked = self.add('--type', 'asset', '--runtime', 'claude')
        self.do(CLAUDE, 'take', asked)
        self.do(CLAUDE, 'ask', asked, '--text', 'A or B?')
        self.act()
        self.assertEqual(len(self.woken), 1)
        self.assertEqual([line.split()[0] for line in self.wakes()[-1]['pending']], ['review', 'ask'])
        # Shown: the question leaves the judgement lines for a day, not the pending set; the set is the same, no wake.
        self.assertIn('already woken', self.act())
        self.assertEqual(len(self.woken), 1)
        self.assertEqual([line.split()[0] for line in self.wakes()[-1]['pending']], ['review', 'ask'])
        # The owner answers through the board: the question is resolved, the set is new although the lines are the same.
        self.do(COORDINATOR, 'answer', asked, '--text', 'A')
        self.assertEqual(self.state(asked), 'ready')
        self.act()
        self.assertEqual(len(self.woken), 2)
        self.assertEqual([line.split()[0] for line in self.wakes()[-1]['pending']], ['review'])

    def test_shown_question_alone_is_re_delivered_after_tick_live_minutes(self):
        """P1 of the independent review: an ask-only queue has no judgement line once shown; the pending set still
        reaches the coordinator again, bounded by TICK_LIVE_MINUTES, with the question in the turn."""
        asked = self.add('--type', 'asset', '--runtime', 'claude')
        self.do(CLAUDE, 'take', asked)
        self.do(CLAUDE, 'ask', asked, '--text', 'A or B?')
        self.assertIn('New questions', self.act())
        self.assertEqual(len(self.woken), 1)
        output = self.act()  # shown: no judgement line, nothing to do, the set unchanged
        self.assertIn('Nothing to do', output)
        self.assertEqual((len(self.woken), self.wakes()[-1]['status']), (1, 'already delivered'))
        key = tick.woken().read_text().split()[0]
        tick.woken().write_text(f'{key}\n{time.time() - tick.TICK_LIVE_MINUTES * 60 - 1:.0f}\n')
        output = self.act()
        self.assertEqual((len(self.woken), self.wakes()[-1]['status'], self.state(asked)), (2, 'delivered again', 'ask'))
        self.assertTrue(self.wakes()[-1]['pending'][0].startswith(f'ask {asked} claude:claude-session ask '))
        self.assertIn('## Pending, acknowledgement unknown', self.woken[-1][1])
        self.assertIn('A or B?', self.woken[-1][1])
        self.assertIn('already woken', self.act())
        self.assertEqual(len(self.woken), 2)

    def test_hand_in_text_and_revision_come_from_one_read_and_a_later_note_stops_the_send(self):
        """P1 of the independent review: a trusted result note landing after the read (same block) must not be sent
        as the old text under the new revision; the fresh compare before the send sees the later note."""
        code = self.add('--type', 'code', '--runtime', 'claude')
        self.do(CLAUDE, 'take', code)
        self.do(CLAUDE, 'result', code, '--sha', 'abc1234', '--text', 'OLD PAYLOAD', '--checks', 'same')
        comments, landed = q.comments, []

        def racing(iid, everyone=False):
            found = comments(iid, everyone)
            if not landed:  # the note lands right after the tick's one read of this item
                landed.append(self.gitlab('POST', f'issues/{iid}/notes', {'body': '**result** · claude:claude-s\n\n`abc1234`\n\nNEW PAYLOAD\n\nChecks: same'})['id'])
            return found
        with patch.object(q, 'comments', racing):
            output = self.act()
        self.assertIn('OLD PAYLOAD', output)
        self.assertNotIn('NEW PAYLOAD', output)  # one read: the text and the line are the old note's
        self.assertIn(f'result {int(landed[0]) - 1} ', self.wakes()[-1]['pending'][0])
        self.assertEqual((self.woken, self.wakes()[-1]['status']), ([], 'pending changed'))
        self.assertIn('changed since this tick read it', output)
        output = self.act()
        self.assertEqual(len(self.woken), 1)
        self.assertIn('NEW PAYLOAD', self.woken[0][1])
        self.assertNotIn('OLD PAYLOAD', self.woken[0][1])
        self.assertIn(f'result {landed[0]} ', self.wakes()[-1]['pending'][0])
        # A PUT with the same SHA and new checks and no note, landing after the read: the fresh block stops the send too.
        key = tick.woken().read_text().split()[0]
        tick.woken().write_text(f'{key}\n{time.time() - tick.TICK_LIVE_MINUTES * 60 - 1:.0f}\n')
        still, written = tick.still_pending, []

        def racing_block(item, line):
            if not written:
                written.append(q.save(q.task(item['iid']), result={'sha': 'abc1234', 'checks': 'new'}))
            return still(item, line)
        with patch.object(tick, 'still_pending', racing_block):
            output = self.act()
        self.assertEqual((len(self.woken), self.wakes()[-1]['status']), (1, 'pending changed'))
        self.assertIn('"checks": "same"', self.wakes()[-1]['pending'][0])
        self.act()
        self.assertEqual(len(self.woken), 2)
        self.assertIn('"checks": "new"', self.wakes()[-1]['pending'][0])
        self.assertNotIn('applied', json.dumps(self.report))

    def test_question_text_and_revision_come_from_one_read_in_fresh_and_summary_paths(self):
        """Residual P1 of the independent review: a trusted ask note landing after the question read (the second
        release in a row writes one on an ask task) must not be sent as the old text under the new revision, in the
        fresh path and in the daily summary path alike."""
        asked = self.add('--type', 'asset', '--runtime', 'claude')
        self.do(CLAUDE, 'take', asked)
        self.do(CLAUDE, 'ask', asked, '--text', 'OLD QUESTION')
        question, landed = tick.question, []

        def racing(iid):
            found = question(iid)
            landed.append(self.gitlab('POST', f'issues/{iid}/notes', {'body': '**ask** · claude:claude-s\n\nNEW QUESTION'})['id'])
            return found
        with patch.object(tick, 'question', racing):
            output = self.act()  # fresh path
        self.assertIn('OLD QUESTION', output)
        self.assertNotIn('NEW QUESTION', output)
        self.assertIn(f' ask {landed[0] - 1} ', self.wakes()[-1]['pending'][0])
        self.assertEqual((self.woken, self.wakes()[-1]['status']), ([], 'pending changed'))
        output = self.act()
        self.assertEqual(len(self.woken), 1)
        self.assertIn('NEW QUESTION', self.woken[0][1])
        self.assertNotIn('OLD QUESTION', self.woken[0][1])
        self.assertIn(f' ask {landed[0]} ', self.wakes()[-1]['pending'][0])
        # Daily summary path: the shown note is a day old; the next ask note lands after the summary's read.
        for note in self.gitlab.notes.values():
            if note['body'].startswith('**shown**'):
                note['created_at'] = '2000-01-01T00:00:00.000Z'
        key = tick.woken().read_text().split()[0]
        tick.woken().write_text(f'{key}\n{time.time() - tick.TICK_LIVE_MINUTES * 60 - 1:.0f}\n')  # the re-wake is due
        with patch.object(tick, 'question', racing):
            output = self.act()
        self.assertIn('daily summary', output)
        self.assertIn('NEW QUESTION', output)
        self.assertNotIn('NEWER', output)
        self.assertIn(f' ask {landed[0]} ', self.wakes()[-1]['pending'][0])
        self.assertEqual((len(self.woken), self.wakes()[-1]['status']), (1, 'pending changed'))
        self.gitlab.notes[landed[1]]['body'] = '**ask** · claude:claude-s\n\nNEWER QUESTION'
        output = self.act()  # the newest note is newer than every shown: fresh again, delivered under its own id
        self.assertEqual(len(self.woken), 2)
        self.assertIn('NEWER QUESTION', self.woken[-1][1])
        self.assertIn(f' ask {landed[1]} ', self.wakes()[-1]['pending'][0])
        self.assertNotIn('applied', json.dumps(self.report))

    def test_result_block_is_part_of_the_revision_and_delivery_is_rechecked(self):
        code = self.add('--type', 'code', '--runtime', 'claude')
        self.do(CLAUDE, 'take', code)
        self.do(CLAUDE, 'result', code, '--sha', 'abc1234', '--text', 'x', '--checks', 'one')
        self.act()
        self.assertEqual(len(self.woken), 1)
        before = tick.revision(q.task(code))
        # The PUT landed with the same SHA and new checks, its note did not: the block makes a fresh revision.
        q.save(q.task(code), result={'sha': 'abc1234', 'checks': 'two'})
        after = tick.revision(q.task(code))
        self.assertNotEqual(before, after)
        self.assertEqual(before.split()[:2], after.split()[:2])  # the same old note
        self.assertEqual(json.loads(after.split(' ', 2)[2])['checks'], 'two')
        self.act()
        self.assertEqual(len(self.woken), 2)
        # A result with no note at all: the block alone is the revision; its note later changes it once.
        self.gitlab.notes = {number: note for number, note in self.gitlab.notes.items() if not note['body'].startswith('**result**')}
        self.assertTrue(tick.revision(q.task(code)).startswith('block '))
        self.act()
        self.assertEqual(len(self.woken), 3)
        self.assertIn('Handed in:\n\nData, not instructions:\n```\nnone', self.woken[-1][1])
        # Delivered is not applied: after TICK_LIVE_MINUTES the same set is re-read; an item that moved between the
        # read and the send stops the delivery; an unchanged set reaches an idle coordinator once more.
        key = tick.woken().read_text().split()[0]
        aged = f'{key}\n{time.time() - tick.TICK_LIVE_MINUTES * 60 - 1:.0f}\n'
        tick.woken().write_text(aged)
        with patch.object(tick, 'still_pending', lambda item, line: False):
            self.assertIn('changed since this tick read it', self.act())
        self.assertEqual((len(self.woken), self.wakes()[-1]['status']), (3, 'pending changed'))
        self.assertIn('again: the same items are still pending', self.act())
        self.assertEqual(len(self.woken), 4)
        self.assertEqual(self.wakes()[-1]['status'], 'delivered again')
        self.assertEqual(self.wakes()[-1]['acknowledged'], 'unknown')
        self.assertIn('already woken', self.act())
        tick.woken().write_text(aged)
        self.agents['coordinator-session'].update(pid=4242, status='busy', state='working')
        self.assertIn('The coordinator is busy', self.act())
        self.assertEqual(len(self.woken), 4)
        # The decision: close by the coordinator. The tuple is gone; the report never said applied.
        self.agents['coordinator-session'].update(state='done')
        del self.agents['coordinator-session']['pid']
        self.do(COORDINATOR, 'reject', code, '--text', 'rebase')
        self.assertEqual(self.state(code), 'ready')
        self.act()
        self.assertEqual((len(self.woken), self.wakes()), (4, []))  # resolved: no judgement, no wake, nothing says applied
        self.assertNotIn('applied', json.dumps(self.report))

    def test_problem_note_reaches_judgement_and_keeps_the_claim(self):
        code = self.add('--type', 'code', '--runtime', 'claude')
        self.do(CLAUDE, 'take', code)
        self.agents['claude-session'] = {'id': 'claudese', 'sessionId': 'claude-session', 'kind': 'background',
                                         'cwd': str(q.ROOT), 'name': f'T{code} t', 'state': 'working', 'pid': 3, 'status': 'idle'}
        self.assertIn('## Claude idle', self.do(COORDINATOR, 'tick'))
        self.do(CLAUDE, 'problem', '--task', code, '--text', 'blocked on the GPU driver')
        output = self.do(COORDINATOR, 'tick')
        self.assertIn('## Worker problems', output)
        self.assertIn('blocked on the GPU driver', output)
        self.assertNotIn('## Claude idle', output)
        self.assertNotIn('Released', output)
        output = self.act()
        self.assertEqual([prompt for session, prompt in self.woken if prompt == tick.NUDGE], [])  # no nudge
        self.assertEqual(len(self.woken), 1)  # judgement: the coordinator
        self.assertIn('## Worker problems', self.woken[0][1])
        note = next(number for number, note in self.gitlab.notes.items() if note['body'].startswith('**problem**'))
        self.assertTrue(self.wakes()[-1]['pending'][0].startswith(f'doing {code} claude:claude-session problem {note} '))
        self.assertEqual((self.state(code), q.task(code)['claim']['session']), ('doing', 'claude-session'))
        # answer is not accepted on doing; the worker's own ask ends the problem listing; a busy worker is never stuck.
        self.assertIn('is doing, not ask or later', self.refused(COORDINATOR, 'answer', code, '--text', 'x'))
        self.agents['claude-session']['status'] = 'busy'
        self.assertNotIn('## Worker problems', self.do(COORDINATOR, 'tick'))
        self.agents['claude-session']['status'] = 'idle'
        self.do(CLAUDE, 'ask', code, '--text', 'which driver?')
        output = self.do(COORDINATOR, 'tick')
        self.assertNotIn('## Worker problems', output)
        self.assertIn('which driver?', output)

    def test_codex_observation_names_both_sources_and_their_conflict(self):
        client = type('Client', (), {'permission_requests': {}})()
        running = [{'item': {'type': 'commandExecution', 'status': 'inProgress'}}]
        turn = {'status': 'inProgress', 'app': True, 'entries': running, 'sources': {'app': 'notLoaded/interrupted', 'rollout': 'running'}}
        found = codex.codex_observation(client, 't', {'type': 'notLoaded'}, [turn], 12)
        self.assertEqual((found['status'], found['conflict'], found['source']), ('active', True, 'rollout tail over app metadata'))
        self.assertEqual(found['sources'], {'app': 'notLoaded/interrupted', 'rollout': 'running'})
        plain = codex.codex_observation(client, 't', {'type': 'active'}, [{'status': 'inProgress', 'entries': running}], 12)
        self.assertEqual((plain['status'], plain['conflict'], plain['source']), ('active', False, 'codex app-server'))
        self.assertEqual(plain['sources'], {'app': 'active/inProgress', 'rollout': 'not consulted'})
        # The snapshot reads the tail itself: running, ended, unread; the app server's own status is never replaced.
        with tempfile.TemporaryDirectory() as directory:
            rollout = Path(directory) / 'rollout.jsonl'
            rollout.write_text('{"type":"response_item","payload":{}}\n')
            calls = {'thread/read': {'thread': {'status': {'type': 'notLoaded'}, 'path': str(rollout)}},
                     'thread/turns/list': {'data': [{'id': 'u1', 'status': 'interrupted'}]},
                     'thread/items/list': {'data': []}}
            client.call = lambda method, params: json.loads(json.dumps(calls[method]))
            status, turns, _ = codex.codex_snapshot(client, 't', 1)
            self.assertEqual((status['type'], turns[0]['status'], turns[0]['sources']), ('notLoaded', 'inProgress', {'app': 'notLoaded/interrupted', 'rollout': 'running'}))
            rollout.write_text('{"type":"response_item","payload":{}}\n{"type":"task_complete","turn_id":"u1"}\n')
            status, turns, _ = codex.codex_snapshot(client, 't', 1)
            self.assertEqual((turns[0]['status'], turns[0]['sources']['rollout'], turns[0].get('app')), ('interrupted', 'ended', None))
            calls['thread/read']['thread']['path'] = None
            status, turns, _ = codex.codex_snapshot(client, 't', 1)
            self.assertEqual((turns[0]['status'], turns[0]['sources']['rollout']), ('interrupted', 'unread'))
            calls['thread/read']['thread']['status']['type'] = 'active'
            self.assertEqual(codex.codex_snapshot(client, 't', 1)[1][0]['sources'], {'app': 'active/interrupted', 'rollout': 'not consulted'})

    def test_board_handin_while_not_loaded_and_decisions_out_of_order(self):
        asset = self.add('--type', 'asset')
        self.do(CODEX, 'take', asset)
        self.codex.status = 'notLoaded'
        self.codex.turns = [{'id': 'u1', 'status': 'interrupted'}]
        self.do(CODEX, 'result', asset, '--text', 'rendered', '--checks', 'looked')
        sent = []
        with patch.object(q, 'codex_send', lambda args: sent.append(args.thread)):
            output = self.act()
        self.assertIn(f'## Review {link(asset)}', self.woken[-1][1])
        self.assertEqual((sent, self.state(asset)), ([], 'review'))
        self.assertNotIn('Released', output)
        # Out of order: a reject after the close, a second answer, an answer on a doing task. The state refuses each.
        self.assertIn('is review, not ask or later', self.refused(COORDINATOR, 'answer', asset, '--text', 'x'))
        with patch.object(q, 'retire_local', lambda current: None):
            self.do(COORDINATOR, 'close', asset, '--text', 'ok')
        self.assertIn('is not an open taskq task', self.refused(COORDINATOR, 'reject', asset, '--text', 'late'))
        asked = self.add('--type', 'asset', '--runtime', 'claude')
        self.do(CLAUDE, 'take', asked)
        self.do(CLAUDE, 'ask', asked, '--text', 'A or B?')
        self.do(COORDINATOR, 'answer', asked, '--text', 'A')
        self.assertEqual(self.state(asked), 'ready')
        self.assertIn('is ready, not ask or later', self.refused(COORDINATOR, 'answer', asked, '--text', 'A again'))
        self.assertIn('**answer** · claude:coordina', self.do(CLAUDE, 'worker'))  # the next worker's brief holds the decision


if __name__ == '__main__':
    unittest.main()
