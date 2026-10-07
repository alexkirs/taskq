"""`doctor`, `doctor --fix`, `init` and `update`: the onboarding checks, the setup they offer, the install's updates."""
import argparse
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys

import taskq as core


def green(sha):
    """Require exact-SHA trusted tests; identified Pages suites have separate site eligibility."""
    repo = '/'.join(core.REPO.rstrip('/').split('/')[-2:])

    def read(endpoint, key=None):
        command = ['gh', 'api', f'repos/{repo}/{endpoint}']
        if key:
            command += ['--paginate', '--slurp']
        done = subprocess.run(command, capture_output=True, text=True, timeout=60)
        if done.returncode:
            raise ValueError('unreadable CI')
        data = json.loads(done.stdout)
        if key and (not isinstance(data, list) or any(not isinstance(page[key], list) for page in data)):
            raise ValueError('invalid CI pages')
        return [item for page in data for item in page[key]] if key else data

    try:
        workflow = read('actions/workflows/tests.yml')
        if workflow['path'] != '.github/workflows/tests.yml' or workflow['state'] != 'active':
            return 'its required tests workflow is not active'
        workflows = read(f'actions/runs?head_sha={sha}&per_page=100', 'workflow_runs')
        checks = read(f'commits/{sha}/check-runs?filter=all&per_page=100', 'check_runs')
        if not isinstance(workflows, list) or not isinstance(checks, list):
            raise ValueError('invalid CI response')
        # Suite identity, not a job name, distinguishes tests and Pages from unrelated checks.
        trusted = [run for run in workflows if run['head_sha'] == sha
                   and run['repository']['full_name'] == repo]
        test_runs = [run for run in trusted if run['workflow_id'] == workflow['id']
                     and run['path'] == workflow['path'] and run['event'] in ('push', 'pull_request')]
        tests = [run for run in test_runs if run['event'] == 'push']
        if not tests:
            return 'it has no trusted exact-SHA tests run yet'
        latest = max(tests, key=lambda run: run['id'])
        mandatory = [latest, *[run for run in test_runs if run['event'] == 'pull_request']]
        if any(run['status'] != 'completed' for run in mandatory):
            return 'CI still running: tests'
        if any(run['conclusion'] != 'success' for run in mandatory):
            return 'CI failed: tests'
        pages = {run['check_suite_id'] for run in trusted
                 if (run['path'], run['event']) in (
                     ('dynamic/pages/pages-build-deployment', 'dynamic'),
                     ('.github/workflows/pages.yml', 'push'),
                     ('.github/workflows/pages.yml', 'pull_request'),
                     ('.github/workflows/pages.yml', 'workflow_dispatch'))}
        test_suites = {run['check_suite_id'] for run in tests}
        current = {}
        for check in checks:
            suite = check['check_suite']['id']
            if check['head_sha'] != sha:
                raise ValueError('wrong check SHA')
            actions = check['app']['slug'] == 'github-actions' and check['app']['id'] == 15368
            if actions and (suite in pages or (check['name'] == 'tests' and suite in test_suites
                                               and suite != latest['check_suite_id'])):
                continue
            key = (suite, check['app']['id'], check['name'])
            if key not in current or check['id'] > current[key]['id']:
                current[key] = check
        required_suites = {run['check_suite_id'] for run in mandatory}
        required = [check for check in current.values() if check['check_suite']['id'] in required_suites
                    and check['name'] == 'tests' and check['app']['slug'] == 'github-actions'
                    and check['app']['id'] == 15368]
        if {check['check_suite']['id'] for check in required} != required_suites:
            return 'it has no trusted exact-SHA tests check yet'
        if any(check['status'] != 'completed' for check in required):
            return 'CI still running: tests'
        if any(check['conclusion'] != 'success' for check in required):
            return 'CI failed: tests'
        waiting = [check['name'] for check in current.values() if check['status'] != 'completed']
        failed = [check['name'] for check in current.values() if check['status'] == 'completed'
                  and check['conclusion'] not in ('success', 'skipped', 'neutral')]
        return (f'CI failed: {", ".join(failed)}' if failed else
                f'CI still running: {", ".join(waiting)}' if waiting else None)
    except (OSError, subprocess.SubprocessError, ValueError, KeyError, TypeError):
        return 'its CI could not be read (gh api)'


def signed(where, sha):
    """None when the `stable` tag fetched into `where` points at `sha` and carries a signature by a key in SIGNERS."""
    if core.git('rev-parse', 'refs/tags/stable^{commit}', cwd=where) != sha:
        return 'the stable tag moved during the update'
    if core.git('-c', 'gpg.format=ssh', '-c', f'gpg.ssh.allowedSignersFile={core.SIGNERS}', 'verify-tag', 'refs/tags/stable', cwd=where) is None:
        return f'the stable tag has no valid signature by a key in {core.SIGNERS}'


