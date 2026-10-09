"""taskq.py on an in-memory board: every command, no network."""
import contextlib
import importlib.util
import io
import json
import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('taskq_single', ROOT / 'taskq.py')
taskq = importlib.util.module_from_spec(spec)
spec.loader.exec_module(taskq)
SESSION = '0123456789abcdef'
REAL_RUN, REAL_POPEN = subprocess.run, subprocess.Popen  # Base fails any real process; the pull test needs git


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
                self.assertRaisesRegex(SystemExit, 'not on origin/main'):
            self.run_cli('close', '1')
        self.assertEqual(self.board.issues[1]['state'], 'open')

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
        self.assertIn('`git fetch origin && git rebase origin/main`, run the tests', prompt)  # #334: up to date before result
        self.assertIn('taskq.md` first and do only what it allows (R13)', prompt)  # #505: spec first
        taskq.CONFIG['board'] = 'gitlab'
        self.assertIn('`glab mr create --yes --target-branch main --source-branch taskq-1', taskq.brief(item, 'claude'))
        taskq.CONFIG['publish'] = 'direct'
        self.assertIn('`git push origin HEAD:main`', taskq.brief(item, 'claude'))
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
    """The five runtime functions over a dict: session -> alive."""

    def __init__(self):
        self.sessions, self.sent, self.stopped = {}, [], []

    def spawn(self, name, prompt, cwd):
        self.names = [*getattr(self, 'names', []), name]
        self.sessions[f's-{name.split()[0]}'] = True
        self.prompt = prompt
        return f's-{name.split()[0]}'

    def send(self, session, text):
        self.sent.append((session, text))
        return session

    def alive(self, session):
        return self.sessions.get(session)

    def link(self, session):
        return f'https://watch/{session}'

    def retire(self, gone, running=True):
        for session, alive in list(self.sessions.items()):
            if gone(int(session.removeprefix('s-T')), session, alive) and (running or not alive):
                self.stopped.append(session)
                del self.sessions[session]


