"""One native gate; preserve test outcome even when a post-check refuses qualification."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args])


def snapshot(root):
    mismatches = []
    entries = git(root, 'ls-tree', '-rz', 'HEAD').split(b'\0')
    count = 0
    for entry in filter(None, entries):
        meta, name = entry.split(b'\t', 1)
        mode, kind, oid = meta.split()
        if kind != b'blob':
            raise ValueError('gate requires tracked blobs, not embedded repositories')
        path = root / os.fsdecode(name)
        try:
            actual = os.fsencode(os.readlink(path)) if mode == b'120000' else path.read_bytes()
        except FileNotFoundError:
            actual = None
        expected = git(root, 'cat-file', 'blob', oid.decode('ascii'))
        if actual != expected:
            mismatches.append(os.fsdecode(name))
        count += 1
    return {'tracked_blobs': count, 'mismatches': mismatches,
            'status': os.fsdecode(git(root, 'status', '--porcelain'))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path.cwd())
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--test', nargs='+')
    args = parser.parse_args()
    root, output = args.root.resolve(), args.output.resolve()
    if output == root or root in output.parents:
        parser.error('evidence output must be outside the checkout')
    if output.exists() and any(output.iterdir()):
        parser.error('evidence output must be new or empty; preserve prior runs')
    output.mkdir(parents=True, exist_ok=True)
    log = output / 'tests.log'
    receipt = {'started_at': datetime.now(timezone.utc).isoformat(), 'python': sys.version,
               'utf8_mode': sys.flags.utf8_mode, 'test_exit': None, 'error': None}
    started = time.monotonic()
    try:
        receipt['head'] = git(root, 'rev-parse', 'HEAD').decode('ascii').strip()
        receipt['before'] = snapshot(root)
        if receipt['before']['status'] or receipt['before']['mismatches']:
            raise ValueError('pre-gate checkout is dirty or differs from Git blobs')
        if any(os.environ.get(key) for key in ('PYTHONUTF8', 'PYTHONIOENCODING')):
            raise ValueError('native gate requires no Python encoding overrides')
        command = [sys.executable, '-B', '-m', 'unittest', *(args.test or ['discover', '-s', 'tests'])]
        receipt['command'] = command
        with log.open('wb') as stream:
            receipt['test_exit'] = subprocess.run(command, cwd=root, stdout=stream,
                                                  stderr=subprocess.STDOUT).returncode
        receipt['after'] = snapshot(root)
        if receipt['after']['status'] or receipt['after']['mismatches']:
            raise ValueError('post-gate checkout changed; artifacts retained')
    except Exception as error:
        receipt['error'] = f'{type(error).__name__}: {error}'
    receipt['elapsed_seconds'] = time.monotonic() - started
    receipt['gate_exit'] = int(receipt['error'] is not None or receipt['test_exit'] != 0)
    if log.exists():
        receipt['log_sha256'] = hashlib.sha256(log.read_bytes()).hexdigest()
    (output / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(receipt))
    return receipt['gate_exit']


if __name__ == '__main__':
    sys.exit(main())
