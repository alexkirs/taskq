"""Multiproject passes over an explicit manifest (#186). Default, stage 2: read-only observation, one bounded
subprocess per project printing an existing #191 v1 report; the parent revalidates and aggregates them. It never
runs tick (even plain tick mutates), update, take, spawn, release, cleanup, board reconciliation or a timer, and
writes nothing. `--act --execution-policy FILE`, stage 3: one guarded native `tick --act` per admitted project.
`--recover-actor-output RUN_ID --execution-policy FILE` reads one actor's recorded actual output, read only (#217).
`python -m taskq.multiproject --manifest FILE [--act --execution-policy FILE] --json`; docs/multiproject-pm.md
is the specification of both files."""
import argparse
import contextlib
from datetime import datetime, timezone
import io
import json
import os
from pathlib import Path
import re
import signal
import socket
import stat
import subprocess
import sys
import time
import tomllib

import taskq as core
from taskq.worker import CLAUDE_ENDED
from taskq.codex import codex_archived

tick, worker = sys.modules['taskq.tick'], sys.modules['taskq.worker']

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
    return checked_entries(entries, entry_errors)


def checked_entries(entries, check):
    """[(entry, errors)]: `check`'s errors, and a shared stable identity, repository or checkout refuses each sharer."""
    checked = [(entry, check(entry)) for entry in entries]
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


def child_env():
    return {**os.environ, 'PYTHONPATH': os.pathsep.join(filter(None, (str(Path(__file__).resolve().parents[1]),
                                                                    os.environ.get('PYTHONPATH'))))}


