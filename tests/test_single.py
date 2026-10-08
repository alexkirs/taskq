"""taskq.py on an in-memory board: every command, no network."""
import contextlib
import importlib.util
import io
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('taskq_single', ROOT / 'taskq.py')  # `import taskq` is the package
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


class Commands(unittest.TestCase):
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
        taskq.CONFIG['publish'] = 'pr'
        with self.assertRaisesRegex(SystemExit, 'not supported yet'):
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
