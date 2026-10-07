"""#185 fixtures: the five-minute TICK runs apart from the interactive PM session. Existing mechanisms only:
the launchd timer (`tick --install-timer`), the checkout's tick lock, `wake` into the [coordinator] session.
No timer is armed, no worker spawned, no live session touched. `expectedFailure` marks a gap that needs a
code change outside this task's scope (docs/pm-tick-separation.md, Gaps)."""
import contextlib
import io
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import Mock, patch

import test_taskq as base  # the in-memory GitLab cycle; run with `discover -s tests`
from test_taskq import CLAUDE, COORDINATOR, q, tick


class TimerOwner(unittest.TestCase):
    """One timer per checkout; it wakes exactly the session recorded in [coordinator]; handoff is an explicit edit."""

    def setUp(self):
        self.enterContext(patch.object(sys, 'platform', 'darwin'))
        tmp = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.tmp, self.runs = tmp, []
        self.enterContext(patch.object(q, 'TICK_BEAT', tmp / '.local/beat'))
        self.enterContext(patch.object(q, 'LOCAL', tmp / 'taskq.local.toml'))
        self.enterContext(patch.object(Path, 'home', return_value=tmp))
        self.enterContext(patch.object(tick.subprocess, 'run', lambda argv, **kwargs: self.runs.append(argv)))

    def install(self, session):
        with patch.dict(os.environ, {'CLAUDE_CODE_SESSION_ID': session}), contextlib.redirect_stdout(io.StringIO()) as out:
            q.main(['tick', '--install-timer'])
        return out.getvalue()

    def test_second_install_replaces_the_timer_and_keeps_the_owner(self):
        self.install('pm-a')
        output = self.install('pm-b')  # another session arms "its" timer: no second agent, no takeover
        agents = list((self.tmp / 'Library/LaunchAgents').iterdir())
        self.assertEqual([path.name for path in agents], [f'taskq.{q.ROOT.name}.plist'])
        self.assertEqual(plistlib.loads(agents[0].read_bytes())['WorkingDirectory'], str(q.ROOT))
        # Every install boots the old agent out before it bootstraps the new one: never two loaded.
        self.assertEqual([argv[1] for argv in self.runs], ['bootout', 'bootstrap'] * 2)
        self.assertEqual(q.personal()['coordinator']['session'], 'pm-a')
        self.assertIn('It wakes the coordinator session pm-a.', output)
        self.assertEqual(q.LOCAL.read_text().count('[coordinator]'), 1)

    def test_explicit_handoff_moves_the_wake_target(self):
        self.install('pm-a')
        q.LOCAL.write_text(q.LOCAL.read_text().replace('"pm-a"', '"pm-b"'))  # the owner's handoff
        with patch.object(q, 'claude_agents', return_value={}), patch.object(q, 'claude_wake') as wake, \
             contextlib.redirect_stdout(io.StringIO()):
            tick.wake('output', ['review 1 abc'])
        self.assertEqual(wake.call_args[0][0], 'pm-b')

    def test_tick_identity_is_the_checkout(self):
        """In-session TICK: the prompt names the main checkout. launchd TICK: the agent runs in it and wakes the
        session recorded in that checkout's taskq.local.toml."""
        self.assertIn(f'cd {q.ROOT} && ', q.TICK_PROMPT.replace('<main checkout>', str(q.ROOT)))
        self.install('pm-a')
        agent = plistlib.loads(next((self.tmp / 'Library/LaunchAgents').iterdir()).read_bytes())
        self.assertEqual((agent['WorkingDirectory'], agent['ProgramArguments'][1:]), (str(q.ROOT), ['-m', 'taskq', 'tick', '--act', '--wake']))

    @unittest.expectedFailure
    def test_gap_wake_turn_names_the_project(self):
        """Gap 3: the wake turn's text names no checkout or repository; only its target session binds it."""
        self.assertIn(str(q.ROOT), tick.WAKE_PROMPT)


class Wake(unittest.TestCase):
    """Delivery into the PM session is a wake; a completed pass is something else. Busy PM: no turn injected."""

    def setUp(self):
        tmp = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.enterContext(patch.object(tick, 'woken', return_value=tmp / 'woken'))
        self.enterContext(patch.object(q, 'personal', return_value={'coordinator': {'session': 'pm-a'}}))
        self.agents = {}
        self.enterContext(patch.object(q, 'claude_agents', lambda: self.agents))
        self.send = self.enterContext(patch.object(q, 'claude_wake'))

    def wake(self, judgement):
        with contextlib.redirect_stdout(io.StringIO()) as out:
            tick.wake('output', judgement)
        return out.getvalue()

    def test_busy_pm_is_not_interrupted_and_gets_the_tick_later(self):
        self.agents = {'pm-a': {'sessionId': 'pm-a', 'pid': 1, 'status': 'busy'}}  # the owner talks to the PM
        self.assertIn('The coordinator is busy: the next tick wakes it.', self.wake(['review 1 abc']))
        self.send.assert_not_called()
        self.assertFalse(tick.woken().exists())  # not counted as delivered
        self.agents['pm-a']['status'] = 'idle'
        self.assertIn('Woke the coordinator pm-a.', self.wake(['review 1 abc']))
        self.send.assert_called_once()

    def test_repeated_tick_coalesces_and_new_items_deliver_again(self):
        for _ in range(3):
            self.wake(['review 1 abc'])
        self.assertEqual(self.send.call_count, 1)
        self.wake(['review 1 abc', 'ask 2'])
        self.assertEqual(self.send.call_count, 2)

    def test_failed_delivery_is_not_a_receipt_and_is_retried(self):
        self.send.side_effect = SystemExit('claude --resume pm-a: the job x failed at once')
        with self.assertRaises(SystemExit):
            self.wake(['review 1 abc'])
        self.assertFalse(tick.woken().exists())
        self.send.side_effect = None
        self.wake(['review 1 abc'])
        self.assertEqual(self.send.call_count, 2)

    def test_delivery_receipt_is_not_a_completed_pass(self):
        """The woken key is all a delivery leaves; nothing reads the PM's turn back."""
        self.wake(['review 1 abc'])
        self.assertTrue(tick.woken().read_text().strip())
        self.assertEqual(self.send.call_args[0][0], 'pm-a')
        self.assertTrue(self.send.call_args[0][1].startswith(tick.WAKE_PROMPT))

    @unittest.expectedFailure
    def test_gap_resumed_pm_is_busy_under_a_new_session_id(self):
        """Gap 1 (#182 evidence): `claude --bg --resume` runs under a new session id with the same name. The
        busy check reads only the recorded id, so a second wake resumes the old id beside the busy one."""
        self.agents = {'pm-a': {'sessionId': 'pm-a', 'name': 'PM', 'state': 'done'},
                       'pm-b': {'sessionId': 'pm-b', 'name': 'PM', 'pid': 2, 'status': 'busy'}}
        self.wake(['review 1 abc', 'ask 2'])
        self.send.assert_not_called()