def works(where):
    """The new code at least starts: `python3 -m taskq --version` in a fresh process."""
    try:
        return not subprocess.run([sys.executable, '-m', 'taskq', '--version'], cwd=where, capture_output=True, timeout=60).returncode
    except (OSError, subprocess.SubprocessError):
        return False


def update(args):
    """Bring this install to the `[update] ref` of REPO: fast-forward of an editable clone, else a reinstall from Git.
    Only to a commit whose CI passed and, for `stable`, a tag signed by a key in SIGNERS. True when it updated.
    A clone with uncommitted changes or commits the ref lacks is left alone; a clone that does not start after the
    fast-forward goes back."""
    say = print if args.verbose else (lambda text: None)
    kind, where = core.install()
    ref = core.UPDATE['ref']
    name = 'refs/heads/main' if ref == 'main' else 'refs/tags/stable'
    old, remote = core.version(), core.git('ls-remote', core.REPO, name, name + '^{}')
    if not remote:
        return say(f'update skipped: {core.REPO} did not answer (or has no {ref})')
    new = remote.splitlines()[-1].split()[0]  # a tag's commit is the peeled `^{}` line, the last one
    if kind is None:
        return print(f'not updated: this taskq is not installed from Git; reinstall: uv tool install --force git+{core.REPO}  (or pipx install --force …)')
    if new == (core.git('rev-parse', 'HEAD', cwd=where) if kind == 'clone' else where):
        return print(f'up to date {old}')
    if reason := green(new):
        return print(f'not updated to {ref} {new[:7]}: {reason}')
    if ref == 'stable':
        # A clone checks the tag in itself; another install in a bare repository kept next to the update stamp.
        store = where if kind == 'clone' else core.UPDATE_STAMP.parent / 'repo.git'
        if kind != 'clone' and not store.exists():
            store.parent.mkdir(parents=True, exist_ok=True)
            core.git('init', '-q', '--bare', str(store))
        if core.git('fetch', '-q', '--no-tags', core.REPO, '+refs/tags/stable:refs/tags/stable', cwd=store) is None:
            return say(f'update skipped: fetch of stable from {core.REPO} failed')
        if reason := signed(store, new):
            return print(f'not updated to stable {new[:7]}: {reason}')
    if kind == 'clone':
        if core.git('status', '--porcelain', '--untracked-files=no', cwd=where):
            return print(f'not updated: {where} has uncommitted changes')
        if ref == 'main' and core.git('fetch', '-q', core.REPO, 'main', cwd=where) is None:
            return say(f'update skipped: fetch from {core.REPO} failed')
        if core.git('merge-base', '--is-ancestor', 'HEAD', new, cwd=where) is None:
            return print(f'not updated: {where} has commits {ref} of {core.REPO} lacks')
        if core.git('merge', '-q', '--ff-only', new, cwd=where) is None:
            return print(f'not updated: fast-forward of {where} failed (`git -C {where} merge --ff-only {new[:7]}` says why)')
        if not works(where):
            core.git('reset', '-q', '--hard', 'ORIG_HEAD', cwd=where)
            return print(f'not updated: {new[:7]} does not start (`python3 -m taskq --version` failed); {where} is back at {old}')
    else:
        # pipx and uv tool by the receipt in their venv, pip otherwise; another installer reinstalls by hand.
        # ponytail: no start check after a reinstall; the clone is where code changes first land.
        prefix = Path(sys.prefix)
        command = [*(['pipx', 'install', '--force'] if (prefix / 'pipx_metadata.json').exists()
                     else ['uv', 'tool', 'install', '--force'] if (prefix / 'uv-receipt.toml').exists()
                     else [sys.executable, '-m', 'pip', 'install', '-q', '--force-reinstall']), f'git+{core.REPO}@{new}']
        try:
            subprocess.run(command, check=True, capture_output=True, text=True, timeout=600)
        except (OSError, subprocess.SubprocessError) as error:
            reason = (getattr(error, 'stderr', None) or str(error)).strip().splitlines()
            return print(f'not updated: `{shlex.join(command)}` failed: {reason[-1] if reason else error}')
    print(f'updated {old} → {new[:7]}')
    return True


def queue_labels():
    """Every label the queue uses: state, runtime, type, problem, priority, area."""
    return ([core.PREFIX + state for state in core.STATES] + [core.RUN + runtime for runtime in core.RUNTIMES] + list(core.TYPES) + [core.PROBLEM]
            + [f'priority-{level}' for level in core.PRIORITIES] + ['area-' + name for name in core.AREAS])


def probe(command):
    """The exit code of a read-only CLI check; None when the program is not installed."""
    try:
        return subprocess.run(command, capture_output=True, timeout=60).returncode
    except FileNotFoundError:
        return None
    except subprocess.TimeoutExpired:
        return 1


