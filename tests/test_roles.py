"""#240 S1-min: typed task supervisor (block `supervisor`) and same-owner continuation. In-memory tracker, mocked
`claude --bg`: no real session, timer or project is touched. Authority here is cooperative, like claims: a session
identity is self-declared, so these fixtures prove the guards, not a root PM identity."""
import json
import unittest
from unittest.mock import patch

import test_taskq as base  # the in-memory GitLab cycle; run with `discover -s tests`

def job(session, name, cwd=None, state='working', **extra):
    running = {'pid': 4242, 'status': 'busy' if state == 'working' else 'idle'} if state not in ('done', 'failed', 'stopped', 'blocked') else {}
    return {session: {'id': session[:8], 'cwd': str(cwd or q.ROOT), 'kind': 'background', 'startedAt': 1791398637752,
                      'sessionId': session, 'name': name, 'state': state, **running, **extra}}

from test_taskq import COORDINATOR, q, tick, worker

OWNER = {'CLAUDE_CODE_SESSION_ID': '', 'CODEX_THREAD_ID': ''}  # the owner's own shell: no session identity
SUPERVISOR = {'CLAUDE_CODE_SESSION_ID': 'supervisor-1', 'CODEX_THREAD_ID': ''}
SUCCESSOR = {'CLAUDE_CODE_SESSION_ID': 'supervisor-2', 'CODEX_THREAD_ID': ''}
WORKER = {'CLAUDE_CODE_SESSION_ID': 'worker-1', 'CODEX_THREAD_ID': ''}
OTHER = {'CLAUDE_CODE_SESSION_ID': 'intruder', 'CODEX_THREAD_ID': ''}


