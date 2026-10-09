"""taskq.py on an in-memory board: every command, no network."""
import contextlib
import importlib.util
import io
import json
import os
import re
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('taskq_single', ROOT / 'taskq.py')
taskq = importlib.util.module_from_spec(spec)
spec.loader.exec_module(taskq)
SESSION = '0123456789abcdef'
REAL_RUN, REAL_POPEN = subprocess.run, subprocess.Popen  # Base fails any real process; the pull test needs git, the sender loop bash


class FakeBoard:
    """The six board functions over a dict of issues."""

    def __init__(self):
        self.issues = {}

    def list(self, state):
        return [dict(issue) for issue in self.issues.values() if issue['state'] == 'open'
                and any(label == f'q-{state}' if state else label.startswith('q-') for label in issue['labels'])]

    def get(self, n):
        return dict(self.issues[n])

    def add(self, title, body, labels):
        n = len(self.issues) + 1
        self.issues[n] = {'iid': n, 'title': title, 'body': body, 'labels': list(labels), 'state': 'open',
                          'updated_at': '2026-10-09T00:00:00Z', 'url': f'https://board/{n}', 'comments': []}
        return n

    def update(self, n, labels=None, body=None):
        self.issues[n].update({key: value for key, value in (('labels', labels), ('body', body)) if value is not None})

    def comment(self, n, text):
        self.issues[n]['comments'].append(text)

    def close(self, n):
        self.issues[n]['state'] = 'closed'


class FixedNow(taskq.datetime):
    @classmethod
    def now(cls, tz=None):
        return cls(2026, 10, 9, tzinfo=tz)  # the moment FakeBoard stamps: a task is 0 minutes old until a test says otherwise


class Base(unittest.TestCase):
    """Hermetic (#427): a temp root, only these environment variables, a fixed clock, no real process."""

    def setUp(self):
        self.board = taskq.BOARD = FakeBoard()
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.root = Path(folder.name)
        taskq.CONFIG = {'board': 'github', 'publish': 'direct', 'root': self.root, 'hosts': {}}
        real = lambda *_, **__: self.fail('a test started a real process')  # a test fakes subprocess.run where it needs one
        for patcher in (
                mock.patch.dict(os.environ, {'CLAUDE_CODE_SESSION_ID': SESSION, 'TASKQ_RUNTIME': 'claude', 'TASKQ_HOST': 'mac'}, clear=True),
                mock.patch.object(taskq, 'runtimes', return_value={}),  # no real worker from an event's dispatch
                mock.patch.object(taskq, 'start_pass', lambda command, **_: taskq.main(command[2:])),  # the child's pass, in process
                mock.patch.object(taskq, 'datetime', FixedNow),
                mock.patch.object(taskq, 'CLONE', self.root),  # no .git, no taskq.md: tick and wait neither pull nor warn
                mock.patch.object(taskq.subprocess, 'run', real), mock.patch.object(taskq.subprocess, 'Popen', real)):
            patcher.start()
            self.addCleanup(patcher.stop)

    def run_cli(self, *argv):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            taskq.main(list(argv))
        return out.getvalue()

    def task(self, n):
        return taskq.parse(self.board.get(n))

    def add(self, title='T', *extra):
        return self.run_cli('add', title, '--goal', 'g', '--acceptance', 'a', '--scope', 'x.py', *extra)



class Commands(Base):

    def test_add_ready_or_waiting(self):
        self.assertEqual(self.add('one', '--runtime', 'claude', '--priority', '1'), '#1 ready\n')
        self.assertEqual(self.add('two', '--deps', '1'), '#2 waiting\n')
        first = self.task(1)
        self.assertEqual((first['state'], first['runtime'], first['priority'], first['scope'], first['type']),
                         ('ready', 'claude', 1, ['x.py'], 'code'))
        self.assertEqual(self.board.issues[1]['comments'], ['**add** · claude:01234567'])
        self.assertEqual(first['pm'], {'runtime': 'claude', 'session': SESSION, 'name': 'mac'})  # #532: its manager, on the board

    def test_list(self):
        self.add('one')
        self.add('two', '--deps', '1')
        self.run_cli('take', '1')
        self.assertEqual(self.run_cli('list').splitlines(), [
            '#2    waiting  p2 any    two  [open dependencies [1]]',
            '#1    doing    p2 any    one  [claude:01234567 @mac]'])
        self.assertEqual(self.run_cli('list', 'doing').splitlines(), ['#1    doing    p2 any    one  [claude:01234567 @mac]'])

    def test_full_cycle(self):
        self.add()
        self.run_cli('take', '1')
        self.assertEqual(self.task(1)['claim'], {'runtime': 'claude', 'session': SESSION, 'name': 'mac'})
        self.run_cli('ask', '1', '--text', 'which?')
        self.assertEqual(self.task(1)['state'], 'ask')
        self.run_cli('answer', '1', '--text', 'this')
        self.run_cli('result', '1', '--sha', 'a' * 40, '--checks', 'ok', '--text', 'done')
        self.assertEqual((self.task(1)['state'], self.task(1)['result']), ('review', {'sha': 'a' * 40, 'checks': 'ok'}))
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as run:
            self.assertEqual(self.run_cli('close', '1'), '#1 closed\n')
        self.assertIn(['merge-base', '--is-ancestor', 'a' * 40, 'origin/main'], [call.args[0][-4:] for call in run.call_args_list])
        issue = self.board.issues[1]
        self.assertEqual(issue['state'], 'closed')
        self.assertFalse([label for label in issue['labels'] if label.startswith('q-')])
        self.assertEqual([text.split('\n')[0] for text in issue['comments']], [
            f'**{action}** · claude:01234567' for action in ('add', 'take', 'ask', 'answer', 'result', 'close')])
        self.assertEqual(issue['comments'][2], '**ask** · claude:01234567\n\nwhich?')

    def test_close_failure_keeps_the_label(self):
        self.add()
        self.run_cli('take', '1')
        self.run_cli('result', '1', '--sha', 'a' * 40)
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)), \
                mock.patch.object(self.board, 'close', side_effect=RuntimeError('board down')), self.assertRaises(RuntimeError):
            self.run_cli('close', '1')
        self.assertEqual((self.board.issues[1]['state'], self.task(1)['state']), ('open', 'review'))  # #496: still on the board

    def test_close_refuses_commit_not_on_main(self):
        self.add()
        self.run_cli('take', '1')
        self.run_cli('result', '1', '--sha', 'b' * 40)
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1)), \
                self.assertRaisesRegex(SystemExit, 'could not fetch origin'):
            self.run_cli('close', '1')
        taskq.CONFIG['publish'] = 'pr'  # no PR and not on main: publication is still refused
        with mock.patch.object(taskq, 'merge', return_value=None), \
                mock.patch.object(taskq.subprocess, 'run', side_effect=[subprocess.CompletedProcess([], 0), subprocess.CompletedProcess([], 1)]), \
                self.assertRaisesRegex(SystemExit, 'not on origin/main'):
            self.run_cli('close', '1')
        self.assertEqual((self.board.issues[1]['state'], self.task(1)['state']), ('open', 'review'))

    def test_close_removes_clean_worktree_keeps_dirty(self):
        trees, calls = [self.root / '.worktrees' / f'taskq-{n}' for n in (1, 2)], []

        def git(command, **_):  # git after `-C <root>`; taskq-2 has uncommitted changes
            calls.append(command[3:])
            return subprocess.CompletedProcess(command, 0, ' M wip.txt\n' if command[4:6] == [str(trees[1]), 'status'] else '', '')
        for n, tree in enumerate(trees, 1):
            tree.mkdir(parents=True)
            self.add()
            self.run_cli('take', str(n))
            self.run_cli('result', str(n), '--sha', 'a' * 40)
        with mock.patch.object(taskq.subprocess, 'run', side_effect=git):
            self.run_cli('close', '1')
            self.run_cli('close', '2')
        self.assertEqual([call for call in calls if call[0] in ('worktree', 'branch')],
                         [['worktree', 'remove', str(trees[0])], ['branch', '-D', 'taskq-1']])
        self.assertEqual(self.board.issues[1]['comments'][-1], '**close** · claude:01234567')
        self.assertEqual(self.board.issues[2]['comments'][-1],
                         '**close** · claude:01234567\n\nkept .worktrees/taskq-2 and branch taskq-2: uncommitted changes')

    def test_close_external_workspace_removes_nothing(self):
        taskq.CONFIG['workspace'] = 'external'  # #477: the host owns the worktree and the branch
        (self.root / '.worktrees' / 'taskq-1').mkdir(parents=True)
        self.add()
        self.run_cli('take', '1')
        self.run_cli('result', '1', '--sha', 'a' * 40)
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, '', '')) as run:
            self.run_cli('close', '1')
        self.assertFalse([call for call in run.call_args_list if {'worktree', 'branch'} & set(call.args[0])])
        self.assertEqual(self.board.issues[1]['comments'][-1], '**close** · claude:01234567\n\nkept: owned by host')

    def test_requeue_and_later(self):
        self.add()
        self.run_cli('take', '1')
        self.run_cli('requeue', '1', '--text', 'stuck')
        self.assertEqual((self.task(1)['state'], self.task(1)['claim']), ('ready', None))
        self.run_cli('later', '1', '--text', 'after release')
        self.assertEqual(self.run_cli('list').splitlines(), ['#1    later    p2 any    T  [after release]'])

    def test_refusals(self):
        self.add()
        self.add('two', '--deps', '1')
        for argv, message in ((('answer', '1', '--text', 'x'), 'is ready, not ask'), (('result', '1', '--sha', 'HEAD'), 'not a commit')):
            with self.assertRaisesRegex(SystemExit, message):
                self.run_cli(*argv)
        self.board.issues[2]['labels'] = ['q-ready']  # moved by hand with an open dependency
        with self.assertRaisesRegex(SystemExit, 'open dependencies'):
            self.run_cli('take', '2')
        os.environ.pop('CLAUDE_CODE_SESSION_ID')
        with self.assertRaisesRegex(SystemExit, 'agent session'):
            self.run_cli('take', '1')


