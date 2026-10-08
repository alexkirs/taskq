"""taskq.py on an in-memory board: every command, no network."""
import contextlib
import importlib.util
import io
import json
import os
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
        self.assertEqual(run.call_args.args[0][-4:], ['merge-base', '--is-ancestor', 'a' * 40, 'origin/main'])
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

    def cli(self, command, **_):
        self.calls.append(command[1:])
        verb = command[2]
        out = {'list': json.dumps(self.prs), 'view': json.dumps({'state': 'MERGED' if self.merged else 'OPEN', 'mergeCommit': {'oid': 'c' * 40}})}
        return subprocess.CompletedProcess(command, int(verb == 'merge' and not self.merged), out.get(verb, ''), 'Pull request is not mergeable')

    def close(self):
        with mock.patch.object(taskq.subprocess, 'run', side_effect=self.cli), mock.patch.object(taskq.shutil, 'which', side_effect=lambda name: name):
            return self.run_cli('close', '1')

    def test_brief(self):
        item = self.task(1)
        prompt = taskq.brief(item, 'claude')
        self.assertIn('`git push --force-with-lease origin HEAD:refs/heads/taskq-1`', prompt)
        self.assertIn('`gh pr create --base main --head taskq-1 --title "<title>" --body "<summary>"`', prompt)
        self.assertIn('--sha <PR head full SHA>', prompt)
        taskq.CONFIG['board'] = 'gitlab'
        self.assertIn('`glab mr create --yes --target-branch main --source-branch taskq-1', taskq.brief(item, 'claude'))
        taskq.CONFIG['publish'] = 'direct'
        self.assertIn('`git push origin HEAD:main`', taskq.brief(item, 'claude'))

    def test_close_merges(self):
        self.assertEqual(self.close(), '#1 closed\n')
        self.assertEqual(self.calls[1], ['pr', 'merge', '7', '--squash', '--delete-branch', '--match-head-commit', 'a' * 40, '-R', 'o/r'])
        self.assertEqual(self.board.issues[1]['state'], 'closed')
        self.assertEqual(self.board.issues[1]['comments'][-1], f'**close** · claude:01234567\n\nmerged {"c" * 40}')

    def test_head_is_not_the_result(self):
        for change in ({'headRefOid': 'b' * 40}, {'baseRefName': 'release'}):
            self.prs[0].update(change)
            with self.assertRaisesRegex(SystemExit, 'do not match the result'):
                self.close()
            self.prs[0].update(headRefOid='a' * 40, baseRefName='main')
        self.assertEqual((self.task(1)['state'], [call[1] for call in self.calls]), ('review', ['list', 'list']))

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
    """The four runtime functions over a dict: session -> alive."""

    def __init__(self):
        self.sessions, self.sent = {}, []

    def spawn(self, name, prompt, cwd):
        self.sessions[f's-{name}'] = True
        self.prompt = prompt
        return f's-{name}'

    def send(self, session, text):
        self.sent.append((session, text))
        return session

    def alive(self, session):
        return self.sessions.get(session)

    def link(self, session):
        return f'https://watch/{session}'


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
        with mock.patch.dict(os.environ, {'TASKQ_RUNTIME': 'none'}), contextlib.redirect_stdout(io.StringIO()):
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
            self.assertEqual(taskq.make_board(config).list(None), ['fake'])


if __name__ == '__main__':
    unittest.main()