class Tick(Base):
    def setUp(self):
        super().setUp()
        self.fake = FakeRuntime()
        taskq.CONFIG.update(limits={'fake': 1}, repo='o/r')
        patcher = mock.patch.object(taskq, 'runtimes', return_value={'fake': self.fake})
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_spawn_one_per_free_slot(self):
        self.add('one')
        self.add('two')
        self.add('elsewhere', '--host', 'win', '--priority', '1')
        out = self.run_cli('tick')
        self.assertEqual((self.task(1)['state'], self.task(1)['claim']), ('doing', {'runtime': 'fake', 'session': 's-T1', 'name': 'mac'}))
        self.assertEqual((self.task(2)['state'], self.task(3)['state']), ('ready', 'ready'))
        self.assertIn('task #1: one', self.fake.prompt)
        self.assertIn('**spawn** · claude:01234567\n\nhttps://watch/s-T1', self.board.issues[1]['comments'])
        self.assertIn('| [#1](https://board/1) | doing | fake | [s-T1](https://watch/s-T1) |', out)
        self.assertTrue(out.endswith('Board: https://github.com/o/r/issues\n'))
        self.fake.sessions['s-T1'] = False  # the worker died: requeued, and the free slot runs it again first
        self.run_cli('tick')
        self.assertEqual([text.split(' ·')[0] for text in self.board.issues[1]['comments']], ['**add**', '**spawn**', '**requeue**', '**spawn**'])
        self.assertIn('session s-T1 is gone', self.board.issues[1]['comments'][2])
        self.assertEqual((self.task(1)['state'], self.task(2)['state']), ('doing', 'ready'))

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

    def test_second_quick_death_asks(self):
        # #393: a worker that dies at once is respawned once, then the owner is asked with the last log line
        self.fake.tail = lambda session: 'error: unsupported model'
        self.add()
        for _ in range(3):
            self.run_cli('tick')
            self.fake.sessions['s-T1'] = False
        self.run_cli('tick')
        self.assertEqual(self.task(1)['state'], 'ask')
        self.assertEqual(len(self.fake.names), 2)  # no third spawn
        self.assertIn('Last log line: error: unsupported model', self.board.issues[1]['comments'][-1])
        self.run_cli('answer', '1', '--text', 'fixed')  # an answer resets the count: the next death requeues
        self.run_cli('tick')
        self.assertEqual((self.task(1)['state'], len(self.fake.names)), ('doing', 3))

    def test_two_passes_at_once_spawn_one_worker(self):
        # #357 (R2): a tick runs while an event pass spawns; it finds the lock held and starts nothing
        taskq.CONFIG['limits'] = {'fake': 2}
        spawn, err = self.fake.spawn, io.StringIO()

        def overlapping(*spawn_args):
            with contextlib.redirect_stderr(err):
                self.run_cli('tick')
            return spawn(*spawn_args)
        self.fake.spawn = overlapping
        self.add('one')  # the add's pass spawns #1, and the tick runs at that moment
        self.assertEqual(self.fake.names, ['T1 CLD one (mac)'])
        self.assertIn('another pass is running', err.getvalue())
        self.fake.spawn = spawn
        stale = [dict(self.board.issues[1], labels=['q-ready'])]  # a list from before the spawn: the re-read sees #1 taken
        with mock.patch.object(self.board, 'list', return_value=stale):
            self.run_cli('tick')
        self.assertEqual(self.fake.names, ['T1 CLD one (mac)'])

    def test_nudge_only_a_silent_worker(self):
        self.add()
        self.run_cli('tick')
        self.board.issues[1]['updated_at'] = taskq.datetime.now(taskq.timezone.utc).isoformat()
        self.run_cli('tick')
        self.assertEqual(self.fake.sent, [])
        self.board.issues[1]['updated_at'] = '2026-01-01T00:00:00Z'
        self.run_cli('tick')
        self.assertEqual(self.fake.sent, [('s-T1', 'continue: read your issue')])
        self.assertTrue(self.board.issues[1]['comments'][-1].startswith('**nudge**'))
        self.fake.sessions['s-T1'] = None  # cannot tell: left alone
        self.run_cli('tick')
        self.assertEqual((self.task(1)['state'], len(self.fake.sent)), ('doing', 1))

    def test_answer_reaches_the_asking_worker_once(self):
        self.add()
        self.run_cli('tick')
        self.board.issues[1]['updated_at'] = taskq.datetime.now(taskq.timezone.utc).isoformat()
        self.run_cli('ask', '1', '--text', 'which?')
        self.run_cli('answer', '1', '--text', 'the first')
        self.run_cli('tick')
        self.run_cli('tick')
        self.assertEqual(self.fake.sent, [('s-T1', 'The owner answered your question:\n\nthe first')])
        self.assertEqual(self.task(1)['state'], 'doing')

    def test_decisions_block_and_answer_by_codes(self):
        # #490: cards for an ask and a review with options; one line of codes answers both
        for title in ('one', 'two', 'three'):
            self.add(title)
        taskq.CONFIG['limits'] = {'fake': 3}
        self.run_cli('tick')
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
        self.run_cli('result', '3', '--sha', 'a' * 40)
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
        self.assertRegex(self.fake.names[0], r'^T1 (CLD|CDX) \S.* \(mac\)$')

    def test_events_spawn_the_next_task_at_once(self):
        # #333 (R4): add, result, close, requeue and answer each run the pass once; a full slot spawns nothing
        spawns = lambda: [n for n in self.board.issues for text in self.board.issues[n]['comments'] if text.startswith('**spawn**')]
        self.add('one')
        self.add('two')
        self.assertEqual(spawns(), [1])  # add: #1 into the free slot; #2 finds it full
        self.run_cli('result', '1', '--sha', 'a' * 40)
        self.assertEqual(spawns(), [1, 2])
        self.add('three')
        self.run_cli('requeue', '2')  # requeue frees the slot, #2 goes first again
        self.assertEqual(spawns(), [1, 2, 2])
        self.run_cli('ask', '2', '--text', 'which?')  # ask is no event: #3 waits for the tick or the next event
        self.assertEqual(spawns(), [1, 2, 2])
        self.run_cli('answer', '2', '--text', 'this')  # the live worker gets the answer, the slot stays full
        self.assertEqual((spawns(), self.fake.sent), ([1, 2, 2], [('s-T2', 'The owner answered your question:\n\nthis')]))
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)):
            self.run_cli('close', '1')  # #1 held no slot: nothing new
            self.assertEqual(spawns(), [1, 2, 2])
            self.run_cli('result', '2', '--sha', 'b' * 40)
            self.assertEqual(spawns(), [1, 2, 2, 3])
            self.fake.sessions['s-T3'] = False  # #3 died: close of #2 requeues and respawns it in the same pass
            self.run_cli('close', '2')
        self.assertEqual(spawns(), [1, 2, 2, 3, 3])
        self.run_cli('tick')  # the safety net finds nothing left to do
        self.assertEqual(spawns(), [1, 2, 2, 3, 3])

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
        self.add()
        self.run_cli('tick')
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
        for name in ('mac', 'win'):
            self.add()
            self.run_cli('tick')
            n = len(self.board.issues)
            claim = {**self.task(n)['claim'], 'name': name}
            taskq.move(self.task(n), 'review', 'result', claim=claim, result={'sha': 'a' * 40})
            with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)):
                self.run_cli('close', str(n))
        self.assertEqual(self.fake.stopped, ['s-T1', 's-T2'])  # the fake lists both here; on a real machine only its own
        self.assertNotIn('stop it there', self.board.issues[1]['comments'][-1])
        self.assertEqual(self.board.issues[2]['comments'][-1], '**close** · claude:01234567\n\nsession s-T2 runs on win: stop it there')

    def test_tick_removes_stopped_sessions_of_closed_tasks(self):
        self.add()
        self.fake.sessions.update({'s-T8': True, 's-T9': False})  # tasks 8 and 9 are not open
        self.run_cli('tick')
        self.assertEqual(self.fake.stopped, ['s-T9'])
        self.assertEqual(set(self.fake.sessions), {'s-T1', 's-T8'})

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