class DirectPublication(Base):
    """Actual git transport to an isolated bare origin; fake board and exact-SHA CI responses."""

    def git(self, *args, folder=None):
        return REAL_RUN(['git', '-C', str(folder or self.root), '-c', 'user.name=t', '-c', 'user.email=t@t', *args],
                        capture_output=True, text=True, check=True).stdout.strip()

    def setUp(self):
        super().setUp()
        patcher = mock.patch.object(taskq.subprocess, 'Popen', REAL_POPEN)
        patcher.start()
        self.addCleanup(patcher.stop)
        remote = tempfile.TemporaryDirectory()
        self.addCleanup(remote.cleanup)
        self.remote = Path(remote.name)
        self.git('init', '--bare', str(self.remote))
        self.git('init', '-b', 'main')
        self.git('commit', '--allow-empty', '-m', 'base')
        self.base = self.git('rev-parse', 'HEAD')
        self.git('remote', 'add', 'origin', str(self.remote))
        self.git('push', 'origin', 'main')
        self.git('checkout', '-b', 'taskq-1')
        self.git('commit', '--allow-empty', '-m', 'candidate')
        self.sha = self.git('rev-parse', 'HEAD')
        self.git('push', 'origin', 'taskq-1')
        taskq.CONFIG.update(repo='o/r', workspace='external')
        self.add()
        self.run_cli('take', '1')
        self.run_cli('result', '1', '--sha', self.sha, '--checks', 'focused candidate checks')
        os.environ['CLAUDE_CODE_SESSION_ID'] = 'reviewer'
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(self.task(1), 'review', 'spawn', supervisor={'runtime': 'claude', 'session': 'reviewer', 'name': 'mac'})
        self.checks = {'check_runs': [{'status': 'completed', 'conclusion': 'success'}]}

    def close(self):
        with mock.patch.object(taskq.subprocess, 'run', REAL_RUN), mock.patch.object(taskq, 'run_api', return_value=self.checks) as api:
            result = self.run_cli('close', '1', '--text', 'accepted exact diff and evidence')
        self.assertEqual(api.call_args.args[3], f'repos/o/r/commits/{self.sha}/check-runs?check_name=tests')
        return result

    def main_sha(self):
        return self.git('rev-parse', 'refs/heads/main', folder=self.remote)

    def test_acceptance_publishes_exact_candidate_and_answer_still_closes(self):
        self.assertEqual(self.main_sha(), self.base)  # result transfers; it never publishes
        self.assertEqual(self.close(), '#1 closed\n')
        self.assertEqual(self.main_sha(), self.sha)
        self.assertIn(f'published {self.sha}', self.board.issues[1]['comments'][-1])
        self.add('answer', '--type', 'research')
        self.run_cli('take', '2')
        self.run_cli('result', '2', '--sha', self.sha, '--text', 'research answer')
        with mock.patch.object(taskq.subprocess, 'run', REAL_RUN), mock.patch.object(taskq, 'run_api', side_effect=AssertionError('answer needs no CI')):
            self.run_cli('close', '2')
        self.assertEqual(self.main_sha(), self.sha)

    def test_unaccepted_worker_and_other_manager_cannot_publish(self):
        for sid in (SESSION, 'other-manager'):
            with self.subTest(session=sid), mock.patch.dict(os.environ, {'CLAUDE_CODE_SESSION_ID': sid}), \
                    self.assertRaisesRegex(SystemExit, 'supervised by'):
                self.close()
            self.assertEqual(self.main_sha(), self.base)
        # Legacy claims have no supervisor, but their worker still cannot accept a new candidate.
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(self.task(1), 'review', 'spawn', supervisor=None)
        with mock.patch.dict(os.environ, {'CLAUDE_CODE_SESSION_ID': SESSION}), \
                self.assertRaisesRegex(SystemExit, 'only the accepting reviewer'):
            self.close()
        self.assertEqual(self.main_sha(), self.base)

    def test_candidate_mismatch_leaves_review_and_main(self):
        self.git('commit', '--allow-empty', '-m', 'unreviewed')
        self.git('push', 'origin', 'taskq-1')
        with self.assertRaisesRegex(SystemExit, 'does not match result'):
            self.close()
        self.assertEqual((self.task(1)['state'], self.main_sha()), ('review', self.base))

    def test_red_pending_or_missing_ci_never_publishes(self):
        for checks in ([], [{'status': 'queued', 'conclusion': None}], [{'status': 'completed', 'conclusion': 'failure'}]):
            self.checks = {'check_runs': checks}
            with self.subTest(checks=checks), self.assertRaisesRegex(SystemExit, 'CI is not green'):
                self.close()
            self.assertEqual((self.task(1)['state'], self.main_sha()), ('review', self.base))

    def test_main_advance_during_ci_refuses_push_without_losing_work(self):
        self.git('checkout', 'main')
        self.git('commit', '--allow-empty', '-m', 'concurrent main')
        newer = self.git('rev-parse', 'HEAD')

        def ci(*_):
            self.git('push', 'origin', 'main')  # main advances after the ancestry check
            return self.checks
        with mock.patch.object(taskq.subprocess, 'run', REAL_RUN), mock.patch.object(taskq, 'run_api', side_effect=ci), \
                self.assertRaisesRegex(SystemExit, 'rejected'):
            self.run_cli('close', '1')
        self.assertEqual((self.task(1)['state'], self.main_sha()), ('review', newer))

    def test_gitlab_candidate_requires_latest_exact_pipeline(self):
        taskq.CONFIG.update(board='gitlab')
        for pipelines in ([], [{'sha': 'b' * 40, 'status': 'success'}], [{'sha': self.sha, 'status': 'failed'}]):
            with mock.patch.object(taskq.subprocess, 'run', REAL_RUN), mock.patch.object(taskq, 'run_api', return_value=pipelines), \
                    self.assertRaisesRegex(SystemExit, 'CI is not green'):
                self.run_cli('close', '1')
            self.assertEqual(self.main_sha(), self.base)
        with mock.patch.object(taskq.subprocess, 'run', REAL_RUN), \
                mock.patch.object(taskq, 'run_api', return_value=[{'sha': self.sha, 'status': 'success'}]) as api:
            self.run_cli('close', '1')
        self.assertIn(f'pipelines?sha={self.sha}', api.call_args.args[3])
        self.assertEqual(self.main_sha(), self.sha)


class PullRequests(Base):
    """pr mode: `close` drives gh / glab, faked here by the command name."""

    def setUp(self):
        super().setUp()
        taskq.CONFIG.update(publish='pr', repo='o/r')
        self.add()
        self.run_cli('take', '1')
        self.run_cli('result', '1', '--sha', 'a' * 40)
        self.calls, self.prs, self.merged = [], [{'number': 7, 'headRefOid': 'a' * 40, 'baseRefName': 'main'}], True
        self.branches = {}  # branch -> its PRs, for tasks other than #1
        self.checks = {'a' * 40: [[('completed', 'success')]]}

    def cli(self, command, **_):
        self.calls.append(command[1:])
        if command[1] == 'api' and 'merge_requests' in command[4]:  # the GitLab gate (#479): the MR's pipelines, newest first
            return subprocess.CompletedProcess(command, 0, json.dumps([{'sha': sha, 'status': status} for sha, status in self.poll('mr')]), '')
        if command[1] == 'api':  # the 'tests' gate (#308): check runs per SHA
            out = {'check_runs': [{'status': status, 'conclusion': conclusion} for status, conclusion in self.poll(command[4].split('/')[4])]}
            return subprocess.CompletedProcess(command, 0, json.dumps(out), '')
        verb = command[2]
        view = {'state': 'merged', 'squash_commit_sha': 'd' * 40} if command[1] == 'mr' else {'state': 'MERGED', 'mergeCommit': {'oid': 'c' * 40}}
        out = {'list': json.dumps(self.branches.get(command[4], self.prs)), 'view': json.dumps(view if self.merged else {'state': 'OPEN'})}
        return subprocess.CompletedProcess(command, int(verb == 'merge' and not self.merged), out.get(verb, ''), 'Pull request is not mergeable')

    def poll(self, sha):  # one poll of the check runs on sha: the polls in order, the last one repeats
        polls = self.checks[sha]
        return polls.pop(0) if len(polls) > 1 else polls[0]

    def close(self, *numbers):
        with mock.patch.object(taskq.subprocess, 'run', side_effect=self.cli), mock.patch.object(taskq.shutil, 'which', side_effect=lambda name: name), \
                mock.patch.object(taskq.time, 'sleep'):
            return self.run_cli('close', *(numbers or ['1']))

    def merges(self):
        return [call for call in self.calls if call[:2] == ['pr', 'merge']]

    def test_brief(self):
        item = self.task(1)
        prompt = taskq.brief(item, 'claude')
        self.assertIn('`git push --force-with-lease origin HEAD:refs/heads/taskq-1`', prompt)
        self.assertIn('`gh pr create --base main --head taskq-1 --title "<title>" --body "<summary>"`', prompt)
        self.assertIn('--sha <PR head full SHA>', prompt)
        self.assertIn('`git fetch origin && git rebase origin/main`, run the tests required by § 10 Testing policy', prompt)  # #334: up to date before result
        self.assertIn('taskq.md` first and do only what it allows (R13)', prompt)  # #505: spec first
        taskq.CONFIG['board'] = 'gitlab'
        self.assertIn('`glab mr create --yes --target-branch main --source-branch taskq-1', taskq.brief(item, 'claude'))
        taskq.CONFIG['publish'] = 'direct'
        self.assertNotIn('HEAD:main', taskq.brief(item, 'claude'))
        self.assertIn('--sha <candidate full SHA>', taskq.brief(item, 'claude'))
        self.assertIn('git worktree add -b taskq-1 .worktrees/taskq-1', taskq.brief(item, 'claude'))
        taskq.CONFIG['workspace'] = 'external'  # #477
        self.assertNotIn('git worktree add', taskq.brief(item, 'claude'))
        self.assertIn('take your workspace from the project instructions (AGENTS.md) or the path the manager gave', taskq.brief(item, 'claude'))

    def test_close_merges(self):
        self.assertEqual(self.close(), '#1 closed\n')
        self.assertEqual(self.merges(), [['pr', 'merge', '7', '--squash', '--delete-branch', '--match-head-commit', 'a' * 40, '-R', 'o/r']])
        self.assertIn(['api', '-X', 'GET', f'repos/o/r/commits/{"a" * 40}/check-runs?check_name=tests'], self.calls)
        self.assertEqual(self.board.issues[1]['state'], 'closed')
        self.assertEqual(self.board.issues[1]['comments'][-1], f'**close** · claude:01234567\n\nmerged {"c" * 40}')

    def test_close_external_keeps_the_branch(self):
        taskq.CONFIG['workspace'] = 'external'  # #477: no --delete-branch; the repo's own policy decides
        self.close()
        self.assertEqual(self.merges(), [['pr', 'merge', '7', '--squash', '--match-head-commit', 'a' * 40, '-R', 'o/r']])
        self.assertEqual(self.board.issues[1]['comments'][-1], f'**close** · claude:01234567\n\nmerged {"c" * 40}\n\nkept: owned by host')

    def test_close_batch_goes_on_after_a_failure(self):
        self.add()
        self.run_cli('take', '2')
        self.run_cli('result', '2', '--sha', 'b' * 40)
        self.branches['taskq-2'], self.checks['b' * 40] = [{'number': 8, 'headRefOid': 'b' * 40, 'baseRefName': 'main'}], [[('completed', 'success')]]
        self.checks['a' * 40] = [[('completed', 'failure')]]
        with self.assertRaisesRegex(SystemExit, 'not closed: #1$'):
            self.close('1', '2')
        self.assertEqual((self.task(1)['state'], self.board.issues[2]['state']), ('ready', 'closed'))

    def test_head_is_not_the_result(self):
        for change in ({'headRefOid': 'b' * 40}, {'baseRefName': 'release'}):
            self.prs[0].update(change)
            with self.assertRaisesRegex(SystemExit, 'do not match the result'):
                self.close()
            self.prs[0].update(headRefOid='a' * 40, baseRefName='main')
        self.assertEqual((self.task(1)['state'], [call[1] for call in self.calls]), ('review', ['list', 'list']))

    def test_waits_for_tests(self):
        self.checks['a' * 40] = [[], [('queued', None)], [('completed', 'success'), ('in_progress', None)], [('completed', 'success')]]
        self.close()
        self.assertEqual((len(self.merges()), self.board.issues[1]['state']), (1, 'closed'))
        self.assertEqual(sum('check-runs' in call[3] for call in self.calls if call[0] == 'api'), 4)

    def test_failed_tests_requeue(self):
        self.checks['a' * 40] = [[('completed', 'failure')]]
        with self.assertRaisesRegex(SystemExit, f'PR 7 check tests failed on {"a" * 40}'):
            self.close()
        self.assertEqual((self.task(1)['state'], self.merges()), ('ready', []))

    def test_behind_merges_without_update(self):  # #359: the fake has no update-branch; any call to it would fail the merge
        for n, sha in ((2, 'b'), (3, 'f')):
            self.add()
            self.run_cli('take', str(n))
            self.run_cli('result', str(n), '--sha', sha * 40)
            self.branches[f'taskq-{n}'] = [{'number': n + 6, 'headRefOid': sha * 40, 'baseRefName': 'main'}]
            self.checks[sha * 40] = [[('completed', 'success')]]
        self.assertEqual(self.close('2', '3', '1'), '#2 closed\n#3 closed\n#1 closed\n')
        self.assertEqual([call[6] for call in self.merges()], ['b' * 40, 'f' * 40, 'a' * 40])
        self.assertFalse([call for call in self.calls if 'update-branch' in call or 'compare' in ' '.join(call)])

    def test_failed_check_returns_to_the_supervisor(self):
        # § 7 Supervisor 3.4 (#525): a supervised task stays with its supervisor: doing, no worker, no result, for a rework
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(self.task(1), 'review', 'result', supervisor={'runtime': 'claude', 'session': SESSION, 'name': 'mac'})
        self.checks['a' * 40] = [[('completed', 'failure')]]
        with self.assertRaisesRegex(SystemExit, f'PR 7 check tests failed on {"a" * 40}'):
            self.close()
        one = self.task(1)
        self.assertEqual((one['state'], one['supervisor']['session'], one['claim']['session'], one['result']), ('doing', SESSION, None, None))
        self.assertEqual(self.merges(), [])

    def test_refusal_requeues(self):
        self.merged = False
        with self.assertRaisesRegex(SystemExit, 'PR 7 did not merge: Pull request is not mergeable'):
            self.close()
        self.assertEqual((self.task(1)['state'], self.task(1)['claim']), ('ready', None))
        self.assertIn('close: PR 7 did not merge: Pull request is not mergeable', self.board.issues[1]['comments'][-1])

    def gitlab(self):
        taskq.CONFIG.update(board='gitlab', host='git.example')
        self.prs = [{'iid': 7, 'sha': 'a' * 40, 'target_branch': 'main'}]

    def test_gitlab_waits_for_pipeline_and_merges_at_sha(self):
        self.gitlab()
        self.checks['mr'] = [[], [('a' * 40, 'running'), ('b' * 40, 'success')], [('a' * 40, 'success')]]
        self.assertEqual(self.close(), '#1 closed\n')
        self.assertEqual(self.calls[0], ['mr', 'list', '--source-branch', 'taskq-1', '--output', 'json', '-R', 'https://git.example/o/r'])
        self.assertEqual(sum(call[:4] == ['api', '-X', 'GET', 'projects/o%2Fr/merge_requests/7/pipelines'] and call[-2:] == ['--hostname', 'git.example']
                             for call in self.calls), 3)
        self.assertEqual([call for call in self.calls if call[:2] == ['mr', 'merge']],
                         [['mr', 'merge', '7', '--squash', '--remove-source-branch', '--sha', 'a' * 40, '--auto-merge=false', '--yes', '-R', 'https://git.example/o/r']])
        self.assertEqual(self.board.issues[1]['comments'][-1], f'**close** · claude:01234567\n\nmerged {"d" * 40}')

    def test_gitlab_failed_pipeline_requeues(self):
        self.gitlab()
        self.checks['mr'] = [[('a' * 40, 'failed')]]
        with self.assertRaisesRegex(SystemExit, f'PR 7 pipeline failed on {"a" * 40}'):
            self.close()
        self.assertEqual((self.task(1)['state'], [call for call in self.calls if call[:2] == ['mr', 'merge']]), ('ready', []))

    def test_gitlab_conflict_requeues(self):
        self.gitlab()
        self.checks['mr'], self.merged = [[('a' * 40, 'success')]], False
        with self.assertRaisesRegex(SystemExit, 'PR 7 did not merge: Pull request is not mergeable'):
            self.close()
        self.assertEqual((self.task(1)['state'], self.task(1)['claim']), ('ready', None))

    def test_review_mode(self):
        with tempfile.TemporaryDirectory() as folder:
            Path(folder, 'taskq.json').write_text('{"publish": "review"}')
            with self.assertRaisesRegex(SystemExit, 'use "direct" or "pr"'):
                taskq.load_config(folder)


