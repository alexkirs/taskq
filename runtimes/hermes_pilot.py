#!/usr/bin/env python3
"""LOCAL-ONLY, opt-in TaskQ/Hermes/Codex topology qualification.

No default-home edits, credential copying, external Git remotes or publication.
Evidence, file board, seeded fixture checkout and bare origin stay under THIS
worktree's .taskq/. Research close uses its genuine local fetch/ancestry gate.
CLI guards prevent accidental forbidden commands; they are not a security sandbox.
Models require provider network; absolute binaries and network remain available.

Usage: python3 runtimes/hermes_pilot.py --run --hermes-home <isolated-home>
       --codex-home <explicit-home> --hermes-command '["/installed/python", "-m", "tui_gateway.entry"]'
An explicitly supplied distinct named Hermes profile may be outside the worktree
and use the supported root credential-pool fallback; the default Hermes home is
refused. Codex may explicitly use its existing ~/.codex login/rollout home.
The harness never writes Codex auth/config. No auth file contents are
read/copied/printed. --prepare-only checks local Git setup with no model calls.
"""
import argparse
import hashlib
import re
import importlib.util
import json
import os
from pathlib import Path
import shutil
import shlex
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent

# A real, local file board, not mocked TaskQ commands or a live board adapter.
FILE_BOARD = '''import contextlib, datetime, fcntl, json, pathlib
ROOT = pathlib.Path(__file__).resolve().parent
PATH = ROOT / 'issues.json'
@contextlib.contextmanager
def transaction():
    with open(ROOT / 'board.lock', 'a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        rows = json.loads(PATH.read_text())
        yield rows
        temporary = PATH.with_suffix('.tmp')
        temporary.write_text(json.dumps(rows))
        temporary.replace(PATH)
def stamp(row): row['updated_at'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
def list(state):
    with transaction() as rows:
        return [row for row in rows.values() if row['state'] == 'open' and (not state or 'q-' + state in row['labels'])]
def closed():
    with transaction() as rows:
        return [row for row in rows.values() if row['state'] == 'closed' and 'taskq-events' in row['labels']]
def ensure_event_label(): pass  # file board accepts arbitrary labels; no remote label resource exists
def get(n):
    with transaction() as rows: return rows[str(n)]
def add(title, body, labels):
    with transaction() as rows:
        n = len(rows) + 1
        rows[str(n)] = dict(iid=n, title=title, body=body, labels=labels, state='open', comments=[], url='local:' + str(n))
        stamp(rows[str(n)]); return n
def update(n, labels=None, body=None):
    with transaction() as rows:
        row = rows[str(n)]
        if labels is not None: row['labels'] = labels
        if body is not None: row['body'] = body
        stamp(row)
def comment(n, text):
    with transaction() as rows:
        rows[str(n)]['comments'].append(text); stamp(rows[str(n)])
def close(n):
    with transaction() as rows:
        rows[str(n)]['state'] = 'closed'; stamp(rows[str(n)])
import contextlib, sqlite3, uuid
GUARD_DB = pathlib.Path(__file__).with_name('coordination.sqlite')
def acquire(owner):
    with contextlib.closing(sqlite3.connect(GUARD_DB, timeout=5)) as db, db:
        db.execute('CREATE TABLE IF NOT EXISTS grant (slot INTEGER PRIMARY KEY CHECK (slot=1), token TEXT, owner TEXT)')
        token = str(uuid.uuid4())
        try:
            db.execute('INSERT INTO grant VALUES (1, ?, ?)', (token, owner))
        except sqlite3.IntegrityError:
            return None
        return token
def release(token):
    with contextlib.closing(sqlite3.connect(GUARD_DB, timeout=5)) as db, db:
        if db.execute('DELETE FROM grant WHERE slot=1 AND token=?', (token,)).rowcount != 1:
            raise RuntimeError('exact grant no longer present')
'''


