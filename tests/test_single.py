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
import threading
import textwrap
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('taskq_single', ROOT / 'taskq.py')
taskq = importlib.util.module_from_spec(spec)
spec.loader.exec_module(taskq)
SESSION = '0123456789abcdef'
BIRTH = f'{taskq.process_domain()}:1' + (':0' if sys.platform == 'darwin' else '')
NEXT_BIRTH = f'{taskq.process_domain()}:2' + (':0' if sys.platform == 'darwin' else '')
REAL_RUN, REAL_POPEN = subprocess.run, subprocess.Popen  # Base fails any real process; the pull test needs git, the sender loop bash


class FakeBoard:
    """The six board functions over a dict of issues."""

    def __init__(self):
        self.issues = {}
        self.guard = None
        self.guard_serial = 0
        self.guard_mutex = threading.Lock()

    def ensure_event_label(self):
        pass  # fake board labels need no native provisioning

    def acquire(self, owner):
        with self.guard_mutex:
            if self.guard is not None:
                return None
            self.guard_serial += 1
            self.guard = f'grant-{self.guard_serial}'
            return self.guard

    def release(self, token):
        with self.guard_mutex:
            if self.guard != token:
                raise RuntimeError('exact grant no longer present')
            self.guard = None

    def list(self, state):
        return [dict(issue) for issue in self.issues.values() if issue['state'] == 'open'
                and (not state or f'q-{state}' in issue['labels'])]  # None: every open issue, a task or not (§ 2)

    def get(self, n):
        return dict(self.issues[n])

    def closed(self):
        return [dict(issue) for issue in self.issues.values() if issue['state'] == 'closed' and taskq.EVENT_LABEL in issue['labels']]

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
        env = {'CLAUDE_CODE_SESSION_ID': SESSION, 'TASKQ_RUNTIME': 'claude', 'TASKQ_HOST': 'mac'}
        if os.name == 'nt':  # Windows has no /usr/bin fallback after clear=True; preserve only the native git directory
            git = taskq.shutil.which('git')
            env.update(PATH=str(Path(git).parent) if git else '', SYSTEMROOT=os.environ['SYSTEMROOT'])
        def child(command, **_):
            try:
                taskq.main(command[2:])
            except SystemExit:
                pass  # real detached child failure cannot fail its settled parent command
        for patcher in (
                mock.patch.dict(os.environ, env, clear=True),
                mock.patch.object(taskq, 'GUARD_WAIT', 0),
                mock.patch.object(taskq, 'runtimes', return_value={}),  # no real worker from an event's dispatch
                mock.patch.object(taskq, 'start_pass', child),  # the child's pass, in process
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

    def wait_and_ack(self, *argv):
        """Simulate the recipient handling its observed batch, then explicitly acknowledging it."""
        actor, held = taskq.session() or {}, taskq.origin() or {}
        if '--task' in argv:
            held = (self.task(int(argv[argv.index('--task') + 1])) or {}).get('supervisor') or {}
        # TickSetup's synthetic runtime uses a native-session fixture; simulate its full recipient identity.
        if held.get('runtime') == 'fake' and held.get('session') == actor.get('session'):
            with mock.patch.object(taskq, 'session', return_value={**actor, 'runtime': 'fake'}):
                out = self.run_cli('wait', *argv)
        else:
            out = self.run_cli('wait', *argv)
        ids = re.findall(r'\[event (\d+:\d+)\]', out)
        if ids:
            extra = ['--pm', argv[argv.index('--pm') + 1]] if '--pm' in argv else []
            identity = taskq.session() or {}
            raw = self.task(int(ids[0].split(':')[0])) if self.board.get(int(ids[0].split(':')[0]))['state'] == 'open' else {'raw': taskq.issue_data(self.board.get(int(ids[0].split(':')[0])))}
            held = raw['raw'].get('supervisor' if '--task' in argv else 'pm') or identity
            with mock.patch.object(taskq, 'session', return_value={**identity, 'runtime': held.get('runtime', identity.get('runtime'))} if identity else None):
                self.run_cli('ack', *ids, *extra)
        return re.sub(r' \[event \d+:\d+\]', '', out)

    def add(self, title='T', *extra):
        return self.run_cli('add', title, '--goal', 'g', '--acceptance', 'a', '--scope', 'x.py', *extra)



class Coordination(Base):
    def test_nested_guard_and_other_thread_cannot_share_grant(self):
        failures = []
        with taskq.coordination():
            token = self.board.guard
            with taskq.coordination():
                self.assertEqual(self.board.guard, token)
            def contender():
                try:
                    with taskq.coordination():
                        failures.append('entered')
                except SystemExit as error:
                    failures.append(str(error))
            thread = threading.Thread(target=contender)
            thread.start(); thread.join(2)
            self.assertFalse(thread.is_alive())
            self.assertEqual(len(failures), 1)
            self.assertIn('project guard busy', failures[0])
            self.assertEqual(self.board.guard, token)
        self.assertIsNone(self.board.guard)

    def test_before_effect_failure_releases_and_post_effect_baseexceptions_retain(self):
        with self.assertRaisesRegex(SystemExit, 'validation'):
            with taskq.coordination():
                taskq.fail('validation')
        self.assertIsNone(self.board.guard)
        for failure in (ValueError('write unknown'), SystemExit('stopped'), KeyboardInterrupt()):
            with self.subTest(failure=type(failure).__name__), self.assertRaises(type(failure)):
                with taskq.coordination():
                    taskq.effect(lambda: (_ for _ in ()).throw(failure))
            token = self.board.guard
            self.assertIsNotNone(token)
            with self.assertRaisesRegex(SystemExit, 'project guard busy'):
                with taskq.coordination():
                    self.fail('entered orphaned guard')
            self.board.release(token)  # explicit fixture quiescence/reconciliation

    def test_caught_nested_effect_failure_poison_stops_further_effects(self):
        later = mock.Mock()
        with self.assertRaisesRegex(SystemExit, 'retained'):
            with taskq.coordination():
                try:
                    with taskq.coordination():
                        taskq.effect(lambda: (_ for _ in ()).throw(RuntimeError('unknown')))
                except RuntimeError:
                    pass
                with self.assertRaisesRegex(SystemExit, 'poisoned'):
                    taskq.effect(later)
        later.assert_not_called()
        self.assertIsNotNone(self.board.guard)

    def test_caught_effect_failure_without_followup_is_nonzero(self):
        with self.assertRaisesRegex(SystemExit, 'retained'):
            with taskq.coordination():
                try:
                    taskq.effect(lambda: (_ for _ in ()).throw(RuntimeError('retire failed')))
                except RuntimeError:
                    pass
        self.assertIsNotNone(self.board.guard)

    def test_poisoned_scope_cannot_dispatch_as_settled(self):
        def bad_transition(args):
            try:
                taskq.effect(lambda: (_ for _ in ()).throw(RuntimeError('unknown write')))
            except RuntimeError:
                pass
            raise taskq.SettledError('nominally settled', [1])
        with mock.patch.object(taskq, 'cmd_add', bad_transition), mock.patch.object(taskq, 'dispatch') as dispatch, \
                self.assertRaisesRegex(SystemExit, 'retained') as error:
            self.add()
        self.assertNotIsInstance(error.exception, taskq.SettledError)
        dispatch.assert_not_called()
        self.assertIsNotNone(self.board.guard)

    def test_sandbox_tick_and_status_never_acquire(self):
        self.add()
        with mock.patch.object(self.board, 'acquire', side_effect=AssertionError('read-only')), \
                mock.patch.dict(os.environ, {'CODEX_SANDBOX': '1'}):
            self.run_cli('tick')
            self.run_cli('status')

    def test_missing_codex_handle_is_unknown_and_failed_archive_keeps_handle(self):
        codex = taskq.Codex()
        self.assertIsNone(codex.state('another-checkout-session'))
        path = self.root / '.taskq' / 'S1.pid'
        path.parent.mkdir(exist_ok=True)
        path.write_text(f'99999 session {BIRTH}')
        with mock.patch.object(taskq, 'process_identity', return_value=('dead', None)), \
                mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1)), \
                self.assertRaisesRegex(RuntimeError, 'archive failed'):
            with taskq.coordination():
                taskq.effect(codex.retire, lambda *args: True)
        self.assertTrue(path.is_file())
        self.assertIsNotNone(self.board.guard)

    def test_failed_claude_stop_never_resumes_or_removes(self):
        claude = taskq.Claude()
        agent = {'id': 'job', 'sessionId': 'sid', 'pid': 1, 'name': 'S1 CDX task (mac)'}
        with mock.patch.object(claude, 'agents', return_value={'sid': agent}), \
                mock.patch.object(claude, 'start') as start, \
                mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1)) as run:
            with self.assertRaisesRegex(RuntimeError, 'stop failed'):
                claude.send('sid', 'resume')
            start.assert_not_called()
            with self.assertRaisesRegex(RuntimeError, 'stop failed'):
                claude.retire(lambda *args: True)
            self.assertTrue(all('rm' not in call.args[0] for call in run.call_args_list))

    def test_unknown_acquisition_and_release_never_fall_back(self):
        with mock.patch.object(self.board, 'acquire', side_effect=TimeoutError('unknown')), \
                mock.patch.object(self.board, 'release') as release, self.assertRaisesRegex(SystemExit, 'no grant assumed'):
            with taskq.coordination():
                self.fail('entered without acknowledgement')
        release.assert_not_called()
        with mock.patch.object(self.board, 'release', side_effect=TimeoutError('unknown')) as release, \
                self.assertRaisesRegex(SystemExit, 'exact token'):
            with taskq.coordination():
                token = self.board.guard
        release.assert_called_once_with(token)
        self.assertEqual(self.board.guard, token)

    def test_custom_read_only_board_refuses_writes_before_read_or_effect(self):
        from types import SimpleNamespace
        self.add()
        taskq.BOARD = SimpleNamespace(list=self.board.list)
        self.assertIn('#1', self.run_cli('list'))
        with self.assertRaisesRegex(SystemExit, 'no atomic acquire/release'):
            self.run_cli('take', '1')
        self.assertEqual(self.task(1)['state'], 'ready')

    def test_every_take_reads_and_writes_inside_guard_and_dispatch_follows_release(self):
        self.add()
        real = self.board.get
        def fresh(n):
            self.assertIsNotNone(self.board.guard)
            return real(n)
        with mock.patch.object(self.board, 'get', fresh):
            self.run_cli('take', '1')
        def dispatch(*args):
            self.assertIsNone(self.board.guard)
        with mock.patch.object(taskq, 'dispatch', dispatch):
            self.run_cli('result', '1', '--sha', 'a' * 40)

    def test_adapters_grant_only_acknowledged_new_token_and_exact_release(self):
        github = taskq.GitHub('o/r')
        github.coordination_label = 'taskq-probe-fixture'
        with mock.patch.object(taskq, 'coordination_api', side_effect=[(404, {}), (201, {'name': github.coordination_label, 'node_id': 'NODE'})]) as api:
            self.assertEqual(github.acquire('owner'), 'NODE')
        self.assertEqual(api.call_args.args[4]['name'], github.coordination_label)
        duplicate = {'errors': [{'resource': 'Label', 'field': 'name', 'code': 'already_exists'}]}
        with mock.patch.object(taskq, 'coordination_api', side_effect=[(404, {}), (422, duplicate)]):
            self.assertIsNone(github.acquire('owner'))
        with mock.patch.object(taskq, 'coordination_api', return_value=(200, {'name': github.coordination_label, 'node_id': 'OTHER'})) as api:
            self.assertIsNone(github.acquire('owner'))
            api.assert_called_once()  # presence polling makes no mutating request
        for response in ((401, {}), (500, {}), (422, {'message': 'spam'})):
            with self.subTest(response=response), mock.patch.object(taskq, 'coordination_api', side_effect=[(404, {}), response]), \
                    self.assertRaisesRegex(RuntimeError, 'failed/unknown'):
                github.acquire('owner')
        with mock.patch.object(taskq, 'coordination_api', return_value=(200, {'data': {'deleteLabel': {}}})) as api:
            github.release('NODE')
        self.assertEqual(api.call_args.args[3:], ('graphql', {'query': 'mutation($id: ID!) { deleteLabel(input: {id: $id}) { clientMutationId } }', 'variables': {'id': 'NODE'}}))
        for data in ({'errors': [{'message': 'failed'}]}, {'data': {'deleteLabel': None}}, {}):
            with mock.patch.object(taskq, 'coordination_api', return_value=(200, data)), self.assertRaises(RuntimeError):
                github.release('NODE')
        lab = taskq.GitLab('o/r', 'gitlab.example', {'coordination_board': 8, 'coordination_label': 9})
        with mock.patch.object(taskq, 'coordination_api', side_effect=[(200, []), (201, {'id': 77})]):
            self.assertEqual(lab.acquire('owner'), 77)
        with mock.patch.object(taskq, 'coordination_api', side_effect=[(200, []), (400, {'message': {'error': 'Label has already been taken'}})]):
            self.assertIsNone(lab.acquire('owner'))
        for response in ((401, {}), (500, {}), (400, {'message': 'bad request'})):
            with mock.patch.object(taskq, 'coordination_api', side_effect=[(200, []), response]), self.assertRaises(RuntimeError):
                lab.acquire('owner')
        with mock.patch.object(taskq, 'coordination_api', return_value=(204, None)) as api:
            lab.release(77)
        self.assertEqual(api.call_args.args[2:], ('DELETE', 'projects/o%2Fr/boards/8/lists/77'))
        with mock.patch.object(taskq, 'coordination_api') as api, self.assertRaisesRegex(RuntimeError, 'explicitly provisioned'):
            taskq.GitLab('o/r').acquire('owner')
        api.assert_not_called()

    def test_structured_coordination_transport_rejects_unknown_success(self):
        with mock.patch.object(taskq.shutil, 'which', return_value='gh'), mock.patch.object(taskq.subprocess, 'run') as run:
            run.return_value = subprocess.CompletedProcess([], 1, 'HTTP/2.0 422 Unprocessable Entity\nX-Test: yes\n\n{"errors":[]}', 'failed')
            self.assertEqual(taskq.coordination_api('gh', None, 'POST', 'test', {}), (422, {'errors': []}))
            self.assertEqual(run.call_args.kwargs['timeout'], 30)
            for out in ('not HTTP', 'HTTP/2.0 201 Created\n\nnot JSON'):
                run.return_value = subprocess.CompletedProcess([], 0, out, '')
                with self.assertRaises(RuntimeError):
                    taskq.coordination_api('gh', None, 'POST', 'test')


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
            result = self.run_cli('close', '1', '--text', 'exact candidate accepted; users get it on main; open: none')
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

    def test_supervisor_close_needs_a_verdict(self):
        # #567: the manager gets a verdict, never a SHA; refused before anything is published
        for text in ('', self.sha, self.sha[:7], f'merged {self.sha}', 'Published it', 'accepted; users get it'):
            with self.subTest(text=text), self.assertRaisesRegex(SystemExit, 'needs a verdict'):
                self.run_cli('close', '1', *(['--text', text] if text else []))
        self.assertEqual((self.task(1)['state'], self.main_sha()), ('review', self.base))
        self.close()
        self.assertIn(f'\n\nexact candidate accepted; users get it on main; open: none\n\npublished {self.sha}', self.board.issues[1]['comments'][-1])

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
            self.assertIsNone(self.board.guard)

    def test_main_advance_during_ci_refuses_push_without_losing_work(self):
        self.git('checkout', 'main')
        self.git('commit', '--allow-empty', '-m', 'concurrent main')
        newer = self.git('rev-parse', 'HEAD')

        def ci(*_):
            self.git('push', 'origin', 'main')  # main advances after the ancestry check
            return self.checks
        with mock.patch.object(taskq.subprocess, 'run', REAL_RUN), mock.patch.object(taskq, 'run_api', side_effect=ci), \
                self.assertRaisesRegex(SystemExit, 'rejected'):
            self.run_cli('close', '1', '--text', 'accepted; users get it; open: none')
        self.assertEqual((self.task(1)['state'], self.main_sha()), ('review', newer))

    def test_gitlab_candidate_requires_latest_exact_pipeline(self):
        taskq.CONFIG.update(board='gitlab')
        for pipelines in ([], [{'sha': 'b' * 40, 'status': 'success'}], [{'sha': self.sha, 'status': 'failed'}]):
            with mock.patch.object(taskq.subprocess, 'run', REAL_RUN), mock.patch.object(taskq, 'run_api', return_value=pipelines), \
                    self.assertRaisesRegex(SystemExit, 'CI is not green'):
                self.run_cli('close', '1', '--text', 'accepted; users get it; open: none')
            self.assertEqual(self.main_sha(), self.base)
        with mock.patch.object(taskq.subprocess, 'run', REAL_RUN), \
                mock.patch.object(taskq, 'run_api', return_value=[{'sha': self.sha, 'status': 'success'}]) as api:
            self.run_cli('close', '1', '--text', 'accepted; users get it; open: none')
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
        with mock.patch.object(taskq.subprocess, 'run', side_effect=self.cli), mock.patch.object(taskq.shutil, 'which', side_effect=lambda name: name):
            return self.run_cli('close', *(numbers or ['1']), '--text', 'accepted; users see it; open: none')

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
        self.assertEqual(self.board.issues[1]['comments'][-1], f'**close** · claude:01234567\n\naccepted; users see it; open: none\n\nmerged {"c" * 40}')

    def test_close_external_keeps_the_branch(self):
        taskq.CONFIG['workspace'] = 'external'  # #477: no --delete-branch; the repo's own policy decides
        self.close()
        self.assertEqual(self.merges(), [['pr', 'merge', '7', '--squash', '--match-head-commit', 'a' * 40, '-R', 'o/r']])
        self.assertEqual(self.board.issues[1]['comments'][-1], f'**close** · claude:01234567\n\naccepted; users see it; open: none\n\nmerged {"c" * 40}\n\nkept: owned by host')

    def test_close_batch_goes_on_after_a_failure(self):
        self.add()
        self.run_cli('take', '2')
        self.run_cli('result', '2', '--sha', 'b' * 40)
        self.branches['taskq-2'], self.checks['b' * 40] = [{'number': 8, 'headRefOid': 'b' * 40, 'baseRefName': 'main'}], [[('completed', 'success')]]
        self.checks['a' * 40] = [[('completed', 'failure')]]
        with self.assertRaisesRegex(SystemExit, 'not closed: #1$'):
            self.close('1', '2')
        self.assertEqual((self.task(1)['state'], self.board.issues[2]['state']), ('ready', 'closed'))

        self.assertIsNone(self.board.guard)  # acknowledged CI-red requeue and other close are settled

    def test_head_is_not_the_result(self):
        for change in ({'headRefOid': 'b' * 40}, {'baseRefName': 'release'}):
            self.prs[0].update(change)
            with self.assertRaisesRegex(SystemExit, 'do not match the result'):
                self.close()
            self.prs[0].update(headRefOid='a' * 40, baseRefName='main')
        self.assertEqual((self.task(1)['state'], [call[1] for call in self.calls]), ('review', ['list', 'list']))

    def test_pending_or_missing_tests_refuse_once_without_requeue(self):
        before = self.board.get(1)
        for checks in ([], [('queued', None)], [('completed', 'success'), ('in_progress', None)]):
            self.calls.clear()
            self.checks['a' * 40] = [checks, [('completed', 'success')]]
            with self.subTest(checks=checks), mock.patch.object(taskq, 'dispatch') as dispatch, \
                    mock.patch.object(taskq.time, 'sleep', side_effect=AssertionError('CI must not wait')) as sleep, \
                    self.assertRaisesRegex(SystemExit, 'pending or missing'):
                self.close()
            self.assertEqual(self.board.get(1), before)
            self.assertEqual(self.merges(), [])
            self.assertEqual(sum('check-runs' in call[3] for call in self.calls if call[0] == 'api'), 1)
            self.assertIsNone(self.board.guard)
            dispatch.assert_not_called()
            sleep.assert_not_called()
        self.close()  # a later attempt sees the green exact head
        self.assertEqual(self.board.issues[1]['state'], 'closed')

    def test_batch_pending_after_close_releases_and_dispatches_only_changes(self):
        self.add()
        self.run_cli('take', '2')
        self.run_cli('result', '2', '--sha', 'b' * 40)
        self.branches['taskq-2'] = [{'number': 8, 'headRefOid': 'b' * 40, 'baseRefName': 'main'}]
        self.checks['b' * 40] = [[('queued', None)]]
        before = self.board.get(2)
        self.checks['a' * 40] = [[('queued', None)]]
        with mock.patch.object(taskq, 'dispatch') as dispatch, self.assertRaisesRegex(SystemExit, 'not closed: #1 #2$'):
            self.close('1', '2')
        self.assertEqual(self.task(1)['state'], 'review')
        self.assertEqual(self.board.get(2), before)
        self.assertIsNone(self.board.guard)
        dispatch.assert_not_called()
        self.checks['a' * 40] = [[('completed', 'success')]]
        with mock.patch.object(taskq, 'dispatch') as dispatch, self.assertRaisesRegex(SystemExit, 'not closed: #2$'):
            self.close('1', '2')
        self.assertEqual(self.board.issues[1]['state'], 'closed')
        self.assertEqual(self.board.get(2), before)
        self.assertIsNone(self.board.guard)
        dispatch.assert_called_once_with('close', [1])

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

    def test_gitlab_checks_pipeline_once_and_merges_at_sha(self):
        self.gitlab()
        self.checks['mr'] = [[], [('a' * 40, 'running'), ('b' * 40, 'success')], [('a' * 40, 'success')]]
        before = self.board.get(1)
        for _ in range(2):
            with mock.patch.object(taskq, 'dispatch') as dispatch, self.assertRaisesRegex(SystemExit, 'pending or missing'):
                self.close()
            self.assertEqual(self.board.get(1), before)
            self.assertIsNone(self.board.guard)
            dispatch.assert_not_called()
        self.assertEqual(self.close(), '#1 closed\n')
        self.assertEqual(self.calls[0], ['mr', 'list', '--source-branch', 'taskq-1', '--output', 'json', '-R', 'https://git.example/o/r'])
        self.assertEqual(sum(call[:4] == ['api', '-X', 'GET', 'projects/o%2Fr/merge_requests/7/pipelines'] and call[-2:] == ['--hostname', 'git.example']
                             for call in self.calls), 3)
        self.assertEqual([call for call in self.calls if call[:2] == ['mr', 'merge']],
                         [['mr', 'merge', '7', '--squash', '--remove-source-branch', '--sha', 'a' * 40, '--auto-merge=false', '--yes', '-R', 'https://git.example/o/r']])
        self.assertEqual(self.board.issues[1]['comments'][-1], f'**close** · claude:01234567\n\naccepted; users see it; open: none\n\nmerged {"d" * 40}')

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