class FakeRuntime:
    """The runtime functions over a dict: session -> True running, False gone, 'idle' (a supervisor between turns), None."""

    def __init__(self):
        self.sessions, self.sent, self.stopped, self.prompts = {}, [], [], {}

    def spawn(self, name, prompt, cwd):
        self.names = [*getattr(self, 'names', []), name]
        tag = name.split()[0]
        count = sum(other.split()[0] == tag for other in self.names[:-1])  # a respawn gets a new id: s-S1, s-S1.1
        session = f's-{tag}' + (f'.{count}' if count else '')
        self.sessions[session], self.prompt, self.prompts[session] = True, prompt, prompt
        return session

    def send(self, session, text):
        self.sent.append((session, text))
        return session

    def alive(self, session):
        return self.sessions.get(session)

    def state(self, session):
        return {True: 'running', False: 'dead', 'idle': 'idle'}.get(self.sessions.get(session))

    def link(self, session):
        return f'https://watch/{session}'

    def retire(self, gone, running=True):
        for session, alive in list(self.sessions.items()):
            if gone(int(re.match(r's-[TS](\d+)', session)[1]), session, alive) and (running or alive is False):
                self.stopped.append(session)
                del self.sessions[session]


class TickSetup(Base):
    """Fixture only, no tests (#534, § 10): a fake runtime and a manager for Tick and Wait."""

    def setUp(self):
        super().setUp()
        self.fake = FakeRuntime()
        taskq.CONFIG.update(limits={'fake': 1}, repo='o/r')
        patcher = mock.patch.object(taskq, 'runtimes', return_value={'fake': self.fake})
        patcher.start()
        self.addCleanup(patcher.stop)
        self.manager('fake')  # this test session files the tasks: their manager; supervisors run in its runtime

    def manager(self, runtime, sid=SESSION, name='mac'):
        """The tasks added from now on record this manager as their `pm` (R3, #532)."""
        patcher = mock.patch.object(taskq, 'origin', return_value={'runtime': runtime, 'session': sid, 'name': name})
        patcher.start()
        self.addCleanup(patcher.stop)

    def unmanaged(self):
        """Tasks added from now on have no manager: they wait, no supervisor starts (R3)."""
        patcher = mock.patch.object(taskq, 'origin', return_value=None)
        patcher.start()
        self.addCleanup(patcher.stop)

    def legacy(self, n, session=None):
        """Task n as an unsupervised worker's (R3 Transition: started before #525 or taken by hand)."""
        session = session or f's-T{n}'
        self.fake.sessions[session] = True
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(self.task(n), 'doing', 'take', claim={'runtime': 'fake', 'session': session, 'name': 'mac'})

    def acting(self, session):
        """Run the next commands as that agent session (a supervisor or a worker)."""
        return mock.patch.dict(os.environ, {'CLAUDE_CODE_SESSION_ID': session})

    def notes(self, n):
        return [text.split(' ·')[0] for text in self.board.issues[n]['comments']]


