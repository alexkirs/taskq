#!/usr/bin/env python3
"""taskq: a task queue on an issue board. One file, stdlib only, python3 >= 3.9. Design: docs/single-file.md."""
import argparse, contextlib, importlib.util, json, os, re, shutil, signal, socket, subprocess, sys, time
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import quote

# --- config + task model --------------------------------------------------------------------

STATES = ('ready', 'waiting', 'doing', 'review', 'ask', 'later')
TYPES = ('code', 'docs', 'research', 'asset')
FIELDS = ('scope', 'deps', 'claim', 'result')
PREFIX, RUN, ON = 'q-', 'run-', 'host-'
BLOCK = re.compile(r'<!-- taskq:start -->\s*```json\n(.*?)\n```\s*<!-- taskq:end -->', re.S)
SESSIONS = {'claude': 'CLAUDE_CODE_SESSION_ID', 'codex': 'CODEX_THREAD_ID'}
CONFIG, BOARD = {}, None  # set by main, or by a test
CHECK_POLLS, CHECK_PAUSE = 60, 10  # pr-mode close waits up to 10 min for the 'tests' check

def fail(message):
    sys.exit(f'taskq: {message}')

def load_config(start=None):
    """taskq.json: the nearest one from `start` (the current directory) up; its folder is the project root."""
    here = Path(start or Path.cwd()).resolve()
    for folder in [here, *here.parents]:
        if (folder / 'taskq.json').is_file():
            if folder.parent.name == '.worktrees' and (folder.parent.parent / 'taskq.json').is_file():
                continue  # a worker's .worktrees/taskq-<N>: the root is the main checkout above it (#333)
            config = {'board': 'github', 'publish': 'direct', **json.loads((folder / 'taskq.json').read_text('utf-8')), 'root': folder}
            return config if config['publish'] in ('direct', 'pr') else fail(f'publish {config["publish"]!r}: use "direct" or "pr"')
    fail('no taskq.json in this directory or above')

def machine():
    name = os.environ.get('TASKQ_HOST') or socket.gethostname()
    return CONFIG.get('hosts', {}).get(name) or name.split('.')[0].lower()

ORCH = {'claude': 'CLD', 'codex': 'CDX', 'dot': 'DOT', 'hermes': 'HRM', 'grok': 'GRK'}

def worker_name(item):
    """R3 naming (#268): `T<N> <ORCH> <title> (<machine>)`; ORCH is who launched it, UNK for the owner's shell."""
    launcher = (session() or {}).get('runtime') or os.environ.get('TASKQ_RUNTIME')
    return f'T{item["iid"]} {ORCH.get(launcher, "UNK")} {item["title"][:40]} ({machine()})'

def session():
    """This agent session, or None for the owner's shell. TASKQ_RUNTIME picks one when a session inherited another's id."""
    found = [r for r in SESSIONS if os.environ.get(SESSIONS[r]) and os.environ.get('TASKQ_RUNTIME', r) == r]
    return {'runtime': found[0], 'session': os.environ[SESSIONS[found[0]]]} if found else None

def who():
    current = session()
    return f'{current["runtime"]}:{current["session"][:8]}' if current else 'owner'

def parse(issue):
    """A board issue as a task, or None when it is not one: no block or not exactly one known q-* label."""
    found = BLOCK.search(issue.get('body') or '')
    labels = issue['labels']
    states = [label[len(PREFIX):] for label in labels if label.startswith(PREFIX)]
    if not found or len(states) != 1 or states[0] not in STATES:
        return None
    raw = json.loads(found.group(1))
    return {**{key: raw.get(key) for key in FIELDS}, 'raw': raw, 'iid': issue['iid'], 'title': issue['title'],
            'state': states[0], 'labels': labels, 'type': next((label for label in labels if label in TYPES), None),
            'runtime': next((label[len(RUN):] for label in labels if label.startswith(RUN)), 'any'),
            'host': next((label[len(ON):] for label in labels if label.startswith(ON)), None),
            'priority': min([int(label[9:]) for label in labels if re.fullmatch(r'priority-\d', label)] or [9]),
            'updated_at': issue.get('updated_at'), 'url': issue.get('url'), 'text': BLOCK.sub('', issue['body']).strip()}

def block(text, fields):
    """The description: the task's text, then its JSON block. Keys the model does not know are kept as they are."""
    return f'{text}\n\n<!-- taskq:start -->\n```json\n{json.dumps(fields, indent=1, ensure_ascii=False)}\n```\n<!-- taskq:end -->'


# --- board ----------------------------------------------------------------------------------
# Six functions: list(state), get(n), add(title, body, labels), update(n, labels=None, body=None), comment(n, text),
# close(n). An issue is {iid, title, body, labels, state: open|closed, updated_at, url} and from get also comments.

def run_api(tool, host, method, path, body=None):
    command = [shutil.which(tool) or fail(f'{tool} not found'), 'api', '-X', method, path]
    command += ['--hostname', host] * bool(host) + ['--input', '-', '-H', 'Content-Type: application/json'] * (body is not None)
    done = subprocess.run(command, input=body and json.dumps(body), capture_output=True, text=True, encoding='utf-8')
    if done.returncode:
        fail(f'{tool} api {method} {path}: {done.stderr.strip() or done.stdout.strip()}')
    return json.loads(done.stdout) if done.stdout.strip() else None