def api_read(command):
    """(exit code, stderr) of a read-only authorized API call; a missing program or a timeout is a failure too."""
    try:
        done = subprocess.run(command, capture_output=True, text=True, timeout=60)
        return done.returncode, done.stderr
    except (OSError, subprocess.TimeoutExpired) as error:
        return 1, str(error)


# Only a positively established authentication failure means login; every other failure is named as what it is.
# ponytail: known wordings of gh/glab and Go's net/tls packages; an unmatched one is `unknown`, never a login gap.
AUTH = re.compile(r'HTTP 401|\b401 Unauthorized|Bad credentials|Requires authentication|not logged in|auth login', re.I)
FAILURES = (('TLS/certificate failure', re.compile(r'x509|certificate|tls:? handshake', re.I)),
            ('network denied or offline', re.compile(r'error connecting to|dial tcp|no such host|could not resolve'
                                                     r'|connection refused|network is unreachable|timed? ?out', re.I)),
            ('permission denied for this token (HTTP 403)', re.compile(r'HTTP 403|\b403 Forbidden|not accessible by integration', re.I)),
            ('server error (HTTP 5xx)', re.compile(r'HTTP 5\d\d|\b5\d\d (Internal Server Error|Bad Gateway|Service Unavailable|Gateway Timeout)')))


def login_gap(cli, host):
    """#177: `auth status` also fails without network (seen live: «token invalid» in a sandbox that denied
    api.github.com). One supported authorized read, `<cli> api user`, confirms it: only an authentication
    failure (401) gives the owner's login step; network, TLS, 403, 5xx and an unrecognised or empty error are
    reported as that blocker, not as a login gap. (what, fix), or None when the read succeeds. Reads only."""
    hostname = ['--hostname', host] if host else []
    code, stderr = api_read([cli, 'api', 'user', *hostname])
    if not code:
        return None
    if AUTH.search(stderr):
        return (f'`{cli}` is not logged in{f" to {host}" if host else ""}',
                f'{cli} auth login{f" --hostname {host}" if host else ""}  (the person runs it: OAuth in the browser)')
    kind = next((label for label, pattern in FAILURES if pattern.search(stderr)), 'unknown failure')
    shown = core.last_line(stderr) or 'no error output'
    return (f'`{cli} api user` failed: {kind}, not a proven login gap ({shown})',
            f'{shlex.join([cli, "api", "user", *hostname])}  (rerun once the cause is fixed; do not run auth login for this)')


def origin_of():
    """(host, path) of this checkout's `origin`, None without one."""
    found = re.match(r'(?:\w+://)?(?:[^@/]+@)?([^:/]+)(?::\d+)?[:/](.+?)(?:\.git)?/?$', core.git('remote', 'get-url', 'origin') or '')
    return found and (found[1], found[2])


def write_config(github, where, host=None):
    """A minimal taskq.toml in the current directory, when none is there (`init --project/--github`, `doctor --fix`)."""
    if Path('taskq.toml').exists():
        return
    section = '[github]\nrepo' if github else '[gitlab]\nproject'
    Path('taskq.toml').write_text(f'# taskq: this project\'s task queue; keys: `taskq contract`, README of taskq.\n'
                                  f'{section} = "{where}"\n' + (f'host = "{host}"\n' if host else ''))
    print(f'wrote taskq.toml for {where}')