class HermesAdmission(Base):
    def setUp(self):
        super().setUp()
        self.native, self.worker = FakeRuntime(), FakeRuntime()
        self.native.available = lambda: True
        self.bridge = taskq.Hermes(self.native)
        self.kinds = {'hermes': self.bridge, 'codex': self.worker}
        taskq.CONFIG.update(limits={'codex': 1}, repo='o/r')
        self.patch = mock.patch.object(taskq, 'runtimes', return_value=self.kinds)
        self.patch.start()
        self.addCleanup(self.patch.stop)

    def native_env(self, sid=SESSION):
        return mock.patch.dict(os.environ, {'TASKQ_RUNTIME': 'hermes', 'HERMES_SESSION_ID': sid})

    def test_identity_no_codex_impersonation(self):
        with mock.patch.dict(os.environ, {'TASKQ_RUNTIME': 'hermes', 'CODEX_THREAD_ID': SESSION}, clear=True):
            with self.assertRaisesRegex(SystemExit, 'genuine HERMES_SESSION_ID'):
                self.add()
            self.assertEqual(self.board.issues, {})
        with mock.patch.dict(os.environ, {'HERMES_SESSION_ID': '   '}, clear=True):
            with self.assertRaisesRegex(SystemExit, 'genuine HERMES_SESSION_ID'):
                taskq.session()
        with self.native_env():
            self.assertEqual(taskq.session(), {'runtime': 'hermes', 'session': SESSION})
            self.assertNotIn('HERMES_SESSION_ID', taskq.worker_env())
            self.assertNotIn('CODEX_THREAD_ID', taskq.worker_env())
        with mock.patch.dict(os.environ, {'HERMES_SESSION_ID': SESSION, 'CODEX_THREAD_ID': 'codex'}, clear=True):
            with self.assertRaisesRegex(SystemExit, 'ambiguous'):
                taskq.session()

    def test_native_supervisor_then_codex_worker(self):
        with self.native_env():
            self.add('native', '--runtime', 'codex')
        item = self.task(1)
        self.assertEqual(item['pm']['runtime'], 'hermes')
        self.assertEqual(item['supervisor']['runtime'], 'hermes')
        self.assertIsNone(item['claim']['session'])
        self.assertEqual(self.native.names, ['S1 HRM native (mac)'])
        self.assertFalse(self.worker.sessions)
        with mock.patch.dict(os.environ, {'TASKQ_RUNTIME': 'codex', 'CODEX_THREAD_ID': 's-S1'}):
            with self.assertRaisesRegex(SystemExit, 'only it'):
                self.run_cli('run', '1')
        with self.native_env('s-S1'):
            self.run_cli('run', '1')
        self.assertEqual(self.task(1)['claim']['runtime'], 'codex')
        self.assertEqual(self.worker.names, ['T1 HRM native (mac)'])
        self.native.sessions['s-S1'] = 'idle'
        with mock.patch.dict(os.environ, {'TASKQ_RUNTIME': 'codex', 'CODEX_THREAD_ID': 's-T1'}):
            self.run_cli('result', '1', '--sha', 'a' * 40)
        self.assertIn('review #1', self.native.sent[-1][1])
        self.assertFalse(self.worker.sent)

    def test_bridge_failure_preserves_board_and_order(self):
        with self.native_env():
            self.add('native', '--runtime', 'codex')
        item = self.task(1)
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(item, 'doing', 'run', order='run')
        before = self.board.get(1)['body']
        for available, state in ((False, True), (True, None), (True, 'invalid')):
            with self.subTest(available=available, state=state):
                self.native.available = lambda: available
                self.native.sessions['s-S1'] = state
                errors = io.StringIO()
                with contextlib.redirect_stderr(errors), self.assertRaises(SystemExit) as error:
                    self.run_cli('tick', '--quiet')
                self.assertEqual(error.exception.code, 1)
                self.assertIsNone(self.board.guard)  # no effects occurred: failed admission releases its grant
                self.assertRegex(errors.getvalue(), 'unavailable|state unknown')
                self.assertEqual(self.board.get(1)['body'], before)
                self.assertFalse(self.worker.sessions)
        self.native.available = lambda: True
        self.native.sessions['s-S1'] = False
        self.run_cli('tick', '--quiet')
        self.assertFalse(self.worker.sessions)  # dead supervisor recovered first
        self.assertEqual(self.task(1)['supervisor']['session'], 's-S1.1')
        self.assertEqual(self.task(1)['raw']['order'], 'run')
        self.run_cli('tick', '--quiet')
        self.assertTrue(self.worker.sessions)

    def test_runtime_file_boundary_with_fake_downstream(self):
        path = self.root / 'hermes.py'
        path.write_text("""def available(): return True
def spawn(name, prompt, cwd):
    assert name.startswith('S1 HRM ')
    return 'fixture-native-session'
def send(session, text): return session
def alive(session): return True
def link(session): return None
def state(session): return 'idle'
def retire(gone, running=True): pass
""")
        taskq.CONFIG['runtimes'] = {'hermes': 'hermes.py'}
        adapter = taskq.Hermes(taskq.load_file('hermes.py'))
        self.assertEqual(adapter.spawn('S1 HRM native (mac)', 'prompt', self.root), 'fixture-native-session')
        self.assertEqual(adapter.state('fixture-native-session'), 'idle')
        self.assertEqual(adapter.send('fixture-native-session', 'review #1'), 'fixture-native-session')

    def test_missing_bridge_and_required_lifecycle_fail_closed(self):
        item = {'pm': {'runtime': 'hermes', 'session': SESSION}}
        with self.assertRaisesRegex(SystemExit, 'not configured'):
            taskq.lead(item, {'codex': self.worker}, admit=True)
        item['pm']['session'] = None
        with self.assertRaisesRegex(SystemExit, 'genuine HERMES_SESSION_ID'):
            taskq.lead(item, self.kinds, admit=True)
        del self.native.available
        with self.assertRaisesRegex(SystemExit, 'needs spawn/send'):
            self.bridge.spawn('S1 HRM native (mac)', 'prompt', self.root)
        self.native.available = lambda: True
        with mock.patch.object(self.native, 'spawn', return_value=None):
            with self.assertRaisesRegex(SystemExit, 'no genuine session id'):
                self.bridge.spawn('S1 HRM native (mac)', 'prompt', self.root)
        with self.native_env():
            out = self.run_cli('arm', 'tick')
        self.assertIn('No Codex resume route', out)
        self.assertNotIn('codex exec', out)

    def test_wait_native_wake_failure_preserves_receipt_then_delivers_once(self):
        with self.native_env():
            self.add('native', '--runtime', 'codex')
        item = self.task(1)
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(item, 'ask', 'ask', 'Need owner decision')
        self.native.sessions[SESSION] = 'idle'
        self.native.owned = lambda sid: sid == SESSION
        receipt = self.root / '.taskq' / f'wait-{SESSION}.json'
        receipt.write_text(json.dumps({'1': 'doing'}))
        before = receipt.read_bytes()
        self.native.wake_manager = mock.Mock(side_effect=TimeoutError('no verified message.complete'))
        with self.native_env():
            with self.assertRaisesRegex(SystemExit, 'wake failed.*receipt unchanged'):
                self.wait_and_ack( '--window', '0')
        self.assertEqual(receipt.read_bytes(), before)
        self.native.wake_manager = mock.Mock(return_value=SESSION)
        with self.native_env():
            self.board.release(self.board.guard)  # fixture confirms failed wake is quiescent before exact-token recovery
            self.assertEqual(self.wait_and_ack( '--window', '0'), 'ask #1\n')
            self.assertEqual(self.wait_and_ack( '--window', '0'), 'tick\n')
        self.native.wake_manager.assert_called_once_with(SESSION, 'ask #1 [event 1:3]')
        self.assertEqual(receipt.read_bytes(), before)  # legacy file is no longer receipt authority

    def test_wait_busy_unknown_and_foreign_manager_never_consume_for_wake(self):
        with self.native_env():
            self.add('native', '--runtime', 'codex')
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(self.task(1), 'ask', 'ask', 'Need owner decision')
        self.native.owned = lambda sid: sid == SESSION
        self.native.wake_manager = mock.Mock(return_value=SESSION)
        receipt = self.root / '.taskq' / f'wait-{SESSION}.json'
        for status in (True, None):
            self.native.sessions[SESSION] = status
            with self.native_env(), self.assertRaises(SystemExit):
                self.wait_and_ack( '--window', '0')
            self.assertFalse(receipt.exists())
        self.native.wake_manager.assert_not_called()
        with self.native_env('different-manager'):
            self.assertIn('ask #1 [event', self.run_cli('wait', '--pm', SESSION, '--window', '0'))
        self.native.wake_manager.assert_not_called()  # --pm is no cross-gateway attach
        self.native.owned = lambda sid: False
        self.assertFalse(receipt.exists())
        with self.native_env():
            self.assertEqual(self.wait_and_ack( '--window', '0'), 'ask #1\n')
        self.native.wake_manager.assert_not_called()  # ordinary printing works without an owned handle



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

    def test_local_limits_replace_defaults_and_reserve_five_codex_slots(self):
        taskq.CONFIG['limits'] = {'fake': 8, 'codex': 1}
        with mock.patch.object(taskq, 'dispatch'):
            for n in range(7):
                self.add(str(n), '--runtime', 'codex', '--host', 'mac')
            self.add('other runtime', '--runtime', 'fake', '--host', 'mac')
            self.add('any', '--host', 'mac')
        config = dict(taskq.CONFIG['limits'])
        with mock.patch.dict(os.environ, {'TASKQ_LIMITS': '{"codex":5}', 'TASKQ_HOST_ONLY': 'mac'}), \
                mock.patch.object(taskq, 'runtimes', return_value={'fake': self.fake, 'codex': self.fake}):
            self.run_cli('tick')
            self.assertEqual([self.task(n)['state'] for n in range(1, 10)], ['doing'] * 5 + ['ready'] * 4)
            self.assertTrue(all(self.task(n)['claim']['runtime'] == 'codex' for n in range(1, 6)))
            self.assertTrue(all(self.task(n)['supervisor']['runtime'] == 'fake' for n in range(1, 6)))
            for n in range(1, 6):
                with self.acting(f's-S{n}'):
                    self.run_cli('run', str(n))
            self.assertEqual(len([name for name in self.fake.names if name.startswith('T')]), 5)
            self.assertEqual(taskq.worker_env()['TASKQ_LIMITS'], '{"codex":5}')
            self.assertEqual(taskq.worker_env()['TASKQ_HOST_ONLY'], 'mac')
        self.assertEqual(taskq.CONFIG['limits'], config)

    def test_strict_host_preserves_unlabelled_foreign_and_conflicting_tasks(self):
        with mock.patch.object(taskq, 'dispatch'):
            self.add('unlabelled')
            self.add('foreign', '--host', 'win')
            self.add('conflicting', '--host', 'mac')
            self.board.issues[3]['labels'].append('host-win')
            self.add('wanted', '--host', 'mac')
            self.legacy(1)
            self.fake.sessions['s-T1'] = False  # strict pass must not requeue or retire this foreign-scope session
        before = json.dumps({n: self.board.issues[n] for n in (1, 2, 3)}, sort_keys=True)
        taskq.CONFIG['limits'] = {'fake': 2}
        with mock.patch.dict(os.environ, {'TASKQ_HOST_ONLY': 'mac'}):
            self.run_cli('tick')
            self.assertEqual(self.task(4)['state'], 'doing')
            self.assertEqual(json.dumps({n: self.board.issues[n] for n in (1, 2, 3)}, sort_keys=True), before)
            self.assertFalse(taskq.stale({})('fake', 1, 's-T1', False))
            with self.assertRaisesRegex(SystemExit, 'outside TASKQ_HOST_ONLY'):
                self.run_cli('take', '2')

    def test_strict_host_rechecks_stale_list_and_pending_worker_order(self):
        with mock.patch.object(taskq, 'dispatch'):
            self.add('wanted', '--host', 'mac')
        listed = json.loads(json.dumps(self.board.list(None)))
        self.board.issues[1]['labels'].remove('host-mac')
        with mock.patch.dict(os.environ, {'TASKQ_HOST_ONLY': 'mac'}), mock.patch.object(self.board, 'list', return_value=listed):
            self.run_cli('tick')
        self.assertEqual(getattr(self.fake, 'names', []), [])
        self.board.issues[1]['labels'].append('host-mac')
        self.run_cli('tick')
        with mock.patch.object(taskq, 'dispatch'), self.acting('s-S1'):
            self.run_cli('run', '1')
        listed = json.loads(json.dumps(self.board.list(None)))
        self.board.issues[1]['labels'].remove('host-mac')
        before = json.dumps(self.board.issues, sort_keys=True)
        with mock.patch.dict(os.environ, {'TASKQ_HOST_ONLY': 'mac'}), mock.patch.object(self.board, 'list', return_value=listed):
            self.run_cli('tick')
        self.assertEqual(self.fake.names, ['S1 UNK wanted (mac)'])
        self.assertEqual(json.dumps(self.board.issues, sort_keys=True), before)

    def test_local_limits_hold_pending_orders_without_rebinding_claims(self):
        taskq.CONFIG['limits'] = {'fake': 3}
        for n in range(3):
            self.add(str(n))
        with mock.patch.object(taskq, 'dispatch'):
            for n in range(1, 4):
                with self.acting(f's-S{n}'):
                    self.run_cli('run', str(n))
        before = self.task(2)['claim'].copy()
        with mock.patch.dict(os.environ, {'TASKQ_LIMITS': '{"fake":1}'}):
            self.run_cli('tick')
            self.assertEqual(self.task(1)['claim']['session'], 's-T1')
            self.assertEqual(self.task(2)['claim'], before)
            self.assertEqual(self.task(2)['raw']['order'], 'run')
            self.assertEqual(self.task(3)['claim']['session'], None)
        with mock.patch.dict(os.environ, {'TASKQ_LIMITS': '{"codex":5}'}):
            self.run_cli('tick')
            self.assertEqual(self.task(2)['claim'], before)
            self.assertEqual(self.task(2)['raw']['order'], 'run')
            self.assertTrue(self.fake.sessions['s-T1'])

    def test_local_limits_count_fresh_active_claims_before_pending_admission(self):
        taskq.CONFIG['limits'] = {'fake': 2}
        for n in range(2):
            self.add(str(n), '--host', 'mac')
        with mock.patch.object(taskq, 'dispatch'):
            for n in (1, 2):
                with self.acting(f's-S{n}'):
                    self.run_cli('run', str(n))
        listed = json.loads(json.dumps(self.board.list(None)))  # both listed as pending, but #2 has since started
        current = self.task(2)
        active = {**current['claim'], 'session': 's-T2'}
        self.board.issues[2]['body'] = taskq.block('g', {**current['raw'], 'claim': active, 'order': None})
        self.fake.sessions['s-T2'] = True
        # An active worker outside this invocation's strict host scope still consumes the local capacity.
        for outside_scope in (False, True):
            with self.subTest(outside_scope=outside_scope):
                if outside_scope:
                    self.board.issues[2]['labels'].remove('host-mac')
                before = json.dumps(self.board.issues, sort_keys=True)
                with mock.patch.dict(os.environ, {'TASKQ_LIMITS': '{"fake":1}', 'TASKQ_HOST_ONLY': 'mac'}), \
                        mock.patch.object(self.board, 'list', return_value=listed), \
                        mock.patch.object(self.board, 'metadata', create=True,
                                          side_effect=lambda n: {k: v for k, v in self.board.get(n).items() if k != 'comments'}) as metadata:
                    self.run_cli('tick')
                self.assertIn(mock.call(2), metadata.call_args_list)  # metadata, not the stale list, owns capacity
                self.assertEqual(self.task(1)['claim']['session'], None)
                self.assertEqual(self.task(1)['raw']['order'], 'run')
                self.assertEqual(self.task(2)['claim'], active)
                self.assertEqual(json.dumps(self.board.issues, sort_keys=True), before)
                self.assertFalse(any(name.startswith('T') for name in self.fake.names))

    def test_local_scope_reports_and_protects_newly_spawned_supervisor(self):
        with mock.patch.object(taskq, 'dispatch'):
            self.add('new', '--host', 'mac')
        retireable = []
        def retire(gone, *args, **kwargs):
            retireable.append(gone('fake', 1, 's-S1', False))
        with mock.patch.dict(os.environ, {'TASKQ_HOST_ONLY': 'mac', 'TASKQ_LIMITS': '{"fake":1}'}), \
                mock.patch.object(taskq, 'retire', retire):
            report = self.run_cli('tick')
        self.assertEqual(retireable, [False])  # even if the new supervisor's first turn already stopped
        self.assertEqual(self.task(1)['supervisor']['session'], 's-S1')
        self.assertIn('In work 1 · Waiting for answer 0 · Ready 0', report)
        self.assertIn('| [#1 new](https://board/1) | doing | fake | [s-S1](https://watch/s-S1) |', report)

    def test_retirement_fresh_read_keeps_current_open_sessions(self):
        self.add('owned', '--host', 'mac')
        earlier = self.task(1)
        for field in ('claim', 'supervisor'):
            with self.subTest(field=field):
                current = {**earlier['raw'], field: {'runtime': 'fake', 'session': 'fresh-session', 'name': 'mac'}}
                self.board.issues[1]['body'] = taskq.block('g', current)
                for state, retireable in (('open', False), ('closed', True)):
                    with self.subTest(state=state), mock.patch.dict(os.environ, {'TASKQ_HOST_ONLY': 'mac'}):
                        self.board.issues[1]['state'] = state
                        self.assertEqual(taskq.stale({1: earlier})('fake', 1, 'fresh-session', False), retireable)
                self.board.issues[1]['state'] = 'open'

    def test_local_scope_never_overwrites_fresh_foreign_execution(self):
        with mock.patch.object(taskq, 'dispatch'):
            self.add('owned', '--host', 'mac')
            self.legacy(1)
        original = json.loads(json.dumps(self.board.issues[1]))
        for state in ('doing', 'waiting'):
            for changed_read in (1, 2):  # stale list, then a change after the accounting refresh but before action
                with self.subTest(state=state, changed_read=changed_read):
                    self.board.issues[1] = json.loads(json.dumps(original))
                    if state == 'waiting':
                        self.board.issues[1]['labels'].remove('q-doing')
                        self.board.issues[1]['labels'].append('q-waiting')
                    listed = json.loads(json.dumps(self.board.list(None)))
                    current = self.task(1)
                    foreign = json.loads(json.dumps(self.board.issues[1]))
                    foreign['labels'] = [label for label in foreign['labels'] if not label.startswith('q-')] + ['q-doing']
                    foreign['body'] = taskq.block('g', {**current['raw'], 'order': 'run',
                        'claim': {'runtime': 'fake', 'session': 'foreign-session', 'name': 'other-host'},
                        'pm': {'runtime': 'fake', 'session': 'foreign-manager', 'name': 'other-host'}})
                    reads = 0
                    def get(n):
                        nonlocal reads
                        reads += 1
                        if reads == changed_read:
                            self.board.issues[1] = foreign
                        return dict(self.board.issues[n])
                    self.fake.sessions['s-T1'] = False
                    with mock.patch.dict(os.environ, {'TASKQ_HOST_ONLY': 'mac', 'TASKQ_LIMITS': '{"fake":1}'}), \
                            mock.patch.object(self.board, 'list', return_value=listed), mock.patch.object(self.board, 'get', get), \
                            mock.patch.object(self.fake, 'alive', side_effect=AssertionError('foreign liveness queried')), \
                            mock.patch.object(self.fake, 'spawn', side_effect=AssertionError('foreign claim replaced')), \
                            mock.patch.object(self.fake, 'send', side_effect=AssertionError('foreign session sent to')):
                        self.run_cli('tick')
                    self.assertEqual(self.board.issues[1], foreign)

    def test_invalid_local_scope_fails_before_mutation_or_dispatch(self):
        for value in ('', '{}', '[]', '{"codex":true}', '{"codex":-1}', '{"codex":1.5}', '{"":5}'):
            with self.subTest(value=value), mock.patch.dict(os.environ, {'TASKQ_LIMITS': value}), \
                    mock.patch.object(taskq, 'dispatch') as dispatch, self.assertRaisesRegex(SystemExit, 'TASKQ_LIMITS'):
                self.add()
            dispatch.assert_not_called()
            self.assertEqual(self.board.issues, {})
        for host in ('', 'win'):
            with mock.patch.dict(os.environ, {'TASKQ_HOST_ONLY': host}), self.assertRaisesRegex(SystemExit, 'TASKQ_HOST_ONLY'):
                self.add()
            self.assertEqual(self.board.issues, {})

    def test_spawn_one_per_free_slot(self):
        # #525 (R2, R3): a ready task gets one supervisor in the manager's runtime; its worker's slot is held from then on
        self.add('one')
        self.add('two')
        self.add('elsewhere', '--host', 'win', '--priority', '1')
        out = self.run_cli('tick')
        one = self.task(1)
        self.assertEqual((one['state'], one['supervisor'], one['claim']), ('doing', {'runtime': 'fake', 'session': 's-S1', 'name': 'mac'},
                                                                         {'runtime': 'fake', 'session': None, 'name': 'mac'}))
        self.assertEqual((self.task(2)['state'], self.task(3)['state'], self.fake.names), ('ready', 'ready', ['S1 UNK one (mac)']))
        self.assertIn('You are the taskq supervisor S1 of task #1: one', self.fake.prompts['s-S1'])
        self.assertIn('**spawn** · claude:01234567\n\nsupervisor s-S1\nhttps://watch/s-S1', self.board.issues[1]['comments'])
        self.assertIn('| [#1 one](https://board/1) | doing | fake | [s-S1](https://watch/s-S1) |', out)
        self.assertTrue(out.startswith(f'{self.root.name} · [board](https://github.com/o/r/issues)\n'))
        self.assertTrue(out.endswith('\n\nMode: events · arm: <arm_tick>\n'))
        with self.acting('s-S1'):
            self.run_cli('run', '1')  # the supervisor orders; its event pass spawns the worker
        self.assertEqual((self.task(1)['claim']['session'], self.fake.names[-1]), ('s-T1', 'T1 UNK one (mac)'))
        self.assertIn('taskq worker for task #1: one', self.fake.prompts['s-T1'])
        self.assertEqual(self.notes(1), ['**add**', '**spawn**', '**run**', '**spawn**'])
        self.assertTrue(self.board.issues[1]['comments'][-1].endswith('worker s-T1\nhttps://watch/s-T1'))
        self.fake.sessions.update({'s-T1': False, 's-S1': 'idle'})  # the worker died, the supervisor sleeps between turns
        self.run_cli('tick')
        self.assertEqual((self.task(1)['state'], self.task(1)['claim']['session'], self.task(2)['state']), ('doing', None, 'ready'))
        self.assertIn('worker s-T1 is gone', self.board.issues[1]['comments'][-1])
        self.assertEqual(self.fake.sent, [('s-S1', 'gone #1 [event 1:5]: read your issue')])  # woken once
        self.run_cli('tick')
        self.assertEqual((len(self.fake.sent), self.fake.names), (1, ['S1 UNK one (mac)', 'T1 UNK one (mac)']))

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
                   'unknown runtime': {'CODEX_THREAD_ID': 'x', 'TASKQ_RUNTIME': 'grok'},
                   # #574: the final rendering client wins over the session running the command, never the worker's runtime
                   'final codex, run in claude': {'CLAUDE_CODE_SESSION_ID': 'mgr', 'TASKQ_CLIENT': 'codex'},
                   'final claude, run in codex': {'CODEX_THREAD_ID': 'mgr', 'TASKQ_CLIENT': 'claude'},
                   'final dot, run in codex': {'CODEX_THREAD_ID': 'mgr', 'TASKQ_CLIENT': 'dot'}}
        for source, env in sources.items():
            with self.subTest(source), mock.patch.dict(os.environ, env, clear=True):
                self.assertEqual(taskq.row(item, kinds, 'mac'), wrapper.replace(
                    'https://alexkirs.github.io/taskq/open.html#', '') if source in ('codex', 'final codex, run in claude') else wrapper)
                self.assertEqual(taskq.row(item, kinds, 'win'), '| [#7](https://board/7) | doing | codex | 019a-thr on mac |')
                self.assertEqual(taskq.row({**item, 'claim': None, 'state': 'ready'}, kinds, 'mac'), '| [#7](https://board/7) | ready | any |  |')
                claimed = {**item, 'claim': {'runtime': 'claude', 'session': 'abcdef12-3456', 'name': 'mac'}}
                with mock.patch.object(claude, 'link', return_value='https://claude.ai/code/session_X'):
                    self.assertEqual(taskq.row(claimed, kinds, 'mac'), '| [#7](https://board/7) | doing | claude | [abcdef12](https://claude.ai/code/session_X) |')

    def test_status_report_one_snapshot_read_only(self):
        """#574 (R6): counters and rows from one list and one filter; review is in work; blocked, waiting and later are never
        ready, nor is one whose dep is an open ordinary issue (same list); `status` lists once and writes, pulls, dispatches
        and spawns nothing; `arm tick` output is no sender proof; titles, reasons and options stay one cell."""
        taskq.CONFIG['limits'] = {'fake': 0}  # the adds' passes start nothing
        with contextlib.redirect_stdout(io.StringIO()):
            self.add('gone')
            self.board.issues[1]['state'] = 'closed'
            for title in ('doing', 'review', 'asking', 'a|b\n[x]'):
                self.add(title)
            self.add('dep closed', '--deps', '1')
            self.add('dep open', '--deps', '5')
            taskq.move(self.task(7), 'ready', 'requeue')  # ready, its dep still open: blocked
            self.add('waits', '--deps', '5')
            self.add('parked')
            self.run_cli('later', '9', '--text', 'not now')
            self.unmanaged()
            self.add('orphan')
            self.manager('fake')
            self.board.add('plain bug', 'no taskq block', ['bug'])  # #11: an open ordinary issue, never a task
            self.add('dep issue', '--deps', '11')
            taskq.move(self.task(12), 'ready', 'requeue')  # ready, its dep an open non-task issue: blocked, not Ready
            for n in (2, 3, 4):
                self.legacy(n)
            self.run_cli('result', '3', '--sha', 'a' * 40, '--text', 'done')
            self.run_cli('ask', '4', '--text', 'Built A | B.\nKeep it?', '--option', 'keep\nit', '--option', 'drop', '--recommend', '2')
        before, lists = json.dumps(self.board.issues, sort_keys=True), []
        listing = self.board.list
        with mock.patch.object(self.board, 'list', lambda state: lists.append(state) or listing(state)), \
                mock.patch.object(self.board, 'get', side_effect=AssertionError('status left its one list')), \
                mock.patch.object(taskq, 'refresh', side_effect=AssertionError('status pulled')), \
                mock.patch.object(taskq, 'start_pass', side_effect=AssertionError('status dispatched')):
            out = self.run_cli('status')
        self.assertEqual((lists, json.dumps(self.board.issues, sort_keys=True), getattr(self.fake, 'names', [])), ([None], before, []))
        self.assertEqual(out, textwrap.dedent(f'''\
            {self.root.name} · [board](https://github.com/o/r/issues)
            In work 2 · Waiting for answer 1 · Ready 2

            | Task | State | Runtime | Session |
            |---|---|---|---|
            | [#2 doing](https://board/2) | doing | fake | [s-T2](https://watch/s-T2) |
            | [#3 review](https://board/3) | review | fake | [s-T3](https://watch/s-T3) |
            | [#5 a\\|b \\[x\\]](https://board/5) | ready | any |  |
            | [#6 dep closed](https://board/6) | ready | any |  |
            | [#7 dep open](https://board/7) | blocked (#5 open) | any |  |
            | [#8 waits](https://board/8) | waiting (#5) | any |  |
            | [#10 orphan](https://board/10) | blocked (no manager) | any |  |
            | [#12 dep issue](https://board/12) | blocked (#11 open) | any |  |

            Questions (answer N.M):

            | Question | Brief reason | Options |
            |---|---|---|
            | [#4 asking](https://board/4) | Built A \\| B. | 4.1 keep it · 4.2 drop ★ |

            Later: [#9 parked](https://board/9)

            Mode: events · arm: <arm_tick>
            '''))
        self.run_cli('arm', 'tick', SESSION)  # prints a sender prompt; taskq still fills nothing (R6 item 6)
        out = self.run_cli('status')
        self.assertEqual((out.count('<arm_tick>'), out.count('<'), out.endswith('\nMode: events · arm: <arm_tick>\n')), (1, 1, True))

    def test_status_unreadable_board_prints_no_fresh_report(self):
        # #580: a failed read must not turn unavailable state into fresh-looking empty counters.
        output = io.StringIO()
        with mock.patch.object(self.board, 'list', side_effect=SystemExit('invalid_grant')) as listed, \
                contextlib.redirect_stdout(output), self.assertRaisesRegex(SystemExit, 'invalid_grant'):
            taskq.cmd_status(None)
        listed.assert_called_once_with(None)
        self.assertEqual(output.getvalue(), '')  # the PM supplies the external blocker block, not fabricated rows
        self.assertEqual(getattr(self.fake, 'names', []), [])

    def test_status_empty_queue_is_compact(self):
        # #574: an empty table or section is left out, no placeholder
        self.assertEqual(self.run_cli('status'), f'{self.root.name} · [board](https://github.com/o/r/issues)\n'
                         'In work 0 · Waiting for answer 0 · Ready 0\n\nMode: events · arm: <arm_tick>\n')

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
        self.assertIn('Waiting for answer 1', out)
        self.assertIn('Last log line: error: unsupported model', self.board.issues[1]['comments'][-1])
        self.run_cli('answer', '1', '--text', 'fixed')  # an answer resets the count: the next death requeues
        self.run_cli('tick')
        self.assertEqual(self.task(1)['state'], 'ready')
        self.assertIn('| [#1 T](https://board/1) | blocked (no manager) | any |  |', self.run_cli('tick'))

    def test_other_checkout_codex_supervisor_handle_is_not_death(self):
        self.add()
        before = json.loads(json.dumps(self.board.issues[1]))
        other = self.root / 'other-checkout' / '.taskq'
        other.mkdir(parents=True)
        (other / 'S1.pid').write_text(f'{os.getpid()} s-S1')
        codex = taskq.Codex()  # its root is this checkout; the recorded supervisor belongs to the other one
        with mock.patch.object(taskq, 'runtimes', return_value={'fake': codex}), \
                mock.patch.object(codex, 'spawn', side_effect=AssertionError('duplicate supervisor')), \
                mock.patch.object(codex, 'send', side_effect=AssertionError('duplicate resume')):
            self.assertIsNone(codex.state('s-S1'))
            self.run_cli('tick')
        self.assertEqual(self.board.issues[1], before)
        self.assertTrue((other / 'S1.pid').is_file())

    def test_two_passes_at_once_spawn_one_worker(self):
        # #357 (R2): a tick runs while an event pass spawns; it finds the lock held and starts nothing
        taskq.CONFIG['limits'] = {'fake': 2}
        spawn, err = self.fake.spawn, io.StringIO()

        def overlapping(*spawn_args):
            with contextlib.redirect_stderr(err), self.assertRaisesRegex(SystemExit, 'project guard'):
                self.run_cli('tick')
            return spawn(*spawn_args)
        self.fake.spawn = overlapping
        self.add('one')  # the add's pass spawns #1's supervisor, and the tick runs at that moment
        self.assertEqual(self.fake.names, ['S1 UNK one (mac)'])
        self.assertIsNone(self.board.guard)
        self.fake.spawn = spawn
        stale = [dict(self.board.issues[1], labels=['q-ready'])]  # a list from before the spawn: the re-read sees #1 taken
        with mock.patch.object(self.board, 'list', return_value=stale):
            self.run_cli('tick')
        self.assertEqual(self.fake.names, ['S1 UNK one (mac)'])

    def test_nudge_only_a_silent_worker(self):
        self.unmanaged()
        self.add()
        self.legacy(1)
        self.board.issues[1]['updated_at'] = taskq.datetime.now(taskq.timezone.utc).isoformat()
        self.run_cli('tick')
        self.assertEqual(self.fake.sent, [])
        self.fake.sessions['s-T1'] = 'idle'
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
        self.fake.sessions[self.task(1)['claim']['session']] = 'idle'
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
        out = self.run_cli('tick').split('Questions (answer N.M):\n\n')[1].split('\n\n')[0]
        self.assertEqual(out.splitlines(), [
            '| Question | Brief reason | Options |', '|---|---|---|',
            '| [#1 one](https://board/1) | Built A and B. · ![1](https://x/shot.png) · https://x/demo.mp4 | 1.1 keep A · 1.2 keep B ★ |',
            '| [#2 two](https://board/2) review | Done X | 2.1 close as is ★ · 2.2 also do Y |'])
        taskq.CONFIG['inline_media'] = False
        self.assertIn(' · https://x/shot.png · ', self.run_cli('tick'))
        for bad, message in (('1.3', 'no option 3'), ('3.1', 'no option 1'), ('1.x', 'codes like'), ('9', 'codes like')):
            with self.assertRaisesRegex(SystemExit, message):
                self.run_cli('answer', '1.2', bad)
        self.assertEqual(self.task(1)['state'], 'ask')  # a bad code moves nothing
        self.fake.sessions['s-T1'] = 'idle'
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, '', '')):
            self.run_cli('answer', '1.2, 2.1')
        self.assertEqual((self.task(1)['state'], self.task(1)['raw']['decision'], self.board.issues[2]['state']), ('doing', None, 'closed'))
        self.assertEqual(self.board.issues[2]['comments'][-1], '**close** · claude:01234567\n\n2.1: close as is')
        self.assertEqual(self.fake.sent[-1], ('s-T1', 'The owner answered your question:\n\n1.2: keep B'))
        self.assertNotIn('Questions', self.run_cli('tick'))
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

    def restricted(self, people, **fields):
        n = self.board.add('restricted', taskq.block('g', {'event_schema': 1, 'deps': [], 'pm': taskq.origin(), **fields}),
                           ['q-ready', 'assignee-only'])
        self.board.issues[n]['assignees'] = people
        return n

    def test_assignee_only_dispatch_and_take(self):
        for people, identity, allowed in ((['alice'], 'alice', True), (['alice'], 'bob', False), (['bob'], 'alice', False),
                                          ([], 'alice', False), (['bob', 'alice'], 'alice', True),
                                          (['alice'], '', False)):
            for boundary in ('tick', 'take'):
                with self.subTest(people=people, identity=identity, boundary=boundary):
                    self.board.issues.clear()
                    self.fake.sessions.clear()
                    self.fake.names = []
                    self.board.user = lambda: identity
                    taskq.CONFIG.update(assignee='alice', me='alice')  # forged filter/cache cannot authenticate
                    n = self.restricted(people)
                    errors = io.StringIO()
                    with contextlib.redirect_stderr(errors):
                        if boundary == 'take' and not allowed:
                            with self.assertRaisesRegex(SystemExit, 'assignee-only'):
                                self.run_cli('take', str(n))
                        else:
                            self.run_cli(boundary, *([str(n)] if boundary == 'take' else []))
                    self.assertEqual(self.task(n)['state'], 'doing' if allowed else 'ready')
                    if not allowed and boundary == 'tick' and 'alice' in people:  # else the `assignee` filter skips it first
                        self.assertIn('assignee-only', errors.getvalue())
        del taskq.CONFIG['assignee']
        self.board.issues.clear()
        self.board.user = lambda: 'foreign'
        n = self.restricted(['alice'])
        with contextlib.redirect_stderr(io.StringIO()):
            self.run_cli('tick')
        self.assertEqual(self.task(n)['state'], 'ready')  # unfiltered manager

    def test_assignee_only_identity_errors(self):
        # #576: an identity error refuses the task (take fails); the pass goes on with the other tasks
        n = self.restricted(['alice'])
        self.fake.names = []
        for error in (RuntimeError('offline'), SystemExit('auth failed')):
            self.board.user = mock.Mock(side_effect=error)
            with self.subTest(error=error), self.assertRaisesRegex(SystemExit, 'assignee-only.*identity'):
                self.run_cli('take', str(n))
            errors = io.StringIO()
            with self.subTest(error=error), contextlib.redirect_stderr(errors):
                self.run_cli('tick')
            self.assertRegex(errors.getvalue(), 'assignee-only.*identity unavailable')
            self.assertEqual(self.task(n)['state'], 'ready')
            self.assertEqual(self.fake.names, [])
        del self.board.user  # a board file without user()
        with self.assertRaisesRegex(SystemExit, 'assignee-only.*identity'):
            self.run_cli('take', str(n))

    def test_assignee_only_label_added_after_the_list(self):
        # #576 race 1: the list lacks `assignee-only`, the fresh read has it: an unsupervised worker is neither requeued nor nudged
        self.board.user = lambda: 'alice'
        n = self.restricted(['bob'])
        self.board.issues[n]['labels'].remove('assignee-only')
        self.legacy(n)
        active = self.task(n)['claim']
        self.fake.sessions[active['session']] = False  # gone: unrestricted, the pass would requeue it
        stale = self.board.list
        self.board.issues[n]['labels'].append('assignee-only')
        listed = lambda state: [{**issue, 'labels': [label for label in issue['labels'] if label != 'assignee-only']} for issue in stale(state)]
        errors = io.StringIO()
        with mock.patch.object(self.board, 'list', side_effect=listed), contextlib.redirect_stderr(errors):
            self.run_cli('tick')
        self.assertIn('not assigned', errors.getvalue())
        self.assertEqual((self.task(n)['state'], self.task(n)['claim']), ('doing', active))
        self.assertEqual((self.fake.sent, self.notes(n)[-1]), ([], '**take**'))

    def test_assignee_only_second_read_reassignment(self):
        # #576 race 2: eligible at supervise's first read, reassigned before its second: no supervisor send, resume or respawn
        for supervisor in ('dead', 'idle'):
            with self.subTest(supervisor=supervisor):
                self.board.issues.clear()
                self.fake.names, self.fake.sent = [], []
                self.board.user = lambda: 'alice'
                n = self.restricted(['alice'])
                self.run_cli('tick')
                boss = self.task(n)['supervisor']
                self.board.comment(n, '**ask** · claude:01234567\n\nworker question')  # an event for an idle supervisor
                self.fake.sessions[boss['session']] = {'dead': False, 'idle': 'idle'}[supervisor]
                real = taskq.lead_state
                def reassign(lead, session):
                    self.board.issues[n]['assignees'] = ['bob']
                    return real(lead, session)
                with mock.patch.object(taskq, 'lead_state', side_effect=reassign), contextlib.redirect_stderr(io.StringIO()), \
                        mock.patch.object(self.board, 'metadata', create=True,
                                          side_effect=lambda n: {k: v for k, v in self.board.get(n).items() if k != 'comments'}) as metadata, \
                        mock.patch.object(self.board, 'comments', create=True, side_effect=AssertionError('ineligible history read')):
                    self.run_cli('tick')
                self.assertGreaterEqual(metadata.call_count, 3)  # accounting plus both admission reads
                self.assertEqual((self.fake.names, self.fake.sent), ([f'S{n} UNK restricted (mac)'], []))
                self.assertEqual((self.task(n)['supervisor'], self.task(n)['state']), (boss, 'doing'))
                self.assertNotIn('**gone**', self.notes(n))

    def test_assignee_only_fresh_read_and_active_continuation(self):
        self.board.user = lambda: 'alice'
        n = self.restricted(['alice'])
        self.fake.names = []
        original = self.board.get
        def reassigned(n):
            return {**original(n), 'assignees': ['bob']}
        with mock.patch.object(self.board, 'get', side_effect=reassigned), contextlib.redirect_stderr(io.StringIO()):
            self.run_cli('tick')
        self.assertEqual(self.fake.names, [])
        self.run_cli('tick')
        boss, claim = self.task(n)['supervisor'], self.task(n)['claim']
        self.board.issues[n]['assignees'] = ['bob']
        with self.acting(boss['session']), contextlib.redirect_stderr(io.StringIO()):
            self.run_cli('run', str(n))
        self.assertEqual((self.task(n)['supervisor'], self.task(n)['claim']), (boss, claim))
        self.assertEqual(self.task(n)['raw']['order'], 'run')
        self.assertEqual(len(self.fake.names), 1)
        self.board.issues[n]['assignees'] = ['alice', 'bob']
        self.run_cli('tick')
        self.run_cli('tick')
        self.assertEqual(len(self.fake.names), 2)  # one supervisor, one worker
        active = self.task(n)['claim']
        self.board.user = lambda: 'bob'
        with self.assertRaisesRegex(SystemExit, 'doing'):
            self.run_cli('take', str(n))
        self.board.issues[n]['assignees'] = []
        self.fake.sessions[active['session']] = False
        with contextlib.redirect_stderr(io.StringIO()):
            self.run_cli('tick')
        self.assertEqual(self.task(n)['claim'], active)  # never steals/requeues active work

    def test_assignee_only_legacy_and_no_label(self):
        self.board.user = mock.Mock(side_effect=RuntimeError('identity unavailable'))
        n = self.restricted(['alice'])
        self.board.issues[n]['labels'].remove('assignee-only')
        self.run_cli('take', str(n))
        self.board.user.assert_not_called()  # unlabeled manual take unchanged
        self.board.issues.clear()
        n = self.restricted(['alice'])
        self.board.issues[n]['labels'].remove('assignee-only')
        self.run_cli('tick')
        self.board.user.assert_not_called()  # unlabeled dispatch unchanged
        with self.assertRaisesRegex(SystemExit, 'not doing|not ready|doing'):
            self.run_cli('take', str(n))
        self.board.issues.clear()
        n = self.restricted(['alice'])
        self.legacy(n)
        active = self.task(n)['claim']
        self.board.user = lambda: 'bob'
        self.fake.sessions[active['session']] = False
        with contextlib.redirect_stderr(io.StringIO()):
            self.run_cli('tick')
        self.assertEqual(self.task(n)['claim'], active)
        self.assertEqual(self.task(n)['state'], 'doing')

    def test_assignee_only_adoption(self):
        self.board.user = lambda: 'bob'
        n = self.restricted(['alice'], pm=None)
        with self.assertRaisesRegex(SystemExit, 'assignee-only'):
            taskq.adopt([n], taskq.session())
        self.assertIsNone(self.task(n)['pm'])
        self.legacy(n)
        claim = self.task(n)['claim']
        taskq.adopt([n], taskq.session())  # migration of active work does not rebind sessions
        self.assertEqual(self.task(n)['claim'], claim)

    def test_assignee_only_take_under_the_dispatch_lock(self):
        # #576 review 1: a dispatcher that claims the task during take's identity call keeps its supervisor and slot
        self.board.user = lambda: 'alice'
        n = self.restricted(['alice'])
        with taskq.coordination(), self.assertRaisesRegex(SystemExit, 'project guard'):
            self.run_cli('take', str(n))  # a pass of this checkout runs: take waits for none, writes nothing
        self.assertEqual((self.task(n)['state'], self.notes(n)), ('ready', []))
        reserved = {'supervisor': {'runtime': 'fake', 'session': 's-S1', 'name': 'mac'}, 'claim': {'runtime': 'fake', 'session': None, 'name': 'mac'}}
        calls = []
        def user():  # the first identity call is slow: a dispatcher claims the task meanwhile
            calls.append(1)
            if len(calls) == 1:
                with contextlib.redirect_stdout(io.StringIO()):
                    taskq.move(self.task(n), 'doing', 'spawn', 'supervisor s-S1', **reserved)
            return 'alice'
        self.board.user = user
        with self.assertRaisesRegex(SystemExit, 'doing|supervisor'):
            self.run_cli('take', str(n))
        self.assertEqual((self.task(n)['supervisor'], self.task(n)['claim']), (reserved['supervisor'], reserved['claim']))

    def test_assignee_only_latest_snapshot_before_send_and_spawn(self):
        # #576 review 2: follow's and replace's own reads are checked: a reassignment there stops the send, retire and spawn
        original = self.board.get
        def reassigned_after(n, reads, transient):  # bob from right after the first `reads` reads; transient: for one read only
            seen = []
            def get(m):
                issue = original(m)
                seen.append(m)
                if m == n and seen.count(n) in (reads, reads + transient):
                    self.board.issues[n]['assignees'] = ['bob'] if seen.count(n) == reads else ['alice']
                return issue
            return get
        for case in ('unsupervised answer', 'supervised answer', 'dispatch', 'worker order', 'transient denial at follow'):
            with self.subTest(case=case):
                self.board.issues.clear()
                self.fake.names, self.fake.sent, self.fake.sessions = [], [], {}
                self.board.user = lambda: 'alice'
                n = self.restricted(['alice'])
                if case == 'unsupervised answer':
                    self.legacy(n)
                if case in ('supervised answer', 'worker order', 'transient denial at follow'):
                    self.run_cli('tick')
                    self.fake.sessions['s-S%d' % n] = True  # running: no supervisor send of its own
                if case in ('supervised answer', 'transient denial at follow'):
                    with self.acting('s-S%d' % n):
                        self.run_cli('run', str(n))
                if case.endswith('answer') or case.startswith('transient'):
                    self.board.comment(n, '**answer** · owner\n\ngo on')
                if case.startswith('transient'):  # #576 review 3: bob only at follow's read; the idle supervisor's answer event must wait too
                    self.fake.sessions['s-S%d' % n] = 'idle'
                before, names = self.task(n), list(self.fake.names)
                if case == 'worker order':
                    with contextlib.redirect_stdout(io.StringIO()):
                        taskq.move(before, 'doing', 'run', 'run', order='run')
                    before = self.task(n)
                with mock.patch.object(self.board, 'get', side_effect=reassigned_after(n, 1, case.startswith('transient'))), contextlib.redirect_stderr(io.StringIO()):
                    self.run_cli('tick')
                after = self.task(n)
                self.assertEqual((self.fake.sent, self.fake.names, self.fake.stopped), ([], names, []))
                self.assertEqual((after['state'], after['claim'], after['supervisor'], after['raw'].get('order')),
                                 (before['state'], before['claim'], before['supervisor'], before['raw'].get('order')))

    def test_assignee_only_report_stays_one_snapshot(self):
        # #576 with #574: read-only `status` never asks the identity nor reads a task; the tick's denied tasks keep their rows
        self.board.user = mock.Mock(side_effect=RuntimeError('identity unavailable'))
        ready, active = self.restricted(['alice']), self.restricted(['alice'])
        self.fake.names = []
        self.legacy(active)
        before = {n: dict(issue, comments=list(issue['comments'])) for n, issue in self.board.issues.items()}
        with mock.patch.object(self.board, 'get', side_effect=AssertionError('status read a task')):
            out = self.run_cli('status')
        self.board.user.assert_not_called()
        self.assertEqual((self.board.issues, self.fake.names, self.fake.sent), (before, [], []))
        with contextlib.redirect_stderr(io.StringIO()) as errors:
            ticked = self.run_cli('tick')
        self.assertEqual(errors.getvalue().count('identity unavailable'), 2)  # both refused, the pass went on
        self.assertEqual((self.task(ready)['state'], self.task(active)['claim']['session'], self.fake.names), ('ready', f's-T{active}', []))
        for text in (out, ticked):
            self.assertIn('In work 1 · Waiting for answer 0 · Ready 1', text)
            self.assertIn(f'| [#{ready} restricted](https://board/{ready}) | ready |', text)

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
        self.assertEqual(self.wait_and_ack( '--window', '0'), 'review #3\n')
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
        # R3 (#572): ORCH is the task's pm on the board, never the caller whose event ran the pass; no pm: the caller
        self.manager('codex')
        with mock.patch.object(taskq, 'runtimes', return_value={'fake': self.fake, 'codex': self.fake}):
            self.add()
            with self.acting('s-S1'):  # a Claude caller
                self.run_cli('run', '1')
        self.assertEqual(self.fake.names, ['S1 CDX T (mac)', 'T1 CDX T (mac)'])
        self.assertEqual(self.fake.prompts['s-S1'].split('\n')[0], 'S1 CDX T (mac)')  # the brief's first line: a fallback title
        self.assertEqual(taskq.worker_name({'iid': 2, 'title': 'x', 'pm': None}), 'T2 CLD x (mac)')

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
        self.fake.sessions[self.task(1)['claim']['session']] = 'idle'
        self.run_cli('answer', '1', '--text', 'this')  # the live worker gets the answer, the slot stays full
        self.assertEqual(spawns(), ['S1', 'T1', 'T1', 'S1', 'T1'])
        self.assertEqual(self.fake.sent, [('s-T1.2', 'The owner answered your question:\n\nthis')])
        with self.acting('s-T1.2'):
            self.run_cli('result', '1', '--sha', 'b' * 40)
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)), self.acting('s-S1.1'):
            self.run_cli('close', '1', '--text', 'checked; works; open: none')  # frees the slot: #2 starts in the same pass
        self.assertEqual(spawns(), ['S1', 'T1', 'T1', 'S1', 'T1', 'S2'])
        self.run_cli('tick')  # the safety net finds nothing left to do
        self.assertEqual(spawns(), ['S1', 'T1', 'T1', 'S1', 'T1', 'S2'])

    def test_event_during_a_pass_is_not_lost(self):
        self.add()
        held = self.board.acquire('other process')
        with mock.patch.object(taskq, 'GUARD_WAIT', 1), \
                mock.patch.object(taskq.time, 'sleep', side_effect=lambda _: self.board.release(held)) as waiting:
            self.run_cli('tick')
        waiting.assert_called_once()
        self.assertIsNone(self.board.guard)
        self.assertFalse((self.root / '.taskq' / 'dispatch.pending').exists())


    def test_arm_tick_without_target_arms_this_session(self):
        out = self.run_cli('arm', 'tick')
        self.assertIn('background command', out)
        self.assertIn('wait', out)
        # #525: one start, then the queue runs by itself; no sender is required
        self.assertIn('Start: run one pass now', out)
        self.assertIn('no sender, timer or extension', out)
        self.assertNotIn('separate sender', out)
        for part in ('Explicit owner arm: execute the proven route', 'Reuse the existing monitor and targeted wait',
                     'repeated arm must not create duplicates', 'Prove an idle-manager wake and the next wait',
                     'never create a replacement sender or bridge'):
            self.assertIn(part, out)

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
            self.assertNotIn('gone', self.wait_and_ack( '--window', '0'))
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
        with mock.patch.object(taskq.subprocess, 'run', side_effect=lambda command, **_: calls.append(command[1:]) or subprocess.CompletedProcess(command, 0)):
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
                (Path(folder) / '.taskq' / f'T{n}.pid').write_text(f'999999999 thread-{n} {BIRTH}')
            calls = []
            with mock.patch.object(taskq.subprocess, 'run', side_effect=lambda command, **_: calls.append(command[1:]) or subprocess.CompletedProcess(command, 0)):
                taskq.Codex().retire(lambda n, *_: n == 3)
            self.assertEqual(calls, [['archive', 'thread-3']])
            self.assertEqual([path.name for path in (Path(folder) / '.taskq').iterdir()], ['T4.pid'])

    def test_codex_alive_from_pid_file(self):
        codex = taskq.Codex()
        (codex.folder() / 'T1.pid').write_text(f'{os.getpid()} thread-1 {taskq.process_identity(os.getpid())[1]}')
        (codex.folder() / 'T2.pid').write_text(f'999999999 thread-2 {BIRTH}')  # above any pid_max: no such process
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
        with mock.patch.object(codex, 'exec', run), mock.patch.object(codex, 'title') as title:
            self.assertEqual(codex.spawn('T1 one (mac)', 'prompt', '.'), 'new-thread')
        self.assertEqual((codex.folder() / 'T1.pid').read_text(), f'{os.getpid()} new-thread {taskq.process_identity(os.getpid())[1]}')
        title.assert_called_once_with('new-thread', 'T1 one (mac)')  # R3 (#572): the new thread gets its native name

    def test_codex_title_waits_for_each_reply(self):
        # R3 (#572): a real local fake `codex app-server`: each request goes only after its own successful reply, the
        # read must return the thread and the name; an error, a wrong read or a hang raises, all within one deadline
        folder = Path(self.root) / 'bin'
        folder.mkdir()
        server = folder / 'codex'
        server.write_text(f'#!{sys.executable}\n' + APP_SERVER)
        server.chmod(0o755)
        # Windows does not execute a shebang: use the real Python process for this same fake stdio server.
        def launch(command, **kwargs):
            return REAL_POPEN(([sys.executable, *command] if os.name == 'nt' else command), **kwargs)
        cases = {'ok': None, 'set-error': 'thread/name/set: ', 'wrong-name': "thread/read: th named 'other'",
                 'silent': 'initialize: no reply', 'hang': None}
        for mode, error in cases.items():
            log = folder / f'{mode}.log'
            with self.subTest(mode), mock.patch.object(taskq.subprocess, 'Popen', launch), \
                    mock.patch.object(taskq.shutil, 'which', return_value=str(server)), mock.patch.object(taskq.Codex, 'WAIT', 2), \
                    mock.patch.dict(os.environ, {'FAKE_MODE': mode, 'FAKE_LOG': str(log)}):
                began = taskq.time.monotonic()
                if error:
                    with self.assertRaisesRegex(ValueError, re.escape(error)):
                        taskq.Codex().title('th', 'S1 CDX one (mac)')
                else:
                    taskq.Codex().title('th', 'S1 CDX one (mac)')
                self.assertLess(taskq.time.monotonic() - began, 10)  # the deadline holds through shutdown
            methods = log.read_text().split()
            self.assertEqual(methods, {'set-error': ['initialize', 'initialized', 'thread/name/set'], 'silent': ['initialize']}.get(
                mode, ['initialize', 'initialized', 'thread/name/set', 'thread/read']))

    def test_codex_unnamed_spawn_stops_and_keeps_its_handle(self):
        # R3 (#572): a thread whose name is not confirmed never counts as spawned: its turn is stopped, its pid file kept
        codex = taskq.Codex()
        log, process = codex.folder() / 'S1.log', mock.Mock(pid=4242)

        def run(*_):
            log.write_text('{"type":"thread.started","thread_id":"th"}\n')
            return process, log
        with mock.patch.object(codex, 'exec', run), \
                mock.patch.object(codex, 'title', side_effect=ValueError('thread/name/set: no rollout found')), \
                self.assertRaisesRegex(taskq.Unnamed, '^th not named: thread/name/set: no rollout found$'):
            codex.spawn('S1 CDX one (mac)', 'prompt', '.')
        process.terminate.assert_called_once_with()
        self.assertEqual((codex.folder() / 'S1.pid').read_text(), '4242 th -')

    def test_unnamed_spawn_is_recorded_gone_not_spawned(self):
        # R3/R11 (#572): the pass fails, the board records the id in a gone note only; the next pass retires it
        def unnamed(name, prompt, cwd):
            sid = spawn(name, prompt, cwd)
            self.fake.sessions[sid] = False  # stopped
            raise taskq.Unnamed(sid, ValueError(f'thread/read: {sid} named None'))
        spawn = self.fake.spawn
        self.fake.spawn = unnamed
        self.add('one')
        self.assertIsNotNone(self.board.guard)
        with self.assertRaisesRegex(SystemExit, 'project guard busy'):
            self.run_cli('tick')
        self.board.release(self.board.guard)  # explicit quiescent fixture recovery: failed session is confirmed stopped
        with self.assertRaisesRegex(SystemExit, r'#1: supervisor s-S1\.1 not named'):
            self.run_cli('tick')
        self.assertEqual((self.task(1)['state'], self.task(1)['supervisor'], set(self.notes(1)[1:])), ('ready', None, {'**gone**'}))  # the add's pass too
        self.assertTrue(self.board.issues[1]['comments'][-1].endswith('\n\nsupervisor s-S1.1 not named: thread/read: s-S1.1 named None'))
        self.fake.spawn = spawn
        self.board.release(self.board.guard)  # both failed sessions are stopped; retain the original retirement assertion
        self.run_cli('tick')
        self.assertEqual((self.fake.stopped, self.task(1)['supervisor']['session']), (['s-S1', 's-S1.1'], 's-S1.2'))

    def test_codex_resume_keeps_the_named_thread(self):
        # R3 (#572): `exec resume` goes to the same thread under its handle; it renames nothing (the name stays native)
        codex = taskq.Codex()
        (codex.folder() / 'S1.pid').write_text(f'999999999 th {BIRTH}')
        with mock.patch.object(codex, 'exec', return_value=(mock.Mock(pid=4242), codex.folder() / 'S1.log')) as run, \
                mock.patch.object(codex, 'title', side_effect=AssertionError('resume renames nothing')):
            self.assertEqual(codex.send('th', 'review #1: read your issue'), 'th')
        run.assert_called_once_with('S1', ['resume', 'th', 'review #1: read your issue'], self.root)
        self.assertEqual((codex.folder() / 'S1.pid').read_text(), '4242 th -')

    def test_supervisor_runtime_follows_the_manager(self):
        # R3 (#525): S<N> runs in the manager's runtime (DOT: Codex), T<N> in the task's; no manager here: the task waits
        lead = FakeRuntime()
        self.manager('dot')
        with mock.patch.object(taskq, 'runtimes', return_value={'fake': self.fake, 'codex': lead}):
            self.add('one', '--runtime', 'fake')
            with self.acting('s-S1'):
                self.run_cli('run', '1')
            self.assertEqual((lead.names, self.fake.names), (['S1 DOT one (mac)'], ['T1 DOT one (mac)']))  # ORCH: the pm, not the Claude caller
            self.assertEqual((self.task(1)['supervisor']['runtime'], self.task(1)['claim']), ('codex', {'runtime': 'fake', 'session': 's-T1', 'name': 'mac'}))
            self.manager('hermes')  # a manager whose runtime has no file here
            taskq.CONFIG['limits'] = {'fake': 2}
            self.add('two')
            self.assertEqual((self.task(2)['state'], len(lead.names)), ('ready', 1))
            with self.assertRaisesRegex(SystemExit, 'Hermes native supervisor bridge is not configured'):
                self.run_cli('tick')
            self.assertIn('| [#2 two](https://board/2) | blocked (no manager) | any |  |', self.run_cli('status'))

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
        self.assertEqual(self.fake.names, ['S1 UNK one (mac)', 'T1 UNK one (mac)'])
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
        self.assertEqual(self.fake.sent, [('s-S1', 'requeue #1 [event 1:5]: read your issue')])
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
        self.assertIn(('s-S1', 'answer #1 [event 1:11]: read your issue'), self.fake.sent)
        self.assertEqual([text for _, text in self.fake.sent].count('requeue #1 [event 1:5]: read your issue'), 1)  # its own requeues woke nothing

    def test_replacement_retires_the_recorded_old_session_first(self):
        # #568: a rework's T<N> and a respawned S<N> retire their predecessor by board-recorded id before they spawn
        # (a Codex spawn rewrites the T<N>/S<N> handle); a stranger's or the live worker's session is never touched
        spawn, seen = self.fake.spawn, []
        self.fake.spawn = lambda name, *rest: (seen.append((name.split()[0], list(self.fake.stopped))), spawn(name, *rest))[1]
        self.add('one')
        self.fake.sessions['s-T7'] = True  # another task's, unrecorded here
        with self.acting('s-S1'):
            self.run_cli('run', '1')
        with self.acting('s-T1'):
            self.run_cli('result', '1', '--sha', 'a' * 40)
        with self.acting('s-S1'):
            self.run_cli('requeue', '1', '--text', 'fix B')  # s-T1 still runs: replaced, retired first
        self.fake.sessions['s-S1'] = False
        self.run_cli('tick')
        self.assertEqual(seen, [('S1', []), ('T1', []), ('T1', ['s-T1']), ('S1', ['s-T1', 's-S1'])])
        self.assertEqual((self.task(1)['claim']['session'], self.fake.sessions.get('s-T1.1'), self.fake.sessions.get('s-T7')), ('s-T1.1', True, True))

    def test_unknown_recorded_predecessor_defers_rework_but_unrelated_handle_does_not(self):
        self.add('one')
        claim = {'runtime': 'codex', 'session': 'old-worker', 'name': 'mac'}
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(self.task(1), 'doing', 'spawn', 'worker old-worker', claim=claim)
            taskq.move(self.task(1), 'doing', 'requeue', 'fix', claim={**claim, 'session': None}, order='rework')
        codex = taskq.Codex()
        owned, unrelated = codex.folder() / 'T1.pid', codex.folder() / 'T7.pid'
        owned.write_text('123 old-worker')
        unrelated.write_text('456 other-worker')
        with mock.patch.object(taskq, 'runtimes', return_value={'fake': self.fake, 'codex': codex}), \
                mock.patch.object(codex, 'spawn', return_value='new-worker') as spawn, \
                mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as archive:
            self.run_cli('tick')
            self.assertIsNone(self.task(1)['claim']['session'])
            self.assertEqual(self.task(1)['raw']['order'], 'rework')
            self.assertEqual(owned.read_text(), '123 old-worker')
            spawn.assert_not_called()
            archive.assert_not_called()
            self.assertIsNone(self.board.guard)  # known deferral has no uncertain side effect
            # A reconciled, exited predecessor permits admission; the unrelated unknown handle stays untouched.
            owned.write_text(f'999999999 old-worker {BIRTH}')
            archive.return_value = subprocess.CompletedProcess([], 1)
            with self.assertRaisesRegex(RuntimeError, 'archive failed'), contextlib.redirect_stderr(io.StringIO()):
                self.run_cli('tick')
            spawn.assert_not_called()
            self.assertIsNotNone(self.board.guard)
            self.assertTrue(owned.exists())
            self.board.release(self.board.guard)  # fixture-only reconciliation: fake archive has no external effects
            archive.return_value = subprocess.CompletedProcess([], 0)
            self.run_cli('tick')
            self.assertEqual(self.task(1)['claim']['session'], 'new-worker')
            self.assertIsNone(self.task(1)['raw']['order'])
            spawn.assert_called_once()
            self.assertEqual(archive.call_args.args[0][-2:], ['archive', 'old-worker'])
            self.assertEqual(unrelated.read_text(), '456 other-worker')

    def test_codex_respawn_keeps_the_replaced_handle(self):
        # #568: a running replaced thread keeps a handle (S<N>-<thread>.pid) so retire still finds and archives it
        codex = taskq.Codex()
        (codex.folder() / 'S1.pid').write_text(f'999999999 old-thread {BIRTH}')
        log = codex.folder() / 'S1.log'

        def run(*_):
            log.write_text('{"thread_id":"new-thread"}\n')
            return mock.Mock(pid=999999998), log
        with mock.patch.object(codex, 'exec', run), mock.patch.object(codex, 'title'):
            codex.spawn('S1 one (mac)', 'prompt', '.')
        calls = []
        with mock.patch.object(taskq.subprocess, 'run', side_effect=lambda command, **_: calls.append(command[1:]) or subprocess.CompletedProcess(command, 0)):
            codex.retire(lambda n, sid, live: sid == 'old-thread')
        self.assertEqual((calls, (codex.folder() / 'S1.pid').read_text()), ([['archive', 'old-thread']], '999999998 new-thread -'))

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
            self.assertEqual(self.wait_and_ack( '--task', '1', '--window', '0'), 'review #1\n')
            self.assertEqual(self.wait_and_ack( '--task', '1', '--window', '0'), 'tick\n')  # once
        with self.acting('s-S2'):
            self.assertEqual(self.wait_and_ack( '--task', '1', '--window', '0'), 'stop #1\n')  # not its task
        with self.acting('s-S1'):
            self.run_cli('requeue', '1', '--text', 'fix X')
        with self.acting('s-T1.1'):
            self.run_cli('result', '1', '--sha', 'b' * 40)
        self.fake.sessions['s-S1'] = 'idle'  # its turn ended, the process is gone
        self.fake.send = lambda session, text: self.fake.sent.append((session, text)) or 's-S1r'
        self.run_cli('tick')
        self.assertEqual(self.fake.sent, [('s-S1', 'review #1 [event 1:8]: read your issue')])
        self.assertEqual((self.task(1)['supervisor']['session'], self.board.issues[1]['comments'][-1]),
                         ('s-S1r', '**nudge** · claude:01234567\n\nsupervisor s-S1r replaces s-S1'))
        with self.acting('s-S1'), self.assertRaisesRegex(SystemExit, 'only it, the task.s manager or the owner'):
            self.run_cli('close', '1')
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)), self.acting('s-S1r'):
            self.assertEqual(self.wait_and_ack( '--task', '1', '--window', '0'), 'tick\n')  # already delivered
            self.run_cli('close', '1', '--text', 'ok; works; open: none')
            self.assertEqual(self.wait_and_ack( '--task', '1', '--window', '0'), 'stop #1\n')
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
            (codex.folder() / f'{name}.pid').write_text(f'{pid} th-{name[1]} {taskq.process_identity(pid)[1] or BIRTH}')
            (codex.folder() / f'{name}.log').write_text(log)
        with mock.patch.dict(os.environ, {'CODEX_HOME': str(home)}):
            self.assertEqual([codex.state(f'th-{n}') for n in (1, 2, 3, 4)], ['running', 'dead', 'dead', None])  # th-2: no rollout; th-4: another checkout may own it
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
        with mock.patch.object(taskq, 'process_identity', side_effect=[('running', BIRTH), ('dead', None)]), mock.patch.object(taskq.time, 'sleep') as slept:
            self.run_cli('tick', '--quiet', '--after', '4242', '--after-birth', BIRTH)
        self.assertEqual((slept.call_count, self.fake.sent), (1, [('s-S1', 'review #1 [event 1:5]: read your issue')]))
        self.run_cli('tick')
        self.assertEqual(len(self.fake.sent), 1)  # once

    def test_codex_turn_starts_a_pass_at_its_end(self):
        # #525: every `codex exec` turn (spawn or resume) gets one detached pass that waits for its pid
        started = []
        with mock.patch.object(taskq.subprocess, 'Popen', return_value=mock.Mock(pid=4242)), \
                mock.patch.object(taskq.shutil, 'which', return_value='codex'), \
                mock.patch.object(taskq, 'start_pass', lambda command, **_: started.append(command[2:])):
            taskq.Codex().exec('S1 CDX one (mac)', ['resume', 'th', 'review #1: read your issue'], self.root)
        self.assertEqual(started, [['tick', '--quiet', '--after', '4242', '--after-birth', '-', '--tasks']])
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