class Tick(TickSetup):

    def test_spawn_one_per_free_slot(self):
        # #525 (R2, R3): a ready task gets one supervisor in the manager's runtime; its worker's slot is held from then on
        self.add('one')
        self.add('two')
        self.add('elsewhere', '--host', 'win', '--priority', '1')
        out = self.run_cli('tick')
        one = self.task(1)
        self.assertEqual((one['state'], one['supervisor'], one['claim']), ('doing', {'runtime': 'fake', 'session': 's-S1', 'name': 'mac'},
                                                                         {'runtime': 'fake', 'session': None, 'name': 'mac'}))
        self.assertEqual((self.task(2)['state'], self.task(3)['state'], self.fake.names), ('ready', 'ready', ['S1 CLD one (mac)']))
        self.assertIn('You are the taskq supervisor S1 of task #1: one', self.fake.prompts['s-S1'])
        self.assertIn('**spawn** · claude:01234567\n\nsupervisor s-S1\nhttps://watch/s-S1', self.board.issues[1]['comments'])
        self.assertIn('| [#1](https://board/1) | doing | fake | [s-S1](https://watch/s-S1) |', out)
        self.assertTrue(out.endswith('Board: https://github.com/o/r/issues\n'))
        with self.acting('s-S1'):
            self.run_cli('run', '1')  # the supervisor orders; its event pass spawns the worker
        self.assertEqual((self.task(1)['claim']['session'], self.fake.names[-1]), ('s-T1', 'T1 CLD one (mac)'))
        self.assertIn('taskq worker for task #1: one', self.fake.prompts['s-T1'])
        self.assertEqual(self.notes(1), ['**add**', '**spawn**', '**run**', '**spawn**'])
        self.assertTrue(self.board.issues[1]['comments'][-1].endswith('worker s-T1\nhttps://watch/s-T1'))
        self.fake.sessions.update({'s-T1': False, 's-S1': 'idle'})  # the worker died, the supervisor sleeps between turns
        self.run_cli('tick')
        self.assertEqual((self.task(1)['state'], self.task(1)['claim']['session'], self.task(2)['state']), ('doing', None, 'ready'))
        self.assertIn('worker s-T1 is gone', self.board.issues[1]['comments'][-1])
        self.assertEqual(self.fake.sent, [('s-S1', 'gone #1: read your issue')])  # woken once
        self.run_cli('tick')
        self.assertEqual((len(self.fake.sent), self.fake.names), (1, ['S1 CLD one (mac)', 'T1 CLD one (mac)']))

    def test_row_links_per_runtime(self):
        claude, codex = taskq.Claude(), taskq.Codex()
        item = {'iid': 7, 'url': 'https://board/7', 'state': 'doing', 'runtime': 'any',
                'claim': {'runtime': 'codex', 'session': '019a-thread', 'name': 'mac'}}
        kinds = {'claude': claude, 'codex': codex}
        self.assertEqual(taskq.row(item, kinds, 'mac'),
                         '| [#7](https://board/7) | doing | codex | [019a-thr](https://alexkirs.github.io/taskq/open.html#codex://threads/019a-thread) |')
        item['claim'] = {'runtime': 'claude', 'session': 'abcdef12-3456', 'name': 'mac'}
        with mock.patch.object(claude, 'link', return_value='https://claude.ai/code/session_X'):
            self.assertEqual(taskq.row(item, kinds, 'mac'), '| [#7](https://board/7) | doing | claude | [abcdef12](https://claude.ai/code/session_X) |')
        self.assertEqual(taskq.row(item, kinds, 'win'), '| [#7](https://board/7) | doing | claude | abcdef12 on mac |')
        self.assertEqual(taskq.row({**item, 'claim': None, 'state': 'ready'}, kinds, 'mac'), '| [#7](https://board/7) | ready | any |  |')

    def test_row_codex_link_per_source_client(self):
        """#521 (R6): a tick run in Codex links Codex threads directly; Claude, a shell or an unknown client get the wrapper."""
        claude, codex = taskq.Claude(), taskq.Codex()
        kinds = {'claude': claude, 'codex': codex}
        item = {'iid': 7, 'url': 'https://board/7', 'state': 'doing', 'runtime': 'any',
                'claim': {'runtime': 'codex', 'session': '019a-thread-full-id', 'name': 'mac'}}
        wrapper = '| [#7](https://board/7) | doing | codex | [019a-thr](https://alexkirs.github.io/taskq/open.html#codex://threads/019a-thread-full-id) |'
        sources = {'codex': {'CODEX_THREAD_ID': 'mgr'}, 'claude': {'CLAUDE_CODE_SESSION_ID': 'mgr'}, 'shell': {},
                   'claude from codex': {'CLAUDE_CODE_SESSION_ID': 'mgr', 'CODEX_THREAD_ID': 'x', 'TASKQ_RUNTIME': 'claude'},
                   'unknown runtime': {'CODEX_THREAD_ID': 'x', 'TASKQ_RUNTIME': 'hermes'}}
        for source, env in sources.items():
            with self.subTest(source), mock.patch.dict(os.environ, env, clear=True):
                self.assertEqual(taskq.row(item, kinds, 'mac'), wrapper.replace(
                    'https://alexkirs.github.io/taskq/open.html#', '') if source == 'codex' else wrapper)
                self.assertEqual(taskq.row(item, kinds, 'win'), '| [#7](https://board/7) | doing | codex | 019a-thr on mac |')
                self.assertEqual(taskq.row({**item, 'claim': None, 'state': 'ready'}, kinds, 'mac'), '| [#7](https://board/7) | ready | any |  |')
                claimed = {**item, 'claim': {'runtime': 'claude', 'session': 'abcdef12-3456', 'name': 'mac'}}
                with mock.patch.object(claude, 'link', return_value='https://claude.ai/code/session_X'):
                    self.assertEqual(taskq.row(claimed, kinds, 'mac'), '| [#7](https://board/7) | doing | claude | [abcdef12](https://claude.ai/code/session_X) |')

    def test_second_quick_death_asks(self):
        # #393: an unsupervised worker that dies at once is requeued once, then the owner is asked with the last log line
        self.unmanaged()
        self.fake.tail = lambda session: 'error: unsupported model'
        self.add()
        self.legacy(1)
        self.fake.sessions['s-T1'] = False
        self.run_cli('tick')
        self.assertEqual(self.task(1)['state'], 'ready')
        self.legacy(1, 's-T1b')
        self.fake.sessions['s-T1b'] = False
        out = self.run_cli('tick')
        self.assertEqual(self.task(1)['state'], 'ask')
        self.assertIn('| [#1](https://board/1) | ask |', out)
        self.assertIn('Last log line: error: unsupported model', self.board.issues[1]['comments'][-1])
        self.run_cli('answer', '1', '--text', 'fixed')  # an answer resets the count: the next death requeues
        self.run_cli('tick')
        self.assertEqual(self.task(1)['state'], 'ready')
        self.assertIn('| [#1](https://board/1) | ready (no manager) | any |  |', self.run_cli('tick'))

    def test_two_passes_at_once_spawn_one_worker(self):
        # #357 (R2): a tick runs while an event pass spawns; it finds the lock held and starts nothing
        taskq.CONFIG['limits'] = {'fake': 2}
        spawn, err = self.fake.spawn, io.StringIO()

        def overlapping(*spawn_args):
            with contextlib.redirect_stderr(err):
                self.run_cli('tick')
            return spawn(*spawn_args)
        self.fake.spawn = overlapping
        self.add('one')  # the add's pass spawns #1's supervisor, and the tick runs at that moment
        self.assertEqual(self.fake.names, ['S1 CLD one (mac)'])
        self.assertIn('another pass is running', err.getvalue())
        self.fake.spawn = spawn
        stale = [dict(self.board.issues[1], labels=['q-ready'])]  # a list from before the spawn: the re-read sees #1 taken
        with mock.patch.object(self.board, 'list', return_value=stale):
            self.run_cli('tick')
        self.assertEqual(self.fake.names, ['S1 CLD one (mac)'])

    def test_nudge_only_a_silent_worker(self):
        self.unmanaged()
        self.add()
        self.legacy(1)
        self.board.issues[1]['updated_at'] = taskq.datetime.now(taskq.timezone.utc).isoformat()
        self.run_cli('tick')
        self.assertEqual(self.fake.sent, [])
        self.board.issues[1]['updated_at'] = '2026-01-01T00:00:00Z'
        self.run_cli('tick')
        self.assertEqual(self.fake.sent, [('s-T1', 'continue: read your issue')])
        self.assertEqual(self.board.issues[1]['comments'][-1], '**nudge** · claude:01234567\n\nworker s-T1')
        self.fake.sessions['s-T1'] = None  # cannot tell: left alone
        self.run_cli('tick')
        self.assertEqual((self.task(1)['state'], len(self.fake.sent)), ('doing', 1))

    def test_answer_reaches_the_asking_worker_once(self):
        self.unmanaged()
        self.add()
        self.legacy(1)
        self.board.issues[1]['updated_at'] = taskq.datetime.now(taskq.timezone.utc).isoformat()
        self.run_cli('ask', '1', '--text', 'which?')
        self.run_cli('answer', '1', '--text', 'the first')
        self.run_cli('tick')
        self.run_cli('tick')
        self.assertEqual(self.fake.sent, [('s-T1', 'The owner answered your question:\n\nthe first')])
        self.assertEqual(self.task(1)['state'], 'doing')

    def test_decisions_block_and_answer_by_codes(self):
        # #490: cards for an ask and a review with options; one line of codes answers both
        self.unmanaged()
        for n, title in enumerate(('one', 'two', 'three'), 1):
            self.add(title)
            self.legacy(n)
        self.run_cli('ask', '1', '--text', 'Built A and B.\nWhich one?', '--option', 'keep A', '--option', 'keep B',
                     '--recommend', '2', '--link', 'https://x/shot.png', '--link', 'https://x/demo.mp4')
        self.run_cli('result', '2', '--sha', 'a' * 40, '--text', 'Done X', '--option', 'close as is', '--option', 'also do Y')
        self.run_cli('result', '3', '--sha', 'a' * 40, '--text', 'plain')  # no options: no card
        out = self.run_cli('tick').split('Decisions (answer: taskq answer N.K ...):\n')[1]
        self.assertEqual(out.splitlines(), [
            '[#1](https://board/1) ask: Built A and B. · ![1](https://x/shot.png) · https://x/demo.mp4 · 1.1 keep A · 1.2 keep B (recommended)',
            '[#2](https://board/2) review: Done X · 2.1 close as is (recommended) · 2.2 also do Y'])
        taskq.CONFIG['inline_media'] = False
        self.assertIn(' · https://x/shot.png · ', self.run_cli('tick'))
        for bad, message in (('1.3', 'no option 3'), ('3.1', 'no option 1'), ('1.x', 'codes like'), ('9', 'codes like')):
            with self.assertRaisesRegex(SystemExit, message):
                self.run_cli('answer', '1.2', bad)
        self.assertEqual(self.task(1)['state'], 'ask')  # a bad code moves nothing
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, '', '')):
            self.run_cli('answer', '1.2, 2.1')
        self.assertEqual((self.task(1)['state'], self.task(1)['raw']['decision'], self.board.issues[2]['state']), ('doing', None, 'closed'))
        self.assertEqual(self.board.issues[2]['comments'][-1], '**close** · claude:01234567\n\n2.1: close as is')
        self.assertEqual(self.fake.sent[-1], ('s-T1', 'The owner answered your question:\n\n1.2: keep B'))
        self.assertNotIn('Decisions', self.run_cli('tick'))
        with self.assertRaisesRegex(SystemExit, 'pick 1 to 1'):
            self.run_cli('ask', '1', '--text', 'q', '--option', 'a', '--recommend', '2')

    def test_waiting_becomes_ready_and_runs(self):
        self.add('one')
        self.add('two', '--deps', '1')
        self.run_cli('later', '1')
        self.board.issues[1]['state'] = 'closed'
        self.run_cli('tick')
        self.assertEqual(self.task(2)['state'], 'doing')
        self.assertIn('**ready** · claude:01234567\n\ndependencies closed', self.board.issues[2]['comments'])
        taskq.CONFIG['limits'] = {'fake': 0}
        self.add('three')
        self.assertNotIn('spawn', self.run_cli('tick'))

    def test_assignee_starts_only_matching_tasks(self):
        # #480: "assignee" set: tick starts, and tick/wait/list show, only tasks assigned to it; unassigned ones are skipped
        self.board.user = lambda: 'alice'
        taskq.CONFIG.update(assignee='me', limits={'fake': 3})
        for title, people in (('nobody', []), ('other', ['bob']), ('mine', ['alice'])):
            self.add(title)  # its event pass sees no assignee yet: starts nothing
            self.board.issues[len(self.board.issues)]['assignees'] = people
        out = self.run_cli('tick')
        self.assertEqual([self.task(n)['state'] for n in (1, 2, 3)], ['ready', 'ready', 'doing'])
        self.assertNotIn('#1 ', out)
        self.assertEqual([line[:2] for line in self.run_cli('list').splitlines()], ['#3'])
        self.run_cli('later', '1')
        with contextlib.redirect_stdout(io.StringIO()):  # unsupervised reviews reach the manager's wait
            taskq.move(self.task(3), 'review', 'result', supervisor=None, result={'sha': 'a' * 40})
        self.board.update(2, labels=['q-review'])
        self.assertEqual(self.run_cli('wait', '--window', '0'), 'review #3\n')
        taskq.CONFIG['assignee'] = 'bob'
        self.run_cli('requeue', '3')
        self.board.update(2, labels=['q-ready'])
        self.run_cli('tick')
        self.assertEqual([self.task(n)['state'] for n in (2, 3)], ['doing', 'ready'])
        del taskq.CONFIG['assignee']  # unset: every task, as before
        self.run_cli('requeue', '1')
        self.run_cli('tick')
        self.assertEqual([self.task(n)['state'] for n in (1, 2, 3)], ['doing', 'doing', 'doing'])

    def test_worker_name_has_task_launcher_title_machine(self):
        self.add()
        with mock.patch.dict(os.environ, {'TASKQ_RUNTIME': 'codex'}):
            self.run_cli('tick')
        self.assertRegex(self.fake.names[0], r'^S1 (CLD|CDX) \S.* \(mac\)$')
        with self.acting('s-S1'):
            self.run_cli('run', '1')
        self.assertEqual(self.fake.names[1], 'T1 CLD T (mac)')

    def test_events_spawn_the_next_task_at_once(self):
        # #333 (R4): add, run, result, requeue, close and answer each run the pass once; a full slot spawns nothing
        spawns = lambda: [name.split()[0] for name in getattr(self.fake, 'names', [])]
        self.add('one')
        self.add('two')
        self.assertEqual(spawns(), ['S1'])  # add: #1's supervisor takes the worker slot; #2 finds it full
        with self.acting('s-S1'):
            self.run_cli('run', '1')
        with self.acting('s-T1'):
            self.run_cli('result', '1', '--sha', 'a' * 40)
        self.assertEqual(spawns(), ['S1', 'T1'])  # a review still holds the slot: the supervisor decides
        with self.acting('s-S1'):
            self.run_cli('requeue', '1', '--text', 'fix X')  # rework: a new worker at once
        self.assertEqual(spawns(), ['S1', 'T1', 'T1'])
        self.assertIn('fix X', self.fake.prompts['s-T1.1'])  # the brief carries the fixes
        self.add('three')
        self.run_cli('requeue', '1')  # the manager drops it: supervisor and slot freed, #1 first again
        self.assertEqual(spawns(), ['S1', 'T1', 'T1', 'S1'])
        self.assertEqual(self.task(1)['supervisor']['session'], 's-S1.1')
        with self.acting('s-S1.1'):
            self.run_cli('run', '1')
            self.run_cli('ask', '1', '--text', 'which?')  # ask is no event: the slot stays held
        self.run_cli('answer', '1', '--text', 'this')  # the live worker gets the answer, the slot stays full
        self.assertEqual(spawns(), ['S1', 'T1', 'T1', 'S1', 'T1'])
        self.assertEqual(self.fake.sent, [('s-T1.2', 'The owner answered your question:\n\nthis')])
        with self.acting('s-T1.2'):
            self.run_cli('result', '1', '--sha', 'b' * 40)
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)), self.acting('s-S1.1'):
            self.run_cli('close', '1', '--text', 'checked')  # frees the slot: #2 starts in the same pass
        self.assertEqual(spawns(), ['S1', 'T1', 'T1', 'S1', 'T1', 'S2'])
        self.run_cli('tick')  # the safety net finds nothing left to do
        self.assertEqual(spawns(), ['S1', 'T1', 'T1', 'S1', 'T1', 'S2'])

    def test_event_during_a_pass_is_not_lost(self):
        self.add()
        calls = []
        real = taskq.one_pass
        def first(args, table=True):
            calls.append(table)
            if len(calls) == 1:
                (taskq.CONFIG['root'] / '.taskq').mkdir(exist_ok=True)
                (taskq.CONFIG['root'] / '.taskq' / 'dispatch.pending').touch()  # an event came while we held the lock
            return real(args, table)
        with mock.patch.object(taskq, 'one_pass', first):
            taskq.cmd_tick(None, table=False)
        self.assertEqual(len(calls), 2)

    def test_arm_tick_without_target_arms_this_session(self):
        out = self.run_cli('arm', 'tick')
        self.assertIn('background command', out)
        self.assertIn('wait', out)
        # #525: one start, then the queue runs by itself; no sender is required
        self.assertIn('Start: run one pass now', out)
        self.assertIn('no sender, timer or extension', out)
        self.assertNotIn('separate sender', out)

    def test_arm_tick_prints_the_codex_compact_line(self):
        out = self.run_cli('arm', 'tick')
        self.assertIn('codex -c model_auto_compact_token_limit=200000 -c "compact_prompt=\\"Keep only the owner\'s open '
                      'questions and decisions; the board is the state.\\""', out)

    def test_brief_carries_the_requeue_reason(self):
        self.add()
        self.run_cli('requeue', '1', '--text', 'fix test X')
        self.assertIn('fix test X', taskq.brief(self.task(1), 'fake'))

    def test_no_event_pass_inside_a_codex_sandbox(self):
        with mock.patch.dict(os.environ, {'CODEX_SANDBOX': 'seatbelt'}):
            self.add()
        self.assertEqual(self.task(1)['state'], 'ready')

    def test_codex_sandbox_never_calls_a_live_worker_gone(self):  # #502
        self.unmanaged()
        self.add()
        self.legacy(1)
        self.fake.sessions['s-T1'] = False  # the sandbox cannot see the worker's process
        with mock.patch.dict(os.environ, {'CODEX_SANDBOX': 'seatbelt'}), mock.patch.object(taskq.time, 'sleep'):
            self.run_cli('tick')
            self.assertNotIn('gone', self.run_cli('wait', '--window', '0'))
        self.assertEqual(self.task(1)['state'], 'doing')
        self.assertNotIn('**requeue**', ' '.join(self.board.issues[1]['comments']))

    def test_event_survives_a_failed_dispatch(self):
        self.fake.spawn = lambda *_: taskq.fail('claude could not start the session')
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            self.assertEqual(self.add(), '#1 ready\n')
        self.assertIn('dispatch stopped: claude could not start the session', err.getvalue())

    def test_event_returns_before_the_detached_spawn(self):
        # #405 (R4): add starts a detached `tick --quiet` with its output in .taskq/dispatch.log and returns; the spawn happens there
        started = []
        with mock.patch.object(taskq, 'start_pass', lambda command, **options: started.append((command, options))):
            self.assertEqual(self.add(), '#1 ready\n')
        [(command, options)] = started
        self.assertEqual((command[1:], options['cwd'], options['stdout'].name, options['stdin']),
                         ([str(ROOT / 'taskq.py'), 'tick', '--quiet', '--tasks', '1'], self.root, str(self.root / '.taskq' / 'dispatch.log'), subprocess.DEVNULL))
        self.assertTrue(options.get('start_new_session') or options.get('creationflags'))  # detached: it outlives the event
        self.assertEqual((self.task(1)['state'], getattr(self.fake, 'names', [])), ('ready', []))  # nothing spawned in this process

    def test_claude_send_keeps_name_and_spawn_flags(self):
        claude, calls = taskq.Claude(), []
        claude.start = lambda arguments, cwd: calls.append(arguments) or 'new'
        claude.agents = lambda: {'s1': {'name': 'T7', 'cwd': '/w'}}
        claude.spawn('T7', 'brief', '/w')
        claude.send('s1', 'continue')
        spawned, resumed = calls
        self.assertEqual(resumed, ['--resume', 's1', *spawned[:-1], 'continue'])
        self.assertEqual(resumed[2:4], ['--name', 'T7'])

    def test_close_stops_only_a_local_worker(self):
        self.unmanaged()
        for name in ('mac', 'win'):
            self.add()
            n = len(self.board.issues)
            self.legacy(n)
            claim = {**self.task(n)['claim'], 'name': name}
            taskq.move(self.task(n), 'review', 'result', claim=claim, result={'sha': 'a' * 40})
            with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)):
                self.run_cli('close', str(n))
        self.assertEqual(self.fake.stopped, ['s-T1', 's-T2'])  # the fake lists both here; on a real machine only its own
        self.assertNotIn('stop it there', self.board.issues[1]['comments'][-1])
        self.assertEqual(self.board.issues[2]['comments'][-1], '**close** · claude:01234567\n\nsession s-T2 runs on win: stop it there')

    def test_tick_removes_stopped_sessions_of_closed_tasks(self):
        # R11 (#525): by recorded id only: a stopped session its closed task records goes; a name alone or a running one stays
        for title in ('one', 'two', 'three'):
            self.add(title)
        for n in (2, 3):
            self.board.issues[n]['state'] = 'closed'
        self.board.issues[2]['comments'].append('**spawn** · fake:x\n\nworker s-T2')
        self.fake.sessions.update({'s-T2': False, 's-T3': False, 's-T8': True})
        self.run_cli('tick')
        self.assertEqual(self.fake.stopped, ['s-T2'])
        self.assertEqual(set(self.fake.sessions), {'s-S1', 's-T3', 's-T8'})

    def test_claude_retire_stops_and_removes_every_session_of_the_task(self):
        claude, calls = taskq.Claude(), []
        claude.agents = lambda: {
            'a': {'id': 'ja', 'name': 'T5 CLD fix (mac)', 'pid': 1, 'state': 'working'},
            'b': {'id': 'jb', 'name': 'T5 CLD fix (mac)', 'state': 'stopped'},
            'c': {'id': 'jc', 'name': 'T6 CLD other (mac)', 'state': 'stopped'},
            'd': {'id': 'jd', 'name': 'T5 notes', 'state': 'stopped'}}  # the owner's own job
        with mock.patch.object(taskq.subprocess, 'run', side_effect=lambda command, **_: calls.append(command[1:])):
            claude.retire(lambda n, *_: n == 5)
            self.assertEqual(calls, [['stop', 'ja'], ['rm', 'ja'], ['rm', 'jb']])
            calls.clear()
            claude.retire(lambda n, *_: n != 6, running=False)
            self.assertEqual(calls, [['rm', 'jb']])

    def test_codex_retire_archives_the_task_threads(self):
        with tempfile.TemporaryDirectory() as folder:
            taskq.CONFIG['root'] = Path(folder)
            (Path(folder) / '.taskq').mkdir()
            for n in (3, 4):
                (Path(folder) / '.taskq' / f'T{n}.pid').write_text(f'999999999 thread-{n}')
            calls = []
            with mock.patch.object(taskq.subprocess, 'run', side_effect=lambda command, **_: calls.append(command[1:])):
                taskq.Codex().retire(lambda n, *_: n == 3)
            self.assertEqual(calls, [['archive', 'thread-3']])
            self.assertEqual([path.name for path in (Path(folder) / '.taskq').iterdir()], ['T4.pid'])

    def test_codex_alive_from_pid_file(self):
        codex = taskq.Codex()
        (codex.folder() / 'T1.pid').write_text(f'{os.getpid()} thread-1')
        (codex.folder() / 'T2.pid').write_text('999999999 thread-2')  # above any pid_max: no such process
        self.assertEqual([codex.alive(s) for s in ('thread-1', 'thread-2', 'thread-3')], [True, False, None])
        self.assertTrue(codex.link('thread-1').endswith('/open.html#codex://threads/thread-1'))

    def test_codex_spawn_takes_the_newest_thread(self):
        # #495: a respawn appends to T1.log; the old thread id above must not win
        codex = taskq.Codex()
        log = codex.folder() / 'T1.log'
        log.write_text('{"type":"thread.started","thread_id":"old-thread"}\n')

        def run(name, arguments, cwd):
            with open(log, 'a') as out:
                out.write('{"type":"thread.started","thread_id":"new-thread"}\n')
            return mock.Mock(pid=os.getpid()), log
        with mock.patch.object(codex, 'exec', run):
            self.assertEqual(codex.spawn('T1 one (mac)', 'prompt', '.'), 'new-thread')
        self.assertEqual((codex.folder() / 'T1.pid').read_text(), f'{os.getpid()} new-thread')

    def test_supervisor_runtime_follows_the_manager(self):
        # R3 (#525): S<N> runs in the manager's runtime (DOT: Codex), T<N> in the task's; no manager here: the task waits
        lead = FakeRuntime()
        self.manager('dot')
        with mock.patch.object(taskq, 'runtimes', return_value={'fake': self.fake, 'codex': lead}):
            self.add('one', '--runtime', 'fake')
            with self.acting('s-S1'):
                self.run_cli('run', '1')
            self.assertEqual((lead.names, self.fake.names), (['S1 CLD one (mac)'], ['T1 CLD one (mac)']))
            self.assertEqual((self.task(1)['supervisor']['runtime'], self.task(1)['claim']), ('codex', {'runtime': 'fake', 'session': 's-T1', 'name': 'mac'}))
            self.manager('hermes')  # a manager whose runtime has no file here
            taskq.CONFIG['limits'] = {'fake': 2}
            self.add('two')
            self.assertEqual((self.task(2)['state'], len(lead.names)), ('ready', 1))
            self.assertIn('| [#2](https://board/2) | ready (no manager) | any |  |', self.run_cli('tick'))

    def test_one_controller_under_duplicate_events(self):
        # R3 (#525): run and close come from the supervisor, the manager or the owner; repeats spawn nothing more
        self.add('one')
        for command in (('run', '1'), ('requeue', '1')):
            with self.acting('s-T9'), self.assertRaisesRegex(SystemExit, 'only it, the task.s manager or the owner controls it'):
                self.run_cli(*command)
        with self.acting(''):  # the owner's shell
            self.run_cli('run', '1')
        with self.acting('s-S1'), self.assertRaisesRegex(SystemExit, 'has a worker: s-T1'):
            self.run_cli('run', '1')
        for _ in range(3):
            self.run_cli('tick')
        self.assertEqual(self.fake.names, ['S1 CLD one (mac)', 'T1 CLD one (mac)'])
        with self.acting('s-T1'):
            self.run_cli('result', '1', '--sha', 'a' * 40)
            with self.assertRaisesRegex(SystemExit, 'only it, the task.s manager or the owner'):  # the worker never closes its own task
                self.run_cli('close', '1')
        with self.assertRaisesRegex(SystemExit, 'has no supervisor'):
            self.add('two')
            self.legacy(2)
            self.run_cli('run', '2')

    def test_worker_requeue_wakes_its_supervisor_and_rework_is_bounded(self):
        # § 7 Supervisor: the worker's requeue keeps the supervisor; an idle one gets each event once, a running one none yet
        self.add('one')
        with self.acting('s-S1'):
            self.run_cli('run', '1')
        with self.acting('s-T1'):
            self.run_cli('requeue', '1', '--text', 'stuck')
        one = self.task(1)
        self.assertEqual((one['state'], one['claim']['session'], one['supervisor']['session'], len(self.fake.names)), ('doing', None, 's-S1', 2))
        self.run_cli('tick')
        self.assertEqual(self.fake.sent, [])  # running: it reads the issue at its next wake
        self.fake.sessions['s-S1'] = 'idle'
        self.run_cli('tick')
        self.run_cli('tick')
        self.assertEqual(self.fake.sent, [('s-S1', 'requeue #1: read your issue')])
        with self.acting('s-S1'):
            self.run_cli('requeue', '1', '--text', 'try B')  # its own requeue: no event for it
            self.run_cli('requeue', '1', '--text', 'try C')
            with self.assertRaisesRegex(SystemExit, 'third rework: ask the owner'):
                self.run_cli('requeue', '1', '--text', 'try D')
            self.run_cli('ask', '1', '--text', 'B and C fail: which?')
        self.assertEqual(([name.split()[0] for name in self.fake.names], self.task(1)['state']), (['S1', 'T1', 'T1', 'T1'], 'ask'))
        self.run_cli('answer', '1', '--text', 'try D')  # an answer resets the bound
        with self.acting('s-S1'):
            self.run_cli('requeue', '1', '--text', 'try D')
        self.assertEqual(len(self.fake.names), 5)
        self.assertIn(('s-S1', 'answer #1: read your issue'), self.fake.sent)
        self.assertEqual([text for _, text in self.fake.sent].count('requeue #1: read your issue'), 1)  # its own requeues woke nothing

    def test_supervisor_death_respawns_then_asks(self):
        # § 7 step 4: the first death respawns S<N> (a resumable one resumes), the second since the last result or answer asks
        self.fake.tail = lambda session: 'error: rate limit'
        self.add('one')
        with self.acting('s-S1'):
            self.run_cli('run', '1')
        self.fake.sessions['s-S1'] = False
        self.run_cli('tick')
        self.assertIn('**gone** · claude:01234567\n\nsupervisor s-S1 is gone: error: rate limit', self.board.issues[1]['comments'])
        self.assertEqual((self.task(1)['supervisor']['session'], self.task(1)['claim']['session'], self.fake.stopped), ('s-S1.1', 's-T1', ['s-S1']))
        self.fake.sessions['s-S1.1'] = False
        self.run_cli('tick')
        self.assertEqual(self.task(1)['state'], 'ask')
        self.assertIn('supervisor s-S1.1 is gone again', self.board.issues[1]['comments'][-1])
        self.fake.resumable = lambda session: True  # Codex: exec resume keeps the thread
        self.run_cli('answer', '1', '--text', 'fixed')  # resets the bound; its pass recovers the supervisor
        self.assertEqual((self.task(1)['supervisor']['session'], self.fake.sent[-1]), ('s-S1.1', ('s-S1.1', 'restart #1: your last turn ended error: rate limit; read your issue')))

    def test_self_waking_supervisor_is_never_sent_to(self):
        # #525 owner clarification: the queue wakes the supervisor, no sender. A Claude one with its process reads its own
        # `wait --task N`; once its process ended the pass resumes it (a new id, recorded; the old one is refused)
        self.fake.SELF_WAKE = True
        self.add('one')
        self.assertIn('wait --task 1` in the background', self.fake.prompts['s-S1'])
        with self.acting('s-S1'):
            self.run_cli('run', '1')
        with self.acting('s-T1'):
            self.run_cli('result', '1', '--sha', 'a' * 40)
        self.assertEqual(self.fake.sent, [])  # its process runs: its own wait reads the event
        with self.acting('s-S1'):
            self.assertEqual(self.run_cli('wait', '--task', '1', '--window', '0'), 'review #1\n')
            self.assertEqual(self.run_cli('wait', '--task', '1', '--window', '0'), 'tick\n')  # once
        with self.acting('s-S2'):
            self.assertEqual(self.run_cli('wait', '--task', '1', '--window', '0'), 'stop #1\n')  # not its task
        with self.acting('s-S1'):
            self.run_cli('requeue', '1', '--text', 'fix X')
        with self.acting('s-T1.1'):
            self.run_cli('result', '1', '--sha', 'b' * 40)
        self.fake.sessions['s-S1'] = 'idle'  # its turn ended, the process is gone
        self.fake.send = lambda session, text: self.fake.sent.append((session, text)) or 's-S1r'
        self.run_cli('tick')
        self.assertEqual(self.fake.sent, [('s-S1', 'review #1: read your issue')])
        self.assertEqual((self.task(1)['supervisor']['session'], self.board.issues[1]['comments'][-1]),
                         ('s-S1r', '**nudge** · claude:01234567\n\nsupervisor s-S1r replaces s-S1'))
        with self.acting('s-S1'), self.assertRaisesRegex(SystemExit, 'only it, the task.s manager or the owner'):
            self.run_cli('close', '1')
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)), self.acting('s-S1r'):
            self.assertEqual(self.run_cli('wait', '--task', '1', '--window', '0'), 'tick\n')  # already delivered
            self.run_cli('close', '1', '--text', 'ok')
            self.assertEqual(self.run_cli('wait', '--task', '1', '--window', '0'), 'stop #1\n')
        self.fake.sessions['s-S1'] = False  # the resume stopped the old job
        self.run_cli('tick')
        self.assertIn('s-S1', self.fake.stopped)  # the replaced id, once stopped
    def test_codex_and_claude_supervisor_state(self):
        # § 7 step 4: Codex idle = exited after turn.completed with a local rollout; Claude dead = unlisted or no pid
        codex, home = taskq.Codex(), self.root / 'codex'
        (home / 'sessions' / '2026' / '10' / '09').mkdir(parents=True)
        (home / 'sessions' / '2026' / '10' / '09' / 'rollout-x-th-1.jsonl').write_text('')
        for name, pid, log in (('S1', os.getpid(), ''), ('S2', 999999999, '{"type":"turn.started"}\n{"type":"turn.completed"}\n'),
                               ('S3', 999999999, '{"type":"turn.completed"}\n{"type":"turn.started"}\n{"type":"turn.failed"}\n')):
            (codex.folder() / f'{name}.pid').write_text(f'{pid} th-{name[1]}')
            (codex.folder() / f'{name}.log').write_text(log)
        with mock.patch.dict(os.environ, {'CODEX_HOME': str(home)}):
            self.assertEqual([codex.state(f'th-{n}') for n in (1, 2, 3, 4)], ['running', 'dead', 'dead', 'dead'])  # th-2: no rollout
            (home / 'sessions' / '2026' / '10' / '09' / 'rollout-x-th-2.jsonl').write_text('')
            self.assertEqual((codex.state('th-2'), codex.resumable('th-2'), codex.resumable('th-3')), ('idle', True, False))
        claude = taskq.Claude()
        agents = {'a': {'pid': 1, 'state': 'working'}, 'b': {'pid': 1, 'state': 'done'}, 'c': {'state': 'done'}, 'd': {'state': 'failed'}}
        with mock.patch.object(claude, 'agents', return_value=agents):
            self.assertEqual([claude.state(s) for s in 'abcde'], ['running', 'running', 'idle', 'dead', 'dead'])
        with mock.patch.object(claude, 'agents', return_value=None):
            self.assertIsNone(claude.state('a'))

    def test_busy_codex_supervisor_gets_the_result_when_its_turn_ends(self):
        # #525: the result lands while the supervisor's turn runs; the pass at that turn's end (`tick --quiet --after PID`)
        # resumes it with the event: no sender, no timer
        self.add('one')
        with self.acting('s-S1'):
            self.run_cli('run', '1')
        with self.acting('s-T1'):
            self.run_cli('result', '1', '--sha', 'a' * 40)
        self.assertEqual(self.fake.sent, [])
        self.fake.sessions['s-S1'] = 'idle'
        with mock.patch.object(taskq, 'pid_alive', side_effect=[True, False]), mock.patch.object(taskq.time, 'sleep') as slept:
            self.run_cli('tick', '--quiet', '--after', '4242')
        self.assertEqual((slept.call_count, self.fake.sent), (1, [('s-S1', 'review #1: read your issue')]))
        self.run_cli('tick')
        self.assertEqual(len(self.fake.sent), 1)  # once

    def test_codex_turn_starts_a_pass_at_its_end(self):
        # #525: every `codex exec` turn (spawn or resume) gets one detached pass that waits for its pid
        started = []
        with mock.patch.object(taskq.subprocess, 'Popen', return_value=mock.Mock(pid=4242)), \
                mock.patch.object(taskq.shutil, 'which', return_value='codex'), \
                mock.patch.object(taskq, 'start_pass', lambda command, **_: started.append(command[2:])):
            taskq.Codex().exec('S1 CDX one (mac)', ['resume', 'th', 'review #1: read your issue'], self.root)
        self.assertEqual(started, [['tick', '--quiet', '--after', '4242', '--tasks']])
        self.assertTrue((self.root / '.taskq' / 'dispatch.log').read_text().endswith('turn end pid 4242\n'))

    def test_recorded_session_cannot_take_the_manager_role(self):
        # R3 (#525): `taskq pm` would let a supervisor or worker pass the gate as the manager; it is refused
        self.add('one')
        self.run_cli('tick')
        with self.acting(''):
            self.run_cli('run', '1')
        self.run_cli('tick')
        for sid in ('s-S1', 's-T1'):
            with self.acting(sid), self.assertRaisesRegex(SystemExit, 'cannot take the manager role'):
                self.run_cli('pm')
        self.assertFalse((self.root / '.taskq' / 'pm.json').exists())  # refused: nothing written


