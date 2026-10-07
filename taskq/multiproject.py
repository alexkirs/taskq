"""Read-only multiproject observation (#186 stage 2): one bounded subprocess per project of an explicit manifest,
each printing an existing #191 v1 report; the parent revalidates and aggregates them. It never runs tick (even plain
tick mutates), update, take, spawn, release, cleanup, board reconciliation or a timer, and writes nothing.
`python -m taskq.multiproject --manifest FILE --json`; docs/multiproject-pm.md is the manifest's specification."""
import argparse
import contextlib
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tomllib

import taskq as core

tick = sys.modules['taskq.tick']

MANIFEST_VERSION = 1
PROVIDERS = {'github': 'github.com', 'gitlab': 'gitlab.com'}  # provider -> default host
MAX_PROJECTS = 50
TIMEOUT = 60  # seconds per project reader, the default of the manifest's `timeout`
MAX_OUTPUT = 1 << 20  # bytes of one reader's report
READER = [sys.executable, '-P', '-m', 'taskq.multiproject', '--observe']  # -P: never import from the checkout's cwd


def now():
    return datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z')


def entry_errors(entry):
    """Type errors of one [[project]] entry; an entry with any is refused before its reader starts."""
    if not isinstance(entry, dict):
        return ['entry is not a table']
    errors = [f'{key}: write a non-empty string' for key in ('provider', 'repository', 'board', 'checkout')
              if not isinstance(entry.get(key), str) or not entry[key]]
    if entry.get('provider') not in PROVIDERS:
        errors.append('provider: write "github" or "gitlab"')
    if 'host' in entry and (not isinstance(entry['host'], str) or not entry['host']):
        errors.append('host: write a non-empty string')
    if not isinstance(entry.get('repository_id'), int) or isinstance(entry.get('repository_id'), bool):
        errors.append('repository_id: write the tracker\'s numeric repository/project id')
    if isinstance(entry.get('checkout'), str) and not Path(entry['checkout']).expanduser().is_absolute():
        errors.append('checkout: write an absolute path')
    timeout = entry.get('timeout', TIMEOUT)
    if not isinstance(timeout, int) or isinstance(timeout, bool) or not 1 <= timeout <= 600:
        errors.append('timeout: write whole seconds, 1..600')
    view = entry.get('view', {})
    if not isinstance(view, dict) or set(view) - {'filter', 'mine', 'limits'}:
        errors.append('view: only filter, mine, limits')
    elif (not isinstance(view.get('filter', ''), str) or not isinstance(view.get('mine', False), bool)
          or not isinstance(view.get('limits', {}), dict)
          or any(not isinstance(count, int) or isinstance(count, bool) or count < 0 for count in view.get('limits', {}).values())):
        errors.append('view: filter is a string, mine a boolean, limits runtime = non-negative integer')
    return errors


def load_manifest(path):
    """[(entry, errors)] in manifest order. A wrong version or shape refuses the whole manifest; a duplicate stable
    identity, repository or local checkout refuses every entry that shares it."""
    manifest = tomllib.loads(Path(path).read_text())
    if manifest.get('version') != MANIFEST_VERSION:
        raise SystemExit(f'{path}: version = {MANIFEST_VERSION} required')
    entries = manifest.get('project', [])
    if not isinstance(entries, list) or not entries or len(entries) > MAX_PROJECTS:
        raise SystemExit(f'{path}: write 1..{MAX_PROJECTS} [[project]] entries')
    checked = [(entry, entry_errors(entry)) for entry in entries]
    keys = {}
    for index, (entry, errors) in enumerate(checked):
        if errors:
            continue
        entry.setdefault('host', PROVIDERS[entry['provider']])
        for name, key in (('stable identity', (entry['provider'], entry['host'].lower(), entry['repository_id'])),
                          ('repository', (entry['provider'], entry['host'].lower(), entry['repository'].lower())),
                          ('checkout binding', str(Path(entry['checkout']).expanduser().resolve()))):
            keys.setdefault((name, key), []).append(index)
    for (name, key), indexes in keys.items():
        if len(indexes) > 1:
            for index in indexes:
                checked[index][1].append(f'duplicate {name} {key} in entries {", ".join(str(i + 1) for i in indexes)}')
    return checked


def repository_url(entry):
    return f'https://{entry["host"]}/{entry["repository"]}'


def remote_identity(url):
    """(host, path) of a git remote URL: https://host/path(.git), ssh://user@host[:port]/path, user@host:path."""
    found = re.fullmatch(r'(?:[a-z][a-z+]*://)?(?:[^@/]+@)?([^/:]+)(?::\d+(?=/))?[:/](.+?)(?:\.git)?/?', url.strip())
    return (found[1].lower(), found[2].lower()) if found else None


