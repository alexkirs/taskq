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


class Base(unittest.TestCase):
    def setUp(self):
        self.board = taskq.BOARD = FakeBoard()
        taskq.CONFIG = {'board': 'github', 'publish': 'direct', 'root': ROOT, 'hosts': {}}
        patcher = mock.patch.dict(os.environ, {'CLAUDE_CODE_SESSION_ID': SESSION, 'TASKQ_RUNTIME': 'claude', 'TASKQ_HOST': 'mac'})
        patcher.start()
        self.addCleanup(patcher.stop)
        patcher = mock.patch.object(taskq, 'runtimes', return_value={})  # no real worker from an event's dispatch
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

    def test_close_refuses_commit_not_on_main(self):
        self.add()
        self.run_cli('take', '1')
        self.run_cli('result', '1', '--sha', 'b' * 40)
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1)), \
                self.assertRaisesRegex(SystemExit, 'not on origin/main'):
            self.run_cli('close', '1')
        self.assertEqual(self.board.issues[1]['state'], 'open')

    def test_close_removes_clean_worktree_keeps_dirty(self):
        real = subprocess.run
        fake = lambda command, **kw: subprocess.CompletedProcess(command, 0) if {'fetch', 'merge-base'} & set(command) else real(command, **kw)
        with tempfile.TemporaryDirectory() as folder:
            root = taskq.CONFIG['root'] = Path(folder)
            git = ['git', '-C', folder, '-c', 'user.name=t', '-c', 'user.email=t@t']
            real([*git, 'init', '-q'], check=True)
            real([*git, 'commit', '-q', '--allow-empty', '-m', 'init'], check=True)
            for n in (1, 2):
                self.add()
                self.run_cli('take', str(n))
                self.run_cli('result', str(n), '--sha', 'a' * 40)
                real([*git, 'worktree', 'add', '-q', '-b', f'taskq-{n}', f'.worktrees/taskq-{n}'], check=True)
            (root / '.worktrees' / 'taskq-2' / 'wip.txt').write_text('x')
            with mock.patch.object(taskq.subprocess, 'run', side_effect=fake):
                self.run_cli('close', '1')
                self.run_cli('close', '2')
            branches = real([*git, 'branch', '--list', 'taskq-*'], capture_output=True, text=True).stdout.split()
            self.assertEqual((sorted(p.name for p in (root / '.worktrees').iterdir()), branches), (['taskq-2'], ['+', 'taskq-2']))
        self.assertEqual(self.board.issues[1]['comments'][-1], '**close** · claude:01234567')
        self.assertEqual(self.board.issues[2]['comments'][-1],
                         '**close** · claude:01234567\n\nkept .worktrees/taskq-2 and branch taskq-2: uncommitted changes')

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
        if command[1] == 'api':  # the 'tests' gate (#308): check runs per SHA
            out = {'check_runs': [{'status': status, 'conclusion': conclusion} for status, conclusion in self.poll(command[4].split('/')[4])]}
            return subprocess.CompletedProcess(command, 0, json.dumps(out), '')
        verb = command[2]
        out = {'list': json.dumps(self.branches.get(command[4], self.prs)), 'view': json.dumps({'state': 'MERGED' if self.merged else 'OPEN', 'mergeCommit': {'oid': 'c' * 40}})}
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
        taskq.CONFIG['board'] = 'gitlab'
        self.assertIn('`glab mr create --yes --target-branch main --source-branch taskq-1', taskq.brief(item, 'claude'))
        taskq.CONFIG['publish'] = 'direct'
        self.assertIn('`git push origin HEAD:main`', taskq.brief(item, 'claude'))

    def test_close_merges(self):
        self.assertEqual(self.close(), '#1 closed\n')
        self.assertEqual(self.merges(), [['pr', 'merge', '7', '--squash', '--delete-branch', '--match-head-commit', 'a' * 40, '-R', 'o/r']])
        self.assertIn(['api', '-X', 'GET', f'repos/o/r/commits/{"a" * 40}/check-runs?check_name=tests'], self.calls)
        self.assertEqual(self.board.issues[1]['state'], 'closed')
        self.assertEqual(self.board.issues[1]['comments'][-1], f'**close** · claude:01234567\n\nmerged {"c" * 40}')

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

    def test_gitlab_and_review_mode(self):
        taskq.CONFIG.update(board='gitlab', host='git.example')
        self.prs = [{'iid': 7, 'sha': 'a' * 40, 'target_branch': 'main'}]
        self.close()
        self.assertEqual(self.calls[1][-3:], ['--yes', '-R', 'https://git.example/o/r'])
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

    def stop(self, session):
        self.stopped.append(session)


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
        self.assertIn('#1     doing    fake     https://watch/s-T1', out)
        self.assertTrue(out.endswith('Board: https://github.com/o/r/issues\n'))
        self.fake.sessions['s-T1'] = False  # the worker died: requeued, and the free slot runs it again first
        self.run_cli('tick')
        self.assertEqual([text.split(' ·')[0] for text in self.board.issues[1]['comments']], ['**add**', '**spawn**', '**requeue**', '**spawn**'])
        self.assertIn('session s-T1 is gone', self.board.issues[1]['comments'][2])
        self.assertEqual((self.task(1)['state'], self.task(2)['state']), ('doing', 'ready'))

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

    def test_event_survives_a_failed_dispatch(self):
        self.fake.spawn = lambda *_: taskq.fail('claude could not start the session')
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            self.assertEqual(self.add(), '#1 ready\n')
        self.assertIn('dispatch stopped: claude could not start the session', err.getvalue())

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
        self.assertEqual(self.fake.stopped, ['s-T1'])
        self.assertNotIn('stop it there', self.board.issues[1]['comments'][-1])
        self.assertEqual(self.board.issues[2]['comments'][-1], '**close** · claude:01234567\n\nsession s-T2 runs on win: stop it there')

    def test_codex_alive_from_pid_file(self):
        with tempfile.TemporaryDirectory() as folder:
            taskq.CONFIG['root'] = Path(folder)
            codex = taskq.Codex()
            (codex.folder() / 'T1.pid').write_text(f'{os.getpid()} thread-1')
            dead = subprocess.Popen(['python3', '-c', ''])
            dead.wait()
            (codex.folder() / 'T2.pid').write_text(f'{dead.pid} thread-2')
            self.assertEqual([codex.alive(s) for s in ('thread-1', 'thread-2', 'thread-3')], [True, False, None])
            self.assertTrue(codex.link('thread-1').endswith('/open.html#codex://threads/thread-1'))


class Model(unittest.TestCase):
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
        self.assertEqual(re.findall(r'^### (R\d+)\. ', text, re.M), [f'R{n}' for n in range(1, 13)])
        self.assertIn('\n### Change rule\n', text)


if __name__ == '__main__':
    unittest.main()
