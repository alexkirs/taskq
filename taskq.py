#!/usr/bin/env python3
"""taskq: a task queue on an issue board. One file, stdlib only, python3 >= 3.9. Design: docs/single-file.md."""
import argparse, contextlib, glob, hashlib, importlib.util, json, os, re, shlex, shutil, signal, socket, subprocess, sys, threading, time
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import quote

# --- config + task model --------------------------------------------------------------------

STATES = ('ready', 'waiting', 'doing', 'review', 'ask', 'later')
TYPES = ('code', 'docs', 'research', 'asset')
FIELDS = ('scope', 'deps', 'claim', 'result', 'supervisor', 'order', 'pm')
PREFIX, RUN, ON = 'q-', 'run-', 'host-'
BLOCK = re.compile(r'<!-- taskq:start -->\s*```json\n(.*?)\n```\s*<!-- taskq:end -->', re.S)
SESSIONS = {'claude': 'CLAUDE_CODE_SESSION_ID', 'codex': 'CODEX_THREAD_ID'}
CONFIG, BOARD = {}, None  # set by main, or by a test
CHECK_POLLS, CHECK_PAUSE = 60, 10  # pr-mode close waits up to 10 min for the 'tests' check / the MR pipeline

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

def worker_name(item, letter='T'):
    """R3 naming (#268, #572): `T<N> <ORCH> <title> (<machine>)`, `S<N> ...` the supervisor; ORCH is the task's `pm` on the
    board, never the caller that runs the pass; a task with no `pm` (R3 Transition): the caller, UNK for the owner's shell."""
    launcher = (item.get('pm') or {}).get('runtime') or (session() or {}).get('runtime') or os.environ.get('TASKQ_RUNTIME')
    return f'{letter}{item["iid"]} {ORCH.get(launcher, "UNK")} {item["title"][:40]} ({machine()})'

def session():
    """This agent session, or None for the owner's shell. TASKQ_RUNTIME picks one when a session inherited another's id."""
    found = [r for r in SESSIONS if os.environ.get(SESSIONS[r]) and os.environ.get('TASKQ_RUNTIME', r) == r]
    return {'runtime': found[0], 'session': os.environ[SESSIONS[found[0]]]} if found else None

def origin():
    """R3 (#532): the task's manager, the session that files or adopts it: {runtime, session, name}; a plain shell names its
    runtime with TASKQ_RUNTIME (no session); neither: None, the task waits for an explicit adoption."""
    me = session() or {}
    runtime = me.get('runtime') or os.environ.get('TASKQ_RUNTIME')
    return {'runtime': runtime, 'session': me.get('session'), 'name': machine()} if runtime else None

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
            'updated_at': issue.get('updated_at'), 'url': issue.get('url'), 'text': BLOCK.sub('', issue['body']).strip(),
            'assignees': issue.get('assignees') or []}

def mine(item):
    """#480: `"assignee"` in taskq.json, "me" (the board's user) or a login: only its tasks run here; unset: every task."""
    wanted = CONFIG.get('assignee')
    if wanted == 'me':
        wanted = CONFIG['me'] = CONFIG.get('me') or BOARD.user()  # one board call per process
    return not wanted or wanted in item['assignees']

def execution_reason(item):
    """#545: an `assignee-only` task runs only for the board's authenticated user among its native assignees. None: allowed."""
    if 'assignee-only' not in item['labels']:
        return None
    prefix = f'#{item["iid"]} assignee-only'
    if not item['assignees']:
        return f'{prefix}: no board assignees; execution refused'
    try:
        identity = BOARD.user()  # never CONFIG['assignee'] or its cached 'me': selection, not authentication
    except (Exception, SystemExit) as error:
        return f'{prefix}: authenticated board identity unavailable ({error}); execution refused'
    if not isinstance(identity, str) or not identity.strip():
        return f'{prefix}: authenticated board identity is empty; execution refused'
    if identity not in item['assignees']:
        return f'{prefix}: authenticated board user {identity} is not assigned; execution refused'
    return None

def executable(issue):
    """#545: a fresh board issue the pass may act on: a task, and eligible; else the reason goes to stderr."""
    item = parse(issue)
    reason = execution_reason(item) if item else f'#{issue["iid"]} is no longer a task'
    reason and print(f'taskq: {reason}', file=sys.stderr)
    return item if not reason else None

def block(text, fields):
    """The description: the task's text, then its JSON block. Keys the model does not know are kept as they are."""
    return f'{text}\n\n<!-- taskq:start -->\n```json\n{json.dumps(fields, indent=1, ensure_ascii=False)}\n```\n<!-- taskq:end -->'


# --- board ----------------------------------------------------------------------------------
# Six functions: list(state), get(n), add(title, body, labels), update(n, labels=None, body=None), comment(n, text),
# close(n). An issue is {iid, title, body, labels, state: open|closed, updated_at, url, assignees} and from get also comments.
# Optional seventh: user() -> the login "assignee": "me" stands for.

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
                'labels': [label['name'] for label in item['labels']], 'state': item['state'], 'updated_at': item['updated_at'],
                'assignees': [user['login'] for user in item.get('assignees') or []]}

    def user(self):
        return run_api('gh', self.host, 'GET', 'user')['login']

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
                'updated_at': item['updated_at'], 'assignees': [user['username'] for user in item.get('assignees') or []]}

    def user(self):
        return run_api('glab', self.host, 'GET', 'user')['username']

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
# Optional fifth: retire(gone, running=True) stops and removes this machine's `T<N>` / `S<N>` sessions for which
# gone(N, session, live) is true: close calls it for its task's workers (#302, #360), the tick for sessions the board
# no longer holds, stopped ones only. A session goes only by the id its task records (R11, #478, #525).
# Optional sixth: tail(session), the last log line. Optional seventh: state(session) -> 'running', 'idle', 'dead' or
# None, a supervisor's liveness (taskq.md § 7 step 4); without it alive() stands in (False: dead).

def worker_env():  # a worker must not inherit the tick's session id, nor a spawning worker's task (#333)
    return {key: value for key, value in os.environ.items() if key not in (*SESSIONS.values(), 'TASKQ_TASK', 'TASKQ_RUNTIME')}

class Claude:
    # #38, #51, #71: a worker gets only these tools, no MCP, no Chrome, and a pinned mode (else `auto` stops `taskq`).
    # `--tools` takes several values: a flag must follow it, never the prompt.
    TOOLS = ['Bash', 'Read', 'Edit', 'Write', 'Glob', 'Grep', 'WebFetch', 'WebSearch']
    names = r'[TS](\d+) [A-Z]{3} '  # worker_name; `taskq cleanup` widens it to the old `T<N> ` / `S<N> ` names
    SELF_WAKE = True  # while its process runs, a supervisor wakes on its own background `taskq wait --task N`

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

    def state(self, session):
        """A supervisor (§ 7 step 4): a listed pid is running (its own background wait wakes it); listed with no pid, its
        turn ended: idle, the pass resumes it (a new id, recorded); not listed or `failed`: dead."""
        agents = self.agents()
        if agents is None:
            return None
        agent = agents.get(session)
        return 'dead' if not agent or not agent.get('pid') and agent.get('state') == 'failed' else 'running' if agent.get('pid') else 'idle'

    def retire(self, gone, running=True):
        """`claude stop` + `claude rm`: duplicate spawns and resumes leave several jobs per task, and a stopped job stays listed (#360)."""
        claude = shutil.which('claude') or 'claude'
        for agent in (self.agents() or {}).values():
            n = re.match(self.names, agent.get('name') or '')  # worker_name: never the owner's own jobs
            if not n or not agent.get('id') or not gone(int(n[1]), agent.get('sessionId'), self.running(agent)) or self.running(agent) and not running:
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

def codex_options():
    # Network on: a worker pushes and calls the board. `"codex": [...]` in taskq.json replaces these options.
    return CONFIG.get('codex', ['-s', 'workspace-write', '-c', 'sandbox_workspace_write.network_access=true',
                                '--add-dir', str(CONFIG['root'] / '.git')])  # git fetch/commit write the main .git

class Unnamed(Exception):
    """R3 (#572): a started session whose native name was not confirmed; `thread` is its id."""

    def __init__(self, thread, why):
        super().__init__(f'{thread} not named: {why}')
        self.thread = thread

