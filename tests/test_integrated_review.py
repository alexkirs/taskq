"""Native guarded integration association over real Git objects; no provider/model calls."""
import ast
import hashlib
import json
import os
import subprocess
from pathlib import Path
from unittest import mock

from tests.test_single import Base, REAL_RUN, REAL_POPEN, SESSION, taskq

VERDICT = 'Accepted mapped onboarding; shared bootstrap is deployed; open: other providers remain unqualified'


class IntegratedReview(Base):
    def setUp(self):
        super().setUp()
        self.git = taskq.shutil.which('git')
        if not self.git:
            self.skipTest('native git unavailable')
        taskq.CONFIG.update(repo='fixture/project', host='github.com', workspace='external', publish='pr')
        self.git_env = {**os.environ, 'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': os.devnull,
                        'GIT_AUTHOR_NAME': 'Fixture', 'GIT_AUTHOR_EMAIL': 'fixture@example.invalid',
                        'GIT_COMMITTER_NAME': 'Fixture', 'GIT_COMMITTER_EMAIL': 'fixture@example.invalid'}
        self.native('init', '-b', 'main')
        self.native('config', 'core.autocrlf', 'false')
        self.native('remote', 'add', 'origin', 'https://github.com/fixture/project.git')
        (self.root / 'taskq.py').write_bytes(b'def bootstrap():\n    return 0\n')
        self.native('add', '.')
        self.native('commit', '-m', 'base')
        self.native('checkout', '-b', 'taskq-1')
        self.source = 'def bootstrap():\n    return 1\n'
        (self.root / 'taskq.py').write_bytes(self.source.encode('utf-8'))
        self.native('commit', '-am', 'original result')
        self.original = self.native('rev-parse', 'HEAD').strip()
        self.native('checkout', 'main')
        (self.root / 'taskq.py').write_bytes(self.source.encode('utf-8'))
        self.map_path = 'tests/integration.json'
        (self.root / 'tests').mkdir()
        node = ast.parse(self.source).body[0]
        self.mapping = {'type': 'taskq.integration.source-map', 'version': 1, 'source_task': 1,
                        'source_sha': self.original, 'integrating_task': 2, 'same_source_functions': True,
                        'functions': {'bootstrap': hashlib.sha256(ast.get_source_segment(self.source, node).encode()).hexdigest()}}
        self.publish()
        worker = {'runtime': 'codex', 'session': 'original-worker', 'name': 'win'}
        supervisor = {'runtime': 'codex', 'session': 'original-supervisor', 'name': 'win'}
        self.identity = taskq.origin()
        submitted = {'id': 1, 'by': 'codex:original', 'text': 'original result'}
        raw = {'event_schema': 3, 'event_seq': 2, 'scope': [], 'deps': [], 'claim': worker,
               'supervisor': supervisor, 'pm': self.identity, 'order': None,
               'result': {'sha': self.original, 'checks': 'retained original checks'},
               'action_payloads': {'result': submitted}, 'application_receipts': {'10': {'retained': True}},
               'decision': {'summary': 'held result', 'options': [], 'links': [], 'recommend': 1},
               'events': []}
        self.board.add('onboarding', taskq.block('goal', raw), ['q-ask', 'task'])
        self.integration_raw = {'event_schema': 3, 'event_seq': 2, 'result': {'sha': self.published},
                                'pm': self.identity, 'scope': [], 'deps': [],
                                'action_payloads': {'result': submitted}, 'events': []}
        self.set_integration()
        self.board.add('shared integration', taskq.block('goal', self.integration_raw), ['task'])
        self.board.issues[2]['state'] = 'closed'
        patcher = mock.patch.object(taskq.subprocess, 'run', side_effect=self.git_read)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.dispatch = mock.patch.object(taskq, 'dispatch').start()
        self.addCleanup(mock.patch.stopall)
        self.before = taskq.issue_data(self.board.get(1))
        self.revision = taskq.question_revision(self.task(1))

    def native(self, *args):
        with mock.patch.object(taskq.subprocess, 'Popen', REAL_POPEN):
            done = REAL_RUN([self.git, '-C', str(self.root), *args], capture_output=True,
                            env=self.git_env, text=True, encoding='utf-8')
        if done.returncode:
            self.fail(done.stderr)
        return done.stdout

    def git_read(self, argv, **kwargs):
        self.assertEqual(argv[:3], [self.git, '-C', str(self.root)])
        self.assertIn(argv[3], ('remote', 'fetch', 'show', 'rev-parse', 'merge-base', 'ls-tree'))
        if argv[3:] == ['fetch', 'origin']:
            return subprocess.CompletedProcess(argv, 0, b'', b'')  # remote transport boundary only
        with mock.patch.object(taskq.subprocess, 'Popen', REAL_POPEN):
            return REAL_RUN(argv, **kwargs, env=self.git_env)

    def publish(self, message='shared integration\n\nTask: https://board/1\nTask: https://board/2'):
        (self.root / self.map_path).write_bytes(json.dumps(self.mapping).encode('utf-8'))
        self.native('add', '.')
        self.native('commit', '--allow-empty', '-m', message)
        self.published = self.native('rev-parse', 'HEAD').strip()
        self.native('update-ref', 'refs/remotes/origin/main', self.published)
        if hasattr(self, 'integration_raw'):
            self.integration_raw['result'] = {'sha': self.published}
            self.set_integration()
            self.board.issues[2]['body'] = taskq.block('goal', self.integration_raw)

    def set_integration(self):
        self.integration_raw['action'] = {'id': 2, 'action': 'close', 'by': f'{self.identity["runtime"]}:{SESSION[:8]}',
                                          'text': f'{VERDICT}\n\nmerged {self.published}', 'recipients': [], 'acks': []}

    def review(self, **overrides):
        values = {'published_sha': self.published, 'integration_task': 2, 'source_map': self.map_path,
                  'question_revision': self.revision, 'command_id': 'integration-1', 'text': VERDICT, **overrides}
        argv = ['review-integrated', '1']
        for key, value in values.items():
            argv += ['--' + key.replace('_', '-'), str(value)]
        return json.loads(self.run_cli(*argv))

    def edit_raw(self, n=1, **fields):
        self.board.issues[n]['body'] = taskq.block('goal', {**taskq.issue_data(self.board.get(n)), **fields})

    def refused(self, pattern, **overrides):
        before = self.board.get(1)
        with self.assertRaisesRegex(SystemExit, pattern):
            self.review(**overrides)
        self.assertEqual(before, self.board.get(1))
        self.dispatch.assert_not_called()

    def test_associate_close_and_exact_replays_preserve_original_without_publication(self):
        with mock.patch.object(taskq, 'merge', side_effect=AssertionError('original PR must not merge')), \
                mock.patch.object(taskq, 'publish_direct', side_effect=AssertionError('no publication')):
            first = self.review()
            self.assertFalse(first['repeated'])
            raw = self.task(1)['raw']
            for key in (*taskq.FIELDS, 'application_receipts'):
                self.assertEqual(raw.get(key), self.before.get(key))
            self.assertEqual(self.task(1)['state'], 'review')
            self.assertTrue(all(not target.startswith('worker:') for target in raw['action']['recipients']))
            self.dispatch.assert_not_called()
            self.assertTrue(self.review()['repeated'])
            self.run_cli('close', '1', '--text', VERDICT)
            self.assertEqual(self.board.get(1)['state'], 'closed')
            closed = self.board.get(1)
            self.dispatch.reset_mock()
            replay = json.loads(self.run_cli('close', '1', '--text', VERDICT))
            self.assertTrue(replay['repeated'])
            self.assertTrue(self.review()['repeated'])
            self.assertEqual(closed, self.board.get(1))
            self.dispatch.assert_not_called()
            self.assertEqual(taskq.issue_data(closed)['result'], self.before['result'])
            self.assertNotEqual(self.original, self.published)

    def test_stale_question_and_foreign_reviewer_refuse(self):
        self.refused('stale integrated', question_revision='0' * 64)
        with mock.patch.object(taskq, 'session', return_value={'runtime': 'codex', 'session': 'foreign'}):
            self.refused('recorded reviewer')

    def test_wrong_source_and_map_function_refuse(self):
        self.mapping['source_sha'] = '0' * 40
        self.publish()
        self.refused('mapping mismatch')
        self.mapping['source_sha'] = self.original
        self.mapping['functions']['bootstrap'] = '0' * 64
        self.publish()
        self.refused('function source differs')

    def test_empty_and_duplicate_key_map_refuse(self):
        self.mapping['functions'] = {}
        self.publish()
        self.refused('invalid function mapping')
        (self.root / self.map_path).write_bytes(b'{"version":1,"version":1}')
        self.native('add', '.')
        self.native('commit', '-m', 'duplicate map\n\nTask: https://board/1\nTask: https://board/2')
        self.published = self.native('rev-parse', 'HEAD').strip()
        self.native('update-ref', 'refs/remotes/origin/main', self.published)
        self.integration_raw['result'] = {'sha': self.published}
        self.set_integration()
        self.board.issues[2]['body'] = taskq.block('goal', self.integration_raw)
        self.refused('duplicate map keys')

    def test_published_commit_not_on_main_refuses(self):
        self.native('update-ref', 'refs/remotes/origin/main', self.original)
        self.refused('provenance git merge-base failed')

    def test_native_close_and_tree_provenance_required(self):
        self.board.issues[2]['state'] = 'open'
        self.refused('already be natively closed')
        self.board.issues[2]['state'] = 'closed'
        self.edit_raw(2, result={'sha': self.original})
        self.refused('trees differ')

    def test_integrating_result_can_be_squashed_only_with_identical_tree(self):
        tree = self.native('rev-parse', f'{self.published}^{{tree}}').strip()
        alternate = self.native('commit-tree', tree, '-p', self.original, '-m', 'integration candidate').strip()
        self.assertNotEqual(alternate, self.published)
        self.edit_raw(2, result={'sha': alternate})
        self.assertFalse(self.review()['repeated'])

    def test_native_close_wrong_commit_or_reviewer_refuses(self):
        event = dict(self.integration_raw['action'], text=f'merged {self.original}')
        self.edit_raw(2, action=event)
        self.refused('already be natively closed')
        self.edit_raw(2, action={**self.integration_raw['action'], 'by': 'codex:foreign'})
        self.refused('reviewer authority')

    def test_origin_and_attribution_mismatch_refuse(self):
        self.native('remote', 'set-url', 'origin', 'https://github.com/other/repo.git')
        self.refused('origin differs')
        self.native('remote', 'set-url', 'origin', 'https://github.com/fixture/project.git')
        self.publish(message='missing attribution')
        self.refused('message needs Task')

    def test_safe_regular_map_path_required(self):
        for path in ('../map.json', '/map.json', 'tests/./integration.json', 'tests\\map.json'):
            with self.subTest(path=path):
                self.refused('safe committed map path', source_map=path)
        self.native('update-index', '--cacheinfo', '120000', self.native('rev-parse', f'HEAD:{self.map_path}').strip(), self.map_path)
        self.native('commit', '-m', 'symlink map\n\nTask: https://board/1\nTask: https://board/2')
        self.published = self.native('rev-parse', 'HEAD').strip()
        self.native('update-ref', 'refs/remotes/origin/main', self.published)
        self.integration_raw['result'] = {'sha': self.published}
        self.set_integration()
        self.board.issues[2]['body'] = taskq.block('goal', self.integration_raw)
        self.refused('committed regular file')

    def test_recovery_and_order_holds_refuse(self):
        for fields in ({'model_recovery': {'held': True}}, {'legacy_recovery': {}}, {'order': {'wait': True}}):
            with self.subTest(fields=fields):
                self.board.issues[1]['body'] = taskq.block('goal', {**self.before, **fields})
                self.revision = taskq.question_revision(self.task(1))
                self.refused('unrecovered|legacy recovery')

    def test_stale_result_and_conflicting_review_receipt_refuse(self):
        self.review()
        self.refused('conflicting integrated', command_id='different')
        self.refused('conflicting integrated', text=VERDICT + ' changed')
        self.edit_raw(result={'sha': '0' * 40})
        self.refused('binding changed')

    def test_changed_provenance_refuses_close_without_closing(self):
        self.review()
        self.edit_raw(2, action={**self.integration_raw['action'], 'text': 'other close'})
        before = self.board.get(1)
        with self.assertRaisesRegex(SystemExit, 'natively closed'):
            self.run_cli('close', '1', '--text', VERDICT)
        self.assertEqual(before, self.board.get(1))

    def test_conflicting_duplicate_close_refuses(self):
        self.review()
        self.run_cli('close', '1', '--text', VERDICT)
        before = self.board.get(1)
        with self.assertRaisesRegex(SystemExit, 'conflicting or incomplete'):
            self.run_cli('close', '1', '--text', VERDICT + ' changed')
        self.assertEqual(before, self.board.get(1))

    def test_exact_worker_settlement_required(self):
        child = {'pid': 123, 'birth': 'exact-birth'}
        runtime = {'session': 'original-worker'}
        completion = {'session': 'original-worker', 'terminal': 'turn.completed'}
        request = {'generation': 1, 'owner': 'worker'}
        turn = {'phase': 'complete', 'key': 'turn-1', 'request': request, 'child': child,
                'runtime': runtime, 'completion': completion}
        self.edit_raw(model_turns={'worker': turn})
        self.revision = taskq.question_revision(self.task(1))
        project = mock.Mock()
        host = mock.Mock()
        project.observe.return_value = {'phase': 'released', 'request': request}
        host.observe.return_value = {'phase': 'released', 'request': request, 'runtime': runtime, 'child': child, 'drain': completion}
        with mock.patch.object(taskq, 'model_providers', return_value=(project, host)), \
                mock.patch.object(taskq, 'process_state', return_value='running') as state:
            self.refused('worker completion/death')
            state.return_value = 'dead'
            host.observe.return_value['phase'] = 'bound'
            self.refused('grant settlement')
            host.observe.return_value['phase'] = 'released'
            self.assertFalse(self.review()['repeated'])

    def test_changed_original_identity_and_retained_review_event_refuse(self):
        self.review()
        for fields in ({'claim': {'runtime': 'codex', 'session': 'replacement'}},
                       {'action_payloads': {**self.task(1)['raw']['action_payloads'],
                                           'result': {'id': 99, 'text': 'changed', 'by': 'owner'}}}):
            original = self.board.get(1)
            self.edit_raw(**fields)
            self.refused('binding changed')
            self.board.issues[1] = original
        payloads = self.task(1)['raw']['action_payloads']
        self.edit_raw(action_payloads={**payloads, 'integrated-review': {**payloads['integrated-review'], 'text': 'altered'}})
        self.refused('event binding changed')

    def test_real_source_delta_is_not_whole_source_equivalence(self):
        (self.root / 'taskq.py').write_bytes(b'def bootstrap():\n    return 2\n')
        self.publish()
        self.refused('function source differs')

    def test_integrated_close_retains_unmerged_original_branch_and_worktree(self):
        taskq.CONFIG['workspace'] = 'worktree'
        self.edit_raw(claim={**self.before['claim'], 'name': 'mac'})
        self.revision = taskq.question_revision(self.task(1))
        tree = self.root / '.worktrees' / 'taskq-1'
        tree.parent.mkdir()
        self.native('worktree', 'add', str(tree), 'taskq-1')
        self.review()
        with mock.patch.object(taskq, 'cleanup', side_effect=AssertionError('unmerged original must be retained')):
            self.run_cli('close', '1', '--text', VERDICT)
        self.assertTrue((tree / 'taskq.py').is_file())
        self.assertEqual(self.native('rev-parse', 'refs/heads/taskq-1').strip(), self.original)

    def test_stale_release_refuses_before_review(self):
        before = self.board.get(1)
        with mock.patch.object(taskq, 'release_reason', return_value='stale selected release'):
            with self.assertRaisesRegex(SystemExit, 'stale selected'):
                self.review()
        self.assertEqual(before, self.board.get(1))

    def test_uncertain_write_cannot_claim_receipt(self):
        with mock.patch.object(self.board, 'update'):
            with self.assertRaisesRegex(RuntimeError, 'readback unknown'):
                self.review()
        self.assertNotIn('integrated_review', self.task(1)['raw'])
