"""#249 (audit F2): `take` is decided by the order of `take` notes, with no tracker lock and no launch reservation;
a supervisor launches its worker with `worker --task N`. In-memory tracker, mocked runtimes: no real session."""
import json
import unittest
from unittest.mock import patch

import test_taskq as base  # the in-memory GitLab cycle; run with `discover -s tests`
from test_taskq import CLAUDE, CODEX, COORDINATOR, q, worker

OTHER = {'CLAUDE_CODE_SESSION_ID': 'other-machine', 'CODEX_THREAD_ID': ''}


class Take(unittest.TestCase):
    setUp, do, refused, add, state, race = base.Cycle.setUp, base.Cycle.do, base.Cycle.refused, base.Cycle.add, base.Cycle.state, base.Cycle.race

    def block(self, iid):
        return json.loads(q.BLOCK.search(self.gitlab.issues[iid]['description'])[1])

    def takes(self, iid):
        return [body for body in self.gitlab.said(iid) if body.startswith('**take**')]

    def test_parallel_takes_one_wins_and_the_loser_leaves_no_note(self):
        iid = self.add('--type', 'research')
        self.assertEqual(self.race((CLAUDE, iid), (OTHER, iid)), ['claude-session'])
        self.assertEqual((self.block(iid)['claim']['session'], len(self.takes(iid))), ('claude-session', 1))
        self.assertNotIn('reservation', self.block(iid))

    def test_a_take_note_without_a_move_is_void_after_take_seconds(self):
        iid = self.add('--type', 'research')
        q.note(iid, 'take')  # a take that died between its note and its move
        self.assertIn('another worker took it first', self.refused(CLAUDE, 'take', iid))
        with patch.object(q, 'TAKE_SECONDS', 0):  # the dead note is older than the window
            self.do(CLAUDE, 'take', iid)
        self.assertEqual(self.state(iid), 'doing')

    def test_release_answer_and_reject_start_the_order_over(self):
        iid = self.add('--type', 'research', '--runtime', 'any')
        self.do(CLAUDE, 'take', iid)
        self.do(COORDINATOR, 'release', iid, '--text', 'dead worker')
        self.do(CODEX, 'take', iid)  # the earlier take note is before the release: not a rival
        self.do(CODEX, 'result', iid, '--text', 'done', '--checks', 'none')
        self.do(COORDINATOR, 'reject', iid, '--text', 'again')
        self.do(CLAUDE, 'take', iid)
        self.assertEqual(self.block(iid)['claim']['session'], 'claude-session')

    def test_worker_task_gives_that_tasks_brief_and_the_supervisor_spawns_with_it(self):
        first, second = self.add('--type', 'research', '--priority', '1'), self.add('--type', 'research')
        self.assertIn(f'take {second}`', self.do(CLAUDE, 'worker', '--task', second))
        self.assertIn(f'take {first}`', self.do(CLAUDE, 'worker'))
        supervisor = {'CLAUDE_CODE_SESSION_ID': 'supervisor-1', 'CODEX_THREAD_ID': ''}
        q.save(q.task(second), supervisor={'runtime': 'claude', 'session': 'supervisor-1'})
        self.assertIn(f'taskq worker --task {second}` and follow', self.do(supervisor, 'supervise', second))
        self.assertEqual(worker.worker_prompt(second), q.WORKER.replace('taskq worker`', f'taskq worker --task {second}`'))

    def test_a_worker_without_task_never_picks_a_supervised_task(self):
        iid = self.add('--type', 'research')
        q.save(q.task(iid), supervisor={'runtime': 'claude', 'session': 'supervisor-1'})
        self.assertIn('No task can start', self.do(CLAUDE, 'worker'))

    def test_a_ready_task_still_naming_a_worker_fails_closed(self):
        iid = self.add('--type', 'research', '--runtime', 'claude')
        q.save(q.task(iid), claim={'runtime': 'claude', 'session': 'foreign-live', 'node': q.node()})
        self.assertIn('claim still names session foreign-live', self.refused(CLAUDE, 'take', iid))
        self.do(COORDINATOR, 'release', iid, '--text', 'its worker is gone')
        self.assertEqual(self.block(iid)['claim'], {'runtime': None, 'session': None})
        self.do(CLAUDE, 'take', iid)
        self.assertEqual(self.block(iid)['claim']['session'], 'claude-session')

    def test_take_keeps_every_admission_guard(self):
        dep = self.add('--type', 'research')
        cases = {'open dependencies': self.add('--type', 'research', '--runtime', 'claude', '--deps', str(dep)),
                 'host is win': self.add('--type', 'research', '--runtime', 'claude', '--host', 'win'),
                 'runtime is codex': self.add('--type', 'research', '--runtime', 'codex')}
        for why, iid in cases.items():
            with self.subTest(why):
                self.assertIn(why, self.refused(CLAUDE, 'take', iid))
                self.assertEqual(self.takes(iid), [])  # refused before its note
        assigned = self.add('--type', 'research')
        q.api('PUT', f'issues/{assigned}', {'assignee_ids': [2]})
        self.assertIn('delegation needs an owner-verified policy', self.refused(CLAUDE, 'take', assigned))


if __name__ == '__main__':
    unittest.main()
