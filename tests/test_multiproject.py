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
                                                  'CLAUDE_CODE_SESSION_ID': 'coordinator', 'CODEX_THREAD_ID': ''}))
        self.enterContext(patch.object(core, 'UPDATE_STAMP', self.dir / 'taskq' / 'update-last'))
        # In-process enrollment reads back through the fixture's subprocesses, never a real tracker.
        self.enterContext(patch.multiple(multiproject, OCCUPANCY=ACT + ['occupancy', '--occupancy'], CHECK=CHECK, ACTOR=ACTOR,
                                         claude_rows=lambda: json.loads((self.dir / 'agents.json').read_text())))
        self.enterContext(patch.object(core, 'CODEX_SOCKET', self.dir / 'no-codex.sock'))  # never the host's runtimes
        self.guard, self.anchor = multiproject.guard_path(), multiproject.anchor_path()
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
        # Six live rows of any kind; F = 8 - 6 = 2 for alpha. Its two launches make beta's occupancy 8: F = 0.
        self.assertEqual(self.limits(alpha), {'claude': (6, True, 2, 0, 2), 'codex': (0, False, 0, 0, 0)})
        self.assertEqual(self.limits(beta), {'claude': (8, True, 0, 0, 0), 'codex': (0, False, 0, 0, 0)})
        spawned = self.calls('spawn')
        self.assertEqual([(row[2], row[3]) for row in spawned], [('acme/alpha2', 'T1 Task 1 (fixture-host)'), ('acme/alpha2', 'T2 Task 2 (fixture-host)')])
        self.assertEqual({row[4] for row in spawned}, {'held'})  # the host guard is held during the native mutation
        self.assertEqual([row[3] for row in self.calls('native')], [{'claude': 2, 'codex': 0}, {'claude': 0, 'codex': 0}])
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
        self.assertIn('claude=2 (L 0 + F 2, occupancy 6/8', multiproject.render(result))

    def test_budget_counts_exact_sessions_once_and_unknowns_hold_every_runtime(self):
        self.assertEqual(multiproject.claude_inventory(LIVE), {'session:owner-chat', 'session:bg-0', 'session:bg-1', 'session:bg-2',
                                                               'session:bg-3', 'session:waiting-owner'})
        self.assertEqual(multiproject.claude_inventory([{'kind': 'interactive', 'pid': 7}]), {'pid:7'})
        self.assertIsNone(multiproject.claude_inventory([{'kind': 'background', 'state': 'working'}]))  # unidentified live row
        self.assertIsNone(multiproject.claude_inventory({'rows': []}))
        self.assertEqual(multiproject.codex_inventory([{'id': 'a', 'status': {'type': 'active'}}, {'id': 'b', 'status': {'type': 'idle'}},
                                                       {'id': 'c', 'name': 'T4 x', 'status': {'type': 'notLoaded'}},
                                                       {'id': 'd', 'name': 'T5 y', 'status': {'type': 'systemError'}}]), {'session:a', 'session:c'})
        caps = {'claude': 8, 'codex': 4}
        own = {'status': 'ok', 'L': {'claude': 1, 'codex': 0},
               'held': [(['claude'], 'session:s1'), (['claude'], 'reservation:r'), (['claude', 'codex'], 'lock:l')]}
        seven = {f'session:s{n}' for n in range(1, 8)}
        found = multiproject.budget(caps, {'claude': seven, 'codex': None}, [own], own, {'claude': 3, 'codex': 2})
        # s1 is one exact session; the reservation without one and the ownerless lock stay separate: 9 > 8, F clamps at 0.
        self.assertEqual((found['claude']['occupancy'], found['claude']['F'], found['claude']['limit']), (9, 0, 1))
        self.assertEqual((found['codex']['known'], found['codex']['limit']), (False, 0))
        small = multiproject.budget(caps, {'claude': {'session:s1'}, 'codex': set()}, [own], own, {'claude': 3, 'codex': 2})
        self.assertEqual((small['claude']['F'], small['claude']['limit'], small['codex']['F'], small['codex']['limit']), (5, 3, 3, 2))

    def test_removed_view_reads_the_old_anchored_binding(self):
        claim = lambda session: {'claim': {'runtime': 'claude', 'session': session, 'node': self.node('gone')}}  # noqa: E731
        gone = self.project('gone', 44, tasks=[(1, 'doing', 'claude', claim('s-1')), (2, 'doing', 'claude', claim('s-2')),
                                               (3, 'doing', 'claude', claim('bg-0'))])
        policy = self.catalog({**gone, 'act': False}, self.beta)
        result = self.act([self.beta], policy)
        self.assertEqual(self.limits(result['projects'][0])['claude'], (8, True, 0, 0, 0))  # 6 rows + s-1, s-2; bg-0 once
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
                                                 (self.alpha, {'machine': 'other-host'}, "the catalog binds 'other-host'"),
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
        foreign = json.loads(good)
        foreign['catalog']['policy']['os_user'] = os.getuid() + 1
        foreign['sha256'] = multiproject.digest(foreign['catalog'])
        self.anchor.write_text(json.dumps(foreign))
        self.assertIn('OS-user domain', self.act([self.alpha], policy)['projects'][0]['errors'][0])
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
        for top in ({'evidence': 'doctor ok'}, {'caps': {'claude': 9}}):
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
        self.assertTrue(self.block('slow', 1)['reservation'])  # the kept actor finished its launch
        again = self.act([self.beta], policy)['projects'][0]
        self.assertEqual((again['status'], self.limits(again)['claude']), ('ok', (7, True, 1, 0, 1)))  # slow's worker counts

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

    def test_crash_frees_the_lock_keeps_the_inode_and_restart_counts_the_orphan_without_releasing_it(self):
        crash = self.project('crash', 91, tasks=[(1,)], spawn={'crash': True})
        policy = self.catalog({**crash, 'limits': {'claude': 1}}, self.beta)
        inode = self.guard.stat().st_ino
        result = self.act([crash, self.beta], policy)
        self.assertEqual([project['status'] for project in result['projects']], ['unknown', 'not_admitted'])
        self.assertIn('actor exit 9 without a result', result['projects'][0]['errors'][0])
        self.assertEqual((multiproject.guard_free({'os_user': os.getuid()}), self.guard.stat().st_ino), (None, inode))
        orphan, written = self.block('crash', 1)['reservation'], self.mutations('crash')
        again = self.act([self.beta], policy)['projects'][0]
        self.assertEqual((again['status'], self.limits(again)['claude']), ('ok', (7, True, 1, 0, 1)))  # the orphan holds a place
        self.assertEqual((self.block('crash', 1)['reservation'], self.mutations('crash')), (orphan, written))  # read back, not released

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
        self.assertEqual([row[2] for row in self.calls('spawn')], ['acme/first', 'acme/other'])
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
        self.assertEqual(self.limits(alone)['claude'], (6, True, 2, 0, 1))
        self.assertEqual(alone['native']['actions'][0]['action'], together['native']['actions'][0]['action'])


def act_child():
    """The acting fixture: the real wrapper, actor or occupancy reader (argv[2]). Each project's tracker is
    test_taskq's in-memory GitLab, pickled between processes; `claude agents --json --all` is agents.json; launches,
    doctor, cleanup, timer, retire and wake are logged fakes; no Codex app server. Every call goes to calls.jsonl."""
    import test_taskq as base
    role, fixture = sys.argv[2], Path(os.environ['TASKQ_FIXTURE'])
    core.machine_id, core.MEMBERS, core.CODEX_SOCKET = base.REAL_MACHINE_ID, None, fixture / 'no-codex.sock'
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