class Wait(TickSetup):
    """#407: `taskq wait` returns once per event, or 'tick' after the window; the clock is patched.
    #534: no longer reruns Tick's tests; none of them reads this clock but three `wait --window 0` calls (no sleep)."""

    def setUp(self):
        super().setUp()
        self.clock = [0.0]
        for name, fake in (('time', lambda: self.clock[0]), ('sleep', lambda s: self.clock.__setitem__(0, self.clock[0] + s))):
            patcher = mock.patch.object(taskq.time, name, fake)
            patcher.start()
            self.addCleanup(patcher.stop)

    def test_events_once_then_tick(self):
        self.unmanaged()  # unsupervised tasks (R3 Transition): the manager reviews, so wait prints review
        self.add()
        self.legacy(1)
        self.assertEqual(self.run_cli('wait'), 'tick\n')  # nothing happened: the safety window
        self.assertEqual(self.clock[0], 600)
        self.run_cli('result', '1', '--sha', 'a' * 40)
        self.add('two')
        self.legacy(2)
        self.fake.sessions['s-T2'] = False
        self.assertEqual(self.run_cli('wait').splitlines(), ['review #1', 'gone #2'])
        self.assertEqual(self.clock[0], 600)  # at once, no sleep
        self.assertEqual(self.run_cli('wait'), 'tick\n')  # never twice for the same event
        self.run_cli('requeue', '1')
        self.legacy(1, 's-T1b')
        start = self.clock[0]
        result = lambda: self.run_cli('result', '1', '--sha', 'b' * 40) if self.clock[0] == start + 50 else None
        with mock.patch.object(taskq.time, 'sleep', lambda s: (self.clock.__setitem__(0, self.clock[0] + s), result())):
            self.assertEqual(self.run_cli('wait'), 'review #1\n')  # back in review while waiting: a new event
        self.assertEqual(self.clock[0], start + 50)

    def test_arm_tick_names_target_and_loop(self):
        out = self.run_cli('arm', 'tick', 'PM main')
        self.assertIn('manager session PM main', out)
        self.assertIn("taskq.py wait --pm 'PM main'`", out)  # #532: a name matches no task's pm, and the prompt says so
        self.assertIn('taskq: no open task records PM main as its pm', out)
        self.assertIn('with SendMessage', out)

    def codex_home(self, folder, thread):
        home = self.root / 'codex'
        (home / folder).mkdir(parents=True)
        (home / folder / f'rollout-2026-10-09T07-46-53-{thread}.jsonl').write_text('')
        return {'TASKQ_RUNTIME': 'codex', 'CODEX_HOME': str(home)}

    def test_arm_tick_in_codex_names_resume(self):
        """#522: a local thread (rollout under sessions/) gets exec resume; the shell loop stops on a failed send."""
        with mock.patch.dict(os.environ, {**self.codex_home('sessions/2026/10/09', 'T1'), 'CODEX_THREAD_ID': 'T1'}):
            sender, self_arm = self.run_cli('arm', 'tick', 'T1'), self.run_cli('arm', 'tick')
        self.assertIn('resume T1 "<its output>"', sender)
        self.assertIn('resume T1 "$e"; do :; done; echo "taskq sender stopped"', sender)
        self.assertNotIn('send_message_to_thread', sender)
        self.assertIn('in the foreground', self_arm)
        self.assertIn('Optional, only to be woken between turns: `', self_arm)
        self.assertIn('arm tick T1`', self_arm)
        self.assertNotIn('Before you end a turn', self_arm)
        self.assertIn('no sender, timer or extension', self_arm)

    def test_arm_tick_shell_loop_stops_on_failed_wait_or_send(self):
        """#522: the printed shell loop sends each event once and stops on the first failed wait or send."""
        with mock.patch.dict(os.environ, self.codex_home('sessions/2026/10/09', 'T1')):
            sender = self.run_cli('arm', 'tick', 'T1')
        self.assertIn('Stay in this one turn and repeat', sender)
        self.assertIn('A failed wait, a failed send or no such send tool: stop', sender)
        loop = sender.split('No agent needed: `', 1)[1].split('` in a terminal', 1)[0]
        wait = f'python3 {Path(taskq.__file__).resolve()} wait'
        bin_, sent = self.root / 'bin', self.root / 'sent'
        bin_.mkdir()
        (bin_ / 'codex').write_text(f'#!/bin/sh\necho "$@" >> {sent}\nexit ${{SEND_EXIT:-0}}\n')
        (bin_ / 'codex').chmod(0o755)
        events = self.root / 'events'
        for send_exit, expect in (('0', 2), ('1', 1)):  # wait fails on its 3rd run; a failed send stops at once
            events.write_text('0')
            fake_wait = f'sh -c \'n=$(cat {events}); echo $((n+1)) > {events}; [ $n -lt 2 ] && echo "review #$n"\''
            sent.write_text('')
            with mock.patch.object(taskq.subprocess, 'Popen', REAL_POPEN):
                run = REAL_RUN(['bash', '-c', loop.replace(wait, fake_wait)], capture_output=True, text=True, timeout=10,
                               env={**os.environ, 'PATH': f'{bin_}:/usr/bin:/bin', 'SEND_EXIT': send_exit})
            self.assertEqual(run.stdout, 'taskq sender stopped\n')
            self.assertEqual(len(sent.read_text().splitlines()), expect)

    def test_arm_tick_in_codex_unknown_target_promises_no_wake(self):
        """#522: no local rollout is not proof of an app thread: no resume, no promised wake, the app sender only if known."""
        with mock.patch.dict(os.environ, {'TASKQ_RUNTIME': 'codex', 'CODEX_HOME': str(self.root / 'none')}):
            sender = self.run_cli('arm', 'tick', 'APP1')
        self.assertIn('no local Codex rollout of APP1, so no `codex exec resume` and no promised wake. Unknown what it is', sender)
        self.assertIn('Only if APP1 is a known Codex app thread: run this prompt in an independent', sender)
        self.assertIn('Not a collaboration subagent of the manager', sender)
        self.assertIn('Send its output, verbatim, to APP1 with `send_message_to_thread`', sender)
        self.assertIn('never retry, never another route', sender)
        self.assertNotIn('resume APP1', sender)
        self.assertNotIn('while ', sender)

    def test_arm_tick_in_codex_archived_thread_has_no_route(self):
        """#522: exec resume of an archived thread is unverified: no sender prompt, unarchive first."""
        with mock.patch.dict(os.environ, self.codex_home('archived_sessions', 'OLD1')):
            out = self.run_cli('arm', 'tick', 'OLD1')
        self.assertIn('OLD1 is archived in Codex', out)
        self.assertIn('`codex unarchive OLD1`', out)
        self.assertNotIn('tick sender', out)

    def test_dispatch_chains_without_a_tick_and_wait_wakes_once(self):
        """#522 (R4), #525: events dispatch with no periodic tick; the manager is woken once per close, never for a supervised review."""
        passes, start = [], taskq.start_pass
        with mock.patch.object(taskq, 'start_pass', lambda command, **o: passes.append(command[2:]) or start(command, **o)):
            self.add('one')
            self.add('two')
            with self.acting('s-S1'):
                self.run_cli('run', '1')
            with self.acting('s-T1'):
                self.run_cli('result', '1', '--sha', 'a' * 40)
        self.assertTrue(passes and all(command[:2] == ['tick', '--quiet'] for command in passes))  # only event passes
        self.assertEqual(self.fake.names, ['S1 CLD one (mac)', 'T1 CLD one (mac)'])  # one spawn each
        self.assertEqual((self.task(1)['state'], self.task(2)['state']), ('review', 'ready'))
        self.assertEqual(self.run_cli('wait'), 'tick\n')  # a supervised review is the supervisor's
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)), self.acting('s-S1'):
            self.run_cli('close', '1', '--text', 'diff and CI on aaaaaaa checked\n\nmore')
        self.assertEqual(self.run_cli('wait'), 'closed #1 diff and CI on aaaaaaa checked\n')
        self.assertEqual(self.run_cli('wait'), 'tick\n')  # never sent twice
        self.assertEqual(self.fake.names[-1], 'S2 CLD two (mac)')  # the close freed the slot
        self.assertEqual((self.fake.stopped, self.fake.sessions['s-S1']), (['s-T1'], True))  # close never stops the session that runs it
        self.fake.sessions['s-S1'] = False  # its turn ended: the next pass retires it
        self.run_cli('tick')
        self.assertEqual(self.fake.stopped, ['s-T1', 's-S1'])