def doctor(args, pending=()):
    """Is this project ready for the queue? Prints each gap with the command that closes it, exit 1 while any is
    open or `pending` (the person's steps `setup` printed) is not empty; prints one line and exits 0 when none is.
    Reads only: no config, label, board or credential changes (manager onboarding: report, agree, then `init`).
    `--fix` is the manager's «do it for me»: `setup`."""
    if getattr(args, 'fix', False):
        return setup(args)
    for name in idle():
        print(f'runtime {name}: skipped, limit 0')
    gaps = []
    gap = lambda what, fix: gaps.append(f'- {what}\n    {fix}')
    origin = origin_of()
    if not origin:
        gap('no git remote `origin` in this checkout', 'git remote add origin <repository URL>')
    config = core.PROJECT_PATH is not None
    if not config:
        try:
            core.configure()
            config = True
        except SystemExit as error:
            if 'no taskq.toml' in str(error) and origin:
                kind = '--github' if 'gitlab' not in origin[0] else '--project'
                host = '' if origin[0] == 'github.com' else f' --host {origin[0]}'
                gap('no taskq.toml', f'taskq init {kind} {origin[1]}{host}  (writes taskq.toml, labels and board)')
            else:
                gap(str(error).removeprefix('taskq: '), 'fix taskq.toml (keys: `taskq contract`, README)')
    github = not core.BOARDS if config else bool(origin) and 'gitlab' not in origin[0]
    host = core.HOST or (origin[0] if origin else None)
    if config and origin and (origin[1].lower() != core.PROJECT_PATH.lower() or (core.HOST and origin[0] != core.HOST)):
        gap(f'origin is {origin[0]}/{origin[1]}, taskq.toml names {core.HOST or ""}{"/" * bool(core.HOST)}{core.PROJECT_PATH}',
            'run taskq from that project\'s checkout, or fix [github] repo / [gitlab] project and host in taskq.toml')
    if config:
        claude = 'claude' not in idle()
        for what, fix in personal_gaps() + tree_gaps() + (permissions_gap(core.ROOT) + trust_gap(core.ROOT) if claude else []):
            gap(what, fix)
        if claude and probe(['claude', 'auth', 'status']):  # None: no claude CLI on this machine, nothing to check
            gap('`claude` is not logged in: background workers stop at «Not logged in»',
                'claude auth login  (the person runs it in this shell, with the same `claude` the tick starts)')
        if getattr(args, 'codex', False) and 'codex' not in idle() and not core.CODEX_SOCKET.exists():
            gap(codex_gap(), f'{core.CODEX_HEADLESS}  (the person runs it; the Codex app, when installed, starts the same server)')
        for root in core.codex_writable() if 'codex' not in idle() else ():
            if not root.exists():
                print(f'warning: [codex] writable {root} does not exist: Codex workers run without it')
    if config or origin:
        cli = 'gh' if github else 'glab'
        status = probe([cli, 'auth', 'status', *(['--hostname', host] if host else [])])
        if status is None:
            gap(f'`{cli}` is not installed', f'brew install {cli}  (or the package manager of this machine)')
        elif status and (found := login_gap(cli, host)):
            gap(*found)
    if gaps or not config:
        return report_gaps(gaps, pending)
    checks = (('write permission', lambda: write_access(github)), ('labels', queue_labels_missing),
              ('board', lambda: board_gaps(github, host)), ('lease', lease_gaps))
    for name, check in checks:
        try:
            for what, fix in check():
                gap(what, fix)
        except SystemExit as error:
            gap(f'{name} could not be read: {str(error).removeprefix("taskq: ")}', 'fix the cause above, then `taskq doctor` again')
    gaps += runtime_gaps()
    report_gaps(gaps, pending)


# #160: the socket is Codex execution only; the tracker and the coordinator's tick work without it.
def codex_gap():
    return (f'no Codex app server at {core.CODEX_SOCKET}: Codex workers cannot start (tracker and tick do not need it); '
            'headless, the Codex CLI\'s app-server daemon serves it')


def personal_gaps():
    """The personal taskq.local.toml: missing, invalid or committed. Reads only."""
    if core.git('ls-files', '--error-unmatch', '--', core.LOCAL.name, cwd=core.LOCAL.parent) is not None:
        return [(f'{core.LOCAL} is tracked by git: personal settings are never committed',
                 f'cd {core.LOCAL.parent} && git rm --cached -- {core.LOCAL.name}  (keeps the local file), then commit')]
    if not core.LOCAL.is_file():
        return [(f'no personal profile {core.LOCAL} (areas, own tasks or pool, Claude/Codex slots of this machine)',
                 f'{core.TOOL} profile init  ({PROFILE_CARD})')]
    try:
        core.personal()
    except SystemExit as error:
        return [(str(error).removeprefix('taskq: '), f'fix {core.LOCAL} by hand (keys: `{core.TOOL} contract`, § Project)')]
    return []


# What `profile init` without flags writes, and the flags the person's confirmed card adds; taskq never guesses them.
PROFILE_CARD = ('built-in defaults: all areas, own tasks and the pool, ' + ', '.join(f'{name} {count}' for name, count in core.default_limits().items())
                + ' slots; confirm the profile card of taskq-manager.md § 1 first and add only its answers: --filter "labels=area-NAME", '
                '--mine or --no-mine, --limit claude=N,codex=M, --preferred-runtime claude|codex')


def ignore_local():
    """Exactly one line each for `/taskq.local.toml` and the task trees `/.worktrees/` in the main checkout's .gitignore
    (`init`, `profile init`); an unanchored line already there counts."""
    path = core.LOCAL.with_name('.gitignore')
    for line in ('/' + core.LOCAL.name, f'/{core.TREES}/'):
        text = path.read_text() if path.exists() else ''
        if not {line, line[1:]} & set(text.splitlines()):
            path.write_text(text + ('\n' if text and not text.endswith('\n') else '') + line + '\n')
            print(f'added {line} to {path}')
    if core.git('ls-files', '--error-unmatch', '--', core.LOCAL.name, cwd=core.LOCAL.parent) is not None:
        print(f'{core.LOCAL} is tracked by git: run `cd {core.LOCAL.parent} && git rm --cached -- {core.LOCAL.name}` (keeps the file), then commit')