@unittest.skipIf(os.name == 'nt', 'flock holder; Windows locking: test_taskq TickBeat')
class Overlap(unittest.TestCase):
    """Timer plus manual TICK: one pass per checkout at a time; the second returns at once, it never waits."""

    def test_manual_tick_during_a_slow_timer_pass_returns_at_once(self):
        args = Mock(install_timer=False, uninstall_timer=False, act=False, spec=['install_timer', 'uninstall_timer', 'act'])
        with tempfile.TemporaryDirectory() as tmp, patch.object(q, 'TICK_BEAT', Path(tmp) / 'beat'), \
                patch.object(tick, 'tick_pass') as run:
            slow = subprocess.Popen([sys.executable, '-c',  # the timer's pass, holding the checkout's lock
                "import fcntl, sys; f = open(sys.argv[1], 'a+b'); fcntl.flock(f, fcntl.LOCK_EX); "
                "print('locked', flush=True); sys.stdin.read()", str(Path(tmp) / 'taskq-tick.lock')],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
            try:
                self.assertEqual(slow.stdout.readline().strip(), 'locked')
                start = time.monotonic()
                with contextlib.redirect_stderr(io.StringIO()) as said:
                    for _ in range(3):  # repeated manual TICKs
                        q.tick(args)
                self.assertLess(time.monotonic() - start, 1)
            finally:
                slow.kill()
                slow.wait(timeout=5)
                slow.stdin.close()
                slow.stdout.close()
            run.assert_not_called()
            self.assertEqual(said.getvalue().count('Skipped: another tick pass is running.'), 3)
            q.tick(args)  # the interrupted holder left its file; the next pass runs
            run.assert_called_once()

    def test_skipped_pass_is_a_refusal_in_json(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(q, 'TICK_BEAT', Path(tmp) / 'beat'), \
                patch.object(tick, 'tick_pass') as run:
            Path(tmp, 'taskq-tick.lock').touch()
            holder = open(Path(tmp, 'taskq-tick.lock'), 'a+b')
            self.addCleanup(holder.close)
            import fcntl
            fcntl.flock(holder, fcntl.LOCK_EX)  # flock is per open file: this process's second open is refused
            with contextlib.redirect_stdout(io.StringIO()) as out, contextlib.redirect_stderr(io.StringIO()):
                q.main(['tick', '--act', '--json'])
        envelope = json.loads(out.getvalue())
        run.assert_not_called()
        self.assertEqual((envelope['outcome'], envelope['refusals']), ('ok', ['another tick pass is running']))
        self.assertEqual(envelope['actions'], [{'action': 'tick', 'status': 'refused', 'reason': 'another tick pass is running'}])


class Restart(unittest.TestCase):
    """A pass after a restart or a repeated TICK reads the queue again; a claim is never duplicated."""
    setUp, do, add, state = base.Cycle.setUp, base.Cycle.do, base.Cycle.add, base.Cycle.state

    def act(self):
        with patch.object(q, 'spawn') as spawn, contextlib.redirect_stderr(io.StringIO()):
            try:
                self.do(COORDINATOR, 'tick', '--act')
            except SystemExit:
                pass  # exit 1: judgement needed
        return [call.args[0].name for call in spawn.call_args_list]

    def test_taken_task_is_not_started_again(self):
        iid = self.add('--type', 'research', '--runtime', 'claude')
        self.assertEqual(self.act(), [f'T{iid} t'])
        self.do(CLAUDE, 'take', iid)
        self.agents = {'claude-session': {'id': 'claudese', 'sessionId': 'claude-session', 'pid': 1, 'status': 'busy'}}
        self.assertEqual(self.act(), [])
        self.assertIn('cannot start: state is doing', base.Cycle.refused(self, COORDINATOR, 'take', iid))

    @unittest.expectedFailure
    def test_gap_spawned_not_yet_taken_is_spawned_again(self):
        """Gap 2: a TICK between spawn and take (manual TICK, restart) spawns a second worker. take's lock
        keeps one claim; the loser runs `worker` again and may take another task."""
        iid = self.add('--type', 'research', '--runtime', 'claude')
        self.assertEqual(self.act(), [f'T{iid} t'])
        self.assertEqual(self.act(), [])


if __name__ == '__main__':
    unittest.main()