class MultiPM(Base):
    """#532 (R1, R3, R4): a Codex and a Claude manager share one checkout and board; each task's `pm` is the authority."""
    A, B = {'CODEX_THREAD_ID': 'pmA-codex', 'TASKQ_RUNTIME': 'codex'}, {'CLAUDE_CODE_SESSION_ID': 'pmB-claude', 'TASKQ_RUNTIME': 'claude'}
    legacy, acting, notes = TickSetup.legacy, TickSetup.acting, TickSetup.notes

    def setUp(self):
        super().setUp()
        self.fake, self.cdx, self.cld, clock = FakeRuntime(), FakeRuntime(), FakeRuntime(), [0.0]
        taskq.CONFIG.update(limits={'fake': 3}, repo='o/r')
        for patcher in (mock.patch.object(taskq, 'runtimes', return_value={'fake': self.fake, 'codex': self.cdx, 'claude': self.cld}),
                        mock.patch.object(taskq.time, 'time', lambda: clock[0]),  # wait's window passes at once
                        mock.patch.object(taskq.time, 'sleep', lambda pause: clock.__setitem__(0, clock[0] + pause))):
            patcher.start()
            self.addCleanup(patcher.stop)
        (self.root / 'taskq.md').write_text('## Principles\nx\n## 7. Manager\ny\n## 8. Runtimes\n')

    def pm(self, env):
        """Run the next commands as that manager session ({}: a plain shell)."""
        return mock.patch.dict(os.environ, {'TASKQ_HOST': 'mac', **env}, clear=True)

    def test_second_manager_neither_reroutes_nor_takes_the_gate(self):
        # the review repro: PM_A Codex adds, PM_B Claude runs `taskq pm`, PM_A ticks: the task keeps a Codex S and PM_A's gate
        with mock.patch.object(taskq, 'dispatch'):  # the add's own pass comes later, as in the repro
            with self.pm(self.A):
                self.add('one', '--runtime', 'fake')
            with self.pm(self.B):
                self.run_cli('pm')
            with self.pm(self.A):
                self.run_cli('tick')
            self.assertEqual((self.cdx.names, getattr(self.cld, 'names', [])), (['S1 CDX one (mac)'], []))
            self.assertEqual(self.task(1)['supervisor']['runtime'], 'codex')
            with self.pm(self.B), self.assertRaisesRegex(SystemExit, 'only it, the task.s manager or the owner'):
                self.run_cli('run', '1')
            with self.pm(self.A):
                self.run_cli('run', '1')  # its own manager passes the gate
        self.assertEqual(self.task(1)['raw']['order'], 'run')
        self.assertNotIn('runtime', json.loads((self.root / '.taskq' / 'pm.json').read_text()))

    def test_two_managers_route_and_wait_independently(self):
        # simultaneous managers: each task keeps its pm's supervisor runtime; each wait gets its own outcomes, once
        with self.pm(self.A):
            self.add('one', '--runtime', 'fake')
        with self.pm(self.B):
            self.add('two', '--runtime', 'fake')
        with self.pm({}):  # a plain shell with no TASKQ_RUNTIME: no pm, the task waits (R3 Transition)
            self.add('three', '--runtime', 'fake')
        self.legacy(3)
        self.assertEqual((self.cdx.names, self.cld.names), (['S1 CDX one (mac)'], ['S2 CLD two (mac)']))
        for env in (self.A, self.B):
            with self.pm(env):
                self.assertEqual(self.run_cli('wait'), 'tick\n')
        for sid in ('s-S1', 's-S2'):
            with self.acting(sid):
                self.run_cli('run', sid[-1])
        with self.acting('s-T1'):
            self.run_cli('result', '1', '--sha', 'a' * 40)
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)), self.acting('s-S1'):
            self.run_cli('close', '1', '--text', 'checked')
        with self.acting('s-T2'):
            self.run_cli('ask', '2', '--text', 'which?')
        with self.acting('s-T3'):
            self.run_cli('result', '3', '--sha', 'b' * 40)
        with self.pm(self.A):
            self.assertEqual(self.run_cli('wait'), 'review #3\nclosed #1 checked\n')
        with self.pm(self.B):
            self.assertEqual(self.run_cli('wait'), 'ask #2\nreview #3\n')
        for env in (self.A, self.B):
            with self.pm(env):
                self.assertEqual(self.run_cli('wait'), 'tick\n')  # never twice, never the other's

    def test_duplicate_events_on_a_shared_task_spawn_one_worker(self):
        # both managers tick a shared task; a list from before the spawn still shows the order: the re-read wins (one T<N>)
        with self.pm(self.A):
            self.add('one', '--runtime', 'fake')
        with mock.patch.object(taskq, 'dispatch'):
            with self.acting('s-S1'):
                self.run_cli('run', '1')
            stale = [dict(self.board.issues[1])]
            with self.pm(self.A):
                self.run_cli('tick')
            for env in (self.B, self.A):
                with self.pm(env), mock.patch.object(self.board, 'list', return_value=stale):
                    self.run_cli('tick')
        self.assertEqual((self.cdx.names, self.fake.names), (['S1 CDX one (mac)'], ['T1 CDX one (mac)']))

    def test_adoption_is_explicit_and_keeps_claims(self):
        # a task with no pm waits; one manager adopts it; the other is refused; a started task keeps its sessions
        with self.pm({}):
            self.add('one', '--runtime', 'fake')
            self.add('two', '--runtime', 'fake')
        self.legacy(2)
        with self.pm(self.A):
            self.assertIn('| [#1](https://board/1) | ready (no manager) |', self.run_cli('tick'))
        claim = self.task(2)['claim']
        with self.pm(self.B):
            self.run_cli('pm', '--adopt', '1', '2')
        with self.pm(self.A), self.assertRaisesRegex(SystemExit, 'cannot adopt: #1 .claude:pmB-clau. already has a manager'):
            self.run_cli('pm', '--adopt', '1')
        self.assertEqual((self.task(1)['pm']['session'], self.task(2)['claim']), ('pmB-claude', claim))
        self.assertEqual(self.board.issues[1]['comments'][-1], '**adopt** · claude:pmB-clau\n\npm claude:pmB-claude')
        with self.pm(self.A):
            self.run_cli('tick')
        self.assertEqual((getattr(self.cdx, 'names', []), self.cld.names), ([], ['S1 CDX one (mac)']))  # A's pass starts B's task in B's runtime (ORCH: the launcher)

    def test_overlapping_adoptions_never_overwrite(self):
        # the review's interleaving: B adopts while A is between its read and its write. A holds the dispatch lock across
        # both, so B is refused with nothing written (a later B meets A's pm: test_adoption_is_explicit_and_keeps_claims)
        with self.pm({}):
            self.add('one', '--runtime', 'fake')
        get, refused = self.board.get, []

        def get_then_b(n):  # B's adoption runs inside A's, right after A read the task
            issue = get(n)
            if not refused:
                with self.pm(self.B), self.assertRaisesRegex(SystemExit, 'holds .taskq/dispatch.lock; nothing adopted'):
                    refused.append(self.run_cli('pm', '--adopt', '1'))
            return issue
        with self.pm(self.A), mock.patch.object(self.board, 'get', get_then_b):
            self.run_cli('pm', '--adopt', '1')
        self.assertEqual(self.task(1)['pm']['session'], 'pmA-codex')
        self.assertEqual(self.notes(1), ['**add**', '**adopt**'])  # one adoption, B wrote nothing
    def test_concurrent_adoption_clis_one_wins(self):
        # bounded real evidence: two `taskq pm --adopt 1` processes at once on a file board with a slow read; exactly one wins
        (self.root / 'taskq.json').write_text('{"board": "board.py", "limits": {"claude": 0, "codex": 0}}')
        (self.root / 'board.py').write_text(textwrap.dedent('''
            import json, time
            from pathlib import Path
            FILE = Path(__file__).with_name('issues.json')
            def load(): return json.loads(FILE.read_text())
            def save(issues): FILE.write_text(json.dumps(issues))
            def list(state): return [i for i in load().values() if i['state'] == 'open']
            def get(n):
                time.sleep(0.5)  # a slow board widens the race window
                return load()[str(n)]
            def add(title, body, labels): raise SystemExit('no add')
            def update(n, labels=None, body=None):
                issues = load(); issues[str(n)].update({k: v for k, v in (('labels', labels), ('body', body)) if v is not None}); save(issues)
            def comment(n, text):
                issues = load(); issues[str(n)]['comments'].append(text); save(issues)
            def close(n): pass
        '''))
        with self.pm({}):
            self.add('one', '--runtime', 'fake')
        (self.root / 'issues.json').write_text(json.dumps({'1': {**self.board.issues[1], 'iid': 1}}))
        script = Path(taskq.__file__).resolve()
        procs = [REAL_POPEN([sys.executable, str(script), 'pm', '--adopt', '1'], cwd=self.root, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, env={'PATH': os.environ.get('PATH', '/usr/bin:/bin'),
                            'TASKQ_HOST': 'mac', 'HOME': str(self.root), **env}) for env in (self.A, self.B)]
        outs = [proc.communicate(timeout=30) for proc in procs]
        codes = sorted(proc.returncode for proc in procs)
        issue = json.loads((self.root / 'issues.json').read_text())['1']
        winners = [env for env, proc in zip((self.A, self.B), procs) if proc.returncode == 0]
        self.assertEqual(codes, [0, 1], outs)
        self.assertIn(next(iter(winners[0].values())), issue['body'])
        self.assertEqual(sum(text.startswith('**adopt**') for text in issue['comments']), 1)
        self.assertTrue(any('cannot adopt' in err for _, err in outs), outs)

    def senders_setup(self):
        """Task 1 is pmA-codex's and task 2 pmB-claude's, both in ask: one outcome for each manager."""
        with self.pm(self.A):
            self.add('one', '--runtime', 'fake')
        with self.pm(self.B):
            self.add('two', '--runtime', 'fake')
        for n in (1, 2):
            self.legacy(n)
            with self.acting(f's-T{n}'):
                self.run_cli('ask', str(n), '--text', 'which?')

    def sender_wait(self, env, target):
        """The wait the `arm tick <target>` prompt prints, run as that sender."""
        with self.pm(env):
            prompt = self.run_cli('arm', 'tick', target)
            pm = re.search(r'wait --pm (\S+)`', prompt)[1]
            return self.run_cli('wait', '--pm', pm)

    def test_agent_sender_delivers_its_managers_outcome_once(self):
        # a separate Claude sender session serves pmA-codex: it gets A's ask once; A's own wait shares the receipt; B keeps its own
        self.senders_setup()
        sender = {'CLAUDE_CODE_SESSION_ID': 'sender-claude', 'TASKQ_RUNTIME': 'claude'}
        self.assertEqual(self.sender_wait(sender, 'pmA-codex'), 'ask #1\n')
        self.assertEqual(self.sender_wait(sender, 'pmA-codex'), 'tick\n')
        with self.pm(self.A):
            self.assertEqual(self.run_cli('wait'), 'tick\n')  # same receipt file: never twice
        with self.pm(self.B):
            self.assertEqual(self.run_cli('wait'), 'ask #2\n')  # never consumed by A's sender
        self.assertFalse((self.root / '.taskq' / 'wait-sender-claude.json').exists())

    def test_shell_sender_delivers_its_managers_outcome_once(self):
        # the printed shell loop runs with no session: it waits as the manager its link names, not as the owner's shell
        self.senders_setup()
        self.assertEqual(self.sender_wait({}, 'https://claude.ai/code/session_pmB-claude'), 'ask #2\n')
        with self.pm(self.B):
            self.assertEqual(self.run_cli('wait'), 'tick\n')
        with self.pm(self.A):
            self.assertEqual(self.run_cli('wait'), 'ask #1\n')
        self.assertFalse((self.root / '.taskq' / 'wait.json').exists())


