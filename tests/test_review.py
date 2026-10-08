"""Review publication against a disposable bare remote and the queue's fake tracker."""
import os
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

import test_taskq as fixtures
from test_taskq import CLAUDE, COORDINATOR, q, worker


class Review(unittest.TestCase):
    do, refused, add, state = fixtures.Cycle.do, fixtures.Cycle.refused, fixtures.Cycle.add, fixtures.Cycle.state

    def setUp(self):
        fixtures.Cycle.setUp(self)
        self.root = self.directory / 'repo'
        self.root.mkdir()
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'user.name', 'Test')
        self.git('config', 'user.email', 'test@example.invalid')
        self.git('config', 'commit.gpgsign', 'false')
        self.config = self.root / 'taskq.toml'
        self.config.write_text('[gitlab]\nproject = "owner/repo"\n[workspace]\npublish = "review"\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'base')
        self.base = self.git('rev-parse', 'HEAD')
        self.remote = self.directory / 'remote.git'
        self.git('init', '-q', '--bare', str(self.remote))
        self.git('remote', 'add', 'origin', str(self.remote))
        self.git('push', '-q', 'origin', 'main')
        before = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, before)
        self.addCleanup(q.configure, Path(__file__).parent / 'taskq.toml')
        q.configure(self.config)
        self.enterContext(patch.object(worker, 'retire_local'))

    def git(self, *args, cwd=None):
        return subprocess.run(['git', '-C', str(cwd or self.root), *args], check=True,
                              capture_output=True, text=True).stdout.strip()

    def main_head(self):
        return self.git('rev-parse', 'refs/heads/main', cwd=self.remote)

    def candidate(self):
        iid = self.add('--type', 'code')
        tree = self.directory / f'taskq-{iid}'
        self.git('worktree', 'add', '-q', '-b', f'taskq-{iid}', str(tree), 'main')
        (tree / f'change-{iid}').write_text('change')
        self.git('add', '.', cwd=tree)
        self.git('commit', '-qm', f'task {iid}', cwd=tree)
        self.git('push', '-q', 'origin', f'taskq-{iid}')
        sha = self.git('rev-parse', 'HEAD', cwd=tree)
        self.do(CLAUDE, 'take', iid)
        self.do(CLAUDE, 'result', iid, '--sha', sha, '--checks', 'passed', '--text', 'ready')
        return iid, tree, sha

    def test_result_reject_and_briefs_never_publish_main(self):
        iid, _, _ = self.candidate()
        self.assertEqual(self.main_head(), self.base)
        self.do(COORDINATOR, 'reject', iid, '--text', 'change requested')
        self.assertEqual(self.main_head(), self.base)
        for item in (q.task(iid), {**q.task(iid), 'claim': None}):
            brief = q.brief(item)
            self.assertIn(f'HEAD:refs/heads/taskq-{iid}', brief)
            self.assertIn('never push main', brief)
            self.assertNotIn('HEAD:main', brief)
        self.do(CLAUDE, 'take', iid)
        self.do(CLAUDE, 'ask', iid, '--text', 'question')
        self.do(COORDINATOR, 'answer', iid, '--text', 'continue')
        self.assertIn('never push main', q.brief(q.task(iid)))

    def test_close_publishes_exact_reviewed_sha_and_keeps_checkout(self):
        iid, _, sha = self.candidate()
        (self.root / 'dirty').write_text('leave me')
        self.do(COORDINATOR, 'close', iid, '--text', 'reviewed')
        self.assertEqual(self.main_head(), sha)
        self.assertEqual(self.git('rev-parse', 'HEAD'), self.base)
        self.assertEqual((self.root / 'dirty').read_text(), 'leave me')
        self.assertEqual(self.gitlab.issues[iid]['state'], 'closed')
        self.assertNotIn('taskq-review-', self.git('worktree', 'list'))

    def test_changed_branch_refuses_stale_review(self):
        iid, tree, _ = self.candidate()
        before = q.task(iid)
        self.git('commit', '--allow-empty', '-qm', 'changed after review', cwd=tree)
        self.git('push', '-q', 'origin', f'taskq-{iid}')
        self.assertIn('not the task branch head', self.refused(COORDINATOR, 'close', iid, '--text', 'reviewed'))
        self.assertEqual(self.main_head(), self.base)
        self.assertEqual(self.state(iid), 'review')
        self.assertEqual(q.task(iid)['result'], before['result'])
        self.assertEqual(q.task(iid)['claim'], before['claim'])

    def test_two_candidates_require_rebase_and_new_review(self):
        first, _, one = self.candidate()
        second, tree, two = self.candidate()
        self.do(COORDINATOR, 'close', first, '--text', 'reviewed')
        self.assertIn('Publication refused', self.refused(COORDINATOR, 'close', second, '--text', 'reviewed'))
        self.assertEqual(self.main_head(), one)
        self.assertEqual(q.task(second)['result']['sha'], two)
        self.do(COORDINATOR, 'reject', second, '--text', 'rebase requested')
        self.git('fetch', '-q', 'origin')
        self.git('rebase', 'origin/main', cwd=tree)
        self.git('push', '-q', '--force-with-lease', 'origin', f'taskq-{second}', cwd=tree)
        new = self.git('rev-parse', 'HEAD', cwd=tree)
        self.assertNotEqual(new, two)
        self.do(CLAUDE, 'take', second)
        self.do(CLAUDE, 'result', second, '--sha', new, '--checks', 'passed', '--text', 'rebased')
        self.assertEqual(self.main_head(), one)
        self.do(COORDINATOR, 'close', second, '--text', 'new SHA reviewed')
        self.assertEqual(self.main_head(), new)
        self.assertEqual(self.git('show', f'{new}:change-{first}'), 'change')
        self.assertEqual(self.git('show', f'{new}:change-{second}'), 'change')

    def test_main_moves_between_fetch_and_push(self):
        iid, _, sha = self.candidate()
        run = subprocess.run
        rival = []

        def race(argv, **kwargs):
            if argv[-3:] == ['push', 'origin', f'{sha}:refs/heads/main']:
                self.git('commit', '--allow-empty', '-qm', 'concurrent publisher')
                self.git('push', '-q', 'origin', 'main')
                rival.append(self.git('rev-parse', 'HEAD'))
            return run(argv, **kwargs)

        with patch.object(worker.subprocess, 'run', race):
            self.assertIn('Publication refused', self.refused(COORDINATOR, 'close', iid, '--text', 'reviewed'))
        self.assertEqual(self.main_head(), rival[0])
        self.assertEqual(self.state(iid), 'review')
        self.assertEqual(q.task(iid)['result']['sha'], sha)

    def test_advanced_main_closes_without_republishing_and_closed_retry_is_safe(self):
        iid, _, sha = self.candidate()
        self.git('merge', '--ff-only', sha)
        self.git('commit', '--allow-empty', '-qm', 'later main')
        self.git('push', '-q', 'origin', 'main')
        before = self.main_head()
        self.do(COORDINATOR, 'close', iid, '--text', 'accepted')
        self.assertEqual((self.main_head(), self.gitlab.issues[iid]['state']), (before, 'closed'))
        self.do(COORDINATOR, 'close', iid, '--text', 'retry')
        self.assertEqual(self.main_head(), before)

    def test_closed_retry_recovers_only_the_receipted_ended_local_claim(self):
        iid, _, _ = self.candidate()
        with patch.object(worker, 'retire_local', side_effect=RuntimeError('interrupted')):
            with self.assertRaisesRegex(RuntimeError, 'interrupted'):
                self.do(COORDINATOR, 'close', iid, '--text', 'accepted')
        self.assertEqual(self.gitlab.issues[iid]['state'], 'closed')
        with patch.object(worker, 'retire_local') as retire:
            self.do(COORDINATOR, 'close', iid, '--text', 'restart')
        retire.assert_called_once()

    def test_missing_published_branch_keeps_review_evidence(self):
        iid, _, sha = self.candidate()
        self.git('merge', '--ff-only', sha)
        self.git('push', '-q', 'origin', 'main')
        self.git('push', '-q', 'origin', '--delete', f'taskq-{iid}')
        before = q.task(iid)
        self.refused(COORDINATOR, 'close', iid, '--text', 'accepted')
        self.assertEqual(self.state(iid), 'review')
        self.assertEqual((q.task(iid)['claim'], q.task(iid)['result']), (before['claim'], before['result']))

    def test_config_default_validation_and_local_override(self):
        for value in ('"typo"', 'true', '7', '[]'):
            self.config.write_text(f'[gitlab]\nproject = "owner/repo"\n[workspace]\npublish = {value}\n')
            with self.assertRaisesRegex(SystemExit, 'publish'):
                q.configure(self.config)
        self.config.write_text('[gitlab]\nproject = "owner/repo"\n')
        q.configure(self.config)
        self.assertEqual(q.PUBLISH, 'direct')
        self.config.write_text('[gitlab]\nproject = "owner/repo"\n[workspace]\npublish = "review"\n')
        q.configure(self.config)
        q.LOCAL.write_text('[workspace]\npublish = "direct"\n')
        with self.assertRaisesRegex(SystemExit, 'unknown key'):
            q.personal()
        self.assertEqual(q.PUBLISH, 'review')