class GitHub:
    def __init__(self, repo, host=None):
        self.repo, self.host = repo, host

    def api(self, method, path, body=None):
        return run_api('gh', self.host, method, f'repos/{self.repo}/{path}', body)

    def trusted(self, item):
        """Anyone may open or comment on a public issue: only collaborators' issues are tasks, their comments answers."""
        return item.get('author_association') in ('OWNER', 'MEMBER', 'COLLABORATOR')

    def pages(self, path):
        found, page = [], 1
        while True:
            batch = self.api('GET', f'{path}{"&" if "?" in path else "?"}per_page=100&page={page}')
            found += batch
            if len(batch) < 100:
                return [item for item in found if self.trusted(item)]
            page += 1

    def issue(self, item):
        # An untrusted author's description is read as empty: never a task.
        return {'iid': item['number'], 'title': item['title'], 'body': self.trusted(item) and item.get('body') or '', 'url': item['html_url'],
                'labels': [label['name'] for label in item['labels']], 'state': item['state'], 'updated_at': item['updated_at']}

    def list(self, state):
        query = 'issues?state=open' + (f'&labels={PREFIX}{state}' if state else '')
        return [self.issue(item) for item in self.pages(query) if 'pull_request' not in item
                and any(label['name'].startswith(PREFIX) for label in item['labels'])]

    def get(self, n):
        return {**self.issue(self.api('GET', f'issues/{n}')), 'comments': [item['body'] for item in self.pages(f'issues/{n}/comments')]}

    def add(self, title, body, labels):
        return self.api('POST', 'issues', {'title': title, 'body': body, 'labels': labels})['number']

    def update(self, n, labels=None, body=None):
        self.api('PATCH', f'issues/{n}', {key: value for key, value in (('labels', labels), ('body', body)) if value is not None})

    def comment(self, n, text):
        self.api('POST', f'issues/{n}/comments', {'body': text})

    def close(self, n):
        self.api('PATCH', f'issues/{n}', {'state': 'closed'})

class GitLab(GitHub):
    members = None  # read once per process

    def api(self, method, path, body=None):
        return run_api('glab', self.host, method, f'projects/{quote(self.repo, safe="")}/{path}', body)

    def trusted(self, item):
        """Members with Reporter or higher: GitLab has no author_association. A member item itself has no author."""
        if self.members is None and 'author' in item:
            self.members = {member['id'] for member in self.pages('members/all') if member['access_level'] >= 20}
        return 'author' not in item or (item['author'] or {}).get('id') in self.members

    def issue(self, item):
        return {'iid': item['iid'], 'title': item['title'], 'body': self.trusted(item) and item.get('description') or '', 'url': item['web_url'],
                'labels': item['labels'], 'state': 'open' if item['state'] == 'opened' else 'closed',
                'updated_at': item['updated_at']}

    def list(self, state):
        query = 'issues?state=opened' + (f'&labels={PREFIX}{state}' if state else '')
        return [self.issue(item) for item in self.pages(query) if any(label.startswith(PREFIX) for label in item['labels'])]

    def get(self, n):
        return {**self.issue(self.api('GET', f'issues/{n}')), 'comments': [
            item['body'] for item in self.pages(f'issues/{n}/notes?sort=asc&activity_filter=only_comments')]}

    def add(self, title, body, labels):
        return self.api('POST', 'issues', {'title': title, 'description': body, 'labels': ','.join(labels)})['iid']

    def update(self, n, labels=None, body=None):
        self.api('PUT', f'issues/{n}', {key: value for key, value in (('labels', None if labels is None else ','.join(labels)),
                                                                     ('description', body)) if value is not None})

    def comment(self, n, text):
        self.api('POST', f'issues/{n}/notes', {'body': text})

    def close(self, n):
        self.api('PUT', f'issues/{n}', {'state_event': 'close'})

