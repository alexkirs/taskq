"""`taskq projects`: the ordinary `taskq tick [--act] --json` in each checkout of the owner's list ([projects] of
taskq.local.toml), each with a timeout and a share of this machine's limits; one R6 report per project (R10).
Specification: docs/multiproject-pm.md."""
import argparse
from collections import Counter
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib

import taskq as core
from taskq.worker import CLAUDE_ENDED

TIMEOUT = 300  # seconds per project's tick
UNKNOWN = 1 << 20  # an unreadable session list: no free slot
TICK = [sys.executable, '-P', '-m', 'taskq', 'tick']  # -P: the installed taskq, never the checkout's cwd


def projects():
    """The owner's list from taskq.local.toml: {name: checkout}, in written order."""
    found = tomllib.loads(core.LOCAL.read_text()).get('projects', {}) if core.LOCAL.is_file() else {}
    if not found or not all(isinstance(path, str) for path in found.values()):
        core.fail(f'{core.LOCAL}: write [projects] name = "/main/checkout/path", or run with --set NAME=PATH ...')
    return found


def write(pairs):
    """Replace the [projects] table of taskq.local.toml with `pairs` (NAME=PATH); the rest of the file stays."""
    listed = {}
    for pair in pairs:
        name, _, path = pair.partition('=')
        path = Path(path).expanduser().resolve()
        if not re.fullmatch(r'[A-Za-z0-9_-]+', name) or not (path / 'taskq.toml').is_file():
            core.fail(f'{pair}: write NAME=PATH, PATH a main checkout with taskq.toml')
        listed[name] = str(path)
    text = core.LOCAL.read_text() if core.LOCAL.is_file() else ''
    text = re.sub(r'(?ms)^\[projects\]\n.*?(?=^\[|\Z)', '', text).rstrip()
    table = '[projects]\n' + ''.join(f'{name} = {json.dumps(path)}\n' for name, path in listed.items())
    core.LOCAL.write_text((text + '\n\n' if text else '') + table)
    return listed


def held(roots):
    """Live `T<N> `/`S<N> ` sessions of each listed checkout: {root: Counter(runtime)}; a task's worker and
    supervisor hold one slot, as in a tick. An unreadable list counts as full (UNKNOWN) in every root."""
    agents = core.claude_agents(strict=True)
    rows = [('claude', agent.get('cwd'), agent.get('name')) for agent in (agents or {}).values()
            if agent.get('state') in ('working', 'blocked') or agent.get('pid') and agent.get('state') not in CLAUDE_ENDED]
    try:
        from taskq.cleanup import cleanup_codex
        threads = cleanup_codex(set(roots)) if core.CODEX_SOCKET.exists() else {}
    except (OSError, SystemExit, ValueError):
        threads = None
    rows += [('codex', thread.get('cwd'), thread.get('name')) for thread in (threads or {}).values()
             if (thread.get('status') or {}).get('type') != 'systemError']
    slots = {(runtime, Path(cwd or '/').resolve(), iid[1]) for runtime, cwd, name in rows if (iid := re.match(r'[TS](\d+) ', name or ''))}
    return {root: Counter({'claude': UNKNOWN * (agents is None), 'codex': UNKNOWN * (threads is None)})
            + Counter(runtime for runtime, where, _ in slots if where == root) for root in roots}


def run(name, checkout, act, timeout, cap, others):
    """One project's tick, `--limit` its own limit within the cap less the other projects' live workers: its JSON
    output, or {'error': ...}. Files, not pipes: a worker the tick launched may hold an inherited pipe open."""
    try:
        core.configure(Path(checkout) / 'taskq.toml')
        own = core.resolve(argparse.Namespace(filter=None, mine=None, limit=None))[0]['limits']
    except SystemExit as error:
        return {'project': name, 'error': str(error)}
    limit = ','.join(f'{key}={min(own[key], max(0, count - others[key]))}' for key, count in cap.items())
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        try:
            done = subprocess.run(TICK + ['--json', '--limit', limit] + (['--act'] if act else []), cwd=checkout,
                                  stdout=out, stderr=err, stdin=subprocess.DEVNULL, timeout=timeout)
        except (subprocess.TimeoutExpired, OSError) as error:
            slow = isinstance(error, subprocess.TimeoutExpired)
            return {'project': name, 'error': f'timeout after {timeout} s' if slow else f'did not start: {error}'}
        text, said = (found.seek(0) or found.read().decode(errors='replace') for found in (out, err))
    try:
        found = json.loads(text.strip().splitlines()[-1])
        if 'report' not in found:  # the tick stopped before its pass
            return {'project': name, 'error': f'tick {found["outcome"]}: ' + '; '.join(found['refusals'])}
        return {'project': name, 'outcome': found['outcome'], 'report': found['report'], 'refusals': found['refusals']}
    except (IndexError, KeyError, TypeError, ValueError):
        last = (said.strip() or text.strip() or 'no output').splitlines()[-1]
        return {'project': name, 'error': f'tick exit {done.returncode}: {last}'}


def render(result):
    """One project's R6 report: heading, Board link, Task | Status | Runtime | Session, then what needs judgement."""
    lines = [f'## {result["project"]}']
    if 'error' in result:
        return '\n'.join(lines + [f'Error: {result["error"]}'])
    report = result['report']
    lines += [f'Board: {report["board"]}', '', '| Task | Status | Runtime | Session |', '|---|---|---|---|']
    lines += [f'| {row["task"]} {row["title"]} | {row["state"]} | {row["runtime"]} @{row["machine"]} | {row["session"]} |'
              for row in report['workers']] or ['| none | | | |']
    return '\n'.join(lines + [f'- {line}' for line in result['refusals']])


def main(argv=None):
    parser = argparse.ArgumentParser(prog='taskq projects', description=__doc__.split('\n')[0])
    parser.add_argument('--set', nargs='+', metavar='NAME=PATH', help="write the owner's list, replacing the old one")
    parser.add_argument('--act', action='store_true', help='run tick --act in each project')
    parser.add_argument('--json', action='store_true', help='one JSON list of the projects\' results')
    parser.add_argument('--timeout', type=int, default=TIMEOUT, help=f'seconds per project (default {TIMEOUT})')
    args = parser.parse_args(argv)
    core.configure()
    if args.set:
        return print('Projects: ' + ', '.join(f'{name} {path}' for name, path in write(args.set).items()))
    listed, cap = projects(), core.resolve(argparse.Namespace(filter=None, mine=None, limit=None))[0]['limits']
    results = []
    for name, path in listed.items():  # the sessions again for each: the last tick may have started some
        live = held([Path(path).resolve() for path in listed.values()])
        others = sum((found for root, found in live.items() if root != Path(path).resolve()), Counter())
        results.append(run(name, path, args.act, args.timeout, cap, others))
    print(json.dumps(results) if args.json else '\n\n'.join(map(render, results)))
    return 1 if any('error' in result or result['outcome'] == 'failure' for result in results) else 0