class Roles(unittest.TestCase):
    setUp, do, refused, state = base.Cycle.setUp, base.Cycle.do, base.Cycle.refused, base.Cycle.state

    def add(self, *extra):
        return base.Cycle.add(self, '--type', 'research', '--runtime', 'claude', *extra)

    def block(self, iid):
        return json.loads(q.BLOCK.search(self.gitlab.issues[iid]['description'])[1])

    def launches(self, session='worker-1'):
        def spawn(name, extra=None, prompt=None, remote_control=True):
            self.agents = {**self.agents, **job(session, name)}
            return session
        self.enterContext(patch.object(worker, 'claude_spawn', spawn))

    def supervised(self, who='supervisor-1'):
        iid = self.add()
        self.do(OWNER, 'edit', iid, '--supervisor', f'claude:{who}')
        return iid

    def started(self):
        """A supervised task whose supervisor launched worker-1 and worker-1 took it."""
        iid = self.supervised()
        self.launches()
        self.do(SUPERVISOR, 'spawn', '--name', f'T{iid} t', '--text', 'go')
        self.do(WORKER, 'take', iid)
        return iid

    def test_only_the_owner_shell_assigns_and_clears(self):
        iid = self.add()
        for who in (SUPERVISOR, COORDINATOR, OTHER):
            self.assertIn("only the owner's shell", self.refused(who, 'edit', iid, '--supervisor', 'claude:supervisor-1'))
        self.assertNotIn('supervisor', self.block(iid))
        self.do(OWNER, 'edit', iid, '--supervisor', 'claude:supervisor-1')
        self.assertEqual(self.block(iid)['supervisor'], {'runtime': 'claude', 'session': 'supervisor-1'})
        self.assertIn('supervisor none → claude:supervisor-1', '\n'.join(self.gitlab.said(iid)))
        self.assertIn("only the owner's shell", self.refused(SUPERVISOR, 'edit', iid, '--supervisor', ''))
        self.do(OWNER, 'edit', iid, '--supervisor', '')
        self.assertNotIn('supervisor', self.block(iid))
        self.assertIn('RUNTIME:SESSION', self.refused(OWNER, 'edit', iid, '--supervisor', 'nobody'))

    def test_only_the_current_supervisor_hands_off(self):
        iid = self.supervised()
        self.assertIn("only the owner's shell", self.refused(COORDINATOR, 'edit', iid, '--supervisor', 'claude:supervisor-2'))
        self.do(SUPERVISOR, 'edit', iid, '--supervisor', 'claude:supervisor-2')
        self.assertEqual(self.block(iid)['supervisor']['session'], 'supervisor-2')
        self.assertIn("only the owner's shell", self.refused(SUPERVISOR, 'edit', iid, '--supervisor', 'claude:supervisor-1'))
        self.do(SUCCESSOR, 'edit', iid, '--supervisor', 'claude:supervisor-1')

    def test_stale_handoff_during_link_is_refused(self):
        """P1: the owner replaces the supervisor while its handoff is inside core.link; the handoff must not win."""
        iid, real, raced = self.supervised(), q.link, []

        def link(target, deps):
            if not raced:
                raced.append(1)
                self.do(OWNER, 'edit', iid, '--supervisor', 'claude:supervisor-2')  # the owner's replacement lands first
            return real(target, deps)
        with patch.object(q, 'link', link):
            self.assertIn('changed while editing', self.refused(SUPERVISOR, 'edit', iid, '--deps', '--supervisor', 'claude:supervisor-3'))
        self.assertEqual(self.block(iid)['supervisor']['session'], 'supervisor-2')
        self.assertNotIn('supervisor-3', '\n'.join(self.gitlab.said(iid)))
        self.do(SUCCESSOR, 'edit', iid, '--deps', '--supervisor', 'claude:supervisor-3')  # the real supervisor still hands off
        self.assertEqual(self.block(iid)['supervisor']['session'], 'supervisor-3')

    def test_supervisor_is_never_the_claim(self):
        iid = self.started()
        self.assertIn('cannot be the claim session', self.refused(OWNER, 'edit', iid, '--supervisor', 'claude:worker-1'))

    def test_only_the_supervisor_launches(self):
        iid = self.supervised()
        self.launches()
        for who in (COORDINATOR, OWNER, OTHER):
            self.assertIn('supervised by claude:supervis', self.refused(who, 'spawn', '--name', f'T{iid} t', '--text', 'go'))
        self.assertEqual((self.gitlab.locked(), self.agents), ([], {}))  # no lock, no session
        self.assertIn(f'supervised by claude:supervis', self.do(COORDINATOR, 'tick'))
        self.assertNotIn(f'T{iid} t', self.do(COORDINATOR, 'tick'))  # no spawn command for it
        self.assertIn('unacknowledged', self.do(OWNER, 'view', iid))
        self.do(SUPERVISOR, 'problem', '--task', iid, '--text', 'a plain note is no acknowledgement')
        self.assertIn('unacknowledged', self.do(OWNER, 'view', iid))
        self.do(SUPERVISOR, 'spawn', '--name', f'T{iid} t', '--text', 'go')
        self.assertIn('supervisor: claude:supervis (acknowledged)', self.do(OWNER, 'view', iid))

    def test_take_only_by_the_launched_worker(self):
        iid = self.supervised()
        self.assertIn('only the worker it launched', self.refused(OTHER, 'take', iid))
        self.assertIn('its supervisor does not take it', self.refused(SUPERVISOR, 'take', iid))
        self.launches()
        self.do(SUPERVISOR, 'spawn', '--name', f'T{iid} t', '--text', 'go')
        self.assertIn('cannot start', self.refused(OTHER, 'take', iid))
        self.do(WORKER, 'take', iid)
        self.assertEqual((self.state(iid), self.block(iid)['claim']['session']), ('doing', 'worker-1'))
        self.assertIn('is yours', self.do(WORKER, 'take', iid))  # idempotent own take

    def test_queue_answer_returns_to_the_same_worker(self):
        iid = self.started()
        self.do(WORKER, 'ask', iid, '--text', 'which?')
        self.assertEqual(self.block(iid)['supervisor']['session'], 'supervisor-1')  # an older-shaped write keeps it
        self.do(COORDINATOR, 'answer', iid, '--text', 'this one')  # #243: the PM relays the owner's answer
        self.assertEqual(self.state(iid), 'ready')
        self.assertIn('only the worker it launched', self.refused(OTHER, 'take', iid))
        self.do(WORKER, 'take', iid)
        self.assertEqual((self.state(iid), self.block(iid)['claim']['session']), ('doing', 'worker-1'))

    def test_in_session_answer_and_owner_decisions_stay(self):
        iid = self.started()
        self.do(WORKER, 'ask', iid, '--text', 'which?')
        self.do(WORKER, 'answer', iid, '--text', 'owner said this one')
        self.assertEqual((self.state(iid), self.block(iid)['claim']['session']), ('doing', 'worker-1'))
        self.do(WORKER, 'ask', iid, '--text', 'again?')
        self.do(OWNER, 'later', iid, '--text', 'not now')
        self.assertEqual(self.state(iid), 'later')

    def test_outside_decisions_and_close(self):
        iid = self.started()
        self.do(WORKER, 'result', iid, '--checks', 'c', '--text', 'done')
        self.assertIn('supervised by', self.refused(COORDINATOR, 'reject', iid, '--text', 'x'))
        self.assertIn('supervised by', self.refused(COORDINATOR, 'close', iid, '--text', 'x'))  # #243 (R3): the PM never closes
        self.do(SUPERVISOR, 'reject', iid, '--text', 'fix')
        self.assertEqual(self.state(iid), 'ready')
        self.assertIn('supervised by', self.refused(COORDINATOR, 'release', iid, '--text', 'x'))
        other = self.supervised()
        self.do(COORDINATOR, 'later', other, '--text', 'x')  # #243: the PM moves tasks for the owner
        self.assertEqual(self.state(other), 'later')
        self.do(WORKER, 'take', iid)
        self.do(WORKER, 'result', iid, '--checks', 'c', '--text', 'fixed')
        self.do(SUPERVISOR, 'close', iid, '--text', 'checked')  # #243 (R3): the supervisor publishes and closes
        self.assertEqual(self.gitlab.issues[iid]['state'], 'closed')

    def test_worker_with_a_claim_continues_its_own_task(self):
        iid = self.started()
        fresh = self.add()
        brief = self.do(WORKER, 'worker', '--limit', 'claude=5')
        self.assertIn(f'it holds #{iid} (doing, your claim). Continue that task', brief)
        self.assertIn(f'task #{iid}', brief)
        self.assertNotIn(f'task #{fresh}', brief)
        self.assertIn(f'task #{fresh}', self.do(OTHER, 'worker', '--limit', 'claude=5'))

    def test_nudge_forbids_fresh_work(self):
        self.assertIn('do not take another task', tick.NUDGE)
        self.assertNotIn('`', tick.NUDGE)  # it goes inside a double-quoted shell argument

    def test_supervisor_count_is_information_deduplicated(self):
        self.started()
        line = 'Supervisors on open tasks: claude 1 (from task metadata; a live supervisor of this machine and its worker'
        self.assertIn(line, self.do(COORDINATOR, 'tick'))
        held = self.add()
        self.do(OWNER, 'edit', held, '--supervisor', 'claude:worker-1')  # also a doing claim: counted there, not twice
        self.assertIn(line, self.do(COORDINATOR, 'tick'))
        self.assertNotIn('enforced by', self.do(COORDINATOR, 'tick'))


if __name__ == '__main__':
    unittest.main()