def load_file(path, root=None):
    """A board or runtime file (relative to the project root): its module-level functions are the protocol."""
    spec = importlib.util.spec_from_file_location(f'taskq_{Path(path).stem}', (root or CONFIG['root']) / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

def make_board(config):
    """`board`: github, gitlab, or a .py file with the six functions at module level."""
    kind = config['board']
    if kind.endswith('.py'):
        return load_file(kind, config['root'])
    return {'github': GitHub, 'gitlab': GitLab}.get(kind, lambda *_: fail(f'unknown board {kind!r}'))(
        config['repo'], config.get('host'))


# --- runtime --------------------------------------------------------------------------------
# Four functions: spawn(name, prompt, cwd) -> session, send(session, text) -> session (a Claude resume may continue
# under a new id), alive(session) -> True/False/None (running / gone / cannot tell), link(session) -> url or None.
# Optional fifth: retire(gone, running=True) stops and removes this machine's `T<N>` sessions whose task gone(N) names:
# close calls it for its task (#302, #360), the tick for closed tasks, stopped sessions only.

def worker_env():  # a worker must not inherit the tick's session id, nor a spawning worker's task (#333)
    return {key: value for key, value in os.environ.items() if key not in (*SESSIONS.values(), 'TASKQ_TASK', 'TASKQ_RUNTIME')}

class Claude:
    # #38, #51, #71: a worker gets only these tools, no MCP, no Chrome, and a pinned mode (else `auto` stops `taskq`).
    # `--tools` takes several values: a flag must follow it, never the prompt.
    TOOLS = ['Bash', 'Read', 'Edit', 'Write', 'Glob', 'Grep', 'WebFetch', 'WebSearch']

    def agents(self):
        """This machine's `claude --bg` sessions by session id, stopped ones too; None when the list cannot be read."""
        try:
            done = subprocess.run([shutil.which('claude') or 'claude', 'agents', '--json', '--all'], capture_output=True, text=True, encoding='utf-8', timeout=60)
            listed = json.loads(done.stdout) if not done.returncode else None
        except (OSError, subprocess.SubprocessError, ValueError):
            return None
        return {item['sessionId']: item for item in listed if isinstance(item, dict) and item.get('kind') == 'background'
                and item.get('sessionId')} if isinstance(listed, list) else None

    def start(self, arguments, cwd):
        """`claude --bg ...`; it prints `backgrounded · <short id>`, `claude agents` gives the full one."""
        done = subprocess.run([shutil.which('claude') or fail('claude not found'), '--bg', *arguments], cwd=cwd, env=worker_env(),
                              capture_output=True, text=True, encoding='utf-8', timeout=120)
        short = re.search(r'backgrounded · (\w+)', re.sub(r'\x1b\[[0-9;]*m', '', done.stdout))  # FORCE_COLOR colours it
        if done.returncode or not short:
            fail(f'claude could not start the session: {done.stderr.strip() or done.stdout.strip()}')
        return next((sid for sid in self.agents() or {} if sid.startswith(short[1])), None) or fail(f'claude agents lacks {short[1]}')

    def flags(self, name):
        """What spawn and send both pass (#301): without --name a resume retitles the job from the prompt, without the rest it runs in the user's mode."""
        mode = CONFIG.get('permission_mode', 'dontAsk')
        settings = {'permissions': {'defaultMode': mode, 'allow': self.TOOLS}}  # dontAsk denies what is not allowed
        return [*(['--name', name] if name else []), '--permission-mode', mode, '--tools', ','.join(self.TOOLS), '--strict-mcp-config',
                '--no-chrome', '--settings', json.dumps(settings)]

    def spawn(self, name, prompt, cwd):
        return self.start([*self.flags(name), prompt], cwd)

    def stop(self, session):
        """`claude stop <job id>` when the session still has a process; its agent, or {} when not listed."""
        agent = (self.agents() or {}).get(session) or {}
        if agent.get('pid'):
            subprocess.run([shutil.which('claude') or 'claude', 'stop', agent['id']], capture_output=True, timeout=60)
        return agent

    def send(self, session, text):
        """Stop the session, then resume it with the text. #284: a stopped session resumes under a new id."""
        agent = self.stop(session)
        return self.start(['--resume', session, *self.flags(agent.get('name')), text], agent.get('cwd') or CONFIG['root'])

    @staticmethod
    def running(agent):
        return bool(agent.get('pid')) and agent.get('state') not in ('done', 'failed', 'stopped')

    def alive(self, session):
        agent = (self.agents() or {}).get(session)
        return None if agent is None else self.running(agent)

    def retire(self, gone, running=True):
        """`claude stop` + `claude rm`: duplicate spawns and resumes leave several jobs per task, and a stopped job stays listed (#360)."""
        claude = shutil.which('claude') or 'claude'
        for agent in (self.agents() or {}).values():
            n = re.match(r'T(\d+) [A-Z]{3} ', agent.get('name') or '')  # worker_name: never the owner's own jobs
            if not n or not gone(int(n[1])) or not agent.get('id') or self.running(agent) and not running:
                continue
            if agent.get('pid'):
                subprocess.run([claude, 'stop', agent['id']], capture_output=True, timeout=60)
            subprocess.run([claude, 'rm', agent['id']], capture_output=True, timeout=60)

    def tail(self, session):
        """The last line of `claude logs`, for an ask (#393)."""
        job = ((self.agents() or {}).get(session) or {}).get('id') or session
        done = subprocess.run([shutil.which('claude') or 'claude', 'logs', job], capture_output=True, text=True, encoding='utf-8', timeout=60)
        return last_line(re.sub(r'\x1b\[[0-9;]*m', '', done.stdout + done.stderr))

    def link(self, session):
        """The Remote Control URL: ~/.claude/jobs/<short>/state.json holds `bridgeSessionId` cse_<id> (#83)."""
        try:
            job = json.loads((Path.home() / '.claude' / 'jobs' / session[:8] / 'state.json').read_text('utf-8'))
        except (OSError, ValueError):
            return None
        bridge = job.get('bridgeSessionId') if job.get('sessionId') == session else None
        return bridge and 'https://claude.ai/code/session_' + re.sub('^(cse_|session_)', '', bridge)

def last_line(text):
    return next((line.strip() for line in reversed(text.splitlines()) if line.strip()), '')

def pid_alive(pid):
    if os.name != 'nt':
        try:
            os.kill(pid, 0)
        except OSError as error:
            return isinstance(error, PermissionError)  # someone else's process
        return True
    import ctypes  # Windows: os.kill(pid, 0) would terminate the process
    handle, code = ctypes.windll.kernel32.OpenProcess(0x1000, False, pid), ctypes.c_ulong()  # QUERY_LIMITED_INFORMATION
    if not handle:
        return False
    ctypes.windll.kernel32.GetExitCodeProcess(handle, ctypes.byref(code))
    ctypes.windll.kernel32.CloseHandle(handle)
    return code.value == 259  # STILL_ACTIVE

class Codex:
    """`codex exec`, headless: one process per turn, its JSONL in .taskq/<name>.log, `<pid> <thread>` in .taskq/<name>.pid."""

    def folder(self):
        (CONFIG['root'] / '.taskq').mkdir(exist_ok=True)
        return CONFIG['root'] / '.taskq'

    def exec(self, name, arguments, cwd):
        # Network on: a worker pushes and calls the board. `"codex": [...]` in taskq.json replaces these options.
        options = CONFIG.get('codex', ['-s', 'workspace-write', '-c', 'sandbox_workspace_write.network_access=true'])
        log, detach = self.folder() / f'{name.split()[0]}.log', {'creationflags': 0x208} if os.name == 'nt' else {'start_new_session': True}
        with open(log, 'ab') as out:  # detached: the worker outlives the tick
            process = subprocess.Popen([shutil.which('codex') or fail('codex not found'), 'exec', '--json', *options, *arguments],
                                       cwd=cwd, env=worker_env(), stdin=subprocess.DEVNULL, stdout=out, stderr=subprocess.STDOUT, **detach)
        return process, log

    def spawn(self, name, prompt, cwd):
        process, log = self.exec(name, ['-C', str(cwd), prompt], cwd)
        for _ in range(600):  # the first JSONL line, thread.started, carries the thread id
            found = re.search(r'"thread_id":\s*"([\w-]+)"', log.read_text('utf-8', 'replace'))
            if found or process.poll() is not None:
                break
            time.sleep(0.1)
        found or fail(f'codex exec gave no thread id: see {log}')
        (log.with_suffix('.pid')).write_text(f'{process.pid} {found[1]}')
        return found[1]

    def pid_file(self, session):
        return next((path for path in self.folder().glob('*.pid') if path.read_text().split()[1:] == [session]), None)

    def send(self, session, text):
        process, log = self.exec(getattr(self.pid_file(session), 'stem', session), ['resume', session, text], CONFIG['root'])
        log.with_suffix('.pid').write_text(f'{process.pid} {session}')
        return session

    def alive(self, session):
        path = self.pid_file(session)
        return None if path is None else pid_alive(int(path.read_text().split()[0]))

    def retire(self, gone, running=True):
        """Kill the turn's process, `codex archive` the thread, drop .taskq/T<N>.pid (#360)."""
        for path in (CONFIG['root'] / '.taskq').glob('T*.pid'):
            n, (pid, thread) = re.fullmatch(r'T(\d+)', path.stem), path.read_text().split()
            if not n or not gone(int(n[1])) or pid_alive(int(pid)) and not running:
                continue
            if pid_alive(int(pid)):
                os.kill(int(pid), signal.SIGTERM)
            subprocess.run([shutil.which('codex') or 'codex', 'archive', thread], capture_output=True, timeout=60)
            path.unlink()

    def tail(self, session):
        path = self.pid_file(session)
        return last_line(path.with_suffix('.log').read_text('utf-8', 'replace')) if path else ''

    def link(self, session):
        return f'{CONFIG.get("pages", "https://alexkirs.github.io/taskq/").rstrip("/")}/open.html#codex://threads/{session}'

def runtimes():
    """claude, codex, and each `"runtimes": {"name": "runtimes/name.py"}` file of taskq.json."""
    return {'claude': Claude(), 'codex': Codex(), **{name: load_file(path) for name, path in CONFIG.get('runtimes', {}).items()}}


# --- commands -------------------------------------------------------------------------------

def task(n, *states):
    issue = BOARD.get(n)
    found = parse(issue) if issue['state'] == 'open' else None
    if not found:
        fail(f'#{n} is not an open taskq task')
    if states and found['state'] not in states:
        fail(f'#{n} is {found["state"]}, not {" or ".join(states)}')
    return found

def move(current, state, action, text='', **fields):
    """One update moves the label and the block together; one comment is the history. State None: no state label."""
    labels = [label for label in current['labels'] if not label.startswith(PREFIX)] + ([PREFIX + state] if state else [])
    raw = {**current['raw'], **{key: current[key] for key in FIELDS}, **fields}
    BOARD.update(current['iid'], labels=labels, body=block(current['text'], raw))
    BOARD.comment(current['iid'], f'**{action}** · {who()}' + (f'\n\n{text}' if text else ''))
    print(f'#{current["iid"]} {state or "closed"}')

def open_deps(deps):
    return [n for n in deps or [] if BOARD.get(n)['state'] == 'open']

def cmd_add(args):
    text = f'## Goal\n\n{args.goal}\n\n## Acceptance\n\n{args.acceptance}'
    state = 'waiting' if open_deps(args.deps) else 'ready'
    labels = [PREFIX + state, f'priority-{args.priority}', args.type] + ([RUN + args.runtime] if args.runtime != 'any' else []) \
        + ([ON + args.host] if args.host else [])
    n = BOARD.add(args.title, block(text, {'scope': args.scope, 'deps': args.deps, 'claim': None, 'result': None}), labels)
    BOARD.comment(n, f'**add** · {who()}')
    print(f'#{n} {state}')

def cmd_list(args):
    found = [item for item in map(parse, BOARD.list(args.state)) if item]
    for item in sorted(found, key=lambda item: (STATES.index(item['state']), item['priority'], item['iid'])):
        claim = item['claim'] or {}
        detail = {'ready': 'continue' if claim else '', 'later': item['raw'].get('waiting_for') or '',
                  'doing': f'{claim.get("runtime")}:{(claim.get("session") or "")[:8]} @{claim.get("name")}' if claim else '',
                  'waiting': f'open dependencies {open_deps(item["deps"])}' if item['state'] == 'waiting' else ''}.get(item['state'], '')
        print(f'#{item["iid"]:<4} {item["state"]:<8} p{item["priority"]} {item["runtime"]:<6} {item["title"]}'
              + (f'  [{detail}]' if detail else ''))

def cmd_take(args):
    current, mine = task(args.n, 'ready'), session() or fail('take needs an agent session: set ' + ' or '.join(SESSIONS.values()))
    if open_deps(current['deps']):
        fail(f'#{args.n} has open dependencies')
    move(current, 'doing', 'take', claim={**mine, 'name': machine()}, result=None)

# Moves with no other check: command -> (states it takes from, state it goes to, block changes).
MOVES = {'ask': (('doing',), 'ask', lambda args: {}), 'answer': (('ask',), 'doing', lambda args: {}),
         'requeue': (STATES, 'ready', lambda args: {'claim': None, 'result': None}),
         'later': (STATES, 'later', lambda args: {'waiting_for': args.text or None}),
         'result': (('doing',), 'review', lambda args: {'result': {'sha': args.sha, 'checks': args.checks}})}

def cmd_move(args):
    sources, state, fields = MOVES[args.command]
    move(task(args.n, *sources), state, args.command, args.text, **fields(args))

def commit(sha):
    """A result's commit: hex only, so it never reaches git as an option."""
    return sha if re.fullmatch('[0-9a-f]{7,40}', sha) else fail(f'{sha!r} is not a commit: 7 to 40 lowercase hex digits')

def merge(current, sha):
    """pr mode: squash-merge the one open PR/MR of branch taskq-<N> into main at the result SHA: the merge commit, None with no PR.
    GitHub (#359): it merges only once the 'tests' check passes on the PR head; a PR behind main is not updated.
    A PR that does not merge (conflict, failing checks) goes back to the worker: requeue with the platform's message."""
    lab, host, branch = CONFIG['board'] == 'gitlab', CONFIG.get('host'), f'taskq-{current["iid"]}'
    where = ['-R', (f'https://{host}/' if lab else f'{host}/') * bool(host) + CONFIG['repo']]  # gh takes HOST/OWNER/REPO, glab a URL

    def cli(*command):
        done = subprocess.run([shutil.which(command[0]) or fail(f'{command[0]} not found'), *command[1:], *where], capture_output=True, text=True, encoding='utf-8')
        return done.returncode, (done.stderr.strip() or done.stdout.strip()) if done.returncode else done.stdout
    code, out = cli(*(['glab', 'mr', 'list', '--source-branch', branch, '--output', 'json'] if lab else ['gh', 'pr', 'list', '--head', branch, '--json', 'number,headRefOid,baseRefName']))
    found = code and fail(out) or [(str(pr.get('iid', pr.get('number'))), pr.get('sha', pr.get('headRefOid')), pr.get('target_branch', pr.get('baseRefName')))
                                   for pr in json.loads(out)]
    if found != [(found and found[0][0], sha, 'main')]:  # exactly one, into main, at the full result SHA
        return found and fail(f'{branch}: open PRs (number, head, base) {found} do not match the result {sha} into main')
    number, head, _ = found[0]

    def back(why):
        move(current, 'ready', 'requeue', f'close: PR {number} {why}', claim=None, result=None)
        fail(f'#{current["iid"]}: PR {number} {why}')
    if not lab:  # #359: main requires 'tests' on the PR head only (not strict); a behind PR merges as is, GitHub refuses a conflict
        api = lambda path: run_api('gh', host, 'GET', f'repos/{CONFIG["repo"]}/{path}')
        for _ in range(CHECK_POLLS):  # ponytail: fixed poll; tests.yml takes 8-13 s
            runs = api(f'commits/{head}/check-runs?check_name=tests')['check_runs']
            if runs and all(run['status'] == 'completed' for run in runs):
                if any(run['conclusion'] != 'success' for run in runs):
                    back(f'check tests failed on {head}')
                break
            time.sleep(CHECK_PAUSE)
        else:
            back(f'check tests did not finish on {head}')
    _, out = cli(*(['glab', 'mr', 'merge', number, '--squash', '--remove-source-branch', '--sha', head, '--auto-merge=false', '--yes'] if lab
                   else ['gh', 'pr', 'merge', number, '--squash', '--delete-branch', '--match-head-commit', head]))
    code, viewed = cli(*(['glab', 'mr', 'view', number, '--output', 'json'] if lab else ['gh', 'pr', 'view', number, '--json', 'state,mergeCommit']))
    pr = {} if code else json.loads(viewed)
    if str(pr.get('state')).lower() != 'merged':  # read back: a merge that reported an error may still have merged
        back(f'did not merge: {out}')
    return pr.get('merge_commit_sha') or pr.get('squash_commit_sha') or pr['mergeCommit']['oid']

def cleanup(current):
    """The worker's .worktrees/taskq-<N> and branch taskq-<N>, on the claim's machine: removed when clean, else kept
    with a note for the close comment. Never --force: that lost a worker's changes (#284)."""
    branch = f'taskq-{current["iid"]}'
    tree = CONFIG['root'] / '.worktrees' / branch
    if (current['claim'] or {}).get('name') != machine() or not tree.is_dir():
        return ''
    git = [shutil.which('git') or fail('git not found'), '-C', str(CONFIG['root'])]
    status = subprocess.run([*git, '-C', str(tree), 'status', '--porcelain'], capture_output=True, text=True, encoding='utf-8')
    removed = not status.returncode and not status.stdout.strip() and \
        not subprocess.run([*git, 'worktree', 'remove', str(tree)], capture_output=True).returncode
    if not removed:
        return f'kept .worktrees/{branch} and branch {branch}: uncommitted changes'
    subprocess.run([*git, 'branch', '-D', branch], capture_output=True)
    return ''

def cmd_close(args):
    """close N [M ...]: in order; no PR is updated (#359). A failed task does not stop the rest."""
    failed = []
    for n in args.n:
        try:
            close_one(argparse.Namespace(n=n, text=args.text))
        except SystemExit as error:
            if len(args.n) == 1:
                raise
            failed.append(n)
            print(error, file=sys.stderr)
    failed and fail(f'not closed: {" ".join(f"#{n}" for n in failed)}')

def close_one(args):
    current = task(args.n, 'review')
    sha = commit((current['result'] or {}).get('sha') or '')
    if CONFIG['publish'] == 'pr' and (merged := merge(current, sha)):
        args.text = f'merged {merged}' + (f'\n\n{args.text}' if args.text else '')
    else:  # direct mode, or a pr-mode task with no PR (an answer): the result must be on main
        git = [shutil.which('git') or fail('git not found'), '-C', str(CONFIG['root'])]
        subprocess.run([*git, 'fetch', 'origin'], capture_output=True)
        if subprocess.run([*git, 'merge-base', '--is-ancestor', sha, 'origin/main'], capture_output=True).returncode:
            fail(f'#{args.n}: result {sha} is not on origin/main')
    claim = current['claim'] or {}
    if hasattr(runtimes().get(claim.get('runtime')), 'retire') and claim.get('name') not in (None, machine()):
        args.text = (f'{args.text}\n\n' if args.text else '') + f'session {claim.get("session")} runs on {claim.get("name")}: stop it there'
    kept = cleanup(current)
    move(current, None, 'close', '\n\n'.join(filter(None, (args.text, kept))))
    BOARD.close(args.n)
    retire(lambda n: n == args.n, f'#{args.n}: could not stop its sessions')

def retire(gone, why, running=True):
    """Each runtime's retire, best effort: a session left behind never fails close or the tick."""
    for runtime in runtimes().values():
        try:
            getattr(runtime, 'retire', lambda *_: None)(gone, running)
        except Exception as error:
            print(f'{why}: {error}', file=sys.stderr)

def history(n):
    """The review notes a new worker must read: every requeue and answer since the last close, with the result they answer."""
    notes = [text for text in BOARD.get(n)['comments'] or [] if text.startswith(('**result**', '**requeue**', '**answer**', '**ask**'))]
    return 'History of this task (read it first; a requeue says what to fix):\n\n' + '\n\n'.join(notes[-6:]) + '\n\n' if notes else ''

def brief(item, runtime):
    """The worker's prompt: the task, its workspace, the taskq commands it uses."""
    n, root, tq = item['iid'], CONFIG['root'], f'python3 {Path(__file__).resolve()}'
    create = f'glab mr create --yes --target-branch main --source-branch taskq-{n} --title "<title>" --description' if CONFIG['board'] == 'gitlab' \
        else f'gh pr create --base main --head taskq-{n} --title "<title>" --body'
    push = f'`git push --force-with-lease origin HEAD:refs/heads/taskq-{n}`, open a pull request once (a push updates it):\n  `{create} "<summary>"`, ' \
        f'then `{tq} result {n} --sha <PR head full SHA>' if CONFIG['publish'] == 'pr' else f'`git push origin HEAD:main`, then\n  `{tq} result {n} --sha <pushed full SHA>'
    return f'''You are the taskq worker for task #{n}: {item["title"]}. The task is claimed for you: do it without asking for confirmation.
Queue tool: `{tq}`. Start every shell command with `export TASKQ_TASK={n} TASKQ_RUNTIME={runtime} &&`.

{item["text"]}

{history(n)}Expected paths: {", ".join(item["scope"] or []) or "none named"}. They say where the work is expected, not what is forbidden.

Workspace: from {root} run `git fetch origin && git worktree add -b taskq-{n} .worktrees/taskq-{n} origin/main`, work only there,
never in the main checkout; a branch taskq-{n} left by an earlier worker: continue it. A task that ends in an answer, not a commit,
needs no worktree. Commands:
- A question only the owner can decide (a product choice, an action that cannot be undone): `{tq} ask {n} --text "<question>"`, then stop.
- Cannot be done: `{tq} requeue {n} --text "<why>"`, then stop.
- Deliver: commit on branch taskq-{n}, `git fetch origin && git rebase origin/main`, run the tests, {push} --checks "<commands and outcome>" --text "<summary>"`, then stop.
  An answer with no commit: the result names the current origin/main SHA and the text holds the answer.
Everything written through taskq is public: no secrets, tokens or paths outside the repository.'''

def age(item):
    """Minutes since the issue last changed: a comment changes it too."""
    changed = datetime.fromisoformat((item['updated_at'] or '').replace('Z', '+00:00'))
    return (datetime.now(timezone.utc) - changed).total_seconds() / 60

@contextlib.contextmanager
def dispatch_lock():
    """Yield True while this process holds .taskq/dispatch.lock, False when another does. Local exclusion, not task state."""
    folder = CONFIG['root'] / '.taskq'
    folder.mkdir(exist_ok=True)
    with open(folder / 'dispatch.lock', 'a+') as handle:
        try:
            if os.name == 'nt':
                import msvcrt
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            yield False
            return
        yield True  # closing the file (or the process exiting) releases the lock

def quick_deaths(n):
    """'requeue ... is gone' notes since the last result or answer."""
    count = 0
    for text in reversed(BOARD.get(n)['comments'] or []):
        if text.startswith(('**result**', '**answer**')):
            break
        count += text.startswith('**requeue**') and ' is gone' in text
    return count

def cmd_tick(args, table=True):
    """One pass: requeue dead workers, nudge silent ones, free waiting tasks, spawn ready ones, print the table."""
    here, kinds = machine(), runtimes()
    limits = CONFIG.get('limits') or {name: 1 for name in kinds}
    with dispatch_lock() as held:  # #357 (R2): one pass at a time per checkout; the list is read under the lock
        items = sorted(filter(None, map(parse, BOARD.list(None))), key=lambda item: (item['priority'], item['iid']))
        if not held:  # another pass runs here now and starts what is ready
            print('taskq: another pass is running; this one only prints the table', file=sys.stderr)
        busy, ready = {}, items if held else []
        for item in ready:
            claim = item['claim'] or {}
            if item['state'] == 'waiting' and not open_deps(item['deps']):
                move(item, 'ready', 'ready', 'dependencies closed')
                item['state'] = 'ready'
            if item['state'] != 'doing' or claim.get('name') != here or claim.get('runtime') not in kinds:
                continue  # another machine's, or a session no runtime here can see
            runtime = kinds[claim['runtime']]
            state = runtime.alive(claim['session'])
            if state is False:
                gone = f'session {claim["session"]} is gone'
                if quick_deaths(item['iid']):  # #393: the second death in a row with no result or answer asks, not respawns
                    try:
                        tail = getattr(runtime, 'tail', lambda _: '')(claim['session'])
                    except Exception as error:  # best effort: the ask goes out without the line
                        tail = f'no log: {error}'
                    move(item, 'ask', 'ask', f'{gone} again, the worker dies at once: fix the runtime, then answer.\n\nLast log line: {tail or "none"}')
                    item['state'] = 'ask'
                    continue
                move(item, 'ready', 'requeue', gone, claim=None, result=None)
                item.update(state='ready', claim=None)
                continue
            last = (BOARD.get(item['iid'])['comments'] or [''])[-1] if state else ''
            answer = last.partition('\n\n')[2] if last.startswith('**answer**') else None  # #307: an answer wakes the worker at once
            if answer is not None or state and age(item) >= 120:
                text = f'The owner answered your question:\n\n{answer}' if answer is not None else 'continue: read your issue'
                claim = {**claim, 'session': runtime.send(claim['session'], text)}
                move(item, 'doing', 'nudge', claim=claim)  # the nudge comment is now the last note: one send per answer
                item['claim'] = claim
            busy[claim['runtime']] = busy.get(claim['runtime'], 0) + 1
        for item in ready:
            if item['state'] != 'ready' or item['host'] not in (None, here) or open_deps(item['deps']):
                continue
            names = [item['runtime']] if item['runtime'] != 'any' else list(limits)
            free = next((name for name in names if name in kinds and busy.get(name, 0) < limits.get(name, 1)), None)
            fresh = free and parse(BOARD.get(item['iid']))  # #357: the board may have moved since the list
            if fresh and fresh['state'] == 'ready':  # the tick claims it: the next tick sees the slot taken, the worker needs no `take`
                item.update(fresh)
                session = kinds[free].spawn(worker_name(item), brief(item, free), CONFIG['root'])
                item['claim'] = {'runtime': free, 'session': session, 'name': here}
                move(item, 'doing', 'spawn', kinds[free].link(session) or '', claim=item['claim'], result=None)
                item['state'], busy[free] = 'doing', busy.get(free, 0) + 1
        if held:  # #360: sessions of tasks no longer open (the list holds open tasks only)
            open_tasks = {item['iid'] for item in items}
            retire(lambda n: n not in open_tasks, 'could not remove sessions of closed tasks', running=False)
    if not table:
        return
    print(f'{"Task":<6} {"State":<8} {"Runtime":<8} Session link')
    for item in items:
        claim = item['claim'] or {}
        runtime = claim.get('runtime') or item['runtime']
        url = kinds[runtime].link(claim['session']) if claim.get('session') and claim.get('name') == here and runtime in kinds else ''
        print(f'#{item["iid"]:<5} {item["state"]:<8} {runtime:<8} {url or claim.get("session") or ""}')
    host, repo = CONFIG.get('host'), CONFIG.get('repo')
    url = CONFIG.get('board_url') or {'github': f'https://{host or "github.com"}/{repo}/issues',
                                      'gitlab': f'https://{host or "gitlab.com"}/{repo}/-/issues'}.get(CONFIG['board'])
    url and print(f'Board: {url}')  # a board file names its page in `board_url`

EVENTS = ('add', 'answer', 'result', 'requeue', 'close')  # R4 (#333): each starts one pass after its move

def dispatch():
    """R4 (#405): the event pass runs in a detached `tick --quiet` child, its output in .taskq/dispatch.log; the event returns at once."""
    if os.environ.get('CODEX_SANDBOX'):  # a sandboxed Codex worker can neither start codex nor see other sessions' pids:
        return  # its pass would requeue live tasks as gone and spawn workers that die at once (#269 run 4b)
    try:
        (CONFIG['root'] / '.taskq').mkdir(exist_ok=True)
        detach = {'creationflags': 0x208} if os.name == 'nt' else {'start_new_session': True}
        with open(CONFIG['root'] / '.taskq' / 'dispatch.log', 'ab') as out:
            start_pass([sys.executable, str(Path(__file__).resolve()), 'tick', '--quiet'], cwd=CONFIG['root'],
                       stdin=subprocess.DEVNULL, stdout=out, stderr=subprocess.STDOUT, **detach)
    except Exception as error:  # never fails the event: the next tick retries
        print(f'taskq: dispatch stopped: {error}; the next tick retries', file=sys.stderr)

start_pass = subprocess.Popen  # tests run the child's pass in process

def event_pass(args):
    """`tick --quiet`: the tick pass without the table. A failure never fails the event: the next tick retries."""
    try:
        cmd_tick(args, table=False)
    except (SystemExit, Exception) as error:
        print(f'taskq: dispatch stopped: {str(error).removeprefix("taskq: ")}; the next tick retries', file=sys.stderr)

def cmd_wait(args):
    """Block until the manager is needed: print 'review #N', 'ask #N', 'gone #N', or 'tick' after the window (#407).
    .taskq/wait.json keeps the states last reported, so an event is printed once."""
    path, kinds, here = CONFIG['root'] / '.taskq' / 'wait.json', runtimes(), machine()
    seen = json.loads(path.read_text('utf-8')) if path.is_file() else {}
    end = time.time() + args.window * 60
    while True:
        now = {}
        for item in filter(None, map(parse, BOARD.list(None))):
            claim, state = item['claim'] or {}, item['state']
            if state == 'doing' and claim.get('name') == here and claim.get('runtime') in kinds \
                    and kinds[claim['runtime']].alive(claim['session']) is False:  # ponytail: one alive call per local worker per poll
                state = 'gone'
            now[str(item['iid'])] = state
        events = [f'{state} #{n}' for n, state in now.items() if state in ('review', 'ask', 'gone') and seen.get(n) != state]
        if events or time.time() >= end:
            path.parent.mkdir(exist_ok=True)
            path.write_text(json.dumps(now), 'utf-8')
            print('\n'.join(events) or 'tick')
            return
        seen = now  # a task that leaves review and comes back while we wait is a new event
        time.sleep(args.every)

SENDERS = {'claude': 'SendMessage', 'codex': 'its thread send'}

def cmd_arm(args):
    """The prompt for a tick-sender session of this runtime: wait, send the output to the manager, repeat (#407)."""
    runtime = (session() or {}).get('runtime') or os.environ.get('TASKQ_RUNTIME')
    wait = f'python3 {Path(__file__).resolve()} wait'
    if not args.target:  # no target: this session ticks itself (Claude: a background command wakes the session on exit)
        return print(f'''Arm the tick in this session. Run `{wait}` as a background command (Claude Code: run_in_background).
When it ends you are woken with its output (`review #N`, `ask #N`, `gone #N` or `tick`): run one pass (`taskq tick`),
do § 7 After each pass for those tasks, then start `{wait}` in the background again. Keep exactly one wait running.
A runtime that cannot wake a session when a background command ends (Codex): use `taskq arm tick "<manager>"` from a separate sender session.''')
    print(f'''You are the taskq tick sender for the manager session {args.target}. Do no task work and run no other taskq command.
Repeat forever, from {CONFIG["root"]}:
1. Run `python3 {Path(__file__).resolve()} wait`. It blocks until the manager is needed (at most 10 minutes) and prints one line per event.
2. Send its output, verbatim, to {args.target} with {SENDERS.get(runtime, "your messaging tool")}.
3. Go back to 1 at once. A failed run or send: say so to {args.target} once, then go on.''')

def main(argv=None):
    global CONFIG, BOARD
    parser = argparse.ArgumentParser(prog='taskq')
    commands = parser.add_subparsers(dest='command', required=True)

    def command(name, function, *options, n=True, text=False):
        sub = commands.add_parser(name)
        if n:
            sub.add_argument('n', type=int)
        if text:
            sub.add_argument('--text', required=text == 'required', default='')
        for flags, extra in options:
            sub.add_argument(*flags, **extra)
        sub.set_defaults(function=function)

    command('add', cmd_add, (('title',), {}), (('--goal',), {'required': True}), (('--acceptance',), {'required': True}),
            (('--scope',), {'nargs': '*', 'default': []}), (('--deps',), {'nargs': '*', 'type': int, 'default': []}),
            (('--type',), {'choices': TYPES, 'default': 'code'}), (('--runtime',), {'default': 'any'}),
            (('--priority',), {'type': int, 'choices': (1, 2), 'default': 2}), (('--host',), {}), n=False)
    command('list', cmd_list, (('state',), {'nargs': '?', 'choices': STATES}), n=False)
    command('take', cmd_take)
    command('ask', cmd_move, text='required')
    command('answer', cmd_move, text='required')
    command('result', cmd_move, (('--sha',), {'required': True, 'type': commit}), (('--checks',), {'default': ''}), text=True)
    command('requeue', cmd_move, text=True)
    command('later', cmd_move, text=True)
    command('close', cmd_close, (('n',), {'nargs': '+', 'type': int}), n=False, text=True)
    command('tick', lambda args: (event_pass if args.quiet else cmd_tick)(args), (('--quiet',), {'action': 'store_true'}), n=False)
    command('wait', cmd_wait, (('--window',), {'type': float, 'default': 10}), (('--every',), {'type': float, 'default': 25}), n=False)
    command('arm', cmd_arm, (('what',), {'choices': ('tick',)}), (('target',), {'nargs': '?'}), n=False)
    args = parser.parse_args(argv)
    if BOARD is None:
        CONFIG = load_config()
        BOARD = make_board(CONFIG)
    args.function(args)
    if args.command in EVENTS:
        dispatch()

if __name__ == '__main__':
    main()
