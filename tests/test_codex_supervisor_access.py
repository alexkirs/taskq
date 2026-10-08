"""#270: Codex supervisors reach the app server; access follows the thread, not the claim."""
import contextlib
import io
import sys
import unittest
from unittest.mock import patch

import taskq as q
codex, tick = sys.modules['taskq.codex'], sys.modules['taskq.tick']  # q.tick is the command

FULL = {'type': 'dangerFullAccess'}


class FakeCodex:
    def __init__(self, names):
        self.names, self.calls = names, []

    def __call__(self, timeout=60):
        return self

    def call(self, method, params):
        self.calls.append((method, params))
        if method == 'thread/read':
            return {'thread': {'status': {'type': 'idle'}, 'name': self.names.get(params['threadId'])}}
        if method == 'thread/start':
            return {'thread': {'id': 's-new'}}
        return {}

    def policy(self):
        return next(params for method, params in reversed(self.calls) if method == 'turn/start')['sandboxPolicy']


def task(iid, **extra):
    return {'iid': iid, 'title': 't', 'state': 'ready', 'runtime': 'codex', 'full_access': False, 'claim': None,
            'supervisor': None, **extra}


class SupervisorAccess(unittest.TestCase):
    def setUp(self):
        self.fake = FakeCodex({'s1': 'S7 t (mac)', 'w1': 'T7 t (mac)', 'w2': 'T8 t (mac)'})
        self.tasks = [task(7, supervisor={'runtime': 'codex', 'session': 's1'}), task(8, full_access=True)]
        for target, name, value in ((codex, 'Codex', self.fake), (codex, 'codex_project', lambda _: 'p'),
                                    (codex, 'codex_announce', lambda *_: None),
                                    (q, 'load', lambda *_, **__: (self.tasks, {7, 8}, [], [], []))):
            self.enterContext(patch.object(target, name, value))
        self.enterContext(contextlib.redirect_stdout(io.StringIO()))

    def test_supervisor_spawn_and_wake_reach_the_app_server(self):
        from taskq.runtimes import get
        args = tick.supervisor_spawn(task(7))
        self.assertTrue(args.full_access)
        get('codex', full_access=args.full_access).spawn(args.name, args.text)
        self.assertEqual(self.fake.policy(), FULL)
        tick.wake_supervisor(self.tasks[0])
        self.assertEqual(self.fake.policy(), FULL)

    def test_send_decides_by_the_thread_not_the_claim(self):
        launches = {8: [{'body': '**launch** · codex:s8\n\nsession w2'}]}
        self.enterContext(patch.object(q, 'comments', lambda iid, **_: launches.get(iid, [])))
        q.main(['codex-send', 's1', '--text', 'wake'])  # a supervisor, whatever the label
        self.assertEqual(self.fake.policy(), FULL)
        q.main(['codex-send', 'w2', '--text', 'resume'])  # FULL_ACCESS worker resumed after its claim was cleared
        self.assertEqual(self.fake.policy(), FULL)
        q.main(['codex-send', 'w1', '--text', 'resume'])  # a plain worker stays sandboxed
        self.assertEqual(self.fake.policy()['type'], 'workspaceWrite')
        self.fake.names['x1'] = 'S8 renamed'  # a name grants nothing
        q.main(['codex-send', 'x1', '--text', 'spoof'])
        self.assertEqual(self.fake.policy()['type'], 'workspaceWrite')

    def test_a_supervisor_problem_is_the_rows_blocker(self):
        item = self.tasks[0]
        notes = [{'body': '**launch** · codex:s1\n\nsession w1'},
                 {'body': '**problem** · codex:s1\n\nno Codex app server on this machine'}]
        with patch.object(q, 'comments', lambda *_, **__: notes):
            self.assertEqual(tick.blocker(item), 'blocked: no Codex app server on this machine')
            notes.append({'body': '**edit** · codex:s1\n\nscope'})
            self.assertIsNone(tick.blocker(item))
        self.assertIsNone(tick.blocker(self.tasks[1]))


if __name__ == '__main__':
    unittest.main()
