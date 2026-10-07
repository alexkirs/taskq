"""Read-only multiproject observation (#186 stage 2) over two fixture repositories; each reader is a real subprocess
whose tracker is a fake that logs every call, so no network and no mutation can pass unseen."""
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import taskq as core  # noqa: E402
import taskq.multiproject as multiproject  # noqa: E402

tick = sys.modules['taskq.tick']
CHILD = [sys.executable, str(Path(__file__).resolve()), '--child']


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


if __name__ == '__main__' and sys.argv[1:2] == ['--child']:
    child()
elif __name__ == '__main__':
    unittest.main()