def verify(entry):
    """Blockers of the local binding and tracker identity, checked before any queue read; configures the core."""
    checkout = Path(entry['checkout']).expanduser().resolve()
    if not (checkout / 'taskq.toml').is_file():
        return [f'checkout {checkout}: no taskq.toml']
    if core.main_checkout(checkout).resolve() != checkout:
        return [f'checkout {checkout}: not a main checkout']
    origin = subprocess.run(['git', '-C', str(checkout), 'remote', 'get-url', 'origin'],
                            capture_output=True, text=True, timeout=30)
    if origin.returncode or remote_identity(origin.stdout) != (entry['host'].lower(), entry['repository'].lower()):
        return [f'checkout origin {origin.stdout.strip() or "unavailable"} is not {repository_url(entry)}']
    try:
        core.configure(checkout / 'taskq.toml')
    except SystemExit as error:
        return [f'taskq.toml: {error}']
    provider = 'gitlab' if core.BOARDS else 'github'
    host = core.HOST or PROVIDERS[provider]
    blockers = [f'taskq.toml {what} {found!r} is not the manifest\'s {wanted!r}' for what, found, wanted in (
        ('provider', provider, entry['provider']), ('host', host.lower(), entry['host'].lower()),
        ('repository', core.PROJECT_PATH.lower(), entry['repository'].lower()), ('board', core.BOARD, entry['board']))
        if found != wanted]
    if blockers:
        return blockers
    found = core.api('GET', 'repository' if provider == 'github' else '/' + core.PROJECT)
    if not isinstance(found, dict) or found.get('id') != entry['repository_id']:
        return [f'tracker repository id {found.get("id") if isinstance(found, dict) else None!r} is not {entry["repository_id"]}']
    return []


def observe(entry):
    """One project's v1 report, read only: the report half of tick's queue pass without its releases, unlocks,
    board moves, launches, beat or update."""
    report = {'contract': tick.report_contract(), 'repository': repository_url(entry), 'profile': {},
              'board': 'unavailable', 'observed_at': now(), 'outcome': 'unknown', 'actions': [], 'refusals': [],
              'workers': [], 'source_status': 'unavailable', 'validation': []}
    try:
        if blockers := verify(entry):
            report.update(outcome='refused', refusals=blockers)
        else:
            view = entry.get('view', {})
            args = argparse.Namespace(filter=view.get('filter'), mine=view.get('mine'), limit=view.get('limits'))
            candidates = core.profile(args)[1]
            report.update(profile=args.profile, source_status='available', outcome='ok')
            try:
                if core.BOARDS:
                    board = next((board for board in core.api('GET', 'boards') if board['name'] == core.BOARD), None)
                    report['board'] = f'{report["repository"]}/-/boards/{board["id"]}' if board else 'unavailable'
                else:
                    report['board'] = (core.api('GET', 'board') or {}).get('url') or 'unavailable'
            except (SystemExit, OSError, ValueError, subprocess.SubprocessError) as error:
                report['refusals'].append(f'board unavailable: {error}')
            shown = [item for item in candidates if item['state'] in ('doing', 'ask', 'review')]
            agents = {}
            if any((item['claim'] or {}).get('runtime') == 'claude' for item in shown):
                agents = core.claude_agents(strict=True)
                if agents is None:
                    agents = {}
                    report['refusals'].append('claude agents inventory unavailable: claude session status unknown')
            for item in shown:
                if item['state'] == 'doing' and (item['claim'] or {}).get('session'):
                    item['_report_activity'] = tick.liveness(item, agents)[1]
                else:
                    item['_report_activity'] = f'issue {core.age(item)} min ago'
            report['workers'] = [tick.report_row(item, agents) for item in shown]
    except (SystemExit, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        report.update(outcome='failure', source_status='unavailable', workers=[], refusals=[f'{type(error).__name__}: {error}'])
    report['validation'] = tick.validate_report(report)
    return report


def run_bounded(command, cwd, timeout):
    """(returncode, stdout, stderr) of one reader, its whole process group killed at the timeout: the reader admits
    no mutation or worker, so terminating it loses nothing. returncode None: timed out."""
    env = {**os.environ, 'PYTHONPATH': os.pathsep.join(filter(None, (str(Path(__file__).resolve().parents[1]),
                                                                     os.environ.get('PYTHONPATH'))))}
    child = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, start_new_session=True)
    try:
        out, err = child.communicate(timeout=timeout)
        return child.returncode, out, err
    except subprocess.TimeoutExpired:
        # ponytail: Windows has no process group here; a grandchild there may outlive the reader until its own end.
        os.killpg(child.pid, signal.SIGKILL) if hasattr(os, 'killpg') else child.kill()
        with contextlib.suppress(subprocess.TimeoutExpired):
            child.communicate(timeout=5)
        return None, b'', b''


