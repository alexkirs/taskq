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

    def changed_under_the_lock(self, iid, change):
        """`change` lands between the launch's first read and its lock: the read under the lock refuses."""
        lock, changes = q.lock, [change]

        def locking(number):
            taken = lock(number)
            if changes and number == iid:
                changes.pop()()
            return taken
        with patch.object(q, 'lock', locking):
            refused = self.refused(COORDINATOR, 'spawn', '--name', f'T{iid} t')
        self.assertEqual(self.spawned, [])
        self.assertEqual((iid in self.gitlab.locked(), 'reservation' in self.block(iid)), (False, False))
        return refused

    def test_every_guard_is_checked_again_under_the_lock(self):
        """Review reproduction (P1): an assignment revoked after the first read still launched a worker."""
        self.launches()
        own = self.add('--type', 'research', '--runtime', 'claude', '--mine')
        self.assertIn('delegation needs an owner-verified policy', self.changed_under_the_lock(
            own, lambda: q.api('PUT', f'issues/{own}', {'assignee_ids': [2]})))
        self.assertEqual(self.gitlab.issues[own]['assignees'], [{'id': 2}])
        dep, iid = self.add('--type', 'research'), self.add('--type', 'research', '--runtime', 'claude', '--scope', 'a')
        self.assertIn(f'open dependencies [{dep}]', self.changed_under_the_lock(iid, lambda: q.save(q.task(iid), deps=[dep])))
        q.save(q.task(iid), deps=[])
        rival = self.add('--type', 'research', '--runtime', 'claude', '--scope', 'a/b')
        self.assertIn(f'scope overlaps #{rival}', self.changed_under_the_lock(iid, lambda: self.do(CLAUDE, 'take', rival)))
        q.save(q.task(iid), scope=['c'])
        self.assertIn('host is win', self.changed_under_the_lock(iid, lambda: q.save(q.task(iid), add=[q.ON + 'win'])))
        q.save(q.task(iid), remove=[q.ON + 'win'])
        self.assertIn('runtime is codex', self.changed_under_the_lock(
            iid, lambda: q.save(q.task(iid), add=[q.RUN + 'codex'], remove=[q.RUN + 'claude'])))
        q.save(q.task(iid), add=[q.RUN + 'claude'], remove=[q.RUN + 'codex'])
        with patch.object(q, 'SHARED', {'profile': {'limits': {'claude': 2}}}):  # the rival's claim holds one place
            self.assertIn('no free claude place', self.changed_under_the_lock(
                iid, lambda: self.do({**CLAUDE, 'CLAUDE_CODE_SESSION_ID': 'w2'}, 'take', self.add(
                    '--type', 'research', '--runtime', 'claude'))))
        with patch.object(q, 'SHARED', {'profile': {'limits': {'claude': 9}}}):
            self.assertIn('state is later', self.changed_under_the_lock(iid, lambda: q.save(q.task(iid), 'later')))

    def take_with_change(self, iid, change, hook='lock'):
        """`change` lands between the take's first read and its lock (`hook`: the lock a plain take sets, or the
        lock check of an adopting worker): the read under the lock refuses."""
        real, changes = getattr(q, hook), [change]

        def hooked(number):
            found = real(number)
            if changes and number == iid:
                changes.pop()()
            return found
        with patch.object(q, hook, hooked):
            return self.refused(WORKER, 'take', iid)

    def test_take_checks_every_guard_again_under_the_lock(self):
        """Review reproduction (P1): a take after a revoked assignment went to doing and wrote the old assignee back."""
        own = self.add('--type', 'research', '--runtime', 'claude', '--mine')
        self.assertIn('delegation needs an owner-verified policy', self.take_with_change(
            own, lambda: q.api('PUT', f'issues/{own}', {'assignee_ids': [2]})))
        self.assertEqual((self.state(own), self.gitlab.issues[own]['assignees'], self.gitlab.locked()), ('ready', [{'id': 2}], []))
        dep, iid = self.add('--type', 'research'), self.add('--type', 'research', '--runtime', 'claude')
        for why, change, undo in ((f'open dependencies [{dep}]', dict(deps=[dep]), dict(deps=[])),
                                  ('runtime is codex', dict(add=[q.RUN + 'codex'], remove=[q.RUN + 'claude']), dict(add=[q.RUN + 'claude'], remove=[q.RUN + 'codex'])),
                                  ('host is win', dict(add=[q.ON + 'win']), dict(remove=[q.ON + 'win']))):
            with self.subTest(why):
                self.assertIn(why, self.take_with_change(iid, lambda change=change: q.save(q.task(iid), **change)))
                self.assertEqual((self.state(iid), self.gitlab.locked()), ('ready', []))
                q.save(q.task(iid), **undo)
        self.do(WORKER, 'take', iid)
        self.assertEqual(self.gitlab.issues[iid]['assignees'], [{'id': 1}])  # an unassigned task becomes the taker's

    def test_refused_adoption_keeps_the_attempt_and_its_lock(self):
        self.launches()
        for case, change, why in (
                ('assignment revoked', lambda iid: q.api('PUT', f'issues/{iid}', {'assignee_ids': [2]}), 'delegation needs'),
                ('dependency added', lambda iid: q.save(q.task(iid), deps=[self.add('--type', 'research')]), 'open dependencies'),
                ('attempt replaced', lambda iid: q.save(q.task(iid), reservation={**q.task(iid)['reservation'], 'attempt': 'other'}),
                 'its reservation changed')):
            with self.subTest(case):
                iid = self.add('--type', 'research', '--runtime', 'claude', '--mine')
                self.spawn(iid)
                self.assertIn(why, self.take_with_change(iid, lambda: change(iid), hook='locks'))
                block = self.block(iid)
                self.assertEqual((self.state(iid), 'reservation' in block, iid in self.gitlab.locked()), ('ready', True, True))
                self.assertEqual(self.gitlab.issues[iid]['assignees'], [{'id': 2 if case == 'assignment revoked' else 1}])
                q.save(q.task(iid), 'later', reservation=None)  # the coordinator settles it; the next case starts clean
                q.unlock(iid)

    def test_adopting_an_assigned_task_writes_no_assignee(self):
        self.launches()
        iid = self.add('--type', 'research', '--runtime', 'claude', '--mine')
        self.spawn(iid)
        api, puts = q.api, []

        def recording(method, path, body=None):
            if method == 'PUT':
                puts.append(body)
            return api(method, path, body)
        with patch.object(q, 'api', recording):
            self.do(WORKER, 'take', iid)
        self.assertEqual(self.state(iid), 'doing')
        self.assertFalse(any('assignee_ids' in body for body in puts))

    def test_a_reservation_that_fails_after_its_write_leaves_no_stale_state(self):
        self.launches()
        iid = self.add('--type', 'research', '--runtime', 'claude', '--scope', 'a')
        load, calls = q.load, []

        def failing(*args, **kwargs):  # the rival read after the write fails
            calls.append(1)
            if len(calls) == 3:
                q.fail('GitLab GET issues failed: HTTP 502')
            return load(*args, **kwargs)
        with patch.object(q, 'load', failing):
            self.assertIn('HTTP 502', self.refused(COORDINATOR, 'spawn', '--name', f'T{iid} t'))
        self.assertEqual((self.spawned, self.gitlab.locked(), 'reservation' in self.block(iid)), ([], [], False))
        api = q.api

        def applied_then_failed(method, path, body=None):  # the write landed, its answer was lost
            done = api(method, path, body)
            if method == 'PUT' and 'reservation' in (body or {}).get('description', '') and '"attempt"' in body['description']:
                q.fail('GitLab PUT failed: HTTP 502')
            return done
        with patch.object(q, 'api', applied_then_failed):
            self.refused(COORDINATOR, 'spawn', '--name', f'T{iid} t')
        self.assertEqual((self.spawned, self.gitlab.locked(), 'reservation' in self.block(iid)), ([], [], False))

        def unreadable(method, path, body=None):  # nothing settles it now: the written attempt keeps its lock
            if method == 'GET' and path == f'issues/{iid}' and 'reservation' in self.block(iid):
                q.fail('GitLab GET failed: HTTP 502')
            done = api(method, path, body)
            if method == 'PUT' and 'reservation' in (body or {}).get('description', ''):
                q.fail('GitLab PUT failed: HTTP 502')
            return done
        with patch.object(q, 'api', unreadable):
            self.refused(COORDINATOR, 'spawn', '--name', f'T{iid} t')
        self.assertEqual((self.spawned, self.gitlab.locked(), 'reservation' in self.block(iid)), ([], [iid], True))

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

    def test_restart_settles_dead_now_and_unknown_after_one_stale_window(self):
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
                    ('bound worker unknown here', 'w', {}, False),
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

    def test_only_the_reserving_principal_settles_and_only_with_its_launch_process_known(self):
        foreign = self.reserved(dead_pid())
        self.gitlab.uid = 2  # another user's coordinator on the same machine
        output, spawns = self.tick()
        self.assertIn('reserved by user 1: settled by that user', output)
        self.assertEqual(('reservation' in self.block(foreign), foreign in self.gitlab.locked(), spawns), (True, True, []))
        self.gitlab.uid = 1
        q.save(q.task(foreign), 'later')
        current = q.task(iid := self.reserved(dead_pid()))
        q.save(current, reservation={key: value for key, value in current['reservation'].items() if key != 'pid'})
        output, spawns = self.tick()
        self.assertIn('its launching process is unknown', output)  # no proof the coordinator ended: unknown holds
        self.assertEqual(('reservation' in self.block(iid), iid in self.gitlab.locked(), spawns), (True, True, []))

    def test_release_acts_only_on_the_attempt_it_read(self):
        iid = self.reserved(dead_pid())
        user, replaced = q.user, []

        def replacing():  # another principal's attempt lands after the release read the task
            if not replaced:
                replaced.append(1)
                current = q.task(iid)
                q.save(current, reservation={**current['reservation'], 'attempt': 'replacement', 'principal': 2})
            return user()
        with patch.object(q, 'user', replacing):
            self.assertIn('its reservation changed since this release read it', self.refused(COORDINATOR, 'release', iid, '--text', 'x'))
        self.assertEqual((self.block(iid)['reservation']['attempt'], iid in self.gitlab.locked()), ('replacement', True))
        self.assertIn('only that user releases it', self.refused(COORDINATOR, 'release', iid, '--text', 'x'))
        self.assertFalse(worker.release_reservation({**q.task(iid), 'reservation': {**q.task(iid)['reservation'], 'attempt': 'mine'}}, 'x'))
        self.gitlab.uid = 1  # the exact attempt, read fresh, but of another principal
        self.assertFalse(worker.release_reservation(q.task(iid), 'x'))
        self.assertEqual((self.block(iid)['reservation']['attempt'], iid in self.gitlab.locked()), ('replacement', True))

    def test_release_helper_reads_ownership_after_the_identity(self):
        """Review reproduction (A): a replacement published while the helper asked who it is was cleared."""
        iid = self.reserved(dead_pid())
        read, user, replaced = q.task(iid), q.user, []

        def replacing():
            if not replaced:
                replaced.append(1)
                current = q.task(iid)
                q.save(current, reservation={**current['reservation'], 'attempt': 'replacement', 'principal': 2})
            return user()
        with patch.object(q, 'user', replacing):
            self.assertFalse(worker.release_reservation(read, 'its worker stopped'))
        self.assertEqual((self.block(iid)['reservation']['attempt'], iid in self.gitlab.locked()), ('replacement', True))

    def test_a_refused_take_keeps_a_lock_another_owner_needs(self):
        """Review reproduction (B): a take refused under its own lock unlocked a foreign reservation's task."""
        for case, change, why in (
                ('foreign reservation', dict(reservation={'attempt': 'foreign', 'runtime': 'claude', 'principal': 2, 'node': 'elsewhere'}), 'reserved by'),
                ('foreign worker claim', dict(claim={'runtime': 'claude', 'session': 'foreign-live'}), 'claim still names session foreign-live')):
            with self.subTest(case):
                iid = self.add('--type', 'research', '--runtime', 'claude')
                self.assertIn(why, self.take_with_change(iid, lambda: q.save(q.task(iid), **change)))
                block = self.block(iid)
                self.assertEqual((self.state(iid), iid in self.gitlab.locked()), ('ready', True))
                self.assertEqual({key: block.get(key) for key in change}, change)
                q.save(q.task(iid), 'later')
        iid = self.add('--type', 'research', '--runtime', 'claude')  # an unowned task after the refusal: the lock goes
        self.take_with_change(iid, lambda: q.save(q.task(iid), 'later'))
        self.assertNotIn(iid, self.gitlab.locked())

    def test_a_take_whose_write_failed_keeps_its_lock_by_what_it_reads_back(self):
        api = q.api
        for case, unreadable in (('write applied, answer lost', False), ('task unreadable after the error', True)):
            with self.subTest(case):
                iid, failed = self.add('--type', 'research', '--runtime', 'claude'), []

                def failing(method, path, body=None):
                    if failed and unreadable and method == 'GET' and path == f'issues/{iid}':
                        q.fail('GitLab GET failed: HTTP 502')
                    done = api(method, path, body)
                    if method == 'PUT' and path == f'issues/{iid}' and '"worker-1"' in (body or {}).get('description', '') and not failed:
                        failed.append(1)
                        q.fail('GitLab PUT failed: HTTP 502')
                    return done
                with patch.object(q, 'api', failing):
                    self.assertIn('HTTP 502', self.refused(WORKER, 'take', iid))
                self.assertEqual((self.state(iid), self.block(iid)['claim']['session'], iid in self.gitlab.locked()), ('doing', 'worker-1', True))
                q.save(q.task(iid), 'later', claim=None)

    def test_a_failed_launch_never_clears_a_replaced_attempt(self):
        iid = self.add('--type', 'research', '--runtime', 'claude')

        def replace_then_fail():
            current = q.task(iid)
            q.save(current, reservation={**current['reservation'], 'attempt': 'replacement', 'principal': 2})
            q.fail('claude could not start the session: crashed')
        self.launches(during=replace_then_fail)
        self.agents = {}
        with patch.object(q, 'claude_agents', lambda strict=False: {}):
            self.refused(COORDINATOR, 'spawn', '--name', f'T{iid} t')
        self.assertEqual((self.block(iid)['reservation']['attempt'], iid in self.gitlab.locked()), ('replacement', True))

    def test_a_foreign_reservation_seen_under_the_lock_keeps_that_lock(self):
        iid = self.add('--type', 'research', '--runtime', 'claude')
        self.launches()
        lock, done = q.lock, []

        def foreign_write(number):
            taken = lock(number)
            if not done:
                done.append(1)
                q.save(q.task(iid), reservation={'attempt': 'foreign', 'runtime': 'claude', 'principal': 2, 'node': 'elsewhere'})
            return taken
        with patch.object(q, 'lock', foreign_write):
            self.assertIn('reserved by', self.refused(COORDINATOR, 'spawn', '--name', f'T{iid} t'))
        self.assertEqual((self.spawned, self.block(iid)['reservation']['attempt'], iid in self.gitlab.locked()), ([], 'foreign', True))

    def test_a_foreign_claim_seen_under_the_reservation_lock_keeps_that_lock(self):
        iid = self.add('--type', 'research', '--runtime', 'claude')
        self.launches()
        lock, done = q.lock, []

        def foreign_claim(number):
            taken = lock(number)
            if not done:
                done.append(1)
                q.save(q.task(iid), claim={'runtime': 'claude', 'session': 'foreign-live'})
            return taken
        with patch.object(q, 'lock', foreign_claim):
            self.assertIn('claim still names session foreign-live', self.refused(COORDINATOR, 'spawn', '--name', f'T{iid} t'))
        self.assertEqual((self.spawned, self.block(iid)['claim']['session'], iid in self.gitlab.locked()), ([], 'foreign-live', True))

    def test_a_ready_task_still_naming_a_worker_fails_closed(self):
        iid = self.add('--type', 'research', '--runtime', 'claude')
        q.save(q.task(iid), claim={'runtime': 'claude', 'session': 'foreign-live', 'node': q.node()})
        self.launches()
        self.assertIn('claim still names session foreign-live', self.refused(COORDINATOR, 'spawn', '--name', f'T{iid} t'))
        self.assertIn('claim still names session foreign-live', self.refused(WORKER, 'take', iid))
        self.assertEqual((self.spawned, self.block(iid)['claim']['session'], self.gitlab.locked()), ([], 'foreign-live', []))
        self.do(COORDINATOR, 'release', iid, '--text', 'its worker is gone')  # the legacy placeholder {None, None}
        self.assertEqual(self.block(iid)['claim'], {'runtime': None, 'session': None})
        self.spawn(iid)
        self.do(WORKER, 'take', iid)
        self.assertEqual((self.spawned, self.block(iid)['claim']['session']), ([f'T{iid} t (mac-1)'], 'worker-1'))

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