class Contract(Base):
    """#430: `taskq pm` gives the manager role; tick and wait pull the clone and say when the contract changed."""

    def git(self, folder, *argv):
        REAL_RUN(['git', '-C', str(folder), '-c', 'user.name=t', '-c', 'user.email=t@t', *argv], check=True, capture_output=True)

    def test_pm_prints_role_and_records_hash(self):
        (self.root / 'taskq.md').write_text((ROOT / 'taskq.md').read_text())
        out = self.run_cli('pm')
        digest = taskq.contract()
        self.assertTrue(out.startswith(f'taskq pm contract {digest}\n'))
        for part in ('### R6. Human report', '### R13. Spec first', '## 7. Manager', '### After each pass', '### Take requests', 'run_in_background'):
            self.assertIn(part, out)
        self.assertNotIn('## 8. Runtimes', out)
        self.assertEqual(json.loads((self.root / '.taskq' / 'pm.json').read_text()), {'contract': digest})  # #532: the hash only

    def test_changed_contract_until_pm(self):
        line = 'The manager contract changed: run taskq pm and follow it from now on.'
        (self.root / 'taskq.md').write_text('v1\n## Principles\nx\n## 7. Manager\ny\n## 8. Runtimes\n')
        self.run_cli('pm')
        self.assertNotIn(line, self.run_cli('tick'))
        (self.root / 'taskq.md').write_text('v2\n## Principles\nx\n## 7. Manager\ny\n## 8. Runtimes\n')
        with mock.patch.object(taskq.time, 'time', side_effect=[0, 1e9]):
            self.assertEqual(self.run_cli('wait').splitlines(), [line, 'tick'])
        self.assertEqual(self.run_cli('tick').splitlines()[0], line)  # first, once per run
        self.run_cli('pm')
        self.assertNotIn(line, self.run_cli('tick'))

    def test_tick_pulls_a_clean_clone(self):
        origin, writer, clone = self.root / 'origin.git', self.root / 'writer', self.root / 'clone'
        with mock.patch.object(taskq.subprocess, 'run', REAL_RUN), mock.patch.object(taskq.subprocess, 'Popen', REAL_POPEN), \
                mock.patch.object(taskq, 'CLONE', clone), \
                mock.patch.dict(os.environ, {'PATH': os.defpath + os.pathsep + os.environ.get('PATH', '')}):
            REAL_RUN(['git', 'init', '-q', '--bare', str(origin)], check=True)
            REAL_RUN(['git', 'clone', '-q', str(origin), str(writer)], check=True, capture_output=True)
            (writer / 'taskq.md').write_text('v1\n')
            self.git(writer, 'add', '.')
            self.git(writer, 'commit', '-qm', 'v1')
            self.git(writer, 'push', '-q', 'origin', 'HEAD')
            REAL_RUN(['git', 'clone', '-q', str(origin), str(clone)], check=True, capture_output=True)
            self.run_cli('pm')
            (writer / 'taskq.md').write_text('v2\n')
            self.git(writer, 'commit', '-qam', 'v2')
            self.git(writer, 'push', '-q', 'origin', 'HEAD')
            self.assertIn('contract changed', self.run_cli('tick'))
            self.assertEqual((clone / 'taskq.md').read_text(), 'v2\n')
            (clone / 'taskq.md').write_text('local edit\n')  # a dirty clone is never pulled
            (writer / 'taskq.md').write_text('v3\n')
            self.git(writer, 'commit', '-qam', 'v3')
            self.git(writer, 'push', '-q', 'origin', 'HEAD')
            self.run_cli('tick')
            self.assertEqual((clone / 'taskq.md').read_text(), 'local edit\n')


