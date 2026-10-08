"""#243: the universal supervisor layer. The tick spawns one supervisor session per ready task on Claude and Codex
(never the worker, never work in the PM session); the supervisor launches, reviews, publishes and closes; one slot
holds a task's supervisor and its worker. In-memory tracker, mocked `claude --bg` and Codex spawn: no real session."""
import contextlib
import io
import json
import unittest
from unittest.mock import patch

import test_taskq as base  # the in-memory GitLab cycle; run with `discover -s tests`
from test_pm_tick_separation import job
from test_taskq import COORDINATOR, q, tick, worker

SUPERVISOR = {'CLAUDE_CODE_SESSION_ID': 'supervisor-1', 'CODEX_THREAD_ID': ''}
WORKER = {'CLAUDE_CODE_SESSION_ID': 'worker-1', 'CODEX_THREAD_ID': ''}


class SupervisorLayer(unittest.TestCase):
    setUp, do, refused, state = base.Cycle.setUp, base.Cycle.do, base.Cycle.refused, base.Cycle.state

    def add(self, runtime='claude'):
        return base.Cycle.add(self, '--type', 'research', '--runtime', runtime)

    def block(self, iid):
        return json.loads(q.BLOCK.search(self.gitlab.issues[iid]['description'])[1])

    def claude(self, *sessions):
        """Mock `claude --bg`: each spawn takes the next session id and is listed busy, as a fresh job is."""
        self.spawned, left = [], list(sessions)

        def spawn(name, extra=None, prompt=None, remote_control=True):
            session = left.pop(0)
            self.spawned.append((name, prompt))
            self.agents = {**self.agents, **job(session, name)}
            return session
        self.enterContext(patch.object(worker, 'claude_spawn', spawn))

    def act(self):
        with contextlib.redirect_stderr(io.StringIO()) as log, contextlib.suppress(SystemExit):
            output = self.do(COORDINATOR, 'tick', '--act')
        return output, log.getvalue()

    def test_claude_tick_spawns_one_supervisor_per_task_and_no_worker(self):
        first, second = self.add(), self.add()
        self.claude('supervisor-1', 'supervisor-2')
        output, log = self.act()
        self.assertEqual([name for name, _ in self.spawned], [f'S{first} t (mac-1)', f'S{second} t (mac-1)'])
        self.assertIn(f'taskq supervise {first}` and follow', self.spawned[0][1])
        self.assertIn(f'Done: spawn a claude supervisor for', log)
        for iid, session in ((first, 'supervisor-1'), (second, 'supervisor-2')):
            found = self.block(iid)
            self.assertEqual(found['supervisor'], {'runtime': 'claude', 'session': session})
            self.assertNotIn('reservation', found)  # the supervisor reserves again for its worker
            self.assertEqual((self.state(iid), found['claim']), ('ready', None))  # nothing taken in the PM's pass
        self.assertEqual(self.gitlab.locked(), [])
        self.act()
        self.assertEqual(len(self.spawned), 2)  # supervised: never started again

    def test_codex_tick_spawns_one_supervisor_per_task(self):
        iid, spawned = self.add('codex'), []

        def spawn(name, prompt=None, full_access=False):
            spawned.append((name, prompt))
            return 'thread-1'
        self.enterContext(patch.object(q, 'codex_spawn', spawn))
        self.enterContext(patch.object(q, 'CODEX_SOCKET', q.ROOT))
        self.act()
        self.assertEqual([name for name, _ in spawned], [f'S{iid} t (mac-1)'])
        self.assertIn(f'taskq supervise {iid}`', spawned[0][1])
        self.assertEqual(self.block(iid)['supervisor'], {'runtime': 'codex', 'session': 'thread-1'})

    def test_supervisor_brief_launches_the_worker_and_refuses_others(self):
        iid = self.add()
        self.claude('supervisor-1', 'worker-1')
        self.act()
        brief = self.do(SUPERVISOR, 'supervise', iid)
        self.assertIn(f'You are the supervisor of task #{iid}, now ready', brief)
        self.assertIn(f"taskq spawn --runtime claude --name 'T{iid} t' --text", brief)
        self.assertIn(f'taskq close {iid}', brief)
        self.assertIn(brief, self.do(SUPERVISOR, 'worker'))  # a supervisor never gets work of its own
        self.assertIn('not by claude:worker-1', self.refused(WORKER, 'supervise', iid))
        self.do(SUPERVISOR, 'spawn', '--name', f'T{iid} t', '--text', 'go')
        self.do(WORKER, 'take', iid)
        self.assertEqual((self.state(iid), self.block(iid)['claim']['session']), ('doing', 'worker-1'))

    def test_slot_holds_supervisor_and_worker_together(self):
        first, second = self.add(), self.add()
        self.claude('supervisor-1', 'worker-1', 'supervisor-2')
        with contextlib.redirect_stderr(io.StringIO()), contextlib.suppress(SystemExit):
            self.do(COORDINATOR, 'tick', '--act', '--limit', 'claude=1,codex=0')
        self.assertEqual([name for name, _ in self.spawned], [f'S{first} t (mac-1)'])  # one slot: one supervisor
        self.do(SUPERVISOR, 'spawn', '--name', f'T{first} t', '--text', 'go')
        self.do(WORKER, 'take', first)
        with contextlib.redirect_stderr(io.StringIO()), contextlib.suppress(SystemExit):
            self.do(COORDINATOR, 'tick', '--act', '--limit', 'claude=1,codex=0')
        self.assertEqual(len(self.spawned), 2)  # its live supervisor and worker hold the one slot: nothing more
        with contextlib.redirect_stderr(io.StringIO()), contextlib.suppress(SystemExit):
            self.do(COORDINATOR, 'tick', '--act', '--limit', 'claude=2,codex=0')
        self.assertEqual(self.spawned[-1][0], f'S{second} t (mac-1)')  # counted once, not twice

    def test_review_wakes_the_supervisor_not_the_pm_and_supervisor_closes(self):
        iid, woken = self.add(), []
        self.claude('supervisor-1', 'worker-1')
        self.act()
        self.do(SUPERVISOR, 'spawn', '--name', f'T{iid} t', '--text', 'go')
        self.do(WORKER, 'take', iid)
        self.do(WORKER, 'result', iid, '--checks', 'c', '--text', 'done')
        self.agents = {**job('supervisor-1', f'S{iid} t (mac-1)', state='idle'), **job('worker-1', f'T{iid} t (mac-1)', state='idle')}
        self.enterContext(patch.object(q, 'claude_wake', lambda session, prompt: woken.append((session, prompt))))
        output, _ = self.act()
        self.assertEqual(woken, [('supervisor-1', tick.SUPERVISE.format(iid=iid))])
        self.assertNotIn(f'## Review', output)  # not the PM's review
        self.assertIn('supervised by', self.refused(COORDINATOR, 'close', iid, '--text', 'x'))  # R3: never the PM
        stopped = []
        self.enterContext(patch.object(q, 'claude_stop', lambda session, remove=False: stopped.append(session)))
        self.enterContext(patch.object(worker, 'claude_stop', lambda session, remove=False: stopped.append(session)))
        self.do(SUPERVISOR, 'close', iid, '--text', 'checked')
        self.assertEqual(self.gitlab.issues[iid]['state'], 'closed')
        self.act()
        self.assertIn('supervisor-1', stopped)  # R11: retired after its accepted close

    def test_answer_from_the_pm_wakes_the_supervisor_for_the_same_worker(self):
        iid, woken = self.add(), []
        self.claude('supervisor-1', 'worker-1')
        self.act()
        self.do(SUPERVISOR, 'spawn', '--name', f'T{iid} t', '--text', 'go')
        self.do(WORKER, 'take', iid)
        self.do(WORKER, 'ask', iid, '--text', 'which?')
        self.do(COORDINATOR, 'answer', iid, '--text', 'this one')
        self.agents = {**job('supervisor-1', f'S{iid} t (mac-1)', state='idle')}
        self.enterContext(patch.object(q, 'claude_wake', lambda session, prompt: woken.append(session)))
        self.act()
        self.assertEqual(woken, ['supervisor-1'])
        self.assertIn('claude --bg --resume worker-1', self.do(SUPERVISOR, 'supervise', iid))
        self.do(WORKER, 'take', iid)  # the same worker continues
        self.assertEqual(self.block(iid)['claim']['session'], 'worker-1')

    def test_workers_table_has_the_same_columns_on_both_runtimes(self):
        rows = []
        with patch.object(q, 'claude_url', lambda session: f'https://claude.ai/code/session_{session}'):
            for runtime in ('claude', 'codex'):
                item = {'iid': 1, 'title': 't', 'state': 'doing', 'web_url': 'https://h/g/p/-/issues/1', 'updated_at': 'x', 'result': None,
                        'claim': {'runtime': runtime, 'session': f'{runtime}-worker'},
                        'supervisor': {'runtime': runtime, 'session': f'{runtime}-supervisor'}}
                rows.append(tick.report_row(item, {}))
        self.assertEqual(list(rows[0]), list(rows[1]))
        self.assertIn('https://claude.ai/code/session_claude-worker', rows[0]['session'])
        self.assertIn('supervisor [session](https://claude.ai/code/session_claude-supervisor)', rows[0]['session'])
        self.assertIn('open.html#codex://threads/codex-worker', rows[1]['session'])
        self.assertIn('open.html#codex://threads/codex-supervisor', rows[1]['session'])
        self.assertEqual({row['runtime'] for row in rows}, {'claude', 'codex'})


if __name__ == '__main__':
    unittest.main()
