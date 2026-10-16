#!/usr/bin/env python3
"""Project-owned selected gate. Unknown impact runs the full Rust suite."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import time
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BUDGET = 900


def selection(paths):
    if paths and all(Path(p).suffix in {'.md', '.txt'} and p not in {'taskq.md', 'AGENTS.md', 'CLAUDE.md'} for p in paths):
        # These pages carry published asset hashes, checked by the Node components.
        return 'web' if any(p in {'docs/design/answer-card.md', 'docs/design/shared-resource-controls.md'} for p in paths) else 'docs'
    if paths and all((p.startswith('src/') and Path(p).suffix in {'.js', '.css', '.html'}) or (p.startswith('tests/') and Path(p).suffix == '.cjs') for p in paths):
        return 'web'
    return 'full'


def changes(base):
    args = ['git', 'diff', '--name-only', '-z', base, '--']
    changed = subprocess.check_output(args, cwd=ROOT).decode().split('\0')
    changed += subprocess.check_output(['git', 'ls-files', '--others', '--exclude-standard', '-z'], cwd=ROOT).decode().split('\0')
    return sorted(set(filter(None, changed)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', default=os.environ.get('CHECK_BASE', 'HEAD^'))
    parser.add_argument('--full', action='store_true')
    parser.add_argument('--plan', action='store_true', help='print selection without running checks')
    args = parser.parse_args()
    try:
        paths = changes(args.base)
        scope = 'full' if args.full or os.environ.get('CHECK_FULL') == 'true' else selection(paths)
    except (subprocess.CalledProcessError, UnicodeError):
        paths, scope = [], 'full'
    if args.plan:
        print('scope='+scope)
        return
    print('Selected checks:', scope, '| base:', args.base, flush=True)
    started = time.monotonic()
    env = dict(os.environ)

    def run(command, extra_env=None):
        left = BUDGET - (time.monotonic() - started)
        if left <= 0:
            raise TimeoutError('selected gate exceeded 900s including setup/build')
        print('+', ' '.join(command), flush=True)
        subprocess.run(command, cwd=ROOT, env={**env, **(extra_env or {})}, check=True, timeout=left)

    try:
        run(['git', 'diff', '--check', args.base if paths else 'HEAD'])
        run([sys.executable, 'scripts/check.test.py'])
        if scope != 'docs':
            run(['cargo', '+stable', 'test', '--locked'] + ([] if scope == 'full' else ['--bin', 'taskq', 'core::tests']))
            if scope == 'full':
                with tempfile.TemporaryDirectory(prefix='taskq-gate-') as install:
                    run(['cargo', '+stable', 'run', '--locked', '--', 'selftest'], {'TASKQ_INSTALL':install, 'TASKQ_SELFTEST_OFFLINE':'1', 'TASKQ_INCIDENT_REPORTS':'off'})
            if os.environ.get('RUNNER_OS') in (None, 'Linux'):
                for script in sorted((ROOT / 'tests').glob('*.test.cjs')):
                    run(['node', script.relative_to(ROOT).as_posix()])
                run(['sh', 'docs/shots/check.test.sh'])
                run(['sh', '.github/release-tag.test.sh'])
                run([sys.executable, '.github/release-check.test.py'])
        for path in paths:
            if path.startswith('tests/') and path.endswith('.browser.cjs') and os.environ.get('RUNNER_OS') is None:
                run(['node', path])
    finally:
        print(f'Selected gate elapsed: {time.monotonic() - started:.3f}s / {BUDGET}s; no retries', flush=True)


if __name__ == '__main__':
    main()