class Codex:
    """`codex exec`, headless: one process per turn, its JSONL in .taskq/<name>.log, `<pid> <thread>` in .taskq/<name>.pid."""

    def folder(self):
        (CONFIG['root'] / '.taskq').mkdir(exist_ok=True)
        return CONFIG['root'] / '.taskq'

    def exec(self, name, arguments, cwd):
        log, detach = self.folder() / f'{name.split()[0]}.log', {'creationflags': 0x208} if os.name == 'nt' else {'start_new_session': True}
        with open(log, 'ab') as out:  # detached: the worker outlives the tick
            process = subprocess.Popen([shutil.which('codex') or fail('codex not found'), 'exec', '--json', *codex_options(), *arguments],
                                       cwd=cwd, env=worker_env(), stdin=subprocess.DEVNULL, stdout=out, stderr=subprocess.STDOUT, **detach)
        dispatch('turn end', [], after=process.pid)  # #525: a sandboxed turn starts no pass; one runs when it ends (no sender, no timer)
        return process, log

    def spawn(self, name, prompt, cwd):
        log = self.folder() / f'{name.split()[0]}.log'
        start = log.stat().st_size if log.exists() else 0  # #495: the log is appended across runs, older ids sit above
        process, log = self.exec(name, ['-C', str(cwd), prompt], cwd)
        for _ in range(600):  # the run's first JSONL line, thread.started, carries the thread id
            with open(log, 'rb') as file:
                file.seek(start)
                found = ([None] + re.findall(r'"thread_id":\s*"([\w-]+)"', file.read().decode('utf-8', 'replace')))[-1]
            if found or process.poll() is not None:
                break
            time.sleep(0.1)
        found or fail(f'codex exec gave no thread id: see {log}')
        pid = log.with_suffix('.pid')
        old = pid.read_text().split() if pid.exists() else []
        if old[1:] and old[1] != found:  # #568: a replaced thread not yet retired (a running one) keeps its handle
            pid.rename(pid.with_name(f'{pid.stem}-{old[1]}.pid'))
        pid.write_text(f'{process.pid} {found}')
        try:
            self.title(found, name)
        except (OSError, ValueError) as error:  # R3 (#572): no unnamed thread works; its pid file stays for the retire (R11)
            process.terminate()
            raise Unnamed(found, error)
        return found

    WAIT = 60  # seconds for the whole app-server exchange, its shutdown included

    def title(self, thread, name):
        """R3 (#572): `codex exec` names no thread; the app-server's `thread/name/set` does, `thread/read` proves it.
        Each request waits for its own successful reply. `exec resume` keeps the name. Raises ValueError unless named."""
        server = subprocess.Popen([shutil.which('codex') or 'codex', 'app-server'], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, env=worker_env(), text=True, encoding='utf-8')
        timer = threading.Timer(self.WAIT, server.kill)  # one deadline through shutdown: a hung server is killed, its stdout ends
        timer.start()

        def send(message):
            server.stdin.write(json.dumps({'jsonrpc': '2.0', **message}) + '\n')
            server.stdin.flush()

        def call(n, method, params):
            send({'id': n, 'method': method, 'params': params})
            for line in server.stdout:  # notifications and the server's own requests pass by
                reply = json.loads(line) if line.startswith('{') else {}
                if reply.get('id') == n and 'method' not in reply:
                    if 'result' not in reply:
                        raise ValueError(f'{method}: {reply.get("error")}')
                    return reply['result']
            raise ValueError(f'{method}: no reply in {self.WAIT} s')
        try:
            call(0, 'initialize', {'clientInfo': {'name': 'taskq', 'version': '1'}})
            send({'method': 'initialized'})
            call(1, 'thread/name/set', {'threadId': thread, 'name': name})
            read = call(2, 'thread/read', {'threadId': thread}).get('thread') or {}
            if (read.get('id'), read.get('name')) != (thread, name):
                raise ValueError(f'thread/read: {read.get("id")} named {read.get("name")!r}')
        finally:
            with contextlib.suppress(OSError):
                server.stdin.close()
            server.wait()  # bounded: the timer kills it
            timer.cancel()
            server.stdout.close()

    def pid_file(self, session):
        return next((path for path in self.folder().glob('*.pid') if path.read_text().split()[1:] == [session]), None)

    def send(self, session, text):
        process, log = self.exec(getattr(self.pid_file(session), 'stem', session), ['resume', session, text], CONFIG['root'])
        log.with_suffix('.pid').write_text(f'{process.pid} {session}')
        return session

    def alive(self, session):
        path = self.pid_file(session)
        return None if path is None else pid_alive(int(path.read_text().split()[0]))

    def state(self, session):
        """A supervisor (§ 7 step 4): its process exits at every turn end, by design. Pid running: running; exited after a
        last turn that ended `turn.completed`, with a local rollout: idle; anything else: dead."""
        path = self.pid_file(session)
        if path is None:
            return 'dead'
        if pid_alive(int(path.read_text().split()[0])):
            return 'running'
        log = path.with_suffix('.log')
        turn = (log.read_text('utf-8', 'replace') if log.exists() else '').rpartition('"turn.started"')
        ends = re.findall(r'"type":\s*"(turn\.completed|turn\.failed|error)"', turn[2]) if turn[1] else []
        return 'idle' if ends[-1:] == ['turn.completed'] and rollout(session) == 'local' else 'dead'

    def resumable(self, session):  # a dead supervisor's first recovery: `exec resume` of the same thread
        return rollout(session) == 'local'

    def retire(self, gone, running=True):
        """Kill the turn's process, `codex archive` the thread, drop .taskq/T<N>.pid or S<N>.pid (#360)."""
        for path in (CONFIG['root'] / '.taskq').glob('*.pid'):
            n, (pid, thread) = re.fullmatch(r'[TS](\d+)(-[\w-]+)?', path.stem), path.read_text().split()  # S<N>-<thread>: a replaced one
            if not n or not gone(int(n[1]), thread, pid_alive(int(pid))) or pid_alive(int(pid)) and not running:
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
    n = BOARD.add(args.title, block(text, {'scope': args.scope, 'deps': args.deps, 'claim': None, 'result': None, 'pm': origin()}), labels)
    BOARD.comment(n, f'**add** · {who()}')
    print(f'#{n} {state}')
    return n

def cmd_list(args):
    found = [item for item in map(parse, BOARD.list(args.state)) if item and mine(item)]
    for item in sorted(found, key=lambda item: (STATES.index(item['state']), item['priority'], item['iid'])):
        claim = item['claim'] or {}
        detail = {'ready': 'continue' if claim else '', 'later': item['raw'].get('waiting_for') or '',
                  'doing': f'{claim.get("runtime")}:{(claim.get("session") or "")[:8]} @{claim.get("name")}' if claim else '',
                  'waiting': f'open dependencies {open_deps(item["deps"])}' if item['state'] == 'waiting' else ''}.get(item['state'], '')
        print(f'#{item["iid"]:<4} {item["state"]:<8} p{item["priority"]} {item["runtime"]:<6} {item["title"]}'
              + (f'  [{detail}]' if detail else ''))

def cmd_take(args):
    current, mine = task(args.n, 'ready'), session() or fail('take needs an agent session: set ' + ' or '.join(SESSIONS.values()))
    guarded = 'assignee-only' in current['labels']  # #576: unlabelled take is unchanged
    with dispatch_lock() if guarded else contextlib.nullcontext(True) as locked:
        if not locked:
            fail('cannot take: another taskq process holds .taskq/dispatch.lock; nothing taken, run it again')
        reason = execution_reason(current)
        if reason:
            fail(reason)
        if guarded:  # re-read under the lock after the identity call: a pass may have claimed the task meanwhile
            current = task(args.n, 'ready')
            reason = execution_reason(current) or current['supervisor'] and f'#{args.n} already has a supervisor'
            if reason:
                fail(reason)
        if open_deps(current['deps']):
            fail(f'#{args.n} has open dependencies')
        move(current, 'doing', 'take', claim={**mine, 'name': machine()}, result=None)

def decision(args):
    """#490: the decision card of an ask or result: what was done (the text's first line), results, options, one recommended."""
    if not args.option and args.recommend != 1:
        fail('--recommend needs --option')
    if args.option and not 1 <= args.recommend <= len(args.option):
        fail(f'--recommend {args.recommend}: pick 1 to {len(args.option)}')
    return {'summary': args.text.strip().split('\n')[0][:120], 'links': args.link, 'options': args.option, 'recommend': args.recommend}

# Moves with no other check: command -> (states it takes from, state it goes to, block changes).
# A supervisor asks from review too: a result with options, or the third rework (§ 7 Supervisor).
DROP = {'claim': None, 'supervisor': None, 'order': None}  # R11: the task's sessions are retired once stopped
MOVES = {'ask': (('doing', 'review'), 'ask', lambda args: {'decision': decision(args)}), 'answer': (('ask',), 'doing', lambda args: {'decision': None}),
         'requeue': (STATES, 'ready', lambda args: {**DROP, 'result': None, 'decision': None}),
         'later': (STATES, 'later', lambda args: {**DROP, 'waiting_for': args.text or None}),
         'result': (('doing',), 'review', lambda args: {'result': {'sha': args.sha, 'checks': args.checks}, 'decision': decision(args)})}

def cmd_move(args):
    sources, state, fields = MOVES[args.command]
    move(task(args.n, *sources), state, args.command, args.text, **fields(args))

def role(current):
    """Who runs this command for the task: supervisor, worker, manager, owner (a plain shell), or None (another session)."""
    me = (session() or {}).get('session')
    return 'owner' if not me else next((name for name, held in (('supervisor', current['supervisor']), ('worker', current['claim']),
                                                                ('manager', current['pm'])) if me == (held or {}).get('session')), None)

def gate(current):
    """R3 one controller (#525): `run`, `close` and a rework `requeue` of a supervised task come from its supervisor, the
    task's own manager, its `pm` (on the owner's word, #532), or the owner's shell; another session, another manager too, is refused."""
    found, boss = role(current), current['supervisor']
    if boss and found not in ('supervisor', 'manager', 'owner'):
        fail(f'#{current["iid"]} is supervised by {boss["runtime"]}:{boss["session"][:8]}: only it, the task\'s manager or the owner controls it')
    return found

def workers(n, comments):
    """Workers spawned for task n since the last answer: the first plus each rework."""
    count = 0
    for text in reversed(comments or []):
        if text.startswith('**answer**'):
            break
        count += text.startswith('**spawn**') and '\n\nworker ' in text
    return count

def cmd_run(args):
    """The supervisor orders its worker: the next pass on its machine spawns T<N> (§ 7 step 4)."""
    current = task(args.n, 'doing')
    current['supervisor'] or fail(f'#{args.n} has no supervisor: the tick runs it')
    gate(current)
    if (current['claim'] or {}).get('session'):
        fail(f'#{args.n} has a worker: {current["claim"]["session"]}')
    move(current, 'doing', 'run', args.text, order='run')

def cmd_requeue(args):
    """Supervised (#525): the supervisor's requeue is a rework (a new worker continues the branch; the third is an ask);
    the worker's drops only its claim and wakes the supervisor; the manager's or owner's drops the supervisor too."""
    current = task(args.n)
    found, claim = role(current), current['claim'] or {}
    if current['supervisor'] and found == 'worker':
        move(current, current['state'], 'requeue', args.text, claim={**claim, 'session': None})
        return
    gate(current)
    if current['supervisor'] and found == 'supervisor':
        if workers(args.n, BOARD.get(args.n)['comments']) >= 3:
            fail(f'#{args.n}: third rework: ask the owner (taskq ask {args.n} ...)')
        move(current, 'doing', 'requeue', args.text, claim={**claim, 'session': None}, result=None, decision=None, order='rework')
    else:
        cmd_move(args)

