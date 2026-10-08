"""Multiproject passes (#186) over isolated fixture repositories. Stage 2: read-only observation, each reader a real
subprocess whose tracker is a fake that logs every call, so no network and no mutation can pass unseen. Stage 3
(pinned Wiki Multiproject-acting-pass dcc97303): the real wrapper, actor and occupancy readers over on-disk in-memory
trackers, a file `claude agents` list and logged fake launches; no real tracker, runtime, project, PM or timer."""
import contextlib
from datetime import datetime, timedelta, timezone
import io
import json
import os
from pathlib import Path
import pickle
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(1, str(Path(__file__).resolve().parent))
import taskq as core  # noqa: E402
import taskq.multiproject as multiproject  # noqa: E402

tick, worker = sys.modules['taskq.tick'], sys.modules['taskq.worker']
CHILD = [sys.executable, str(Path(__file__).resolve()), '--child']
ACT = [sys.executable, str(Path(__file__).resolve()), '--act-child']
ACTOR = ACT + ['actor', '--actor']
CHECK = ACT + ['check', '--enroll-check']
# A separate process asks the host guard: `held` while an actor owns it.
PROBE = ('import fcntl, os, sys\nfd = os.open(sys.argv[1], os.O_RDWR)\ntry:\n    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)\n'
         '    print("free")\nexcept BlockingIOError:\n    print("held")')
# A worker descendant: does any of its open descriptors name the guard's inode?
DESCENDANT = """import json, os, sys, time
guard = os.stat(sys.argv[1])
names = os.listdir('/dev/fd')
inherited = []
for name in names:
    try:
        found = os.fstat(int(name))
    except OSError:
        continue
    inherited.append((found.st_dev, found.st_ino) == (guard.st_dev, guard.st_ino))
open(sys.argv[2], 'w').write(json.dumps({'pid': os.getpid(), 'inherited': any(inherited)}))
time.sleep(30)
"""


def child():
    """The test reader: the real `observe` with this checkout's fixture.json as its tracker and runtimes."""
    fixture = json.loads(Path('fixture.json').read_text())
    log = Path(fixture['log'])

    def write(kind, value):
        with log.open('a') as out:
            out.write(json.dumps([kind, value]) + '\n')
    write('reader', str(Path.cwd()))
    if 'sleep' in fixture:
        subprocess.Popen(['sleep', str(fixture['sleep'])])  # a grandchild holding the pipes: the group kill ends it
        time.sleep(fixture['sleep'])
    if 'exit' in fixture:
        sys.exit(fixture['exit'])
    if 'emit' in fixture:
        return print(fixture['emit'] if isinstance(fixture['emit'], str) else json.dumps(fixture['emit']))

    class Logged(subprocess.Popen):
        def __init__(self, args, *rest, **kwargs):
            write('command', [str(arg) for arg in args])
            super().__init__(args, *rest, **kwargs)
    subprocess.Popen = Logged

    def api(method, path, body=None):
        write('api', [method, path])
        if method != 'GET':
            raise AssertionError(f'mutation {method} {path}')
        if fixture.get('unavailable') and path.startswith('issues?'):
            raise SystemExit('fixture: tracker unavailable')
        if path == '/user':
            return {'id': 1}
        if path == 'repository':
            return {'id': fixture['repo_id']}
        if path == 'board':
            return {'url': fixture['board_url']}
        if path.startswith('issues?'):
            wanted = dict(pair.split('=', 1) for pair in path.partition('?')[2].split('&')).get('labels')
            return [issue for issue in fixture['issues'] if not wanted or wanted in issue['labels']]
        return []  # notes and label events of `age`
    core.api = api
    core.claude_agents = lambda strict=False: fixture.get('agents', {})

    def no_codex(**kwargs):
        raise OSError('fixture: no codex app server')
    core.Codex = no_codex
    multiproject.main(['--observe', sys.argv[2]])


def issue(repo, iid, state, claim=None, labels=(), sha=None):
    block = {'scope': [], 'deps': [], 'claim': claim, 'waiting_for': None, 'result': {'sha': sha} if sha else None}
    return {'iid': iid, 'title': f'Task {iid}', 'labels': [f'q-{state}', 'code', *labels], 'assignees': [],
            'author_association': 'OWNER', 'description': core.render('goal', block),
            'updated_at': '2026-10-01T00:00:00Z', 'web_url': f'https://github.com/{repo}/issues/{iid}'}