class Cleanup(Base):
    """#476: `taskq cleanup` on a temp git repo with an origin; #1 open, #2-#5 closed."""

    def git(self, *argv, folder=None):
        return REAL_RUN(['git', '-C', str(folder or self.root), '-c', 'user.name=t', '-c', 'user.email=t@t', *argv],
                        check=True, capture_output=True, text=True).stdout

    def commit(self, folder, name):
        Path(folder, name).write_text(f'{name}\n')
        self.git('add', name, folder=folder)
        self.git('commit', '-qm', name, folder=folder)

    def setUp(self):
        super().setUp()
        self.fake = FakeRuntime()
        taskq.CONFIG.update(limits={'fake': 1}, repo='o/r')
        for patcher in (mock.patch.object(taskq, 'runtimes', return_value={'fake': self.fake}),
                        mock.patch.object(taskq, 'open_prs', return_value={'taskq-1': 11, 'taskq-9': 12}),
                        mock.patch.object(taskq.subprocess, 'run', REAL_RUN), mock.patch.object(taskq.subprocess, 'Popen', REAL_POPEN),
                        mock.patch.dict(os.environ, {'PATH': os.defpath})):
            patcher.start()
            self.addCleanup(patcher.stop)
        origin = tempfile.TemporaryDirectory()
        self.addCleanup(origin.cleanup)
        self.git('init', '-q', '--bare', origin.name)
        self.git('init', '-q', '-b', 'main')
        self.commit(self.root, 'a')
        self.git('remote', 'add', 'origin', origin.name)
        self.git('push', '-q', '-u', 'origin', 'main')
        for n in range(1, 6):
            self.add(f't{n}')  # no manager here: nothing spawns
        with contextlib.redirect_stdout(io.StringIO()):  # #1 an unsupervised worker's
            taskq.move(self.task(1), 'doing', 'take', claim={'runtime': 'fake', 'session': 's-T1', 'name': 'mac'})
        tree = lambda n: self.root / '.worktrees' / f'taskq-{n}'
        for n in (1, 2, 3, 4, 5):
            self.git('worktree', 'add', '-q', '-b', f'taskq-{n}', str(tree(n)), 'origin/main')
        self.commit(tree(2), 'b')  # merged: main fast-forwards to it
        self.git('push', '-q', 'origin', 'taskq-2')
        self.commit(tree(4), 'd')  # unmerged, on origin too
        self.git('push', '-q', 'origin', 'taskq-4')
        self.commit(tree(5), 'e')  # squash-merged: main has the same change in its own commit
        (tree(3) / 'notes').write_text('work')  # dirty
        for n in (4, 5):
            self.git('worktree', 'remove', str(tree(n)))
        self.git('merge', '-q', '--ff-only', 'taskq-2')
        self.commit(self.root, 'e')
        self.git('push', '-q', 'origin', 'main')
        for n in (2, 3, 4, 5):
            self.board.issues[n]['state'] = 'closed'
        self.fake.sessions.update({'s-T1': False, 's-T2': True, 's-T3': False, 's-T4': False})  # #1's worker died; #2's still runs
        self.board.issues[3]['comments'].append('**spawn** · fake:tick\n\nhttps://watch/s-T3')  # #478: recorded; s-T4: a name only
        (self.root / '.taskq' / 'S2.pid').write_text('999999999 old')
        (self.root / '.taskq' / 'wait.json').write_text('{"1": "doing", "2": "review"}')

    def state(self):
        return (sorted(path.name for path in (self.root / '.worktrees').iterdir()),
                self.git('for-each-ref', '--format=%(refname)', 'refs/heads', 'refs/remotes/origin/main', 'refs/remotes/origin/taskq-*').split(),
                sorted(self.fake.sessions), sorted(path.name for path in (self.root / '.taskq').glob('*.pid')),
                (self.root / '.taskq' / 'wait.json').read_text())

    def cleanup(self, *argv):
        return self.run_cli('cleanup', *argv).splitlines()

    def test_removes_leftovers_keeps_work(self):
        before = self.state()
        dry = self.cleanup('--dry-run')
        self.assertEqual(self.state(), before)  # --dry-run changes nothing
        out = self.cleanup()
        self.assertEqual(dry, [line.replace('removed', 'would remove', 1) for line in out])
        self.assertEqual(out, [
            'removed worktree .worktrees/taskq-2',
            'removed branch taskq-2', 'removed branch taskq-5', 'removed remote branch origin/taskq-2',
            'removed fake session s-T3 of #3', 'removed .taskq/S2.pid', 'removed .taskq/wait.json entries #2',
            'kept worktree .worktrees/taskq-1: open task', 'kept worktree .worktrees/taskq-3: dirty',
            'kept branch taskq-1: open task', 'kept branch taskq-3: its worktree is kept',
            'kept branch taskq-4: unmerged commits', 'kept remote branch origin/taskq-4: unmerged commits',
            'kept fake session s-T1 of #1: open task', 'kept fake session s-T2 of #2: running',
            'kept fake session s-T4 of #4: name only, not recorded on the board',
            'mess: branch taskq-4: task #4 is not open', 'mess: remote branch origin/taskq-4: task #4 is not open',
            'mess: #1 doing: session s-T1 is gone', 'mess: PR 12 (taskq-9): task #9 is not open'])
        self.assertEqual(self.state(), (['taskq-1', 'taskq-3'], [
            'refs/heads/main', 'refs/heads/taskq-1', 'refs/heads/taskq-3', 'refs/heads/taskq-4',
            'refs/remotes/origin/main', 'refs/remotes/origin/taskq-4'], ['s-T1', 's-T2', 's-T4'], [], '{"1": "doing"}'))
        after = self.state()
        self.assertEqual(self.cleanup(), out[7:])  # a second run removes nothing, keeps and reports the same
        self.assertEqual(self.state(), after)

    def test_remote_branch_deleted_elsewhere(self):
        sha = self.git('rev-parse', 'origin/taskq-4').strip()
        self.git('push', '-q', 'origin', '--delete', 'taskq-4')  # #515: deleted on the remote, a stale origin/taskq-4 left here
        self.git('update-ref', 'refs/remotes/origin/taskq-4', sha)
        out = self.cleanup()
        self.assertFalse([line for line in out if 'origin/taskq-4' in line])
        self.assertNotIn('refs/remotes/origin/taskq-4', self.state()[1])

    def test_external_workspace_touches_no_worktree_or_branch(self):
        taskq.CONFIG['workspace'] = 'external'  # #477
        trees, refs = self.state()[:2]
        self.assertIn('kept worktrees and branches: owned by host (workspace: external)', self.cleanup())
        self.assertEqual(self.state()[:2], (trees, refs))

    def test_old_session_names_and_review_without_pr(self):
        claude = taskq.Claude()
        claude.agents = lambda: {'a': {'id': 'ja', 'sessionId': 'a', 'name': 'S2 supervisor', 'state': 'stopped'},
                                 'b': {'id': 'jb', 'sessionId': 'b1234567x', 'name': 'T3 old title', 'state': 'stopped'}}
        self.board.issues[3]['comments'].append('**take** · claude:b1234567')
        taskq.CONFIG['publish'] = 'pr'
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(self.task(1), 'review', 'result', result={'sha': 'f' * 40})
        with mock.patch.object(taskq, 'runtimes', return_value={'claude': claude}):
            out = self.cleanup('--dry-run')
        self.assertIn('kept claude session a of #2: name only, not recorded on the board', out)
        self.assertIn('would remove claude session b1234567x of #3', out)
        self.assertNotIn('mess: #1 review: no open PR', out)  # taskq-1 has PR 11
        taskq.open_prs.return_value = {}
        with mock.patch.object(taskq, 'runtimes', return_value={}):
            self.assertIn('mess: #1 review: no open PR', self.cleanup('--dry-run'))


class Model(Base):

    def test_block_keeps_unknown_keys(self):
        board = taskq.BOARD = FakeBoard()
        n = board.add('t', taskq.block('text', {'scope': [], 'deps': [], 'claim': None, 'result': None, 'extra': {'x': 1}}),
                      ['q-later'])
        with mock.patch.dict(os.environ, {'TASKQ_RUNTIME': 'none'}), contextlib.redirect_stdout(io.StringIO()), \
                mock.patch.object(taskq, 'runtimes', return_value={}):
            taskq.main(['requeue', str(n)])
        found = taskq.parse(board.get(n))
        self.assertEqual((found['raw']['extra'], found['text'], found['state']), ({'x': 1}, 'text', 'ready'))
        self.assertEqual(board.issues[n]['comments'], ['**requeue** · owner'])
        self.assertIsNone(taskq.parse({**board.get(n), 'labels': ['q-ready', 'q-doing']}))

    def test_github_untrusted_issue_is_no_task(self):
        github = taskq.GitHub('o/r')
        item = {'number': 1, 'title': 't', 'body': taskq.block('x', {}), 'html_url': 'u', 'labels': [{'name': 'q-ready'}],
                'state': 'open', 'updated_at': 'now', 'author_association': 'NONE'}
        self.assertIsNone(taskq.parse(github.issue(item)))
        self.assertTrue(taskq.parse(github.issue({**item, 'author_association': 'OWNER'})))

    def test_config_and_board_file(self):
        with tempfile.TemporaryDirectory() as folder:
            Path(folder, 'taskq.json').write_text('{"board": "boards/fake.py"}')
            Path(folder, 'boards').mkdir()
            Path(folder, 'boards', 'fake.py').write_text('def list(state):\n    return ["fake"]\n')
            Path(folder, 'sub').mkdir()
            config = taskq.load_config(Path(folder, 'sub'))
            self.assertEqual((config['publish'], config['root']), ('direct', Path(folder).resolve()))
            tree = Path(folder, '.worktrees', 'taskq-1')  # a worker's worktree has its own taskq.json: the root stays the checkout
            tree.mkdir(parents=True)
            Path(tree, 'taskq.json').write_text('{}')
            self.assertEqual(taskq.load_config(tree)['root'], Path(folder).resolve())
            self.assertEqual(taskq.make_board(config).list(None), ['fake'])

    def test_contract_keeps_principles(self):
        # #311: a rewrite of taskq.md must not drop a principle silently (change rule: amend, never overwrite)
        text = (ROOT / 'taskq.md').read_text()
        self.assertEqual(re.findall(r'^### (R\d+)\. ', text, re.M), [f'R{n}' for n in range(1, 14)])
        self.assertIn('\n### Change rule\n', text)
        self.assertIn('\n## Product\n', text)  # #505: product decisions live here


FILE_BOARD = '''import json, pathlib
PATH = pathlib.Path(__file__).with_name('issues.json')
def load(): return json.loads(PATH.read_text()) if PATH.exists() else {}
def save(issues): PATH.write_text(json.dumps(issues))
def list(state): return [issue for issue in load().values() if issue['state'] == 'open' and issue.get('listed')]
def get(n): return load()[str(n)]
def add(title, body, labels):
    issues = load(); n = len(issues) + 1
    issues[str(n)] = {'iid': n, 'title': title, 'body': body, 'labels': labels, 'state': 'open',  # not listed yet: GitHub's list lags
                      'updated_at': '2026-10-09T00:00:00Z', 'url': '', 'comments': []}
    save(issues); return n
def update(n, labels=None, body=None):
    issues = load(); issues[str(n)].update({k: v for k, v in (('labels', labels), ('body', body)) if v is not None}, listed=True); save(issues)
def comment(n, text):
    issues = load(); issues[str(n)]['comments'].append(text); save(issues)
def close(n):
    issues = load(); issues[str(n)]['state'] = 'closed'; save(issues)
'''
FILE_RUNTIME = '''import pathlib
def spawn(name, prompt, cwd): pathlib.Path(__file__).with_name('spawned').write_text(name); return 's1'
def send(session, text): return session
def alive(session): return True
def link(session): return None
'''


class RealChild(unittest.TestCase):
    """#481: `add` in a real process starts the real detached `tick --quiet` child; it spawns and logs, though the list lags the add."""

    def test_add_spawns_from_the_detached_child(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        root = Path(folder.name)
        (root / 'board.py').write_text(FILE_BOARD)
        (root / 'fake.py').write_text(FILE_RUNTIME)
        (root / 'taskq.json').write_text(json.dumps({'board': 'board.py', 'runtimes': {'fake': 'fake.py'}, 'limits': {'fake': 1}}))
        env = {'PATH': '', 'TASKQ_HOST': 'mac', 'HOME': str(root), 'TASKQ_RUNTIME': 'fake'}  # a shell manager: the task's pm runtime  # no claude or codex on PATH: their retire fails quietly
        done = REAL_RUN([taskq.sys.executable, str(ROOT / 'taskq.py'), 'add', 'T', '--goal', 'g', '--acceptance', 'a'],
                        cwd=root, env=env, capture_output=True, text=True, timeout=60)
        self.assertEqual((done.returncode, done.stdout), (0, '#1 ready\n'), done.stderr)
        path, end = root / '.taskq' / 'dispatch.log', taskq.time.time() + 20
        while taskq.time.time() < end:  # #500: the child writes 'spawned' before it logs '#1 doing': wait for both
            log = path.read_text() if path.exists() else ''
            if (root / 'spawned').exists() and '#1 doing\n' in log:
                break
            taskq.time.sleep(0.1)
        self.assertEqual((root / 'spawned').read_text() if (root / 'spawned').exists() else None, 'S1 UNK T (mac)', log)
        self.assertRegex(log, r'^\S+ \S+ add #1\n#1 doing\n')


class Live526Red(unittest.TestCase):
    def test_controlled_red(self):
        self.fail('controlled CI failure #526')


if __name__ == '__main__':
    unittest.main()