def codes(words):
    """'43.1 44.2' (spaces or commas) -> [(43, 1), (44, 2)]."""
    found = [re.fullmatch(r'(\d+)\.(\d+)', word) for word in re.split(r'[\s,]+', ' '.join(words).strip())]
    return [(int(code[1]), int(code[2])) for code in found] if all(found) else fail(f'{" ".join(words)!r}: answer N --text A, or codes like 43.1 44.2')

def cmd_answer(args):
    """`answer N --text A`, or `answer 43.1 44.2` (#490): each code picks an option of the task's card. An ask goes back to
    `doing` with the option's text; a review: an option starting `close` closes it, another goes back to the worker.
    Every code is checked before any task moves."""
    if args.text:
        if len(args.n) != 1 or not args.n[0].isdigit():
            fail('answer N --text A: one task number')
        cmd_move(argparse.Namespace(**{**vars(args), 'n': int(args.n[0])}))
        return [int(args.n[0])]
    picks = []
    for n, k in codes(args.n):
        current = task(n, 'ask', 'review')
        options = (current['raw'].get('decision') or {}).get('options') or []
        picks.append((current, k, 0 < k <= len(options) and options[k - 1] or fail(f'#{n} has no option {k}')))
    for current, k, text in picks:
        if current['state'] == 'review' and text.lower().startswith('close'):
            close_one(argparse.Namespace(n=current['iid'], text=f'{current["iid"]}.{k}: {text}'))
        else:
            move(current, 'doing', 'answer', f'{current["iid"]}.{k}: {text}', decision=None)
    return [current['iid'] for current, _, _ in picks]

def commit(sha):
    """A result's commit: hex only, so it never reaches git as an option."""
    return sha if re.fullmatch('[0-9a-f]{7,40}', sha) else fail(f'{sha!r} is not a commit: 7 to 40 lowercase hex digits')

def merge(current, sha):
    """pr mode: squash-merge the one open PR/MR of branch taskq-<N> into main at the result SHA: the merge commit, None with no PR.
    It merges only once the gate passes on the PR head: GitHub the 'tests' check (#359), GitLab the MR's pipeline (#479);
    a PR behind main is not updated.
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

    def back(why):  # a supervised task stays with its supervisor: it requeues with the fixes (§ 7 Supervisor 3.4)
        kept = {'claim': {**(current['claim'] or {}), 'session': None}} if current['supervisor'] else {'claim': None}
        move(current, 'doing' if current['supervisor'] else 'ready', 'requeue', f'close: PR {number} {why}', result=None, **kept)
        fail(f'#{current["iid"]}: PR {number} {why}')
    if lab:  # the MR's latest pipeline on its head; failed, canceled or skipped sends it back
        gate = 'pipeline'
        runs = lambda: [(pipe['status'] in ('success', 'failed', 'canceled', 'skipped'), pipe['status'] == 'success') for pipe in
                        run_api('glab', host, 'GET', f'projects/{quote(CONFIG["repo"], safe="")}/merge_requests/{number}/pipelines')
                        if pipe['sha'] == head][:1]
    else:  # #359: main requires 'tests' on the PR head only (not strict); a behind PR merges as is, GitHub refuses a conflict
        gate = 'check tests'
        runs = lambda: [(run['status'] == 'completed', run['conclusion'] == 'success') for run in
                        run_api('gh', host, 'GET', f'repos/{CONFIG["repo"]}/commits/{head}/check-runs?check_name=tests')['check_runs']]
    for _ in range(CHECK_POLLS):  # ponytail: fixed poll; tests.yml takes 8-13 s
        polled = runs()
        if polled and all(done for done, _ in polled):
            if not all(ok for _, ok in polled):
                back(f'{gate} failed on {head}')
            break
        time.sleep(CHECK_PAUSE)
    else:
        back(f'{gate} did not finish on {head}')
    keep = CONFIG.get('workspace') == 'external'  # #477: the host owns the branch; the repo's own policy may still delete it
    _, out = cli(*(['glab', 'mr', 'merge', number, '--squash', *['--remove-source-branch'] * (not keep), '--sha', head, '--auto-merge=false', '--yes'] if lab
                   else ['gh', 'pr', 'merge', number, '--squash', *['--delete-branch'] * (not keep), '--match-head-commit', head]))
    code, viewed = cli(*(['glab', 'mr', 'view', number, '--output', 'json'] if lab else ['gh', 'pr', 'view', number, '--json', 'state,mergeCommit']))
    pr = {} if code else json.loads(viewed)
    if str(pr.get('state')).lower() != 'merged':  # read back: a merge that reported an error may still have merged
        back(f'did not merge: {out}')
    return pr.get('merge_commit_sha') or pr.get('squash_commit_sha') or pr['mergeCommit']['oid']

def cleanup(current):
    """The worker's .worktrees/taskq-<N> and branch taskq-<N>, on the claim's machine: removed when clean, else kept
    with a note for the close comment. Never --force: that lost a worker's changes (#284). `"workspace": "external"`: never (#477)."""
    if CONFIG.get('workspace') == 'external':
        return 'kept: owned by host'
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

def publish_direct(current, sha, git):
    """#533: close is acceptance; transfer and qualify the exact candidate before main can change."""
    if role(current) not in ('supervisor', 'manager', 'owner'):
        fail(f'#{current["iid"]}: only the accepting reviewer can publish a direct candidate')

    def run(*args):
        done = subprocess.run([*git, *args], capture_output=True, text=True, encoding='utf-8')
        if done.returncode:
            fail(done.stderr.strip() or f'git {args[0]} failed')
        return done.stdout.strip()

    branch = f'refs/heads/taskq-{current["iid"]}'
    if run('ls-remote', 'origin', branch).split()[:1] != [sha]:
        fail(f'{branch}: remote candidate does not match result {sha}')
    run('merge-base', '--is-ancestor', 'origin/main', sha)  # fast-forward only; never discard new main work
    board, host = CONFIG['board'], CONFIG.get('host')
    if board == 'github':
        checks = run_api('gh', host, 'GET', f'repos/{CONFIG["repo"]}/commits/{sha}/check-runs?check_name=tests')['check_runs']
        green = bool(checks) and all(c['status'] == 'completed' and c['conclusion'] == 'success' for c in checks)
    elif board == 'gitlab':
        checks = run_api('glab', host, 'GET', f'projects/{quote(CONFIG["repo"], safe="")}/pipelines?sha={sha}')
        green = bool(checks) and checks[0]['sha'] == sha and checks[0]['status'] == 'success'
    else:  # custom board: the reviewer verifies project-owned CI/checks (§ 6)
        green = True
    if not green:
        fail(f'#{current["iid"]}: CI is not green on candidate {sha}')
    if run('ls-remote', 'origin', branch).split()[:1] != [sha]:
        fail(f'{branch}: remote candidate changed during acceptance')
    run('push', 'origin', f'{sha}:refs/heads/main')
    run('fetch', 'origin')
    run('merge-base', '--is-ancestor', sha, 'origin/main')  # read back, including a concurrent later fast-forward

def verdict(text):
    """#567 (§ 7 Supervisor 3.3): the manager gets `<accepted>; <changed for the user>; open: <follow-ups or none>`, never a SHA."""
    line = (text or '').strip().split('\n')[0].strip()
    if not line or re.fullmatch(r'[0-9a-f]{7,40}', line) or re.match(r'(merged|published)\b', line, re.I) or 'open:' not in line:
        fail('close --text needs a verdict: "<what was accepted>; <what changed for the user>; open: <follow-ups or none>"')

def close_one(args):
    current = task(args.n, 'review')
    gate(current)
    if role(current) == 'supervisor':
        verdict(args.text)
    sha = commit((current['result'] or {}).get('sha') or '')
    if CONFIG['publish'] == 'pr' and (merged := merge(current, sha)):
        args.text = (f'{args.text}\n\n' if args.text else '') + f'merged {merged}'  # #567: the verdict stays the first line
    else:  # an already-published result/answer, or an unpublished direct candidate
        git = [shutil.which('git') or fail('git not found'), '-C', str(CONFIG['root'])]
        fetched = subprocess.run([*git, 'fetch', 'origin'], capture_output=True)
        if fetched.returncode:
            fail(f'#{args.n}: could not fetch origin for publication')
        if subprocess.run([*git, 'merge-base', '--is-ancestor', sha, 'origin/main'], capture_output=True).returncode:
            if CONFIG['publish'] != 'direct':
                fail(f'#{args.n}: result {sha} is not on origin/main')
            publish_direct(current, sha, git)
            args.text = (f'{args.text}\n\n' if args.text else '') + f'published {sha}'
    claim = current['claim'] or {}
    if hasattr(runtimes().get(claim.get('runtime')), 'retire') and claim.get('name') not in (None, machine()):
        args.text = (f'{args.text}\n\n' if args.text else '') + f'session {claim.get("session")} runs on {claim.get("name")}: stop it there'
    kept = cleanup(current)
    BOARD.close(args.n)  # first (#496): a failed close keeps the q-* label, the task stays on the board
    move(current, None, 'close', '\n\n'.join(filter(None, (args.text, kept))))
    if os.environ.get('CODEX_SANDBOX'):  # #502: the sandbox can neither stop nor archive: the next pass outside retires them
        return
    issue, me = BOARD.get(args.n), (session() or {}).get('session')  # R11: its recorded workers, never the session running close
    retire(lambda name, n, sid, _: n == args.n and sid != me and recorded(issue, name, sid) == 'worker', f'#{args.n}: could not stop its sessions')

def recorded(issue, runtime, sid):
    """R11 (#478, #525): 'supervisor' or 'worker' when the task records this session (the block's supervisor or claim,
    a spawn, nudge or gone note, the take note's `<runtime>:<id[:8]>`), else None: a name alone never counts."""
    found = BLOCK.search((issue or {}).get('body') or '')
    raw = json.loads(found.group(1)) if found else {}
    if not sid:
        return None
    for name, key in (('supervisor', 'supervisor'), ('worker', 'claim')):
        if (raw.get(key) or {}).get('runtime') == runtime and raw[key].get('session') == sid:
            return name
    named = re.compile(rf'(?<![\w-]){re.escape(sid)}(?![\w-])')
    for text in (issue or {}).get('comments') or []:
        if text.startswith(('**spawn**', '**nudge**', '**gone**')) and named.search(text):
            return 'supervisor' if '\n\nsupervisor ' in text else 'worker'
        if text.startswith(f'**take** · {runtime}:{sid[:8]}'):
            return 'worker'
    return None