def tree_gaps():
    """This project's task trees (`taskq-N`) outside `.worktrees/`, each with the command that moves it. Reads only:
    a worker may still run in an old tree, so the person moves it."""
    listed = core.git('worktree', 'list', '--porcelain', cwd=core.ROOT) or ''
    trees = [Path(line[len('worktree '):]) for line in listed.splitlines() if line.startswith('worktree ')]
    return [(f'task tree {tree} is outside {core.ROOT / core.TREES}',
             f'cd {core.ROOT} && mkdir -p {core.TREES} && git worktree move {shlex.quote(str(tree))} {core.TREES}/{tree.name}  '
             '(when no worker runs in it)')
            for tree in trees if re.fullmatch(r'taskq-\d+', tree.name) and tree.parent.resolve() != (core.ROOT / core.TREES).resolve()]


def profile_init(args):
    """`profile init`: write the person's confirmed profile to taskq.local.toml, built-in defaults for what no flag
    names; an existing file is never overwritten. Changes nothing else: no tracker, permissions, timer or worker."""
    if core.LOCAL.exists():
        core.fail(f'{core.LOCAL} exists: edit it by hand; `profile init` never overwrites it')
    mine = False if args.mine is None else args.mine
    text = ('# taskq: this person\'s profile on this machine; never committed (keys: `taskq contract`, § Project).\n'
            f'[profile]\nfilter = {json.dumps(args.filter or "")}\nmine = {str(mine).lower()}\n'
            + (f'preferred_runtime = "{args.preferred_runtime}"\n' if args.preferred_runtime else '')
            + '\n[profile.limits]\n' + ''.join(f'{name} = {count}\n' for name, count in {**core.default_limits(), **(args.limit or {})}.items()))
    core.LOCAL.write_text(text)
    print(f'wrote {core.LOCAL}')
    ignore_local()


def idle():
    """Runtimes this machine never starts (effective profile limit 0): doctor and `--fix` skip their checks and setup.
    Claude too (#160): a Codex-only machine's coordinator is not a Claude session, so its login, trust and
    permissions are no gap there."""
    try:
        limits = core.resolve(argparse.Namespace(filter=None, mine=None, limit=None))[0]['limits']
    except SystemExit:
        return []  # a broken profile is personal_gaps' gap
    return [name for name, count in limits.items() if not count]


def runtime_gaps():
    """Each [runtimes.<name>] `doctor` command, run from the main checkout: its own `- what / fix` lines as one gap
    while it exits nonzero. It runs without the session variables, like a worker of that app."""
    gaps = []
    for name, item in core.EXECUTORS.items():
        if not item.get('doctor') or name in idle():
            continue
        try:
            done = subprocess.run(shlex.split(item['doctor']), cwd=core.ROOT, env=core.selftest_env(), capture_output=True, text=True, timeout=120)
            code, output = done.returncode, done.stdout + done.stderr
        except (OSError, subprocess.TimeoutExpired) as error:
            code, output = 'not run', str(error)
        if code:
            fix = f'{item["setup"]}  (prints the steps)' if item.get('setup') else 'the lines above'
            lines = ''.join(f'\n    {line}' for line in output.strip().splitlines())
            gaps.append(f'- runtime {name}: `{item["doctor"]}` exit {code}{lines}\n    fix: {fix}')
    return gaps


def write_access(github):
    if github:
        push = core.api('GET', 'repository')['permissions']['push']
    else:  # Developer (30) or above may push and edit issues
        push = max((level or {}).get('access_level', 0) for level in core.api('GET', '/' + core.PROJECT)['permissions'].values()) >= 30
    return [] if push else [(f'this account cannot write to {core.PROJECT_PATH}',
                             f'ask an owner of {core.PROJECT_PATH} for write access (GitLab: Developer or above)')]


def queue_labels_missing():
    have = {label['name'] for label in core.pages('labels')}
    missing = [name for name in queue_labels() if name not in have]
    return [(f'labels missing: {", ".join(missing)}', 'taskq init')] if missing else []


def board_gaps(github, host):
    if not github:
        board = next((board for board in core.api('GET', 'boards') if board['name'] == core.BOARD), None)
        columns = board and [item['label']['name'] for item in sorted(board['lists'], key=lambda item: item['position'])]
        want = [core.PREFIX + state for state in core.STATES]
        return [] if columns == want else [(f'board {core.BOARD} ' + ('missing' if board is None else f'columns are {columns}, not {want}'), 'taskq init')]
    board = core.api('GET', 'board')
    if board is False:
        return [('the gh token lacks the scope `project`: no Projects v2 board', f'gh auth refresh -h {host or "github.com"} -s project  (the person confirms in the browser)')]
    if not board:
        return [(f'no Projects v2 board {core.BOARD}', 'taskq init')]
    return ([(f'board {core.BOARD} Status options are {list(board["options"])}, not {list(core.STATES)}', 'taskq init')]
            * (list(board['options']) != list(core.STATES)))