class Wait(Tick):
    """#407: `taskq wait` returns once per event, or 'tick' after the window; the clock is patched."""

    def setUp(self):
        super().setUp()
        self.clock = [0.0]
        for name, fake in (('time', lambda: self.clock[0]), ('sleep', lambda s: self.clock.__setitem__(0, self.clock[0] + s))):
            patcher = mock.patch.object(taskq.time, name, fake)
            patcher.start()
            self.addCleanup(patcher.stop)

    def test_events_once_then_tick(self):
        self.add()
        self.run_cli('tick')
        self.assertEqual(self.run_cli('wait'), 'tick\n')  # nothing happened: the safety window
        self.assertEqual(self.clock[0], 600)
        self.run_cli('result', '1', '--sha', 'a' * 40)
        self.add('two')  # the event pass spawns it
        self.fake.sessions['s-T2'] = False
        self.assertEqual(self.run_cli('wait').splitlines(), ['review #1', 'gone #2'])
        self.assertEqual(self.clock[0], 600)  # at once, no sleep
        self.assertEqual(self.run_cli('wait'), 'tick\n')  # never twice for the same event
        self.run_cli('requeue', '1')
        self.run_cli('tick')
        start = self.clock[0]
        result = lambda: self.run_cli('result', '1', '--sha', 'b' * 40) if self.clock[0] == start + 50 else None
        with mock.patch.object(taskq.time, 'sleep', lambda s: (self.clock.__setitem__(0, self.clock[0] + s), result())):
            self.assertEqual(self.run_cli('wait'), 'review #1\n')  # back in review while waiting: a new event
        self.assertEqual(self.clock[0], start + 50)

    def test_arm_tick_names_target_and_loop(self):
        out = self.run_cli('arm', 'tick', 'PM main')
        self.assertIn('manager session PM main', out)
        self.assertIn('taskq.py wait`', out)
        self.assertIn('with SendMessage', out)

    def test_arm_tick_in_codex_names_resume(self):
        """#522: a CLI thread (local rollout) gets exec resume."""
        home = self.root / 'codex'
        (home / 'sessions/2026/10/09').mkdir(parents=True)
        (home / 'sessions/2026/10/09/rollout-2026-10-09T07-46-53-T1.jsonl').write_text('')
        with mock.patch.dict(os.environ, {'TASKQ_RUNTIME': 'codex', 'CODEX_THREAD_ID': 'T1', 'CODEX_HOME': str(home)}):
            sender, self_arm = self.run_cli('arm', 'tick', 'T1'), self.run_cli('arm', 'tick')
        self.assertIn('resume T1 "<its output>"', sender)
        self.assertIn('resume T1 "$e"; done', sender)
        self.assertNotIn('send_message_to_thread', sender)
        self.assertIn('in the foreground', self_arm)
        self.assertIn('arm tick T1`', self_arm)

    def test_arm_tick_in_codex_app_thread_uses_native_sender(self):
        """#522: an app thread has no local rollout; exec resume would fail `no rollout found`, so no resume is printed."""
        with mock.patch.dict(os.environ, {'TASKQ_RUNTIME': 'codex', 'CODEX_HOME': str(self.root / 'none')}):
            sender = self.run_cli('arm', 'tick', 'APP1')
        self.assertIn('no local rollout of APP1', sender)
        self.assertIn('with the Codex app tool `send_message_to_thread`', sender)
        self.assertNotIn('resume APP1', sender)
        self.assertNotIn('while :', sender)


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
        self.assertEqual(json.loads((self.root / '.taskq' / 'pm.json').read_text()), {'contract': digest})

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
            self.add(f't{n}')  # the add's pass spawns #1 into the one slot
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
        n = board.add('t', taskq.block('text', {'scope': [], 'deps': [], 'claim': None, 'result': None, 'supervisor': {'x': 1}}),
                      ['q-later'])
        with mock.patch.dict(os.environ, {'TASKQ_RUNTIME': 'none'}), contextlib.redirect_stdout(io.StringIO()), \
                mock.patch.object(taskq, 'runtimes', return_value={}):
            taskq.main(['requeue', str(n)])
        found = taskq.parse(board.get(n))
        self.assertEqual((found['raw']['supervisor'], found['text'], found['state']), ({'x': 1}, 'text', 'ready'))
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
        env = {'PATH': '', 'TASKQ_HOST': 'mac', 'HOME': str(root)}  # no claude or codex on PATH: their retire fails quietly
        done = REAL_RUN([taskq.sys.executable, str(ROOT / 'taskq.py'), 'add', 'T', '--goal', 'g', '--acceptance', 'a'],
                        cwd=root, env=env, capture_output=True, text=True, timeout=60)
        self.assertEqual((done.returncode, done.stdout), (0, '#1 ready\n'), done.stderr)
        path, end = root / '.taskq' / 'dispatch.log', taskq.time.time() + 20
        while taskq.time.time() < end:  # #500: the child writes 'spawned' before it logs '#1 doing': wait for both
            log = path.read_text() if path.exists() else ''
            if (root / 'spawned').exists() and '#1 doing\n' in log:
                break
            taskq.time.sleep(0.1)
        self.assertEqual((root / 'spawned').read_text() if (root / 'spawned').exists() else None, 'T1 UNK T (mac)', log)
        self.assertRegex(log, r'^\S+ \S+ add #1\n#1 doing\n')


if __name__ == '__main__':
    unittest.main()