def retire(gone, why, running=True):
    """Each runtime's retire, best effort: a session left behind never fails close or the tick. gone(runtime, n, session, live)."""
    for name, runtime in runtimes().items():
        try:
            getattr(runtime, 'retire', lambda *_: None)(lambda n, sid=None, live=None, name=name: gone(name, n, sid, live), running)
        except Exception as error:
            print(f'{why}: {error}', file=sys.stderr)

def open_prs():
    """{branch: PR/MR number} of the open PRs of the repo, None when they cannot be read (a board file, no CLI)."""
    lab, host = CONFIG['board'] == 'gitlab', CONFIG.get('host')
    if CONFIG['board'] not in ('github', 'gitlab') or not shutil.which('glab' if lab else 'gh'):
        return None
    command = ['glab', 'mr', 'list', '--output', 'json', '-R', f'https://{host}/{CONFIG["repo"]}' if host else CONFIG['repo']] if lab else \
        ['gh', 'pr', 'list', '--state', 'open', '--limit', '500', '--json', 'number,headRefName', '-R', f'{host}/{CONFIG["repo"]}' if host else CONFIG['repo']]
    done = subprocess.run([shutil.which(command[0]), *command[1:]], capture_output=True, text=True, encoding='utf-8')
    return None if done.returncode else {pr.get('source_branch', pr.get('headRefName')): pr.get('iid', pr.get('number')) for pr in json.loads(done.stdout)}

def merged(git, ref):
    """True: `ref` holds nothing origin/main lacks (an ancestor, or a squash-merged PR: merging it changes no file).
    False: unmerged work. None: cannot tell (no origin/main, git older than 2.38)."""
    if not git('merge-base', '--is-ancestor', ref, 'origin/main').returncode:
        return True
    tree, main = git('merge-tree', '--write-tree', 'origin/main', ref), git('rev-parse', 'origin/main^{tree}')
    if tree.returncode > 1 or main.returncode:
        return None
    return not tree.returncode and tree.stdout.split()[:1] == [main.stdout.strip()]

def cmd_cleanup(args):
    """#476: on demand only. Remove this machine's leftovers of tasks not open: clean worktrees, branches with nothing
    unmerged, taskq's own sessions, stale .taskq handles; print what was kept and why, then the queue mess.
    Never --force; never unmerged work (R11). `workspace: external` (#477): worktrees and branches are not touched."""
    root, dry, here, kinds = CONFIG['root'], args.dry_run, machine(), runtimes()
    verb, removed, kept, mess = 'would remove' if args.dry_run else 'removed', [], [], []
    items = list(filter(None, map(parse, BOARD.list(None))))
    states = {item['iid']: 'open' for item in items}

    issues = {}

    def state(n):  # open, closed, or unknown (no such issue, a board error)
        if n not in states:
            try:
                issues[n] = BOARD.get(n)
                states[n] = issues[n]['state']
            except (Exception, SystemExit):
                states[n] = 'unknown'
        return states[n]

    def git(*argv):
        return subprocess.run([shutil.which('git') or fail('git not found'), '-C', str(root), *argv], capture_output=True, text=True, encoding='utf-8')

    def act(what, *command):  # one removal: done (or only printed with --dry-run), else kept with git's reason
        done = None if dry else git(*command)
        if done and done.returncode:
            kept.append(f'{what}: {last_line(done.stderr + done.stdout)}')
            return False
        removed.append(f'{verb} {what}')
        return True
    task_of = lambda name: int(re.fullmatch(r'taskq-(\d+)', name)[1])
    gone_trees = set()
    if CONFIG.get('workspace') == 'external':
        kept.append('worktrees and branches: owned by host (workspace: external)')
    else:
        git('fetch', '-q', '--prune', 'origin')  # #515: a branch GitHub already deleted is neither reported nor pushed
        dry or git('worktree', 'prune')
        for tree in sorted((root / '.worktrees').glob('taskq-*'), key=lambda path: path.name):
            if not re.fullmatch(r'taskq-\d+', tree.name) or not tree.is_dir():
                continue
            n, what = task_of(tree.name), f'worktree .worktrees/{tree.name}'
            status = git('-C', str(tree), 'status', '--porcelain')
            why = {'open': 'open task', 'unknown': 'unknown task'}.get(state(n)) or \
                ('unknown' if status.returncode else 'dirty' if status.stdout.strip() else None)
            if why:
                kept.append(f'{what}: {why}')
            elif act(what, 'worktree', 'remove', str(tree)):
                gone_trees.add(tree.name)
        listed = git('for-each-ref', '--format=%(refname)', 'refs/heads/taskq-*', 'refs/remotes/origin/taskq-*').stdout.split()
        for ref in listed:
            name = ref.rsplit('/', 1)[1]
            if not re.fullmatch(r'taskq-\d+', name):
                continue
            local, n = ref.startswith('refs/heads/'), task_of(name)
            what = f'branch {name}' if local else f'remote branch origin/{name}'
            if state(n) == 'open':
                kept.append(f'{what}: open task')
                continue
            if local and (root / '.worktrees' / name).is_dir() and name not in gone_trees:
                kept.append(f'{what}: its worktree is kept')
                continue
            whole = merged(git, ref)
            if whole:
                act(what, *(['branch', '-D', name] if local else ['push', '-q', 'origin', '--delete', name]))
            else:
                kept.append(f'{what}: {"unknown" if whole is None else "unmerged commits"}')
                mess.append(f'{what}: task #{n} is not open')

    def goner(name):  # #478: only a stopped session the board records for a closed task; a name alone is reported, never removed
        def gone(n, session=None, live=None):
            what = f'{name} session {session or "?"} of #{n}'
            why = {'open': 'open task', 'unknown': 'unknown task'}.get(state(n)) or \
                ('running' if live else 'liveness unknown' if live is None else None) or \
                (None if recorded(issues[n], name, session) else 'name only, not recorded on the board')
            if why:
                kept.append(f'{what}: {why}')
                return False
            removed.append(f'{verb} {what}')
            return not dry
        return gone
    for name, runtime in kinds.items():
        if isinstance(runtime, Claude):
            runtime.names = r'[TS](\d+) '  # old names too: `T<N> <title>`, the gone supervisor `S<N>`
        try:
            getattr(runtime, 'retire', lambda *_: None)(goner(name), False)  # never a live worker
        except Exception as error:
            kept.append(f'{name} sessions: unknown ({error})')
    for path in sorted((root / '.taskq').glob('*.pid')):
        n = re.fullmatch(r'S(\d+)', path.stem)  # the gone supervisor's; a T<N>.pid is Codex's handle, its retire decides (#478)
        pid = (path.read_text().split() or ['0'])[0]
        if n and state(int(n[1])) == 'closed' and not (pid.isdigit() and pid_alive(int(pid))):
            dry or path.unlink()
            removed.append(f'{verb} .taskq/{path.name}')
    for wait in sorted((root / '.taskq').glob('wait*.json')):  # one per manager (#532)
        seen = json.loads(wait.read_text('utf-8'))
        stale = [n for n in seen if not n.isdigit() or state(int(n)) != 'open']
        if stale:
            dry or wait.write_text(json.dumps({n: value for n, value in seen.items() if n not in stale}), 'utf-8')
            removed.append(f'{verb} .taskq/{wait.name} entries {" ".join(f"#{n}" for n in stale)}')
    prs = open_prs()
    for item in items:
        claim, n, sha = item['claim'] or {}, item['iid'], (item['result'] or {}).get('sha') or ''
        if item['state'] == 'doing' and claim.get('name') == here and claim.get('runtime') in kinds \
                and kinds[claim['runtime']].alive(claim['session']) is False:
            mess.append(f'#{n} doing: session {claim["session"]} is gone')
        if item['state'] == 'review' and CONFIG['publish'] == 'pr' and prs is not None and f'taskq-{n}' not in prs and not (
                re.fullmatch('[0-9a-f]{7,40}', sha) and not git('merge-base', '--is-ancestor', sha, 'origin/main').returncode):  # an answer is on main
            mess.append(f'#{n} review: no open PR')
    for branch, number in sorted((prs or {}).items(), key=lambda pair: str(pair[0])):
        found = re.fullmatch(r'taskq-(\d+)', branch or '')
        if found and state(int(found[1])) != 'open':
            mess.append(f'PR {number} ({branch}): task #{found[1]} is not open')
    if prs is None:
        mess.append('open PRs: unknown (no gh/glab or a board file)')
    print('\n'.join([*removed, *(f'kept {line}' for line in kept), *(f'mess: {line}' for line in mess)]) or 'nothing to clean')

def history(n):
    """The review notes a new worker must read: every requeue and answer since the last close, with the result they answer."""
    notes = [text for text in BOARD.get(n)['comments'] or [] if text.startswith(('**result**', '**requeue**', '**answer**', '**ask**'))]
    return 'History of this task (read it first; a requeue says what to fix):\n\n' + '\n\n'.join(notes[-6:]) + '\n\n' if notes else ''