def module(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    sys.modules[name] = result
    spec.loader.exec_module(result)
    return result


CANDIDATE_FILES = ('AGENTS.md', 'taskq.md', 'taskq.py', 'runtimes/hermes.py',
                   'runtimes/hermes_pilot.py', 'tests/test_single.py')


def provenance():
    def git(*args):
        return subprocess.check_output(['git', '-C', str(ROOT), *args], text=True, timeout=10).strip()
    return {'head': git('rev-parse', 'HEAD'), 'dirty': git('status', '--porcelain=v1', '--untracked-files=all'),
            'sha256': {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in CANDIDATE_FILES}}


def unchanged(before):
    if provenance() != before:
        raise ValueError('candidate HEAD/dirty state/file hashes changed during pilot; no PASS')


def calculation_command(command):
    # One specific fixture, not a shell evaluator or a general review oracle.
    if not isinstance(command, str):
        return False
    try:
        argv = shlex.split(command)
        if len(argv) == 3 and Path(argv[0]).name in ('bash', 'sh') and argv[1] in ('-c', '-lc'):
            argv = shlex.split(argv[2])  # Codex's command_execution includes its shell wrapper.
        if (len(argv) == 7 and argv[:2] == ['export', 'TASKQ_TASK=1']
                and argv[2] in ('TASKQ_RUNTIME=hermes', 'TASKQ_RUNTIME=codex') and argv[3] == '&&'):
            argv = argv[4:]  # Section 5's exact fixture prefix; no shell execution.
        return (len(argv) == 3 and Path(argv[0]).name == 'python3' and argv[1] == '-c'
                and bool(re.fullmatch(r'print\(\s*17\s*\+\s*25\s*\)', argv[2])))
    except ValueError:
        return False


def supervisor_calculation(history, turn, receipt):
    rows = history.get('messages', [])
    anchors = [i for i, row in enumerate(rows) if row.get('row_id') == receipt['user_row_id']]
    ends = [i for i, row in enumerate(rows) if row.get('row_id') == receipt['final_assistant_row_id']]
    if len(anchors) != 1 or len(ends) != 1 or anchors[0] >= ends[0]:
        raise ValueError('supervisor review history boundary unavailable')
    tools = {row.get('tool_call_id'): row for row in rows[anchors[0]+1:ends[0]] if row.get('role') == 'tool'}
    started = {}
    for event in turn.get('tools', []):
        payload = event.get('payload') or {}
        tid = payload.get('tool_id')
        if event.get('type') == 'tool.start':
            started[tid] = payload
        elif event.get('type') == 'tool.complete':
            row, start = tools.get(tid, {}), started.get(tid, {})
            args = payload.get('args') or {}
            result = payload.get('result') or {}
            if (row.get('name') == start.get('name') == payload.get('name') == 'terminal'
                    and row.get('args') == start.get('args') == args
                    and calculation_command(args.get('command')) and isinstance(result, dict)
                    and result.get('exit_code') == 0 and not result.get('error')
                    and str(result.get('output', '')).strip() == '42'):
                return {'tool_id': tid, 'user_row_id': receipt['user_row_id'],
                        'final_assistant_row_id': receipt['final_assistant_row_id'], 'result': result}
    raise ValueError('independent supervisor review lacks successful calculation tool evidence in exact review turn')


def worker_calculation(path, sid):
    active = None
    for line in path.read_text().splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if event.get('type') == 'thread.started':
            active = event.get('thread_id')
        item = event.get('item') or {}
        if (active == sid and event.get('type') == 'item.completed' and item.get('type') == 'command_execution'
                and item.get('status') == 'completed' and item.get('exit_code') == 0
                and calculation_command(item.get('command')) and str(item.get('aggregated_output', '')).strip() == '42'):
            return {'thread_id': sid, 'item_id': item.get('id'), 'exit_code': 0, 'output': '42'}
    raise ValueError('actual Codex worker log lacks successful calculation command evidence')


def run_identity(root):
    host = 'local-pilot-' + root.name.rsplit('-', 1)[-1]
    return {'host': host, 'manager_name': f'PM HRM local pilot ({host})'}


def record_supervisor(evidence, manager, sid):
    if sid == manager:
        raise ValueError('native supervisor SID equals manager SID; independent review refused')
    evidence.setdefault('sessions', {}).update(manager=manager, supervisor=sid)


def topology_failure(error):
    if type(error).__name__ == 'Unnamed':
        # Extract only the bridge's sanitized RPC method/code, never exception bodies.
        code = re.search(r'Hermes session\.title refused \(code (-?\d+)\)', str(error))
        if code:
            reason = 'native title conflict or invalid title' if code[1] == '4022' else 'native title RPC rejected'
            return f'Unnamed: session.title code {code[1]} ({reason}); use a fresh pilot fixture; do not alter prior sessions'
        if 'Hermes stored row/native title persistence not confirmed' in str(error):
            return 'Unnamed: native title/stored-row persistence not confirmed; inspect retained owner evidence'
        return 'Unnamed: native naming not confirmed; inspect retained owner evidence (exception body withheld)'
    return str(error) if isinstance(error, (ValueError, TimeoutError)) else type(error).__name__


def preflight(args):
    blockers = []
    for name, value, config in (('Hermes', args.hermes_home, 'config.yaml'),
                                ('Codex', args.codex_home, 'config.toml')):
        if not value:
            blockers.append(f'{name}: explicitly supplied pre-provisioned home is missing')
            continue
        home = Path(value).expanduser().resolve()
        defaults = {(Path.home() / '.hermes').resolve(), (Path.home() / '.codex').resolve()}
        named_profile = name == 'Hermes' and home.parent.name == 'profiles' and home.name != 'default'
        explicit_codex_default = name == 'Codex' and home == (Path.home() / '.codex').resolve()
        if not explicit_codex_default and (home in defaults or (ROOT not in home.parents and not named_profile)):
            blockers.append(f'{name}: supply a distinct isolated home or explicit named Hermes profile; default home refused')
        elif not (home / config).is_file():
            blockers.append(f'{name}: isolated {config} is missing')
    try:
        command = json.loads(args.hermes_command or '[]')
        if not (isinstance(command, list) and command and all(isinstance(x, str) for x in command)
                and command[-2:] == ['-m', 'tui_gateway.entry']):
            blockers.append('Hermes: supply installed Python -m tui_gateway.entry JSON argv')
    except ValueError:
        blockers.append('Hermes: launch argv is not valid JSON')
    if not shutil.which('codex'):
        blockers.append('Codex: actual CLI is unavailable on PATH')
    if not args.run:
        blockers.append('Authenticated execution is opt-in: --run is required')
    return blockers


def prepare(root):
    """Seed ONLY this disposable fixture; never push or touch the candidate's Git config."""
    for filename in ('taskq.py', 'taskq.md'):
        shutil.copyfile(ROOT / filename, root / filename)
    (root / 'runtimes').mkdir()
    shutil.copyfile(ROOT / 'runtimes/hermes.py', root / 'runtimes/hermes.py')
    (root / '.gitignore').write_text('.taskq/\norigin.git/\nbin/\nissues.json\nboard.lock\ncommands.log\nevidence.json\n__pycache__/\n')
    (root / 'AGENTS.md').write_text(
        'Read taskq.md. LOCAL-ONLY research pilot. Queue tool: python3 ' + str(root / 'taskq.py') + '. '
        'Only board.py/its local issues.json is the board; read the whole issue including comments with '
        'python3 -c "import board,json; print(json.dumps(board.get(1)))". '
        'Do not invoke gh/glab or any live board; their examples in briefs are replaced by this file-board read. '
        'Research needs no worktree, edits or new commit. Use git rev-parse origin/main for result SHA. '
        'BOTH worker and supervisor: calculation must be its own terminal tool call, containing only '
        'export TASKQ_TASK=1 TASKQ_RUNTIME=codex && python3 -c "print(17 + 25)" for the worker, or '
        'export TASKQ_TASK=1 TASKQ_RUNTIME=hermes && python3 -c "print(17 + 25)" for the supervisor. '
        'Do not chain any other commands onto the calculation. Look up the SHA with git rev-parse origin/main '
        'in the next separate terminal tool call, using its required TaskQ export prefix. '
        'The ONLY origin is this fixture origin.git; git fetch origin is permitted and required by close. '
        'No push/publication, external Git URLs, other worktrees, credentials/config/plugin/core changes. '
        'Supervisor: run, end turn; after review independently execute python3 -c "print(17 + 25)" in the terminal tool, then close with a verdict containing '
        'open: none, and end turn. Manager: never do the research or supervisor review/close.\n')
    config = {'root': root, 'board': 'board.py', 'publish': 'direct', 'workspace': 'external',
              'runtimes': {'hermes': 'runtimes/hermes.py'}, 'limits': {'codex': 1}, 'hosts': {}}
    (root / 'taskq.json').write_text(json.dumps({key: value for key, value in config.items() if key != 'root'}))
    git = shutil.which('git')
    if not git:
        raise ValueError('git is unavailable for disposable local fixture')
    env = {**os.environ, 'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': os.devnull, 'GIT_ALLOW_PROTOCOL': 'file'}
    # Remove environment-injected Git config; all fixture settings are explicit/local.
    for key in tuple(env):
        if key.startswith(('GIT_CONFIG_KEY_', 'GIT_CONFIG_VALUE_')) or key in ('GIT_CONFIG_COUNT', 'GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE'):
            env.pop(key)
    def run(*argv):
        done = subprocess.run([git, '-C', str(root), *argv], env=env, capture_output=True, text=True, timeout=30)
        if done.returncode:
            raise ValueError('local Git fixture setup failed: ' + argv[0])
        return done.stdout.strip()
    run('init', '--initial-branch=main')
    run('config', 'user.name', 'TaskQ local fixture')
    run('config', 'user.email', 'taskq-fixture@localhost')
    run('config', 'commit.gpgsign', 'false')
    run('config', 'core.hooksPath', str(root / 'empty-hooks'))
    run('config', 'protocol.allow', 'never')
    run('config', 'protocol.file.allow', 'always')
    run('add', 'taskq.py', 'taskq.md', 'taskq.json', 'AGENTS.md', '.gitignore', 'board.py', 'runtimes/hermes.py')
    run('commit', '-m', 'Local-only research fixture seed')
    seed = run('rev-parse', 'HEAD')
    origin = root / 'origin.git'
    run('clone', '--bare', '--no-hardlinks', str(root), str(origin))
    run('remote', 'add', 'origin', str(origin))
    run('fetch', 'origin')
    run('branch', '--set-upstream-to=origin/main', 'main')
    # Accidental CLI guards only: absolute binaries/network remain available; no OS sandbox.
    binary = root / 'bin'
    binary.mkdir()
    guard = '#!' + sys.executable + '\nimport os,subprocess,sys\n'
    (binary / 'git').write_text(guard +
        "for word in sys.argv[1:]:\n    if word in ('push','pull','clone','init','commit','commit-tree','remote','config','update-ref','worktree','reset','checkout','add'):\n        sys.exit('LOCAL-ONLY pilot: Git mutation/publication command refused')\n" +
        'os.execv(' + repr(git) + ', [' + repr(git) + '] + sys.argv[1:])\n')
    for command in ('gh', 'glab'):
        (binary / command).write_text(guard + "sys.exit('LOCAL-ONLY pilot: use board.py, no external board CLI')\n")
    for path in binary.iterdir():
        path.chmod(0o755)
    return config, seed


def run_topology(args, root, evidence):
    """Genuine models/TaskQ/Git only; prepare local origin before any model calls."""
    config, seed = prepare(root)
    evidence['seed_sha'] = seed
    identity = run_identity(root)
    evidence['run_identity'] = identity
    isolated = {key: os.environ[key] for key in ('PATH', 'LANG', 'HOME') if key in os.environ}
    isolated['PATH'] = str(root / 'bin') + os.pathsep + os.environ.get('PATH', os.defpath)
    isolated.update(HERMES_HOME=str(Path(args.hermes_home).expanduser().resolve()), CODEX_HOME=str(Path(args.codex_home).expanduser().resolve()),
                    TASKQ_HERMES_COMMAND=args.hermes_command, TASKQ_HOST=identity['host'], TMPDIR=str(root),
                    GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull, GIT_ALLOW_PROTOCOL='file')
    os.environ.clear()
    os.environ.update(isolated)
    taskq = module(root / 'taskq.py', 'taskq_local_pilot')
    taskq.CONFIG = config
    taskq.BOARD = taskq.load_file('board.py')
    runtime = taskq.load_file('runtimes/hermes.py')
    owners = []
    deadline = time.monotonic() + min(args.timeout, 600)

    def cli(*argv, manager=None):
        env = dict(isolated)
        if manager:
            env.update(TASKQ_RUNTIME='hermes', HERMES_SESSION_ID=manager)
        # CLONE and its origin are disposable local fixture repositories only.
        with open(root / 'commands.log', 'ab') as out:
            result = subprocess.run([sys.executable, str(root / 'taskq.py'), *argv], cwd=root,
                                    env=env, stdout=out, stderr=out, timeout=min(60, max(1, deadline - time.monotonic())))
        if result.returncode:
            raise ValueError('TaskQ command refused: ' + argv[0] + '; see retained commands.log')

    try:
        manager = runtime.spawn(identity['manager_name'],
            'You are the TaskQ manager of this isolated file-board project. Read AGENTS.md and taskq.md, '
            'Use python3 ' + str(root / 'taskq.py') + ' for every queue command. Run pm, then file exactly one '
            'research task with runtime codex: execute python3 -c \"print(17 + 25)\" to calculate independently, no file edits. Include Read AGENTS.md '
            'and use the local file board in its goal. Explicitly include in the task goal: BOTH worker and supervisor '
            'must calculate in their own separate terminal tool call containing only the required '
            'export TASKQ_TASK=1 TASKQ_RUNTIME=codex (worker) or TASKQ_RUNTIME=hermes (supervisor) prefix '
            'followed by && python3 -c "print(17 + 25)"; do not chain any other commands. '
            'Look up the SHA with git rev-parse origin/main in the next separate terminal tool call, with its required export prefix. '
            'Acceptance: answer is 42, and supervisor independently verifies it. '
            'Never run worker commands or review/close it yourself. End this turn after filing.', root)
        owners.append(manager)
        evidence['sessions'] = {'manager': manager}
        runtime.completed(manager, timeout=min(60, args.timeout))
        initial_turn = runtime.data_for(manager)['turn']
        evidence['stages'].append('native manager model turn with persisted receipt')
        while time.monotonic() < deadline:
            rows = taskq.BOARD.list(None)
            all_rows = json.loads((root / 'issues.json').read_text())
            if len(all_rows) != 1:
                raise ValueError('manager did not file exactly one task on the isolated file board')
            issue = all_rows['1']
            if issue['state'] == 'closed':
                current = json.loads(taskq.BLOCK.search(issue['body']).group(1))
                boss = current.get('supervisor') or {}
                if boss.get('runtime') != 'hermes' or not runtime.owned(boss.get('session', '')):
                    raise ValueError('close has no same-bridge native supervisor')
                record_supervisor(evidence, manager, boss['session'])
                comment = next((text for text in reversed(issue['comments']) if text.startswith('**close**')), '')
                if f"hermes:{boss['session'][:8]}" not in comment:
                    raise ValueError('no recorded independent native supervisor close')
                claim = current.get('claim') or {}
                if claim.get('runtime') != 'codex' or not claim.get('session') or (current.get('result') or {}).get('sha') != seed:
                    raise ValueError('closed research task lacks the actual Codex worker/seed result identity')
                result_note = next((text for text in reversed(issue['comments']) if text.startswith('**result**')), '')
                if f"codex:{claim['session'][:8]}" not in result_note or '42' not in result_note.partition('\n\n')[2]:
                    raise ValueError('no recorded Codex worker research answer')
                supervisor_proof = runtime.completed(boss['session'], timeout=max(1, min(60, deadline - time.monotonic())))
                review_data = runtime.data_for(boss['session'])
                review_turn = runtime.call(boss['session'], '_turn', turn=review_data['turn'])
                if 'review #1' not in review_turn.get('prompt', ''):
                    raise ValueError('native supervisor close has no worker-review model turn')
                evidence['supervisor_receipt'] = supervisor_proof['persisted_turn']
                evidence['supervisor_calculation'] = supervisor_calculation(
                    runtime.call(boss['session'], 'session.history'), review_turn, supervisor_proof['persisted_turn'])
                evidence['worker_calculation'] = worker_calculation(root / '.taskq' / 'T1.log', claim['session'])
                # The preceding loop's wait may already have delivered the closed event.
                delivery = runtime.data_for(manager)
                turn = runtime.call(manager, '_turn', turn=delivery['turn'])
                if delivery['turn'] == initial_turn or turn.get('prompt') != taskq.closed(1):
                    cli('wait', '--window', '0', manager=manager)
                    delivery = runtime.data_for(manager)
                    turn = runtime.call(manager, '_turn', turn=delivery['turn'])
                if delivery['turn'] == initial_turn or turn.get('prompt') != taskq.closed(1):
                    raise ValueError('manager turn is not the exact closed board outcome')
                proof = runtime.completed(manager)
                evidence['stages'].append('verified native manager closed-event model wake')
                evidence['manager_receipt'] = proof['persisted_turn']
                evidence['status'] = 'PASS'
                return
            current = taskq.parse(issue)
            if not current or (current.get('pm') or {}).get('session') != manager:
                raise ValueError('file-board manager identity invariant failed')
            claim, boss = current.get('claim') or {}, current.get('supervisor') or {}
            if boss.get('runtime') == 'hermes' and boss.get('session'):
                record_supervisor(evidence, manager, boss['session'])
            if boss.get('runtime') == 'hermes' and boss.get('session') and boss['session'] not in owners:
                owners.append(boss['session'])
                evidence['stages'].append('board-recorded native Hermes supervisor')
            if claim.get('session') and claim.get('runtime') != 'codex':
                raise ValueError('worker is not the actual Codex CLI runtime')
            if current['state'] == 'ask':
                raise ValueError('pilot entered ask; see local board comments for the actual invariant/auth blocker')
            # Explicit local ticks are interventions, never claimed as unattended delivery.
            cli('tick', '--quiet')
            if runtime.state(manager) == 'idle':
                cli('wait', '--window', '0', manager=manager)
            time.sleep(.25)
        raise TimeoutError('bounded topology deadline: independent research close/manager wake not observed')
    finally:
        # Explicitly owned fixture teardown; no TaskQ board transition is simulated here.
        cleanup = []
        for sid in reversed(owners):
            try:
                if runtime.owned(sid):
                    data = runtime.data_for(sid)
                    runtime.stop(sid)
                    cleanup.append({'session': sid, 'owner_dead': runtime.birth(data['owner']['pid']) is None,
                                    'gateway_dead': runtime.birth(data['owner']['gateway']['pid']) is None})
            except Exception as error:
                cleanup.append({'session': sid, 'blocker': type(error).__name__})
        evidence['cleanup'] = cleanup
        if any(row.get('blocker') or not row.get('owner_dead') or not row.get('gateway_dead') for row in cleanup):
            evidence['status'] = 'BLOCKED'
            evidence['blockers'].append('owned-process cleanup not confirmed; no topology PASS')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run', action='store_true')
    parser.add_argument('--hermes-home')
    parser.add_argument('--codex-home')
    parser.add_argument('--hermes-command')
    parser.add_argument('--auth-ready', action='store_true', help='compatibility flag; auth is checked by the actual runtime')
    parser.add_argument('--prepare-only', action='store_true', help='seed local file-board/Git fixture without model calls')
    parser.add_argument('--timeout', type=int, default=180)
    args = parser.parse_args(argv)
    root = ROOT / '.taskq' / ('hermes-pilot-' + uuid.uuid4().hex[:12])
    root.mkdir(parents=True, mode=0o700)
    (root / 'board.py').write_text(FILE_BOARD)
    (root / 'issues.json').write_text('{}')
    evidence = {'status': 'BLOCKED', 'topology': 'Hermes manager -> Hermes supervisor -> actual Codex CLI research -> independent close -> manager wake',
                'scope': 'local-only authorization and accidental CLI guards; no OS network isolation; provider network required',
                'candidate': provenance(), 'stages': [], 'blockers': [] if args.prepare_only else preflight(args), 'cleanup': [], 'model_processes_launched': False}
    if args.prepare_only:
        try:
            _, evidence['seed_sha'] = prepare(root)
            evidence['status'] = 'PREPARED'
        except Exception as error:
            evidence['blockers'].append('Local fixture preparation failed: ' + type(error).__name__)
    elif not evidence['blockers']:
        evidence['model_processes_launched'] = True
        try:
            run_topology(args, root, evidence)
        except (Exception, SystemExit) as error:
            evidence['status'] = 'BLOCKED'
            # Do not print provider exception bodies, config, prompts or credentials.
            evidence['blockers'].append('Real topology stopped: ' + topology_failure(error))
    try:
        unchanged(evidence['candidate'])
    except (Exception, SystemExit) as error:
        evidence['status'] = 'BLOCKED'
        evidence['blockers'].append('Candidate provenance changed/unavailable; no PASS')
    (root / 'evidence.json').write_text(json.dumps(evidence, indent=2))
    for blocker in evidence['blockers']:
        print('BLOCKED: ' + blocker)
    print('Evidence: ' + str(root.relative_to(ROOT)))
    return 0 if evidence['status'] in ('PASS', 'PREPARED') else 2


if __name__ == '__main__':
    sys.exit(main())
