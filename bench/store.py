#!/usr/bin/env python3
"""Time taskq tracker operations; writes require --write and use one throwaway task."""
import argparse
import os
import re
import subprocess
import sys
import time


def run(name, command):
    env = {**os.environ, 'TASKQ_TRACE': '1'}
    started = time.monotonic()
    done = subprocess.run([sys.executable, '-m', 'taskq', *command], text=True, capture_output=True, env=env)
    calls = len(re.findall(r'^taskq trace:', done.stderr, re.M))
    print(f'{name}: {time.monotonic() - started:.2f} s, {calls} HTTP calls, exit {done.returncode}')
    if done.returncode:
        print(done.stderr.strip() or done.stdout.strip(), file=sys.stderr)
        raise SystemExit(done.returncode)
    return done.stdout


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--iid', type=int, required=True, help='existing task for read-only view')
    parser.add_argument('--write', action='store_true', help='also create, move and close a throwaway task')
    args = parser.parse_args(argv)
    run('list open tasks', ['list'])
    run('view one', ['view', str(args.iid)])
    if not args.write:
        return
    created = run('create throwaway', ['add', '--title', 'bench throwaway', '--goal', 'measure tracker store',
                                       '--acceptance', 'closed by this benchmark', '--type', 'research', '--runtime', 'codex'])
    iid = int(re.search(r'#(\d+)', created).group(1))
    run('take', ['take', str(iid)])
    run('note', ['beat', str(iid)])
    run('state move', ['ask', str(iid), '--text', 'benchmark state move'])
    run('answer', ['answer', str(iid), '--text', 'close it'])
    run('take again', ['take', str(iid)])
    run('result', ['result', str(iid), '--text', 'benchmark complete', '--checks', 'bench/store.py'])
    run('close', ['close', str(iid), '--text', 'benchmark throwaway'])


if __name__ == '__main__':
    main()