def read_project(entry, errors, reader):
    """One bounded read attempt of one entry; every failure stays explicit, never empty healthy work."""
    get = entry.get if isinstance(entry, dict) else lambda key: None
    result = {'repository': get('repository') if errors else repository_url(entry),
              **{key: get(key) for key in ('provider', 'repository_id', 'board', 'checkout')},
              'status': 'refused', 'errors': list(errors), 'report': None, 'received_applied': 'unknown'}
    if errors:
        return result
    try:
        code, out, err = run_bounded(reader + [json.dumps(entry)], Path(entry['checkout']).expanduser(), entry.get('timeout', TIMEOUT))
    except OSError as error:
        return {**result, 'status': 'failed', 'errors': [f'reader did not start: {error}']}
    if code is None:
        return {**result, 'status': 'timeout', 'errors': [f'reader exceeded {entry.get("timeout", TIMEOUT)} s; terminated']}
    tail = core.codex_line(err.decode(errors='replace')[-500:]) if err else ''
    if code:
        return {**result, 'status': 'failed', 'errors': [f'reader exit {code}: {tail or "no output"}']}
    try:
        if len(out) > MAX_OUTPUT:
            raise ValueError('report too large')
        report = json.loads(out)
        problems = tick.validate_report(report)
    except (ValueError, TypeError, AttributeError, KeyError) as error:
        return {**result, 'status': 'malformed', 'errors': [f'reader output is not a v1 report: {error}']}
    if problems == ['missing report fields'] or problems == ['invalid validation blockers']:
        return {**result, 'status': 'malformed', 'errors': problems}
    if report['repository'] != result['repository']:
        problems.append(f'report repository {report["repository"]} is not {result["repository"]}')
    if report['outcome'] == 'refused':
        return {**result, 'status': 'refused', 'errors': report['refusals'] + problems, 'report': report}
    return {**result, 'status': 'ok' if not problems and report['outcome'] == 'ok' else 'blocked',
            'errors': problems + ([] if report['outcome'] == 'ok' else [f'outcome {report["outcome"]}'] + report['refusals']),
            'report': report}


def aggregate(manifest, reader=READER):
    contract = tick.report_contract()
    projects = [read_project(entry, errors, reader) for entry, errors in load_manifest(manifest)]
    return {'multiproject': MANIFEST_VERSION, 'manifest': str(Path(manifest).resolve()), 'observed_at': now(),
            'contract': {key: contract[key] for key in ('version', 'sha256', 'source')},
            'outcome': 'ok' if all(project['status'] == 'ok' for project in projects) else 'blocked',
            'received_applied': 'unknown', 'projects': projects}


def render(result):
    lines = [f'Multiproject observation v{result["multiproject"]} of {result["manifest"]}',
             f'Observed: {result["observed_at"]}; outcome: {result["outcome"]}; received/applied: unknown']
    for project in result['projects']:
        lines += ['', f'# {project["repository"]} ({project["provider"]} id {project["repository_id"]}, '
                      f'board {project["board"]}): {project["status"]}']
        lines += [f'Blocker: {error}' for error in project['errors']]
        if project['report']:
            try:
                lines.append(tick.render_report(project['report']))
            except (KeyError, TypeError, AttributeError) as error:
                lines.append(f'Report not renderable: {error}')
    return '\n'.join(lines)


def main(argv=None, reader=READER):
    parser = argparse.ArgumentParser(prog='python -m taskq.multiproject', description=__doc__.split('\n')[0])
    parser.add_argument('--manifest', help='the explicit project manifest (docs/multiproject-pm.md)')
    parser.add_argument('--json', action='store_true', help='print the aggregate as JSON')
    parser.add_argument('--observe', metavar='ENTRY', help=argparse.SUPPRESS)  # the reader: one verified entry, JSON
    args = parser.parse_args(argv)
    if args.observe:
        with contextlib.redirect_stdout(sys.stderr):  # stdout carries only the report
            report = observe(json.loads(args.observe))
        return print(json.dumps(report))
    if not args.manifest:
        parser.error('--manifest is required')
    result = aggregate(args.manifest, reader)
    print(json.dumps(result) if args.json else render(result))
    if result['outcome'] != 'ok':
        raise SystemExit(1)


if __name__ == '__main__':
    main()