def lease_gaps():
    """#145: the coordinator is [coordinator] machine of taskq.toml; a ref of the old lease (#44) is only clutter."""
    refs = [] if core.BOARDS else core.api('GET', 'leases')
    return [(f'leftover coordinator lease {ref} (the coordinator is [coordinator] machine of taskq.toml now)',
             f'{core.TOOL} doctor --fix  (deletes it)') for ref in refs]


def report_gaps(gaps, pending=()):
    if not gaps and pending:
        print(f'not ready: {len(pending)} step(s) of the person pending: ' + '; '.join(pending))
        sys.exit(1)
    if not gaps:
        checked = [name for name, item in core.EXECUTORS.items() if item.get('doctor') and name not in idle()]
        return print(f'ready: {core.PROJECT_PATH} — config, CLI login, write access, labels and board {core.BOARD}'
                     + (f', runtime {", ".join(checked)}' if checked else ''))
    print(f'not ready: {len(gaps)} gap(s); each line is the command that closes it\n' + '\n'.join(gaps))
    sys.exit(1)


# What worker and coordinator sessions need in `<main checkout>/.claude/settings.local.json` (manager contract § 1,
# «Permissions»: the person merges it once). dontAsk runs the allow list silently and denies the rest: no prompt and no
# auto-mode classifier. Without it the user's own defaultMode (e.g. `auto`) applies to every session of the checkout.
PERMISSION_MODE = 'dontAsk'
WORKER_ALLOW = ('Bash', 'Read', 'Edit', 'Write', 'Glob', 'Grep', 'NotebookEdit', 'WebFetch', 'WebSearch', 'Agent', 'Skill',
                'ToolSearch', 'SendMessage', 'ListAgents', 'CronCreate', 'CronDelete', 'CronList', 'mcp__ccd_session_mgmt',
                'mcp__ccd_session', 'mcp__scheduled-tasks', 'mcp__serena')  # Cron*, ListAgents: the coordinator's (§ 2, § 3)


def permissions_missing(root):
    """WORKER_ALLOW entries and `defaultMode: dontAsk` the checkout's settings.local.json lacks (all of them without
    the file); reads only."""
    path = Path(root) / '.claude' / 'settings.local.json'
    try:
        permissions = json.loads(path.read_text()).get('permissions', {}) if path.exists() else {}
    except json.JSONDecodeError as error:
        core.fail(f'{path} is not valid JSON ({error}): fix it by hand, taskq does not overwrite it')
    return ([item for item in WORKER_ALLOW if item not in permissions.get('allow', [])]
            + [f'defaultMode: {PERMISSION_MODE}'] * (permissions.get('defaultMode') != PERMISSION_MODE))


def permissions_command(root):
    """One shell line that merges WORKER_ALLOW and dontAsk into the checkout's settings.local.json, keeping every
    other entry; invalid JSON stops it unchanged. The command of taskq-manager.md § 1 «Permissions»."""
    script = ('import json, pathlib; path = pathlib.Path(".claude/settings.local.json"); '
              'data = json.loads(path.read_text()) if path.exists() else {}; '
              'permissions = data.setdefault("permissions", {}); allow = permissions.setdefault("allow", []); '
              f'allow.extend(item for item in {json.dumps(WORKER_ALLOW)} if item not in allow); '
              f'permissions["defaultMode"] = "{PERMISSION_MODE}"; path.parent.mkdir(exist_ok=True); '
              'path.write_text(json.dumps(data, indent=2) + "\\n")')
    return f'cd {shlex.quote(str(root))} && python3 -c {shlex.quote(script)}'


def permissions_gap(root):
    """The doctor line for missing permissions: what is missing, why, and the one command that closes it."""
    missing = permissions_missing(root)
    return [(f'{root}/.claude/settings.local.json lacks {", ".join(missing)}: sessions of this checkout stop on '
             f'prompts or the auto-mode classifier', f'{permissions_command(root)}  (the person runs it once, '
             f'after the rules of taskq-manager.md § 1 «Permissions»; taskq never edits permission settings)')] if missing else []