class RuntimeProcessBoundary(Base):
    """Local processes only. No installed model CLI, board network or paid calls."""

    def test_runtime_capability_fallback_cannot_invent_idle(self):
        legacy = mock.Mock(spec=['alive'])
        for value, expected in ((True, 'running'), (False, 'dead'), (None, 'unknown')):
            legacy.alive.return_value = value
            self.assertEqual(taskq.runtime_state(legacy, 'sid'), expected)
        native = mock.Mock(spec=['state'])
        for value in ('running', 'idle', 'dead', 'unknown', None, 'unexpected'):
            native.state.return_value = value
            self.assertEqual(taskq.runtime_state(native, 'sid'), value if value in ('running', 'idle', 'dead') else 'unknown')

    def test_darwin_native_birth_layout_errors_and_retirement_capability(self):
        import ctypes
        import errno
        import struct
        # Apple xnu proc_bsdinfo: 136 bytes, start timeval at offset 120; an independent byte fixture.
        library = mock.Mock()
        def reply(pid, flavor, arg, buffer, size):
            self.assertEqual((pid, flavor, arg, size), (123, 3, 0, 136))
            data = struct.pack('=12I16s32s5IiQQ', 0, status, 0, returned_pid, *([0] * 8),
                               b'mock', b'mock', *([0] * 6), 1700000000, 123456)
            ctypes.memmove(buffer, data, len(data))
            ctypes.set_errno(error)
            return length
        library.proc_pidinfo.side_effect = reply
        with mock.patch.object(ctypes, 'CDLL', return_value=library):
            for status, returned_pid, length, error, expected in (
                    (2, 123, 136, 0, ('running', f'{taskq.process_domain("darwin")}:1700000000:123456')),
                    (5, 123, 136, 0, ('dead', f'{taskq.process_domain("darwin")}:1700000000:123456')),
                    (2, 123, 0, errno.ESRCH, ('dead', None)),
                    (2, 123, 0, errno.EPERM, ('unknown', None)),
                    (2, 123, 128, 0, ('unknown', None)),
                    (2, 124, 136, 0, ('unknown', None))):
                with self.subTest(status=status, length=length, error=error, pid=returned_pid):
                    self.assertEqual(taskq.darwin_identity(123), expected)
        with mock.patch.object(taskq.os, 'name', 'posix'), mock.patch.object(taskq.sys, 'platform', 'darwin'), \
                mock.patch.object(taskq, 'process_identity', return_value=('running', f'{taskq.process_domain("darwin")}:1:2')), \
                mock.patch.object(taskq.os, 'kill') as kill, \
                self.assertRaisesRegex(RuntimeError, 'safe process termination unavailable'):
            taskq.stop_process(123, f'{taskq.process_domain("darwin")}:1:2')
        kill.assert_not_called()

    def test_legacy_and_unknown_handles_are_preserved(self):
        codex = taskq.Codex()
        path = codex.folder() / 'T1.pid'
        for text in ('123 sid', '123 sid -', '123 sid malformed', '123 sid windows:1', f'123 sid {BIRTH}'):
            with self.subTest(handle=text), mock.patch.object(taskq, 'process_identity', return_value=('unknown', None)), \
                    mock.patch.object(taskq, 'stop_process') as stop, mock.patch.object(codex, 'exec') as execute:
                path.write_text(text)
                self.assertEqual(codex.state('sid'), 'unknown')
                with self.assertRaisesRegex(RuntimeError, 'unknown.*refused'):
                    codex.send('sid', 'continue')
                codex.retire(lambda *args: True)
                self.assertEqual(path.read_text(), text)
                execute.assert_not_called()
                stop.assert_not_called()

    def test_foreign_process_domains_are_unknown_even_when_local_pid_is_absent(self):
        win = 'windows:' + 'a' * 64
        mac = 'darwin:' + 'a' * 64
        linux = 'linux:aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'
        codex = taskq.Codex()
        path = codex.folder() / 'T1.pid'
        cases = ((linux, win + ':1'), (win, linux + ':1'),
                 (linux, 'linux:bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb:1'),
                 (win, 'windows:' + 'b' * 64 + ':1'),
                 (mac, 'darwin:' + 'b' * 64 + ':1:0'),
                 (win, mac + ':1:0'), (mac, win + ':1'),
                 (win, 'windows:1'), (mac, 'darwin:1:0'))
        for local, recorded in cases:
            with self.subTest(local=local, recorded=recorded), \
                    mock.patch.object(taskq, 'process_domain', return_value=local), \
                    mock.patch.object(taskq, 'process_identity', return_value=('dead', None)) as probe, \
                    mock.patch.object(codex, 'exec') as execute, \
                    mock.patch.object(taskq.subprocess, 'run') as archive:
                original = f'123 sid {recorded}'
                path.write_text(original)
                self.assertEqual(codex.state('sid'), 'unknown')
                with self.assertRaisesRegex(RuntimeError, 'unknown.*refused'):
                    codex.send('sid', 'continue')
                codex.retire(lambda *args: True)
                self.assertEqual(path.read_text(), original)
                probe.assert_not_called()
                execute.assert_not_called()
                archive.assert_not_called()

    def test_running_retirement_without_safe_signal_defers(self):
        codex = taskq.Codex()
        path = codex.folder() / 'T1.pid'
        path.write_text(f'123 sid {BIRTH}')
        # Pre-created paths avoid changing pathlib's host selection while exercising Darwin's capability branch.
        root = mock.MagicMock()
        root.__truediv__.return_value.glob.return_value = [path]
        with mock.patch.dict(taskq.CONFIG, root=root), mock.patch.object(taskq.os, 'name', 'posix'), \
                mock.patch.object(taskq.sys, 'platform', 'darwin'), \
                mock.patch.object(taskq, 'read_process', return_value=(123, 'sid', BIRTH)), \
                mock.patch.object(taskq, 'process_state', return_value='running'), \
                mock.patch.object(taskq, 'stop_process') as stop:
            self.assertIs(codex.retire(lambda *args: True), False)
            self.assertIs(codex.retire(lambda *args: False), True)
            stop.assert_not_called()
        self.assertEqual(path.read_text(), f'123 sid {BIRTH}')

    def test_windows_and_darwin_domains_derive_from_native_hostname(self):
        for platform in ('windows', 'darwin'):
            with mock.patch.object(taskq.socket, 'gethostname', return_value='host-a'):
                first = taskq.process_domain(platform)
            with mock.patch.object(taskq.socket, 'gethostname', return_value='host-b'):
                second = taskq.process_domain(platform)
            self.assertNotEqual(first, second)
            self.assertEqual(len(first.split(':')[1]), 64)
            self.assertTrue(first.startswith(platform + ':'))
        with mock.patch.object(taskq, 'process_identity', return_value=('running', None)):
            self.assertEqual(taskq.process_state(123, BIRTH), 'unknown')

    def test_reused_pid_never_signals_foreign_process_or_waits_for_it(self):
        codex = taskq.Codex()
        path = codex.folder() / 'T1.pid'
        path.write_text(f'123 sid {BIRTH}')
        with mock.patch.object(taskq, 'process_identity', return_value=('running', NEXT_BIRTH)), \
                mock.patch.object(taskq, 'stop_process') as stop, \
                mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as archive, \
                mock.patch.object(taskq, 'cmd_tick') as tick, mock.patch.object(taskq.time, 'sleep') as sleep:
            codex.retire(lambda *args: True)
            taskq.event_pass(type('Args', (), {'after': 123, 'after_birth': BIRTH})())
            stop.assert_not_called()
            sleep.assert_not_called()
            tick.assert_called_once()
            self.assertEqual(archive.call_args.args[0][-2:], ['archive', 'sid'])
            self.assertFalse(path.exists())

    def test_turn_end_unknown_identity_refuses_dispatch(self):
        for birth in (None, '-', 'windows:1', BIRTH):
            with mock.patch.object(taskq, 'process_identity', return_value=('unknown', None)), \
                    mock.patch.object(taskq, 'cmd_tick') as tick, contextlib.redirect_stderr(io.StringIO()), \
                    self.assertRaises(SystemExit):
                taskq.event_pass(type('Args', (), {'after': 123, 'after_birth': birth})())
            tick.assert_not_called()

    @unittest.skipUnless(os.name == 'nt' or sys.platform in ('linux', 'darwin'), 'supported process birth API required')
    def test_real_mock_codex_turn_busy_refusal_and_identified_exit(self):
        script = self.root / 'fake_codex.py'
        release = self.root / 'release'
        script.write_text("import pathlib, sys, time\nrelease=pathlib.Path(sys.argv[1])\n"
                          "print('{\"type\":\"thread.started\",\"thread_id\":\"local-thread\"}', flush=True)\n"
                          "print('{\"type\":\"turn.started\"}', flush=True)\n"
                          "while not release.exists(): time.sleep(.01)\n"
                          "print('{\"type\":\"turn.completed\"}', flush=True)\n")
        children = []
        def launch(command, **kwargs):
            child = REAL_POPEN([sys.executable, str(script), str(release)], **kwargs)
            children.append(child)
            return child
        codex = taskq.Codex()
        try:
            with mock.patch.object(taskq.subprocess, 'Popen', launch), \
                    mock.patch.object(taskq.shutil, 'which', return_value=sys.executable), \
                    mock.patch.object(taskq, 'dispatch') as dispatch, mock.patch.object(codex, 'title'):
                self.assertEqual(codex.spawn('T1 mock', 'local only', self.root), 'local-thread')
                child = children[0]
                path = codex.pid_file('local-thread')
                original = path.read_text()
                pid, sid, birth = taskq.read_process(path)
                self.assertEqual(taskq.process_state(pid, birth), 'running')
                with self.assertRaisesRegex(RuntimeError, 'running.*refused'):
                    codex.send(sid, 'do not overlap')
                self.assertEqual(len(children), 1)
                self.assertEqual(path.read_text(), original)
                dispatch.assert_called_once_with('turn end', [], after=pid, after_birth=birth)
            def finish(_):
                release.touch()
                child.wait(timeout=5)
            with mock.patch.object(taskq, 'time', mock.Mock(sleep=finish)), mock.patch.object(taskq, 'cmd_tick') as tick:
                taskq.event_pass(type('Args', (), {'after': pid, 'after_birth': birth})())
                tick.assert_called_once()
            self.assertEqual(taskq.process_state(pid, birth), 'dead')
            self.assertIn('turn.completed', path.with_suffix('.log').read_text())
            with mock.patch.object(taskq, 'rollout', return_value='local'):
                self.assertEqual(codex.state(sid), 'idle')
        finally:
            release.touch()
            for child in children:
                if child.poll() is None:
                    child.terminate()
                child.wait(timeout=5)

    @unittest.skipUnless(os.name == 'nt' or (sys.platform == 'linux' and hasattr(os, 'pidfd_open')
                                          and hasattr(taskq.signal, 'pidfd_send_signal')), 'safe signal primitive required')
    def test_real_owned_process_retirement_waits_for_exit(self):
        child = REAL_POPEN([sys.executable, '-c', 'import time; time.sleep(30)'])
        try:
            state, birth = taskq.process_identity(child.pid)
            self.assertEqual(state, 'running')
            self.assertTrue(birth)
            taskq.stop_process(child.pid, birth)
            child.wait(timeout=5)
            self.assertEqual(taskq.process_state(child.pid, birth), 'dead')
        finally:
            if child.poll() is None:
                child.terminate()
            child.wait(timeout=5)


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
        self.assertEqual(self.wait_and_ack(), 'tick\n')  # nothing happened: the safety window
        self.assertEqual(self.clock[0], 600)
        self.run_cli('result', '1', '--sha', 'a' * 40)
        self.add('two')
        self.legacy(2)
        self.fake.sessions['s-T2'] = False
        self.assertEqual(self.wait_and_ack().splitlines(), ['review #1', 'gone #2'])
        self.assertEqual(self.clock[0], 600)  # at once, no sleep
        self.assertEqual(self.wait_and_ack(), 'tick\n')  # never twice for the same event
        self.run_cli('requeue', '1')
        self.legacy(1, 's-T1b')
        start = self.clock[0]
        result = lambda: self.run_cli('result', '1', '--sha', 'b' * 40) if self.clock[0] == start + 50 else None
        with mock.patch.object(taskq.time, 'sleep', lambda s: (self.clock.__setitem__(0, self.clock[0] + s), result())):
            self.assertEqual(self.wait_and_ack(), 'review #1\n')  # back in review while waiting: a new event
        self.assertEqual(self.clock[0], start + 50)

    def test_arm_tick_names_target_and_loop(self):
        out = self.run_cli('arm', 'tick', 'PM main')
        self.assertIn('manager session PM main', out)
        self.assertIn("taskq.py wait --pm 'PM main'`", out)  # #532: a name matches no task's pm, and the prompt says so
        self.assertIn('taskq: no open task records PM main as its pm', out)
        self.assertIn('with SendMessage', out)
        self.assertIn('Reuse the existing monitor and targeted wait', out)
        self.assertIn('Prove an idle-manager wake and the next wait', out)

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
        self.assertIn('resume T1 "$e" && printf', sender)
        self.assertIn('ack --stdin --pm T1; do :; done;', sender)
        self.assertNotIn('send_message_to_thread', sender)
        self.assertIn('in the foreground', self_arm)
        self.assertIn('Optional, only to be woken between turns: `', self_arm)
        self.assertIn('arm tick T1`', self_arm)
        self.assertNotIn('Before you end a turn', self_arm)
        self.assertIn('no sender, timer or extension', self_arm)

    def test_arm_tick_links_use_the_same_pm_for_lookup_wait_and_send(self):
        """#595: a supported link must select the same route as its id, without mutating queue state."""
        for folder, route in (('sessions/2026/10/09', 'local'), ('archived_sessions', 'archived'), ('missing', None)):
            thread = f'PM-{route}'
            links = (thread, f'codex://threads/{thread}',
                     f'https://alexkirs.github.io/taskq/open.html#codex://threads/{thread}')
            with self.subTest(route=route), mock.patch.dict(os.environ, self.codex_home(folder, thread)):
                before = json.dumps(self.board.issues, sort_keys=True)
                for target in links:
                    with self.subTest(target=target):
                        out = self.run_cli('arm', 'tick', target)
                        if route == 'local':
                            self.assertIn(f'resume {thread} "<its output>"', out)
                            self.assertNotIn('send_message_to_thread', out)
                        elif route == 'archived':
                            self.assertIn(f'{thread} is archived in Codex', out)
                            self.assertNotIn('tick sender', out)
                        else:
                            self.assertIn(f'no local Codex rollout of {thread}', out)
                            self.assertIn(f'to {thread} with `send_message_to_thread`', out)
                            self.assertNotIn(f'resume {thread}', out)
                        if route != 'archived':
                            self.assertIn(f'wait --pm {thread}`', out)
                        self.assertEqual(out, self.run_cli('arm', 'tick', target))
                self.assertEqual(json.dumps(self.board.issues, sort_keys=True), before)

    def test_arm_tick_shell_loop_stops_on_failed_wait_or_send(self):
        """#522: the printed shell loop sends each event once and stops on the first failed wait or send."""
        with mock.patch.dict(os.environ, self.codex_home('sessions/2026/10/09', 'T1')):
            sender = self.run_cli('arm', 'tick', 'codex://threads/T1')
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
                run = REAL_RUN(['bash', '-c', loop.replace(wait, fake_wait).replace(f'python3 {Path(taskq.__file__).resolve()} ack --stdin --pm T1', 'cat >/dev/null')], capture_output=True, text=True, timeout=10,
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
        self.assertEqual(self.fake.names, ['S1 UNK one (mac)', 'T1 UNK one (mac)'])  # one spawn each
        self.assertEqual((self.task(1)['state'], self.task(2)['state']), ('review', 'ready'))
        self.assertEqual(self.wait_and_ack(), 'tick\n')  # a supervised review is the supervisor's
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)), self.acting('s-S1'):
            self.run_cli('close', '1', '--text', 'one shipped; users get one; open: none\n\nmore')
        self.assertEqual(self.wait_and_ack(), 'closed #1 one shipped; users get one; open: none\n')
        self.assertEqual(self.wait_and_ack(), 'tick\n')  # never sent twice
        self.assertEqual(self.fake.names[-1], 'S2 UNK two (mac)')  # the close freed the slot
        self.assertEqual((self.fake.stopped, self.fake.sessions['s-S1']), (['s-T1'], True))  # close never stops the session that runs it
        self.fake.sessions['s-S1'] = False  # its turn ended: the next pass retires it
        self.run_cli('tick')
        self.assertEqual(self.fake.stopped, ['s-T1', 's-S1'])