def brief(item, runtime):
    """The worker's prompt: its name first (R3, #572: a runtime's fallback title), the task, its workspace, the taskq commands it uses."""
    n, root, tq = item['iid'], CONFIG['root'], f'python3 {Path(__file__).resolve()}'
    create = f'glab mr create --yes --target-branch main --source-branch taskq-{n} --title "<title>" --description' if CONFIG['board'] == 'gitlab' \
        else f'gh pr create --base main --head taskq-{n} --title "<title>" --body'
    push = f'`git push --force-with-lease origin HEAD:refs/heads/taskq-{n}`, open a pull request once (a push updates it):\n  `{create} "<summary>"`, ' \
        f'then `{tq} result {n} --sha <PR head full SHA>' if CONFIG['publish'] == 'pr' else f'`git push --force-with-lease origin HEAD:refs/heads/taskq-{n}`, then\n  `{tq} result {n} --sha <candidate full SHA>'
    workspace = f'take your workspace from the project instructions (AGENTS.md) or the path the manager gave, on branch taskq-{n};\nthe host owns it: never remove it' \
        if CONFIG.get('workspace') == 'external' else f'from {root} run `git fetch origin && git worktree add -b taskq-{n} .worktrees/taskq-{n} origin/main`, work only there,\nnever in the main checkout'
    return f'''{worker_name(item)}
You are the taskq worker for task #{n}: {item["title"]}. The task is claimed for you: do it without asking for confirmation.
Queue tool: `{tq}`. Start every shell command with `export TASKQ_TASK={n} TASKQ_RUNTIME={runtime} &&`.
Read `{Path(__file__).resolve().with_name("taskq.md")}` first and do only what it allows (R13): a task that conflicts with a recorded decision is an ask with options, not an edit.

{item["text"]}

{history(n)}Expected paths: {", ".join(item["scope"] or []) or "none named"}. They say where the work is expected, not what is forbidden.

Workspace: {workspace}; a branch taskq-{n} left by an earlier worker: continue it. A task that ends in an answer, not a commit,
needs no worktree. Commands:
- A question only the owner can decide (a product choice, an action that cannot be undone): `{tq} ask {n} --text "<what was done; the question>"
  --option "<A>" --option "<B>" --recommend <K> [--link <url of a result, image or video>]`, then stop. A result that leaves the owner a choice
  takes the same --option/--recommend/--link; an option starting `close` accepts the result.
- Cannot be done: `{tq} requeue {n} --text "<why>"`, then stop.
- Deliver: commit on branch taskq-{n}, `git fetch origin && git rebase origin/main`, run the tests required by § 10 Testing policy, {push} --checks "<commands and outcome>" --text "<summary>"`, then stop.
  An answer with no commit: the result names the current origin/main SHA and the text holds the answer.
Everything written through taskq is public: no secrets, tokens or paths outside the repository.'''

def supervisor_brief(item, runtime, kind):
    """S<N>'s prompt (§ 7 Supervisor): its name first (R3), the task, its orders, review and close; never the task's code."""
    n, tq, lab = item['iid'], f'python3 {Path(__file__).resolve()}', CONFIG['board'] == 'gitlab'
    wait = f'run `{tq} wait --task {n}` in the background (run_in_background) and end your turn; its output wakes you' \
        if getattr(kind, 'SELF_WAKE', False) else 'end your turn; the queue wakes you with the event'
    ci = f'glab api "projects/:id/pipelines?sha=<sha>"' if lab else 'gh run list --commit <sha>'
    view = f'glab issue view {n} --comments' if lab else f'gh issue view {n} --comments'
    nudge = f'glab issue note {n} -m "nudge: <text>"' if lab else f'gh issue comment {n} --body "nudge: <text>"'
    return f'''{worker_name(item, 'S')}
You are the taskq supervisor S{n} of task #{n}: {item["title"]}. You are its only controller: you order its worker, follow it,
review its result and close or rework it. You never edit the task's code, never start a session yourself, never decide for the owner.
Queue tool: `{tq}`. Start every shell command with `export TASKQ_TASK={n} TASKQ_RUNTIME={runtime} &&`.
Read `{Path(__file__).resolve().with_name("taskq.md")}` (§ 7 Supervisor, § 6) first and do only what it allows (R13).

{item["text"]}

{history(n)}Steps:
1. Read the whole issue: `{view}`. A task that conflicts with a recorded decision: `{tq} ask {n}` with options.
2. `{tq} run {n}` orders the worker (the queue spawns it); then {wait}.
3. Woken by `review #{n}`: `git fetch origin`, `git show <sha> --stat`, then the diff, against every Acceptance item and taskq.md
   ({CONFIG["publish"]} mode{": the PR diff" if CONFIG["publish"] == "pr" else ""}). A commit: CI on exactly that SHA is green (`{ci}`, where the project has CI);
   an answer on origin/main needs no CI. Evaluate § 10 testing evidence, sensitivity, blindspots and applicable
   isolated candidate live proof BEFORE publication. Missing required evidence is rework or ask, never PASS.
   close records your acceptance and publishes the exact candidate; the worker never pushes main.
   - Accepted: `{tq} close {n} --text "<what was accepted>; <what changed for the user>; open: <follow-ups or none>"`, then end
     your turn. One line for the manager, who thinks in tasks: no SHA, diff or test log; a bare SHA or `merged ...` is refused.
   - Not accepted, CI red, or close sent it back: `{tq} requeue {n} --text "<exact fixes>"`; a new worker continues the branch.
     Then wait as in 2. The third rework is refused: ask the owner.
   - A result with options (a choice for the owner): `{tq} ask {n} --text "<...>" --option "<A>" --option "<B>" --recommend <K>`.
4. Woken by `gone #{n}`, `requeue #{n}` (the worker's) or `ask #{n}`: read why; `{tq} requeue {n} --text "<what to do>"` or ask the owner
   (a product choice, a second death). `answer #{n}`: act on it; a live worker gets the answer from the queue. Then wait as in 2.
5. A silent worker (120 min, issue unchanged): `{nudge}`; the queue sends the text to it.
6. `stop #{n}`: the task is closed or no longer yours: end your turn. `tick`: wait again.
Everything written through taskq is public: no secrets, tokens or paths outside the repository.'''

def note(who, sid, kind):
    """A spawn note names the role and the id (R11 retires by it), then the link when the runtime has one."""
    return f'{who} {sid}' + (f'\n{link}' if (link := kind.link(sid)) else '')

def spawn_named(item, kind, letter, prompt):
    """Spawn under the R3 name. A session the runtime could not name (#572) is stopped, recorded by a `gone` note
    (R11 retires it), never as a spawn; the pass fails (§ 7) and the next one tries again."""
    try:
        return kind.spawn(worker_name(item, letter), prompt, CONFIG['root'])
    except Unnamed as error:
        role = 'supervisor' if letter == 'S' else 'worker'
        BOARD.comment(item['iid'], f'**gone** · {who()}\n\n{role} {error}')
        fail(f'#{item["iid"]}: {role} {error}')

def replace(item, kind, runtime, role, running=False):
    """#568: before a replacement spawn, retire the task's earlier `role` sessions the board records (R11, #478), never
    another task's. A Codex spawn rewrites `.taskq/T<N>.pid` / `S<N>.pid`, the old thread's handle. A replaced
    worker goes running or not; a supervisor only once stopped (R11), its handle kept until then. Best effort."""
    issue = BOARD.get(item['iid'])
    if not executable(issue):
        return False  # #576: a reassignment seen by this read: no retire, no spawn
    try:
        getattr(kind, 'retire', lambda *_: None)(lambda n, sid=None, live=None: n == item['iid']
                                                  and recorded(issue, runtime, sid) == role, running)
    except Exception as error:
        print(f'#{item["iid"]}: could not retire the replaced {role}: {error}', file=sys.stderr)
    return True

EVENT_OF = {'result': 'review', 'ask': 'ask', 'answer': 'answer', 'gone': 'gone', 'requeue': 'requeue'}

def pending(issue, boss):
    """The supervisor's events since it last got them (.taskq/S<N>.seen, else since its spawn note): `review #N`,
    `ask #N`, `answer #N`, `gone #N` (its worker), `requeue #N` (by its worker); its own ask and requeue are no event.
    Returns them and the comment count to mark them seen with."""
    n, sid, comments = issue['iid'], boss['session'], issue.get('comments') or []
    start = max([i + 1 for i, text in enumerate(comments) if text.startswith('**spawn**') and f'\n\nsupervisor {sid}' in text] or [0])
    held = (CONFIG['root'] / '.taskq' / f'S{n}.seen').read_text().split() if (CONFIG['root'] / '.taskq' / f'S{n}.seen').is_file() else []
    start = max(start, int(held[1])) if held[:1] == [sid] else start
    found = []
    for text in comments[start:]:
        head, _, body = text.partition('\n\n')
        action, _, by = head.partition(' · ')
        event = EVENT_OF.get(action.strip('*'))
        if event and not (event in ('ask', 'requeue') and by.endswith(f':{sid[:8]}')) and (event != 'gone' or body.startswith('worker ')):
            found.append(f'{event} #{n}')
    return found, len(comments)

def seen(n, sid, count):
    (CONFIG['root'] / '.taskq').mkdir(exist_ok=True)
    (CONFIG['root'] / '.taskq' / f'S{n}.seen').write_text(f'{sid} {count}')

def lead_state(kind, sid):
    """§ 7 step 4: running, idle or dead; None cannot tell. A runtime without state(): alive() (False: dead)."""
    return kind.state(sid) if hasattr(kind, 'state') else {True: 'running', False: 'dead'}.get(kind.alive(sid))

def lead_deaths(comments):
    """Supervisor deaths since the last result or answer (#393 bound)."""
    count = 0
    for text in reversed(comments or []):
        if text.startswith(('**result**', '**answer**')):
            break
        count += text.startswith('**gone**') and '\n\nsupervisor ' in text
    return count

def tail_of(kind, sid):
    try:
        return getattr(kind, 'tail', lambda _: '')(sid)
    except Exception as error:  # best effort: the note goes out without the line
        return f'no log: {error}'

def follow(item, kind, claim, supervised):
    """A live worker: an answer, a supervisor's `nudge:` comment or 120 silent minutes reach it once (step 2, step 4).
    None: this read shows the task ineligible (#576): the caller ends the task's step."""
    issue = BOARD.get(item['iid'])
    if not executable(issue):
        return None  # #576: a reassignment seen by this read: no send, the claim stays
    last = (issue['comments'] or [''])[-1]
    answer = last.partition('\n\n')[2] if last.startswith('**answer**') else None  # #307: an answer wakes the worker at once
    told = last.partition('nudge:')[2].strip() if supervised and last.startswith('nudge:') else None
    if answer is None and told is None and age(item) < 120:
        return claim
    text = f'The owner answered your question:\n\n{answer}' if answer is not None else told or 'continue: read your issue'
    old = claim['session']
    claim = {**claim, 'session': kind.send(old, text)}  # R11: a Claude send resumes under a new id; the note names both
    move(item, item['state'], 'nudge', f'worker {claim["session"]}' + (f' replaces {old}' * (old != claim['session'])), claim=claim)
    item['claim'] = claim  # the nudge comment is now the last note: one send per answer
    return claim