def trust_gap(root):
    """The doctor line while Claude Code's folder trust for `root` is not accepted."""
    if trusted(root):
        return []
    config, keys = trust_keys(root)
    if config == core.CLAUDE_CONFIG:
        return [(f'Claude folder trust not accepted for {root}: worker sessions cannot start there',
                 f'cd {shlex.quote(str(root))} && claude  (the person accepts «Trust this folder» once, then quits)')]
    # #139: a Windows claude under WSL keeps trust in the Windows home under the UNC path; set that key directly.
    script = ('import json, pathlib; path = pathlib.Path(%r); data = json.loads(path.read_text()); '
              'data.setdefault("projects", {}).setdefault(%r, {})["hasTrustDialogAccepted"] = True; '
              'path.write_text(json.dumps(data, indent=2))') % (str(config), keys[0])
    return [(f'Claude folder trust not accepted for {keys[0]} (the Windows claude sees {root} so): worker sessions cannot start there',
             f'python3 -c {shlex.quote(script)}  (the person runs it once while no claude runs)')]


def windows_claude_binary():
    """True under WSL when `claude` is a Windows binary (e.g. the npm shim /mnt/c/nvm4w/nodejs/claude)."""
    found = shutil.which('claude')
    return bool(os.environ.get('WSL_DISTRO_NAME') and found and os.path.realpath(found).startswith('/mnt/'))


def windows_claude():
    """Under WSL with a Windows `claude` (`windows_claude_binary`): (UNC prefix of this
    distribution, the Windows ~/.claude.json). None elsewhere. That claude sees the checkout as //wsl.localhost/..."""
    distro = os.environ.get('WSL_DISTRO_NAME')
    if not windows_claude_binary():
        return None
    try:  # cmd.exe refuses a UNC working directory: run it from the Windows drive
        home = subprocess.run(['cmd.exe', '/c', 'echo %USERPROFILE%'], cwd='/mnt/c', capture_output=True, text=True, timeout=30).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return None
    if not re.fullmatch(r'[A-Za-z]:\\.*', home):
        return None
    # ponytail: default automount root /mnt; a custom [automount] root in wsl.conf needs `wslpath`
    return f'//wsl.localhost/{distro}', Path(f'/mnt/{home[0].lower()}{home[2:].replace(chr(92), "/")}/.claude.json')


def trust_keys(root):
    """The Claude config the `claude` of this machine reads, and its folder keys for `root` and each folder above."""
    root = Path(os.path.realpath(root))
    folders = [str(folder) for folder in (root, *root.parents)]
    win = windows_claude()
    return (win[1], [win[0] + folder.rstrip('/') for folder in folders]) if win else (core.CLAUDE_CONFIG, folders)


def trusted(root):
    """Has Claude Code's folder trust been accepted for `root` or a folder above it?"""
    config, keys = trust_keys(root)
    try:
        projects = json.loads(config.read_text()).get('projects', {})
    except (OSError, json.JSONDecodeError):
        return False
    return any(projects.get(key, {}).get('hasTrustDialogAccepted') for key in keys)


def setup(args):
    """`doctor --fix`, the manager's «do it for me»: each step a command can do is done (idempotent: a rerun says
    `ok`); each step only the person can do (CLI install and login, OAuth scope, folder trust, worker permissions,
    app sign-in) is printed as one command and not attempted. Never starts a worker or a timer, never reads or
    writes credentials or host security settings. Ends with the read-only `doctor`: exit 0 only when ready."""
    pending = []

    def person(command, why):
        pending.append(command)
        print(f'you: {command}\n    {why}')

    def stop():
        print(f'stopped: the step above is the person\'s; then `taskq doctor --fix` again. No workers or timer started.')
        sys.exit(1)
    origin = origin_of()
    if core.PROJECT_PATH is None:
        if not origin:
            person('git remote add origin <repository URL>', 'the queue lives in the tracker of this checkout\'s origin')
            stop()
        if not Path('taskq.toml').exists():
            write_config('gitlab' not in origin[0], origin[1], None if origin[0] == 'github.com' else origin[0])
        core.configure()  # a broken taskq.toml stops here with its error, unchanged
    else:
        print('ok: taskq.toml')
    github = not core.BOARDS
    host = core.HOST or (origin[0] if origin else None)
    if origin and (origin[1].lower() != core.PROJECT_PATH.lower() or (core.HOST and origin[0] != core.HOST)):
        core.fail(f'origin is {origin[0]}/{origin[1]}, taskq.toml names {core.HOST or ""}{"/" * bool(core.HOST)}{core.PROJECT_PATH}: '
             'say which project is meant; nothing changed')
    cli = 'gh' if github else 'glab'
    status = probe([cli, 'auth', 'status', *(['--hostname', host] if host else [])])
    if status is None:
        person(f'brew install {cli}', f'`{cli}` is not installed (or the package manager of this machine)')
        stop()
    if status and (found := login_gap(cli, host)):
        person(found[1].split('  (')[0], found[0])
        stop()
    print(f'ok: {cli} logged in')
    if write_access(github):
        person(f'ask an owner of {core.PROJECT_PATH} for write access', 'this account cannot write to the repository')
        stop()
    gaps = queue_labels_missing() + board_gaps(github, host)
    scope = [fix.split('  (')[0] for what, fix in gaps if 'scope' in what]
    if gaps and len(gaps) > len(scope):
        migrate(args)
        print('done: labels' + ' and board' * (not scope))
    else:
        print('ok: labels' + ' and board' * (not scope))
    if lease_gaps():
        core.api('DELETE', 'leases')
        print('done: removed the leftover coordinator lease')
    for fix in scope:
        person(fix, 'a GitHub board needs the token scope `project` (browser consent); until then the queue works with labels only')
    if 'claude' not in idle():
        for what, fix in permissions_gap(core.ROOT):
            person(fix.split('  (')[0], what)
        if not permissions_missing(core.ROOT):
            print('ok: worker permissions')
        for what, fix in trust_gap(core.ROOT):
            person(fix.split('  (')[0], what + '; accept «Trust this folder» once, then quit')
        if trusted(core.ROOT):
            print('ok: Claude folder trust')
    if args.codex and 'codex' not in idle():
        if core.CODEX_SOCKET.exists():
            print(f'ok: Codex app project {core.codex_project(core.Codex(timeout=60))}')
        else:
            person(core.CODEX_HEADLESS, codex_gap() + '; the Codex app, when installed, starts the same server')
    if core.LOCAL.is_file():
        print(f'ok: personal profile {core.LOCAL}')
    else:
        person(f'{core.TOOL} profile init', PROFILE_CARD)
    for name, item in core.EXECUTORS.items():
        if item.get('setup') and name not in idle():
            person(f'cd {core.ROOT} && {item["setup"]}', f'runtime {name}: its app steps (sign-in, bot, trigger) are the person\'s')
    print('No workers or timer started.' + (f' Pending for the person: {len(pending)} step(s) above.' if pending else ''))
    doctor(argparse.Namespace(codex=args.codex), pending)