class Multiproject(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.dir = Path(folder.name).resolve()
        self.log = self.dir / 'calls.jsonl'
        self.alpha = self.repo('alpha', 11, 'alpha-board', issues=[
            issue('acme/alpha', 1, 'doing', {'runtime': 'codex', 'session': 'thread-1'}, sha='a' * 40),
            issue('acme/alpha', 2, 'review', {'runtime': 'codex', 'session': 'thread-2'}, labels=['area-x']), issue('acme/alpha', 3, 'ready')])
        self.beta = self.repo('beta', 22, 'beta-board', issues=[issue('acme/beta', 7, 'ask', {'runtime': 'codex', 'session': 'thread-7'}, labels=['area-y'])])

    def repo(self, name, repo_id, board, origin=None, toml_repo=None, **fixture):
        path = self.dir / name
        path.mkdir()
        subprocess.run(['git', 'init', '-q', str(path)], check=True)
        subprocess.run(['git', '-C', str(path), 'remote', 'add', 'origin', origin or f'git@github.com:acme/{name}.git'], check=True)
        (path / 'taskq.toml').write_text(f'[github]\nrepo = "{toml_repo or "acme/" + name}"\nboard = "{board}"\n'
                                         f'[profile]\nlimits = {{ codex = 1 }}\n')
        self.fixture(path, repo_id=repo_id, board_url=f'https://github.com/users/acme/projects/{repo_id}', **fixture)
        return {'provider': 'github', 'repository_id': repo_id, 'repository': f'acme/{name}', 'board': board,
                'checkout': str(path)}

    def fixture(self, path, **values):
        (path / 'fixture.json').write_text(json.dumps({'log': str(self.log), 'issues': [], **values}))

    def manifest(self, *entries, version=1):
        def value(item):
            return json.dumps(item) if not isinstance(item, dict) else \
                '{ ' + ', '.join(f'{key} = {value(inner)}' for key, inner in item.items()) + ' }'
        text = f'version = {version}\n' + ''.join(
            '\n[[project]]\n' + ''.join(f'{key} = {value(item)}\n' for key, item in entry.items()) for entry in entries)
        path = self.dir / 'manifest.toml'
        path.write_text(text)
        return path

    def run_all(self, *entries):
        return multiproject.aggregate(self.manifest(*entries), CHILD)

    def calls(self, kind=None):
        if not self.log.exists():
            return []
        rows = [json.loads(line) for line in self.log.read_text().splitlines()]
        return [value for found, value in rows if found == kind] if kind else rows

    def assertReadOnly(self):
        self.assertEqual([call for call in self.calls('api') if call[0] != 'GET'], [])
        for command in self.calls('command'):
            self.assertEqual(command[0], 'git', command)  # no taskq, claude, codex or gh: no worker launch, no write
            self.assertTrue({'rev-parse', 'remote'} & set(command), command)

    def test_two_isolated_repositories_with_their_own_settings_boards_and_profiles(self):
        alpha = {**self.alpha, 'view': {'filter': 'labels=area-x', 'limits': {'codex': 2}}}
        result = self.run_all(alpha, self.beta)
        self.assertEqual(result['outcome'], 'ok', result)
        self.assertEqual(result['received_applied'], 'unknown')
        one, two = result['projects']
        self.assertEqual([one['status'], two['status']], ['ok', 'ok'])
        self.assertEqual(one['report']['board'], 'https://github.com/users/acme/projects/11')
        self.assertEqual(two['report']['board'], 'https://github.com/users/acme/projects/22')
        self.assertEqual((one['report']['profile']['filter'], one['report']['profile']['limits']['codex']), ('labels=area-x', 2))
        self.assertEqual((two['report']['profile']['filter'], two['report']['profile']['limits']['codex']), ('', 1))
        self.assertEqual([row['task'] for row in one['report']['workers']], ['[#2](https://github.com/acme/alpha/issues/2)'])
        self.assertEqual([row['state'] for row in two['report']['workers']], ['ask'])
        self.assertEqual(two['report']['workers'][0]['event_at'], '2026-10-01T00:00:00Z')
        self.assertEqual(sorted(self.calls('reader')), sorted([self.alpha['checkout'], self.beta['checkout']]))
        self.assertReadOnly()

    def test_worker_links_commit_and_unknown_runtime_status_are_preserved(self):
        report = self.run_all(self.alpha)['projects'][0]['report']
        doing = report['workers'][0]
        self.assertEqual(doing['task'], '[#1](https://github.com/acme/alpha/issues/1)')
        self.assertIn('open.html#codex://threads/thread-1', doing['session'])
        self.assertEqual(doing['commit'], f'[{"a" * 40}](https://github.com/acme/alpha/commit/{"a" * 40})')
        self.assertIn('status unknown: fixture: no codex app server', doing['last_activity'])
        self.assertEqual(report['validation'], [])
        self.assertEqual(report['actions'], [])
        self.assertReadOnly()

    def test_duplicate_or_mismatched_identities_and_bindings_are_refused_before_reading(self):
        twin = {**self.beta, 'repository': 'acme/twin'}  # same stable id and checkout as beta
        wrong_origin = self.repo('gamma', 33, 'g', origin='https://github.com/evil/gamma.git')
        wrong_config = self.repo('delta', 44, 'd', toml_repo='acme/other')
        wrong_id = self.repo('epsilon', 55, 'e')
        wrong_board = {**self.repo('zeta', 66, 'z'), 'board': 'other'}
        result = self.run_all(self.alpha, self.beta, twin, {**wrong_origin}, wrong_config,
                              {**wrong_id, 'repository_id': 56}, wrong_board, {'provider': 'svn'})
        status = [project['status'] for project in result['projects']]
        self.assertEqual(status, ['ok', 'refused', 'refused', 'refused', 'refused', 'refused', 'refused', 'refused'])
        self.assertTrue(any('duplicate stable identity' in error for error in result['projects'][1]['errors']))
        self.assertTrue(any('duplicate checkout binding' in error for error in result['projects'][2]['errors']))
        self.assertIn('is not https://github.com/acme/gamma', result['projects'][3]['errors'][0])
        self.assertIn("repository 'acme/other'", result['projects'][4]['errors'][0])
        self.assertIn('tracker repository id 55 is not 56', result['projects'][5]['errors'][0])
        self.assertIn("board 'z'", result['projects'][6]['errors'][0])
        self.assertIn('provider: write "github" or "gitlab"', result['projects'][7]['errors'])
        self.assertNotIn(self.beta['checkout'], self.calls('reader'))  # a duplicate starts no reader
        issue_reads = [call for call in self.calls('api') if call[1].startswith('issues?')]
        self.assertEqual(len(issue_reads), 1)  # only alpha's queue was read
        self.assertEqual(result['outcome'], 'blocked')
        self.assertReadOnly()

    def test_slow_and_failed_projects_do_not_stop_the_next_one(self):
        slow = {**self.repo('slow', 31, 's', sleep=60), 'timeout': 2}
        crash = self.repo('crash', 32, 'c', exit=3)
        started = time.monotonic()
        result = self.run_all(slow, crash, self.beta)
        self.assertLess(time.monotonic() - started, 30)
        self.assertEqual([project['status'] for project in result['projects']], ['timeout', 'failed', 'ok'])
        self.assertIsNone(result['projects'][0]['report'])
        self.assertIn('reader exit 3', result['projects'][1]['errors'][0])
        self.assertReadOnly()

    def test_malformed_stale_and_unavailable_observations_stay_explicit(self):
        old = (datetime.now(timezone.utc) - timedelta(hours=1)).isoformat().replace('+00:00', 'Z')
        report = {'contract': tick.report_contract(), 'repository': 'https://github.com/acme/stale', 'profile': {'filter': '', 'mine': False, 'limits': {}},
                  'board': 'https://github.com/users/acme/projects/1', 'observed_at': old, 'outcome': 'ok', 'actions': [],
                  'refusals': [], 'workers': [], 'source_status': 'available', 'validation': []}
        result = self.run_all(self.repo('garbage', 41, 'g', emit='not json'),
                              self.repo('partial', 42, 'p', emit={'outcome': 'ok', 'workers': []}),
                              self.repo('stale', 43, 's', emit=report),
                              self.repo('down', 44, 'd', unavailable=True), self.beta)
        garbage, partial, stale, down, healthy = result['projects']
        self.assertEqual([garbage['status'], partial['status'], stale['status'], down['status'], healthy['status']],
                         ['malformed', 'malformed', 'blocked', 'blocked', 'ok'])
        self.assertIn('invalid/stale observed_at; obtain a fresh tick', stale['errors'])
        self.assertEqual(down['report']['source_status'], 'unavailable')
        self.assertEqual(down['report']['workers'], [])  # unknown, never empty healthy work
        self.assertIn('Workers: unknown (source unavailable)', multiproject.render(result))
        self.assertIn('outcome failure', down['errors'])
        self.assertReadOnly()

    def test_one_entry_manifest_matches_the_same_project_in_a_larger_one(self):
        alone = self.run_all(self.beta)['projects'][0]
        together = self.run_all(self.alpha, self.beta)['projects'][1]
        for project in (alone, together):
            project['report'].pop('observed_at')
        self.assertEqual(alone, together)
        self.assertEqual(alone['status'], 'ok')

    def test_entries_added_and_removed_from_the_saved_manifest_take_effect_on_the_next_run(self):
        before = sorted(path.name for path in self.dir.iterdir())
        self.assertEqual([p['repository'] for p in self.run_all(self.alpha)['projects']], ['https://github.com/acme/alpha'])
        self.assertEqual(len(self.run_all(self.alpha, self.beta)['projects']), 2)
        self.log.unlink()
        self.assertEqual([p['repository'] for p in self.run_all(self.beta)['projects']], ['https://github.com/acme/beta'])
        self.assertEqual(self.calls('reader'), [self.beta['checkout']])  # a removed entry is read no more
        self.assertEqual(sorted(path.name for path in self.dir.iterdir()), sorted(before + ['calls.jsonl', 'manifest.toml']))
        for name in ('alpha', 'beta'):  # no state of its own in a project either
            self.assertEqual(sorted(path.name for path in (self.dir / name).iterdir()), ['.git', 'fixture.json', 'taskq.toml'])

    def test_manifest_shape_errors(self):
        with self.assertRaises(SystemExit):
            multiproject.load_manifest(self.manifest(self.alpha, version=2))
        with self.assertRaises(SystemExit):
            multiproject.load_manifest(self.manifest())
        bad = {**self.alpha, 'repository_id': '11', 'checkout': 'relative', 'timeout': 0, 'view': {'filter': 1}}
        errors = multiproject.load_manifest(self.manifest(bad))[0][1]
        self.assertEqual(len(errors), 4, errors)

    def test_remote_identity(self):
        for url in ('https://github.com/Acme/Alpha.git', 'git@github.com:acme/alpha.git',
                    'ssh://git@github.com:22/acme/alpha', 'https://token@github.com/acme/alpha/\n'):
            self.assertEqual(multiproject.remote_identity(url), ('github.com', 'acme/alpha'), url)
        self.assertEqual(multiproject.remote_identity('https://gitlab.example/group/sub/project.git'),
                         ('gitlab.example', 'group/sub/project'))

    def test_real_entrypoint_runs_the_reader_in_its_own_process(self):
        wrong = self.repo('real', 77, 'r', toml_repo='acme/elsewhere')  # refused locally: no tracker or network read
        done = subprocess.run([sys.executable, '-m', 'taskq.multiproject', '--manifest', str(self.manifest(wrong)), '--json'],
                              cwd=self.dir, capture_output=True, text=True, timeout=60,
                              env={**os.environ, 'PYTHONPATH': str(ROOT)})
        self.assertEqual(done.returncode, 1, done.stderr)
        project = json.loads(done.stdout)['projects'][0]
        self.assertEqual(project['status'], 'refused')
        self.assertIn("repository 'acme/elsewhere'", project['errors'][0])
        self.assertEqual(project['report']['contract']['sha256'], tick.REPORT_SHA256)
        self.assertEqual(self.calls(), [])  # the real reader never touched the fixture's fake


def toml(top, entries):
    def value(item):
        return json.dumps(item) if not isinstance(item, dict) else '{ ' + ', '.join(f'{key} = {value(inner)}' for key, inner in item.items()) + ' }'
    return ''.join(f'{key} = {value(item)}\n' for key, item in top.items()) + ''.join(
        '\n[[project]]\n' + ''.join(f'{key} = {value(item)}\n' for key, item in entry.items()) for entry in entries)


LIVE = [  # `claude agents --json --all`: six rows hold a place, any kind; a finished one does not
    {'id': 'i', 'kind': 'interactive', 'sessionId': 'owner-chat', 'cwd': '/elsewhere', 'pid': 101, 'status': 'idle'},
    *({'id': f'b{n}', 'kind': 'background', 'sessionId': f'bg-{n}', 'cwd': '/elsewhere', 'name': f'job {n}', 'state': 'working',
       'status': 'busy', 'pid': 200 + n} for n in range(4)),
    {'id': 'k', 'kind': 'background', 'sessionId': 'waiting-owner', 'cwd': '/elsewhere', 'name': 'x', 'state': 'blocked'},
    {'id': 'd', 'kind': 'background', 'sessionId': 'finished', 'cwd': '/elsewhere', 'name': 'y', 'state': 'done', 'pid': 300, 'status': 'idle'}]


class Acting(unittest.TestCase):
    """#186 stage 3: the guarded acting pass. Fixture-only: it qualifies no live project, runtime, PM or timer."""

    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.dir = Path(folder.name).resolve()
        (self.dir / 'taskq').mkdir(mode=0o700)
        self.machine = 'ab' * 16
        (self.dir / 'taskq' / 'machine-id').write_text(self.machine + '\n')
        self.enterContext(patch.dict(os.environ, {'XDG_STATE_HOME': str(self.dir), 'TASKQ_FIXTURE': str(self.dir), 'TASKQ_HOST': 'fixture-host',
                                                  'CLAUDE_CODE_SESSION_ID': 'coordinator', 'CODEX_THREAD_ID': '',
                                                  'CODEX_HOME': str(self.dir / 'codex-home')}))
        self.enterContext(patch.object(core, 'UPDATE_STAMP', self.dir / 'taskq' / 'update-last'))
        # In-process enrollment reads back through the fixture's subprocesses, never a real tracker.
        self.enterContext(patch.multiple(multiproject, OCCUPANCY=ACT + ['occupancy', '--occupancy'], CHECK=CHECK, ACTOR=ACTOR,
                                         claude_rows=lambda: json.loads((self.dir / 'agents.json').read_text())))
        self.enterContext(patch.object(core, 'CODEX_SOCKET', self.dir / 'no-codex.sock'))  # never the host's runtimes
        self.guard, self.anchor = multiproject.guard_path(), multiproject.anchor_path()
        self.records = self.dir / 'taskq' / 'multiproject-output'
        self.agents(LIVE)
        self.alpha = self.project('alpha', 11, tasks=[(1,), (2,)])
        self.beta = self.project('beta', 22, tasks=[(1,)])

    def agents(self, rows):
        (self.dir / 'agents.json').write_text(json.dumps(rows))

    def node(self, name):
        import hashlib
        return hashlib.sha256(f'acme/{name}:{self.machine}'.encode()).hexdigest()[:12]

    def project(self, name, repo_id, uid=1, tasks=(), update=False, **settings):
        """A main checkout with taskq.toml, origin and fixture.json, and its tracker with `tasks`: (iid, state, runtime, block, assignees)."""
        import test_taskq
        path = self.dir / name
        path.mkdir()
        subprocess.run(['git', 'init', '-q', str(path)], check=True)
        subprocess.run(['git', '-C', str(path), 'remote', 'add', 'origin', f'git@gitlab.example:acme/{name}.git'], check=True)
        (path / 'taskq.toml').write_text(f'[gitlab]\nproject = "acme/{name}"\nhost = "gitlab.example"\nboard = "{name}-board"\n'
                                         f'[update]\nauto = {str(update).lower()}\n')
        (path / 'fixture.json').write_text(json.dumps(settings))
        store = test_taskq.Gitlab()
        store.repo_id, store.uid, store.boards = repo_id, uid, [{'id': 7, 'name': f'{name}-board', 'lists': []}]
        for iid, state, runtime, block, assignees in ((*task, 'ready', 'claude', {}, ())[:1] + task[1:] + ('ready', 'claude', {}, ())[len(task) - 1:]
                                                      for task in tasks):
            store.issues[iid] = {'iid': iid, 'state': 'opened', 'web_url': f'https://gitlab.example/acme/{name}/-/issues/{iid}',
                                 'title': f'Task {iid}', 'labels': [f'q-{state}', 'code', f'run-{runtime}'],
                                 'description': core.render('goal', {'scope': [], 'deps': [], 'claim': None, 'waiting_for': None, 'result': None, **block}),
                                 'assignees': [{'id': one} for one in assignees], 'milestone_id': None,
                                 'updated_at': store.now(), 'created_at': store.now(), 'author': {'id': 1}}
            store.created = max(store.created, iid)
        (self.dir / f'acme_{name}.pickle').write_bytes(pickle.dumps(store))
        return {'provider': 'gitlab', 'host': 'gitlab.example', 'repository_id': repo_id, 'repository': f'acme/{name}',
                'board': f'{name}-board', 'checkout': str(path)}

    def store(self, name):
        return pickle.loads((self.dir / f'acme_{name}.pickle').read_bytes())

    def block(self, name, iid):
        return json.loads(core.BLOCK.search(self.store(name).issues[iid]['description'])[1])

    def catalog(self, *entries, file='policy.toml', enroll=True, **top):
        """The execution policy file; `enroll`: the owner's explicit initial enrollment of it, which must succeed."""
        bindings = [{'principal': 1, 'act': True, 'limits': {'claude': 2, 'codex': 0}, 'effects': list(multiproject.EFFECTS),
                     'timeout': 120, **entry} for entry in entries]
        path = self.dir / file
        path.write_text(toml({'version': 1, 'machine': 'fixture-host', 'os_user': os.getuid(), **top}, bindings))
        if enroll:
            found = multiproject.accept(path, CHECK)
            self.assertEqual((found['status'], found['errors']), ('enrolled', []))
        return path

    def act(self, view, policy):
        manifest = self.dir / 'manifest.toml'
        manifest.write_text(toml({'version': 1}, view))
        return multiproject.act(manifest, policy, ACTOR)

    def calls(self, kind=None):
        rows = [json.loads(line) for line in (self.dir / 'calls.jsonl').read_text().splitlines()] if (self.dir / 'calls.jsonl').exists() else []
        return [row for row in rows if kind in (None, row[1])]

    def mutations(self, name):
        return [row for row in self.calls('api') if row[2] == f'acme/{name}' and row[3] != 'GET']

    def probe(self):
        return subprocess.run([sys.executable, '-c', PROBE, str(self.guard)], capture_output=True, text=True).stdout.strip()

    def wait(self, condition, seconds=60):
        deadline = time.monotonic() + seconds
        while not condition():
            self.assertLess(time.monotonic(), deadline, 'fixture condition not reached')
            time.sleep(0.1)

    def limits(self, project):
        return {runtime: (found['occupancy'], found['known'], found['F'], found['L'], found['limit']) for runtime, found in project['budget'].items()}

    def test_one_aggregate_budget_over_two_projects_reaches_native_launch_admission(self):
        foreign = {'attempt': 'f00d', 'runtime': 'claude', 'principal': 2, 'coordinator': 'claude:other', 'node': '0' * 12, 'pid': 1}
        self.alpha = self.project('alpha2', 33, tasks=[(1,), (2,), (3, 'ready', 'claude', {}, (2,)), (4, 'ready', 'claude', {'reservation': foreign})])
        policy = self.catalog({**self.alpha}, {**self.beta, 'limits': {'claude': 1}})
        result = self.act([self.alpha, self.beta], policy)
        alpha, beta = result['projects']
        self.assertEqual([alpha['status'], beta['status']], ['ok', 'ok'], result)
        # Seven live rows of any kind, including terminal+PID; F = 8 - 7 = 1 for alpha. Its launch makes beta's occupancy 8: F = 0.
        self.assertEqual(self.limits(alpha), {'claude': (7, True, 1, 0, 1), 'codex': (0, False, 0, 0, 0)})
        self.assertEqual(self.limits(beta), {'claude': (8, True, 0, 0, 0), 'codex': (0, False, 0, 0, 0)})
        spawned = self.calls('spawn')
        self.assertEqual([(row[2], row[3]) for row in spawned], [('acme/alpha2', 'S1 Task 1 (fixture-host)')])  # #243: the task's supervisor
        self.assertEqual({row[4] for row in spawned}, {'held'})  # the host guard is held during the native mutation
        self.assertEqual([row[3] for row in self.calls('native')], [{'claude': 1, 'codex': 0}, {'claude': 0, 'codex': 0}])
        self.assertEqual(self.store('alpha2').issues[3]['assignees'], [{'id': 2}])
        self.assertEqual(self.block('alpha2', 4)['reservation'], foreign)
        self.assertNotIn('reservation', self.block('alpha2', 3))
        self.assertEqual([row for row in self.calls('api') if row[0] in ('occupancy', 'check') and row[3] != 'GET'], [])
        self.assertEqual(self.mutations('beta'), [])
        for project in (alpha, beta):
            self.assertEqual(tick.validate_report(project['report']), [])
            self.assertEqual(project['received_applied'], 'unknown')
            self.assertEqual(project['guard']['inode'], self.guard.stat().st_ino)
        self.assertEqual(result['received_applied'], 'unknown')
        self.assertIn('claude=1 (L 0 + F 1, occupancy 7/8', multiproject.render(result))

    def test_budget_counts_exact_sessions_once_and_unknowns_hold_every_runtime(self):
        self.assertEqual(multiproject.claude_inventory(LIVE), {'session:owner-chat', 'session:bg-0', 'session:bg-1', 'session:bg-2',
                                                               'session:bg-3', 'session:waiting-owner', 'session:finished'})
        self.assertEqual(multiproject.claude_inventory([{'kind': 'interactive', 'pid': 7}]), {'pid:7'})
        self.assertIsNone(multiproject.claude_inventory([{'kind': 'background', 'state': 'working'}]))  # unidentified live row
        self.assertIsNone(multiproject.claude_inventory({'rows': []}))
        self.assertEqual(multiproject.codex_inventory([{'id': 'a', 'status': {'type': 'active'}}, {'id': 'b', 'status': {'type': 'idle'}},
                                                       {'id': 'c', 'name': 'T4 x', 'status': {'type': 'notLoaded'}},
                                                       {'id': 'd', 'name': 'T5 y', 'status': {'type': 'systemError'}}]), {'session:a', 'session:c'})
        caps = {'claude': 8, 'codex': 4}
        own = {'status': 'ok', 'L': {'claude': 1, 'codex': 0},
               'held': [(['claude'], 'session:s1'), (['claude'], 'reservation:r'), (['claude', 'codex'], 'reservation:l')]}
        seven = {f'session:s{n}' for n in range(1, 8)}
        found = multiproject.budget(caps, {'claude': seven, 'codex': None}, [own], own, {'claude': 3, 'codex': 2})
        # s1 is one exact session; the reservations without one stay separate: 9 > 8, F clamps at 0.
        self.assertEqual((found['claude']['occupancy'], found['claude']['F'], found['claude']['limit']), (9, 0, 1))
        self.assertEqual((found['codex']['known'], found['codex']['limit']), (False, 0))
        small = multiproject.budget(caps, {'claude': {'session:s1'}, 'codex': set()}, [own], own, {'claude': 3, 'codex': 2})
        self.assertEqual((small['claude']['F'], small['claude']['limit'], small['codex']['F'], small['codex']['limit']), (5, 3, 3, 2))

    def test_archived_codex_claim_keeps_ownership_but_frees_only_proven_inactive_budget(self):
        archived = {'status': 'ok', 'L': {'claude': 0, 'codex': 0}, 'uncertain': [],
                    'held': [(['codex'], 'session:191')], 'inactive': ['session:191'], 'protected': []}
        caps, live = {'claude': 8, 'codex': 4}, {'claude': set(), 'codex': {'session:a', 'session:b', 'session:c'}}
        found = multiproject.budget(caps, live, [archived], archived, {'codex': 4})
        self.assertEqual((archived['held'], found['codex']['occupancy'], found['codex']['F']),
                         ([(['codex'], 'session:191')], 3, 1))
        # A resumed or live exact session wins over its archive readback; protected aliases remain retained too.
        live['codex'].add('session:191')
        self.assertEqual(multiproject.budget(caps, live, [archived], archived, {'codex': 4})['codex']['F'], 0)
        live['codex'].remove('session:191')
        archived['protected'] = ['session:191']
        self.assertEqual(multiproject.budget(caps, live, [archived], archived, {'codex': 4})['codex']['F'], 0)
        archived['protected'] = []
        twin = {**archived, 'held': [(['codex'], 'session:191')]}
        self.assertEqual(multiproject.budget(caps, live, [archived, twin], archived, {'codex': 4})['codex']['F'], 1)
        twin['protected'] = ['session:191']
        self.assertEqual(multiproject.budget(caps, live, [archived, twin], archived, {'codex': 4})['codex']['F'], 0)

    def test_ended_claude_executor_frees_compute_only_with_exact_terminal_no_pid_binding_proof(self):
        one, two = 'aac1d605-82f4-4b65-804c-ea07b78674cd', '3b45fa67-ed29-40de-93ba-f41f13a1345c'
        items = [{'iid': 198, 'state': 'review', 'claim': {'runtime': 'claude', 'session': one}, 'reservation': None},
                 {'iid': 205, 'state': 'later', 'claim': {'runtime': 'claude', 'session': two}, 'reservation': None}]
        rows = [{'id': 'aac1d605', 'kind': 'background', 'sessionId': one, 'cwd': str(self.dir), 'state': 'done'},
                {'id': '3b45fa67', 'kind': 'background', 'sessionId': two, 'cwd': str(self.dir), 'state': 'failed'}]
        with patch.multiple(multiproject, verify=lambda entry: [], locality=lambda owner: 'local', claude_rows=lambda: rows), \
                patch.multiple(core, user=lambda: 1, load=lambda: (items,), issues=lambda query: [],
                               room=lambda everything, capacity: {'claude': -2, 'codex': 0}):
            read = multiproject.occupancy({'principal': 1, 'host': 'example.test', 'repository': 'acme/test', 'checkout': str(self.dir)})
        self.assertEqual((read['held'], read['inactive'], read['protected'], read['L']['claude']),
                         ([(['claude'], f'session:{one}'), (['claude'], f'session:{two}')], [f'session:{one}', f'session:{two}'], [], 2))
        found = multiproject.budget({'claude': 2, 'codex': 1}, {'claude': set(), 'codex': set()}, [read], read, {'claude': 2})
        self.assertEqual((found['claude']['occupancy'], found['claude']['F'], found['claude']['L']), (0, 2, 2))

    def test_ended_claude_executor_holds_for_pid_live_alias_or_bad_binding_proof(self):
        item = {'iid': 185, 'state': 'review', 'claim': {'runtime': 'claude', 'session': '185'}, 'reservation': None}
        base = {'kind': 'background', 'sessionId': '185', 'cwd': str(self.dir), 'state': 'done', 'status': 'idle'}
        for row, state, reservation in (({**base, 'pid': 17629}, 'review', None),
                                        ({**base, 'cwd': '/wrong'}, 'review', None),
                                        (base, 'doing', None),
                                        (base, 'review', {'attempt': 'x', 'runtime': 'claude', 'principal': 1, 'node': 'local', 'pid': 1})):
            with self.subTest(row=row, state=state, reservation=reservation), \
                    patch.multiple(multiproject, verify=lambda entry: [], locality=lambda owner: 'local', claude_rows=lambda: [row]), \
                    patch.multiple(core, user=lambda: 1, load=lambda: ([{**item, 'state': state, 'reservation': reservation}],), issues=lambda query: [],
                                   room=lambda everything, capacity: {'claude': -1, 'codex': 0}), \
                    patch.object(worker, 'launch_session', return_value='185' if reservation else None):
                read = multiproject.occupancy({'principal': 1, 'host': 'example.test', 'repository': 'acme/test', 'checkout': str(self.dir)})
            self.assertEqual(read['inactive'], ['session:185'] if reservation else [])
            self.assertIn('session:185', read['protected'])
            found = multiproject.budget({'claude': 1, 'codex': 1}, {'claude': set(), 'codex': set()}, [read], read, {'claude': 1})
            self.assertEqual(found['claude']['occupancy'], 1)
            self.assertEqual(multiproject.claude_inventory([row]), {'session:185'} if row.get('pid') else set())

    def test_ended_claude_executor_rejects_interactive_missing_malformed_or_contradictory_rows(self):
        row = {'id': 'aac1d605', 'kind': 'background', 'sessionId': '198', 'cwd': str(self.dir), 'state': 'done'}
        for state in ('done', 'failed', 'stopped'):
            with self.subTest(state=state):
                self.assertTrue(multiproject.claude_executor_ended('198', self.dir, [{**row, 'state': state}]))
        for rows in (None, [{}], [{**row, 'kind': 'interactive'}], [{**row, 'cwd': '/wrong'}],
                     [{**row, 'state': 'working'}], [{**row, 'status': 'working'}], [{**row, 'status': 'blocked'}],
                     [{**row, 'status': 'busy'}], [{**row, 'status': None}], [{**row, 'status': 1}], [{**row, 'status': 'unknown'}],
                     [{**row, 'pid': 1}], [{**row, 'sessionId': 'other'}], [row, {**row, 'state': 'working'}]):
            with self.subTest(rows=rows):
                self.assertFalse(multiproject.claude_executor_ended('198', self.dir, rows))
        self.assertIsNone(multiproject.claude_inventory(None))
        self.assertIsNone(multiproject.claude_inventory([{}]))

    def test_terminal_status_contradictions_hold_raw_inventory_and_budget(self):
        session = '7b1c192f-23d7-4b03-94e2-ad05c2538a08'
        row = {'id': '7b1c192f', 'kind': 'background', 'sessionId': session, 'cwd': str(self.dir), 'state': 'done'}
        self.assertEqual(multiproject.claude_inventory([row]), set())
        for status in ('working', 'blocked', 'busy', None, 1, 'unknown'):
            with self.subTest(status=status):
                inventory = multiproject.claude_inventory([{**row, 'status': status}])
                self.assertEqual(inventory, {f'session:{session}'})
                read = {'status': 'ok', 'L': {'claude': 0, 'codex': 0}, 'uncertain': [],
                        'held': [(['claude'], f'session:{session}')], 'inactive': [f'session:{session}'], 'protected': []}
                found = multiproject.budget({'claude': 1, 'codex': 1}, {'claude': inventory, 'codex': set()}, [read], read, {'claude': 1})
                self.assertEqual((found['claude']['occupancy'], found['claude']['F']), (1, 0))

    def test_terminal_inventory_excludes_only_identified_background_rows(self):
        session = 'aac1d605-82f4-4b65-804c-ea07b78674cd'
        row = {'id': 'aac1d605', 'kind': 'background', 'sessionId': session, 'cwd': str(self.dir), 'state': 'done'}
        self.assertEqual(multiproject.claude_inventory([row]), set())
        read = {'status': 'ok', 'L': {'claude': 0, 'codex': 0}, 'uncertain': [],
                'held': [(['claude'], f'session:{session}')], 'inactive': [f'session:{session}'], 'protected': []}
        for bad, expected in (({'sessionId': None}, None), ({'kind': 'interactive'}, {f'session:{session}'}),
                              ({'kind': None}, {f'session:{session}'}), ({'cwd': None}, {f'session:{session}'}),
                              ({'cwd': 1}, {f'session:{session}'})):
            with self.subTest(bad=bad):
                inventory = multiproject.claude_inventory([{**row, **bad}])
                self.assertEqual(inventory, expected)
                found = multiproject.budget({'claude': 1, 'codex': 1}, {'claude': inventory, 'codex': set()}, [read], read, {'claude': 1})
                self.assertEqual((found['claude']['known'], found['claude']['F']), (expected is not None, 0))

    def test_occupancy_keeps_mixed_state_same_session_ineligible(self):
        items = [{'iid': 191, 'state': 'review', 'claim': {'runtime': 'codex', 'session': 'same'}, 'reservation': None},
                 {'iid': 192, 'state': 'doing', 'claim': {'runtime': 'codex', 'session': 'same'}, 'reservation': None}]
        with patch.multiple(multiproject, verify=lambda entry: [], locality=lambda owner: 'local',
                            codex_archived_inactive=lambda session: session == 'same'), \
                patch.multiple(core, user=lambda: 1, load=lambda: (items,), issues=lambda query: [],
                               room=lambda everything, capacity: {'claude': 0, 'codex': -1}):
            read = multiproject.occupancy({'principal': 1, 'host': 'example.test', 'repository': 'acme/test'})
        self.assertEqual((read['held'], read['inactive'], read['protected'], read['L']['codex']),
                         ([(['codex'], 'session:same'), (['codex'], 'session:same')], ['session:same'], ['session:same'], 1))
        self.assertEqual(multiproject.checked_read(read)['status'], 'ok')
        found = multiproject.budget({'claude': 2, 'codex': 6}, {'claude': set(), 'codex': set()}, [read], read, {'codex': 4})
        self.assertEqual((found['codex']['occupancy'], found['codex']['F'], found['codex']['L'], found['codex']['limit']), (1, 5, 1, 4))

    def test_codex_archive_proof_requires_exact_archived_inactive_thread(self):
        case = self

        class Codex:
            socket = type('Socket', (), {'close': lambda self: None})()

            def __init__(self, thread):
                self.thread = thread

            def call(self, method, params):
                case.assertEqual((method, params), ('thread/read', {'threadId': '191'}))
                return {'thread': self.thread}

        good = {'id': '191', 'path': '/x/archived_sessions/191.jsonl', 'status': {'type': 'idle'}}
        for thread, expected in ((good, True), ({**good, 'id': 'old'}, False),
                                 ({**good, 'status': {'type': 'active'}}, False),
                                 ({**good, 'path': '/x/sessions/191.jsonl'}, False)):
            with self.subTest(thread=thread), patch.object(core, 'Codex', lambda: Codex(thread)):
                self.assertIs(multiproject.codex_archived_inactive('191'), expected)
        with patch.object(core, 'Codex', side_effect=OSError('unavailable')):
            self.assertFalse(multiproject.codex_archived_inactive('191'))

    def test_removed_view_reads_the_old_anchored_binding(self):
        claim = lambda session: {'claim': {'runtime': 'claude', 'session': session, 'node': self.node('gone')}}  # noqa: E731
        gone = self.project('gone', 44, tasks=[(1, 'doing', 'claude', claim('s-1')), (2, 'doing', 'claude', claim('s-2')),
                                               (3, 'doing', 'claude', claim('bg-0'))])
        policy = self.catalog({**gone, 'act': False}, self.beta)
        result = self.act([self.beta], policy)
        self.assertEqual(self.limits(result['projects'][0])['claude'], (9, True, 0, 0, 0))  # 7 rows + s-1, s-2; bg-0 once
        self.assertEqual([item['status'] for item in result['projects'][0]['catalog']], ['ok', 'ok'])
        self.assertEqual(self.mutations('gone'), [])

    def test_unknown_catalog_or_runtime_refuses_before_native_pass_even_with_L(self):
        mine = {'claim': {'runtime': 'claude', 'session': 'bg-1', 'node': self.node('alpha2')}}
        self.alpha = self.project('alpha2', 33, tasks=[(1,), (2, 'doing', 'claude', mine)])
        policy = self.catalog(self.alpha, self.beta)
        self.agents('not json')  # the Claude inventory this project holds (L = 1) is unknown
        found = self.act([self.alpha], policy)['projects'][0]
        self.assertEqual((found['status'], found['budget']['claude']['L']), ('refused', 1))
        self.assertIn('claude inventory unknown', found['errors'][0])
        self.agents(LIVE)
        (self.dir / 'beta').rename(self.dir / 'beta-moved')  # an anchored binding's checkout is gone: ownership unknown
        found = self.act([self.alpha], policy)['projects'][0]
        self.assertEqual(found['status'], 'refused')
        self.assertIn('catalog ownership unknown, nothing run', found['errors'][0])
        self.assertEqual((self.calls('native'), self.calls('spawn'), self.mutations('alpha2')), ([], [], []))
        (self.dir / 'beta-moved').rename(self.dir / 'beta')
        found = self.act([self.alpha], policy)['projects'][0]  # known again: the native pass runs (its doing row has no https link)
        self.assertEqual((found['status'], found['errors']), ('blocked', ['worker session link unavailable; use the printed attach/app reference']))
        self.assertEqual(len(self.calls('native')), 1)

    def edit(self, name, iid=None, lock=False, **block):
        """Change one fixture tracker in place: a task's block fields, or an own lock award on task `iid`."""
        store = self.store(name)
        if block:
            store.issues[iid]['description'] = core.render('goal', {**self.block(name, iid), **block})
        if lock:
            store.awards[99] = {'id': 99, 'iid': iid, 'name': core.LOCK, 'user': {'id': store.uid}, 'created_at': store.now()}
        (self.dir / f'acme_{name}.pickle').write_bytes(pickle.dumps(store))

    def test_unproven_ownership_in_any_binding_refuses_before_native_pass_even_with_L(self):
        mine = {'claim': {'runtime': 'claude', 'session': 'bg-1', 'node': self.node('alpha2')}}
        self.alpha = self.project('alpha2', 33, tasks=[(1,), (2, 'doing', 'claude', mine)])
        policy = self.catalog(self.alpha, self.beta)
        clean = (self.dir / 'acme_beta.pickle').read_bytes()
        here = {'node': self.node('beta'), 'attempt': 'a77', 'coordinator': 'claude:coordina'}
        for change, why in (({'iid': 1, 'state': None, 'claim': {'runtime': 'mystery', 'session': 'm-1', 'node': self.node('beta')}},
                              "same-host claim of runtime 'mystery'"),
                            ({'iid': 1, 'lock': True}, 'tracker lock without a claim or reservation'),
                            ({'iid': 1, 'reservation': {**here, 'runtime': 'claude', 'principal': 1}}, "pid None: its owner or launch is unproven"),
                            ({'iid': 1, 'reservation': {**here, 'runtime': 'claude', 'principal': 2, 'pid': 7}}, 'principal 2'),
                            ({'iid': 1, 'reservation': {**here, 'runtime': 'mystery', 'principal': 1, 'pid': 7}}, "runtime 'mystery'")):
            (self.dir / 'acme_beta.pickle').write_bytes(clean)
            if change.pop('state', 1) is None:
                store = self.store('beta')
                store.issues[1]['labels'] = ['q-doing', 'code', 'run-claude']
                (self.dir / 'acme_beta.pickle').write_bytes(pickle.dumps(store))
            self.edit('beta', **change)
            before = self.store('beta').issues[1]['description']
            found = self.act([self.alpha], policy)['projects'][0]
            self.assertEqual(found['status'], 'refused', why)
            self.assertIn('beta: uncertain', found['errors'][0])
            self.assertIn(why, found['errors'][0])
            self.assertEqual((self.calls('native'), self.mutations('alpha2'), self.mutations('beta')), ([], [], []))
            self.assertEqual(self.store('beta').issues[1]['description'], before)  # read back, never released
        (self.dir / 'acme_beta.pickle').write_bytes(clean)
        self.assertEqual(self.act([self.alpha], policy)['projects'][0]['status'], 'blocked')  # known again: the pass runs
        self.assertEqual(len(self.calls('native')), 1)

    def test_claims_and_reservations_of_unknown_locality_refuse_and_known_remote_is_excluded(self):
        self.alpha = self.project('alpha2', 33, tasks=[(1,), (2,)])
        policy = self.catalog(self.alpha, {**self.beta, 'limits': {'claude': 0}})  # beta starts nobody: occupancy stays the fixture's
        before, clean = self.anchor.read_bytes(), (self.dir / 'acme_alpha2.pickle').read_bytes()
        nameless = {'runtime': 'claude', 'session': 'unproven-host-session'}

        def retain(state, **block):
            (self.dir / 'acme_alpha2.pickle').write_bytes(clean)
            store = self.store('alpha2')
            store.issues[1]['labels'] = [f'q-{state}', 'code', 'run-claude']
            (self.dir / 'acme_alpha2.pickle').write_bytes(pickle.dumps(store))
            self.edit('alpha2', 1, **block)
        cases = [(state, {'claim': nameless}) for state in ('review', 'ask', 'ready', 'doing')] + [
            ('doing', {'claim': {**nameless, 'node': 'xyz'}}), ('doing', {'claim': {**nameless, 'node': 5}}),
            ('review', {'claim': {**nameless, 'host': ''}}),
            ('ready', {'reservation': {'attempt': 'n0de', 'runtime': 'claude', 'principal': 1, 'pid': 7}})]
        for state, block in cases:
            retain(state, **block)
            described = self.store('alpha2').issues[1]['description']
            found = self.act([self.beta], policy)['projects'][0]
            self.assertEqual(found['status'], 'refused', block)
            self.assertIn('without a proven machine', found['errors'][0])
            self.assertEqual(found['catalog'][0]['L'], {'claude': 0, 'codex': 0})  # exactly native room: nothing local proven
            refused = multiproject.accept(self.catalog(self.beta, enroll=False, file='candidate.toml'), CHECK)
            self.assertIn('alpha2: ownership unknown', ' '.join(refused['errors']), block)
            self.assertEqual((self.anchor.read_bytes(), self.store('alpha2').issues[1]['description']), (before, described))
        self.assertEqual((self.calls('native'), self.mutations('alpha2')), ([], []))
        # No node, but the Claude app imported the session here: native evidence of a local claim, a held place.
        app = self.dir / 'app-sessions' / 'account' / 'org'
        app.mkdir(parents=True)
        (app / 'local_unproven-host-session.json').write_text('{}')
        retain('review', claim=nameless)
        found = self.act([self.beta], policy)['projects'][0]
        self.assertEqual((found['status'], found['budget']['claude']['occupancy']), ('ok', 8))
        refused = multiproject.accept(self.catalog(self.beta, enroll=False, file='candidate.toml'), CHECK)
        self.assertIn('https://gitlab.example/acme/alpha2: still owns session:unproven-host-session', refused['errors'])
        self.assertEqual(self.anchor.read_bytes(), before)
        # A well-formed node of another machine is that machine's to settle: excluded, not counted.
        retain('doing', claim={**nameless, 'node': '0' * 12})
        found = self.act([self.beta], policy)['projects'][0]
        self.assertEqual((found['status'], found['budget']['claude']['occupancy'], found['catalog'][0]['L']), ('ok', 7, {'claude': 0, 'codex': 0}))
        replaced = multiproject.accept(self.catalog(self.beta, enroll=False, file='candidate.toml'), CHECK)
        self.assertEqual((replaced['status'], replaced['generation']), ('replaced', 2))

    def test_malformed_readback_is_unknown_and_refuses_before_native_pass(self):
        policy = self.catalog(self.alpha)
        for read in ({'status': 'ok', 'held': [(['mystery'], 'lock:unknown-owner')], 'L': {'claude': 1, 'codex': 0}},
                     {'status': 'ok', 'held': [], 'L': {'claude': 1}},  # no uncertainty signal at all
                     {'status': 'ok', 'held': [(['claude'], 'claim:x')], 'L': {'claude': 1}, 'uncertain': []},
                     {'status': 'ok', 'held': [], 'L': {'claude': -1}, 'uncertain': []}, {'status': 'weird'}):
            with patch.multiple(multiproject, verify=lambda entry: [], workflow=lambda binding, machine: [], update_due=lambda: [],
                                read_occupancy=lambda binding, errors: read, claude_rows=lambda: [], codex_threads=lambda: []), \
                    patch.object(multiproject, 'native_pass') as native:
                found = multiproject.act_project({'entry': self.alpha, 'policy': str(policy)})
            self.assertEqual((found['status'], native.call_count), ('refused', 0), read)
            self.assertIn('occupancy readback malformed', found['errors'][0])

    def test_enrollment_is_explicit_runs_no_tick_and_only_table_order_may_change(self):
        policy = self.catalog(self.alpha, self.beta, enroll=False)
        result = self.act([self.alpha, self.beta], policy)  # never enrolled on a first --act
        self.assertEqual([project['status'] for project in result['projects']], ['refused', 'refused'])
        self.assertIn('no accepted execution catalog', result['projects'][0]['errors'][0])
        self.assertEqual((self.calls(), self.anchor.exists()), ([], False))
        found = multiproject.accept(policy, CHECK)
        self.assertEqual((found['status'], found['generation'], found['errors']), ('enrolled', 1, []))
        self.assertEqual({row[0] for row in self.calls()}, {'check'})  # identity and workflow checks only
        self.assertEqual([row for row in self.calls('api') if row[3] != 'GET'], [])
        self.assertEqual((self.calls('native'), self.calls('spawn')), ([], []))
        self.assertEqual(self.anchor.stat().st_mode & 0o777, 0o600)
        anchor = json.loads(self.anchor.read_text())
        self.assertEqual((anchor['generation'], anchor['catalog']['policy']['os_user']), (1, os.getuid()))
        self.assertEqual(multiproject.accept(policy, CHECK)['status'], 'unchanged')
        reordered = self.catalog(self.beta, {**self.alpha, 'effects': list(reversed(multiproject.EFFECTS))}, enroll=False)
        self.assertEqual(self.act([self.beta], reordered)['projects'][0]['status'], 'ok')
        changed = self.catalog(self.alpha, {**self.beta, 'limits': {'claude': 1}}, enroll=False)
        result = self.act([self.beta], changed)
        self.assertIn('differs from accepted generation 1', result['projects'][0]['errors'][0])
        self.assertEqual(len(self.calls('native')), 1)
        for n, (binding, top, why) in enumerate((({**self.alpha, 'principal': 5}, {}, 'the catalog binds 5'),
                                                 (self.alpha, {'machine': 'other-host'}, "not 501 on 'other-host'".replace('501', str(os.getuid()))),
                                                 ({**self.alpha, 'ready': True}, {}, 'ready: unknown key; readiness and effects come from'),
                                                 ({**self.alpha, 'effects': ['queue']}, {}, "['cleanup', 'idle_stop'] the catalog did not select"))):
            bad = self.catalog(binding, enroll=False, file=f'bad-{n}.toml', **top)
            refused = multiproject.accept(bad, CHECK)
            self.assertEqual(refused['status'], 'refused')
            self.assertIn(why, ' '.join(refused['errors']))
        self.assertEqual(json.loads(self.anchor.read_text()), anchor)

    def test_replacement_needs_every_old_binding_settled_and_keeps_the_anchor_otherwise(self):
        old = self.catalog(self.alpha, self.beta, file='old.toml')
        before = self.anchor.read_bytes()
        hidden = {'attempt': 'h1dd', 'runtime': 'claude', 'principal': 1, 'coordinator': 'claude:coordina', 'node': self.node('alpha'), 'pid': 1}
        store = self.store('alpha')
        store.issues[1]['description'] = core.render('goal', {**self.block('alpha', 1), 'reservation': hidden})
        (self.dir / 'acme_alpha.pickle').write_bytes(pickle.dumps(store))
        for candidate in (self.catalog(self.beta, enroll=False), self.catalog({**self.alpha, 'principal': 2}, self.beta, enroll=False)):
            refused = multiproject.accept(candidate, CHECK)
            self.assertEqual(refused['status'], 'refused')
            self.assertIn('https://gitlab.example/acme/alpha: still owns reservation:', ' '.join(refused['errors']))
            self.assertEqual(self.anchor.read_bytes(), before)
        self.assertIn('differs from accepted generation 1', self.act([self.beta], self.catalog(self.beta, enroll=False))['projects'][0]['errors'][0])
        (self.dir / 'acme_alpha.pickle').write_bytes(pickle.dumps(store))
        self.edit('alpha', iid=2, lock=True)  # the former principal's lock with no claim or reservation
        refused = multiproject.accept(self.catalog({**self.alpha, 'principal': 2}, self.beta, enroll=False), CHECK)
        self.assertIn('alpha: ownership unknown: https://gitlab.example/acme/alpha#2: tracker lock without a claim or reservation', ' '.join(refused['errors']))
        self.assertEqual(self.anchor.read_bytes(), before)
        store = self.store('alpha')
        store.awards.clear()
        store.issues[1]['description'] = core.render('goal', {**self.block('alpha', 1), 'reservation': None})
        (self.dir / 'acme_alpha.pickle').write_bytes(pickle.dumps(store))
        self.agents(LIVE + [{'id': 'a', 'kind': 'background', 'sessionId': 'in-alpha', 'cwd': self.alpha['checkout'], 'name': 'T1 x', 'state': 'working', 'pid': 9}])
        refused = multiproject.accept(self.catalog(self.beta, enroll=False), CHECK)
        self.assertIn('claude sessions still run in anchored checkouts: in-alpha', refused['errors'])
        self.agents(LIVE)
        (self.dir / 'beta').rename(self.dir / 'beta-moved')
        refused = multiproject.accept(self.catalog({**self.alpha, 'limits': {'claude': 1}}, enroll=False), CHECK)
        self.assertIn('https://gitlab.example/acme/beta: ownership unknown', ' '.join(refused['errors']))
        (self.dir / 'beta-moved').rename(self.dir / 'beta')
        self.assertEqual(self.anchor.read_bytes(), before)
        self.assertEqual(self.mutations('alpha'), [])  # nothing released, stolen or settled by the adapter
        review = {'runtime': 'claude', 'session': 'owned-review', 'node': self.node('alpha')}  # its worker is in no inventory
        store = self.store('alpha')
        store.issues[2]['labels'] = ['q-review', 'code', 'run-claude']
        store.issues[2]['description'] = core.render('goal', {**self.block('alpha', 2), 'claim': review})
        (self.dir / 'acme_alpha.pickle').write_bytes(pickle.dumps(store))
        refused = multiproject.accept(self.catalog(self.beta, enroll=False), CHECK)
        self.assertIn('https://gitlab.example/acme/alpha: still owns session:owned-review', refused['errors'])
        self.assertEqual((self.anchor.read_bytes(), self.calls('native')), (before, []))
        store.issues[2]['labels'] = ['q-ready', 'code', 'run-claude']
        store.issues[2]['description'] = core.render('goal', {**self.block('alpha', 2), 'claim': None})
        (self.dir / 'acme_alpha.pickle').write_bytes(pickle.dumps(store))
        replaced = multiproject.accept(self.catalog(self.beta, enroll=False), CHECK)
        self.assertEqual((replaced['status'], replaced['generation']), ('replaced', 2))
        self.assertEqual(self.calls('native'), [])
        self.assertIn('differs from accepted generation 2', self.act([self.beta], old)['projects'][0]['errors'][0])

    def test_corrupt_lost_or_foreign_anchor_refuses_and_is_never_reset(self):
        policy = self.catalog(self.alpha)
        good = self.anchor.read_bytes()
        for broken, why in ((b'{"version": 1', 'malformed'), (json.dumps({**json.loads(good), 'version': 2}).encode(), 'malformed'),
                            (json.dumps({**json.loads(good), 'sha256': '0' * 64}).encode(), 'malformed')):
            self.anchor.write_bytes(broken)
            self.assertIn(why, self.act([self.alpha], policy)['projects'][0]['errors'][0])
            self.assertIn(why, multiproject.accept(policy, CHECK)['errors'][0])
            self.assertEqual(self.anchor.read_bytes(), broken)
        def resigned(change):
            anchor = json.loads(good)
            change(anchor)
            anchor['sha256'] = multiproject.digest(anchor['catalog'])
            return json.dumps(anchor).encode()
        for broken in (resigned(lambda anchor: anchor['catalog']['policy'].update(extra=1)),
                       resigned(lambda anchor: anchor['catalog']['project'][0]['limits'].update(claude=65)),
                       resigned(lambda anchor: anchor['catalog']['project'][0].pop('effects')),
                       resigned(lambda anchor: anchor['catalog']['project'][0].update(host='GitLab.Example')),  # not canonical
                       resigned(lambda anchor: anchor.update(generation=True)), resigned(lambda anchor: anchor.update(receipt='applied'))):
            self.anchor.write_bytes(broken)
            self.assertIn('malformed', self.act([self.alpha], policy)['projects'][0]['errors'][0])
            self.assertIn('malformed', multiproject.accept(policy, CHECK)['errors'][0])
            self.assertEqual(self.anchor.read_bytes(), broken)
        elsewhere = resigned(lambda anchor: anchor['catalog']['policy'].update(machine='a-different-host'))
        self.anchor.write_bytes(elsewhere)
        for found in (self.act([self.alpha], policy)['projects'][0], multiproject.accept(policy, CHECK),
                      multiproject.accept(self.catalog({**self.alpha, 'limits': {'claude': 1}}, enroll=False, file='replace.toml'), CHECK)):
            self.assertIn("belongs to OS user", found['errors'][0])
            self.assertIn("on 'a-different-host'", found['errors'][0])
        self.assertEqual(self.anchor.read_bytes(), elsewhere)
        foreign = json.loads(good)
        foreign['catalog']['policy']['os_user'] = os.getuid() + 1
        foreign['sha256'] = multiproject.digest(foreign['catalog'])
        self.anchor.write_text(json.dumps(foreign))
        self.assertIn(f'belongs to OS user {os.getuid() + 1}', self.act([self.alpha], policy)['projects'][0]['errors'][0])
        self.anchor.unlink()
        (self.dir / 'elsewhere.json').write_bytes(good)
        self.anchor.symlink_to(self.dir / 'elsewhere.json')
        self.assertIn('refused, left as it is', self.act([self.alpha], policy)['projects'][0]['errors'][0])
        self.assertTrue(self.anchor.is_symlink())
        self.anchor.unlink()  # state loss: no anchor, no acting until the owner enrolls again
        self.assertIn('no accepted execution catalog', self.act([self.alpha], policy)['projects'][0]['errors'][0])
        self.assertIn('outside the catalog', multiproject.accept(self.catalog(self.alpha, os_user=os.getuid() + 1, enroll=False), CHECK)['errors'][0])
        self.assertEqual((self.calls('native'), self.anchor.exists()), ([], False))

    def test_actor_identity_and_view_mismatch_refuse_before_any_mutation(self):
        policy = self.catalog(self.alpha, {**self.beta, 'act': False})
        for top in ({'evidence': 'doctor ok'}, {'caps': {'claude': 65}}):
            with self.assertRaises(SystemExit):
                multiproject.load_policy(self.catalog(self.alpha, enroll=False, file='bad.toml', **top))
        result = self.act([{**self.alpha, 'board': 'other-board'}, self.beta], policy)
        self.assertIn('no valid execution catalog binding', result['projects'][0]['errors'][0])
        self.assertIn('catalog-only binding', result['projects'][1]['errors'][0])
        store = self.store('alpha')
        store.uid = 3  # the tracker now authenticates someone else
        (self.dir / 'acme_alpha.pickle').write_bytes(pickle.dumps(store))
        found = self.act([self.alpha], policy)['projects'][0]
        self.assertIn('the tracker principal is 3, the catalog binds 1; no delegation exists', found['errors'])
        self.assertEqual((self.mutations('alpha'), self.calls('native')), ([], []))
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            multiproject.main(['--manifest', str(self.dir / 'manifest.toml'), '--act'])

    def test_actor_rechecks_the_workflow_and_a_failure_does_not_stop_the_next(self):
        late = self.project('late', 62, tasks=[(1,)])
        stale = self.project('stale', 63, tasks=[(1,)], update=True)
        broken = self.project('broken', 64, tasks=[(1,)], lock_fails=True)
        policy = self.catalog(late, stale, broken, self.beta)
        (self.dir / 'late' / 'fixture.json').write_text(json.dumps({'doctor': '`claude` is not logged in'}))  # after enrollment
        result = self.act([late, stale, broken, self.beta], policy)
        self.assertEqual([project['status'] for project in result['projects']], ['refused', 'refused', 'failed', 'ok'])
        self.assertIn('`claude` is not logged in', result['projects'][0]['errors'][0])
        self.assertIn('auto-update is due', result['projects'][1]['errors'][0])
        self.assertEqual(result['projects'][2]['native']['outcome'], 'failure')
        for name in ('late', 'stale'):
            self.assertEqual(self.mutations(name), [])
        self.assertNotIn('reservation', self.block('broken', 1))
        self.assertEqual([row[2] for row in self.calls('spawn')], ['acme/beta'])
        self.assertEqual(result['outcome'], 'blocked')

    def test_timeout_keeps_actor_and_guard_and_admits_nothing_more_then_restart_reads_back(self):
        slow = self.project('slow', 71, tasks=[(1,)], spawn={'sleep': 8})
        policy = self.catalog({**slow, 'timeout': 3, 'limits': {'claude': 1}}, self.beta)
        inode = self.guard.stat().st_ino
        result = self.act([slow, self.beta], policy)
        self.assertEqual([project['status'] for project in result['projects']], ['unknown', 'not_admitted'])
        self.assertIn('outcome unknown; nothing more is admitted', result['projects'][1]['errors'][0])
        self.wait(lambda: self.calls('spawn'))
        self.assertEqual(self.probe(), 'held')  # the actor was not killed: it still owns the guard
        self.assertIsNotNone(multiproject.guard_free({'os_user': os.getuid()}))
        self.wait(lambda: multiproject.guard_free({'os_user': os.getuid()}) is None)
        self.assertEqual(self.guard.stat().st_ino, inode)
        self.assertFalse(any(row[0] == 'actor' and 'beta' in json.dumps(row) for row in self.calls()))
        self.assertTrue(self.block('slow', 1)['supervisor'])  # the kept actor finished its launch (#243: a supervisor)
        again = self.act([self.beta], policy)['projects'][0]
        self.assertEqual((again['status'], self.limits(again)['claude']), ('ok', (8, True, 0, 0, 0)))  # terminal+PID and slow's worker count

    def test_wrapper_death_keeps_the_actor_and_its_guard_and_descendants_inherit_nothing(self):
        held = self.project('held', 81, tasks=[(1,)], spawn={'descendant': True, 'sleep': 3})
        manifest = self.dir / 'manifest.toml'
        manifest.write_text(toml({'version': 1}, [held]))
        wrapper = subprocess.Popen(ACT + ['wrapper', '--manifest', str(manifest), '--act', '--execution-policy',
                                          str(self.catalog({**held, 'limits': {'claude': 1}})), '--json'],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.wait(lambda: self.calls('spawn'))
        wrapper.send_signal(signal.SIGKILL)
        wrapper.wait()
        self.assertEqual(self.probe(), 'held')
        self.wait(lambda: multiproject.guard_free({'os_user': os.getuid()}) is None)
        descendant = json.loads((self.dir / 'descendant.json').read_text())
        self.addCleanup(lambda: os.kill(descendant['pid'], signal.SIGKILL))
        os.kill(descendant['pid'], 0)  # still running, and the guard is free: it never held it
        self.assertFalse(descendant['inherited'])
        self.assertIn('Attempt', '\n'.join(note['body'] for note in self.store('held').notes.values()))
        self.assertTrue(any(row['sessionId'].startswith('w-held') for row in json.loads((self.dir / 'agents.json').read_text())))
        # The dead wrapper printed nothing; the record folder names the run, and its actual output survives.
        [record] = os.listdir(self.records)
        found = multiproject.recover(record.removesuffix('.json'), self.dir / 'policy.toml')
        self.assertEqual((found['status'], found['output']['status']), ('recovered', 'ok'), found)
        self.assertEqual(found['output']['guard']['inode'], self.guard.stat().st_ino)  # the one continuous guard
        self.assertEqual(len(self.calls('native')), 1)

    def test_crash_frees_the_lock_keeps_the_inode_and_restart_counts_the_orphan_without_releasing_it(self):
        crash = self.project('crash', 91, tasks=[(1,)], spawn={'crash': True})
        policy = self.catalog({**crash, 'limits': {'claude': 1}}, self.beta)
        inode = self.guard.stat().st_ino
        result = self.act([crash, self.beta], policy)
        self.assertEqual([project['status'] for project in result['projects']], ['unknown', 'not_admitted'])
        self.assertIn('actor exit 9 without a result', result['projects'][0]['errors'][0])
        self.assertEqual((multiproject.guard_free({'os_user': os.getuid()}), self.guard.stat().st_ino), (None, inode))
        orphan, written = self.block('crash', 1)['reservation'], self.mutations('crash')
        lost = multiproject.recover(result['projects'][0]['run_id'], policy)  # the crash left its record empty
        self.assertEqual((lost['status'], lost['output'], lost['received_applied']), ('unknown', None, 'unknown'))
        self.assertIn('is empty: its actor has not completed it', lost['errors'][0])
        again = self.act([self.beta], policy)['projects'][0]
        self.assertEqual((again['status'], self.limits(again)['claude']), ('ok', (8, True, 0, 0, 0)))  # terminal+PID and the orphan hold places
        self.assertEqual((self.block('crash', 1)['reservation'], self.mutations('crash')), (orphan, written))  # read back, not released

    def test_late_actor_output_is_recovered_after_wrapper_timeout_and_a_restart_while_it_runs_acts_on_nothing(self):
        slow = self.project('slow', 71, tasks=[(1,)], spawn={'sleep': 8})
        policy = self.catalog({**slow, 'timeout': 3, 'limits': {'claude': 1}}, self.beta)
        result = self.act([slow, self.beta], policy)
        late = result['projects'][0]
        self.assertEqual((late['status'], result['received_applied']), ('unknown', 'unknown'))
        self.assertIn(f'recoverable with run {late["run_id"]}', late['errors'][0])
        self.assertIn(f'--recover-actor-output {late["run_id"]} --execution-policy {policy}', late['recovery'])
        self.assertIn('Actual output record: python -m taskq.multiproject --recover-actor-output', multiproject.render(result))
        self.wait(lambda: self.calls('spawn'))
        for _ in range(2):  # a restarted wrapper or operator while the actor runs: pending, nothing started or replayed
            pending = multiproject.recover(late['run_id'], policy)
            self.assertEqual((pending['status'], pending['output'], pending['received_applied']), ('pending', None, 'unknown'))
            self.assertIn('host guard held', pending['errors'][0])
        self.assertEqual((len(self.calls('native')), len(self.calls('spawn'))), (1, 1))
        self.wait(lambda: multiproject.guard_free({'os_user': os.getuid()}) is None)
        path = self.records / f'{late["run_id"]}.json'
        self.assertEqual((path.stat().st_mode & 0o777, self.records.stat().st_mode & 0o777), (0o600, 0o700))
        written = path.read_bytes()
        for _ in range(2):  # across restarts: the same actual output, read only
            found = multiproject.recover(late['run_id'], policy)
            self.assertEqual((found['status'], found['errors'], found['received_applied']), ('recovered', [], 'unknown'), found)
            output = found['output']
            self.assertEqual((output['status'], output['native']['outcome']), ('ok', 'ok'))
            self.assertEqual(tick.validate_report(output['report'], tick.report_timestamp(found['completed_at'])), [])
            self.assertEqual((output['guard']['inode'], output['guard']['pid']), (self.guard.stat().st_ino, json.loads(written)['actor']['pid']))
        self.assertEqual((len(self.calls('native')), len(self.calls('spawn')), path.read_bytes()), (1, 1, written))
        self.assertEqual([row for row in self.calls('api') if row[0] == 'occupancy' and row[3] != 'GET'], [])
        done = subprocess.run(ACT + ['wrapper', '--recover-actor-output', late['run_id'], '--execution-policy', str(policy), '--json'],
                              capture_output=True, text=True, timeout=120)
        self.assertEqual((done.returncode, json.loads(done.stdout)['status']), (0, 'recovered'), done.stderr)
        self.assertEqual(len(self.calls('native')), 1)
        self.assertTrue(path.exists())  # never deleted by recovery

    def test_mismatched_or_damaged_records_stay_unknown_and_unknown_readback_refuses_recovery(self):
        policy = self.catalog(self.alpha, {**self.beta, 'limits': {'claude': 1}})
        run = self.act([self.beta], policy)['projects'][0]['run_id']
        path = self.records / f'{run}.json'
        good = path.read_bytes()
        self.assertEqual(multiproject.recover(run, policy)['status'], 'recovered')

        def changed(**fields):
            return json.dumps({**json.loads(good), **fields}).encode()
        record = json.loads(good)
        guard, actual = record['result']['guard'], record['result']

        def result(**fields):
            return changed(result={**actual, **fields})
        dropped = {key: value for key, value in record.items() if key != 'log'}
        cases = [  # the reviewer's four probes first: each recovered `ok` before
            (changed(actor={}, result={**actual, 'guard': None}), 'actor pid'), (changed(completed_at='not-a-timestamp'), 'completion time'),
            (result(report=None), 'v1 report'), (result(guard={**guard, 'device': -1}), 'guard'),
            (changed(version=True), 'version'), (changed(version='1'), 'version'), (changed(version=2), 'version'),
            (changed(completed_at='2999-01-01T00:00:00Z'), 'completion time'), (changed(completed_at=None), 'completion time'),
            (changed(policy_sha256='0' * 64), 'policy'), (changed(policy_sha256=None), 'policy'), (changed(binding_sha256='0' * 64), 'binding'),
            (changed(repository='https://gitlab.example/acme/alpha'), 'binding'), (changed(run_id='0' * 32), 'run id'),
            (changed(os_user=os.getuid() + 1), 'OS user'), (changed(os_user=float(os.getuid())), 'OS user'),
            (changed(machine='a-different-host'), 'machine'), (changed(actor={'pid': 0}), 'actor pid'), (changed(actor={'pid': True}), 'actor pid'),
            (changed(actor={'pid': guard['pid'], 'ppid': 1}), 'actor pid'), (changed(actor={'pid': guard['pid'] + 1}), 'guard'),
            (changed(actor=None), 'actor pid'), (changed(log=None), 'diagnostics'), (changed(log='x' * (multiproject.MAX_LOG + 1)), 'diagnostics'),
            (result(guard={**guard, 'inode': guard['inode'] + 1}), 'guard'), (result(guard={**guard, 'device': guard['device'] + 1}), 'guard'),
            (result(guard={**guard, 'path': str(self.dir / 'other.lock')}), 'guard'), (result(guard={**guard, 'domain': 'OS user 0 only'}), 'guard'),
            (result(guard={**guard, 'inode': float(guard['inode'])}), 'guard'), (result(guard={**guard, 'extra': 1}), 'guard'),
            (result(guard=None), 'guard: only a refusal'), (result(status='refused', guard=None), 'guard: only a refusal'),
            (result(status='refused'), 'refusal: it ran nothing native'), (result(status='applied'), 'result schema'),
            (result(errors=[1]), 'result schema'), (result(errors='none'), 'result schema'), (result(budget=[]), 'result schema'),
            (changed(result={key: value for key, value in actual.items() if key != 'budget'}), 'result schema'),
            (changed(result={**actual, 'received_applied': 'applied'}), 'result schema'),
            (result(errors=['x' * multiproject.MAX_OUTPUT]), 'result schema'), (result(native=None), 'native outcome'),
            (result(native={**actual['native'], 'outcome': 'applied'}), 'native outcome'), (result(native={'outcome': 'ok'}), 'native outcome'),
            (result(status='judgement_needed', report=None), 'v1 report'),
            (result(report={**actual['report'], 'observed_at': '2000-01-01T00:00:00Z'}), 'v1 report'),  # stale at its completion
            (result(report={'outcome': 'ok'}), 'v1 report'), (result(report='ok'), 'v1 report'), (changed(result=None), 'result schema'),
            (json.dumps(dropped).encode(), 'malformed'), (changed(receipt='applied'), 'malformed'), (b'[]', 'malformed'),
            (good[:len(good) // 2], 'truncated'), (b'', 'is empty'), (b' ' * (multiproject.MAX_RECORD + 1), 'oversized')]
        for broken, why in cases:
            path.write_bytes(broken)
            found = multiproject.recover(run, policy)
            self.assertEqual((found['status'], found['output'], found['received_applied']), ('unknown', None, 'unknown'), why)
            self.assertIn(why, found['errors'][0])
            self.assertEqual(path.read_bytes(), broken)  # left as it is
        path.write_bytes(good)
        os.chmod(path, 0o644)
        self.assertIn('alone', multiproject.recover(run, policy)['errors'][0])
        os.chmod(path, 0o600)
        path.rename(self.dir / 'elsewhere.json')
        path.symlink_to(self.dir / 'elsewhere.json')
        self.assertEqual(multiproject.recover(run, policy)['status'], 'unknown')
        path.unlink()
        self.assertIn('No such file', multiproject.recover(run, policy)['errors'][0])
        (self.dir / 'elsewhere.json').rename(path)
        self.assertIn('is not a run id', multiproject.recover('../anchor', policy)['errors'][0])
        os.chmod(self.records, 0o755)
        self.assertIn('alone; refused', multiproject.recover(run, policy)['errors'][0])
        os.chmod(self.records, 0o700)
        changed_policy = self.catalog(self.alpha, {**self.beta, 'limits': {'claude': 2}}, enroll=False, file='changed.toml')
        self.assertIn('differs from accepted generation 1', multiproject.recover(run, changed_policy)['errors'][0])
        # Fresh readback: unknown catalog ownership or a needed inventory refuses qualification; nothing is inferred.
        (self.dir / 'alpha').rename(self.dir / 'alpha-moved')
        found = multiproject.recover(run, policy)
        self.assertEqual((found['status'], found['output'], found['received_applied']), ('refused', None, 'unknown'))
        self.assertIn('recovery not qualified: https://gitlab.example/acme/alpha: failed', found['errors'][0])
        (self.dir / 'alpha-moved').rename(self.dir / 'alpha')
        self.agents('not json')
        self.assertIn('claude inventory unknown', multiproject.recover(run, policy)['errors'][0])
        self.agents(LIVE)
        self.assertEqual(multiproject.recover(run, policy)['status'], 'recovered')
        self.assertEqual(len(self.calls('native')), 1)

    def test_a_refusal_before_admission_recovers_as_refused_and_bounds_count_stored_bytes(self):
        policy = self.catalog(self.alpha, {**self.beta, 'limits': {'claude': 1}})
        run = multiproject.allocate_record()
        fd, _ = multiproject.guard({'os_user': os.getuid()})  # the actor meets a held guard: refused before admission
        try:
            subprocess.run(ACTOR + [json.dumps({'entry': self.beta, 'policy': str(policy), 'run': run})], cwd=self.beta['checkout'],
                           capture_output=True, timeout=120, check=True)
        finally:
            os.close(fd)
        found = multiproject.recover(run, policy)
        self.assertEqual((found['status'], found['output']['status'], found['output']['guard'], found['output']['native']),
                         ('recovered', 'refused', None, None), found)
        self.assertIn('host guard held', found['output']['errors'][0])
        self.assertEqual((self.calls('native'), self.calls('spawn')), ([], []))
        # Escaping never inflates a bounded output or diagnostics: both are counted as stored, UTF-8.
        actual = json.loads((self.records / f'{self.act([self.beta], policy)["projects"][0]["run_id"]}.json').read_text())['result']
        actual['guard']['pid'] = os.getpid()  # this process completes the record below
        loaded = multiproject.load_policy(policy)
        binding = multiproject.binding_for(self.beta, loaded[1])
        wide = {**actual, 'errors': ['é' * 400_000]}  # 0.8 MB stored, 2.4 MB if escaped
        run = multiproject.allocate_record()
        multiproject.complete_record(run, wide, *loaded, binding, '😀\x01' * 40_000 + 'newest')
        stored = json.loads((self.records / f'{run}.json').read_text())
        self.assertLessEqual(multiproject.size(stored['log']), multiproject.MAX_LOG)
        self.assertTrue(stored['log'].endswith('newest'))
        found = multiproject.recover(run, policy)
        self.assertEqual((found['status'], found['output']['errors']), ('recovered', wide['errors']), found['errors'])
        run = multiproject.allocate_record()
        with self.assertRaisesRegex(SystemExit, 'above 1 MiB'):
            multiproject.complete_record(run, {**actual, 'errors': ['é' * 600_000]}, *loaded, binding, '')
        self.assertIn('is empty', multiproject.recover(run, policy)['errors'][0])  # never partial output

    def tree(self):
        """Every path under the fixture's state folder with its inode, mode and bytes: any write shows."""
        return {str(path.relative_to(self.dir)): (path.lstat().st_ino, path.lstat().st_mode,
                                                  path.read_bytes() if path.is_file() and not path.is_symlink() else None)
                for path in [self.dir / 'taskq', *(self.dir / 'taskq').rglob('*')] if os.path.lexists(path)}

    def test_recovery_locks_the_existing_guard_read_only_and_never_recreates_missing_state(self):
        policy = self.catalog(self.alpha, {**self.beta, 'limits': {'claude': 1}})
        run = self.act([self.beta], policy)['projects'][0]['run_id']
        native = len(self.calls('native'))
        aside = self.dir / 'guard-aside'
        refusals = []

        def unknown(why):
            before = self.tree()
            found = multiproject.recover(run, policy)
            self.assertEqual((found['status'], found['output'], found['received_applied']), ('unknown', None, 'unknown'), why)
            self.assertEqual(self.tree(), before, why)  # no folder, file, inode, mode or byte changed
            refusals.append(found['errors'][0])
        self.guard.rename(aside)
        unknown('missing guard')
        self.assertFalse(os.path.lexists(self.guard))
        self.guard.symlink_to(aside)
        unknown('symlinked guard')
        self.assertTrue(self.guard.is_symlink())
        self.guard.unlink()
        aside.rename(self.guard)
        os.chmod(self.guard, 0o666)
        unknown('guard writable by others')
        os.chmod(self.guard, 0o600)
        os.chmod(self.guard.parent, 0o775)
        unknown('guard folder writable by the group')
        os.chmod(self.guard.parent, 0o700)
        real = os.getuid()
        with patch.object(multiproject.os, 'getuid', lambda: real + 1):
            unknown('another OS user')
        self.assertIn('No such file', refusals[0])
        self.assertIn('Too many levels of symbolic links', refusals[1])
        self.assertIn('alone; refused', refusals[2])
        state = self.dir / 'taskq'
        state.rename(self.dir / 'state-aside')  # the guard's folder and every record gone
        found = multiproject.recover(run, policy)
        self.assertEqual((found['status'], found['output'], os.path.lexists(state)), ('unknown', None, False))
        with self.assertRaises(FileNotFoundError):  # the read-only lock itself creates no folder either
            multiproject.guard({'os_user': os.getuid()}, create=False)
        self.assertFalse(os.path.lexists(state))
        (self.dir / 'state-aside').rename(state)
        # Recovery holds the guard through its fresh readback: a racing actor is refused, no native call.
        reader, raced = multiproject.read_occupancy, []

        def racing(binding, errors):
            if not raced:
                raced.append(self.act([self.alpha], policy)['projects'][0])
            return reader(binding, errors)
        with patch.object(multiproject, 'read_occupancy', racing):
            found = multiproject.recover(run, policy)
        self.assertEqual((found['status'], found['received_applied']), ('recovered', 'unknown'), found['errors'])
        self.assertEqual(raced[0]['status'], 'refused')
        self.assertIn('host guard held', raced[0]['errors'][0])
        self.assertEqual(len(self.calls('native')), native)
        self.assertEqual(multiproject.guard_free({'os_user': os.getuid()}), None)  # released at its end

    def test_record_limit_refuses_the_next_actor_before_native_mutation_and_deletes_nothing(self):
        policy = self.catalog(self.alpha, {**self.beta, 'limits': {'claude': 1}})
        self.records.mkdir(mode=0o700)
        for n in range(multiproject.MAX_RECORDS - 1):  # unresolved records of earlier runs, empty or not
            (self.records / f'{n:032x}.json').write_bytes(b'' if n % 2 else b'{}')
        before = sorted(os.listdir(self.records))
        result = self.act([self.alpha, self.beta], policy)
        self.assertEqual([project['status'] for project in result['projects']], ['ok', 'refused'], result)
        self.assertIn(f'32 records at the limit {multiproject.MAX_RECORDS}; recover them', result['projects'][1]['errors'][0])
        self.assertEqual(len(self.calls('native')), 1)
        self.assertEqual(len(os.listdir(self.records)), multiproject.MAX_RECORDS)
        self.assertTrue(set(before) <= set(os.listdir(self.records)))  # nothing evicted
        self.assertEqual(self.mutations('beta'), [])

    def test_differing_principals_and_enrollment_contend_on_one_host_inode(self):
        first = self.project('first', 101, tasks=[(1,)], spawn={'sleep': 4})
        other = self.project('other', 102, uid=2, tasks=[(1,)])
        policy = self.catalog({**first, 'limits': {'claude': 1}}, {**other, 'principal': 2}, self.beta)
        before = self.anchor.read_bytes()
        manifest = self.dir / 'first.toml'
        manifest.write_text(toml({'version': 1}, [first]))
        wrapper = subprocess.Popen(ACT + ['wrapper', '--manifest', str(manifest), '--act', '--execution-policy', str(policy), '--json'],
                                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.wait(lambda: self.calls('spawn'))
        refused = self.act([other, self.beta], policy)
        self.assertEqual([project['status'] for project in refused['projects']], ['refused', 'not_admitted'])
        self.assertIn('host guard held', refused['projects'][0]['errors'][0])
        racing = multiproject.accept(self.catalog({**first, 'limits': {'claude': 1}}, {**other, 'principal': 2}, enroll=False, file='next.toml'), CHECK)
        self.assertIn('host guard held', racing['errors'][0])
        self.assertEqual(self.anchor.read_bytes(), before)
        out, _ = wrapper.communicate(timeout=60)
        later = self.act([other], policy)['projects'][0]
        self.assertEqual(later['status'], 'ok', later)
        self.assertEqual(later['guard']['inode'], json.loads(out)['projects'][0]['guard']['inode'])
        self.assertEqual([row[2] for row in self.calls('spawn')], ['acme/first'])
        fd, _ = multiproject.guard({'os_user': os.getuid()})  # an enrollment holding the guard keeps every actor out
        try:
            self.assertIn('host guard held', self.act([self.beta], policy)['projects'][0]['errors'][0])
        finally:
            os.close(fd)

    def test_unsupported_guard_domain_is_refused_without_permission_changes(self):
        with self.assertRaisesRegex(SystemExit, 'outside the catalog'):
            multiproject.guard({'os_user': os.getuid() + 1})
        self.assertFalse(self.guard.exists())
        self.guard.touch()
        os.chmod(self.guard, 0o666)
        inode = self.guard.stat().st_ino
        with self.assertRaisesRegex(SystemExit, 'alone; refused, no permission changed'):
            multiproject.guard({'os_user': os.getuid()})
        self.assertEqual((self.guard.stat().st_mode & 0o777, self.guard.stat().st_ino), (0o666, inode))
        result = self.act([self.alpha, self.beta], self.catalog(self.alpha, self.beta, os_user=os.getuid() + 1, enroll=False))
        self.assertEqual([project['status'] for project in result['projects']], ['refused', 'refused'])
        self.assertEqual(self.calls(), [])

    def test_one_entry_view_acts_like_the_same_project_in_a_larger_view(self):
        idle = self.project('idle', 111)
        policy = self.catalog(idle, {**self.beta, 'limits': {'claude': 1}})
        before = (self.dir / 'acme_beta.pickle').read_bytes()
        alone = self.act([self.beta], policy)['projects'][0]
        (self.dir / 'acme_beta.pickle').write_bytes(before)
        self.agents(LIVE)
        together = self.act([idle, self.beta], policy)['projects'][1]
        self.assertEqual((alone['status'], together['status']), ('ok', 'ok'))
        self.assertEqual(alone['budget'], together['budget'])
        self.assertEqual(self.limits(alone)['claude'], (7, True, 1, 0, 1))
        self.assertEqual(alone['native']['actions'][0]['action'], together['native']['actions'][0]['action'])

    def claim(self, name, iid, state, session):
        """A retained same-host claim of `session` on task `iid`, its worker in no inventory."""
        store = self.store(name)
        store.issues[iid]['labels'] = [f'q-{state}', 'code', 'run-claude']
        store.issues[iid]['description'] = core.render('goal', {**self.block(name, iid),
                                                                'claim': {'runtime': 'claude', 'session': session, 'node': self.node(name)}})
        (self.dir / f'acme_{name}.pickle').write_bytes(pickle.dumps(store))

    def test_caps_and_limits_are_explicit_counts_up_to_the_validation_bound_and_default_to_8_and_4(self):
        for caps in ({'claude': 2, 'codex': 6}, {'claude': 0, 'codex': 0}, {'claude': 64, 'codex': 64}, {'codex': 6}):
            policy, _ = multiproject.load_policy(self.catalog({**self.alpha, 'limits': {'claude': 2, 'codex': 6}}, enroll=False, caps=caps))
            self.assertEqual(policy['caps'], {**multiproject.CAPS, **caps})
        for bad in ({'claude': -1}, {'claude': True}, {'codex': 1.5}, {'codex': '6'}, {'gemini': 1}, {'claude': 65}):
            with self.assertRaisesRegex(SystemExit, 'caps: write', msg=bad):
                multiproject.load_policy(self.catalog(self.alpha, enroll=False, file='bad.toml', caps=bad))
            _, [(_, errors)] = multiproject.load_policy(self.catalog({**self.alpha, 'limits': bad}, enroll=False, file='bad.toml'))
            self.assertIn('limits: write { claude = 0..64, codex = 0..64 }', errors, bad)
        self.catalog(self.alpha)  # no caps: the anchor keeps the 8/4 default it always had
        self.assertEqual(json.loads(self.anchor.read_text())['catalog']['policy']['caps'], {'claude': 8, 'codex': 4})

    def test_caps_only_replacement_keeps_retained_claims_and_over_cap_refuses_before_native_pass(self):
        self.catalog(self.alpha, self.beta)
        self.claim('alpha', 1, 'review', 'owned-review')
        self.claim('alpha', 2, 'doing', 'bg-1')  # a live worker of alpha: L = 1
        target = self.catalog(self.alpha, self.beta, enroll=False, file='target.toml', caps={'claude': 2, 'codex': 6})
        self.assertIn('codex inventory unknown: its cap change cannot be checked', multiproject.accept(target, CHECK)['errors'])
        with patch.object(multiproject, 'codex_threads', lambda: []):
            found = multiproject.accept(target, CHECK)
            self.assertEqual((found['status'], found['generation'], found['errors']), ('replaced', 2, []))
            anchor = json.loads(self.anchor.read_text())
            self.assertEqual((anchor['generation'], anchor['sha256'], anchor['catalog']['policy']['caps']),
                             (2, multiproject.digest(anchor['catalog']), {'claude': 2, 'codex': 6}))
            self.assertEqual(multiproject.accept(target, CHECK)['status'], 'unchanged')
        self.assertEqual([self.block('alpha', iid)['claim']['session'] for iid in (1, 2)], ['owned-review', 'bg-1'])
        # Seven live rows and the review claim: occupancy 8 above the cap 2 (and 0). Refused before the native pass:
        # nobody new, nobody stopped, nothing released, cleaned or written.
        zero = self.catalog(self.alpha, self.beta, enroll=False, file='zero.toml', caps={'claude': 0, 'codex': 6})
        for policy in (target, zero):
            if policy is zero:
                self.assertEqual(multiproject.accept(zero, CHECK)['status'], 'replaced')
            before = [self.anchor.read_bytes()] + [(self.dir / f'acme_{name}.pickle').read_bytes() for name in ('alpha', 'beta')]
            result = self.act([self.alpha, self.beta], policy)
            for project, L in zip(result['projects'], (1, 0)):
                self.assertEqual((project['status'], self.limits(project)['claude'][:2], self.limits(project)['claude'][3]), ('refused', (8, True), L))
                self.assertIn(f'claude occupancy 8 above cap {0 if policy is zero else 2}', project['errors'][0])
            self.assertEqual([self.anchor.read_bytes()] + [(self.dir / f'acme_{name}.pickle').read_bytes() for name in ('alpha', 'beta')], before)
            self.assertEqual([row[0:2] for row in self.calls() if row[1] in ('native', 'spawn', 'cleanup', 'stop', 'wake', 'timer')], [])
        self.assertEqual((self.mutations('alpha'), self.mutations('beta')), ([], []))
        # At the cap exactly the pass runs with no new room; a raised cap grants only cap - fresh occupancy.
        for caps, room in (({'claude': 8, 'codex': 6}, 0), ({'claude': 11, 'codex': 6}, 2)):
            policy = self.catalog(self.alpha, self.beta, enroll=False, file=f'cap-{caps["claude"]}.toml', caps=caps)
            self.assertEqual(multiproject.accept(policy, CHECK)['status'], 'replaced')  # codex unchanged: its inventory not needed
            found = self.act([self.beta], policy)['projects'][0]
            self.assertEqual((found['status'], self.limits(found)['claude']), ('ok', (8, True, caps['claude'] - 8, 0, room)))
        self.assertEqual([row[3] for row in self.calls('native')], [{'claude': 0, 'codex': 0}, {'claude': 2, 'codex': 0}])
        self.assertEqual([self.block('alpha', iid)['claim']['session'] for iid in (1, 2)], ['owned-review', 'bg-1'])

    def test_over_cap_refuses_before_native_pass_for_either_runtime_with_L(self):
        policy = self.catalog({**self.alpha, 'limits': {'claude': 2, 'codex': 2}}, caps={'claude': 2, 'codex': 2})
        for runtime in multiproject.CAPS:
            read = {'status': 'ok', 'held': [([runtime], 'session:a')], 'inactive': [], 'protected': [], 'uncertain': [],
                    'L': {'claude': 0, 'codex': 0, runtime: 1}}
            live = ['a', 'b', 'c']
            rows = [{'sessionId': session, 'state': 'working'} for session in live] if runtime == 'claude' else []
            threads = [{'id': session, 'status': {'type': 'active'}} for session in live] if runtime == 'codex' else []
            with patch.multiple(multiproject, verify=lambda entry: [], workflow=lambda binding, machine: [], update_due=lambda: [],
                                read_occupancy=lambda binding, errors: read, claude_rows=lambda: rows, codex_threads=lambda: threads), \
                    patch.object(multiproject, 'native_pass') as native:
                found = multiproject.act_project({'entry': self.alpha, 'policy': str(policy)})
            self.assertEqual((found['status'], native.call_count), ('refused', 0), runtime)
            self.assertEqual((found['budget'][runtime]['occupancy'], found['budget'][runtime]['L'], found['budget'][runtime]['F']), (3, 1, 0))
            self.assertIn(f'{runtime} occupancy 3 above cap 2', found['errors'][0])

    def test_caps_only_replacement_refuses_unknown_ownership_or_a_held_guard_and_keeps_the_anchor(self):
        self.catalog(self.alpha, self.beta)
        before = self.anchor.read_bytes()
        lower = self.catalog(self.alpha, self.beta, enroll=False, file='lower.toml', caps={'claude': 2})
        (self.dir / 'beta').rename(self.dir / 'beta-moved')
        self.assertIn('https://gitlab.example/acme/beta: ownership unknown', ' '.join(multiproject.accept(lower, CHECK)['errors']))
        (self.dir / 'beta-moved').rename(self.dir / 'beta')
        self.edit('alpha', iid=2, lock=True)
        self.assertIn('tracker lock without a claim or reservation', ' '.join(multiproject.accept(lower, CHECK)['errors']))
        self.edit('alpha', iid=1, reservation={'attempt': 'a1', 'runtime': 'claude', 'principal': 1, 'node': 'xyz'})
        self.assertIn('without a proven machine', ' '.join(multiproject.accept(lower, CHECK)['errors']))
        self.assertEqual(self.anchor.read_bytes(), before)
        fd, _ = multiproject.guard({'os_user': os.getuid()})  # an actor holding the host guard
        try:
            self.assertIn('host guard held', multiproject.accept(lower, CHECK)['errors'][0])
        finally:
            os.close(fd)
        self.agents('not json')
        self.assertIn('claude inventory unknown: its cap change cannot be checked', multiproject.accept(lower, CHECK)['errors'])
        self.assertEqual((self.anchor.read_bytes(), self.calls('native'), self.mutations('alpha')), (before, [], []))

    def test_caps_with_any_binding_change_still_needs_full_settlement_of_retained_grants(self):
        self.catalog(self.alpha, self.beta)
        before = self.anchor.read_bytes()
        self.claim('alpha', 1, 'review', 'owned-review')
        caps = {'caps': {'claude': 2, 'codex': 6}}
        for n, bindings in enumerate(([self.beta], [{**self.alpha, 'principal': 2}, self.beta], [{**self.alpha, 'act': False}, self.beta],
                                      [{**self.alpha, 'effects': ['queue', 'cleanup', 'idle_stop', 'queue']}, {**self.beta, 'limits': {'claude': 1}}],
                                      [{**self.alpha, 'timeout': 60}, self.beta])):
            with patch.object(multiproject, 'codex_threads', lambda: []):
                refused = multiproject.accept(self.catalog(*bindings, enroll=False, file=f'changed-{n}.toml', **caps), CHECK)
            self.assertIn('https://gitlab.example/acme/alpha: still owns session:owned-review', refused['errors'], bindings)
            self.assertEqual(self.anchor.read_bytes(), before)
        self.assertEqual((self.block('alpha', 1)['claim']['session'], self.mutations('alpha')), ('owned-review', []))


def act_child():
    """The acting fixture: the real wrapper, actor or occupancy reader (argv[2]). Each project's tracker is
    test_taskq's in-memory GitLab, pickled between processes; `claude agents --json --all` is agents.json; launches,
    doctor, cleanup, timer, retire and wake are logged fakes; no Codex app server. Every call goes to calls.jsonl."""
    import test_taskq as base
    role, fixture = sys.argv[2], Path(os.environ['TASKQ_FIXTURE'])
    core.machine_id, core.MEMBERS, core.CODEX_SOCKET = base.REAL_MACHINE_ID, None, fixture / 'no-codex.sock'
    core.CLAUDE_APP_SESSIONS = fixture / 'app-sessions'  # local app evidence of a claim: the fixture's, never the host's
    agents = fixture / 'agents.json'

    def write(*row):
        with (fixture / 'calls.jsonl').open('a') as out:
            out.write(json.dumps([role, *row]) + '\n')

    def settings():
        return json.loads(Path('fixture.json').read_text()) if Path('fixture.json').is_file() else {}

    def api(method, path, body=None):
        stored = fixture / f'{core.PROJECT_PATH.replace("/", "_")}.pickle'
        store = pickle.loads(stored.read_bytes())
        write('api', core.PROJECT_PATH, method, path)
        if path == '/' + core.PROJECT:
            return {'id': store.repo_id}
        if method == 'POST' and 'award_emoji' in path and settings().get('lock_fails'):
            raise SystemExit('fixture: tracker write refused')
        found = store(method, path, body)
        if method != 'GET':
            stored.write_bytes(pickle.dumps(store))
        return found

    def rows():
        try:
            return json.loads(agents.read_text())
        except ValueError:
            return None

    def doctor(args):
        if settings().get('doctor'):
            print(f'not ready: 1 gap(s); each line is the command that closes it\n- {settings()["doctor"]}')
            sys.exit(1)
        print('ready')

    def spawn(name, extra=None, prompt=None, remote_control=True):
        hook = settings().get('spawn', {})
        probe = subprocess.run([sys.executable, '-c', PROBE, str(multiproject.guard_path())], capture_output=True, text=True).stdout.strip()
        write('spawn', core.PROJECT_PATH, name, probe, os.getpid())
        if hook.get('crash'):
            os._exit(9)
        if hook.get('descendant'):
            subprocess.Popen([sys.executable, '-c', DESCENDANT, str(multiproject.guard_path()), str(fixture / 'descendant.json')],
                             close_fds=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(hook.get('sleep', 0))
        found = rows()
        session = f'w-{core.PROJECT_PATH.split("/")[-1]}-{len(found)}'
        agents.write_text(json.dumps(found + [{'id': session[:8], 'cwd': str(core.ROOT), 'kind': 'background', 'sessionId': session,
                                               'name': name, 'state': 'working', 'status': 'busy', 'pid': os.getpid()}]))
        return session
    core.api, multiproject.claude_rows, core.doctor, worker.claude_spawn = api, rows, doctor, spawn
    core.claude_agents = lambda strict=False: (None if strict else {}) if rows() is None else {
        row['sessionId']: row for row in rows() if row.get('kind') == 'background'}
    native = multiproject.native_pass

    def logged(view, limits):
        write('native', core.PROJECT_PATH, limits)
        return native(view, limits)
    multiproject.native_pass = logged
    sys.modules['taskq.cleanup'].scheduled = lambda args: write('cleanup')
    tick.timer = lambda install: write('timer', install)
    core.claude_stop = lambda session, remove=False: write('stop', session)
    core.claude_wake = lambda session, prompt, extra=None: write('wake', session)
    multiproject.ACTOR, multiproject.OCCUPANCY, multiproject.CHECK = ACTOR, ACT + ['occupancy', '--occupancy'], CHECK
    write('start', os.getpid())
    multiproject.main(sys.argv[3:])


if __name__ == '__main__' and sys.argv[1:2] == ['--child']:
    child()
elif __name__ == '__main__' and sys.argv[1:2] == ['--act-child']:
    act_child()
elif __name__ == '__main__':
    unittest.main()