def run_bounded(command, cwd, timeout):
    """(returncode, stdout, stderr) of one reader, its whole process group killed at the timeout: the reader admits
    no mutation or worker, so terminating it loses nothing. returncode None: timed out."""
    child = subprocess.Popen(command, cwd=cwd, env=child_env(), stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
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


def project_result(entry, errors):
    get = entry.get if isinstance(entry, dict) else lambda key: None
    return {'repository': get('repository') if errors else repository_url(entry),
            **{key: get(key) for key in ('provider', 'repository_id', 'board', 'checkout')},
            'status': 'refused', 'errors': list(errors), 'report': None, 'received_applied': 'unknown'}


def read_project(entry, errors, reader):
    """One bounded read attempt of one entry; every failure stays explicit, never empty healthy work."""
    result = project_result(entry, errors)
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


# --- #186 stage 3: the guarded acting pass (docs/multiproject-pm.md § Acting) --------------------------

CAPS = {'claude': 8, 'codex': 4}  # aggregate places of one host's verified OS-user domain when the policy names none
MAX_COUNT = 64  # validation bound of an explicit cap or limit; never a capacity grant
EFFECTS = ('queue', 'cleanup', 'idle_stop')  # what native `tick --act` does that a catalog binding may select
IDENTITY = ('provider', 'host', 'repository_id', 'repository', 'board', 'checkout', 'timeout')
BINDING = ('principal', 'act', 'limits', 'effects')
ACT_TIMEOUT = 600  # seconds per actor, the default of a binding's `timeout`
NOT_EVIDENCE = "readiness and effects come from the project's own workflow (doctor, its settings), never from this file"
ACTOR = [sys.executable, '-P', '-m', 'taskq.multiproject', '--actor']
OCCUPANCY = [sys.executable, '-P', '-m', 'taskq.multiproject', '--occupancy']
CHECK = [sys.executable, '-P', '-m', 'taskq.multiproject', '--enroll-check']
ANCHOR_VERSION = 1


def counts(value):
    return isinstance(value, dict) and set(value) <= set(CAPS) and all(
        isinstance(count, int) and not isinstance(count, bool) and 0 <= count <= MAX_COUNT for count in value.values())


def binding_errors(entry):
    """Errors of one execution catalog binding: its identity as in the manifest, then its execution selection."""
    if not isinstance(entry, dict):
        return ['entry is not a table']
    errors = entry_errors({key: value for key, value in entry.items() if key not in BINDING})
    errors += [f'{key}: unknown key; {NOT_EVIDENCE}' for key in sorted(set(entry) - set(IDENTITY) - set(BINDING))]
    if not isinstance(entry.get('principal'), int) or isinstance(entry.get('principal'), bool):
        errors.append("principal: write the tracker user id this project's pass acts as")
    if not isinstance(entry.get('act'), bool):
        errors.append('act: write true (admitted) or false (catalog-only: its ownership still counts)')
    if not counts(entry.get('limits')):
        errors.append(f'limits: write {{ claude = 0..{MAX_COUNT}, codex = 0..{MAX_COUNT} }}')
    if not isinstance(entry.get('effects'), list) or any(effect not in EFFECTS for effect in entry['effects']):
        errors.append(f'effects: write a list of {", ".join(EFFECTS)}')
    return errors


def load_policy(path):
    """(policy, [(binding, errors)]): the owner's reviewed execution catalog. A wrong shape refuses all acting."""
    return parse_policy(tomllib.loads(Path(path).read_text()), path)


def parse_policy(policy, path):
    """`load_policy` of a parsed table: the policy file's, or an anchor's catalog, which must pass the same checks."""
    if not isinstance(policy, dict):
        raise SystemExit(f'{path}: not a table')
    problems = [f'{key}: unknown key; {NOT_EVIDENCE}' for key in sorted(set(policy) - {'version', 'machine', 'os_user', 'caps', 'project'})]
    if policy.get('version') != MANIFEST_VERSION:
        problems.append(f'version = {MANIFEST_VERSION} required')
    if not isinstance(policy.get('machine'), str) or not policy['machine']:
        problems.append("machine: write this host's taskq machine name")
    if not isinstance(policy.get('os_user'), int) or isinstance(policy.get('os_user'), bool):
        problems.append("os_user: write the numeric OS user id of the guard's domain")
    if not counts(policy.setdefault('caps', {})):
        problems.append(f'caps: write {{ claude = 0..{MAX_COUNT}, codex = 0..{MAX_COUNT} }}; omitted ones are {CAPS}')
    entries = policy.get('project')
    if not isinstance(entries, list) or not entries or len(entries) > MAX_PROJECTS:
        problems.append(f'write 1..{MAX_PROJECTS} [[project]] bindings')
    if problems:
        raise SystemExit(f'{path}: ' + '; '.join(problems))
    policy['caps'] = {**CAPS, **policy['caps']}
    return policy, checked_entries(entries, binding_errors)


def binding_key(entry):
    return (entry['provider'], entry['host'].lower(), entry['repository_id'], entry['repository'].lower(), entry['board'],
            str(Path(entry['checkout']).expanduser().resolve()))


def binding_for(entry, catalog):
    """The catalog binding a valid view entry acts under; SystemExit when it is not admitted. The view grants nothing."""
    for binding, errors in catalog:
        try:
            same = binding_key(binding) == binding_key(entry)
        except (KeyError, TypeError, AttributeError):  # a binding too broken to compare matches nothing
            continue
        if same and errors:
            raise SystemExit('execution catalog binding refused: ' + '; '.join(errors))
        if same:
            if not binding['act']:
                raise SystemExit('catalog-only binding (act = false): its ownership counts, it is never admitted')
            return binding
    raise SystemExit('no valid execution catalog binding with this provider, host, repository id, repository, board and checkout')


def guard_path():
    """This host's one acting guard, beside machine-id in the OS user's taskq state folder; keyed by nothing else."""
    return core.UPDATE_STAMP.parent / 'multiproject-acting.lock'


class GuardHeld(SystemExit):
    """Another process holds the host guard."""


def guard(policy, create=True):
    """Lock the host guard for this process: (fd, stat). The fd is close-on-exec and never passed on, so a spawned
    worker holds nothing; the OS frees the lock when this process ends, a crash included; the file is never deleted
    or replaced. Outside the catalog's one verified OS-user domain, or without flock, refused: nothing is widened.
    `create=False`, read only: lock the existing guard only, through no symlink; no folder or file is created."""
    if os.name != 'posix':
        raise SystemExit('host guard: only POSIX flock is qualified; this platform is refused')
    import fcntl
    uid, path = os.getuid(), guard_path()
    if uid != policy['os_user']:
        raise SystemExit(f"host guard: OS user {uid} is outside the catalog's verified domain {policy['os_user']}; refused, no permission changed")
    if create:
        path.parent.mkdir(parents=True, exist_ok=True)
    folder = path.parent.stat() if create else os.lstat(path.parent)
    if not stat.S_ISDIR(folder.st_mode) or folder.st_uid != uid or folder.st_mode & 0o022:
        raise SystemExit(f"host guard: {path.parent} is not OS user {uid}'s alone; refused, no permission changed")
    fd = os.open(path, (os.O_RDWR | os.O_CREAT if create else os.O_RDONLY) | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        held = os.fstat(fd)
        if not stat.S_ISREG(held.st_mode) or held.st_uid != uid or held.st_mode & 0o022:
            raise SystemExit(f"host guard: {path} is not a file of OS user {uid} alone; refused, no permission changed")
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise GuardHeld('host guard held: an acting pass of this host is running') from None
        found = os.stat(path, follow_symlinks=False)
        if (found.st_dev, found.st_ino) != (held.st_dev, held.st_ino):
            raise SystemExit(f'host guard: {path} was replaced; refused')
    except BaseException:
        os.close(fd)
        raise
    return fd, held


def guard_free(policy):
    """None when no actor holds the host guard now; else why not, as a refusal."""
    try:
        os.close(guard(policy)[0])
    except (SystemExit, OSError) as error:
        return str(error)
    return None


def claude_rows():
    """Every row of `claude agents --json --all`, interactive ones too; None when the list cannot be read."""
    try:
        done = subprocess.run(['claude', 'agents', '--json', '--all'], capture_output=True, text=True, timeout=60)
        return json.loads(done.stdout) if not done.returncode else None
    except (OSError, subprocess.SubprocessError, ValueError):
        return None


def claude_inventory(rows):
    """Identities of the host's Claude sessions that may hold a place: every row of any kind without a terminal state
    (`claude_agents(strict=True)` keeps background rows only). None, unknown: an unreadable list or unidentified row."""
    if not isinstance(rows, list) or not all(isinstance(row, dict) for row in rows):
        return None
    found = set()
    for row in rows:
        if row.get('state') in CLAUDE_ENDED and row.get('pid') is None:
            continue
        session, pid = row.get('sessionId'), row.get('pid')
        if isinstance(session, str) and session:
            found.add(f'session:{session}')
        elif isinstance(pid, int) and not isinstance(pid, bool):
            found.add(f'pid:{pid}')
        else:
            return None
    return found


def claude_executor_ended(session, checkout, rows):
    """Fresh CLI proof that one exact background executor ended in this binding. This is not archive or ownership proof."""
    if not isinstance(rows, list) or not all(isinstance(row, dict) for row in rows):
        return False
    matches = [row for row in rows if row.get('sessionId') == session]
    if not (len(matches) == 1 and matches[0].get('kind') == 'background'
            and matches[0].get('state') in CLAUDE_ENDED and isinstance(matches[0].get('status'), str)
            and matches[0].get('status') != 'busy' and matches[0].get('pid') is None
            and isinstance(matches[0].get('cwd'), str)):
        return False
    try:
        return Path(matches[0]['cwd']).resolve() == Path(checkout).resolve()
    except OSError:
        return False


def codex_threads():
    """Every unarchived thread of the host's Codex app server; None, unknown: no app server or a failed read."""
    if not core.CODEX_SOCKET.exists():
        return None
    try:
        codex, threads, cursor = core.Codex(), [], None
        try:
            while True:
                page = codex.call('thread/list', {'archived': False, 'limit': 100, 'cursor': cursor})
                threads += page['data']
                if not (cursor := page.get('nextCursor')):
                    break
        finally:
            codex.socket.close()
        return [thread for thread in threads if isinstance(thread['id'], str)]
    except (OSError, SystemExit, ValueError, KeyError, TypeError, AttributeError):
        return None


def codex_inventory(threads):
    """Identities of the Codex threads that may hold a place: running ones and every `T<N>` worker thread not in
    systemError (as #185 counts workers). None stays unknown."""
    if threads is None:
        return None
    found = set()
    for thread in threads:
        kind = (thread.get('status') or {}).get('type')
        if kind not in ('idle', 'notLoaded', 'systemError') or kind != 'systemError' and tick.worker_iid(thread.get('name')):
            found.add(f'session:{thread["id"]}')
    return found


CODEX_INACTIVE = {'idle', 'notLoaded', 'systemError'}


def codex_archived_inactive(session):
    """Fresh supported proof that this exact retained Codex worker is archived and cannot execute. Unknown stays false."""
    try:
        codex = core.Codex()
        try:
            thread = codex.call('thread/read', {'threadId': session})['thread']
        finally:
            codex.socket.close()
        return (isinstance(thread, dict) and thread.get('id') == session and codex_archived(thread)
                and (thread.get('status') or {}).get('type') in CODEX_INACTIVE)
    except (OSError, SystemExit, ValueError, KeyError, TypeError, AttributeError):
        return False


def locality(owner):
    """Where a claim or reservation lives: 'local', 'remote' (a well-formed node or pre-#39 host of another
    machine) or None, unknown: a malformed node or host, or neither and no local app evidence (`local_claim`)."""
    node, host = owner.get('node'), owner.get('host')
    if node is not None:
        return None if not isinstance(node, str) or not re.fullmatch(r'[0-9a-f]{12}', node) else 'local' if core.local_node(node) else 'remote'
    if host is not None:
        return None if not isinstance(host, str) or not host else 'local' if host == socket.gethostname() else 'remote'
    return 'local' if core.local_claim(owner) else None


def occupancy(entry):
    """One catalog binding's same-host ownership, read only, from #208's evidence: `L`, exactly what native `room`
    subtracts; each proven claim (any state) and reservation as (runtimes, identity); everything unproven as uncertain."""
    if blockers := verify(entry):
        return {'status': 'refused', 'errors': blockers}
    if (uid := core.user()) != entry['principal']:
        return {'status': 'refused', 'errors': [f'the tracker principal is {uid}, the catalog binds {entry["principal"]}']}
    everything, where = core.load()[0], repository_url(entry)
    held, inactive, protected, uncertain = [], [], set(), []  # inactive is budget-only; ownership always remains held
    claude = {}
    for item in everything:
        claim, found, task = item['claim'] or {}, item.get('reservation') or {}, f'{where}#{item["iid"]}'
        # Any state: a review, ask, ready or later claim still names its worker; an empty inventory never settles it.
        # A released claim ({'runtime': None, 'session': None}) owns nothing.
        place = locality(claim) if any(value is not None for value in claim.values()) else 'none'
        if place is None:
            uncertain.append(f'{task}: claim of session {claim.get("session")!r} without a proven machine (node {claim.get("node")!r}, '
                             f'host {claim.get("host")!r})')
        elif place == 'local':
            if claim.get('runtime') in CAPS and isinstance(claim.get('session'), str) and claim['session']:
                identity = f'session:{claim["session"]}'
                held.append(([claim['runtime']], identity))
                if (item['state'] in ('review', 'ask', 'later') and claim['runtime'] == 'codex'
                        and codex_archived_inactive(claim['session'])):
                    inactive.append(identity)
                elif claim['runtime'] == 'claude':
                    claude.setdefault(identity, []).append(item['state'])
                else:
                    protected.add(identity)
            else:
                uncertain.append(f'{task}: same-host claim of runtime {claim.get("runtime")!r}, session {claim.get("session")!r}')
        place = locality(found) if item.get('reservation') is not None else 'none'
        if place is None:
            uncertain.append(f'{task}: reservation {found.get("attempt") if isinstance(found, dict) else found!r} without a proven machine')
        elif place == 'local':
            session = worker.launch_session(item['iid'], found.get('attempt') or '')
            if found.get('runtime') not in CAPS or found.get('principal') != entry['principal'] or not (session or found.get('pid')):
                uncertain.append(f'{task}: reservation {found.get("attempt")!r} of runtime {found.get("runtime")!r}, principal '
                                 f'{found.get("principal")!r}, pid {found.get("pid")!r}: its owner or launch is unproven')
            else:  # a launch with its session, or with its launching process that native reconcile can settle
                identity = f'session:{session}' if session else f'reservation:{task}:{found.get("attempt")}'
                held.append(([found['runtime']], identity))
                protected.add(identity)
    if claude:
        rows = claude_rows()
        for identity, states in claude.items():
            session = identity.removeprefix('session:')
            if all(state in ('review', 'ask', 'later') for state in states) and claude_executor_ended(session, entry['checkout'], rows):
                inactive.append(identity)
            else:
                protected.add(identity)
    owned = {item['iid'] for item in everything if (item['claim'] or {}).get('session') or item.get('reservation')}
    for issue in core.issues(f'state=opened&my_reaction_emoji={core.LOCK}'):
        if issue['iid'] not in owned:  # a take or launch between its lock and its write, or one that died there
            uncertain.append(f'{where}#{issue["iid"]}: tracker lock without a claim or reservation')
    taken = core.room(everything, dict.fromkeys(core.RUNTIMES, 0))
    return {'status': 'ok', 'L': {runtime: -count for runtime, count in taken.items()}, 'held': held, 'inactive': inactive,
            'protected': sorted(protected), 'uncertain': uncertain}


def checked_read(found):
    """A readback as admission may use it. Off-shape is unknown; listed uncertainty is `uncertain`: both refuse."""
    try:
        if found['status'] in ('refused', 'failed') and all(isinstance(error, str) for error in found['errors']):
            return found
        valid = (found['status'] == 'ok' and set(found) == {'status', 'L', 'held', 'inactive', 'protected', 'uncertain'}
                 and all(isinstance(runtime, str) and type(count) is int and count >= 0 for runtime, count in found['L'].items())
                 and all(len(pair) == 2 and isinstance(pair[0], list) and pair[0] and all(runtime in CAPS for runtime in pair[0])
                         and isinstance(pair[1], str) and pair[1].startswith(('session:', 'reservation:')) for pair in found['held'])
                 and isinstance(found['held'], list) and isinstance(found['uncertain'], list)
                 and all(isinstance(identity, str) and identity.startswith('session:') for identity in found['inactive'])
                 and all(isinstance(identity, str) and identity.startswith(('session:', 'reservation:')) for identity in found['protected'])
                 and all(isinstance(reason, str) for reason in found['uncertain']))
    except (KeyError, TypeError, AttributeError):
        valid = False
    if not valid:
        return {'status': 'failed', 'errors': ['occupancy readback malformed: ownership unknown']}
    return {**found, 'status': 'uncertain', 'errors': found['uncertain']} if found['uncertain'] else found


def read_occupancy(binding, errors):
    """One bounded read-only occupancy subprocess of one catalog binding; anything but `ok` is unknown occupancy."""
    if errors:
        return {'status': 'refused', 'errors': errors}
    try:
        code, out, err = run_bounded(OCCUPANCY + [json.dumps(binding)], Path(binding['checkout']).expanduser(), binding.get('timeout', TIMEOUT))
        found = json.loads(out) if code == 0 else None
    except (OSError, ValueError) as error:
        return {'status': 'failed', 'errors': [f'occupancy reader: {error}']}
    if not isinstance(found, dict) or found.get('status') not in ('ok', 'refused'):
        return {'status': 'failed', 'errors': [f'occupancy reader exit {code}: ' + core.codex_line((err or b'').decode(errors='replace')[-300:])]}
    return found


def budget(caps, inventory, reads, own, limits):
    """Per runtime: the conservative occupancy of host and whole catalog, F = max(0, cap - occupancy) and the native
    limit min(project limit, L + F). Two grants count once only as the same exact session; an unknown inventory or
    any unread catalog binding makes F zero for every runtime it may hold (all of them, for a binding)."""
    complete = all(read['status'] == 'ok' for read in reads)
    found = {}
    for runtime, cap in caps.items():
        known = complete and inventory.get(runtime) is not None
        live = set(inventory.get(runtime) or ())
        held = live | {identity for read in reads if read['status'] == 'ok'
                       for runtimes, identity in read['held'] if runtime in runtimes}
        inactive = {identity for read in reads if read['status'] == 'ok' for identity in read.get('inactive', ())}
        protected = {identity for read in reads if read['status'] == 'ok' for identity in read.get('protected', ())}
        held -= inactive - live - protected
        free = max(0, cap - len(held)) if known else 0
        local = own['L'].get(runtime, 0)
        found[runtime] = {'cap': cap, 'occupancy': len(held), 'known': known, 'F': free, 'L': local,
                          'project_limit': limits.get(runtime, 0), 'limit': min(limits.get(runtime, 0), local + free)}
    return found


def workflow(binding, machine):
    """Blockers from the project's own workflow, never from the catalog: host and principal bindings and, for an
    acting binding, native effects its settings turn on that the catalog did not select and `taskq doctor` (local
    preflight only: no UI approval, PM transport or live readiness)."""
    from taskq.cleanup_schedule import settings
    blockers = []
    if core.machine() != machine:
        blockers.append(f'this host is {core.machine()!r}, the catalog binds {machine!r}')
    if (uid := core.user()) != binding['principal']:
        blockers.append(f'the tracker principal is {uid}, the catalog binds {binding["principal"]}; no delegation exists')
    if not binding['act']:
        return blockers
    needed = ['queue'] + ['cleanup'] * settings()['enabled'] + ['idle_stop'] * bool(core.personal().get('idle', {}).get('stop', 5))
    if missing := [effect for effect in needed if effect not in binding['effects']]:
        blockers.append(f'its settings turn on native effects {missing} the catalog did not select')
    with contextlib.redirect_stdout(io.StringIO()) as said:
        try:
            core.doctor(argparse.Namespace(fix=False, codex=binding['limits'].get('codex', 0) > 0))
        except SystemExit as error:
            if error.code:
                blockers.append('taskq doctor: ' + ' '.join(said.getvalue().split())[-300:])
    return blockers


def update_due():
    """An auto-update its tick would exec into: that exec would end the guarded pass."""
    if core.UPDATE['auto'] and (not core.UPDATE_STAMP.exists()
                                or time.time() - core.UPDATE_STAMP.stat().st_mtime >= core.seconds(core.UPDATE['every'])):
        return ['auto-update is due: run `taskq update` in the checkout first; its exec would end the guarded pass']
    return []


# --- the accepted catalog anchor (Wiki 5e238093 correction A-D) ---------------------------------------
# Owner execution-selection/version state only: never ownership, readiness or received/applied evidence.

def anchor_path():
    return core.UPDATE_STAMP.parent / 'multiproject-execution.json'


def canonical(policy, catalog):
    """The catalog's effective fields in one order: only table and list order is ignored. A refused binding has none."""
    if refused := [f'{project_result(binding, errors)["repository"]}: {"; ".join(errors)}' for binding, errors in catalog if errors]:
        raise SystemExit('execution catalog binding refused: ' + ' | '.join(refused))
    return {'policy': {key: policy[key] for key in ('version', 'machine', 'os_user', 'caps')},
            'project': sorted((canonical_binding(binding) for binding, _ in catalog), key=lambda binding: json.dumps(binding, sort_keys=True))}


def canonical_binding(binding):
    return {**{key: binding[key] for key in ('provider', 'repository_id', 'repository', 'board', 'principal', 'act')},
            'host': binding['host'].lower(), 'checkout': str(Path(binding['checkout']).expanduser().resolve()),
            'timeout': binding.get('timeout', ACT_TIMEOUT), 'limits': {runtime: binding['limits'].get(runtime, 0) for runtime in CAPS},
            'effects': sorted(set(binding['effects']))}


def digest(snapshot):
    import hashlib
    return hashlib.sha256(json.dumps(snapshot, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


ANCHOR_KEYS = {'version', 'generation', 'sha256', 'catalog', 'accepted_at', 'accepted_by', 'note'}


def read_anchor(machine):
    """The accepted anchor of this OS user on `machine` (the policy's host, which every binding check verifies
    against its project), or None when there is none. Unreadable, a symlink, another owner or mode, another version,
    a catalog off the enrollment schema, another OS user or machine: SystemExit, the file left as it is; never reset."""
    path = anchor_path()
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except FileNotFoundError:
        return None
    except OSError as error:
        raise SystemExit(f'execution anchor {path}: {error}; refused, left as it is') from None
    try:
        found = os.fstat(fd)
        if not stat.S_ISREG(found.st_mode) or found.st_uid != os.getuid() or found.st_mode & 0o022:
            raise SystemExit(f'execution anchor {path} is not a file of OS user {os.getuid()} alone; refused, left as it is')
        with os.fdopen(os.dup(fd), 'rb') as handle:
            text = handle.read(MAX_OUTPUT + 1)
    finally:
        os.close(fd)
    try:
        anchor = json.loads(text)
        catalog = anchor['catalog']
        valid = (set(anchor) == ANCHOR_KEYS and anchor['version'] == ANCHOR_VERSION and type(anchor['generation']) is int
                 and anchor['generation'] >= 1 and all(isinstance(anchor[key], str) for key in ('sha256', 'accepted_at', 'accepted_by', 'note'))
                 and anchor['sha256'] == digest(catalog) and set(catalog) == {'policy', 'project'} and isinstance(catalog['policy'], dict)
                 # Its catalog must be exactly what enrollment derives from such a policy: every shape, type and range.
                 and canonical(*parse_policy({**catalog['policy'], 'project': json.loads(json.dumps(catalog['project']))}, path)) == catalog)
    except (ValueError, KeyError, TypeError, IndexError, AttributeError, SystemExit):
        valid = False
    if not valid:
        raise SystemExit(f'execution anchor {path} is malformed or of another version; refused, left as it is')
    domain = (catalog['policy']['os_user'], catalog['policy']['machine'])
    if domain != (os.getuid(), machine):
        raise SystemExit(f'execution anchor {path} belongs to OS user {domain[0]} on {domain[1]!r}, not {os.getuid()} on {machine!r}; '
                         'refused, left as it is')
    return anchor


def write_anchor(anchor):
    write_atomic(anchor_path(), json.dumps(anchor, sort_keys=True, indent=1).encode())


def write_atomic(path, data):
    """Replace `path` atomically: a draft in the same folder, fsync, rename, fsync of the folder."""
    draft = path.with_name(f'.{path.name}.{os.getpid()}')
    fd = os.open(draft, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        with os.fdopen(fd, 'wb') as out:  # a whole write: os.write may stop short
            out.write(data)
            out.flush()
            os.fsync(fd)
        os.replace(draft, path)
    except BaseException:
        draft.unlink(missing_ok=True)
        raise
    folder = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(folder)
    finally:
        os.close(folder)


def anchored(policy, catalog):
    """The anchor whose catalog equals this policy file exactly (table order aside); SystemExit otherwise."""
    anchor = read_anchor(policy['machine'])
    if anchor is None:
        raise SystemExit('no accepted execution catalog: the owner enrolls it with --accept-execution-policy first')
    if anchor['sha256'] != digest(canonical(policy, catalog)):
        raise SystemExit(f'the execution policy differs from accepted generation {anchor["generation"]}; '
                         'a change takes an explicit --accept-execution-policy replacement')
    return anchor


def binding_check(spec):
    """One binding of a candidate catalog, in its checkout: identity, then its workflow (no tick, no action)."""
    binding = spec['binding']
    return {'repository': repository_url(binding), 'errors': verify(binding) or workflow(binding, spec['machine'])}


def caps_only(anchor, snapshot):
    """Whether `snapshot` differs from the anchored catalog in its aggregate caps alone."""
    old = anchor['catalog']
    return old['project'] == snapshot['project'] and {**old['policy'], 'caps': None} == {**snapshot['policy'], 'caps': None}


def settled(anchor, caps=None):
    """Blockers against replacing `anchor`: a fresh read-only readback of every anchored binding must show no
    same-host claim, reservation or ownerless lock, and every required runtime inventory must be readable and show
    no live session in an anchored checkout. Unknown never settles; nothing is released. `caps`: a caps-only
    replacement, whose bindings stay: their known grants remain and keep counting, so the readback must only be
    complete and known, and the inventory of each runtime whose cap changes readable."""
    old = anchor['catalog']['project']
    blockers = []
    for binding in old:
        read = checked_read(read_occupancy(binding, []))
        where = repository_url(binding)
        if read['status'] != 'ok':
            blockers.append(f'{where}: ownership unknown: ' + '; '.join(read.get('errors', [])))
        elif read['held'] and caps is None:
            blockers.append(f'{where}: still owns ' + ', '.join(identity for _, identity in read['held']))
    if caps is not None:
        changed = [runtime for runtime in CAPS if caps[runtime] != anchor['catalog']['policy']['caps'][runtime]]
        read = {'claude': lambda: claude_inventory(claude_rows()), 'codex': lambda: codex_inventory(codex_threads())}
        return blockers + [f'{runtime} inventory unknown: its cap change cannot be checked' for runtime in changed if read[runtime]() is None]
    checkouts = [Path(binding['checkout']) for binding in old]
    inside = lambda cwd: any(Path(cwd or '/').resolve().is_relative_to(checkout) for checkout in checkouts)  # noqa: E731
    rows, threads = claude_rows(), codex_threads()
    for runtime, found, live in (('claude', rows if claude_inventory(rows) is not None else None,
                                  lambda row: row.get('state') not in CLAUDE_ENDED and inside(row.get('cwd'))),
                                 ('codex', threads, lambda thread: codex_inventory([thread]) and inside(thread.get('cwd')))):
        if found is None:
            if any(binding['limits'][runtime] for binding in old):
                blockers.append(f'{runtime} inventory unknown: anchored grants cannot be settled')
        elif sessions := [item.get('sessionId') or item.get('id') for item in found if live(item)]:
            blockers.append(f'{runtime} sessions still run in anchored checkouts: {", ".join(map(str, sessions))}')
    return blockers


def accept(policy_path, check=None):
    """`--accept-execution-policy`: the owner's explicit enrollment or replacement of the catalog anchor, all under
    the host guard; no native tick or action. A replacement needs every previous binding settled first; one that
    changes the aggregate caps alone needs their ownership known, never released."""
    result = {'status': 'refused', 'errors': [], 'anchor': str(anchor_path()), 'generation': None, 'sha256': None, 'bindings': []}
    try:
        policy, catalog = load_policy(policy_path)
        snapshot = canonical(policy, catalog)
        fd, _ = guard(policy)
    except (SystemExit, OSError, ValueError) as error:
        return {**result, 'errors': [str(error)]}
    try:
        anchor = read_anchor(policy['machine'])
        if anchor and anchor['sha256'] == digest(snapshot):
            return {**result, 'status': 'unchanged', 'generation': anchor['generation'], 'sha256': anchor['sha256']}
        for binding, _ in catalog:
            try:
                code, out, err = run_bounded((check or CHECK) + [json.dumps({'binding': binding, 'machine': policy['machine']})],
                                             Path(binding['checkout']).expanduser(), binding.get('timeout', TIMEOUT))
                found = json.loads(out) if code == 0 else {'repository': repository_url(binding), 'errors': [f'check exit {code}']}
            except (OSError, ValueError) as error:
                found = {'repository': repository_url(binding), 'errors': [f'check: {error}']}
            result['bindings'].append(found)
        blockers = [f'{found["repository"]}: {error}' for found in result['bindings'] for error in found['errors']]
        if anchor:
            blockers += settled(anchor, snapshot['policy']['caps'] if caps_only(anchor, snapshot) else None)
        if blockers:
            return {**result, 'errors': blockers}
        generation = anchor['generation'] + 1 if anchor else 1
        write_anchor({'version': ANCHOR_VERSION, 'generation': generation, 'sha256': digest(snapshot), 'catalog': snapshot,
                      'accepted_at': now(), 'accepted_by': core.who(), 'note': 'execution selection only; no ownership, readiness or receipt'})
        return {**result, 'status': 'replaced' if anchor else 'enrolled', 'generation': generation, 'sha256': digest(snapshot)}
    except (SystemExit, OSError, ValueError) as error:
        return {**result, 'errors': [str(error)]}
    finally:
        os.close(fd)


def native_pass(view, limits):
    """(exit code, stdout) of the existing single-project `tick --act --json` in this process, under its checkout lock."""
    argv = ['tick', '--act', '--json', '--limit', ','.join(f'{runtime}={count}' for runtime, count in limits.items())]
    if view.get('filter') is not None:
        argv += ['--filter', view['filter']]
    if 'mine' in view:
        argv.append('--mine' if view['mine'] else '--no-mine')
    with contextlib.redirect_stdout(io.StringIO()) as out:
        try:
            core.main(argv)
            code = 0
        except SystemExit as error:
            code = error.code if isinstance(error.code, int) else 2
    return code, out.getvalue()


def act_project(spec, record=lambda found, policy, catalog, binding: None):
    """One admitted project in this one process: the host guard first, held without a gap through final admission,
    the complete-catalog readback and the native pass, released at this process's end (a dead wrapper does not end
    it; a crash does, and leaves the inode). Status `unknown` once the native pass may have run unread. `record`
    gets the result while the guard is still held: the actor's output record is complete before the guard frees."""
    entry = spec['entry']
    result = {'status': 'refused', 'errors': [], 'guard': None, 'budget': None, 'catalog': None, 'native': None, 'report': None}
    policy = catalog = binding = None
    try:
        policy, catalog = load_policy(spec['policy'])
        binding = binding_for(entry, catalog)
        fd, held = guard(policy)
    except (SystemExit, OSError, ValueError) as error:
        record(found := {**result, 'errors': [str(error)]}, policy, catalog, binding)
        return found
    result['guard'] = {'path': str(guard_path()), 'device': held.st_dev, 'inode': held.st_ino, 'pid': os.getpid(),
                       'domain': guard_domain(policy)}
    try:
        record(found := admitted(entry, policy, catalog, binding, result), policy, catalog, binding)
        return found
    finally:
        os.close(fd)


def admitted(entry, policy, catalog, binding, result):
    """`act_project` under the held guard: final admission, readback, budget and the native pass."""
    try:
        anchored(policy, catalog)
        if blockers := verify(binding) or workflow(binding, policy['machine']) or update_due():
            return {**result, 'errors': blockers}
        reads = [checked_read(read_occupancy(other, errors)) for other, errors in catalog]
        own = next(read for (other, _), read in zip(catalog, reads) if other is binding)
        result['catalog'] = [{'repository': project_result(other, errors)['repository'], 'status': read['status'],
                              'errors': read.get('errors', []), 'L': read.get('L')} for (other, errors), read in zip(catalog, reads)]
        # Unknown ownership or an unreadable inventory the project touches refuses before native_pass: its zero
        # limits would still let the pass release, move and reconcile.
        if unread := [f'{found["repository"]}: {found["status"]} ({"; ".join(found["errors"])})' for found in result['catalog'] if found['status'] != 'ok']:
            return {**result, 'errors': ['catalog ownership unknown, nothing run: ' + ', '.join(unread)]}
        view = entry.get('view', {})
        limits = {runtime: min(count, view.get('limits', {}).get(runtime, count)) for runtime, count in binding['limits'].items()}
        inventory = {'claude': claude_inventory(claude_rows()), 'codex': codex_inventory(codex_threads())}
        result['budget'] = budget(policy['caps'], inventory, reads, own, limits)
        if unknown := [runtime for runtime in CAPS if inventory[runtime] is None and (limits.get(runtime, 0) or own['L'].get(runtime, 0))]:
            return {**result, 'errors': [f'{" and ".join(unknown)} inventory unknown for a runtime this project holds or may start; nothing run']}
        # Above a (lowered) cap the pass would still release, reconcile and clean up: occupancy it may change or free.
        # Workers and claims stay; nothing is stopped or released to fit the cap. At the cap exactly, the pass runs with L.
        if over := [f'{runtime} occupancy {found["occupancy"]} above cap {found["cap"]}' for runtime, found in result['budget'].items()
                    if found['known'] and found['occupancy'] > found['cap'] and (limits.get(runtime, 0) or own['L'].get(runtime, 0))]:
            return {**result, 'errors': [f'{", ".join(over)} for a runtime this project holds or may start; workers kept, nothing run']}
        result['status'] = 'unknown'  # from here a native mutation may happen
        code, out = native_pass(view, {runtime: result['budget'].get(runtime, {}).get('limit', 0) for runtime in core.RUNTIMES})
        native = json.loads(out.strip().splitlines()[-1])
        report = native.get('report')
        problems = tick.validate_report(report)
        result.update(native={key: native.get(key) for key in ('outcome', 'actions', 'refusals')}, report=report,
                      status=('failed' if code not in (0, 1) else 'blocked' if problems else 'ok' if code == 0 else 'judgement_needed'),
                      errors=problems + (native.get('refusals') or []))
        return result
    except (Exception, SystemExit) as error:  # noqa: BLE001 - before the native pass a refusal, after it unknown
        return {**result, 'errors': result['errors'] + [f'{type(error).__name__}: {error}']}


def run_actor(command, cwd, timeout):
    """(returncode, stdout, stderr) of one actor in its own session; returncode None: still running at the timeout.
    Never killed: it holds the host guard through a native pass that a kill could cut in half."""
    child = subprocess.Popen(command, cwd=cwd, env=child_env(), stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, start_new_session=True)
    try:
        out, err = child.communicate(timeout=timeout)
        return child.returncode, out, err
    except subprocess.TimeoutExpired:
        return None, b'', b''


ACT_STATUS = ('ok', 'judgement_needed', 'blocked', 'failed', 'refused', 'unknown')
ACT_KEYS = ('status', 'errors', 'guard', 'budget', 'catalog', 'native', 'report')


def checked_result(found):
    """An actor result as the wrapper may use it, its report revalidated now; None: no result."""
    if not isinstance(found, dict) or found.get('status') not in ACT_STATUS:
        return None
    found['errors'] = list(found.get('errors') or [])
    if found.get('report') is not None and found['status'] in ('ok', 'judgement_needed'):
        if problems := [problem for problem in tick.validate_report(found['report']) if problem not in found['errors']]:
            found.update(status='blocked', errors=problems + found['errors'])
    return {key: found.get(key) for key in ACT_KEYS}


def acting(binding, entry, policy_path, actor):
    """One actor for one admitted entry; its JSON or an explicit unknown, never empty success. Its output record is
    allocated first: at the record limit, or without a protected record folder, nothing starts."""
    timeout = binding.get('timeout', ACT_TIMEOUT)
    try:
        run = allocate_record()
    except (SystemExit, OSError) as error:
        return {'status': 'refused', 'errors': [f'actor output record: {error}; nothing run']}
    recovery = {'run_id': run, 'recovery': f'python -m taskq.multiproject --recover-actor-output {run} --execution-policy '
                                          f'{Path(policy_path).resolve()} --json'}
    try:
        code, out, err = run_actor(actor + [json.dumps({'entry': entry, 'policy': str(Path(policy_path).resolve()), 'run': run})],
                                   Path(entry['checkout']).expanduser(), timeout)
    except OSError as error:
        return {'status': 'failed', 'errors': [f'actor did not start: {error}'], **recovery}
    if code is None:
        return {'status': 'unknown', 'errors': [f'actor still running after {timeout} s: kept with its host guard; outcome unknown; '
                                                f'its actual output is recoverable with run {run} once it ends'], **recovery}
    try:
        found = checked_result(json.loads(out) if len(out) <= MAX_OUTPUT else None)
    except ValueError:
        found = None
    if found is None:
        tail = core.codex_line(err.decode(errors='replace')[-300:]) if err else 'no output'
        return {'status': 'unknown', 'errors': [f'actor exit {code} without a result ({tail}); its effects are unknown'], **recovery}
    return {**found, **recovery}


# --- actor output records (#217): one bounded record of one actor's actual output, never a receipt ---------

MAX_RECORDS = 32  # files in the record folder, any kind; at the limit the next actor is refused, nothing evicted
MAX_LOG = 64 << 10  # bytes of an actor's diagnostics in its record
RECORD_KEYS = {'version', 'run_id', 'completed_at', 'policy_sha256', 'binding_sha256', 'os_user', 'machine', 'repository',
               'actor', 'result', 'log'}
RUN_ID = re.compile(r'[0-9a-f]{32}')


def record_folder(create=False):
    """The protected record folder beside the guard: a real folder of this OS user alone; SystemExit otherwise."""
    folder = core.UPDATE_STAMP.parent / 'multiproject-output'
    if create:
        folder.mkdir(mode=0o700, parents=True, exist_ok=True)
    found = os.lstat(folder)
    if not stat.S_ISDIR(found.st_mode) or found.st_uid != os.getuid() or found.st_mode & 0o077:
        raise SystemExit(f'{folder} is not a folder of OS user {os.getuid()} alone; refused, left as it is')
    return folder


def record_path(run, create=False):
    if not isinstance(run, str) or not RUN_ID.fullmatch(run):
        raise SystemExit(f'{run!r} is not a run id')
    return record_folder(create) / f'{run}.json'


def allocate_record():
    """A fresh run id with its empty record, created exclusively 0600; counted after creation, so two racing wrappers
    both refuse rather than pass the limit. Only its own just-created empty file is ever removed."""
    import uuid
    run = uuid.uuid4().hex
    path = record_path(run, create=True)
    os.close(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600))
    if (count := len(os.listdir(path.parent))) > MAX_RECORDS:
        path.unlink()
        raise SystemExit(f'{count - 1} records at the limit {MAX_RECORDS}; recover them and remove resolved ones by hand '
                         '(docs/multiproject-pm.md § Actor output records)')
    return run


def empty_record(run):
    """The path of `run`'s allocated record that no actor has completed yet; SystemExit otherwise."""
    path = record_path(run)
    found = os.lstat(path)
    if not stat.S_ISREG(found.st_mode) or found.st_uid != os.getuid() or found.st_mode & 0o077 or found.st_size:
        raise SystemExit(f'actor output record {path} is not an allocated empty record; refused, nothing run')
    return path


def size(value):
    """Bytes of `value` as its record stores it: UTF-8, so escaping never inflates a bounded output or diagnostics."""
    return len(json.dumps(value, sort_keys=True, ensure_ascii=False).encode())


def complete_record(run, found, policy, catalog, binding, log):
    """The actor's atomic completion write of its actual output; a result over MAX_OUTPUT leaves the record empty.
    Diagnostics keep their newest part that fits MAX_LOG as stored."""
    while size(log) > MAX_LOG:
        log = log[-(len(log) * 3 // 4):]
    record = {'version': 1, 'run_id': run, 'completed_at': now(), 'os_user': os.getuid(), 'actor': {'pid': os.getpid()},
              'policy_sha256': digest(canonical(policy, catalog)) if policy else None,
              'binding_sha256': digest(canonical_binding(binding)) if binding else None,
              'machine': policy and policy['machine'], 'repository': binding and repository_url(binding),
              'result': {key: found[key] for key in ACT_KEYS}, 'log': log}
    if size(record['result']) > MAX_OUTPUT:
        raise SystemExit('actual output above 1 MiB: the record stays incomplete')
    write_atomic(empty_record(run), json.dumps(record, sort_keys=True, ensure_ascii=False).encode())


MAX_RECORD = MAX_OUTPUT + MAX_LOG + 4096  # result, diagnostics and the small provenance fields


def read_record(run):
    """`run`'s completed record, or SystemExit why its output is unavailable: missing, empty (pending or crashed),
    a symlink, another owner or mode, oversized, truncated or not a table of exactly its keys. Read only."""
    path = record_path(run)
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError as error:
        raise SystemExit(f'record {path}: {error}') from None
    try:
        found = os.fstat(fd)
        if not stat.S_ISREG(found.st_mode) or found.st_uid != os.getuid() or found.st_mode & 0o077:
            raise SystemExit(f'record {path} is not a file of OS user {os.getuid()} alone')
        with os.fdopen(os.dup(fd), 'rb') as handle:
            text = handle.read(MAX_RECORD + 1)
    finally:
        os.close(fd)
    if not text:
        raise SystemExit(f'record {path} is empty: its actor has not completed it (still running, crashed or cut off)')
    if len(text) > MAX_RECORD:
        raise SystemExit(f'record {path} is oversized')
    try:
        record = json.loads(text)
    except ValueError:
        record = None
    if not isinstance(record, dict) or set(record) != RECORD_KEYS:
        raise SystemExit(f'record {path} is truncated or malformed')
    return record


def guard_domain(policy):
    return f'OS user {policy["os_user"]} only; no host-global capacity is claimed'


def qualified(record, run, policy, catalog, held):
    """(binding, output) of a completed record whose every field matches exactly: schema, types, ranges, bounds,
    run, policy, binding, domain, actor and the full stable identity of the guard `held` locked now. SystemExit naming the first mismatch:
    its output stays unknown. `ok` and `judgement_needed` need a native outcome and a v1 report valid as of
    completion; no guard only for a refusal before admission, with nothing native."""
    def need(ok, what):
        if not ok:
            raise SystemExit(f'record does not match its {what}: output unknown')

    def whole(value, low=0):
        return type(value) is int and value >= low
    need(type(record['version']) is int and record['version'] == 1, 'version')
    need(record['run_id'] == run, 'run id')
    at = tick.report_timestamp(record['completed_at']) if isinstance(record['completed_at'], str) else None
    need(at is not None and at <= time.time() + 60, 'completion time')
    need(isinstance(record['policy_sha256'], str) and record['policy_sha256'] == digest(canonical(policy, catalog)), 'policy')
    found = record['binding_sha256']
    binding = next((binding for binding, _ in catalog if isinstance(found, str) and digest(canonical_binding(binding)) == found), None)
    need(binding is not None and binding['act'] and record['repository'] == repository_url(binding), 'binding')
    need(whole(record['os_user']) and record['os_user'] == os.getuid() == policy['os_user'], 'OS user')
    need(isinstance(record['machine'], str) and record['machine'] == policy['machine'], 'machine')
    actor = record['actor']
    need(isinstance(actor, dict) and set(actor) == {'pid'} and whole(actor['pid'], 1), 'actor pid')
    need(isinstance(record['log'], str) and size(record['log']) <= MAX_LOG, 'diagnostics bound')
    result = record['result']
    need(isinstance(result, dict) and set(result) == set(ACT_KEYS) and result['status'] in ACT_STATUS
         and isinstance(result['errors'], list) and all(isinstance(error, str) for error in result['errors'])
         and isinstance(result['budget'], (dict, type(None))) and isinstance(result['catalog'], (list, type(None)))
         and size(result) <= MAX_OUTPUT, 'result schema')
    status, guarded, native, report = (result[key] for key in ('status', 'guard', 'native', 'report'))
    if guarded is None:
        need(status == 'refused' and all(result[key] is None for key in ('budget', 'catalog', 'native', 'report')),
             'guard: only a refusal before admission has none')
    else:
        need(isinstance(guarded, dict) and whole(guarded.get('device')) and whole(guarded.get('inode'), 1) and guarded == {
            'path': str(guard_path()), 'device': held.st_dev, 'inode': held.st_ino, 'pid': actor['pid'], 'domain': guard_domain(policy)}, 'guard')
    if status == 'refused':
        need(native is None and report is None, 'refusal: it ran nothing native')
    if status in ('ok', 'judgement_needed', 'blocked', 'failed'):
        need(isinstance(native, dict) and set(native) == {'outcome', 'actions', 'refusals'}
             and native['outcome'] in ('ok', 'judgement_needed', 'failure', 'unknown', 'refused')
             and isinstance(native['actions'], list) and isinstance(native['refusals'], list), 'native outcome')
    if status in ('ok', 'judgement_needed'):
        need(report is not None and tick.validate_report(report, at) == [], 'v1 report')
    return binding, result


def recover(run, policy_path):
    """`--recover-actor-output`: the actual output of one actor run, read only. Pending while the guard is held;
    unknown without its exact completed record; refused while fresh catalog ownership or a needed inventory is
    unknown. It never runs a native pass, tick, take, spawn, release or enrollment, and never infers a receipt.
    It locks the existing guard read only, and holds it through every check: no actor starts while it qualifies,
    and a missing or invalid guard is unknown, never recreated."""
    result = {'run_id': run, 'status': 'unknown', 'errors': [], 'record': None, 'completed_at': None, 'output': None,
              'received_applied': 'unknown'}
    try:
        policy, catalog = load_policy(policy_path)
        anchored(policy, catalog)
        try:
            fd, held = guard(policy, create=False)
        except GuardHeld as error:  # an actor still runs: its record is not final yet
            return {**result, 'status': 'pending', 'errors': [str(error)]}
        except OSError as error:
            raise SystemExit(f'host guard: {error}; nothing created') from None
        try:
            return qualify(result, run, policy, catalog, held)
        finally:
            os.close(fd)
    except (SystemExit, OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        return {**result, 'errors': [str(error)]}


def qualify(result, run, policy, catalog, held):
    """`recover` under its read-only guard lock: the record, then the fresh readback."""
    result['record'] = str(record_path(run))
    record = read_record(run)
    try:
        binding, output = qualified(record, run, policy, catalog, held)
    except (KeyError, TypeError, AttributeError, ValueError, OSError) as error:
        raise SystemExit(f'record does not match its schema ({type(error).__name__}: {error}): output unknown') from None
    reads = [checked_read(read_occupancy(other, errors)) for other, errors in catalog]
    own = next(read for (other, _), read in zip(catalog, reads) if other is binding)
    errors = [f'{project_result(other, errors)["repository"]}: {read["status"]} ({"; ".join(read.get("errors", []))})'
              for (other, errors), read in zip(catalog, reads) if read['status'] != 'ok']
    inventory = {'claude': lambda: claude_inventory(claude_rows()), 'codex': lambda: codex_inventory(codex_threads())}
    errors += [f'{runtime} inventory unknown' for runtime in CAPS
               if (binding['limits'].get(runtime, 0) or (own.get('L') or {}).get(runtime, 0)) and inventory[runtime]() is None]
    if errors:
        return {**result, 'status': 'refused', 'errors': ['fresh readback unknown, recovery not qualified: ' + ', '.join(errors)]}
    return {**result, 'status': 'recovered', 'completed_at': record['completed_at'], 'output': output}


def act(manifest, policy_path, actor=None):
    """One acting attempt per admitted entry, serially, in manifest order. A known failure lets the next project
    run; an unknown outcome or a held guard admits nothing more in this invocation."""
    contract = tick.report_contract()
    policy, catalog = load_policy(policy_path)
    projects, stop, refused = [], None, None
    try:
        anchored(policy, catalog)
    except SystemExit as error:  # no actor starts: the whole invocation is refused
        refused = str(error)
    for entry, errors in load_manifest(manifest):
        result = project_result(entry, errors)
        if not errors and refused:
            result['errors'] = [refused]
        elif not errors and stop:
            result.update(status='not_admitted', errors=[stop])
        elif not errors:
            try:
                binding = binding_for(entry, catalog)
            except SystemExit as error:
                result['errors'] = [str(error)]
            else:
                if held := guard_free(policy):
                    result['errors'] = [held]
                    stop = f'{held}; nothing more is admitted in this invocation'
                else:
                    result.update(acting(binding, entry, policy_path, actor or ACTOR))
                    if result['status'] == 'unknown':
                        stop = f'{result["repository"]}: outcome unknown; nothing more is admitted in this invocation'
        projects.append(result)
    return {'multiproject': MANIFEST_VERSION, 'mode': 'act', 'manifest': str(Path(manifest).resolve()),
            'policy': str(Path(policy_path).resolve()), 'observed_at': now(),
            'contract': {key: contract[key] for key in ('version', 'sha256', 'source')},
            'outcome': 'ok' if all(project['status'] == 'ok' for project in projects) else 'blocked',
            'received_applied': 'unknown', 'projects': projects}


def render(result):
    lines = [f'Multiproject {"acting pass" if result.get("mode") == "act" else "observation"} v{result["multiproject"]} of {result["manifest"]}',
             f'Observed: {result["observed_at"]}; outcome: {result["outcome"]}; received/applied: unknown']
    for project in result['projects']:
        lines += ['', f'# {project["repository"]} ({project["provider"]} id {project["repository_id"]}, '
                      f'board {project["board"]}): {project["status"]}']
        lines += [f'Blocker: {error}' for error in project['errors']]
        if project.get('recovery'):
            lines.append(f'Actual output record: {project["recovery"]}')
        if project.get('budget'):
            lines.append('Limits: ' + ', '.join(
                f'{runtime}={found["limit"]} (L {found["L"]} + F {found["F"]}, occupancy {found["occupancy"]}/{found["cap"]}'
                f'{"" if found["known"] else " unknown"}, project {found["project_limit"]})' for runtime, found in project['budget'].items()))
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
    parser.add_argument('--act', action='store_true', help='one guarded native `tick --act` per admitted project')
    parser.add_argument('--execution-policy', metavar='FILE', help='the accepted execution catalog that --act requires')
    parser.add_argument('--accept-execution-policy', metavar='FILE', help='enroll or replace the accepted execution catalog; no tick')
    parser.add_argument('--recover-actor-output', metavar='RUN_ID', help="read one actor run's recorded actual output; read only")
    parser.add_argument('--enroll-check', metavar='SPEC', help=argparse.SUPPRESS)  # one candidate binding's checks, JSON
    parser.add_argument('--observe', metavar='ENTRY', help=argparse.SUPPRESS)  # the reader: one verified entry, JSON
    parser.add_argument('--occupancy', metavar='BINDING', help=argparse.SUPPRESS)  # one binding's ownership, JSON
    parser.add_argument('--actor', metavar='SPEC', help=argparse.SUPPRESS)  # one admitted project's acting pass, JSON
    args = parser.parse_args(argv)
    if args.observe or args.occupancy or args.enroll_check:
        with contextlib.redirect_stdout(sys.stderr):  # stdout carries only the result
            found = (observe(json.loads(args.observe)) if args.observe else occupancy(json.loads(args.occupancy)) if args.occupancy
                     else binding_check(json.loads(args.enroll_check)))
        return print(json.dumps(found))
    if args.accept_execution_policy:
        with contextlib.redirect_stdout(sys.stderr):
            found = accept(args.accept_execution_policy)
        print(json.dumps(found) if args.json else f'{found["status"]}: generation {found["generation"]} of {found["anchor"]}'
              + ''.join(f'\nBlocker: {error}' for error in found['errors']))
        if found['status'] == 'refused':
            raise SystemExit(1)
        return
    if args.actor:
        # Its output is one JSON line at the end; a dead wrapper's broken pipe can then cut nothing but that line.
        # The same result goes first to its output record, which outlives the wrapper (§ Actor output records).
        spec = json.loads(args.actor)

        def record(found, *loaded):
            with contextlib.suppress(SystemExit, OSError, ValueError, TypeError):  # unwritten: the record stays empty, unknown
                complete_record(spec['run'], found, *loaded, said.getvalue())
        with contextlib.redirect_stdout(io.StringIO()) as said, contextlib.redirect_stderr(said):
            try:
                empty_record(spec.get('run'))
            except (SystemExit, OSError) as error:
                found = {key: None for key in ACT_KEYS} | {'status': 'refused', 'errors': [str(error)]}
            else:
                found = act_project(spec, record)
        with contextlib.suppress(OSError):
            print(json.dumps({**found, 'log': said.getvalue()[-2000:]}), flush=True)
        return
    if args.recover_actor_output:
        if not args.execution_policy or args.manifest or args.act:
            parser.error('--recover-actor-output takes --execution-policy alone')
        with contextlib.redirect_stdout(sys.stderr):
            found = recover(args.recover_actor_output, args.execution_policy)
        print(json.dumps(found) if args.json else f'{found["status"]}: run {found["run_id"]}; received/applied: unknown'
              + ''.join(f'\nBlocker: {error}' for error in found['errors']))
        if found['status'] != 'recovered':
            raise SystemExit(1)
        return
    if not args.manifest:
        parser.error('--manifest is required')
    if args.act != bool(args.execution_policy):
        parser.error('--act and --execution-policy go together')
    result = act(args.manifest, args.execution_policy) if args.act else aggregate(args.manifest, reader)
    print(json.dumps(result) if args.json else render(result))
    if result['outcome'] != 'ok':
        raise SystemExit(1)


if __name__ == '__main__':
    main()