class EventDelivery(Base):
    """Board receipts survive comments, checkout changes and response loss; no live services."""

    def setUp(self):
        super().setUp()
        patcher = mock.patch.object(taskq, 'dispatch')
        patcher.start()
        self.addCleanup(patcher.stop)
        self.add()

    def worker(self):
        claim = {'runtime': 'fake', 'session': 'worker-one', 'name': 'mac'}
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(self.task(1), 'doing', 'take', claim=claim)
        fake = FakeRuntime()
        fake.sessions[claim['session']] = 'idle'
        return fake, claim

    def test_two_answers_survive_ordinary_comments_and_ack_exact_recipient(self):
        fake, claim = self.worker()
        for answer in ('first', 'second'):
            self.run_cli('ask', '1', '--text', 'which?')
            self.run_cli('answer', '1', '--text', answer)
            self.board.comment(1, 'ordinary progress note')
            self.board.comment(1, '**nudge** · owner\n\nsupervisor unrelated-session')
        with taskq.coordination(), contextlib.redirect_stdout(io.StringIO()):
            taskq.follow(self.task(1), fake, claim, False)
            taskq.follow(self.task(1), fake, claim, False)
        self.assertEqual(fake.sent, [('worker-one', 'The owner answered your question:\n\nfirst\n\nsecond')])
        self.assertEqual(taskq.event_pending(self.task(1)['raw'], taskq.recipient('worker', claim)), [])
        self.assertEqual(len(taskq.event_pending(self.task(1)['raw'], taskq.recipient('manager', self.task(1)['pm']))), 2)

    def test_comment_failure_preserves_action_dispatch_and_grant(self):
        self.worker()
        with mock.patch.object(self.board, 'comment', side_effect=SystemExit('comment transport lost')), \
                contextlib.redirect_stderr(io.StringIO()) as err, mock.patch.object(taskq, 'dispatch') as dispatch:
            self.run_cli('ask', '1', '--text', 'choose A')
        self.assertEqual(self.task(1)['state'], 'ask')
        self.assertEqual(self.task(1)['raw']['action']['text'], 'choose A')
        self.assertIsNone(self.board.guard)
        dispatch.assert_called_once_with('ask', 1) if 'ask' in taskq.EVENTS else None
        self.assertIn('diagnostic comment failed', err.getvalue())
        first = self.run_cli('wait', '--window', '0', '--json')
        self.assertEqual(self.run_cli('wait', '--window', '0', '--json'), first)
        self.assertFalse(list((self.root / '.taskq').glob('wait*.json')))

    def test_wait_replays_until_explicit_ack_shared_across_checkouts(self):
        self.worker()
        self.run_cli('ask', '1', '--text', 'question')
        first = json.loads(self.run_cli('wait', '--window', '0', '--json'))
        token = first['events'][0]['id']
        self.assertEqual(json.loads(self.run_cli('wait', '--window', '0', '--json')), first)
        other = self.root / 'other'; other.mkdir()
        with mock.patch.dict(taskq.CONFIG, root=other):
            self.assertEqual(json.loads(self.run_cli('wait', '--window', '0', '--json')), first)
            self.run_cli('ack', token)
        self.run_cli('ack', token)
        self.assertEqual(self.run_cli('wait', '--window', '0'), 'tick\n')

    def test_ack_lost_response_replays_idempotently_but_retains_unknown_guard(self):
        self.worker(); self.run_cli('ask', '1', '--text', 'question')
        token = json.loads(self.run_cli('wait', '--json', '--window', '0'))['events'][0]['id']
        update = self.board.update
        def lost(*args, **kwargs):
            update(*args, **kwargs)
            raise RuntimeError('response lost')
        with mock.patch.object(self.board, 'update', side_effect=lost), self.assertRaisesRegex(RuntimeError, 'response lost'):
            self.run_cli('ack', token)
        self.assertIsNotNone(self.board.guard)
        self.board.guard = None  # fixture's explicit quiescent reconciliation; production never steals a grant
        self.run_cli('ack', token)
        self.assertEqual(self.run_cli('wait', '--window', '0'), 'tick\n')

    def test_bound_never_overwrites_pending_payload_and_future_schema_refused(self):
        self.worker()
        with mock.patch.object(taskq, 'EVENT_LIMIT', 1):
            self.run_cli('ask', '1', '--text', 'preserve me')
            before = self.board.get(1)['body']
            with self.assertRaisesRegex(SystemExit, 'pending event capacity'):
                self.run_cli('answer', '1', '--text', 'cannot erase')
        self.assertEqual(self.board.get(1)['body'], before)
        self.assertIsNone(self.board.guard)
        self.board.issues[1]['body'] = taskq.block('g', {**self.task(1)['raw'], 'event_schema': 2})
        with self.assertRaisesRegex(SystemExit, 'unsupported event_schema'):
            self.run_cli('wait', '--window', '0')

    def test_closed_before_first_wait_and_same_account_foreign_session(self):
        self.worker()
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(self.task(1), None, 'close', 'accepted; open: none')
        self.board.close(1)
        event = json.loads(self.run_cli('wait', '--window', '0', '--json'))['events'][0]
        self.assertIn('closed #1 accepted; open: none', event['text'])
        with mock.patch.dict(os.environ, CLAUDE_CODE_SESSION_ID='another-agent'):
            self.assertEqual(self.run_cli('wait', '--window', '0'), 'tick\n')
            with self.assertRaisesRegex(SystemExit, 'recorded recipient'):
                self.run_cli('ack', event['id'])
            self.run_cli('ack', event['id'], '--pm', SESSION)  # explicit sender delegation
        self.assertEqual(self.run_cli('wait', '--window', '0'), 'tick\n')

    def test_adoption_resolves_wildcard_and_replacements_cancel_obsolete_recipients(self):
        fake, claim = self.worker()
        raw = self.task(1)['raw']; raw['pm'] = None
        self.board.issues[1]['body'] = taskq.block('g', raw)
        self.run_cli('ask', '1', '--text', 'shared question')
        token = json.loads(self.run_cli('wait', '--json', '--window', '0'))['events'][0]['id']
        self.run_cli('ack', token)
        self.assertIn(taskq.EVENT_LABEL, self.board.get(1)['labels'])  # other managers can still observe while unowned
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(self.task(1), 'ask', 'adopt', pm=taskq.origin())
        self.assertNotIn(taskq.EVENT_LABEL, self.board.get(1)['labels'])
        self.run_cli('answer', '1', '--text', 'old worker answer')
        self.assertTrue(taskq.event_pending(self.task(1)['raw'], taskq.recipient('worker', claim)))
        replacement = {**claim, 'session': 'replacement-worker'}
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(self.task(1), 'doing', 'spawn', 'worker replacement-worker', claim=replacement)
        self.assertEqual(taskq.event_pending(self.task(1)['raw'], taskq.recipient('worker', claim)), [])
        self.assertEqual(taskq.event_pending(self.task(1)['raw'], taskq.recipient('worker', replacement)), [])
        self.assertEqual(taskq.recorded(self.board.get(1), 'fake', 'worker-one'), 'worker')

    def test_answer_delivery_does_not_consume_a_separate_pending_nudge(self):
        fake, claim = self.worker()
        self.run_cli('ask', '1', '--text', 'question')
        self.run_cli('answer', '1', '--text', 'answer')
        self.board.comment(1, 'nudge: also inspect the output')
        self.board.comment(1, 'unrelated ordinary note')
        with taskq.coordination(), contextlib.redirect_stdout(io.StringIO()):
            taskq.follow(self.task(1), fake, claim, True)
            taskq.follow(self.task(1), fake, claim, True)
            taskq.follow(self.task(1), fake, claim, True)
        self.assertEqual([text for _, text in fake.sent], ['The owner answered your question:\n\nanswer', 'also inspect the output'])

    def test_busy_worker_never_fetches_history_or_consumes_event(self):
        fake, claim = self.worker()
        self.run_cli('ask', '1', '--text', 'question'); self.run_cli('answer', '1', '--text', 'answer')
        item, issue = self.task(1), self.board.get(1)
        fake.sessions[claim['session']] = True
        metadata = {key: value for key, value in issue.items() if key != 'comments'}
        with mock.patch.object(self.board, 'metadata', return_value=metadata, create=True), \
                mock.patch.object(self.board, 'get', side_effect=AssertionError('busy history fetched')):
            self.assertEqual(taskq.follow(item, fake, claim, True), claim)
        self.assertEqual(fake.sent, [])
        self.assertTrue(taskq.event_pending(self.task(1)['raw'], taskq.recipient('worker', claim)))

    def test_replacement_supervisor_cancels_its_old_events_and_missing_logs_keep_counts(self):
        self.worker()
        boss = {'runtime': 'fake', 'session': 'old-boss', 'name': 'mac'}
        with contextlib.redirect_stdout(io.StringIO()), mock.patch.object(self.board, 'comment', side_effect=RuntimeError('log down')):
            taskq.move(self.task(1), 'doing', 'spawn', 'supervisor old-boss', supervisor=boss)
            taskq.move(self.task(1), 'review', 'result', 'result payload', result={'sha': 'a' * 40})
            self.assertTrue(taskq.event_pending(self.task(1)['raw'], taskq.recipient('supervisor', boss)))
            taskq.move(self.task(1), 'review', 'spawn', 'supervisor new-boss', supervisor={**boss, 'session': 'new-boss'})
            taskq.move(self.task(1), 'review', 'gone', 'supervisor new-boss is gone')
        self.assertEqual(taskq.event_pending(self.task(1)['raw'], taskq.recipient('supervisor', boss)), [])
        self.assertEqual(self.task(1)['raw']['retry_counts']['supervisor'], 1)
        self.assertEqual(taskq.recorded(self.board.get(1), 'fake', 'old-boss'), 'supervisor')

    def test_manager_runtime_collision_never_wakes_or_acks_another_runtime(self):
        self.worker(); self.run_cli('ask', '1', '--text', 'Claude question')
        native = mock.Mock()
        native.owned.return_value = True
        native.state.return_value = 'idle'
        native.wake_manager.return_value = SESSION
        with mock.patch.dict(os.environ, TASKQ_RUNTIME='hermes', HERMES_SESSION_ID=SESSION), \
                mock.patch.object(taskq, 'runtimes', return_value={'hermes': native}):
            self.assertEqual(self.run_cli('wait', '--window', '0'), 'tick\n')
            observed = json.loads(self.run_cli('wait', '--window', '0', '--pm', SESSION, '--json'))
            self.assertEqual(len(observed['events']), 1)  # an explicitly delegated sender may observe it
            native.wake_manager.assert_not_called()
            with self.assertRaisesRegex(SystemExit, 'recorded recipient'):
                self.run_cli('ack', observed['events'][0]['id'])
        self.assertTrue(taskq.event_pending(self.task(1)['raw'], taskq.recipient('manager', self.task(1)['pm'])))

    def test_migration_imports_all_pending_answers_after_the_worker_boundary(self):
        fake, claim = self.worker()
        raw = {key: value for key, value in self.task(1)['raw'].items() if key not in ('event_schema', 'events', 'event_seq', 'action')}
        self.board.issues[1]['body'] = taskq.block('g', raw)
        self.board.issues[1]['comments'] += ['**answer** · owner\n\nalready delivered',
            '**nudge** · owner\n\nworker worker-one', '**answer** · owner\n\nfirst pending',
            'ordinary progress', '**answer** · owner\n\nsecond pending']
        self.run_cli('migrate', '--apply', '--controllers-stopped')
        pending = taskq.event_pending(self.task(1)['raw'], taskq.recipient('worker', claim))
        self.assertEqual([event['text'] for event in pending], ['first pending', 'second pending'])
        self.assertEqual(len({event['id'] for event in pending}), 2)
        with taskq.coordination(), contextlib.redirect_stdout(io.StringIO()):
            taskq.follow(self.task(1), fake, claim, False)
            taskq.follow(self.task(1), fake, claim, False)
        self.assertEqual(fake.sent, [('worker-one', 'The owner answered your question:\n\nfirst pending\n\nsecond pending')])

    def test_resumed_worker_keeps_acknowledged_manual_nudge_cursor(self):
        fake, claim = self.worker()
        self.board.comment(1, 'nudge: already delivered instruction')
        raw = self.task(1)['raw']; raw['worker_comment_cursor'] = {'worker-one': len(self.board.issues[1]['comments']) - 1}
        self.board.issues[1]['body'] = taskq.block('g', raw)
        self.run_cli('ask', '1', '--text', 'question'); self.run_cli('answer', '1', '--text', 'new answer')
        fake.send = mock.Mock(return_value='worker-two'); fake.sessions['worker-two'] = 'idle'
        with taskq.coordination(), contextlib.redirect_stdout(io.StringIO()):
            resumed = taskq.follow(self.task(1), fake, claim, True)
            taskq.follow(self.task(1), fake, resumed, True)
        fake.send.assert_called_once_with('worker-one', 'The owner answered your question:\n\nnew answer')
        self.assertEqual(self.task(1)['raw']['worker_comment_cursor']['worker-two'], raw['worker_comment_cursor']['worker-one'])

    def test_wait_does_not_report_review_worker_as_gone_or_overwrite_fresh_state(self):
        fake, claim = self.worker(); fake.sessions[claim['session']] = False
        with mock.patch.object(taskq, 'runtimes', return_value={'fake': fake}):
            self.run_cli('result', '1', '--sha', 'a' * 40)
            out = self.run_cli('wait', '--window', '0')
            self.assertIn('review #1', out); self.assertNotIn('gone #1', out)
            self.assertNotIn('observed_dead', self.task(1)['raw'])
            # A read after guard acquisition sees a result or an ownership change, even though list said doing.
            for change in ('state', 'pm'):
                with self.subTest(change=change):
                    item = self.task(1)
                    with contextlib.redirect_stdout(io.StringIO()):
                        taskq.move(item, 'doing', 'take', claim=claim)
                    def fresh(n):
                        if change == 'state':
                            self.board.issues[n]['labels'] = ['q-review']
                        else:
                            body = taskq.issue_data(self.board.get(n)); body['pm'] = {'runtime': 'claude', 'session': 'other-manager', 'name': 'mac'}
                            self.board.issues[n]['body'] = taskq.block('g', body)
                        return self.board.get(n)
                    with mock.patch.object(self.board, 'metadata', side_effect=fresh, create=True):
                        self.assertNotIn('gone #1', self.run_cli('wait', '--window', '0'))
                    self.assertNotIn('observed_dead', self.task(1)['raw'])

    def test_failed_rework_and_result_logs_keep_full_payload_through_session_changes(self):
        fake, claim = self.worker()
        boss = {'runtime': 'claude', 'session': SESSION, 'name': 'mac'}
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(self.task(1), 'doing', 'spawn', 'supervisor ' + SESSION, supervisor=boss)
        fixes = 'FIX_UNIQUE: repair the complete implementation'
        with mock.patch.object(self.board, 'comment', side_effect=RuntimeError('log unavailable')):
            self.run_cli('requeue', '1', '--text', fixes)
        self.assertIn(fixes, taskq.brief(self.task(1), 'fake'))
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(self.task(1), 'doing', 'spawn', 'worker replacement', claim={**claim, 'session': 'replacement'}, order=None)
        result = 'Completed research\nUNIQUE_RESULT_DETAILS_REQUIRED_BY_REVIEW'
        with mock.patch.object(self.board, 'comment', side_effect=RuntimeError('log unavailable')):
            self.run_cli('result', '1', '--sha', 'a' * 40, '--text', result)
        fake.sessions[SESSION] = 'idle'; fake.send = mock.Mock(return_value='resumed-supervisor')
        with taskq.coordination(), contextlib.redirect_stdout(io.StringIO()):
            taskq.supervise(self.task(1), {'claude': fake, 'fake': fake})
        current = self.task(1)
        self.assertEqual(current['supervisor']['session'], 'resumed-supervisor')
        self.assertEqual(current['raw']['action_payloads']['requeue']['text'], fixes)
        self.assertEqual(current['raw']['action_payloads']['result']['text'], result)
        self.assertIn(result, taskq.supervisor_brief(current, 'claude', fake))
        self.assertIn(fixes, taskq.brief(current, 'fake'))

    def test_legacy_answer_ignores_unrelated_last_comment(self):
        fake, claim = self.worker()
        issue = self.board.get(1)
        raw = {key: value for key, value in self.task(1)['raw'].items() if key not in ('event_schema', 'events', 'event_seq', 'action')}
        issue['body'] = taskq.block('g', raw)
        issue['comments'] += ['**answer** · owner\n\nlegacy answer', 'ordinary comment']
        self.assertEqual(taskq.legacy_worker_message(issue, claim, False)[:2], ('answer', 'legacy answer'))
        migrated = taskq.initialize_events(issue)
        self.assertEqual(migrated['events'][-1]['text'], 'legacy answer')
        self.assertEqual(migrated['claim'], claim)


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
                self.assertEqual(self.wait_and_ack(), 'tick\n')
        for sid in ('s-S1', 's-S2'):
            with self.acting(sid):
                self.run_cli('run', sid[-1])
        with self.acting('s-T1'):
            self.run_cli('result', '1', '--sha', 'a' * 40)
        with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)), self.acting('s-S1'):
            self.run_cli('close', '1', '--text', 'checked; works; open: none')
        with self.acting('s-T2'):
            self.run_cli('ask', '2', '--text', 'which?')
        with self.acting('s-T3'):
            self.run_cli('result', '3', '--sha', 'b' * 40)
        with self.pm(self.A):
            self.assertEqual(self.wait_and_ack(), 'review #3\nclosed #1 checked; works; open: none\n')
        with self.pm(self.B):
            self.assertEqual(self.wait_and_ack(), 'ask #2\nreview #3\n')
        for env in (self.A, self.B):
            with self.pm(env):
                self.assertEqual(self.wait_and_ack(), 'tick\n')  # never twice, never the other's

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
            self.assertIn('| [#1 one](https://board/1) | blocked (no manager) |', self.run_cli('tick'))
        claim = self.task(2)['claim']
        with self.pm(self.B):
            self.run_cli('pm', '--adopt', '1', '2')
        with self.pm(self.A), self.assertRaisesRegex(SystemExit, 'cannot adopt: #1 .claude:pmB-clau. already has a manager'):
            self.run_cli('pm', '--adopt', '1')
        self.assertEqual((self.task(1)['pm']['session'], self.task(2)['claim']), ('pmB-claude', claim))
        self.assertEqual(self.board.issues[1]['comments'][-1], '**adopt** · claude:pmB-clau\n\npm claude:pmB-claude')
        with self.pm(self.A):
            self.run_cli('tick')
        self.assertEqual((getattr(self.cdx, 'names', []), self.cld.names), ([], ['S1 CLD one (mac)']))  # A's pass starts B's task in B's runtime; ORCH is B's, not A's (#572)

    def test_overlapping_adoptions_never_overwrite(self):
        # the review's interleaving: B adopts while A is between its read and its write. A holds the dispatch lock across
        # both, so B is refused with nothing written (a later B meets A's pm: test_adoption_is_explicit_and_keeps_claims)
        with self.pm({}):
            self.add('one', '--runtime', 'fake')
        get, refused = self.board.get, []

        def get_then_b(n):  # B's adoption runs inside A's, right after A read the task
            issue = get(n)
            if not refused:
                with self.pm(self.B), self.assertRaisesRegex(SystemExit, 'holds the project guard'):
                    refused.append(self.run_cli('pm', '--adopt', '1'))
            return issue
        with self.pm(self.A), mock.patch.object(self.board, 'get', get_then_b):
            self.run_cli('pm', '--adopt', '1')
        self.assertEqual(self.task(1)['pm']['session'], 'pmA-codex')
        self.assertEqual(self.notes(1), ['**add**', '**adopt**'])  # one adoption, B wrote nothing
    def test_concurrent_adoption_clis_one_wins(self):
        # bounded real evidence: two `taskq pm --adopt 1` processes at once on a file board with a slow read; exactly one wins
        (self.root / 'taskq.json').write_text('{"board": "board.py", "update": false, "limits": {"claude": 0, "codex": 0}}')
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
        with open(self.root / 'board.py', 'a') as fixture:
            fixture.write('\nimport pathlib\n' + FILE_GUARD)
        with self.pm({}):
            self.add('one', '--runtime', 'fake')
        (self.root / 'issues.json').write_text(json.dumps({'1': {**self.board.issues[1], 'iid': 1}}))
        script = Path(taskq.__file__).resolve()
        procs = [REAL_POPEN([sys.executable, str(script), 'pm', '--adopt', '1'], cwd=self.root, text=True, encoding='utf-8',
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
            return self.wait_and_ack( '--pm', pm)

    def test_agent_sender_delivers_its_managers_outcome_once(self):
        # a separate Claude sender session serves pmA-codex: it gets A's ask once; A's own wait shares the receipt; B keeps its own
        self.senders_setup()
        sender = {'CLAUDE_CODE_SESSION_ID': 'sender-claude', 'TASKQ_RUNTIME': 'claude'}
        self.assertEqual(self.sender_wait(sender, 'pmA-codex'), 'ask #1\n')
        self.assertEqual(self.sender_wait(sender, 'pmA-codex'), 'tick\n')
        with self.pm(self.A):
            self.assertEqual(self.wait_and_ack(), 'tick\n')  # same receipt file: never twice
        with self.pm(self.B):
            self.assertEqual(self.wait_and_ack(), 'ask #2\n')  # never consumed by A's sender
        self.assertFalse((self.root / '.taskq' / 'wait-sender-claude.json').exists())

    def test_shell_sender_delivers_its_managers_outcome_once(self):
        # the printed shell loop runs with no session: it waits as the manager its link names, not as the owner's shell
        self.senders_setup()
        self.assertEqual(self.sender_wait({}, 'https://claude.ai/code/session_pmB-claude'), 'ask #2\n')
        with self.pm(self.B):
            self.assertEqual(self.wait_and_ack(), 'tick\n')
        with self.pm(self.A):
            self.assertEqual(self.wait_and_ack(), 'ask #1\n')
        self.assertFalse((self.root / '.taskq' / 'wait.json').exists())


class Contract(Base):
    """The running release owns its contract; explicit updates never replace running code."""

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

    def test_pm_onboarding_is_one_read_only_snapshot(self):
        (self.root / 'taskq.md').write_text((ROOT / 'taskq.md').read_text())
        self.add('assigned')
        with mock.patch.dict(os.environ, {'TASKQ_RUNTIME': ''}, clear=True):
            self.add('unassigned')
            self.add('claimed')
        with contextlib.redirect_stdout(io.StringIO()):
            taskq.move(self.task(3), 'doing', 'take', claim={'runtime': 'codex', 'session': 'foreign', 'name': 'mac'})
        self.board.add('ordinary issue', '', [])
        taskq.CONFIG['assignee'] = 'nobody'  # onboarding must reveal tasks hidden by the report filter
        before = json.dumps(self.board.issues, sort_keys=True)
        with mock.patch.object(self.board, 'list', wraps=self.board.list) as listed, \
                mock.patch.object(taskq, 'dispatch', side_effect=AssertionError('pm dispatched')):
            out = self.run_cli('pm')
        listed.assert_called_once_with(None)
        self.assertEqual(json.dumps(self.board.issues, sort_keys=True), before)
        hints = [line for line in out.splitlines() if line.startswith('Unassigned manager:')]
        self.assertEqual(len(hints), 2)
        for n, line in zip((2, 3), hints):
            self.assertIn(f'#{n} ', line)
            self.assertIn(f'project {self.root.name}:', line)
            self.assertIn(f'cd {taskq.shlex.quote(str(self.root))} && taskq pm --adopt {n}', line)
            self.assertIn('No ownership or claims changed.', line)
        for part in ('executing PM fills the field', 'relays the completed block unchanged',
                     'must not replace known executing-PM state', 'no interval from a default or `arm tick` output',
                     'If an answer is ambiguous across projects', 'never automatic',
                     'no stale table presented as fresh', 'no fabricated zero counters',
                     'An explicit owner request to arm means execute', 'continued next wait',
                     'repeated arm must', 'Keep external runtime blockers visible',
                     'do not begin login, retry GitLab or change credentials',
                     'verified actionable session link', "decision's `--link` field", 'Problem N (session choice, not board task)',
                     'never passes it to `taskq answer`', 'require the project and `task` or `problem` qualifier',
                     'for macOS Codex desktop use `TASKQ_CLIENT=codex`', 'Root never rewrites links',
                     'never ARM evidence or a fourth ARM state', 'send acceptance alone is not receipt',
                     'detailed PM proof stays private'):
            self.assertIn(part, out)

    def test_pm_refuses_recorded_session_roles_before_onboarding(self):
        self.add()
        (self.root / 'taskq.md').write_text((ROOT / 'taskq.md').read_text())
        for field in ('claim', 'supervisor'):
            with self.subTest(field=field):
                with contextlib.redirect_stdout(io.StringIO()):
                    taskq.move(self.task(1), 'doing', 'take',
                               **dict({'claim': None, 'supervisor': None},
                                      **{field: {'runtime': 'claude', 'session': SESSION, 'name': 'mac'}}))
                before = json.dumps(self.board.issues, sort_keys=True)
                with self.assertRaisesRegex(SystemExit, 'cannot take the manager role'):
                    self.run_cli('pm')
                self.assertEqual(json.dumps(self.board.issues, sort_keys=True), before)

    def test_changed_contract_until_pm(self):
        line = 'The manager contract changed: run taskq pm and follow it from now on.'
        (self.root / 'taskq.md').write_text('v1\n## Principles\nx\n## 7. Manager\ny\n## 8. Runtimes\n')
        self.run_cli('pm')
        self.assertNotIn(line, self.run_cli('tick'))
        (self.root / 'taskq.md').write_text('v2\n## Principles\nx\n## 7. Manager\ny\n## 8. Runtimes\n')
        with mock.patch.object(taskq.time, 'time', side_effect=[0, 1e9]):
            self.assertEqual(self.wait_and_ack().splitlines(), [line, 'tick'])
        self.assertEqual(self.run_cli('tick').splitlines()[0], line)  # first, once per run
        self.run_cli('pm')
        self.assertNotIn(line, self.run_cli('tick'))

    def test_refresh_never_fetches_or_changes_source(self):
        (self.root / '.git').write_text('gitdir: elsewhere')  # linked worktree also remains untouched
        (self.root / 'taskq.md').write_text('old contract')
        self.run_cli('pm')
        (self.root / 'taskq.md').write_text('new contract')
        with mock.patch.object(taskq.subprocess, 'run', side_effect=AssertionError('refresh ran git')):
            self.assertIn('contract changed', self.run_cli('tick'))
            self.run_cli('pm')
        self.assertEqual((self.root / 'taskq.md').read_text(), 'new contract')

    def test_update_exact_sha_with_real_isolated_git_and_atomic_pointer(self):
        origin, writer, install = self.root / 'origin.git', self.root / 'writer', self.root / 'install'
        with mock.patch.object(taskq.subprocess, 'run', REAL_RUN), mock.patch.object(taskq.subprocess, 'Popen', REAL_POPEN), \
                mock.patch.object(taskq, 'CLONE', writer), \
                mock.patch.dict(os.environ, {'PATH': os.defpath + os.pathsep + os.environ.get('PATH', '')}):
            REAL_RUN(['git', 'init', '-q', '--bare', str(origin)], check=True)
            REAL_RUN(['git', 'clone', '-q', str(origin), str(writer)], check=True, capture_output=True)
            self.git(writer, 'checkout', '-qb', 'main')
            (writer / 'taskq.py').write_text('print("qualified")\n')
            (writer / 'taskq.md').write_text('qualified contract\n')
            self.git(writer, 'add', '.')
            self.git(writer, 'commit', '-qm', 'candidate')
            self.git(writer, 'push', '-q', '-u', 'origin', 'main')
            sha = taskq.update_git(writer, 'rev-parse', 'HEAD')
            args = ('update', '--commit', sha, '--install-dir', str(install))
            self.assertIn('Preview only', self.run_cli(*args))
            self.assertFalse(install.exists())
            proof = self.root / 'proof.json'
            proof.write_text(json.dumps(dict(commit=sha, upstream=str(origin), tests='passed', review='accepted')))
            with mock.patch.object(taskq, 'qualified_checks', side_effect=SystemExit('CI red')):
                with self.assertRaisesRegex(SystemExit, 'CI red'):
                    self.run_cli(*args, '--apply', '--qualification', str(proof))
            self.assertFalse(install.exists())
            install.mkdir()
            pointer = install / 'current.json'
            pointer.write_text(json.dumps({'commit': sha, 'path': str(writer)}))
            before = taskq.update_git(writer, 'rev-parse', 'HEAD')
            with mock.patch.object(taskq, 'qualified_checks') as checked:
                self.run_cli(*args, '--apply', '--qualification', str(proof))
                checked.assert_called_once_with(sha, str(origin))
            selected = json.loads(pointer.read_text())
            release = Path(selected['path'])
            self.assertEqual(selected['commit'], sha)
            self.assertEqual(taskq.update_git(release, 'rev-parse', 'HEAD'), sha)
            self.assertEqual((release / 'taskq.md').read_text(), 'qualified contract\n')
            self.assertEqual(taskq.update_git(writer, 'rev-parse', 'HEAD'), before)
            self.assertEqual(taskq.update_git(writer, 'status', '--porcelain'), '')
            old_pointer = pointer.read_text()
            with mock.patch.object(taskq, 'qualified_checks'):
                self.run_cli(*args, '--apply', '--qualification', str(proof))
            self.assertEqual(pointer.read_text(), old_pointer)
            # A second exact SHA cannot race an active installer; a completed checkout survives pointer failure.
            (writer / 'taskq.md').write_text('next qualified contract\n')
            self.git(writer, 'commit', '-qam', 'next')
            self.git(writer, 'push', '-q', 'origin', 'main')
            next_sha = taskq.update_git(writer, 'rev-parse', 'HEAD')
            next_proof = self.root / 'next-proof.json'
            next_proof.write_text(json.dumps(dict(commit=next_sha, upstream=str(origin), tests='passed', review='accepted')))
            next_args = ('update', '--commit', next_sha, '--install-dir', str(install), '--apply', '--qualification', str(next_proof))
            with taskq.installation_lock(install), mock.patch.object(taskq, 'qualified_checks'):
                with self.assertRaisesRegex(SystemExit, 'installation busy'):
                    self.run_cli(*next_args)
            self.assertEqual(pointer.read_text(), old_pointer)
            with mock.patch.object(taskq, 'qualified_checks'), mock.patch.object(taskq.os, 'replace', side_effect=OSError('pointer interrupted')):
                with self.assertRaisesRegex(OSError, 'pointer interrupted'):
                    self.run_cli(*next_args)
            self.assertEqual(pointer.read_text(), old_pointer)
            self.assertFalse((install / '.update.lock').exists())
            with mock.patch.object(taskq, 'qualified_checks'):
                self.run_cli(*next_args)
            self.assertEqual(json.loads(pointer.read_text())['commit'], next_sha)
            old_pointer = pointer.read_text()
            with mock.patch.object(taskq, 'qualified_checks'), self.assertRaisesRegex(SystemExit, 'update git merge-base'):
                self.run_cli(*args, '--apply', '--qualification', str(proof))
            self.assertEqual(pointer.read_text(), old_pointer)
            # A reviewed commit outside upstream main is not installable; failed checkout never switches pointer.
            self.git(writer, 'checkout', '-qb', 'unmerged')
            (writer / 'taskq.md').write_text('not merged\n')
            self.git(writer, 'commit', '-qam', 'unmerged')
            unmerged = taskq.update_git(writer, 'rev-parse', 'HEAD')
            proof.write_text(json.dumps(dict(commit=unmerged, upstream=str(origin), tests='passed', review='accepted')))
            with mock.patch.object(taskq, 'qualified_checks'), self.assertRaisesRegex(SystemExit, 'update git merge-base'):
                self.run_cli('update', '--commit', unmerged, '--install-dir', str(install), '--apply', '--qualification', str(proof))
            self.assertEqual(pointer.read_text(), old_pointer)

    def test_update_requires_exact_upstream_ci_not_just_operator_evidence(self):
        sha = 'a' * 40
        good = dict(name='tests', head_sha=sha, status='completed', conclusion='success')
        cases = [([], False), ([good], True), ([good, dict(good, conclusion='failure')], False),
                 ([dict(good, head_sha='b' * 40)], False), ([dict(good, status='queued')], False)]
        for checks, allowed in cases:
            with self.subTest(checks=checks), mock.patch.object(taskq.shutil, 'which', return_value='gh'), \
                    mock.patch.object(taskq.subprocess, 'run', return_value=mock.Mock(returncode=0,
                        stdout=json.dumps([{'check_runs': checks}]), stderr='')):
                if allowed:
                    taskq.qualified_checks(sha, 'https://github.com/alexkirs/taskq')
                else:
                    with self.assertRaisesRegex(SystemExit, 'exact qualified SHA'):
                        taskq.qualified_checks(sha, 'https://github.com/alexkirs/taskq')
        with self.assertRaisesRegex(SystemExit, 'canonical'):
            taskq.qualified_checks(sha, 'https://example.com/arbitrary/taskq')

    def test_update_ci_pins_canonical_host_despite_gh_host_override(self):
        sha = 'a' * 40
        check = dict(name='tests', head_sha=sha, status='completed', conclusion='success')
        with mock.patch.dict(os.environ, {'GH_HOST': 'enterprise.invalid'}), \
                mock.patch.object(taskq.shutil, 'which', return_value='gh'), \
                mock.patch.object(taskq.subprocess, 'run', return_value=mock.Mock(returncode=0,
                    stdout=json.dumps([{'check_runs': [check]}]), stderr='')) as run:
            taskq.qualified_checks(sha, 'https://github.com/alexkirs/taskq')
        argv = run.call_args.args[0]
        self.assertEqual(argv[argv.index('--hostname') + 1], 'github.com')
        self.assertEqual(argv[-1], f'repos/alexkirs/taskq/commits/{sha}/check-runs')

    def test_managed_launcher_pins_new_process_code_and_contract(self):
        install = self.root / 'installed'
        source = (ROOT / 'taskq.py').read_text('utf-8')
        for label, marker in (('a', 'A'), ('b', 'B')):
            release = install / 'releases' / (label * 40)
            release.mkdir(parents=True)
            (release / 'taskq.py').write_text(source, encoding='utf-8')
            (release / 'taskq.md').write_text(f'contract {marker}', encoding='utf-8')
        pointer = install / 'current.json'
        command = [sys.executable, '-X', 'utf8', '-B', str(ROOT / 'taskq.py'), 'launch', '--install-dir', str(install), '--', 'version']
        with mock.patch.object(taskq.subprocess, 'Popen', REAL_POPEN):
            for label, marker in (('a', 'A'), ('b', 'B')):
                release = install / 'releases' / (label * 40)
                pointer.write_text(json.dumps({'commit': release.name, 'path': str(release)}))
                child = REAL_RUN(command, cwd=self.root, capture_output=True, text=True, encoding='utf-8',
                                 env={**os.environ, 'PYTHONDONTWRITEBYTECODE': '1'}, timeout=15)
                self.assertEqual(child.returncode, 0, child.stderr)
                result = json.loads(child.stdout)
                self.assertEqual(result['source'], str(release))
                self.assertEqual(result['contract'], taskq.hashlib.sha256(f'contract {marker}'.encode()).hexdigest()[:12])
                self.assertEqual(result['selected']['commit'], release.name)
                self.assertEqual(result['install'], str(install))
                self.assertFalse(result['stale'])
        # Forwarded argv and pinned release identity use this same interpreter, with no board/config read.
        with mock.patch.dict(os.environ, {'TASKQ_INSTALL_DIR': str(install)}), mock.patch.object(taskq.os, 'execv') as execute:
            self.run_cli('launch', '--', 'contract')
            execute.assert_called_once_with(sys.executable, [sys.executable, str(release / 'taskq.py'), 'contract'])
            self.assertEqual(os.environ['TASKQ_RELEASE_COMMIT'], release.name)

    def test_managed_launcher_refuses_invalid_or_escaping_release(self):
        install = self.root / 'installed'
        release = install / 'releases' / ('a' * 40)
        release.mkdir(parents=True)
        (release / 'taskq.py').write_text('print("never")')
        (release / 'taskq.md').write_text('contract')
        pointer = install / 'current.json'
        cases = [dict(commit='short', path=str(release)), dict(commit=release.name, path=str(self.root)),
                 dict(commit='b' * 40, path=str(release)), dict(commit=release.name, path='relative'), []]
        for data in cases:
            pointer.write_text(json.dumps(data))
            with self.subTest(data=data), mock.patch.object(taskq.os, 'execv') as execute:
                with self.assertRaisesRegex(SystemExit, 'invalid installation pointer/release'):
                    self.run_cli('launch', '--install-dir', str(install), '--', 'version')
                execute.assert_not_called()
        pointer.write_text(json.dumps(dict(commit=release.name, path=str(release))))
        (release / 'taskq.md').unlink()
        with self.assertRaisesRegex(SystemExit, 'no regular taskq.md'):
            self.run_cli('launch', '--install-dir', str(install), '--', 'version')

    def test_update_network_timeouts_are_bounded_and_preserve_pointer(self):
        pointer = self.root / 'current.json'
        pointer.write_text('previous selection')
        timeout = subprocess.TimeoutExpired('offline', 30)
        with mock.patch.object(taskq.subprocess, 'run', side_effect=timeout) as run:
            for operation, limit in (('ls-remote', 30), ('clone', 120)):
                with self.assertRaisesRegex(SystemExit, 'timed out'):
                    taskq.update_git(self.root, operation, 'origin')
                self.assertEqual(run.call_args.kwargs['timeout'], limit)
            with mock.patch.object(taskq.shutil, 'which', return_value='gh'), self.assertRaisesRegex(SystemExit, 'CI timed out'):
                taskq.qualified_checks('a' * 40, 'https://github.com/alexkirs/taskq')
            self.assertEqual(run.call_args.kwargs['timeout'], 30)
        self.assertEqual(pointer.read_text(), 'previous selection')

    def test_stale_managed_process_keeps_contract_but_refuses_effects(self):
        install = self.root / 'installation'
        old = install / 'releases' / ('a' * 40)
        new = install / 'releases' / ('b' * 40)
        old.mkdir(parents=True); new.mkdir()
        (old / 'taskq.md').write_text('old contract')
        (new / 'taskq.md').write_text('new contract')
        pointer = install / 'current.json'
        pointer.write_text(json.dumps({'commit': old.name, 'path': str(old)}))
        with mock.patch.dict(os.environ, {'TASKQ_INSTALL_DIR': str(install)}), mock.patch.object(taskq, 'CLONE', old):
            self.add('before switch')
            old_digest = taskq.contract()
            pointer.write_text(json.dumps({'commit': new.name, 'path': str(new)}))
            self.assertEqual(taskq.contract(), old_digest)
            self.assertIn(str(old / 'taskq.md'), self.run_cli('contract'))
            self.assertIn('before switch', self.run_cli('list'))
            before = json.dumps(self.board.issues, sort_keys=True)
            with self.assertRaisesRegex(SystemExit, 'stale or invalid'):
                self.add('old process cannot write')
            with self.assertRaisesRegex(SystemExit, 'stale or invalid'):
                taskq.effect(self.board.comment, 1, 'cannot write through helper')
            with mock.patch.object(taskq, 'start_pass') as spawn, contextlib.redirect_stderr(io.StringIO()) as errors:
                taskq.dispatch('result', [1])
            spawn.assert_not_called()
            self.assertIn('stale or invalid', errors.getvalue())
            self.assertEqual(json.dumps(self.board.issues, sort_keys=True), before)
            with mock.patch.object(taskq, 'CLONE', new):
                self.assertIsNone(taskq.release_reason())
                self.assertNotEqual(taskq.contract(), old_digest)
                self.add('fresh process')
            self.assertEqual((old / 'taskq.md').read_text(), 'old contract')

    def test_event_label_setup_is_explicit_and_unknown_create_retains_guard(self):
        for adapter, tool in ((taskq.GitHub, 'gh'), (taskq.GitLab, 'glab')):
            board = adapter('owner/repo')
            with self.subTest(adapter=tool), mock.patch.object(taskq, 'coordination_api',
                    return_value=(200, {'name': 'taskq-events'})) as api:
                board.ensure_event_label()
                self.assertEqual(api.call_count, 1)
                self.assertEqual(api.call_args.args[0], tool)
            with self.subTest(create=tool), mock.patch.object(taskq, 'coordination_api',
                    side_effect=[(404, {}), (201, {'name': 'taskq-events'})]) as api:
                with taskq.coordination():
                    board.ensure_event_label()
                self.assertEqual(api.call_count, 2)
                self.assertIsNone(self.board.guard)
        with mock.patch.object(taskq, 'coordination_api', side_effect=[(404, {}), (500, {})]):
            with self.assertRaisesRegex(SystemExit, 'creation failed/unknown'):
                with taskq.coordination():
                    board.ensure_event_label()
        self.assertIsNotNone(self.board.guard)

    def test_legacy_migration_guard_preservation_idempotence_and_future_refusal(self):
        self.add()
        issue = self.board.issues[1]
        raw = dict(self.task(1)['raw'], unknown={'keep': [1, 2]}, claim={'session': 'old'}, pm={'session': 'owner'})
        raw.pop('event_schema', None)
        original_text = '  human prose before\n\n'
        suffix = '\n  human prose after\n'
        issue['body'] = original_text + taskq.block('', raw).lstrip('\n') + suffix
        issue['comments'] = ['original trusted history']
        original = json.loads(json.dumps(issue))
        # Migration orchestration is tested independently of the event helper's own legacy conversion cases.
        def initialize(source):
            return dict(taskq.parse(source)['raw'], event_schema=1)
        with mock.patch.object(taskq, 'initialize_events', side_effect=initialize, create=True), \
                mock.patch.object(taskq, 'event_labels', side_effect=lambda raw, labels: labels, create=True):
            self.assertIn('1 task(s) would migrate', self.run_cli('migrate'))
            self.assertEqual(issue, original)
            with self.assertRaisesRegex(SystemExit, 'controllers-stopped'):
                self.run_cli('migrate', '--apply')
            with self.assertRaisesRegex(SystemExit, 'legacy event_schema'):
                self.run_cli('later', '1')
            self.assertIn('#1', self.run_cli('list'))
            self.assertEqual(issue, original)
            with mock.patch.object(self.board, 'update', wraps=self.board.update) as updated:
                self.run_cli('migrate', '--apply', '--controllers-stopped')
                updated.assert_called_once()
            self.assertIsNone(self.board.guard)
            migrated = json.loads(json.dumps(issue))
            self.assertEqual(self.task(1)['raw'], dict(raw, event_schema=1))
            self.assertTrue(issue['body'].startswith(original_text))
            self.assertTrue(issue['body'].endswith(suffix))
            for key in ('comments', 'labels', 'state'):
                self.assertEqual(issue[key], original[key])
            self.assertIn('0 task(s) migrated', self.run_cli('migrate', '--apply', '--controllers-stopped'))
            self.assertEqual(issue, migrated)
            self.add('second')
            issue['body'] = taskq.block('legacy', raw)
            self.board.issues[2]['body'] = taskq.block('future', dict(raw, event_schema=2))
            before = json.dumps(self.board.issues, sort_keys=True)
            with self.assertRaisesRegex(SystemExit, 'unsupported event_schema'):
                self.run_cli('migrate', '--apply', '--controllers-stopped')
            self.assertEqual(json.dumps(self.board.issues, sort_keys=True), before)
            self.assertIsNone(self.board.guard)



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
        (self.root / '.taskq' / 'S2.pid').write_text(f'999999999 old {BIRTH}')
        (self.root / '.taskq' / 'wait.json').write_text('{"1": "doing", "2": "review"}')

    def state(self):
        return (sorted(path.name for path in (self.root / '.worktrees').iterdir()),
                self.git('for-each-ref', '--format=%(refname)', 'refs/heads', 'refs/remotes/origin/main', 'refs/remotes/origin/taskq-*').split(),
                sorted(self.fake.sessions), sorted(path.name for path in (self.root / '.taskq').glob('*.pid')),
                (self.root / '.taskq' / 'wait.json').read_text())

    def cleanup(self, *argv):
        return self.run_cli('cleanup', *argv).splitlines()

    def test_cleanup_does_not_report_idle_or_unknown_worker_as_dead(self):
        real_state = self.fake.state
        real_alive = self.fake.alive
        for state in ('idle', None):
            with self.subTest(state=state), mock.patch.object(self.fake, 'state',
                    side_effect=lambda sid: state if sid == 's-T1' else real_state(sid)), \
                    mock.patch.object(self.fake, 'alive', side_effect=lambda sid: False if sid == 's-T1' else real_alive(sid)):
                self.assertNotIn('mess: #1 doing: session s-T1 is gone', self.cleanup('--dry-run'))

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

    def test_metadata_reads_skip_comments_and_history_keeps_adapter_trust(self):
        for board, lab in ((taskq.GitHub('o/r'), False), (taskq.GitLab('o/r'), True)):
            with self.subTest(board=type(board).__name__):
                taskq.BOARD = board
                raw = {'number': 1, 'iid': 1, 'title': 't', 'body': taskq.block('g', {}),
                       'description': taskq.block('g', {}), 'html_url': 'u', 'web_url': 'u',
                       'labels': ['q-ready'] if lab else [{'name': 'q-ready'}],
                       'state': 'opened' if lab else 'open', 'updated_at': 'now', 'author_association': 'OWNER'}
                board.members = {1}
                trusted = {'body': 'trusted answer', 'author': {'id': 1}, 'author_association': 'OWNER'}
                untrusted = {'body': 'untrusted answer', 'author': {'id': 2}, 'author_association': 'NONE'}
                with mock.patch.object(board, 'api', side_effect=[raw, raw, [trusted, untrusted]]) as api:
                    issue = taskq.read_issue(1)
                    self.assertNotIn('comments', issue)
                    self.assertEqual(taskq.open_deps([1]), [1])  # a second fresh metadata request
                    self.assertEqual(api.call_args_list, [mock.call('GET', 'issues/1')] * 2)
                    self.assertEqual(taskq.issue_history(issue)['comments'], ['trusted answer'])
                    self.assertEqual(api.call_count, 3)
                    self.assertIn('/notes?' if lab else '/comments?', api.call_args.args[1])
                with mock.patch.object(board, 'api', side_effect=[raw, [trusted]]) as api:
                    self.assertEqual(board.get(1)['comments'], ['trusted answer'])  # old public get stays complete
                    self.assertEqual(api.call_count, 2)

    def test_legacy_get_fallback_retains_comments_without_an_extra_read(self):
        self.add()
        with mock.patch.object(self.board, 'get', wraps=self.board.get) as get:
            issue = taskq.read_issue(1)
            self.assertEqual(taskq.issue_history(issue), issue)
            get.assert_called_once_with(1)

    def test_block_keeps_unknown_keys(self):
        board = taskq.BOARD = FakeBoard()
        n = board.add('t', taskq.block('text', {'event_schema': 1, 'scope': [], 'deps': [], 'claim': None, 'result': None, 'extra': {'x': 1}}),
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

    def test_assignee_only_native_adapters(self):
        for board, tool, key in ((taskq.GitHub('o/r'), 'gh', 'login'),
                                  (taskq.GitLab('o/r', 'gitlab.example'), 'glab', 'username')):
            with self.subTest(board=tool):
                native = {'title': 'T', 'body': taskq.block('g', {'event_schema': 1}), 'description': taskq.block('g', {'event_schema': 1}),
                          'number': 1, 'iid': 1, 'html_url': 'u', 'web_url': 'u', 'updated_at': 'now',
                          'author_association': 'OWNER', 'assignees': [{key: 'alice'}, {key: 'bob'}],
                          'labels': [{'name': 'q-ready'}, {'name': 'assignee-only'}] if tool == 'gh'
                                    else ['q-ready', 'assignee-only'], 'state': 'open' if tool == 'gh' else 'opened'}
                taskq.BOARD = board
                item = taskq.parse(board.issue(native))
                self.assertEqual(item['assignees'], ['alice', 'bob'])
                with mock.patch.object(taskq.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, json.dumps({key: 'alice'}), '')) as run, \
                        mock.patch.object(taskq.shutil, 'which', lambda name: name):
                    self.assertIsNone(taskq.execution_reason(item))
                command = run.call_args.args[0]
                self.assertEqual(command[1:6], ['api', '-X', 'GET', 'user'] + (['--hostname'] if tool == 'glab' else []))
                if tool == 'glab':
                    self.assertEqual(command[-1], 'gitlab.example')

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


APP_SERVER = '''import json, os, sys, time
mode, log = os.environ['FAKE_MODE'], open(os.environ['FAKE_LOG'], 'w')
for line in sys.stdin:
    message = json.loads(line)
    log.write(message['method'] + '\\n')
    log.flush()
    if mode == 'silent':
        time.sleep(60)
    if 'id' not in message:
        continue
    out = [{'method': 'note'}, {'id': 9, 'method': 'ask'}, {'id': 7, 'result': {}}]  # notifications and others first, slowly
    if message['method'] == 'thread/name/set' and mode == 'set-error':
        out.append({'id': message['id'], 'error': {'code': -32600, 'message': 'no rollout found'}})
    else:
        name = 'other' if mode == 'wrong-name' else 'S1 CDX one (mac)'
        out.append({'id': message['id'], 'result': {'thread': {'id': 'th', 'name': name}} if message['method'] == 'thread/read' else {}})
    for reply in out:
        time.sleep(0.05)
        print(json.dumps(reply), flush=True)
if mode == 'hang':  # replied, then never exits
    time.sleep(60)
'''

FILE_GUARD = """
import contextlib, sqlite3, uuid
GUARD_DB = pathlib.Path(__file__).with_name('coordination.sqlite')
def acquire(owner):
    with contextlib.closing(sqlite3.connect(GUARD_DB, timeout=5)) as db, db:
        db.execute('CREATE TABLE IF NOT EXISTS grant (slot INTEGER PRIMARY KEY CHECK (slot=1), token TEXT, owner TEXT)')
        token = str(uuid.uuid4())
        try:
            db.execute('INSERT INTO grant VALUES (1, ?, ?)', (token, owner))
        except sqlite3.IntegrityError:
            return None
        return token
def release(token):
    with contextlib.closing(sqlite3.connect(GUARD_DB, timeout=5)) as db, db:
        if db.execute('DELETE FROM grant WHERE slot=1 AND token=?', (token,)).rowcount != 1:
            raise RuntimeError('exact grant no longer present')
"""

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
FILE_BOARD += FILE_GUARD

FILE_RUNTIME = '''import pathlib
def spawn(name, prompt, cwd): pathlib.Path(__file__).with_name('spawned').write_text(name); return 's1'
def send(session, text): return session
def alive(session): return True
def link(session): return None
'''


class RealChild(unittest.TestCase):
    """#481: `add` in a real process starts the real detached `tick --quiet` child; it spawns and logs, though the list lags the add."""

    def test_quiet_guard_timeout_is_nonzero_without_writes(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / 'board.py').write_text(FILE_BOARD)
            (root / 'taskq.json').write_text(json.dumps({'board': 'board.py', 'update': False}))
            board = taskq.load_file('board.py', root)
            token = board.acquire('other process')
            try:
                code = f"import runpy; d=runpy.run_path({str(ROOT / 'taskq.py')!r}); d['main'].__globals__['GUARD_WAIT']=0.05; d['main'](['tick','--quiet'])"
                env = {k: v for k, v in os.environ.items() if k != 'CODEX_SANDBOX'}
                done = REAL_RUN([sys.executable, '-c', code], cwd=root, env=env, capture_output=True, text=True, encoding='utf-8', timeout=5)
                self.assertNotEqual(done.returncode, 0)
                self.assertIn('project guard busy', done.stderr)
                self.assertFalse((root / 'issues.json').exists())
                self.assertIsNone(board.acquire('still blocked'))
            finally:
                board.release(token)

    def test_two_checkouts_share_board_guard_and_one_slot(self):
        with tempfile.TemporaryDirectory() as folder:
            shared = Path(folder)
            issues = {str(n): {'iid': n, 'title': f'task {n}', 'body': taskq.block('g', {'event_schema': 1, 'deps': [],
                        'pm': {'runtime': 'fake', 'session': 'pm', 'name': 'fixture'}}),
                        'labels': ['q-ready'], 'state': 'open', 'listed': True, 'comments': [],
                        'updated_at': '2026-10-09T00:00:00Z', 'url': ''} for n in (1, 2)}
            (shared / 'issues.json').write_text(json.dumps(issues))
            roots = [shared / name for name in ('checkout-a', 'checkout-b')]
            processes = []
            try:
                for root in roots:
                    root.mkdir()
                    (root / 'taskq.json').write_text(json.dumps({'board': 'board.py', 'update': False,
                        'runtimes': {'fake': 'fake.py'}, 'limits': {'fake': 1}}))
                    (root / 'board.py').write_text(FILE_BOARD +
                        f'\nPATH = pathlib.Path({str(shared / "issues.json")!r})\n'
                        f'GUARD_DB = pathlib.Path({str(shared / "coordination.sqlite")!r})\n')
                    (root / 'fake.py').write_text(
                        'import pathlib, time\n'
                        f'LOG = pathlib.Path({str(shared / "spawned")!r})\n'
                        'def spawn(name, prompt, cwd):\n'
                        '    with LOG.open("a") as out: out.write(name + "\\n")\n'
                        '    time.sleep(0.3)\n'
                        '    return "session-" + name.split()[0]\n'
                        'def alive(sid): return True\n'
                        'def send(sid, text): return sid\n'
                        'def link(sid): return None\n')
                    env = {**os.environ, 'TASKQ_HOST': 'fixture'}
                    for key in (*taskq.SESSIONS.values(), 'TASKQ_RUNTIME', 'TASKQ_HOST_ONLY', 'TASKQ_LIMITS', 'CODEX_SANDBOX'):
                        env.pop(key, None)
                    processes.append(REAL_POPEN([sys.executable, str(ROOT / 'taskq.py'), 'tick'], cwd=root,
                        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding='utf-8'))
                outputs = [process.communicate(timeout=15) for process in processes]
                self.assertEqual([process.returncode for process in processes], [0, 0], outputs)
                self.assertEqual(len((shared / 'spawned').read_text().splitlines()), 1, outputs)
                current = json.loads((shared / 'issues.json').read_text())
                self.assertEqual(current['1']['labels'], ['q-doing'])
                self.assertEqual(current['2']['labels'], ['q-ready'])
                self.assertEqual(taskq.parse(current['1'])['supervisor']['session'], 'session-S1')
                self.assertFalse(any((root / '.taskq' / name).exists() for root in roots
                                     for name in ('dispatch.lock', 'dispatch.pending')))
            finally:
                for process in processes:
                    if process.poll() is None:
                        process.kill()
                    process.communicate(timeout=5)

    def test_native_redirected_unicode_report_and_cli_error(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / 'taskq.py').write_text((ROOT / 'taskq.py').read_text('utf-8'), encoding='utf-8')
            (root / 'board.py').write_text(FILE_BOARD, encoding='utf-8')
            (root / 'taskq.json').write_text(json.dumps({'board': 'board.py', 'update': False, 'board_url': 'https://board'}))
            issue = {'iid': 1, 'title': 'Привет 🌍', 'body': taskq.block('g', {'event_schema': 1, 'deps': []}), 'labels': ['q-ready'],
                     'state': 'open', 'listed': True, 'comments': [], 'updated_at': '2026-10-09T00:00:00Z', 'url': ''}
            (root / 'issues.json').write_text(json.dumps({'1': issue}))
            env = {'PATH': '', 'HOME': str(root), 'TASKQ_HOST': 'win',
                   'PYTHONIOENCODING': 'cp1252' if os.name == 'nt' else 'utf-8'}
            if os.name == 'nt':
                env['SYSTEMROOT'] = os.environ['SYSTEMROOT']
            for command, code, stream, expected in (('status', 0, 'stdout', 'Привет 🌍'), ('💥', 2, 'stderr', '💥')):
                with self.subTest(command=command):
                    done = REAL_RUN([sys.executable, str(root / 'taskq.py'), command], cwd=root, env=env,
                                    capture_output=True, timeout=20)
                    self.assertEqual(done.returncode, code, done.stderr)
                    self.assertIn(expected, getattr(done, stream).decode('utf-8'))
                    self.assertNotIn(b'UnicodeEncodeError', done.stderr)

    def test_local_scope_survives_native_event_and_turn_end_children(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            # The completion marker is after the actual child's main(), not a sleep or spawn-return oracle.
            source = (ROOT / 'taskq.py').read_text('utf-8')
            (root / 'taskq.py').write_text(source + "\nif __name__ == '__main__' and sys.argv[1] == 'tick':\n"
                                         "    (Path.cwd() / 'completed').write_text('done')\n", encoding='utf-8')
            (root / 'board.py').write_text(FILE_BOARD, encoding='utf-8')
            (root / 'fake.py').write_text('''import json, os, pathlib
def spawn(name, prompt, cwd):
    with pathlib.Path(__file__).with_name('spawned').open('a') as out:
        out.write(json.dumps([name, os.environ.get('TASKQ_HOST_ONLY'), os.environ.get('TASKQ_LIMITS')]) + '\\n')
    return 's-' + name.split()[0]
def send(session, text): return session
def alive(session): return True
def link(session): return None
''', encoding='utf-8')
            config = json.dumps({'board': 'board.py', 'update': False, 'runtimes': {'fake': 'fake.py', 'codex': 'fake.py'},
                                 'limits': {'fake': 8, 'codex': 1}})
            (root / 'taskq.json').write_text(config)
            issues = {}
            for n, labels in enumerate((['host-win'], ['host-win'], ['host-win'], [], ['host-mac'],
                                       ['host-win', 'run-fake'], ['host-win', 'host-mac']), 1):
                issues[str(n)] = {'iid': n, 'title': f'T{n}', 'body': taskq.block('g', {'event_schema': 1, 'deps': [], 'pm':
                                 {'runtime': 'fake', 'session': SESSION, 'name': 'win'}}), 'labels': ['q-ready', 'priority-2', *labels],
                                 'state': 'open', 'listed': True, 'comments': [],
                                 'updated_at': '2026-10-09T00:00:00Z', 'url': ''}
            (root / 'issues.json').write_text(json.dumps(issues))
            env = {'PATH': '', 'HOME': str(root), 'TASKQ_HOST': 'win', 'TASKQ_RUNTIME': 'fake',
                   'TASKQ_HOST_ONLY': 'win', 'TASKQ_LIMITS': '{"codex":2}'}
            if os.name == 'nt':
                env['SYSTEMROOT'] = os.environ['SYSTEMROOT']

            def invoke(*args):
                done = REAL_RUN([sys.executable, str(root / 'taskq.py'), *args], cwd=root, env=env,
                                capture_output=True, text=True, timeout=20)
                self.assertEqual(done.returncode, 0, done.stderr)

            def completed():
                end = taskq.time.monotonic() + 20
                while not (root / 'completed').exists() and taskq.time.monotonic() < end:
                    taskq.time.sleep(0.05)
                self.assertTrue((root / 'completed').exists(), (root / '.taskq' / 'dispatch.log').read_text())
                (root / 'completed').unlink()

            invoke('add', 'event', '--goal', 'g', '--acceptance', 'a', '--host', 'win')
            completed()
            saved = json.loads((root / 'issues.json').read_text())
            self.assertEqual([taskq.parse(saved[str(n)])['state'] for n in range(1, 9)], ['doing'] * 2 + ['ready'] * 6)
            for n in range(3, 8):
                self.assertEqual(saved[str(n)], issues[str(n)])
            invoke('run', '1')  # the real detached event now admits the first reserved worker
            completed()
            self.assertEqual(taskq.parse(json.loads((root / 'issues.json').read_text())['1'])['claim']['session'], 's-T1')
            # A real turn-end child takes --after, uses the same inherited scope, and admits the second order.
            saved = json.loads((root / 'issues.json').read_text())
            parsed = taskq.parse(saved['2'])
            saved['2']['body'] = taskq.block('g', {**parsed['raw'], 'order': 'run'})
            (root / 'issues.json').write_text(json.dumps(saved))
            launcher = "import taskq; taskq.CONFIG=taskq.load_config(); taskq.dispatch('turn end', [], after=2147483647, after_birth=taskq.process_domain()+':1'+(':0' if taskq.sys.platform=='darwin' else ''))"
            done = REAL_RUN([sys.executable, '-c', launcher], cwd=root, env=env, capture_output=True, text=True, timeout=20)
            self.assertEqual(done.returncode, 0, done.stderr)
            completed()
            spawned = [json.loads(line) for line in (root / 'spawned').read_text().splitlines()]
            self.assertEqual([entry[0].split()[0] for entry in spawned], ['S1', 'S2', 'T1', 'T2'])
            self.assertTrue(all(entry[1:] == ['win', '{"codex":2}'] for entry in spawned))
            self.assertEqual((root / 'taskq.json').read_text(), config)

    def test_assignee_only_cli_board_runtime_boundary(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / 'taskq.py').write_text((ROOT / 'taskq.py').read_text())  # no real clone refresh
            (root / 'board.py').write_text(FILE_BOARD + "\ndef user(): return pathlib.Path(__file__).with_name('identity').read_text()\n")
            (root / 'fake.py').write_text(FILE_RUNTIME)
            (root / 'taskq.json').write_text(json.dumps({'board': 'board.py', 'runtimes': {'fake': 'fake.py'},
                                                       'limits': {'fake': 1}, 'assignee': 'alice'}))
            env = {'PATH': '', 'HOME': str(root), 'TASKQ_HOST': 'mac', 'TASKQ_RUNTIME': 'claude',
                   'CLAUDE_CODE_SESSION_ID': SESSION}
            for boundary in ('tick', 'take'):
                for people, identity, allowed in ((['alice'], 'alice', True), (['bob', 'alice'], 'alice', True),
                                                  (['alice'], 'bob', False), ([], 'alice', False)):
                    with self.subTest(boundary=boundary, people=people, identity=identity):
                        (root / 'identity').write_text(identity)
                        (root / 'spawned').unlink(missing_ok=True)
                        issue = {'iid': 1, 'title': 'T', 'body': taskq.block('g', {'event_schema': 1, 'deps': [], 'pm':
                                 {'runtime': 'fake', 'session': SESSION, 'name': 'mac'}}), 'labels': ['q-ready', 'assignee-only'],
                                 'assignees': people, 'state': 'open', 'listed': True, 'comments': [],
                                 'updated_at': '2026-10-09T00:00:00Z', 'url': ''}
                        (root / 'issues.json').write_text(json.dumps({'1': issue}))
                        done = REAL_RUN([sys.executable, str(root / 'taskq.py'), boundary] + (['1'] if boundary == 'take' else []),
                                        cwd=root, env=env, capture_output=True, text=True, timeout=20)
                        self.assertEqual(done.returncode, 0 if allowed or boundary == 'tick' else 1, done.stderr)
                        saved = taskq.parse(json.loads((root / 'issues.json').read_text())['1'])
                        self.assertEqual(saved['state'], 'doing' if allowed else 'ready')
                        self.assertEqual((root / 'spawned').exists(), allowed and boundary == 'tick')
                        if allowed and boundary == 'tick':
                            self.assertEqual((root / 'spawned').read_text(), 'S1 UNK T (mac)')  # ORCH of the pm's runtime (#572)
                            self.assertEqual(saved['supervisor']['session'], 's1')
                        if not allowed and (boundary == 'take' or 'alice' in people):  # else the `assignee` filter skips it first
                            self.assertIn('assignee-only', done.stderr)
                            self.assertEqual(saved['claim'], None)

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


class HermesNativeBoundary(unittest.TestCase):
    """Real owner/stdin boundary, protocol-accurate fake downstream; optional real Hermes."""
    def setUp(self):
        artifacts = ROOT / '.taskq' / 'native-tests'
        artifacts.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix='tq-h-', dir=str(artifacts))
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.runtime = taskq.load_file('runtimes/hermes.py', ROOT)
        self.runtime.folder = lambda: self.root / '.taskq'
        self.runtime.host = lambda: taskq
        self.addCleanup(self.shutdown)

    def shutdown(self):
        # Explicit teardown of only this fixture's verified owners, including failed setup.
        for endpoint in self.runtime.folder().glob('*.ipc'):
            owner = json.loads((endpoint / 'owner.json').read_text())
            self.runtime.request(endpoint, '_stop', expected=owner)
            import time
            end = time.monotonic() + 8
            while self.runtime.birth(owner['pid']) == owner['birth'] and time.monotonic() < end:
                time.sleep(.02)
            self.assertNotEqual(self.runtime.birth(owner['pid']), owner['birth'], 'fixture owner survived teardown')
            self.assertIsNone(self.runtime.birth(owner['gateway']['pid']), 'gateway survived teardown')

    def fake(self, mode='normal'):
        gateway = self.root / 'gateway.py'
        gateway.write_text(HERMES_TUI_FIXTURE)
        return mock.patch.dict(os.environ, {'HERMES_HOME': str(self.root), 'FAKE_HERMES_MODE': mode,
            'TASKQ_HERMES_COMMAND': json.dumps([sys.executable, str(gateway)])})

    def exercise(self):
        prompt = 'Compute 17 + 25. Reply with decimal answer only. Do not use tools.'
        sid = self.runtime.spawn('S1 HRM isolated (test)', prompt, self.root)
        self.assertNotEqual(sid, self.runtime.data_for(sid)['runtime_id'])
        payload = self.runtime.completed(sid)
        self.assertEqual(payload['text'].strip(), '42')
        self.assertNotIn('42', prompt)  # proof cannot come from finding expected text in a user prompt
        receipt = payload['persisted_turn']
        self.assertNotEqual(receipt['user_row_id'], receipt['final_assistant_row_id'])
        self.assertEqual(self.runtime.state(sid), 'idle')
        self.assertTrue(self.runtime.alive(sid))  # idle worker is not dead
        self.assertEqual(self.runtime.wake_manager(sid, 'Compute 19 + 23. Decimal answer only. No tools.'), sid)
        data = self.runtime.data_for(sid)
        events = [json.loads(line) for line in Path(data['endpoint']).with_suffix('.events.jsonl').read_text().splitlines()]
        self.assertEqual(len({event['submit'] for event in events if event['event']['type'] == 'message.complete'}), 2)
        self.runtime.retire(lambda *args: True)
        self.assertEqual(self.runtime.state(sid), 'dead')
        self.assertFalse(list(self.runtime.folder().glob('h-*.handle.json')))
        self.assertIsNone(self.runtime.birth(data['owner']['pid']))
        self.assertIsNone(self.runtime.birth(data['owner']['gateway']['pid']))
        self.assertTrue(Path(data['endpoint']).with_suffix('.exit.json').exists())

    def test_real_stdio_model_events_and_retirement(self):
        with self.fake():
            self.exercise()

    def test_prompt_echo_wrong_event_and_failed_completion_never_pass(self):
        for mode in ('echo_only', 'wrong_session', 'error_complete', 'wrong_receipt'):
            with self.subTest(mode=mode), self.fake(mode):
                sid = self.runtime.spawn('S1 HRM isolated (test)', 'Say 42; user text contains 42.', self.root)
                with self.assertRaises((ValueError, TimeoutError)):
                    self.runtime.completed(sid, timeout=.3)
                # Reading an echoed prompt is insufficient even when it contains the answer.
                history = self.runtime.call(sid, 'session.history')['messages']
                self.assertIn('42', history[0]['text'])
                self.runtime.stop(sid)

    def test_identical_prompt_replay_cannot_consume_wait(self):
        with self.fake('replay_identical_delayed'):
            sid = self.runtime.spawn('PM HRM isolated (test)', 'ask #1', self.root)
            first = self.runtime.completed(sid)['persisted_turn']
            previous_token = self.runtime.data_for(sid)['turn']
            completed = self.runtime.completed

            def short_completion(session, timeout=None):
                self.assertNotEqual(self.runtime.data_for(session)['turn'], previous_token)
                try:
                    return completed(session, timeout=.3 if timeout is None else timeout)
                except TimeoutError as error:
                    self.assertEqual(str(error), 'Hermes message.complete/idle proof deadline exceeded')
                    raise

            board = FakeBoard()
            raw = {'pm': {'runtime': 'hermes', 'session': sid, 'name': 'test'}}
            board.add('Need owner', '<!-- taskq:start -->\n```json\n' + json.dumps(raw) + '\n```\n<!-- taskq:end -->', ['q-ask'])
            receipt = self.root / '.taskq' / f'wait-{sid}.json'
            with mock.patch.object(taskq, 'BOARD', board), \
                    mock.patch.object(taskq, 'CONFIG', {'root': self.root, 'board': 'board.py', 'publish': 'direct', 'hosts': {}}), \
                    mock.patch.object(taskq, 'CLONE', self.root), \
                    mock.patch.object(taskq, 'runtimes', return_value={'hermes': taskq.Hermes(self.runtime)}), \
                    mock.patch.object(self.runtime, 'completed', side_effect=short_completion) as completion_wait, \
                    mock.patch.dict(os.environ, {'TASKQ_RUNTIME': 'hermes', 'HERMES_SESSION_ID': sid, 'TASKQ_HOST': 'test'}):
                with self.assertRaisesRegex(SystemExit, r'^taskq: Hermes manager wake failed \(TimeoutError\); event receipt unchanged$'), contextlib.redirect_stdout(io.StringIO()):
                    taskq.main(['wait', '--window', '0'])
                completion_wait.assert_called_once_with(sid)  # admission finished; only proof timed out
                self.assertTrue((self.root / 'admission-delayed').exists())
                self.assertFalse(receipt.exists(), 'old completion consumed the board event')
                data = self.runtime.data_for(sid)
                self.assertNotEqual(data['turn'], previous_token)
                turn = self.runtime.call(sid, '_turn', turn=data['turn'])
                self.assertTrue(turn['started'])
                self.assertIsNone(turn['complete'])
                self.assertEqual(turn['boundary'], first['final_assistant_row_id'])
                events = [json.loads(line) for line in Path(data['endpoint']).with_suffix('.events.jsonl').read_text().splitlines()]
                replayed = [row['event']['payload']['persisted_turn'] for row in events
                            if row['submit'] == data['turn'] and row['event']['type'] == 'message.complete']
                self.assertEqual(replayed, [first], 'test must reject the actual old receipt under the new token')
                (self.root / 'release-real').touch()
                self.runtime.call(sid, 'session.history')  # fixture releases delayed native event on next RPC
                actual = self.runtime.completed(sid, timeout=1)['persisted_turn']
                self.assertGreater(actual['user_row_id'], first['final_assistant_row_id'])
                board.release(board.guard)  # the delayed turn is now complete: explicit fixture recovery of its exact grant
                with contextlib.redirect_stdout(io.StringIO()):
                    taskq.main(['wait', '--window', '0'])
                self.assertEqual(json.loads(receipt.read_text()), {'1': 'ask'})
            self.runtime.stop(sid)

    def test_admission_timeout_reproduces_stale_handle_token(self):
        with self.fake('replay_identical_held'):
            sid = self.runtime.spawn('PM HRM isolated (test)', 'ask #1', self.root)
            self.runtime.completed(sid)
            previous_token = self.runtime.data_for(sid)['turn']
            request = self.runtime.request
            clock = self.runtime.time.monotonic
            marker = self.root / 'admission-delayed'

            def expire_admission(path, method, params=None, expected=None):
                if method != '_submit':
                    return request(path, method, params, expected)
                start = clock()

                def admission_clock():
                    if clock() - start > 5:
                        self.fail('fixture never reached native prompt admission')
                    # Expire only after the owner replaced its token, before the delayed reply.
                    return start + self.runtime.WAIT + 1 if marker.exists() else start
                with mock.patch.object(self.runtime.time, 'monotonic', side_effect=admission_clock):
                    return request(path, method, params, expected)

            with mock.patch.object(self.runtime, 'request', side_effect=expire_admission):
                with self.assertRaisesRegex(TimeoutError, '^Hermes RPC deadline exceeded; delivery unknown$'):
                    self.runtime.send(sid, 'ask #1')
            (self.root / 'release-admission').touch()
            self.assertEqual(self.runtime.data_for(sid)['turn'], previous_token)
            with self.assertRaisesRegex(ValueError, '^Hermes RPC failed: KeyError$'):
                self.runtime.call(sid, '_turn', turn=previous_token)
            self.runtime.stop(sid)

    def test_missing_persisted_history_boundary_refuses_submit(self):
        with self.fake('missing_history'):
            with self.assertRaisesRegex(ValueError, 'history boundary unavailable'):
                self.runtime.spawn('S1 HRM isolated (test)', 'unused', self.root)
        self.assertNotIn('prompt.submit', json.loads((self.root / 'calls.json').read_text()))

    def test_unpersisted_title_never_resumes_or_submits(self):
        with self.fake('unpersisted'):
            with self.assertRaisesRegex(taskq.Unnamed, 'persistence not confirmed'):
                self.runtime.spawn('S1 HRM isolated (test)', 'unused', self.root)
        self.assertFalse(list(self.runtime.folder().glob('*.ipc')))
        calls = json.loads((self.root / 'calls.json').read_text())
        self.assertNotIn('session.resume', calls)
        self.assertNotIn('prompt.submit', calls)
        self.runtime.retire(lambda *args: True)

    def test_busy_at_owner_admission_is_refused(self):
        with self.fake('busy_at_submit'):
            with self.assertRaisesRegex(ValueError, 'busy/unknown'):
                self.runtime.spawn('S1 HRM isolated (test)', 'unused', self.root)
        self.assertNotIn('prompt.submit', json.loads((self.root / 'calls.json').read_text()))
        self.assertFalse(list(self.runtime.folder().glob('*.ipc')))

    def test_unknown_structured_state_and_owner_reuse_fail_closed(self):
        for snapshot in ({'output': 'Agent Running: No'}, {'running': False},
                         {'session_id': 'live', 'session_key': 'stored', 'running': False, 'hydrating': True},
                         {'session_id': 'other', 'session_key': 'stored', 'running': False}):
            self.assertIsNone(self.runtime.snapshot_state(snapshot, 'stored', 'live'))
        self.assertEqual(self.runtime.snapshot_state({'session_id': 'live', 'session_key': 'stored', 'running': False}, 'stored', 'live'), 'idle')
        endpoint = self.root / 'reused.ipc'
        endpoint.mkdir()
        owner = {'pid': os.getpid(), 'birth': 'not-the-current-birth', 'nonce': 'old'}
        (endpoint / 'owner.json').write_text(json.dumps(owner))
        with self.assertRaisesRegex(ConnectionRefusedError, 'identity changed'):
            self.runtime.request(endpoint, '_stop', expected=owner)
        self.assertEqual(sorted(path.name for path in endpoint.iterdir()), ['owner.json'])
        self.assertTrue(self.runtime.birth(os.getpid()))

    def test_owner_sigterm_kills_gateway_children_and_keeps_evidence(self):
        with self.fake('child'):
            sid = self.runtime.spawn('S1 HRM isolated (test)', 'Compute 17 + 25.', self.root)
            self.runtime.completed(sid)
            data = self.runtime.data_for(sid)
            fd = self.runtime.owner_fd(data['owner'])
            try:
                self.runtime.signal.pidfd_send_signal(fd, self.runtime.signal.SIGTERM)
            finally:
                os.close(fd)
            import time
            deadline = time.monotonic() + 8
            while self.runtime.birth(data['owner']['pid']) == data['owner']['birth'] and time.monotonic() < deadline:
                time.sleep(.02)
            self.assertEqual(self.runtime.state(sid), 'dead')
            self.assertIsNone(self.runtime.birth(data['owner']['gateway']['pid']))
            child = json.loads((self.root / 'child.json').read_text())
            self.assertNotEqual(self.runtime.birth(child['pid']), child['birth'])
            self.assertTrue(Path(data['endpoint']).with_suffix('.events.jsonl').exists())
            self.assertTrue(Path(data['endpoint']).with_suffix('.exit.json').exists())
            self.runtime.retire(lambda *args: True)

    def test_current_wait_requires_exact_native_completion_before_receipt(self):
        with self.fake():
            sid = self.runtime.spawn('PM HRM isolated (test)', 'Compute 17 + 25.', self.root)
            self.runtime.completed(sid)
            board = FakeBoard()
            raw = {'pm': {'runtime': 'hermes', 'session': sid, 'name': 'test'}}
            board.add('Need owner', '<!-- taskq:start -->\n```json\n' + json.dumps(raw) + '\n```\n<!-- taskq:end -->', ['q-ask'])
            config = {'root': self.root, 'board': 'board.py', 'publish': 'direct', 'hosts': {}}
            env = {'TASKQ_RUNTIME': 'hermes', 'HERMES_SESSION_ID': sid, 'TASKQ_HOST': 'test'}
            with mock.patch.object(taskq, 'BOARD', board), mock.patch.object(taskq, 'CONFIG', config), \
                    mock.patch.object(taskq, 'CLONE', self.root), \
                    mock.patch.object(taskq, 'runtimes', return_value={'hermes': taskq.Hermes(self.runtime)}), \
                    mock.patch.dict(os.environ, env):
                out = io.StringIO()
                with contextlib.redirect_stdout(out):
                    taskq.main(['wait', '--window', '0'])
                self.assertEqual(out.getvalue(), 'ask #1\n')
                data = self.runtime.data_for(sid)
                turn = self.runtime.call(sid, '_turn', turn=data['turn'])
                self.assertEqual(turn['prompt'], 'ask #1')
                self.assertTrue(turn['started'])
                self.assertEqual(turn['complete']['status'], 'complete')
                self.assertTrue(turn['complete']['persisted_turn']['complete'])
                receipt = self.root / '.taskq' / f'wait-{sid}.json'
                self.assertEqual(json.loads(receipt.read_text()), {'1': 'ask'})
                with contextlib.redirect_stdout(io.StringIO()):
                    taskq.main(['wait', '--window', '0'])
                self.assertEqual(self.runtime.data_for(sid)['turn'], data['turn'])
            self.runtime.stop(sid)

    def test_installed_hermes_isolated(self):
        home, command = os.environ.get('TASKQ_HERMES_TEST_HOME'), os.environ.get('TASKQ_HERMES_COMMAND')
        if not home or not command:
            self.skipTest('BLOCKER: explicitly supply pre-provisioned isolated TASKQ_HERMES_TEST_HOME and TASKQ_HERMES_COMMAND; no credentials copied')
        home = Path(home).resolve()
        if home == (Path.home() / '.hermes').resolve() or not (home / 'config.yaml').is_file():
            self.skipTest('BLOCKER: isolated config.yaml unavailable or default home refused')
        # Named profiles may use Hermes's root pool fallback without local auth files.
        # Explicit home/command opt in; the actual protocol/model boundary proves auth.
        with mock.patch.dict(os.environ, {'HERMES_HOME': str(home)}):
            self.exercise()


class HermesPilotLocal(unittest.TestCase):
    """No models/auth: real disposable Git/file-board boundary and profile admission."""
    def setUp(self):
        artifacts = ROOT / '.taskq' / 'pilot-tests'
        artifacts.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=artifacts)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.pilot = taskq.load_file('runtimes/hermes_pilot.py', ROOT)

    def test_repeated_runs_have_distinct_native_names_and_hosts(self):
        item = {'iid': 1, 'title': 'Same research task', 'pm': {'runtime': 'hermes'}}
        names = []
        for suffix in ('0123456789ab', 'fedcba987654'):
            identity = self.pilot.run_identity(self.root / ('hermes-pilot-' + suffix))
            self.assertEqual(identity['host'], 'local-pilot-' + suffix)
            self.assertEqual(identity['manager_name'], f"PM HRM local pilot ({identity['host']})")
            with mock.patch.dict(os.environ, {'TASKQ_HOST': identity['host']}), \
                    mock.patch.object(taskq, 'CONFIG', {'hosts': {}}):
                supervisor = taskq.worker_name(item, 'S')
                worker = taskq.worker_name(item, 'T')
            self.assertEqual(supervisor, f"S1 HRM Same research task ({identity['host']})")
            self.assertEqual(worker, f"T1 HRM Same research task ({identity['host']})")
            names.append((identity['manager_name'], supervisor, worker))
        for first, second in zip(*names):
            self.assertNotEqual(first, second)

    def test_unnamed_evidence_retains_safe_title_code_without_provider_body(self):
        error = taskq.Unnamed('native-session', ValueError('Hermes RPC failed: Hermes session.title refused (code 4022)'))
        evidence_reason = self.pilot.topology_failure(error)
        self.assertIn('Unnamed: session.title code 4022', evidence_reason)
        self.assertIn('fresh pilot fixture', evidence_reason)
        self.assertNotIn('native-session', evidence_reason)
        unknown = taskq.Unnamed('native-session', ValueError('provider secret body'))
        self.assertNotIn('provider secret body', self.pilot.topology_failure(unknown))
        self.assertIn('exception body withheld', self.pilot.topology_failure(unknown))
        pending = taskq.Unnamed('native-session', ValueError('Hermes stored row/native title persistence not confirmed'))
        self.assertIn('persistence not confirmed', self.pilot.topology_failure(pending))

    def test_supervisor_must_be_distinct_from_manager_and_ids_are_recorded(self):
        evidence = {}
        with self.assertRaisesRegex(ValueError, 'SID equals manager SID'):
            self.pilot.record_supervisor(evidence, 'manager', 'manager')
        self.assertEqual(evidence, {})
        self.pilot.record_supervisor(evidence, 'manager', 'supervisor')
        self.assertEqual(evidence['sessions'], {'manager': 'manager', 'supervisor': 'supervisor'})

    def test_provenance_includes_harness_and_rejects_changes(self):
        before = self.pilot.provenance()
        self.assertIn('runtimes/hermes_pilot.py', before['sha256'])
        self.assertIn('tests/test_single.py', before['sha256'])
        self.assertIn('taskq.md', before['sha256'])
        self.assertRegex(before['head'], r'^[0-9a-f]{40}$')
        self.pilot.unchanged(before)
        changed = json.loads(json.dumps(before))
        changed['sha256']['runtimes/hermes_pilot.py'] = 'changed'
        with mock.patch.object(self.pilot, 'provenance', return_value=changed):
            with self.assertRaisesRegex(ValueError, 'changed during pilot'):
                self.pilot.unchanged(before)

    def test_calculation_contract_prefix_and_codex_wrapper_are_exact(self):
        supervisor = 'export TASKQ_TASK=1 TASKQ_RUNTIME=hermes && python3 -c "print(17 + 25)"'
        worker = 'export TASKQ_TASK=1 TASKQ_RUNTIME=codex && python3 -c "print(17 + 25)"'
        for command in (supervisor, worker, "/bin/bash -lc '" + worker + "'"):
            with self.subTest(command=command):
                self.assertTrue(self.pilot.calculation_command(command))
        # Actual pilot worker combined calculation and SHA lookup: deliberately reject it.
        recorded_worker = "/bin/bash -lc '" + worker + " && git rev-parse origin/main'"
        for command in (recorded_worker, supervisor + '; echo 42', supervisor + ' && echo 42',
                        supervisor.replace('python3 -c "print(17 + 25)"', 'echo 42'),
                        supervisor.replace('TASKQ_TASK=1', 'TASKQ_TASK=2'),
                        supervisor.replace('TASKQ_RUNTIME=hermes', 'TASKQ_RUNTIME=other'),
                        supervisor.replace(' && ', ' ; '),
                        supervisor.replace(' && ', ' EXTRA=spoof && ')):
            with self.subTest(command=command):
                self.assertFalse(self.pilot.calculation_command(command))

    def test_independent_review_requires_successful_tool_in_exact_turn(self):
        args = {'command': 'export TASKQ_TASK=1 TASKQ_RUNTIME=hermes && python3 -c "print(17 + 25)"'}
        tool = {'role': 'tool', 'name': 'terminal', 'tool_call_id': 'calc', 'args': args}
        rows = [{'role': 'user', 'row_id': 10, 'text': 'review #1'}, tool,
                {'role': 'assistant', 'row_id': 14, 'text': 'closed: 42'}]
        receipt = {'user_row_id': 10, 'final_assistant_row_id': 14}
        start = {'tool_id': 'calc', 'name': 'terminal', 'args': args}
        complete = {**start, 'result': {'output': '42\n', 'exit_code': 0, 'error': None}}
        turn = {'tools': [{'type': 'tool.start', 'payload': start}, {'type': 'tool.complete', 'payload': complete}]}
        proof = self.pilot.supervisor_calculation({'messages': rows}, turn, receipt)
        self.assertEqual(proof['tool_id'], 'calc')
        for history, events in ((rows, {'tools': []}), (rows[:1]+rows[2:], turn),
                                ([tool]+rows[:1]+rows[2:], turn), (rows, {'tools': turn['tools'][1:]})):
            with self.assertRaisesRegex(ValueError, 'lacks successful calculation'):
                self.pilot.supervisor_calculation({'messages': history}, events, receipt)
        self.assertFalse(self.pilot.calculation_command('echo \"print(17 + 25)\"; echo 42'))
        complete['result']['exit_code'] = 1
        with self.assertRaises(ValueError):
            self.pilot.supervisor_calculation({'messages': rows}, turn, receipt)

    def test_worker_echo_is_not_calculation_evidence(self):
        path = self.root / 'T1.log'
        events = [{'type': 'thread.started', 'thread_id': 'worker'},
                  {'type': 'item.completed', 'item': {'type': 'agent_message', 'text': '42'}}]
        path.write_text('\n'.join(map(json.dumps, events)))
        with self.assertRaises(ValueError):
            self.pilot.worker_calculation(path, 'worker')
        events.append({'type': 'item.completed', 'item': {'id': 'calc', 'type': 'command_execution',
                      'command': '/bin/bash -lc \'export TASKQ_TASK=1 TASKQ_RUNTIME=codex && python3 -c "print(17 + 25)"\'', 'status': 'completed', 'exit_code': 0, 'aggregated_output': '42\n'}})
        path.write_text('\n'.join(map(json.dumps, events)))
        self.assertEqual(self.pilot.worker_calculation(path, 'worker')['item_id'], 'calc')
        with self.assertRaises(ValueError):
            self.pilot.worker_calculation(path, 'other-worker')

    def args(self, hermes, codex):
        return taskq.argparse.Namespace(hermes_home=str(hermes), codex_home=str(codex),
            hermes_command=json.dumps([sys.executable, '-m', 'tui_gateway.entry']), run=True, auth_ready=False)

    def test_named_profile_without_auth_file_or_env_is_accepted_default_is_refused(self):
        project = self.root / 'project'
        project.mkdir()
        profile = self.root / 'global' / 'profiles' / 'taskqpilot20261009'
        profile.mkdir(parents=True)
        (profile / 'config.yaml').write_text('model: fixture\n')
        codex = project / 'codex'
        codex.mkdir()
        (codex / 'config.toml').write_text('')
        args = self.args(profile, codex)
        with mock.patch.object(self.pilot, 'ROOT', project), \
                mock.patch.object(self.pilot.shutil, 'which', return_value='/fixture/codex'), \
                mock.patch.dict(os.environ, {}, clear=True):
            self.assertEqual(self.pilot.preflight(args), [])
            args.hermes_home = str(Path.home() / '.hermes')
            self.assertTrue(any('default home refused' in item for item in self.pilot.preflight(args)))
        self.assertEqual(sorted(path.name for path in profile.iterdir()), ['config.yaml'])
        self.assertFalse((codex / 'auth.json').exists())  # runtime, not file existence, determines auth

    def test_existing_default_codex_home_only_when_explicit_no_auth_or_config_writes(self):
        project = self.root / 'project'
        project.mkdir()
        user = self.root / 'user'
        profile = user / '.hermes' / 'profiles' / 'pilot'
        profile.mkdir(parents=True)
        (profile / 'config.yaml').write_text('model: fixture\n')
        codex = user / '.codex'
        codex.mkdir()
        config = codex / 'config.toml'
        config.write_text('# existing fixture config\n')
        before = config.read_bytes()
        args = self.args(profile, codex)
        with mock.patch.object(self.pilot, 'ROOT', project), mock.patch.object(Path, 'home', return_value=user), \
                mock.patch.object(self.pilot.shutil, 'which', return_value='/fixture/codex'), \
                mock.patch.dict(os.environ, {'HOME': str(user), 'CODEX_HOME': str(codex)}, clear=True):
            self.assertEqual(self.pilot.preflight(args), [])
            args.codex_home = '~/.codex'
            self.assertEqual(self.pilot.preflight(args), [])
            args.codex_home = None
            self.assertTrue(any('Codex: explicitly supplied' in item for item in self.pilot.preflight(args)))
            args.codex_home = str(codex)
            args.hermes_home = str(user / '.hermes')
            self.assertTrue(any('default home refused' in item for item in self.pilot.preflight(args)))
        self.assertEqual(config.read_bytes(), before)
        self.assertEqual(sorted(path.name for path in codex.iterdir()), ['config.toml'])

    def test_optional_hermes_named_profile_has_no_local_auth_assertion_gate(self):
        profile = self.root / 'profiles' / 'pilot'
        profile.mkdir(parents=True)
        (profile / 'config.yaml').write_text('model: fixture\n')
        check = HermesNativeBoundary('test_installed_hermes_isolated')
        # This tests opt-in selection only. No gateway/model/auth is mocked as successful.
        def selected():
            self.assertEqual(os.environ['HERMES_HOME'], str(profile.resolve()))
            self.assertNotIn('TASKQ_HERMES_TEST_AUTH_READY', os.environ)
        check.exercise = mock.Mock(side_effect=selected)
        with mock.patch.dict(os.environ, {'TASKQ_HERMES_TEST_HOME': str(profile),
                'TASKQ_HERMES_COMMAND': json.dumps([sys.executable, '-m', 'tui_gateway.entry'])}, clear=True):
            check.test_installed_hermes_isolated()
        check.exercise.assert_called_once_with()
        self.assertEqual(sorted(path.name for path in profile.iterdir()), ['config.yaml'])

    def test_disposable_origin_genuine_research_close_without_push(self):
        (self.root / 'board.py').write_text(self.pilot.FILE_BOARD)
        (self.root / 'issues.json').write_text('{}')
        config, seed = self.pilot.prepare(self.root)
        instructions = (self.root / 'AGENTS.md').read_text()
        self.assertIn('BOTH worker and supervisor: calculation must be its own terminal tool call, containing only', instructions)
        for runtime in ('codex', 'hermes'):
            self.assertIn(f'export TASKQ_TASK=1 TASKQ_RUNTIME={runtime} && python3 -c "print(17 + 25)"', instructions)
        self.assertIn('Do not chain any other commands onto the calculation.', instructions)
        self.assertIn('in the next separate terminal tool call, using its required TaskQ export prefix.', instructions)
        git = taskq.shutil.which('git')
        def read(*argv):
            return REAL_RUN([git, '-C', str(self.root), *argv], capture_output=True, text=True, timeout=20)
        self.assertEqual(read('remote', 'get-url', 'origin').stdout.strip(), str(self.root / 'origin.git'))
        self.assertEqual(read('rev-parse', 'origin/main').stdout.strip(), seed)
        bare_before = REAL_RUN([git, '--git-dir', str(self.root / 'origin.git'), 'show-ref'], capture_output=True, text=True, timeout=20).stdout
        local = taskq.load_file('taskq.py', self.root)
        local.CONFIG, local.BOARD = config, local.make_board(config)
        raw = {'claim': {'runtime': 'codex', 'session': 'fixture-worker', 'name': 'local-pilot'},
               'supervisor': {'runtime': 'hermes', 'session': 'fixture-supervisor', 'name': 'local-pilot'},
               'pm': {'runtime': 'hermes', 'session': 'fixture-manager', 'name': 'local-pilot'},
               'result': {'sha': seed, 'checks': 'local research answer'}}
        body = '<!-- taskq:start -->\n```json\n' + json.dumps(raw) + '\n```\n<!-- taskq:end -->'
        n = local.BOARD.add('Research', body, ['q-review', 'research', 'run-codex'])
        local.BOARD.comment(n, '**result** · codex:fixture-\n\n42')
        env = {'PATH': str(self.root / 'bin') + os.pathsep + os.environ.get('PATH', os.defpath),
               'TASKQ_RUNTIME': 'hermes', 'HERMES_SESSION_ID': 'fixture-supervisor', 'TASKQ_HOST': 'local-pilot',
               'GIT_CONFIG_GLOBAL': os.devnull, 'GIT_CONFIG_NOSYSTEM': '1', 'GIT_ALLOW_PROTOCOL': 'file'}
        with mock.patch.dict(os.environ, env, clear=True), mock.patch.object(local, 'runtimes', return_value={}), \
                mock.patch.object(local, 'dispatch') as dispatch, contextlib.redirect_stdout(io.StringIO()):
            local.main(['close', '1', '--text', 'Independently checked 42; research answered; open: none'])
        issue = local.BOARD.get(1)
        self.assertEqual(issue['state'], 'closed')
        self.assertIn('**close** · hermes:fixture-', issue['comments'][-1])
        self.assertEqual(read('rev-parse', 'HEAD').stdout.strip(), seed)
        bare_after = REAL_RUN([git, '--git-dir', str(self.root / 'origin.git'), 'show-ref'], capture_output=True, text=True, timeout=20).stdout
        self.assertEqual(bare_after, bare_before)  # close fetched, never published or pushed
        self.assertTrue((self.root / '.git' / 'FETCH_HEAD').is_file())
        remote = REAL_RUN([str(self.root / 'bin/git'), 'fetch', 'https://invalid.example/taskq.git'],
                          cwd=self.root, env=env, capture_output=True, text=True, timeout=20)
        self.assertNotEqual(remote.returncode, 0)
        self.assertIn("transport 'https' not allowed", remote.stderr)
        for command in ([str(self.root / 'bin/git'), 'push', 'origin', 'HEAD:main'],
                        [str(self.root / 'bin/gh'), 'issue', 'list']):
            done = REAL_RUN(command, cwd=self.root, env=env, capture_output=True, text=True, timeout=20)
            self.assertNotEqual(done.returncode, 0)
            self.assertIn('LOCAL-ONLY', done.stderr)



HERMES_TUI_FIXTURE = r'''import json, os, pathlib, subprocess, sys, time
sid, live, title, persisted, rows, activations = 'stored-native-id', 'live-process-id', '', False, [], 0
mode = os.environ.get('FAKE_HERMES_MODE', 'normal')
calls = []
previous, pending = None, None
if mode == 'child':
    child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(120)'], start_new_session=True)
    fields = pathlib.Path(f'/proc/{child.pid}/stat').read_text().rpartition(')')[2].split()
    pathlib.Path('child.json').write_text(json.dumps({'pid': child.pid, 'birth': fields[19]}))
def emit(kind, payload=None, target=live):
    print(json.dumps({'jsonrpc': '2.0', 'method': 'event', 'params': {'type': kind, 'session_id': target, 'payload': payload or {}}}), flush=True)
def snapshot():
    return {'session_id': live, 'session_key': sid, 'running': False, 'status': 'idle', 'messages': [], 'info': {}, 'message_count': len(rows)}
for line in sys.stdin:
    req = json.loads(line); p, method = req['params'], req['method']
    calls.append(method); pathlib.Path('calls.json').write_text(json.dumps(calls))
    if pending and pathlib.Path('release-real').exists():
        user_id = len(rows) + 1
        rows.extend([{'role': 'user', 'row_id': user_id, 'text': pending}, {'role': 'assistant', 'row_id': user_id + 1, 'text': '42'}])
        previous = {'text': '42', 'status': 'complete', 'persisted_turn': {'row_ids': [user_id, user_id+1], 'complete': True, 'user_row_id': user_id, 'final_assistant_row_id': user_id+1}}
        emit('message.complete', previous)
        pending = None
    if method == 'session.create':
        assert not persisted
        result = {'session_id': live, 'stored_session_id': sid, 'messages': [], 'message_count': 0, 'info': {}}
    elif method == 'session.resume':
        assert p['session_id'] == sid and persisted, 'cannot resume an unpersisted stored ID'
        result = snapshot()
    else:
        assert p['session_id'] == live
        if method == 'session.title':
            if 'title' in p:
                title = p['title']; persisted = mode != 'unpersisted'
                result = {'title': title, 'pending': not persisted}
            else: result = {'title': title, 'session_key': sid}
        elif method == 'session.activate':
            activations += 1
            result = snapshot()
            if mode == 'busy_at_submit' and activations >= 2: result['running'] = True
        elif method == 'prompt.submit':
            assert persisted
            result = {'status': 'streaming'}
            if mode in ('replay_identical', 'replay_identical_delayed', 'replay_identical_held') and previous and not pathlib.Path('release-real').exists():
                if mode in ('replay_identical_delayed', 'replay_identical_held'):
                    pathlib.Path('admission-delayed').touch()
                    if mode == 'replay_identical_held':
                        deadline = time.monotonic() + 5
                        while not pathlib.Path('release-admission').exists():
                            assert time.monotonic() < deadline, 'admission fixture release deadline'
                            time.sleep(.02)
                    else:
                        time.sleep(.4)  # longer than the proof-only deadline; transport admission must finish
                pending = p['text']
                emit('message.start')
                emit('message.complete', previous)
                print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': result}), flush=True)
                continue
            user_id = len(rows) + 1
            rows.append({'role': 'user', 'row_id': user_id, 'text': p['text']})
            emit('message.start')
            if mode != 'echo_only':
                assistant_id = len(rows) + 1
                rows.append({'role': 'assistant', 'row_id': assistant_id, 'text': '42'})
                payload = {'text': '42', 'status': 'error' if mode == 'error_complete' else 'complete',
                    'persisted_turn': {'row_ids': [user_id, assistant_id], 'complete': True,
                        'user_row_id': assistant_id if mode == 'wrong_receipt' else user_id, 'final_assistant_row_id': assistant_id}}
                previous = payload
                emit('message.complete', payload, target='unowned-session' if mode == 'wrong_session' else live)
        elif method == 'session.history': result = {} if mode == 'missing_history' else {'messages': rows, 'count': len(rows)}
        elif method == 'session.close': result = {'closed': True}
        else: raise AssertionError(method)
    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': result}), flush=True)
'''


if __name__ == '__main__':
    unittest.main()