def supervise(item, kinds):
    """§ 7 step 4, a supervised task whose supervisor runs here: the pass is its hands, never its judge."""
    n, boss, claim = item['iid'], item['supervisor'], item['claim'] or {}
    lead = kinds.get(boss['runtime'])
    if lead is None:
        return
    fresh = executable(BOARD.get(n))  # #532: the list may lag a spawn; #545: or an assignee-only label or reassignment
    if not fresh:
        return  # ineligible: sessions, claim and order stay as they are
    if item['raw'].get('order') and claim.get('runtime') in kinds and fresh['raw'].get('order') and not (fresh['claim'] or {}).get('session'):  # run or a rework requeue: a new worker on branch taskq-<N> (#291)
        kind = kinds[claim['runtime']]
        if not replace(item, kind, claim['runtime'], 'worker', running=True):
            return
        sid = spawn_named(item, kind, 'T', brief(item, claim['runtime']))
        item['claim'], item['raw']['order'] = {**claim, 'session': sid}, None
        move(item, 'doing', 'spawn', note('worker', sid, kind), claim=item['claim'], order=None)
    elif item['state'] == 'doing' and claim.get('session') and claim.get('runtime') in kinds:
        kind = kinds[claim['runtime']]
        live = kind.alive(claim['session'])
        if live is False:  # the supervisor decides: rework or ask
            item['claim'] = {**claim, 'session': None}
            move(item, 'doing', 'gone', f'worker {claim["session"]} is gone', claim=item['claim'])
        elif live:
            if follow(item, kind, claim, True) is None:
                return  # #576: no supervisor send, resume or respawn either
    state = lead_state(lead, boss['session'])
    issue = BOARD.get(n)
    if not executable(issue):
        return  # #545: a reassignment seen by this second read stops every supervisor send, resume and respawn
    if state == 'dead':
        evidence = tail_of(lead, boss['session']) or 'no log'
        if lead_deaths(issue['comments']):  # the second death since the last result or answer: the owner decides
            move(item, 'ask', 'ask', f'supervisor {boss["session"]} is gone again: fix the runtime, then answer.\n\nLast log line: {evidence}')
            return
        move(item, item['state'], 'gone', f'supervisor {boss["session"]} is gone: {evidence}')
        if getattr(lead, 'resumable', lambda _: False)(boss['session']):  # Codex: the same thread, the same id
            lead.send(boss['session'], f'restart #{n}: your last turn ended {evidence}; read your issue')
        else:  # a new supervisor adopts the live worker from the board; the dead one is retired first
            if not replace(item, lead, boss['runtime'], 'supervisor'):
                return
            sid = spawn_named(item, lead, 'S', supervisor_brief(item, boss['runtime'], lead))
            item['supervisor'] = {**boss, 'session': sid}
            move(item, item['state'], 'spawn', note('supervisor', sid, lead), supervisor=item['supervisor'])
    else:
        found, count = pending(issue, boss)
        if found and state == 'idle':  # running: its own wait (Claude) or the pass at its turn's end (Codex) delivers them; idle: its process ended: resume it with the events (#525: no sender, no timer)
            sid = lead.send(boss['session'], f'{" ".join(found)}: read your issue')
            seen(n, sid, count)
            if sid != boss['session']:  # Claude resumes under a new id (#284): record it; the old one is refused and retired
                item['supervisor'] = {**boss, 'session': sid}
                move(item, item['state'], 'nudge', f'supervisor {sid} replaces {boss["session"]}', supervisor=item['supervisor'])

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
        except OSError:  # mark the event before the holder can end, so its pending check sees it (#481)
            (folder / 'dispatch.pending').touch()
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
    """One pass; a pass that found the lock busy left .taskq/dispatch.pending, so run once more after ours (no lost event)."""
    pending = CONFIG['root'] / '.taskq' / 'dispatch.pending'
    held = one_pass(args, table)
    while held and pending.exists():  # only the lock holder reruns; a busy pass just leaves the mark
        pending.unlink(missing_ok=True)
        held = one_pass(args, False)

def direct():
    """R6 (#521): the tick's source client is the session running it. Codex: direct codex:// links; Claude, a shell or
    anything else: the https wrapper. ponytail: CODEX_THREAD_ID cannot tell the Codex app from the CLI or IDE."""
    return (session() or {}).get('runtime') == 'codex'

def stale(by_number):
    """R11: gone(runtime, n, session, live) for the pass: a session its task records, of a task not open, or replaced
    (no longer the claim or the supervisor). The open tasks' current sessions are answered without a board read."""
    issues = {}

    def gone(name, n, sid, _live):
        current = by_number.get(n)
        if current and sid in ((current['claim'] or {}).get('session'), (current['supervisor'] or {}).get('session')):
            return False
        if n not in issues:
            try:
                issues[n] = BOARD.get(n)
            except (Exception, SystemExit):
                issues[n] = None
        return bool(issues[n] and recorded(issues[n], name, sid))
    return gone

def row(item, kinds, here, waits=None):
    """R6 (#489): one markdown row, `[#N](issue)` and `[<session[:8]>](link)`; a session with no link here stays plain text.
    `waits`: why a ready task does not start (no manager on this machine, § 7 step 3)."""
    claim = item['claim'] if (item['claim'] or {}).get('session') else item.get('supervisor') or item['claim'] or {}  # no worker yet: its supervisor
    runtime, session = claim.get('runtime') or item['runtime'], claim.get('session') or ''
    url = session and claim.get('name') == here and runtime in kinds and kinds[runtime].link(session)
    if url and runtime == 'codex' and direct():  # #521: Codex opens its own thread link; the wrapper only loads a page first
        url = f'codex://threads/{session}'
    task = f'[#{item["iid"]}]({item["url"]})' if item.get('url') else f'#{item["iid"]}'
    cell = f'[{session[:8]}]({url})' if url else session and f'{session[:8]} on {claim.get("name")}'
    state = f'{item["state"]} ({waits})' if waits and item['state'] == 'ready' else item['state']
    return f'| {task} | {state} | {runtime} | {cell} |'

def one_pass(args, table=True):
    """One pass: free waiting tasks, follow unsupervised workers, act for supervisors, spawn ready tasks' supervisors, print the table."""
    here, kinds = machine(), runtimes()
    limits = CONFIG.get('limits') or {name: 1 for name in kinds}

    def lead(item):  # R3 (#532): the supervisor follows the task's own manager (DOT: Codex); None: no manager can start it here
        runtime = (item['pm'] or {}).get('runtime')
        runtime = {'dot': 'codex'}.get(runtime, runtime)
        return runtime if runtime in kinds else None
    with dispatch_lock() as held:  # #357 (R2): one pass at a time per checkout; the list is read under the lock
        tasks = getattr(args, 'tasks', None) or []  # #481: the list lags the event's own write (a new issue, a label): read those directly
        issues = [issue for issue in BOARD.list(None) if issue['iid'] not in tasks] + [issue for issue in map(BOARD.get, tasks) if issue['state'] == 'open']
        items = sorted(filter(None, map(parse, issues)), key=lambda item: (item['priority'], item['iid']))
        if not held:  # another pass runs here now: dispatch_lock marked the event, that pass runs once more when it ends
            print('taskq: another pass is running; it will run again for this event', file=sys.stderr)
        blind = bool(os.environ.get('CODEX_SANDBOX'))  # #502: a sandbox sees no other session alive: it would requeue live workers as gone
        if blind:
            print('taskq: inside a Codex sandbox: the pass only prints the table', file=sys.stderr)
        busy, ready = {}, items if held and not blind else []
        for item in ready:
            claim = item['claim'] or {}
            if item['state'] == 'waiting' and not open_deps(item['deps']):
                move(item, 'ready', 'ready', 'dependencies closed')
                item['state'] = 'ready'
            if item['supervisor']:  # step 4; the worker's slot is held from the supervisor's spawn to close
                if item['supervisor'].get('name') == here:
                    supervise(item, kinds)
                if item['state'] in ('doing', 'review', 'ask') and claim.get('name') == here:
                    busy[claim.get('runtime')] = busy.get(claim.get('runtime'), 0) + 1
                continue
            if item['state'] != 'doing' or claim.get('name') != here or claim.get('runtime') not in kinds:
                continue  # another machine's, or a session no runtime here can see
            if not executable(BOARD.get(item['iid'])):  # #545: the list's labels may lag; an ineligible task keeps its session and slot
                busy[claim['runtime']] = busy.get(claim['runtime'], 0) + 1
                continue
            runtime = kinds[claim['runtime']]  # step 2: unsupervised (R3 Transition: started before #525, or taken by hand)
            state = runtime.alive(claim['session'])
            if state is False:
                gone = f'session {claim["session"]} is gone'
                if quick_deaths(item['iid']):  # #393: the second death in a row with no result or answer asks, not respawns
                    move(item, 'ask', 'ask', f'{gone} again, the worker dies at once: fix the runtime, then answer.\n\n'
                                             f'Last log line: {tail_of(runtime, claim["session"]) or "none"}')
                    item['state'] = 'ask'
                    continue
                move(item, 'ready', 'requeue', gone, claim=None, result=None)
                item.update(state='ready', claim=None)
                continue
            if state:
                claim = follow(item, runtime, claim, False) or claim  # denied: the original claim keeps its slot
            busy[claim['runtime']] = busy.get(claim['runtime'], 0) + 1
        for item in ready:
            if item['state'] != 'ready' or item['host'] not in (None, here) or not mine(item) or open_deps(item['deps']) \
                    or not lead(item) or item['pm'].get('name') != here:
                continue  # no manager: the task waits, the table says so; another machine's manager: that machine starts it (§ 7 step 3)
            names = [item['runtime']] if item['runtime'] != 'any' else list(limits)
            free = next((name for name in names if name in kinds and busy.get(name, 0) < limits.get(name, 1)), None)
            fresh = free and executable(BOARD.get(item['iid']))  # #357: the board may have moved since the list; #545: eligibility
            if fresh and fresh['state'] == 'ready' and fresh['pm'] == item['pm']:  # the tick claims the slot for the worker; the supervisor orders the worker (`run`)
                item.update(fresh)
                runtime = lead(item)
                if not replace(item, kinds[runtime], runtime, 'supervisor'):  # a requeued task's old S<N>
                    continue
                session = spawn_named(item, kinds[runtime], 'S', supervisor_brief(item, runtime, kinds[runtime]))
                item.update(supervisor={'runtime': runtime, 'session': session, 'name': here}, claim={'runtime': free, 'session': None, 'name': here})
                move(item, 'doing', 'spawn', note('supervisor', session, kinds[runtime]), supervisor=item['supervisor'], claim=item['claim'],
                     result=None, order=None)
                item['state'], busy[free] = 'doing', busy.get(free, 0) + 1
        if held and not blind:  # R11 (#360, #525): stopped sessions the board no longer holds, by recorded id only
            retire(stale({item['iid']: item for item in items}), 'could not remove stopped sessions', running=False)
    if not table:
        return held
    print('| Task | State | Runtime | Session |\n|---|---|---|---|')
    for item in filter(mine, items):
        print(row(item, kinds, here, None if lead(item) else 'no manager'))
    host, repo = CONFIG.get('host'), CONFIG.get('repo')
    url = CONFIG.get('board_url') or {'github': f'https://{host or "github.com"}/{repo}/issues',
                                      'gitlab': f'https://{host or "gitlab.com"}/{repo}/-/issues'}.get(CONFIG['board'])
    url and print(f'Board: {url}')  # a board file names its page in `board_url`
    cards = decisions(filter(mine, items))
    cards and print('\n'.join(['', 'Decisions (answer: taskq answer N.K ...):', *cards]))
    return held

