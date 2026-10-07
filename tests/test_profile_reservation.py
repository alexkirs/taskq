"""#208 fixtures (#186 stage 1): a worker launch reserves its task through the tracker lock before it spawns, and
the launched worker adopts exactly that reservation through `take`. Pinned spec: Wiki Atomic-reservation-before-
worker-launch at 9ccb572258c14c0252ba74e2db6d1f288a5f12fb. In-memory tracker, mocked runtimes: no real session,
timer or project is touched; these fixtures qualify no live transport, owner approval or multiproject execution."""
import contextlib
import io
import json
import os
import subprocess
import sys
import unittest
from unittest.mock import patch

import test_taskq as base  # the in-memory GitLab cycle; run with `discover -s tests`
from test_pm_tick_separation import job
from test_taskq import CLAUDE, COORDINATOR, q, worker

WORKER = {'CLAUDE_CODE_SESSION_ID': 'worker-1', 'CODEX_THREAD_ID': ''}
OTHER = {'CLAUDE_CODE_SESSION_ID': 'intruder', 'CODEX_THREAD_ID': ''}


def dead_pid():
    done = subprocess.Popen([sys.executable, '-c', 'pass'])
    done.wait()
    return done.pid


class Reservation(unittest.TestCase):
    setUp, do, refused, add, state = base.Cycle.setUp, base.Cycle.do, base.Cycle.refused, base.Cycle.add, base.Cycle.state

    def launches(self, session='worker-1', during=None, error=None):
        """Mock `claude --bg`: record each launch, list the new job as `claude agents` would, run `during` inside."""
        self.spawned = []

        def spawn(name, extra=None, prompt=None, remote_control=True):
            self.spawned.append(name)
            if error:
                q.fail(error)
            self.agents = {**self.agents, **job(session, name)}
            if during:
                during()
            return session
        self.enterContext(patch.object(worker, 'claude_spawn', spawn))

    def spawn(self, iid, who=COORDINATOR):
        return self.do(who, 'spawn', '--name', f'T{iid} t', '--text', 'go')

    def block(self, iid):
        return json.loads(q.BLOCK.search(self.gitlab.issues[iid]['description'])[1])

    def said(self, iid):
        return '\n'.join(self.gitlab.said(iid))

    def test_launch_reserves_before_spawn_and_the_matching_worker_adopts(self):
        iid = self.add('--type', 'research', '--runtime', 'claude')
        seen = []
        self.launches(during=lambda: seen.append((self.block(iid).get('reservation'), self.gitlab.locked())))
        self.spawn(iid)
        reservation, locked = seen[0]
        self.assertEqual(locked, [iid])  # the lock and the attempt exist before the runtime is asked
        self.assertEqual((reservation['runtime'], reservation['principal'], reservation['coordinator']), ('claude', 1, 'claude:coordina'))
        self.assertNotIn('session', reservation)  # no worker identity invented before the launch
        self.assertEqual(self.state(iid), 'ready')
        self.assertIn(f'launched session worker-1', self.said(iid))
        self.assertIn('reserved by claude:coordina @mac-1', self.refused(OTHER, 'take', iid))
        self.assertIn(f'take {iid}', self.do(WORKER, 'worker', '--limit', 'claude=1'))  # its place is the reservation's
        self.do(WORKER, 'take', iid)
        block = self.block(iid)
        self.assertEqual((self.state(iid), block['claim']['session'], 'reservation' in block), ('doing', 'worker-1', False))
        self.assertEqual(self.gitlab.locked(), [iid])
        self.assertIn('Adopted reservation', self.said(iid))

    def test_worker_faster_than_the_launch_note_adopts_by_its_live_job(self):
        iid = self.add('--type', 'research', '--runtime', 'claude')
        self.launches(during=lambda: self.do(WORKER, 'take', iid))
        self.spawn(iid)
        self.assertEqual(self.block(iid)['claim']['session'], 'worker-1')
        self.agents = job('impostor', f'T{iid + 1} t (mac-1)')
        other = self.add('--type', 'research', '--runtime', 'claude')
        self.launches(session='worker-2', during=lambda: self.assertIn('reserved by', self.refused(
            {**WORKER, 'CLAUDE_CODE_SESSION_ID': 'impostor-of-other-task'}, 'take', other)))
        self.spawn(other)
        self.assertEqual(self.state(other), 'ready')

    def test_two_coordinators_of_different_users_and_machines_race_one_task(self):
        """B reads, A reserves and launches, then B locks: the lock refuses B, which launches nothing."""
        iid = self.add('--type', 'research', '--runtime', 'any')
        self.launches()
        load, raced = q.load, []

        def racing(*args, **kwargs):
            found = load(*args, **kwargs)
            if not raced:
                raced.append(1)
                with patch.object(q, 'load', load), patch.object(q.socket, 'gethostname', return_value='mac-1.local'):
                    self.gitlab.uid = 1
                    self.spawn(iid)
                self.gitlab.uid = 2
            return found
        self.gitlab.uid = 2
        with patch.object(q, 'load', racing), patch.object(q.socket, 'gethostname', return_value='win-2.lan'):
            self.assertIn('another coordinator or worker holds its lock', self.refused(
                {**COORDINATOR, 'CLAUDE_CODE_SESSION_ID': 'coordinator-b'}, 'spawn', '--name', f'T{iid} t'))
        self.assertEqual(len(self.spawned), 1)
        self.assertEqual(self.block(iid)['reservation']['principal'], 1)

    def test_intersecting_filters_start_one_worker(self):
        iid = self.add('--type', 'research', '--runtime', 'claude', '--area', 'maps', '--priority', '1')
        self.launches()
        with contextlib.redirect_stderr(io.StringIO()):
            self.do(COORDINATOR, 'tick', '--act', '--filter', 'labels=area-maps')
            with contextlib.suppress(SystemExit):
                self.do({**COORDINATOR, 'CLAUDE_CODE_SESSION_ID': 'coordinator-b'}, 'tick', '--act', '--filter', 'labels=priority-1')
        self.assertEqual(self.spawned, [f'T{iid} t (mac-1)'])
        self.assertIn(f'reserved by claude:coordina @mac-1', self.do(COORDINATOR, 'list'))

    def test_overlapping_scopes_reserved_at_once_do_not_both_launch(self):
        wide, narrow = self.add('--type', 'research', '--runtime', 'claude', '--scope', 'a'), self.add('--type', 'research', '--runtime', 'claude', '--scope', 'a/b')
        self.launches()
        lock, raced = q.lock, []

        def racing(iid):
            if iid == wide and not raced:
                raced.append(1)  # the other coordinator reserves the narrow task after this one read the queue
                self.spawn(narrow)
            return lock(iid)
        with patch.object(q, 'lock', racing):
            self.assertIn(f'scope overlaps #{narrow}', self.refused(COORDINATOR, 'spawn', '--name', f'T{wide} t'))
        self.assertEqual(self.spawned, [f'T{narrow} t (mac-1)'])
        self.assertNotIn('reservation', self.block(wide))
        self.assertEqual(self.gitlab.locked(), [narrow])
        self.assertIn(f'scope overlaps #{narrow}', self.refused(COORDINATOR, 'spawn', '--name', f'T{wide} t'))

    def test_failed_launch_without_a_worker_releases_its_attempt(self):
        iid = self.add('--type', 'research', '--runtime', 'claude')
        self.launches(error='claude could not start the session: Not logged in')
        self.assertIn('Not logged in', self.refused(COORDINATOR, 'spawn', '--name', f'T{iid} t'))
        self.assertEqual((self.gitlab.locked(), 'reservation' in self.block(iid)), ([], False))
        self.assertIn('launch failed, no worker started', self.said(iid))
        self.launches()
        self.spawn(iid)
        self.assertEqual(len(self.spawned), 1)

    def test_unknown_launch_outcome_keeps_the_reservation(self):
        for case, error, agents in (('unlisted', 'claude agents does not list the new session ab12', {}),
                                    ('inventory unreadable', 'claude could not start the session: timeout', None)):
            with self.subTest(case):
                iid = self.add('--type', 'research', '--runtime', 'claude')
                self.launches(error=error)
                with patch.object(q, 'claude_agents', lambda strict=False, agents=agents: agents if strict else (agents or {})), \
                     contextlib.redirect_stderr(io.StringIO()):
                    self.refused(COORDINATOR, 'spawn', '--name', f'T{iid} t')
                self.assertIn('outcome unknown', self.said(iid))
                self.assertIn(iid, self.gitlab.locked())
                self.assertIn('reserved by', self.refused(COORDINATOR, 'spawn', '--name', f'T{iid} t'))

    def reserved(self, pid, session=None, **agents):
        """A reservation left by a launch whose process ended (`pid`), with its launch note when `session`."""
        iid = self.add('--type', 'research', '--runtime', 'claude')
        with patch.dict(os.environ, COORDINATOR):
            attempt = q.reserve(iid, 'claude')
            if session:
                worker.launched_note(iid, attempt, session)
        current = q.task(iid)
        q.save(current, reservation={**current['reservation'], 'pid': pid})
        self.agents = agents
        return iid

    def tick(self):
        with contextlib.redirect_stderr(io.StringIO()), patch.object(q, 'spawn') as spawn:
            with contextlib.suppress(SystemExit):
                output = self.do(COORDINATOR, 'tick', '--act')
        return output, [call.args[0].name for call in spawn.call_args_list]

    def test_restart_settles_by_evidence_never_by_age(self):
        dead = dead_pid()
        with patch.object(q, 'LOCK_SECONDS', -1), patch.object(q, 'STALE_MINUTES', -1):
            running = self.reserved(os.getpid())  # its launch still runs
            output, spawns = self.tick()
            self.assertIn('its launch is still running', output)
            self.assertEqual((spawns, self.gitlab.locked()), ([], [running]))
            q.save(q.task(running), 'later', reservation=None)
            q.unlock(running)
            for case, session, rows, kept in (
                    ('worker alive before its take', None, job('w', 'T{iid} t (mac-1)'), True),
                    ('pending approval', 'w', job('w', 'T{iid} t (mac-1)', state='blocked'), True),
                    ('bound worker unknown here', 'w', {}, True),
                    ('bound worker stopped', 'w', job('w', 'T{iid} t (mac-1)', state='failed'), False),
                    ('interrupted, no worker', None, {}, False)):
                with self.subTest(case):
                    iid = self.reserved(dead, session)
                    self.agents = {sid: {**row, 'name': row['name'].format(iid=iid)} for sid, row in rows.items()}
                    output, spawns = self.tick()
                    self.assertEqual('reservation' in self.block(iid), kept, output)
                    self.assertEqual(iid in self.gitlab.locked(), kept)
                    self.assertEqual(spawns, [] if kept else [f'T{iid} t'])  # released: the next start may launch it
                    if kept:
                        q.save(q.task(iid), reservation=None)
                    q.unlock(iid)
                    q.save(q.task(iid), 'later')

    def test_another_machines_reservation_is_not_settled_here(self):
        with patch.object(q.socket, 'gethostname', return_value='win-2.lan'):
            iid = self.reserved(dead_pid())
        output, spawns = self.tick()
        self.assertIn('reserved on another machine', output)
        self.assertEqual((spawns, self.gitlab.locked()), ([], [iid]))

    def test_revoked_assignment_and_foreign_release_keep_ownership(self):
        iid = self.add('--type', 'research', '--runtime', 'claude')
        self.launches()
        self.spawn(iid)
        q.api('PUT', f'issues/{iid}', {'assignee_ids': [2]})  # the task went to another user meanwhile
        self.assertIn('delegation needs an owner-verified policy', self.refused(WORKER, 'take', iid))
        self.assertEqual((self.state(iid), self.gitlab.issues[iid]['assignees']), ('ready', [{'id': 2}]))
        self.gitlab.uid = 2
        self.assertIn('only that user releases it', self.refused(COORDINATOR, 'release', iid, '--text', 'mine now'))
        self.gitlab.uid = 1
        self.do(COORDINATOR, 'release', iid, '--text', 'no worker of it runs')
        self.assertEqual((self.gitlab.locked(), 'reservation' in self.block(iid)), ([], False))
        self.assertIn('delegation needs', self.refused(COORDINATOR, 'spawn', '--name', f'T{iid} t'))

    def test_other_principal_adopting_is_recorded_and_assignee_kept(self):
        iid = self.add('--type', 'research', '--runtime', 'claude', '--mine')
        self.launches()
        self.spawn(iid)
        q.api('PUT', f'issues/{iid}', {'assignee_ids': [1, 2]})  # the owner shares it with user 2
        self.gitlab.uid = 2
        self.do(WORKER, 'take', iid)
        self.assertIn('execution principal 2, reserved by 1', self.said(iid))
        self.assertEqual(self.gitlab.issues[iid]['assignees'], [{'id': 1}, {'id': 2}])

    def test_fresh_admission_guards(self):
        dep = self.add('--type', 'research')
        cases = {'open dependencies': self.add('--type', 'research', '--runtime', 'claude', '--deps', str(dep)),
                 'host is win': self.add('--type', 'research', '--runtime', 'claude', '--host', 'win'),
                 'runtime is codex': self.add('--type', 'research', '--runtime', 'codex')}
        self.launches()
        for why, iid in cases.items():
            with self.subTest(why):
                self.assertIn(why, self.refused(COORDINATOR, 'spawn', '--name', f'T{iid} t'))
        with patch.object(q, 'CODEX_SOCKET', self.directory / 'none'):
            self.assertIn('no Codex app server', self.refused(COORDINATOR, 'spawn', '--runtime', 'codex', '--name', f'T{cases["runtime is codex"]} t'))
        first, second = (self.add('--type', 'research', '--runtime', 'claude') for _ in range(2))
        with patch.object(q, 'SHARED', {'profile': {'limits': {'claude': 1}}}):
            self.spawn(first)
            self.assertIn('no free claude place', self.refused(COORDINATOR, 'spawn', '--name', f'T{second} t'))
        self.assertEqual(self.spawned, [f'T{first} t (mac-1)'])
        self.assertEqual(self.gitlab.locked(), [first])

    def test_single_project_take_and_plain_sessions_unchanged(self):
        iid = self.add('--type', 'research', '--runtime', 'claude')
        self.launches(session='probe')
        self.do(COORDINATOR, 'spawn', '--name', 'probe session')  # no task in its name: no reservation
        self.assertEqual(self.gitlab.locked(), [])
        self.do(CLAUDE, 'take', iid)
        self.assertEqual((self.state(iid), self.gitlab.issues[iid]['assignees']), ('doing', [{'id': 1}]))
        self.assertNotIn('reservation', self.block(iid))


if __name__ == '__main__':
    unittest.main()