def migrate(args):
    """`init`: a new project, and once per schema change; idempotent. Labels for every state, runtime, type and
    priority; the board with one list
    per state in STATES order; state labels taskq no longer has leave the board, and leave GitLab once no
    issue carries them; every open task gets its `relates_to` links. Claims, results and history stay. The personal
    taskq.local.toml and the task trees `.worktrees/` get their .gitignore lines."""
    ignore_local()
    have = {label['name']: label for label in core.pages('labels')}
    for name in queue_labels():
        if name not in have:
            have[name] = core.api('POST', 'labels', {'name': name, 'color': '#6699cc'})
    board = cards = None
    if not core.BOARDS:  # GitHub: the Projects v2 board; every open task gets a card in the column of its label
        board = core.api('POST', 'board')
        cards = board and core.api('GET', 'board/items')
    else:
        board = next((board for board in core.api('GET', 'boards') if board['name'] == core.BOARD), None) or core.api('POST', 'boards', {'name': core.BOARD})
        lists = {item['label']['name']: item for item in board['lists']}
        for name, item in lists.items():
            if name.startswith(core.PREFIX) and name[len(core.PREFIX):] not in core.STATES:
                core.api('DELETE', f'boards/{board["id"]}/lists/{item["id"]}')
        for state in core.STATES:
            if core.PREFIX + state not in lists:
                core.api('POST', f'boards/{board["id"]}/lists', {'label_id': have[core.PREFIX + state]['id']})
        # GitLab shifts positions on every create and refuses a move to the current place: compare fresh positions.
        for position, state in enumerate(core.STATES):
            item = next(item for item in core.api('GET', f'boards/{board["id"]}/lists') if item['label']['name'] == core.PREFIX + state)
            if item['position'] != position:
                core.api('PUT', f'boards/{board["id"]}/lists/{item["id"]}', {'position': position})
    for name in have:
        if name.startswith(core.PREFIX) and name[len(core.PREFIX):] not in core.STATES:
            carriers = [issue['iid'] for issue in core.issues(f'state=all&labels={name}')]
            if carriers:
                print(f'label {name} kept: still on {sorted(carriers)}')
            else:
                core.api('DELETE', f'labels/{name}')
    everything = core.load()[0]
    for item in everything:
        core.link(item['iid'], item['deps'])
        if board and not core.BOARDS and cards.get(item['iid']) is None:
            core.api('PUT', f'board/items/{item["iid"]}', {'status': item['state']})
    if not board:
        print(f'no board on GitHub: the token lacks the scope `project`; run `gh auth refresh -h {core.HOST or "github.com"} -s project`, then `taskq init` again')
    print((f'board {board["url"] if not core.BOARDS else board["id"]}' if board else 'labels only') +
          f': {", ".join(core.PREFIX + state for state in core.STATES)}; links checked on {len(everything)} tasks')