MEDIA = re.compile(r'\.(png|jpe?g|gif|webp|svg)(\?.*)?$', re.I)

def decisions(items):
    """#490: one line per task waiting on the owner: an ask, or a review with options. Images inline unless `inline_media` is false."""
    lines = []
    for item in items:
        card = item['raw'].get('decision') or {}
        if not (item['state'] == 'ask' or item['state'] == 'review' and card.get('options')):
            continue
        n, inline = item['iid'], CONFIG.get('inline_media', True)
        links = [f'![{n}]({link})' if inline and MEDIA.search(link) else link for link in card.get('links') or []]
        options = [f'{n}.{k} {text}' + ' (recommended)' * (k == card.get('recommend')) for k, text in enumerate(card.get('options') or [], 1)]
        head = f'[#{n}]({item["url"]})' if item.get('url') else f'#{n}'
        lines.append(' · '.join(filter(None, [f'{head} {item["state"]}: {card.get("summary") or item["title"]}', *links, *options])))
    return lines

EVENTS = ('add', 'answer', 'run', 'result', 'requeue', 'close')  # R4 (#333): each starts one pass after its move

def dispatch(command, tasks, after=None):
    """R4 (#405): the event pass runs in a detached `tick --quiet` child, its output in .taskq/dispatch.log; the event returns at once."""
    if os.environ.get('CODEX_SANDBOX'):  # a sandboxed Codex worker can neither start codex nor see other sessions' pids:
        return  # its pass would requeue live tasks as gone and spawn workers that die at once (#269 run 4b)
    try:
        (CONFIG['root'] / '.taskq').mkdir(exist_ok=True)
        detach = {'creationflags': 0x208} if os.name == 'nt' else {'start_new_session': True}
        tasks = [str(n) for n in (tasks if isinstance(tasks, list) else [tasks])]
        with open(CONFIG['root'] / '.taskq' / 'dispatch.log', 'ab') as out:
            out.write(f'{datetime.now():%Y-%m-%d %H:%M:%S} {command} {" ".join(f"#{n}" for n in tasks) or f"pid {after}"}\n'.encode())
            out.flush()  # the event, then the child's lines
            start_pass([sys.executable, str(Path(__file__).resolve()), 'tick', '--quiet', *['--after', str(after)] * bool(after), '--tasks', *tasks],
                       cwd=CONFIG['root'],
                       stdin=subprocess.DEVNULL, stdout=out, stderr=subprocess.STDOUT, **detach)
    except Exception as error:  # never fails the event: the next tick retries
        print(f'taskq: dispatch stopped: {error}; the next tick retries', file=sys.stderr)

start_pass = subprocess.Popen  # tests run the child's pass in process

def event_pass(args):
    """`tick --quiet`: the tick pass without the table. `--after PID`: first wait for that Codex turn to end (R4 #525).
    A failure never fails the event: the next tick retries."""
    while args.after and pid_alive(args.after):  # ponytail: 5 s poll of one pid; lives exactly as long as the turn
        time.sleep(5)
    try:
        cmd_tick(args, table=False)
    except (SystemExit, Exception) as error:
        print(f'taskq: dispatch stopped: {str(error).removeprefix("taskq: ")}; the next tick retries', file=sys.stderr)

def closed(n):
    """`closed #N <verdict>` for a supervised task that closed: the first line of its close comment, the supervisor's verdict (#567)."""
    issue = BOARD.get(int(n))
    found = BLOCK.search(issue.get('body') or '') if issue['state'] == 'closed' else None
    if not found or not json.loads(found.group(1)).get('supervisor'):
        return None
    text = next((text for text in reversed(issue.get('comments') or []) if text.startswith('**close**')), '')
    body = text.partition('\n\n')[2].strip()
    return f'closed #{n} {body.splitlines()[0]}' if body else f'closed #{n}'

def cmd_wait(args):
    """Block until the manager is needed: print 'ask #N', 'closed #N <verdict>' (supervised), 'review #N' and 'gone #N'
    (unsupervised worker, or a dead supervisor), or 'tick' after the window (#407, #525).
    .taskq/wait-<session>.json (a shell: wait.json) keeps the states last reported to this manager, so an event is printed once each."""
    if args.task:
        return wait_task(args)
    me, kinds, here = args.pm or (session() or {}).get('session'), runtimes(), machine()  # --pm: a sender waits as its manager (R4)
    path = CONFIG['root'] / '.taskq' / (f'wait-{me}.json' if me else 'wait.json')  # #532: each manager's events, once each
    ours = lambda item: not me or (item['pm'] or {}).get('session') in (None, me)  # its own tasks and those with no manager
    seen = json.loads(path.read_text('utf-8')) if path.is_file() else {}
    end = time.time() + args.window * 60
    while True:
        now, blind = {}, bool(os.environ.get('CODEX_SANDBOX'))
        for item in filter(ours, filter(mine, filter(None, map(parse, BOARD.list(None))))):
            claim, boss, state = item['claim'] or {}, item['supervisor'] or {}, item['state']
            if boss:  # a supervised review is its supervisor's (#524); only a dead supervisor is the manager's
                state = 'gone' if not blind and boss.get('name') == here and boss.get('runtime') in kinds \
                    and lead_state(kinds[boss['runtime']], boss['session']) == 'dead' else 'review*' if state == 'review' else state
            elif state == 'doing' and not blind and claim.get('name') == here and claim.get('runtime') in kinds \
                    and kinds[claim['runtime']].alive(claim['session']) is False:  # ponytail: one alive call per local worker per poll
                state = 'gone'
            now[str(item['iid'])] = state
        events = [f'{state} #{n}' for n, state in now.items() if state in ('review', 'ask', 'gone') and seen.get(n) != state]
        events += filter(None, (closed(n) for n in seen if n.isdigit() and n not in now))  # left the open list: closed?
        if events or time.time() >= end:
            path.parent.mkdir(exist_ok=True)
            path.write_text(json.dumps(now), 'utf-8')
            print('\n'.join(events) or 'tick')
            return
        seen = now  # a task that leaves review and comes back while we wait is a new event
        time.sleep(args.every)

def wait_task(args):
    """The supervisor's wait (§ 7 Supervisor): block until its task needs it, print its events (`pending`), `stop #N` when the
    task is closed or no longer this session's, or `tick` after the window. Events are marked seen once printed."""
    n, me, end = args.task, (session() or {}).get('session'), time.time() + args.window * 60
    while True:
        issue = BOARD.get(n)
        current = parse(issue) if issue['state'] == 'open' else None
        boss = (current or {}).get('supervisor') or {}
        if not me or boss.get('session') != me:
            return print(f'stop #{n}')
        found, count = pending(issue, boss)
        if found or time.time() >= end:
            found and seen(n, me, count)
            return print('\n'.join(found) or 'tick')
        time.sleep(args.every)

SENDERS = {'claude': 'SendMessage'}
CLONE = Path(__file__).resolve().parent  # the taskq clone: its taskq.md is the manager contract (#430)

def contract():
    """The short hash of the clone's taskq.md, None without one."""
    path = CLONE / 'taskq.md'
    return hashlib.sha256(path.read_bytes()).hexdigest()[:12] if path.is_file() else None

def refresh():
    """#430, before tick and wait: `git pull --ff-only` a clean clone, then tell a manager whose contract is stale to re-read it."""
    if (CLONE / '.git').exists():
        git = [shutil.which('git') or 'git', '-C', str(CLONE)]
        status = subprocess.run([*git, 'status', '--porcelain', '--untracked-files=no'], capture_output=True, text=True, encoding='utf-8')
        if not status.returncode and not status.stdout.strip():
            # fetch into origin/main, then fast-forward from that ref: FETCH_HEAD is shared with concurrent fetches of task branches
            pulled = subprocess.run([*git, 'fetch', '-q', 'origin'], capture_output=True, text=True, encoding='utf-8')
            if not pulled.returncode:  # the clone's own upstream, whatever its default branch is called
                pulled = subprocess.run([*git, 'merge', '--ff-only', '-q', '@{upstream}'], capture_output=True, text=True, encoding='utf-8')
            if pulled.returncode:
                print(f'taskq: git pull --ff-only failed: {last_line(pulled.stderr + pulled.stdout)}', file=sys.stderr)
    path = CONFIG['root'] / '.taskq' / 'pm.json'
    known = json.loads(path.read_text('utf-8')).get('contract') if path.is_file() else None
    if contract() and known and known != contract():  # no pm.json: this session never took the role
        print('The manager contract changed: run taskq pm and follow it from now on.')

