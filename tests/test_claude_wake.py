"""#284: waking a stopped Claude supervisor. `claude --bg --resume` forks a new session id; the transcript proves the
fork, and the block follows it so the supervisor keeps its task. Mocked CLI, in-memory tracker: no real session."""
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import test_supervisor_layer as layer
from test_taskq import q, worker
from taskq.runtimes.claude import Adapter

FORK = {'CLAUDE_CODE_SESSION_ID': 'supervisor-2', 'CODEX_THREAD_ID': ''}


def transcripts(root, **sessions):
    """Write `~/.claude/projects/p/<id>.jsonl` with the given message uuids."""
    folder = root / 'projects' / 'p'
    folder.mkdir(parents=True, exist_ok=True)
    for session, uuids in sessions.items():
        (folder / f'{session}.jsonl').write_text(''.join(json.dumps({'type': 'user', 'uuid': uuid}) + '\n' for uuid in [None, *uuids]))


class AdapterWake(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.agents = layer.job('old-session', 'S7 t (mac)', state='stopped')
        self.enterContext(patch.object(q, 'CLAUDE_JOBS', self.root / 'jobs'))
        self.enterContext(patch.object(worker, 'claude_agents', lambda **kwargs: dict(self.agents)))
        self.enterContext(patch.object(worker, 'claude_stop', lambda session, remove=False: None))
        self.enterContext(patch.object(q.time, 'sleep', lambda seconds: None))

    def wake(self, fork):
        def run(argv, **kwargs):
            if fork:
                self.agents.update(layer.job('new-session', 'S7 t (mac)', state='idle'))
            return SimpleNamespace(returncode=0, stdout='', stderr='')
        clock = iter(range(0, 100, 10))
        with patch.object(q.subprocess, 'run', run), patch.object(q.time, 'time', lambda: next(clock)):
            return Adapter().send('old-session', 'continue')

    def test_fork_proven_by_transcript_returns_the_new_id(self):
        transcripts(self.root, **{'old-session': ['a', 'b'], 'new-session': ['a', 'b', 'c']})
        self.assertEqual(self.wake(fork=True), 'new-session')

    def test_new_job_without_copied_transcript_is_no_fork(self):
        transcripts(self.root, **{'old-session': ['a'], 'new-session': ['x']})
        self.assertEqual(self.wake(fork=True), 'old-session')

    def test_resume_under_the_same_id_keeps_it(self):
        self.assertEqual(self.wake(fork=False), 'old-session')


class SupervisorWake(unittest.TestCase):
    real_launch = True
    setUp, do, refused, state = layer.SupervisorLayer.setUp, layer.SupervisorLayer.do, layer.SupervisorLayer.refused, layer.SupervisorLayer.state
    add, block, claude, act = layer.SupervisorLayer.add, layer.SupervisorLayer.block, layer.SupervisorLayer.claude, layer.SupervisorLayer.act

    def review(self):
        iid = self.add()
        self.claude('supervisor-1', 'worker-1')
        self.act()
        self.do(layer.SUPERVISOR, 'spawn', '--name', f'T{iid} t', '--text', 'go')
        self.do(layer.WORKER, 'take', iid)
        self.do(layer.WORKER, 'result', iid, '--checks', 'c', '--text', 'done')
        self.agents = {**layer.job('supervisor-1', f'S{iid} t (mac-1)', state='stopped'), **layer.job('worker-1', f'T{iid} t (mac-1)', state='idle')}
        return iid

    def test_tick_wakes_a_stopped_supervisor_and_records_its_fork(self):
        iid, woken = self.review(), []
        self.enterContext(patch.object(q, 'claude_wake', lambda session, prompt: woken.append(session) or 'supervisor-2'))
        self.act()
        self.assertEqual(woken, ['supervisor-1'])
        self.assertEqual(self.block(iid)['supervisor'], {'runtime': 'claude', 'session': 'supervisor-2'})
        self.assertIn('claude --bg --resume worker-1', self.do(FORK, 'supervise', iid))  # its launch stays its own
        self.assertIn('not by claude:supervis', self.refused(layer.SUPERVISOR, 'supervise', iid))

    def test_forked_session_adopts_itself_from_its_transcript(self):
        iid = self.review()
        self.assertIn('only that session or the owner', self.refused(FORK, 'reject', iid, '--text', 'x'))
        transcripts(self.directory, **{'supervisor-1': ['a', 'b'], 'supervisor-2': ['a', 'b', 'c']})
        self.assertIn('claude --bg --resume worker-1', self.do(FORK, 'supervise', iid))
        self.assertEqual(self.block(iid)['supervisor'], {'runtime': 'claude', 'session': 'supervisor-2'})
        self.do(FORK, 'reject', iid, '--text', 'fix it')
        self.assertEqual(self.state(iid), 'ready')

    def test_brief_forbids_renaming_and_helper_sessions(self):
        iid = self.add()
        self.claude('supervisor-1')
        self.act()
        self.assertIn('never rename it) and start no other session', self.do(layer.SUPERVISOR, 'supervise', iid))

if __name__ == '__main__':
    unittest.main()