def adopt(numbers, me):
    """R3 Transition (#532): record this session as the `pm` of tasks with none. The dispatch lock and a read under it keep
    two adoptions in one checkout from overwriting each other; across checkouts nothing is atomic (R4)."""
    if not origin():
        fail('cannot adopt: a plain shell needs TASKQ_RUNTIME')
    with dispatch_lock() as locked:
        if not locked:
            fail('cannot adopt: another taskq process holds .taskq/dispatch.lock; nothing adopted, run it again')
        adopted = [task(n) for n in numbers]
        held = [f'#{item["iid"]} ({item["pm"]["runtime"]}:{(item["pm"]["session"] or "shell")[:8]})' for item in adopted if item['pm']]
        if held:  # explicit, never the last writer
            fail(f'cannot adopt: {" ".join(held)} already has a manager')
        for item in adopted:
            reason = not item['claim'] and not item['supervisor'] and execution_reason(item)
            if reason:  # #545: an unstarted task; recorded sessions only change their PM routing
                fail(f'cannot adopt: {reason}')
        for item in adopted:
            move(item, item['state'], 'adopt', f'pm {origin()["runtime"]}:{me.get("session") or "shell"}', pm=origin())

def cmd_pm(args):
    """The manager role: Principles and § 7 of taskq.md, then how to tick this session; the hash goes to .taskq/pm.json,
    nothing else: a task's manager is its own `pm` on the board (R3, #532). `--adopt N`: become the `pm` of tasks with none."""
    me = session() or {}
    if me and any(role(item) in ('supervisor', 'worker') for item in map(parse, BOARD.list(None)) if item):
        fail('a recorded supervisor or worker cannot take the manager role (R3 one controller)')
    if args.adopt:
        adopt(args.adopt, me)
    text, digest = (CLONE / 'taskq.md').read_text('utf-8'), contract()
    sections = re.findall(r'^## (?:Principles|7\. Manager)\b.*?(?=^## )', text, re.M | re.S)
    (CONFIG['root'] / '.taskq').mkdir(exist_ok=True)
    (CONFIG['root'] / '.taskq' / 'pm.json').write_text(json.dumps({'contract': digest}), 'utf-8')
    print(f'taskq pm contract {digest}\nYou are the taskq manager of {CONFIG["root"]}. Follow this role from now on; '
          f'`taskq` is `python3 {Path(__file__).resolve()}`.\n\n' + '\n'.join(sections))
    cmd_arm(argparse.Namespace(target=None))

CODEX_COMPACT = ('-c model_auto_compact_token_limit=200000 -c "compact_prompt=\\"Keep only the owner\'s open questions and '
                 'decisions; the board is the state.\\""')  # #507: a compacted manager costs ~10x less per tick (#503)

def rollout(thread):
    """#522: where the thread's local Codex rollout is: 'local' (exec resume finds it), 'archived', or None (unknown)."""
    home = Path(os.environ.get('CODEX_HOME') or Path.home() / '.codex')
    name = f'rollout-*-{glob.escape(thread)}.jsonl'
    return next((kind for kind, pattern in (('local', f'sessions/*/*/*/{name}'), ('archived', f'archived_sessions/{name}'))
                 if next(home.glob(pattern), None)), None)

def cmd_arm(args):
    """The prompt for a tick-sender session of this runtime: wait, send the output to the manager, repeat (#407)."""
    runtime = (session() or {}).get('runtime') or os.environ.get('TASKQ_RUNTIME')
    wait = f'python3 {Path(__file__).resolve()} wait'
    start = ('Start: run one pass now (`taskq tick`, outside a Codex sandbox). From then on the approved queue runs by itself (R4): '
             'supervisors, workers, reviews, reworks, closes and the next task start on queue events and Codex turn ends; '
             'no sender, timer or extension. Arming below only brings you the short outcomes.\n')
    if not args.target and runtime == 'codex':  # #510: a Codex session is not woken when a background command ends
        thread = os.environ.get('CODEX_THREAD_ID') or '<this thread>'
        return print(f'''{start}Arm the tick in this session. Codex is not woken when a background command ends, so tick in the foreground:
loop {{ run `{wait}`; on its output (`ask #N`, `closed #N <verdict>`, `review #N`, `gone #N` or `tick`) run one pass (`taskq tick`) and do
§ 7 After each pass for those tasks }}. Between turns the outcomes wait on the board for your next pass; the queue does not.
Optional, only to be woken between turns: `python3 {Path(__file__).resolve()} arm tick {thread}` prints a sender prompt for a
thread with a local rollout only; no wake of a Codex app thread is promised (#522).
Codex manager: start it with `codex {CODEX_COMPACT}`.''')
    if not args.target:  # no target: this session ticks itself (Claude: a background command wakes the session on exit)
        return print(f'''{start}Arm the tick in this session. Run `{wait}` as a background command (Claude Code: run_in_background).
When it ends you are woken with its output (`ask #N`, `closed #N <verdict>`, `review #N`, `gone #N` or `tick`): run one pass (`taskq tick`),
do § 7 After each pass for those tasks, then start `{wait}` in the background again. Keep exactly one wait running.
Codex manager: start it with `codex {CODEX_COMPACT}` (Claude: .claude/settings.json autoCompactWindow 200000).''')
    resume = f'codex exec {shlex.join(codex_options())} resume {args.target}'  # the options a worker turn gets
    pm = re.split(r'session_|threads/|/', args.target)[-1]  # #532: the sender consumes as its manager, the id the board records
    wait = f'{wait} --pm {shlex.quote(pm)}'
    send, shell, note = f'with {SENDERS.get(runtime, "your messaging tool")}', '', ''
    where = rollout(args.target) if runtime == 'codex' else None
    if where == 'local':  # a CLI thread: exec resume finds it
        send = f'by running `{resume} "<its output>"`: a new turn on that thread wakes it'
        shell = (f'\nNo agent needed: `cd {CONFIG["root"]} && while e=$({wait}) && {resume} "$e"; do :; done; '
                 'echo "taskq sender stopped"` in a terminal.')
    elif where == 'archived':  # #522: exec resume of an archived thread is unverified (R12): no route
        return print(f'taskq: {args.target} is archived in Codex; `exec resume` of an archived thread is unverified, so no sender.\n'
                     f'Run `codex unarchive {args.target}`, then `taskq arm tick {args.target}` again.')
    elif runtime == 'codex':  # #522: unknown target; an app thread fails `no rollout found` (#269)
        send = 'with `send_message_to_thread`'
        note = (f'taskq: no local Codex rollout of {args.target}, so no `codex exec resume` and no promised wake. Unknown what it is: '
                'a Codex app thread (exec resume fails `no rollout found`), a thread name, a typo or another machine\'s thread.\n'
                f'Only if {args.target} is a known Codex app thread: run this prompt in an independent, user-visible Codex app session '
                'whose send_message_to_thread reaches it. Not a collaboration subagent of the manager: it cannot send to its ancestor '
                'and starts no turn. A session without that tool (a CLI worker) hands this prompt to the owner or the app manager.\n'
                'Workers still dispatch without a sender (R4 event chain); only review, ask and gone wait for the manager.\n\n')
    if not any((item['pm'] or {}).get('session') == pm for item in map(parse, BOARD.list(None)) if item):
        note += f'taskq: no open task records {pm} as its pm; this wait shows only tasks with no manager until one does.\n'
    print(f'''{note}You are the taskq tick sender for the manager session {args.target}. Do no task work and run no other taskq command.
Stay in this one turn and repeat, from {CONFIG["root"]}; do not end the turn between events (an ended turn forwards nothing):
1. Run `{wait}`. It blocks until the manager is needed (at most 10 minutes) and prints one line per event.
2. Send its output, verbatim, to {args.target} {send}.
3. Go back to 1 at once. A failed wait, a failed send or no such send tool: stop, say here
   `taskq sender stopped: <error>` once; never retry, never another route.{shell}''')

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
    card = ((('--option',), {'action': 'append', 'default': []}), (('--recommend',), {'type': int, 'default': 1}),
            (('--link',), {'action': 'append', 'default': []}))  # #490: the decision card
    command('ask', cmd_move, *card, text='required')
    command('answer', cmd_answer, (('n',), {'nargs': '+'}), n=False, text=True)
    command('result', cmd_move, (('--sha',), {'required': True, 'type': commit}), (('--checks',), {'default': ''}), *card, text=True)
    command('requeue', cmd_requeue, text=True)
    command('run', cmd_run, text=True)
    command('later', cmd_move, text=True)
    command('close', cmd_close, (('n',), {'nargs': '+', 'type': int}), n=False, text=True)
    command('tick', lambda args: (event_pass if args.quiet else cmd_tick)(args), (('--quiet',), {'action': 'store_true'}),
            (('--tasks',), {'nargs': '*', 'type': int, 'default': []}), (('--after',), {'type': int}), n=False)
    command('wait', cmd_wait, (('--window',), {'type': float, 'default': 10}), (('--every',), {'type': float, 'default': 25}),
            (('--task',), {'type': int}), (('--pm',), {}), n=False)
    command('arm', cmd_arm, (('what',), {'choices': ('tick',)}), (('target',), {'nargs': '?'}), n=False)
    command('pm', cmd_pm, (('--adopt',), {'nargs': '+', 'type': int, 'default': []}), n=False)
    command('cleanup', cmd_cleanup, (('--dry-run',), {'action': 'store_true'}), n=False)
    args = parser.parse_args(argv)
    if BOARD is None:
        CONFIG = load_config()
        BOARD = make_board(CONFIG)
    if args.command == 'wait' and not args.task or args.command == 'tick' and not args.quiet:
        refresh()
    done = args.function(args)
    if args.command in EVENTS:
        dispatch(args.command, done if args.command in ('add', 'answer') else args.n)

if __name__ == '__main__':
    main()
