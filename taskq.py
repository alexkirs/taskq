#!/usr/bin/env python3
"""taskq: a task queue on an issue board. One file, stdlib only, python3 >= 3.9. Design: docs/single-file.md."""
import argparse, contextlib, glob, hashlib, importlib.util, json, os, re, shlex, shutil, signal, socket, subprocess, sys, tempfile, threading, time, uuid
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import quote

# --- config + task model --------------------------------------------------------------------

STATES = ('ready', 'waiting', 'doing', 'review', 'ask', 'later')
TYPES = ('code', 'docs', 'research', 'asset')
FIELDS = ('scope', 'deps', 'claim', 'result', 'supervisor', 'order', 'pm')
PREFIX, RUN, ON = 'q-', 'run-', 'host-'
BLOCK = re.compile(r'<!-- taskq:start -->\s*```json\n(.*?)\n```\s*<!-- taskq:end -->', re.S)
SESSIONS = {'claude': 'CLAUDE_CODE_SESSION_ID', 'codex': 'CODEX_THREAD_ID', 'hermes': 'HERMES_SESSION_ID'}
CONFIG, BOARD = {}, None  # set by main, or by a test
GUARD, GUARD_WAIT, GUARD_PAUSE = threading.local(), 30, 2
EVENT_SCHEMA, EVENT_LIMIT = 3, 64
EVENT_LABEL = 'taskq-events'
PAYLOAD_ACTIONS = ('ask', 'answer', 'requeue', 'result')

class SettledError(SystemExit):
    """A fully known refusal; tasks lists only acknowledged transitions that need an event."""
    def __init__(self, message, tasks):
        super().__init__(f'taskq: {message}')
        self.tasks = tasks

def fail(message):
    held = getattr(GUARD, "held", None)
    if held and held["effects"]:
        held["poisoned"] = True
    sys.exit(f'taskq: {message}')

def no_window():
    return {'creationflags': 0x08000000} if os.name == 'nt' else {}  # CREATE_NO_WINDOW


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

def local_limits():
    """#604: explicit inherited limits replace, never merge with, shared defaults."""
    if 'TASKQ_LIMITS' not in os.environ:
        return None
    try:
        limits = json.loads(os.environ['TASKQ_LIMITS'])
    except ValueError:
        limits = None
    if not isinstance(limits, dict) or not limits or any(not re.fullmatch(r'[a-z][a-z0-9_-]*', name)
            or type(value) is not int or value < 0 for name, value in limits.items()):
        fail('TASKQ_LIMITS needs a nonempty JSON object of runtime names to nonnegative integers')
    return limits

def host_scope(item):
    """#604: explicit host only; conflicting labels fail closed, including fresh board reads."""
    wanted = os.environ.get('TASKQ_HOST_ONLY')
    return wanted is None or [label for label in item['labels'] if label.startswith(ON)] == [ON + wanted]

ORCH = {'claude': 'CLD', 'codex': 'CDX', 'dot': 'DOT', 'hermes': 'HRM', 'grok': 'GRK'}

def worker_name(item, letter='T'):
    """R3 naming (#268, #572): `T<N> <ORCH> <title> (<machine>)`, `S<N> ...` the supervisor; ORCH is the task's `pm` on the
    board, never the caller that runs the pass; a task with no `pm` (R3 Transition): the caller, UNK for the owner's shell."""
    launcher = (item.get('pm') or {}).get('runtime') or (session() or {}).get('runtime') or os.environ.get('TASKQ_RUNTIME')
    return f'{letter}{item["iid"]} {ORCH.get(launcher, "UNK")} {item["title"][:40]} ({machine()})'

def session():
    """This agent session, or None for the owner's shell. TASKQ_RUNTIME picks one when a session inherited another's id."""
    selected = os.environ.get('TASKQ_RUNTIME')
    if (selected == 'hermes' or not selected and 'HERMES_SESSION_ID' in os.environ) and not os.environ.get('HERMES_SESSION_ID', '').strip():
        fail('Hermes needs genuine HERMES_SESSION_ID from the native runtime/bridge')
    if not selected and os.environ.get('HERMES_SESSION_ID') and any(os.environ.get(SESSIONS[r]) for r in ('claude', 'codex')):
        fail('ambiguous Hermes identity: select TASKQ_RUNTIME explicitly')
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
    raw = issue_data(issue)
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

def compatibility_reason(item):
    stale = release_reason()
    if stale:
        return stale
    version = item['raw'].get('event_schema', 0)
    if type(version) is not int or version not in (0, 1, EVENT_SCHEMA):
        return f'#{item["iid"]}: unsupported event_schema {version!r}; writes refused'
    if version != EVENT_SCHEMA:
        return f'#{item["iid"]}: legacy event_schema {version} is read-only; review taskq repair before starting work'
    if 'legacy_recovery' in item['raw']:
        return f'#{item["iid"]}: legacy recovery hold; no mutation, resume, replacement or acknowledgement'
    return None


def execution_reason(item):
    """#545: an `assignee-only` task runs only for the board's authenticated user among its native assignees. None: allowed."""
    if compatibility_reason(item):
        return compatibility_reason(item)
    return execution_policy_reason(item)

def execution_policy_reason(item):
    """Fresh board eligibility shared by legacy and explicitly opt-in schema2 handlers."""
    if not host_scope(item):
        return f'#{item["iid"]}: outside TASKQ_HOST_ONLY={os.environ["TASKQ_HOST_ONLY"]}; execution refused'
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

def schema2_execution_reason(item):
    if not mine(item):
        return f'#{item["iid"]}: outside configured assignee selection; execution refused'
    return release_reason() or execution_policy_reason(item)

def executable(issue, expected=None):
    """#545: a fresh board issue the pass may act on: a task, and eligible; else the reason goes to stderr."""
    item = parse(issue)
    reason = execution_reason(item) if item else f'#{issue["iid"]} is no longer a task'
    if not reason and expected and \
            (any(item[key] != expected[key] for key in ('state', 'pm', 'claim', 'supervisor'))
             or item['raw'].get('order') != expected['raw'].get('order')):
        reason = f'#{item["iid"]}: execution changed since this pass read it; leaving it untouched'
    reason and print(f'taskq: {reason}', file=sys.stderr)
    return item if not reason else None

def block(text, fields):
    """The description: the task's text, then its JSON block. Keys the model does not know are kept as they are."""
    return f'{text}\n\n<!-- taskq:start -->\n```json\n{json.dumps(fields, indent=1, ensure_ascii=False)}\n```\n<!-- taskq:end -->'


def issue_data(issue):
    found = BLOCK.search(issue.get('body') or '')
    raw = json.loads(found.group(1)) if found else {}
    version = raw.get('event_schema', 0)
    if type(version) is not int or version not in (0, 1, EVENT_SCHEMA, 2):
        fail(f'unsupported event_schema {version!r}; update TaskQ before using this board')
    return raw


def recipient(role, identity):
    identity = identity or {}
    return f'{role}:{identity.get("runtime", "owner")}:{identity.get("session") or "owner"}'


def event_targets(raw, action, text, actor=None):
    targets, actor = [], actor or session() or {}
    boss, worker = raw.get('supervisor') or {}, raw.get('claim') or {}
    manager = recipient('manager', raw['pm']) if raw.get('pm') else 'manager:*'
    if action in ('ask', 'close') or action == 'result' and not boss or action == 'gone' and (not boss or text.startswith('supervisor ')):
        targets.append(manager)
    if boss.get('session') and action in ('result', 'ask', 'answer', 'gone', 'requeue') \
            and not (action in ('ask', 'requeue') and actor.get('session') == boss['session']) \
            and (action != 'gone' or text.startswith('worker ')):
        targets.append(recipient('supervisor', boss))
    if action == 'answer' and worker.get('session'):
        targets.append(recipient('worker', worker))
    return targets


def event_pending(raw, target):
    return [event for event in raw.get('events', []) if not set(event['recipients']).issubset(event.get('acks', []))
            and target not in event.get('acks', [])
            and (target in event['recipients'] or target.startswith('manager:')
                 and 'manager:*' in event['recipients'] and 'manager:*' not in event.get('acks', []))]


def event_labels(raw, labels):
    pending_manager = any(target.startswith('manager:') and target not in event.get('acks', [])
                          for event in raw.get('events', []) for target in event['recipients'])
    return [label for label in labels if label != EVENT_LABEL] + ([EVENT_LABEL] if pending_manager else [])


def reconcile_recipients(raw):
    """An explicit controller replacement ends its old recipient's pending work; adoption resolves wildcard PM."""
    for event in raw.get('events', []):
        for target in list(event['recipients']):
            role_name = target.split(':')[0]
            if target == 'manager:*' and raw.get('pm'):
                event['recipients'] = [recipient('manager', raw['pm']) if value == target else value for value in event['recipients']]
            elif role_name in ('worker', 'supervisor') and target != recipient(role_name, raw.get('claim' if role_name == 'worker' else 'supervisor')):
                event['acks'] = list(dict.fromkeys([*event.get('acks', []), target]))
                event['cancelled'] = list(dict.fromkeys([*event.get('cancelled', []), target]))


def append_event(raw, action, text, targets=None):
    events = [dict(event) for event in raw.get('events', [])
              if not set(event['recipients']).issubset(event.get('acks', []))]
    targets = event_targets(raw, action, text) if targets is None else targets
    if targets and len(events) >= EVENT_LIMIT:
        fail(f'pending event capacity ({EVENT_LIMIT}) reached; deliver/ack outstanding events first')
    number = raw.get('event_seq', 0) + 1
    event = {'id': number, 'action': action, 'text': text, 'by': who(), 'recipients': targets, 'acks': []}
    if action in PAYLOAD_ACTIONS:
        raw['action_payloads'] = {**raw.get('action_payloads', {}), action: {key: event[key] for key in ('id', 'by', 'text')}}
    if targets:
        events.append(event)
    raw.update(event_schema=EVENT_SCHEMA, event_seq=number, action=event, events=events)
    return event


def initialize_events(issue):
    """Explicit migration helper: preserve current outstanding signals, not every historical outcome."""
    raw = dict(issue_data(issue))
    if raw.get('event_schema') == EVENT_SCHEMA:
        return raw
    if raw.get('event_schema') == 1:
        return {**raw, 'event_schema': EVENT_SCHEMA}
    comments = issue.get('comments') or []
    payloads = dict(raw.get('action_payloads') or {})
    for index, note_text in enumerate(comments):
        head, _, text = note_text.partition('\n\n')
        action, _, by = head.partition(' · ')
        if action.strip('*') in PAYLOAD_ACTIONS:
            payloads[action.strip('*')] = {'id': f'legacy:{index + 1}', 'by': by, 'text': text}
    raw['action_payloads'] = payloads
    deaths = 0
    for note_text in reversed(comments):
        if note_text.startswith(('**result**', '**answer**')):
            break
        deaths += note_text.startswith('**requeue**') and ' is gone' in note_text
    raw.update(event_schema=EVENT_SCHEMA, event_seq=0, events=[], retry_counts={
        'workers': workers(issue['iid'], comments), 'supervisor': lead_deaths(comments),
        'worker': deaths})
    state = next((label[len(PREFIX):] for label in issue['labels'] if label.startswith(PREFIX)), '')
    action = 'close' if issue['state'] == 'closed' else {'review': 'result', 'ask': 'ask'}.get(state)
    if action:
        text = next((note.partition('\n\n')[2] for note in reversed(comments) if note.startswith(f'**{action}**')), '')
        targets = [target for target in event_targets(raw, action, text) if target.startswith('manager:')]
        append_event(raw, action, text, targets)
    elif state == 'doing':
        claim = raw.get('claim') or {}
        if claim.get('session'):
            for action_name, text, _ in legacy_worker_messages(issue, claim, bool(raw.get('supervisor'))):
                if action_name == 'answer':
                    append_event(raw, 'answer', text, [recipient('worker', claim)])
    boss = raw.get('supervisor') or {}
    if boss.get('session') and issue['state'] == 'open':
        for action_name, text in legacy_supervisor_events(issue, boss):
            append_event(raw, action_name, text, [recipient('supervisor', boss)])
    raw['legacy_comments'] = len(comments)
    claim = raw.get('claim') or {}
    if claim.get('session'):
        signal = legacy_worker_message(issue, claim, bool(boss))
        raw['worker_comment_cursor'] = {claim['session']: signal[2] - 1 if signal else len(comments) - 1}
    return raw


def diagnostic_comment(n, text):
    """Only log comments: run_api's fail() must not poison an otherwise settled authoritative action."""
    held = getattr(GUARD, 'held', None)
    poisoned = held and held['poisoned']
    try:
        BOARD.comment(n, text)
    except (Exception, SystemExit) as error:
        if held:
            held['poisoned'] = poisoned
        print(f'taskq: #{n} action saved; diagnostic comment failed: {error}', file=sys.stderr)


# --- board ----------------------------------------------------------------------------------
# Six functions: list(state), get(n), add(title, body, labels), update(n, labels=None, body=None), comment(n, text),
# close(n). An issue is {iid, title, body, labels, state: open|closed, updated_at, url, assignees} and from get also comments.
# Optional seventh: user() -> the login "assignee": "me" stands for.

def run_api(tool, host, method, path, body=None):
    command = [shutil.which(tool) or fail(f'{tool} not found'), 'api', '-X', method, path]
    command += ['--hostname', host] * bool(host) + ['--input', '-', '-H', 'Content-Type: application/json'] * (body is not None)
    done = subprocess.run(command, input=body and json.dumps(body), capture_output=True, **no_window(), text=True, encoding='utf-8')
    if done.returncode:
        fail(f'{tool} api {method} {path}: {done.stderr.strip() or done.stdout.strip()}')
    return json.loads(done.stdout) if done.stdout.strip() else None

def coordination_api(tool, host, method, path, body=None):
    """Structured status only for coordination: API errors are never inferred from CLI prose."""
    command = [shutil.which(tool) or fail(f'{tool} not found'), 'api', '--include', '-X', method, path]
    command += ['--hostname', host] * bool(host)
    if body is not None:
        command += ['--input', '-', '-H', 'Content-Type: application/json']
    done = subprocess.run(command, input=json.dumps(body) if body is not None else None,
                          capture_output=True, **no_window(), text=True, encoding='utf-8', timeout=30)
    header, separator, payload = done.stdout.partition('\n\n')
    status = re.match(r'HTTP/\S+\s+(\d+)', header)
    if not separator or not status:
        raise RuntimeError(f'{tool} coordination response unknown; inspect the board before retrying')
    try:
        data = json.loads(payload) if payload.strip() else None
    except ValueError as error:
        raise RuntimeError(f'{tool} coordination response is not JSON') from error
    code = int(status[1])
    if done.returncode and 200 <= code < 300:
        raise RuntimeError(f'{tool} coordination CLI failed after HTTP {code}; outcome unknown')
    return code, data


def effect(function, *args, **kwargs):
    """An exception after effects begin poisons this grant, even when a caller catches it."""
    reason = release_reason()
    if reason:
        fail(reason)
    held = getattr(GUARD, 'held', None)
    if held:
        if held['poisoned']:
            fail('project guard poisoned by an earlier effect failure; further effects refused')
        held['effects'] = True
    try:
        return function(*args, **kwargs)
    except BaseException:
        if held:
            held['poisoned'] = True
        raise


@contextlib.contextmanager
def coordination():
    """Board-scoped exclusion; only a same-thread nested operation can share an acknowledged grant."""
    held = getattr(GUARD, 'held', None)
    identity = origin()
    if held:
        if held['board'] is not BOARD or held['pid'] != os.getpid() or held['identity'] != identity:
            fail('another operation holds the project guard; ownership cannot be inherited')
        try:
            yield
        except BaseException as error:
            held['poisoned'] |= held['effects'] and not isinstance(error, SettledError)
            raise
        return
    if not all(callable(getattr(BOARD, method, None)) for method in ('acquire', 'release')):
        fail('board adapter has no atomic acquire/release; writes refused, read-only commands remain available')
    owner = f'{machine()} {who()} pid={os.getpid()} {uuid.uuid4()}'
    deadline = time.monotonic() + GUARD_WAIT
    while True:
        try:
            token = BOARD.acquire(owner)
        except BaseException as error:
            fail(f'project guard acquisition failed ({error}); owner {owner}; inspect the board, no grant assumed')
        if token is not None:
            if not token:
                fail('board adapter returned an empty guard token; outcome unknown, inspect the board')
            break
        if time.monotonic() >= deadline:
            fail('project guard busy; no operation performed; inspect the holder or retry after it finishes')
        time.sleep(min(GUARD_PAUSE, max(0, deadline - time.monotonic())))
    held = {'board': BOARD, 'pid': os.getpid(), 'identity': identity, 'token': token, 'effects': False, 'poisoned': False}
    GUARD.held = held
    try:
        try:
            yield
        except BaseException as error:
            held['poisoned'] |= held['effects'] and not isinstance(error, SettledError)
            raise
    finally:
        GUARD.held = None
        if held['poisoned']:
            print(f'taskq: project guard retained after partial/unknown effects; owner {owner}; '
                  f'exact token {json.dumps(token)}; stop/drain controllers and reconcile before explicit recovery', file=sys.stderr)
            if sys.exc_info()[0] is None or isinstance(sys.exc_info()[1], SettledError):
                fail('operation had caught effect failures; project guard retained')
        else:
            try:
                BOARD.release(token)
            except BaseException as error:
                fail(f'project guard release failed ({error}); exact token {json.dumps(token)}; '
                     'inspect the board after quiescence; no automatic retry')


class GitHub:
    coordination_label = 'taskq-coordination'  # fixtures may isolate the same adapter protocol on a disposable name
    def __init__(self, repo, host=None, options=None):
        self.repo, self.host, self.options = repo, host, options or {}

    def api(self, method, path, body=None):
        return run_api('gh', self.host, method, f'repos/{self.repo}'+(f'/{path}' if path else ''), body)

    def capacity_provider(self, caps):
        return BoardCapacity(self, caps)

    def observe_guard(self):
        code, data = coordination_api('gh', self.host, 'GET', f'repos/{self.repo}/labels/{quote(self.coordination_label, safe="")}')
        if code == 404:
            return {'state': 'absent'}
        if code == 200 and isinstance(data, dict) and data.get('name') == self.coordination_label and data.get('node_id'):
            return {'state': 'held', 'identity': str(data['node_id']), 'owner': data.get('description') or ''}
        return {'state': 'unknown'}

    def acquire(self, owner):
        code, data = coordination_api('gh', self.host, 'GET', f'repos/{self.repo}/labels/{quote(self.coordination_label, safe="")}')
        if code == 200 and isinstance(data, dict) and data.get('name') == self.coordination_label and data.get('node_id'):
            return None  # presence is only a contention hint, never proof of our grant
        if code != 404:
            raise RuntimeError(f'GitHub guard lookup failed (HTTP {code})')
        code, data = coordination_api('gh', self.host, 'POST', f'repos/{self.repo}/labels',
                                      {'name': self.coordination_label, 'color': '666666', 'description': owner[:100]})
        if code == 422 and isinstance(data, dict) and any(error.get('resource') == 'Label'
                and error.get('field') == 'name' and error.get('code') == 'already_exists'
                for error in data.get('errors', []) if isinstance(error, dict)):
            return None
        if code != 201 or not isinstance(data, dict) or data.get('name') != self.coordination_label or not isinstance(data.get('node_id'), str) or not data['node_id']:
            raise RuntimeError(f'GitHub guard creation failed/unknown (HTTP {code}); inspect taskq-coordination')
        return data['node_id']

    def release(self, token):
        code, data = coordination_api('gh', self.host, 'POST', 'graphql',
            {'query': 'mutation($id: ID!) { deleteLabel(input: {id: $id}) { clientMutationId } }', 'variables': {'id': token}})
        if code != 200 or not isinstance(data, dict) or data.get('errors') or \
                not isinstance(data.get('data'), dict) or 'deleteLabel' not in data['data'] or data['data']['deleteLabel'] is None:
            raise RuntimeError(f'GitHub exact guard release failed/unknown (HTTP {code})')

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
        return [self.issue(item) for item in self.pages(query) if 'pull_request' not in item]  # None: every open issue (§ 2)

    def closed(self):
        return [self.issue(item) for item in self.pages(f'issues?state=closed&labels={EVENT_LABEL}') if 'pull_request' not in item]

    def get(self, n):
        return {**self.metadata(n), 'comments': self.comments(n)}

    def metadata(self, n):
        return self.issue(self.api('GET', f'issues/{n}'))

    def comments(self, n):
        return [item['body'] for item in self.pages(f'issues/{n}/comments')]

    def ensure_event_label(self):
        """Explicit migration/setup only; no auto-provision in ordinary event mutations."""
        gitlab = isinstance(self, GitLab)
        tool = 'glab' if gitlab else 'gh'
        base = f'projects/{quote(self.repo, safe="")}' if gitlab else f'repos/{self.repo}'
        code, data = coordination_api(tool, self.host, 'GET', f'{base}/labels/taskq-events')
        if code == 200 and isinstance(data, dict) and data.get('name') == 'taskq-events':
            return
        if code != 404:
            fail(f'taskq-events label lookup failed/unknown (HTTP {code})')
        code, data = effect(coordination_api, tool, self.host, 'POST', f'{base}/labels',
                            {'name': 'taskq-events', 'color': '#666666' if gitlab else '666666',
                             'description': 'TaskQ pending manager events'})
        if code != 201 or not isinstance(data, dict) or data.get('name') != 'taskq-events':
            fail(f'taskq-events label creation failed/unknown (HTTP {code}); reconcile before retry')

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
        return run_api('glab', self.host, method, f'projects/{quote(self.repo, safe="")}'+(f'/{path}' if path else ''), body)

    def coordination_path(self):
        board, label = (self.options.get(key) for key in ('coordination_board', 'coordination_label'))
        if any(type(value) is not int or value <= 0 for value in (board, label)):
            raise RuntimeError('GitLab writes require explicitly provisioned board_options coordination_board/coordination_label IDs')
        return f'projects/{quote(self.repo, safe="")}/boards/{board}/lists', label

    def observe_guard(self):
        path, label = self.coordination_path()
        code, data = coordination_api('glab', self.host, 'GET', path)
        if code != 200 or not isinstance(data, list):
            return {'state': 'unknown'}
        holders = [item for item in data if (item.get('label') or {}).get('id') == label]
        return {'state': 'held', 'identity': ','.join(str(item.get('id')) for item in holders), 'owner': ''} if holders else {'state': 'absent'}

    def acquire(self, owner):
        path, label = self.coordination_path()
        code, data = coordination_api('glab', self.host, 'GET', path)
        if code != 200 or not isinstance(data, list):
            raise RuntimeError(f'GitLab coordination board unavailable (HTTP {code})')
        if any((item.get('label') or {}).get('id') == label for item in data):
            return None  # non-authoritative hint; only a new acknowledged POST grants ownership
        code, data = coordination_api('glab', self.host, 'POST', path, {'label_id': label})
        if code == 400 and isinstance(data, dict) and data.get('message') == {'error': 'Label has already been taken'}:
            return None
        if code != 201 or not isinstance(data, dict) or type(data.get('id')) is not int or data['id'] <= 0:
            raise RuntimeError(f'GitLab guard creation failed/unknown (HTTP {code}); inspect configured board/label')
        return data['id']

    def release(self, token):
        path, _ = self.coordination_path()
        if type(token) is not int or token <= 0:
            raise RuntimeError('invalid GitLab exact guard token')
        code, _ = coordination_api('glab', self.host, 'DELETE', f'{path}/{token}')
        if code != 204:
            raise RuntimeError(f'GitLab exact guard release failed/unknown (HTTP {code})')

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
        return [self.issue(item) for item in self.pages(query)]  # None: every open issue (§ 2)

    def comments(self, n):
        return [item['body'] for item in self.pages(f'issues/{n}/notes?sort=asc&activity_filter=only_comments')]

    def closed(self):
        return [self.issue(item) for item in self.pages(f'issues?state=closed&labels={EVENT_LABEL}')]

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
        config['repo'], config.get('host'), config.get('board_options'))


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
            done = subprocess.run([shutil.which('claude') or 'claude', 'agents', '--json', '--all'], capture_output=True, **no_window(), text=True, encoding='utf-8', timeout=60)
            listed = json.loads(done.stdout) if not done.returncode else None
        except (OSError, subprocess.SubprocessError, ValueError):
            return None
        return {item['sessionId']: item for item in listed if isinstance(item, dict) and item.get('kind') == 'background'
                and item.get('sessionId')} if isinstance(listed, list) else None

    def start(self, arguments, cwd):
        """`claude --bg ...`; it prints `backgrounded · <short id>`, `claude agents` gives the full one."""
        done = subprocess.run([shutil.which('claude') or fail('claude not found'), '--bg', *arguments], cwd=cwd, env=worker_env(),
                              capture_output=True, **no_window(), text=True, encoding='utf-8', timeout=120)
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
            done = subprocess.run([shutil.which('claude') or 'claude', 'stop', agent['id']], capture_output=True, **no_window(), timeout=60)
            if done.returncode:
                raise RuntimeError('claude stop failed; session was not confirmed stopped')
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
                done = subprocess.run([claude, 'stop', agent['id']], capture_output=True, **no_window(), timeout=60)
                if done.returncode:
                    raise RuntimeError('claude stop failed; retirement refused')
            done = subprocess.run([claude, 'rm', agent['id']], capture_output=True, **no_window(), timeout=60)
            if done.returncode:
                raise RuntimeError('claude rm failed; retirement unconfirmed')

    def tail(self, session):
        """The last line of `claude logs`, for an ask (#393)."""
        job = ((self.agents() or {}).get(session) or {}).get('id') or session
        done = subprocess.run([shutil.which('claude') or 'claude', 'logs', job], capture_output=True, **no_window(), text=True, encoding='utf-8', timeout=60)
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

@contextlib.contextmanager
def windows_process(pid, terminate=False):
    """A stable kernel handle; never truncate a 64-bit HANDLE through ctypes defaults."""
    import ctypes
    from ctypes import wintypes
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE, *[ctypes.POINTER(wintypes.FILETIME)] * 4]
    kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
    handle = kernel.OpenProcess(0x1000 | 0x100000 | (1 if terminate else 0), False, pid)
    if not handle:
        error = ctypes.get_last_error()
        if error == 87:  # ERROR_INVALID_PARAMETER: no such PID
            raise ProcessLookupError(pid)
        raise OSError(error, 'OpenProcess failed')
    try:
        yield kernel, handle
    finally:
        kernel.CloseHandle(handle)


def process_domain(platform=None):
    """Local observation domain; a foreign OS/host PID cannot prove the recorded process died."""
    platform = platform or ('windows' if os.name == 'nt' else sys.platform)
    try:
        if platform == 'linux':
            boot = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
            return f'linux:{boot}' if re.fullmatch(r'[0-9a-f-]{36}', boot) else None
        if platform in ('windows', 'darwin'):
            hostname = socket.gethostname().strip().casefold()
            return f'{platform}:{hashlib.sha256(hostname.encode()).hexdigest()}' if hostname else None
    except OSError:
        pass
    return None


def birth_domain(birth):
    if not isinstance(birth, str) or not re.fullmatch(
            r'(?:windows:[0-9a-f]{64}:\d+|linux:[0-9a-f-]{36}:\d+|darwin:[0-9a-f]{64}:\d+:\d+)', birth):
        return None
    return ':'.join(birth.split(':')[:2])


def windows_birth(kernel, handle):
    import ctypes
    from ctypes import wintypes
    times = [wintypes.FILETIME() for _ in range(4)]
    if not kernel.GetProcessTimes(handle, *[ctypes.byref(t) for t in times]):
        raise OSError('GetProcessTimes failed')
    domain = process_domain('windows')
    return f'{domain}:{(times[0].dwHighDateTime << 32) | times[0].dwLowDateTime}' if domain else None


def darwin_identity(pid):
    """libproc PROC_PIDTBSDINFO: xnu/bsd/sys/proc_info.h and libproc.c, not ps text."""
    import ctypes
    import errno
    class BsdInfo(ctypes.Structure):
        _fields_ = [(name, ctypes.c_uint32) for name in
                    ('flags', 'status', 'xstatus', 'pid', 'ppid', 'uid', 'gid', 'ruid', 'rgid', 'svuid', 'svgid', 'reserved')]
        _fields_ += [('comm', ctypes.c_char * 16), ('name', ctypes.c_char * 32)]
        _fields_ += [(name, ctypes.c_uint32) for name in ('nfiles', 'pgid', 'pjobc', 'tdev', 'tpgid')]
        _fields_ += [('nice', ctypes.c_int32), ('start_sec', ctypes.c_uint64), ('start_usec', ctypes.c_uint64)]
    lib = ctypes.CDLL('/usr/lib/libproc.dylib', use_errno=True)
    lib.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
    lib.proc_pidinfo.restype = ctypes.c_int
    info = BsdInfo()
    ctypes.set_errno(0)
    size = lib.proc_pidinfo(pid, 3, 0, ctypes.byref(info), ctypes.sizeof(info))
    if size == 0 and ctypes.get_errno() == errno.ESRCH:
        return 'dead', None
    if size != ctypes.sizeof(info) or info.pid != pid or not info.start_sec or info.start_usec >= 1000000:
        return 'unknown', None
    domain = process_domain('darwin')
    return (('dead' if info.status == 5 else 'running'), f'{domain}:{info.start_sec}:{info.start_usec}') if domain else ('unknown', None)


def process_identity(pid):
    """(running/dead/unknown, birth). OS process creation identity, never a bare PID."""
    if not isinstance(pid, int) or pid <= 0:
        return 'unknown', None
    try:
        if os.name == 'nt':
            with windows_process(pid) as (kernel, handle):
                wait = kernel.WaitForSingleObject(handle, 0)
                if wait == 0:
                    return 'dead', windows_birth(kernel, handle)
                if wait != 258:  # WAIT_TIMEOUT: still running
                    return 'unknown', None
                return 'running', windows_birth(kernel, handle)
        if sys.platform == 'darwin':
            return darwin_identity(pid)
        if sys.platform == 'linux':
            domain = process_domain('linux')
            if domain is None:
                return 'unknown', None
            try:
                fields = Path(f'/proc/{pid}/stat').read_text().rpartition(')')[2].split()
            except FileNotFoundError:
                return 'dead', None
            return ('dead' if fields[0] in ('Z', 'X') else 'running'), f'{domain}:{fields[19]}'
    except ProcessLookupError:
        return 'dead', None
    except (OSError, ValueError, IndexError):
        pass
    return 'unknown', None  # unsupported POSIX hosts cannot safely establish ownership


def process_state(pid, birth):
    domain = birth_domain(birth)
    if domain is None or domain != process_domain():
        return 'unknown'  # legacy/foreign ownership is never inferred from a local PID or its absence
    state, actual = process_identity(pid)
    if state == 'unknown' or state == 'running' and actual is None or actual is not None and birth_domain(actual) != domain:
        return 'unknown'
    return 'dead' if actual is not None and actual != birth else state


def stop_process(pid, birth):
    """Pin before checking identity. No os.kill fallback: PID reuse must never kill a foreign process."""
    if process_state(pid, birth) == 'dead':
        return
    if process_state(pid, birth) != 'running':
        raise RuntimeError('process ownership unknown; recovery handle retained')
    if os.name == 'nt':
        with windows_process(pid, terminate=True) as (kernel, handle):
            if windows_birth(kernel, handle) != birth:
                return
            if not kernel.TerminateProcess(handle, 1) or kernel.WaitForSingleObject(handle, 5000) != 0:
                raise RuntimeError('process termination unconfirmed; recovery handle retained')
    elif sys.platform == 'linux' and hasattr(os, 'pidfd_open') and hasattr(signal, 'pidfd_send_signal'):
        import select
        try:
            fd = os.pidfd_open(pid)
        except ProcessLookupError:
            return
        try:
            if process_state(pid, birth) == 'dead':
                return
            if process_state(pid, birth) != 'running':
                raise RuntimeError('process ownership unknown; recovery handle retained')
            signal.pidfd_send_signal(fd, signal.SIGTERM)
            if not select.select([fd], [], [], 5)[0]:
                raise RuntimeError('process termination unconfirmed; recovery handle retained')
        finally:
            os.close(fd)
    else:
        raise RuntimeError('safe process termination unavailable; recovery handle retained')


def read_process(path):
    """Compatibility parser: PID/session remain readable; only a birth-bearing record proves ownership."""
    try:
        fields = path.read_text(encoding='utf-8').split()
        return int(fields[0]), fields[1], fields[2] if len(fields) == 3 else None
    except (OSError, ValueError, IndexError):
        return 0, None, None


def write_process(path, process, session):
    birth = getattr(process, 'taskq_birth', None)
    if not isinstance(birth, str):
        birth = process_identity(process.pid)[1]
    temporary = path.with_name(f'.{path.name}.{uuid.uuid4().hex}.tmp')
    with open(temporary, 'x', encoding='utf-8') as stream:
        stream.write(f'{process.pid} {session} {birth or "-"}')
    os.replace(temporary, path)  # failure preserves the previous handle and the temporary evidence


def codex_options():
    # R9 (#620): owner-approved full access on every host; explicit project options replace the whole default.
    return CONFIG.get('codex', ['-s', 'danger-full-access'])

class Unnamed(Exception):
    """R3 (#572): a started session whose native name was not confirmed; `thread` is its id."""

    def __init__(self, thread, why):
        super().__init__(f'{thread} not named: {why}')
        self.thread = thread

class Codex:
    """`codex exec`, headless: one process per turn, its JSONL in .taskq/<name>.log, `<pid> <thread> <birth>` in .taskq/<name>.pid."""

    def folder(self):
        (CONFIG['root'] / '.taskq').mkdir(exist_ok=True)
        return CONFIG['root'] / '.taskq'

    def exec(self, name, arguments, cwd):
        log, detach = self.folder() / f'{name.split()[0]}.log', {'creationflags': 0x208} if os.name == 'nt' else {'start_new_session': True}
        owned = CONFIG.get('_model_launch')
        with contextlib.ExitStack() as stack:
            prompt = stack.enter_context(tempfile.TemporaryFile())
            out = stack.enter_context(open(log, 'ab'))
            err = stack.enter_context(open(log.with_suffix('.stderr'), 'ab')) if owned else subprocess.STDOUT
            prompt.write(arguments[-1].encode('utf-8'))
            prompt.seek(0)  # file-backed stdin avoids both argv limits and blocking writes to a slow child
            process = subprocess.Popen([shutil.which('codex') or fail('codex not found'), 'exec', '--json', *codex_options(), *arguments[:-1], '-'],
                                       cwd=cwd, env=worker_env(), stdin=prompt, stdout=out, stderr=err, **detach)
        process.taskq_birth = process_identity(process.pid)[1]
        if owned:
            owned(process, log)
        dispatch('turn end', [], after=process.pid, after_birth=process.taskq_birth)  # #525: a sandboxed turn starts no pass; one runs when it ends (no sender, no timer)
        return process, log

    def spawn(self, name, prompt, cwd):
        log = self.folder() / f'{name.split()[0]}.log'
        target = log.with_suffix('.pid')
        if target.exists() or target.is_symlink():
            pid, sid, birth = read_process(target)
            if not sid or process_state(pid, birth) != 'dead':
                raise RuntimeError(f'codex target handle {target.name} is unresolved or running; spawn refused')
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
        write_process(pid, process, found)
        try:
            self.title(found, name, process=process)
        except (OSError, ValueError) as error:  # R3 (#572): no unnamed thread works; its pid file stays for the retire (R11)
            process.terminate()
            raise Unnamed(found, error)
        return found

    WAIT = 60  # seconds for the whole app-server exchange, its shutdown included

    def title(self, thread, name, process=None):
        """R3 (#572): `codex exec` names no thread; the app-server's `thread/name/set` does, `thread/read` proves it.
        Each request waits for its own successful reply. `exec resume` keeps the name. Raises ValueError unless named."""
        server = subprocess.Popen([shutil.which('codex') or 'codex', 'app-server'], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, env=worker_env(), text=True, encoding='utf-8', **no_window())
        deadline, request_id = time.monotonic() + self.WAIT, 0
        timer = threading.Timer(self.WAIT, server.kill)  # one deadline through shutdown: a hung server is killed, its stdout ends
        timer.start()

        def send(message):
            server.stdin.write(json.dumps({'jsonrpc': '2.0', **message}) + '\n')
            server.stdin.flush()

        def call(method, params, wait_rollout=False):
            nonlocal request_id
            while True:
                if time.monotonic() >= deadline:
                    raise ValueError(f'{method}: no reply in {self.WAIT} s')
                n, request_id = request_id, request_id + 1
                send({'id': n, 'method': method, 'params': params})
                for line in server.stdout:  # notifications and the server's own requests pass by
                    reply = json.loads(line) if line.startswith('{') else {}
                    if reply.get('id') == n and 'method' not in reply:
                        if 'result' in reply:
                            return reply['result']
                        error = reply.get('error')
                        missing = (isinstance(error, dict) and error.get('code') == -32600
                                   and error.get('message') == f'no rollout found for thread id {thread}')
                        # #616: a new rollout can exist before its first session metadata line is written.
                        empty = (isinstance(error, dict) and error.get('code') == -32603
                                 and isinstance(error.get('message'), str) and re.fullmatch(
                                     r'failed to set thread name: Fatal error: failed to update thread metadata '
                                     + re.escape(thread) + r': thread-store internal error: failed to read session metadata '
                                     + r'(?P<path>(?:[A-Za-z]:[\\/]|/|\\\\)[^:\r\n]*[\\/]rollout-\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-'
                                     + re.escape(thread) + r'\.jsonl): rollout at (?P=path) is empty', error['message']))
                        remaining = deadline - time.monotonic()
                        if wait_rollout and (missing or empty) and remaining > 0 and process is not None and process.poll() is None:
                            time.sleep(min(.1, remaining))
                            break  # same app-server, fresh request ID, same overall deadline
                        raise ValueError(f'{method}: {error}')
                else:
                    raise ValueError(f'{method}: no reply in {self.WAIT} s')
        try:
            call('initialize', {'clientInfo': {'name': 'taskq', 'version': '1'}})
            send({'method': 'initialized'})
            call('thread/name/set', {'threadId': thread, 'name': name}, wait_rollout=True)
            read = call('thread/read', {'threadId': thread}).get('thread') or {}
            if (read.get('id'), read.get('name')) != (thread, name):
                raise ValueError(f'thread/read: {read.get("id")} named {read.get("name")!r}')
        finally:
            with contextlib.suppress(OSError):
                server.stdin.close()
            server.wait()  # bounded: the timer kills it
            timer.cancel()
            server.stdout.close()

    def pid_file(self, session):
        return next((path for path in self.folder().glob('*.pid') if read_process(path)[1] == session), None)

    def send(self, session, text):
        state = self.state(session)
        if state not in ('idle', 'dead'):
            raise RuntimeError(f'codex session {state or "unknown"}; resume refused')
        process, log = self.exec(getattr(self.pid_file(session), 'stem', session), ['resume', session, text], CONFIG['root'])
        write_process(log.with_suffix('.pid'), process, session)
        return session

    def alive(self, session):
        return {'running': True, 'idle': False, 'dead': False}.get(self.state(session))

    def observe(self, sid):
        """Read-only diagnostic evidence, not a lifecycle/retirement authorization."""
        observation = {'state': 'unknown', 'code': None, 'source': 'cli-turn', 'problems': []}
        if not re.fullmatch(r'[\w-]{1,64}', sid or ''):
            return observation
        folder = CONFIG['root'] / '.taskq'  # do not call folder()/state(): they may mkdir
        path = next((p for p in folder.glob('*.pid') if read_process(p)[1] == sid), None)
        if path:
            pid, _, birth = read_process(path)
            process = process_state(pid, birth)
            log = path.with_suffix('.log')
            entries, offsets = [], []
            try:
                with log.open('rb') as stream:
                    history_start = max(0, log.stat().st_size - 16 * 1024 * 1024)
                    stream.seek(history_start)
                    if history_start:
                        observation['problems'].append({'code': 'history-incomplete', 'source': 'cli-turn', 'evidence_id': 'truncated'})
                    while True:
                        position = stream.tell()
                        line = stream.readline()
                        if not line:
                            break
                        try:
                            value = json.loads(line)
                        except (ValueError, UnicodeDecodeError):
                            continue
                        if isinstance(value, dict):
                            entries.append(value)
                            offsets.append(position)
            except OSError:
                observation['problems'].append({'code': 'history-incomplete', 'source': 'cli-turn', 'evidence_id': 'unreadable'})
            attributed, positions, owner, unidentified = [], [], None, False
            for position, entry in zip(offsets, entries):
                if entry.get('type') == 'thread.started':
                    owner = entry.get('thread_id')
                if owner == sid:
                    attributed.append(entry)
                    positions.append(position)
                elif owner is None and entry.get('type') in ('turn.started', 'item.completed', 'turn.completed', 'turn.failed', 'error'):
                    unidentified = True
            if unidentified or not attributed:
                observation['problems'].append({'code': 'history-incomplete', 'source': 'cli-turn', 'evidence_id': 'attribution-unknown'})
            entries, offsets = attributed, positions
            if birth is None:
                observation['problems'].append({'code': 'legacy-handle', 'source': 'cli-turn', 'evidence_id': 'birth-missing'})
            start = next((i for i in range(len(entries) - 1, -1, -1)
                          if entries[i].get('type') == 'turn.started'), None)
            observation['evidence_id'] = f'turn-offset:{offsets[start]}' if start is not None else 'turn-unavailable'
            latest = entries[start:] if start is not None else []
            terminal = next((e['type'] for e in reversed(latest)
                             if e.get('type') in ('turn.completed', 'turn.failed', 'error')), None)
            observation['state'] = ('running' if process == 'running' else
                                    'idle' if process == 'dead' and terminal == 'turn.completed'
                                    and rollout(sid) == 'local' else 'unknown')
            if process == 'dead' and terminal in ('turn.failed', 'error'):
                observation['code'] = 'interrupted-turn'
            failures = {}
            for position, entry in zip(offsets, entries):
                item = entry.get('item') or {}
                if not isinstance(item, dict) or entry.get('type') != 'item.completed' or item.get('type') != 'command_execution':
                    continue
                command = item.get('command')
                if not isinstance(command, str) or not isinstance(item.get('exit_code'), int):
                    continue
                failures.pop(command, None)  # exact successful retry settles that command's earlier refusal
                if not item['exit_code']:
                    continue
                output = item.get('aggregated_output') or ''
                if not isinstance(output, str):
                    continue
                failure = None
                for marker, code in (
                    ('already has an active writer', 'active-writer'),
                    ('no access token', 'surface-auth'), ('no refresh token', 'surface-auth'),
                    ('helper_unknown_error', 'runtime-rejected'), ('POLICYDENY', 'runtime-rejected'),
                    ('pong_timeout', 'transport-loss'), ('project guard busy', 'ack-contention'),
                    ('No module named \'fcntl\'', 'native-windows'), ('dubious ownership', 'native-windows'),
                    ('hook timeout', 'after-hook')):
                    if marker in output:
                        failure = code
                        break
                if not failure and re.search(r"taskq(?:\.py)?[\"']?\s+result\s+\d+", command):
                    failure = 'result-unsubmitted'
                if failure:
                    failures[command] = (failure, f'offset:{position};item:{item.get("id") or "unavailable"}', bool(re.search(r"taskq(?:\.py)?[\"']?\s+result\s+\d+", command)))
            if observation['code']:
                observation['problems'].append({**observation, 'problems': []})
            for code, item_id, submission in failures.values():
                record = {'code': code, 'source': 'cli-turn', 'evidence_id': item_id}
                observation['problems'].append(record)
                observation.update(record)
                if submission and code != 'result-unsubmitted':
                    observation['problems'].append({**record, 'code': 'result-unsubmitted'})
            if read_process(path) != (pid, sid, birth):
                return {'state': 'unknown', 'code': None, 'source': 'handle-changed'}
        home = Path(os.environ.get('CODEX_HOME') or Path.home() / '.codex')
        lock = home / 'thread-writer-locks' / f'{sid}.lock'
        lsof = shutil.which('lsof') if lock.is_file() else None
        if lsof:
            try:
                holder = subprocess.run([lsof, '-t', '--', str(lock)], capture_output=True, text=True,
                                        timeout=2, **no_window())
                if holder.returncode == 0 and re.search(r'^\d+$', holder.stdout, re.M):
                    observation.update(state='unknown', code='active-writer', source='writer-holder',
                                       evidence_id='holder:' + ','.join(sorted(set(re.findall(r'^\d+$', holder.stdout, re.M)))))
                    observation['problems'].append({key: observation[key] for key in ('code', 'source', 'evidence_id')})
                elif holder.returncode != 1 or holder.stderr:
                    observation.update(state='unknown', source='writer-unverified')
            except (OSError, subprocess.TimeoutExpired):
                observation.update(state='unknown', source='writer-unverified')
        elif lock.is_file():
            observation.update(state='unknown', source='writer-unverified')
        if observation['source'] == 'writer-unverified':
            observation['problems'].append({'code': 'unknown-runtime', 'source': 'writer-unverified', 'evidence_id': 'holder-unverified'})
        return observation

    def state(self, session):
        """A supervisor (§ 7 step 4): its process exits at every turn end, by design. Pid running: running; exited after a
        last turn that ended `turn.completed`, with a local rollout: idle; anything else: dead."""
        path = self.pid_file(session)
        if path is None:
            return None
        pid, _, birth = read_process(path)
        state = process_state(pid, birth)
        if state != 'dead':
            return state
        log = path.with_suffix('.log')
        turn = (log.read_text('utf-8', 'replace') if log.exists() else '').rpartition('"turn.started"')
        ends = re.findall(r'"type":\s*"(turn\.completed|turn\.failed|error)"', turn[2]) if turn[1] else []
        return 'idle' if ends[-1:] == ['turn.completed'] and rollout(session) == 'local' else 'dead'

    def resumable(self, session):  # a dead supervisor's first recovery: `exec resume` of the same thread
        return rollout(session) == 'local'

    def retire(self, gone, running=True):
        """True: selected predecessors retired; False: preserve/defer. Unknown effects still raise."""
        complete = True
        safe_stop = os.name == 'nt' or (sys.platform == 'linux' and hasattr(os, 'pidfd_open')
                                       and hasattr(signal, 'pidfd_send_signal'))
        for path in (CONFIG['root'] / '.taskq').glob('*.pid'):
            n = re.fullmatch(r'[TS](\d+)(-[\w-]+)?', path.stem)
            pid, thread, birth = read_process(path)
            if not n or not thread:
                continue
            state = process_state(pid, birth)
            live = None if state == 'unknown' else state == 'running'
            if not gone(int(n[1]), thread, live):
                continue
            if state == 'unknown' or state == 'running' and (not running or not safe_stop):
                complete = False
                continue
            if state == 'running':
                stop_process(pid, birth)
            done = subprocess.run([shutil.which('codex') or 'codex', 'archive', thread], capture_output=True, **no_window(), timeout=60)
            if done.returncode:
                raise RuntimeError('codex archive failed; recovery handle retained')
            path.unlink()
        return complete

    def tail(self, session):
        path = self.pid_file(session)
        return last_line(path.with_suffix('.log').read_text('utf-8', 'replace')) if path else ''

    def link(self, session):
        return f'{CONFIG.get("pages", "https://alexkirs.github.io/taskq/").rstrip("/")}/open.html#codex://threads/{session}'

class Hermes:
    """Admission guard over the existing runtime-file interface; no native transport or session registry."""

    def __init__(self, adapter):
        self.adapter = adapter

    def check(self):
        required = ('spawn', 'send', 'alive', 'link', 'state', 'retire', 'available')
        if not all(callable(getattr(self.adapter, name, None)) for name in required):
            fail('Hermes native supervisor bridge needs spawn/send/alive/link/state/retire/available')
        try:
            ready = self.adapter.available()
        except Exception as error:
            fail(f'Hermes native supervisor bridge unavailable: {error}')
        if ready is not True:
            fail('Hermes native supervisor bridge unavailable')

    def __getattr__(self, name):
        return getattr(self.adapter, name)

    def spawn(self, name, prompt, cwd):
        self.check()
        return self.identity(self.adapter.spawn(name, prompt, cwd))

    def send(self, sid, text):
        self.check()
        return self.identity(self.adapter.send(sid, text))

    @staticmethod
    def identity(sid):
        if not isinstance(sid, str) or not sid.strip():
            fail('Hermes bridge returned no genuine session id')
        return sid

    def state(self, sid):
        self.check()
        state = self.adapter.state(sid)
        if state not in ('running', 'idle', 'dead'):
            fail('Hermes supervisor state unknown: admission stopped')
        return state

def runtimes():
    """claude, codex, and each `"runtimes": {"name": "runtimes/name.py"}` file of taskq.json."""
    extra = {name: load_file(path) for name, path in CONFIG.get('runtimes', {}).items()}
    if 'hermes' in extra:
        extra['hermes'] = Hermes(extra['hermes'])
    return {'claude': Claude(), 'codex': Codex(), **extra}


# --- commands -------------------------------------------------------------------------------

def read_issue(n):
    """Fresh metadata when supported; legacy adapters keep their complete get(n) contract."""
    return getattr(BOARD, 'metadata', BOARD.get)(n)


def issue_history(issue):
    """Load trusted comments only when consumed, without another metadata request."""
    if 'comments' not in issue:
        comments = getattr(BOARD, 'comments', None)
        issue = {**issue, 'comments': comments(issue['iid']) if comments else BOARD.get(issue['iid'])['comments']}
    return issue


def task(n, *states):
    issue = read_issue(n)
    found = parse(issue) if issue['state'] == 'open' else None
    if not found:
        fail(f'#{n} is not an open taskq task')
    if compatibility_reason(found):
        fail(compatibility_reason(found))
    if states and found['state'] not in states:
        fail(f'#{n} is {found["state"]}, not {" or ".join(states)}')
    return found

def move(current, state, action, text='', **fields):
    """One update moves the label and the block together; one comment is the history. State None: no state label."""
    if compatibility_reason(current):
        fail(compatibility_reason(current))
    if current['raw'].get('model_turns') and (current.get('claim') or {}).get('session') \
            and 'claim' in fields and fields['claim'] != current['claim']:
        fail('model worker identity retirement/replacement is not qualified; preserve pending receipts')
    labels = [label for label in current['labels'] if not label.startswith(PREFIX)] + ([PREFIX + state] if state else [])
    raw = {**current['raw'], **{key: current[key] for key in FIELDS}, **fields}
    raw['events'] = [dict(event, acks=list(event.get('acks', []))) for event in raw.get('events', [])]
    reconcile_recipients(raw)
    append_event(raw, action, text)
    labels = event_labels(raw, labels)
    counts = dict(raw.get('retry_counts') or {})
    if action in ('answer', 'result'):
        counts.update(supervisor=0, worker=0)
    if action == 'answer':
        counts['workers'] = 0
    if action == 'spawn' and text.startswith('worker '):
        counts['workers'] = counts.get('workers', 0) + 1
    if action == 'gone' and text.startswith('supervisor '):
        counts['supervisor'] = counts.get('supervisor', 0) + 1
    if action == 'requeue' and ' is gone' in text:
        counts['worker'] = counts.get('worker', 0) + 1
    raw['retry_counts'] = counts
    records = list(raw.get('session_records') or [])
    for source in (current, raw):
        for role_name, key in (('supervisor', 'supervisor'), ('worker', 'claim')):
            identity = source.get(key) or {}
            if identity.get('session'):
                record = {'role': role_name, 'runtime': identity['runtime'], 'session': identity['session']}
                if record not in records:
                    records.append(record)
    raw['session_records'] = records
    effect(BOARD.update, current['iid'], labels=labels, body=block(current['text'], raw))
    current.update(raw=raw, labels=labels, **{key: raw.get(key) for key in FIELDS})
    if state:
        current['state'] = state
    diagnostic_comment(current['iid'], f'**{action}** · {who()}' + (f'\n\n{text}' if text else ''))
    print(f'#{current["iid"]} {state or "closed"}')

def open_deps(deps):
    return [n for n in deps or [] if read_issue(n)['state'] == 'open']

def cmd_add(args):
    text = f'## Goal\n\n{args.goal}\n\n## Acceptance\n\n{args.acceptance}'
    state = 'later' if args.later else 'waiting' if open_deps(args.deps) else 'ready'
    labels = [PREFIX + state, f'priority-{args.priority}', args.type] + ([RUN + args.runtime] if args.runtime != 'any' else []) \
        + ([ON + args.host] if args.host else [])
    raw = {'scope': args.scope, 'deps': args.deps, 'claim': None, 'result': None, 'pm': origin()}
    append_event(raw, 'add', '')
    n = effect(BOARD.add, args.title, block(text, raw), labels)
    diagnostic_comment(n, f'**add** · {who()}')
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
    reason = execution_reason(current)
    if reason:
        fail(reason)
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
    current = parse(read_issue(args.n))
    if current and current['raw'].get('event_schema') == 2:
        return queue_move(args, current)
    if args.command == 'result' and not args.sha:
        fail('legacy result requires --sha')
    if args.command == 'result' and current and current['raw'].get('model_turns'):
        turn = current['raw']['model_turns'].get('worker') or {}
        receipts = current['raw'].get('application_receipts') or {}
        if any(str(event_id) not in receipts or receipts[str(event_id)]['git_sha'] != args.sha for event_id in turn.get('event_ids', [])):
            fail('model result requires native applied receipts at its exact Git SHA')
    sources, state, fields = MOVES[args.command]
    move(task(args.n, *sources), state, args.command, args.text, **fields(args))

def role(current):
    """Who runs this command for the task: supervisor, worker, manager, owner (a plain shell), or None (another session)."""
    identity = session() or {}
    me = identity.get('session')
    return 'owner' if not me else next((name for name, held in (('supervisor', current['supervisor']), ('worker', current['claim']),
                                                                ('manager', current['pm'])) if me == (held or {}).get('session')
        and ('hermes' not in (identity.get('runtime'), (held or {}).get('runtime'))
             or identity.get('runtime') == (held or {}).get('runtime'))), None)

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
    current = parse(read_issue(args.n))
    if current and current['raw'].get('event_schema') == 2:
        if current.get('result'):
            fail('accepted native result requires separately qualified rework; no automatic resume')
        life = current['raw'].get('lifecycle') or {}
        outcome = lifecycle(args.n, 'resume' if life.get('phase') == 'parked' else 'admit')
        print(json.dumps(outcome)); return
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
    if found == 'worker' and (current['supervisor'] or current['raw'].get('model_turns')):
        turn = (current['raw'].get('model_turns') or {}).get('worker')
        if turn:
            move(current, current['state'], 'requeue', args.text,
                 model_recovery={'session': claim.get('session'), 'turn': turn['key'], 'reason': args.text})
            return
        move(current, current['state'], 'requeue', args.text, claim={**claim, 'session': None})
        return
    gate(current)
    if current['raw'].get('model_recovery'):
        fail('model worker recovery is unresolved; preserve claim and pending application receipts')
    if current['supervisor'] and found == 'supervisor':
        attempts = current['raw'].get('retry_counts', {}).get('workers')
        if attempts is None:
            attempts = workers(args.n, issue_history(read_issue(args.n))['comments'])
        if attempts >= 3:
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
        current = parse(read_issue(int(args.n[0])))
        if current and current['raw'].get('model_turns') and current['state'] in ('doing', 'review'):
            gate(current)
            latest = next((event for event in reversed(current['raw'].get('events', [])) if event['action'] == 'answer'), None)
            if latest and latest['text'] == args.text:
                print(f'#{current["iid"]} answer already recorded; no new delivery')
                return []
        cmd_move(argparse.Namespace(**{**vars(args), 'n': int(args.n[0])}))
        return [int(args.n[0])]
    picks = []
    for n, k in codes(args.n):
        current = parse(read_issue(n))
        if not current or current['raw'].get('event_schema') != 2:
            current = task(n, 'ask', 'review')
        elif current['state'] != 'ask':
            fail(f'#{n}: schema2 options require its current ask')
        options = (current['raw'].get('decision') or {}).get('options') or []
        picks.append((current, k, 0 < k <= len(options) and options[k - 1] or fail(f'#{n} has no option {k}')))
    for current, k, text in picks:
        if current['raw'].get('event_schema') == 2:
            queue_move(argparse.Namespace(**{**vars(args), 'n':current['iid'], 'text':f'{current["iid"]}.{k}: {text}'}),current)
        elif current['state'] == 'review' and text.lower().startswith('close'):
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
        done = subprocess.run([shutil.which(command[0]) or fail(f'{command[0]} not found'), *command[1:], *where], capture_output=True, **no_window(), text=True, encoding='utf-8')
        return done.returncode, (done.stderr.strip() or done.stdout.strip()) if done.returncode else done.stdout
    code, out = cli(*(['glab', 'mr', 'list', '--source-branch', branch, '--output', 'json'] if lab else ['gh', 'pr', 'list', '--head', branch, '--json', 'number,headRefOid,baseRefName']))
    found = code and fail(out) or [(str(pr.get('iid', pr.get('number'))), pr.get('sha', pr.get('headRefOid')), pr.get('target_branch', pr.get('baseRefName')))
                                   for pr in json.loads(out)]
    if found != [(found and found[0][0], sha, 'main')]:  # exactly one, into main, at the full result SHA
        return found and fail(f'{branch}: open PRs (number, head, base) {found} do not match the result {sha} into main')
    number, head, _ = found[0]

    def back(why, settled=True):  # a supervised task stays with its supervisor: it requeues with the fixes (§ 7 Supervisor 3.4)
        kept = {'claim': {**(current['claim'] or {}), 'session': None}} if current['supervisor'] else {'claim': None}
        move(current, 'doing' if current['supervisor'] else 'ready', 'requeue', f'close: PR {number} {why}', result=None, **kept)
        if settled:
            raise SettledError(f'#{current["iid"]}: PR {number} {why}', [current['iid']])
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
    checked = runs()  # never wait for CI while holding the project's mutation guard
    if not checked or not all(done for done, _ in checked):
        raise SettledError(f'#{current["iid"]}: PR {number} {gate} pending or missing on {head}; retry after CI finishes', [])
    if not all(ok for _, ok in checked):
        back(f'{gate} failed on {head}')
    keep = CONFIG.get('workspace') == 'external'  # #477: the host owns the branch; the repo's own policy may still delete it
    _, out = effect(cli, *(['glab', 'mr', 'merge', number, '--squash', *['--remove-source-branch'] * (not keep), '--sha', head, '--auto-merge=false', '--yes'] if lab
                   else ['gh', 'pr', 'merge', number, '--squash', *['--delete-branch'] * (not keep), '--match-head-commit', head]))
    code, viewed = cli(*(['glab', 'mr', 'view', number, '--output', 'json'] if lab else ['gh', 'pr', 'view', number, '--json', 'state,mergeCommit']))
    pr = {} if code else json.loads(viewed)
    if str(pr.get('state')).lower() != 'merged':  # read back: a merge that reported an error may still have merged
        back(f'did not merge: {out}', settled=False)
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
    status = subprocess.run([*git, '-C', str(tree), 'status', '--porcelain'], capture_output=True, **no_window(), text=True, encoding='utf-8')
    removed = not status.returncode and not status.stdout.strip() and \
        not effect(subprocess.run, [*git, 'worktree', 'remove', str(tree)], capture_output=True, **no_window()).returncode
    if not removed:
        return f'kept .worktrees/{branch} and branch {branch}: uncommitted changes'
    effect(subprocess.run, [*git, 'branch', '-D', branch], capture_output=True, **no_window())
    return ''

def cmd_close(args):
    """close N [M ...]: in order; no PR is updated (#359). A failed task does not stop the rest."""
    failed, changed = [], []
    for n in args.n:
        try:
            close_one(argparse.Namespace(n=n, text=args.text))
            changed.append(n)
        except SystemExit as error:
            if (getattr(GUARD, 'held', None) or {}).get('poisoned'):
                raise  # no more closes or settled-event dispatch after an uncertain effect
            if len(args.n) == 1:
                raise
            failed.append(n)
            if isinstance(error, SettledError):
                changed.extend(error.tasks)
            print(error, file=sys.stderr)
    if failed:
        raise SettledError(f'not closed: {" ".join(f"#{n}" for n in failed)}', changed)

def publish_direct(current, sha, git):
    """#533: close is acceptance; transfer and qualify the exact candidate before main can change."""
    if role(current) not in ('supervisor', 'manager', 'owner'):
        fail(f'#{current["iid"]}: only the accepting reviewer can publish a direct candidate')

    def run(*args):
        done = (effect(subprocess.run, [*git, *args], capture_output=True, **no_window(), text=True, encoding='utf-8')
                if args[0] in ('push', 'fetch') else subprocess.run([*git, *args], capture_output=True, **no_window(), text=True, encoding='utf-8'))
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
        raise SettledError(f'#{current["iid"]}: CI is not green on candidate {sha}', [])
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
    append_event(dict(current['raw']), 'close', args.text)  # capacity before publication/close effects
    if role(current) == 'supervisor':
        verdict(args.text)
    sha = commit((current['result'] or {}).get('sha') or '')
    if CONFIG['publish'] == 'pr' and (merged := merge(current, sha)):
        args.text = (f'{args.text}\n\n' if args.text else '') + f'merged {merged}'  # #567: the verdict stays the first line
    else:  # an already-published result/answer, or an unpublished direct candidate
        git = [shutil.which('git') or fail('git not found'), '-C', str(CONFIG['root'])]
        fetched = subprocess.run([*git, 'fetch', 'origin'], capture_output=True, **no_window())
        if fetched.returncode:
            fail(f'#{args.n}: could not fetch origin for publication')
        if subprocess.run([*git, 'merge-base', '--is-ancestor', sha, 'origin/main'], capture_output=True, **no_window()).returncode:
            if CONFIG['publish'] != 'direct':
                fail(f'#{args.n}: result {sha} is not on origin/main')
            publish_direct(current, sha, git)
            args.text = (f'{args.text}\n\n' if args.text else '') + f'published {sha}'
    claim = current['claim'] or {}
    if hasattr(runtimes().get(claim.get('runtime')), 'retire') and claim.get('name') not in (None, machine()):
        args.text = (f'{args.text}\n\n' if args.text else '') + f'session {claim.get("session")} runs on {claim.get("name")}: stop it there'
    kept = cleanup(current)
    effect(BOARD.close, args.n)  # first (#496): a failed close keeps the q-* label, the task stays on the board
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
    for record in raw.get('session_records', []):
        if record['runtime'] == runtime and record['session'] == sid:
            return record['role']
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
            effect(getattr(runtime, 'retire', lambda *_: None), lambda n, sid=None, live=None, name=name: gone(name, n, sid, live), running)
        except Exception as error:
            print(f'{why}: {error}', file=sys.stderr)

def open_prs():
    """{branch: PR/MR number} of the open PRs of the repo, None when they cannot be read (a board file, no CLI)."""
    lab, host = CONFIG['board'] == 'gitlab', CONFIG.get('host')
    if CONFIG['board'] not in ('github', 'gitlab') or not shutil.which('glab' if lab else 'gh'):
        return None
    command = ['glab', 'mr', 'list', '--output', 'json', '-R', f'https://{host}/{CONFIG["repo"]}' if host else CONFIG['repo']] if lab else \
        ['gh', 'pr', 'list', '--state', 'open', '--limit', '500', '--json', 'number,headRefName', '-R', f'{host}/{CONFIG["repo"]}' if host else CONFIG['repo']]
    done = subprocess.run([shutil.which(command[0]), *command[1:]], capture_output=True, **no_window(), text=True, encoding='utf-8')
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
    if not dry:
        for item in items:
            if compatibility_reason(item):
                fail(compatibility_reason(item))
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
        return subprocess.run([shutil.which('git') or fail('git not found'), '-C', str(root), *argv], capture_output=True, **no_window(), text=True, encoding='utf-8')

    def act(what, *command):  # one removal: done (or only printed with --dry-run), else kept with git's reason
        done = None if dry else effect(git, *command)
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
            (getattr(runtime, 'retire', lambda *_: None)(goner(name), False) if dry else
             effect(getattr(runtime, 'retire', lambda *_: None), goner(name), False))  # never a live worker
        except Exception as error:
            kept.append(f'{name} sessions: unknown ({error})')
    for path in sorted((root / '.taskq').glob('*.pid')):
        n = re.fullmatch(r'S(\d+)', path.stem)  # the gone supervisor's; a T<N>.pid is Codex's handle, its retire decides (#478)
        pid, _, birth = read_process(path)
        if n and state(int(n[1])) == 'closed' and process_state(pid, birth) == 'dead':
            dry or effect(path.unlink)
            removed.append(f'{verb} .taskq/{path.name}')
    for wait in sorted((root / '.taskq').glob('wait*.json')):  # one per manager (#532)
        seen = json.loads(wait.read_text('utf-8'))
        stale = [n for n in seen if not n.isdigit() or state(int(n)) != 'open']
        if stale:
            dry or effect(wait.write_text, json.dumps({n: value for n, value in seen.items() if n not in stale}), 'utf-8')
            removed.append(f'{verb} .taskq/{wait.name} entries {" ".join(f"#{n}" for n in stale)}')
    prs = open_prs()
    for item in items:
        claim, n, sha = item['claim'] or {}, item['iid'], (item['result'] or {}).get('sha') or ''
        if item['state'] == 'doing' and claim.get('name') == here and claim.get('runtime') in kinds \
                and runtime_state(kinds[claim['runtime']], claim['session']) == 'dead':
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
    issue = BOARD.get(n)
    notes = [text for text in issue['comments'] or [] if text.startswith(('**result**', '**requeue**', '**answer**', '**ask**'))][-6:]
    for action, payload in issue_data(issue).get('action_payloads', {}).items():
        if action in PAYLOAD_ACTIONS:
            text = f'**{action}** · {payload["by"]}' + (f'\n\n{payload["text"]}' if payload['text'] else '')
            if text not in notes:
                notes.append(text)
    return 'History of this task (read it first; a requeue says what to fix):\n\n' + '\n\n'.join(notes) + '\n\n' if notes else ''

def powershell(runtime):
    """Only native Windows Codex uses this shell; Claude's Bash and POSIX hosts keep their route."""
    return sys.platform == 'win32' and runtime == 'codex'

def shell_quote(value, native=False):
    return "'" + str(value).replace("'", "''") + "'" if native else shlex.quote(str(value))

def queue_tool(runtime):
    script = Path(__file__).resolve()
    return native_command([sys.executable, str(script)], capture=True) if powershell(runtime) else f'python3 {script}'

def native_command(arguments, capture=False):
    """PS 5.1 reparses native argv. Pass a quote-free program to Python; it preserves argv and inherited stdio."""
    encoded = json.dumps(arguments, ensure_ascii=True).encode('utf-8').hex()
    extra = '+json.loads(base64.b64decode(sys.argv[1]))' if capture else ''
    program = f"import base64,json,subprocess,sys;sys.exit(subprocess.call(json.loads(bytes.fromhex('{encoded}')){extra}))"
    command = f'& {shell_quote(sys.executable, True)} -c {shell_quote(program, True)}'
    if not capture:
        return command
    return ('& { $taskqArgs = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes('
            '(ConvertTo-Json -InputObject @($args | ForEach-Object { [string]$_ }) -Compress))); '
            '[Console]::OutputEncoding = $OutputEncoding = [System.Text.UTF8Encoding]::new($false); '
            f'$input | {command} $taskqArgs; $global:LASTEXITCODE = $LASTEXITCODE }}')

def shell_instructions(runtime, n=None):
    if powershell(runtime):
        prefix = f'$env:TASKQ_TASK={shell_quote(n, True)}; $env:TASKQ_RUNTIME={shell_quote(runtime, True)}; $ErrorActionPreference=\'Stop\';'
        return ('Use exec_command with shell="powershell.exe", login=false and native Windows paths. '
                'Do not invoke Bash, WSL or a shell bridge. '
                + (f'Start every shell command with `{prefix}`.' if n is not None else 'Set $ErrorActionPreference=\'Stop\' in each command.'))
    return f'Start every shell command with `export TASKQ_TASK={n} TASKQ_RUNTIME={runtime} &&`.' if n is not None else ''

def shell_steps(runtime, *commands):
    """PowerShell 5.1 has no &&: native command failures must stop before the next step."""
    return ('; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; ' if powershell(runtime) else ' && ').join(commands)

def brief(item, runtime):
    """The worker's prompt: its name first (R3, #572: a runtime's fallback title), the task, its workspace, the taskq commands it uses."""
    n, root, tq = item['iid'], CONFIG['root'], queue_tool(runtime)
    create = f'glab mr create --yes --target-branch main --source-branch taskq-{n} --title "<title>" --description' if CONFIG['board'] == 'gitlab' \
        else f'gh pr create --base main --head taskq-{n} --title "<title>" --body'
    push = f'`git push --force-with-lease origin HEAD:refs/heads/taskq-{n}`, open a pull request once (a push updates it):\n  `{create} "<summary>"`, ' \
        f'then `{tq} result {n} --sha <PR head full SHA>' if CONFIG['publish'] == 'pr' else f'`git push --force-with-lease origin HEAD:refs/heads/taskq-{n}`, then\n  `{tq} result {n} --sha <candidate full SHA>'
    workspace = f'take your workspace from the project instructions (AGENTS.md) or the path the manager gave, on branch taskq-{n};\nthe host owns it: never remove it' \
        if CONFIG.get('workspace') == 'external' else f'from {root} run `{shell_steps(runtime, "git fetch origin", f"git worktree add -b taskq-{n} .worktrees/taskq-{n} origin/main")}`, work only there,\nnever in the main checkout'
    return f'''{worker_name(item)}
You are the taskq worker for task #{n}: {item["title"]}. The task is claimed for you: do it without asking for confirmation.
Queue tool: `{tq}`. {shell_instructions(runtime, n)}
{release_context()}
Read `{Path(__file__).resolve().with_name("taskq.md")}` first and do only what it allows (R13): a task that conflicts with a recorded decision is an ask with options, not an edit.

{item["text"]}

{history(n)}Expected paths: {", ".join(item["scope"] or []) or "none named"}. They say where the work is expected, not what is forbidden.

Workspace: {workspace}; a branch taskq-{n} left by an earlier worker: continue it. A task that ends in an answer, not a commit,
needs no worktree. Commands:
- A question only the owner can decide (a product choice, an action that cannot be undone): `{tq} ask {n} --text "<what was done; the question>"
  --option "<A>" --option "<B>" --recommend <K> [--link <url of a result, image or video>]`, then stop. A result that leaves the owner a choice
  takes the same --option/--recommend/--link; an option starting `close` accepts the result.
- Cannot be done: `{tq} requeue {n} --text "<why>"`, then stop.
- Deliver: commit on branch taskq-{n}, `{shell_steps(runtime, "git fetch origin", "git rebase origin/main")}`, run the tests required by § 10 Testing policy, {push} --checks "<commands and outcome>" --text "<summary>"`, then stop.
  An answer with no commit: the result names the current origin/main SHA and the text holds the answer.
Everything written through taskq is public: no secrets, tokens or paths outside the repository.'''

def supervisor_brief(item, runtime, kind):
    """S<N>'s prompt (§ 7 Supervisor): its name first (R3), the task, its orders, review and close; never the task's code."""
    n, tq, lab = item['iid'], queue_tool(runtime), CONFIG['board'] == 'gitlab'
    wait = f'run `{tq} wait --task {n}` in the background (run_in_background) and end your turn; its output wakes you' \
        if getattr(kind, 'SELF_WAKE', False) else 'end your turn; the queue wakes you with the event'
    ci = f'glab api "projects/:id/pipelines?sha=<sha>"' if lab else 'gh run list --commit <sha>'
    view = f'glab issue view {n} --comments' if lab else f'gh issue view {n} --comments'
    nudge = f'glab issue note {n} -m "nudge: <text>"' if lab else f'gh issue comment {n} --body "nudge: <text>"'
    return f'''{worker_name(item, 'S')}
You are the taskq supervisor S{n} of task #{n}: {item["title"]}. You are its only controller: you order its worker, follow it,
review its result and close or rework it. You never edit the task's code, never start a session yourself, never decide for the owner.
Queue tool: `{tq}`. {shell_instructions(runtime, n)}
{release_context()}
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
   (a product choice, a second death). `answer #{n}`: act on it; an idle worker gets the answer from the queue.
   After handling observed `[event N:ID]`, run `{tq} ack N:ID` for those IDs before waiting again.
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
        if isinstance(kind, (Codex, Claude, Hermes)):
            return owned_model_turn(item, kind, 'worker' if letter == 'T' else 'supervisor', prompt)
        return effect(kind.spawn, worker_name(item, letter), prompt, CONFIG['root'])
    except Unnamed as error:
        role = 'supervisor' if letter == 'S' else 'worker'
        # The failed spawn already poisoned the guard. Append only its identity for explicit recovery; no further admission.
        BOARD.comment(item['iid'], f'**gone** · {who()}\n\n{role} {error}')
        fail(f'#{item["iid"]}: {role} {error}')


def model_providers(admission=False):
    selected = CONFIG.get('capacity')
    if not isinstance(selected, dict) or not callable(getattr(BOARD, 'capacity_provider', None)):
        fail('owned Codex model turns require explicit project/host capacity providers')
    for name in ('project_caps', 'host_caps'):
        if type((selected.get(name) or {}).get('model:codex')) is not int:
            fail('finite model:codex capacity required; no unlimited fallback')
        limits = local_limits() if local_limits() is not None else CONFIG.get('limits', {})
        if admission and name == 'host_caps' and selected[name]['model:codex'] > limits.get('codex', 0):
            fail('model capacity exceeds invocation limit; explicitly provision matching finite capacity')
    project = BOARD.capacity_provider(selected['project_caps'])
    if project.scope != 'project' or project.owner != CONFIG.get('repo') or project.caps != selected['project_caps']:
        fail('model project capacity authority mismatch')
    return project, SQLiteCapacity(selected['host_path'], 'host', machine(), selected['host_caps'])


def model_reconcile(item):
    turns = item['raw'].get('model_turns') or {}
    if not turns:
        return
    project, host = model_providers()
    for role_name, turn in list(turns.items()):
        if turn['phase'] == 'complete':
            continue
        lease = host.observe(turn['key'])
        if not lease or lease['phase'] not in ('bound', 'drained', 'released'):
            continue  # launch outcome unknown or reservation waiting; no guessed release
        try:
            settled = host.settle_model(turn['key'], turn['runtime'])
        except (ValueError, OSError):
            continue
        host.release(turn['key'])
        project.release(turn['key'])
        issue = read_issue(item['iid'])
        raw = issue_data(issue)
        if (raw.get('model_turns') or {}).get(role_name) != turn:
            fail('model turn changed during settlement; preserve newer board state')
        raw['model_turns'][role_name] = {**turn, 'phase': 'complete', 'completion': settled['drain']}
        fresh = write_task_verified(issue, raw, issue['labels'])
        item.update(parse(fresh))


def model_role_state(item, role_name):
    if role_name == 'worker' and item['raw'].get('model_recovery'):
        return 'unknown'
    turn = (item['raw'].get('model_turns') or {}).get(role_name)
    if not turn:
        return None
    if turn['phase'] == 'complete':
        return 'idle' if turn['completion']['terminal'] == 'turn.completed' else 'unknown'
    return 'running' if turn['phase'] == 'bound' else 'unknown'


def model_event_input(event):
    return {key: event[key] for key in ('id', 'action', 'text', 'by', 'recipients')}


def owned_model_turn(item, kind, role_name, prompt, events=(), resume=None):
    """Existing CLI adapter, durable no-retry intent, separate finite model budget."""
    if not isinstance(kind, Codex):
        fail('runtime model-turn binding is not qualified; no alternate-worker fallback')
    if role_name == 'worker' and item.get('result'):
        fail('model rework requires explicit qualified order; accepted result cannot auto-resume')
    limits = local_limits() if local_limits() is not None else CONFIG.get('limits', {})
    if limits.get('codex', 0) < 1:
        return None
    model_reconcile(item)
    issue = read_issue(item['iid'])
    current = parse(issue)
    if not current or not executable(issue, item):
        fail('task changed before model admission')
    raw = current['raw']
    if role_name == 'worker' and raw.get('model_recovery'):
        return None  # no implicit retry/replacement after a native failure report
    turns = dict(raw.get('model_turns') or {})
    prior = turns.get(role_name)
    event_ids = [event['id'] for event in events]
    inputs = [model_event_input(event) for event in events]
    digest = hashlib.sha256(json.dumps(inputs, sort_keys=True).encode()).hexdigest()
    if prior and prior['phase'] not in ('complete', 'reserving'):
        return None  # reservation/binding unknown: never spawn another executor
    if prior and prior['phase'] == 'complete' and prior['event_ids'] == event_ids and prior['event_digest'] == digest:
        return prior['runtime']['session']  # exact already handled input, no new turn
    project, host = model_providers(admission=True)
    generation = (prior or {}).get('generation', 0) + 1
    request = {'repo': CONFIG['repo'], 'task': item['iid'], 'role': role_name, 'generation': generation,
               'owner': origin(), 'age': raw.get('admission_age', time.time()), 'priority': item['priority'],
               'events': event_ids, 'event_digest': digest, 'prompt_sha256': hashlib.sha256(prompt.encode()).hexdigest(),
               'demand': {'model:codex': 1}}
    if hasattr(project, 'domain'):
        request['project_domain'] = project.domain
    key = hashlib.sha256(json.dumps(request, sort_keys=True).encode()).hexdigest()
    name = worker_name(item, 'T' if role_name == 'worker' else 'S')
    log = kind.folder() / (name.split()[0] + '.log')
    prefix = log.read_bytes() if log.exists() else b''
    runtime = {'kind': 'codex-model-turn', 'session': resume, 'log': str(log.resolve()),
               'offset': len(prefix), 'prefix_sha256': hashlib.sha256(prefix).hexdigest()}
    turn = {'key': key, 'generation': generation, 'phase': 'reserving', 'request': request,
            'event_ids': event_ids, 'event_inputs': inputs, 'event_digest': digest, 'runtime': runtime}
    if prior and prior['phase'] == 'reserving':
        if prior['event_ids'] != event_ids or prior['event_digest'] != digest \
                or prior['request']['prompt_sha256'] != request['prompt_sha256']:
            return None
        turn, key, request, runtime = prior, prior['key'], prior['request'], prior['runtime']
    elif prior:
        history = list(raw.get('model_turn_history') or [])
        if len(history) >= EVENT_LIMIT:
            fail('model history full; explicit settled-history compaction required')
        raw['model_turn_history'] = history + [prior]
    turns[role_name] = turn
    raw['model_turns'] = turns
    write_task_verified(issue, raw, issue['labels'])  # before reservations or Popen
    if project.reserve(key, request)['phase'] != 'reserved':
        return None
    if host.reserve(key, request)['phase'] != 'reserved':
        return None
    if not host.launch(key, runtime):
        fail('model launch intent already consumed; no retry')
    def record(value):
        fresh = read_issue(item['iid'])
        data = issue_data(fresh)
        if (data.get('model_turns') or {}).get(role_name) != turn:
            fail('model intent changed before binding')
        data['model_turns'][role_name] = value
        write_task_verified(fresh, data, fresh['labels'])
        turn.clear(); turn.update(value)
    record({**turn, 'phase': 'launching'})
    def bind(process, actual_log):
        if actual_log.resolve() != log.resolve():
            raise RuntimeError('model log changed before launch')
        lease = host.bind_model(key, process.pid, process.taskq_birth, runtime)
        record({**turn, 'phase': 'bound', 'child': lease['child']})
    CONFIG['_model_launch'] = bind
    try:
        sid = effect(kind.send, resume, prompt) if resume else effect(kind.spawn, name, prompt, CONFIG['root'])
    finally:
        CONFIG.pop('_model_launch', None)
    if resume and sid != resume:
        fail('Codex native resume changed session; retain model grant')
    bound_runtime = {**runtime, 'session': sid}
    with host.transaction() as db:
        lease = host.row(db, key)
        if lease['phase'] != 'bound' or lease['runtime'] != runtime or lease['child'] != turn['child']:
            fail('model native binding changed')
        lease['runtime'] = bound_runtime
        host.put(db, key, lease)
    record({**turn, 'runtime': bound_runtime})
    item.update(parse(read_issue(item['iid'])))
    return sid

def replace(item, kind, runtime, role, running=False):
    """#568: before a replacement spawn, retire the task's earlier `role` sessions the board records (R11, #478), never
    another task's. A Codex spawn rewrites `.taskq/T<N>.pid` / `S<N>.pid`, the old thread's handle. A replaced
    worker goes running or not; a supervisor only once stopped (R11). Explicit incomplete retirement defers admission."""
    issue = BOARD.get(item['iid'])
    if not executable(issue, item):
        return False  # #576: a reassignment seen by this read: no retire, no spawn
    try:
        retired = effect(getattr(kind, 'retire', lambda *_: None), lambda n, sid=None, live=None: n == item['iid']
                                                  and recorded(issue, runtime, sid) == role, running)
        if retired is False:
            return False
    except Exception as error:
        print(f'#{item["iid"]}: could not retire the replaced {role}: {error}', file=sys.stderr)
        raise
    return True

EVENT_OF = {'result': 'review', 'ask': 'ask', 'answer': 'answer', 'gone': 'gone', 'requeue': 'requeue'}

def pending(issue, boss):
    """The supervisor's events since it last got them (.taskq/S<N>.seen, else since its spawn note): `review #N`,
    `ask #N`, `answer #N`, `gone #N` (its worker), `requeue #N` (by its worker); its own ask and requeue are no event.
    Returns them and the comment count to mark them seen with."""
    raw = issue_data(issue)
    if raw.get('event_schema'):
        events = event_pending(raw, recipient('supervisor', boss))
        return [event_line(issue['iid'], event) for event in events], [event['id'] for event in events]
    return [f'{EVENT_OF[action]} #{issue["iid"]}' for action, _ in legacy_supervisor_events(issue, boss)], len(issue.get('comments') or [])


def legacy_supervisor_events(issue, boss):
    """Legacy ordered history and existing checkout receipt; used only for compatibility/migration."""
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
            found.append((action.strip('*'), body))
    return found

def seen(n, sid, count):
    if isinstance(count, list):
        issue = read_issue(n)
        boss = issue_data(issue).get('supervisor') or {}
        acknowledge(issue, recipient('supervisor', boss), count)
        return
    (CONFIG['root'] / '.taskq').mkdir(exist_ok=True)
    (CONFIG['root'] / '.taskq' / f'S{n}.seen').write_text(f'{sid} {count}')

def runtime_state(kind, sid):
    """Minimal capability shared by workers/supervisors; legacy alive cannot prove idle."""
    state = kind.state(sid) if callable(getattr(kind, 'state', None)) else {True: 'running', False: 'dead'}.get(kind.alive(sid))
    return state if state in ('running', 'idle', 'dead') else 'unknown'


lead_state = runtime_state

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


def event_line(n, event):
    name = {'result': 'review', 'close': 'closed', 'observed-gone': 'gone'}.get(event['action'], event['action'])
    verdict = (' ' + event['text'].splitlines()[0]) if name == 'closed' and event['text'] else ''
    return f'{name} #{n}{verdict} [event {n}:{event["id"]}]'


def acknowledge(issue, target, ids):
    raw = issue_data(issue)
    if 'legacy_recovery' in raw:
        fail(f'#{issue["iid"]}: legacy recovery hold; acknowledgement refused')
    if raw.get('event_schema') == 2:
        fail('schema2 events require their native application handler; legacy ack refused')
    events = [dict(event, acks=list(event.get('acks', []))) for event in raw.get('events', [])]
    for number in ids:
        if not 0 < number <= raw.get('event_seq', 0):
            fail(f'#{issue["iid"]}: unknown event {number}')
        event = next((event for event in events if event['id'] == number), None)
        if event is None:  # acknowledged entries are compacted; sequence identities are never reused
            continue
        if target not in event['recipients'] and not (target.startswith('manager:') and 'manager:*' in event['recipients']):
            fail(f'#{issue["iid"]}:{number}: event belongs to another recipient')
        if target not in event['acks']:
            event['acks'].append(target)
        if issue['state'] == 'closed' and not raw.get('pm') and target.startswith('manager:') and 'manager:*' in event['recipients']:
            if 'manager:*' not in event['acks']:
                event['acks'].append('manager:*')
    if events != raw.get('events', []):
        raw['events'] = events
        effect(BOARD.update, issue['iid'], labels=event_labels(raw, issue['labels']), body=block(BLOCK.sub('', issue['body']).strip(), raw))


def cmd_ack(args):
    if args.pm:
        fail('sender acknowledgement refused; exact recorded recipient owns native application/handling')
    tokens = list(args.events)
    if args.stdin:
        tokens += re.findall(r'\[event (\d+:\d+)\]', sys.stdin.read())
    parsed = []
    for token in tokens:
        matched = re.fullmatch(r'(\d+):(\d+)', token)
        if not matched:
            fail(f'invalid event identity {token!r}; expected N:ID')
        issue = read_issue(int(matched[1]))
        raw = issue_data(issue)
        actor, boss, pm = session() or {}, raw.get('supervisor') or {}, raw.get('pm') or {}
        if raw.get('event_schema') == 2:
            if args.pm:
                fail('schema2 sender ack refused; native application owns acknowledgement')
            if origin() != (boss or pm) or int(matched[2]) != (raw.get('lifecycle') or {}).get('event'):
                fail('schema2 ack requires exact recorded controller and admitted application event')
            parsed.append((int(matched[1]),None,int(matched[2])))
            continue
        chosen = getattr(args, 'role', None)
        if chosen == 'manager' and actor.get('session') and origin() == pm:
            target = recipient('manager', pm)
        elif chosen != 'manager' and actor.get('session') and actor.get('session') == boss.get('session') and actor.get('runtime') == boss.get('runtime'):
            target = recipient('supervisor', boss)
        elif chosen != 'supervisor' and (not pm or actor.get('session') == pm.get('session') and actor.get('runtime') == pm.get('runtime')):
            target = recipient('manager', actor)
        else:
            fail(f'#{matched[1]}: ack requires its recorded recipient; sender delegation is refused')
        parsed.append((int(matched[1]), target, int(matched[2])))
    for n, target, number in parsed:
        if target is None:
            native_ack(n,number)
        else:
            acknowledge(read_issue(n), target, [number])



def materialize_artifact(path, content):
    """One supported idempotent action, no overwrites or arbitrary command execution."""
    if path.is_symlink():
        raise ValueError('artifact symlink refused')
    if path.exists():
        if not path.is_file() or path.read_bytes() != content:
            raise ValueError('existing artifact differs; reconcile, never overwrite')
        return False
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, 'O_NOFOLLOW', 0), 0o600)
    with os.fdopen(fd, 'wb') as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    if os.name == 'posix':
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    return True


def verify_result_notification(item, event):
    """Read-only independent acceptance check; never issue acceptance as another identity."""
    _, current, project, host, _, fingerprint, artifact_for = lifecycle_prepare(item['iid'], manager=True)
    if current != item or ReceiptOperation(current,event['id'],recipient('manager',current.get('pm'))).input['event'] != event:
        fail('result notification changed during verification')
    item=current;raw = item['raw']; life = raw.get('lifecycle') or {}
    execution=raw['execution']
    if life.get('event') != execution['event'] or life.get('artifact') != artifact_for(execution['event']) or life.get('demand') != execution['demand']:
        fail('result notification current execution differs from accepted lifecycle')
    authority = item.get('supervisor') or item.get('pm')
    if not authority or event.get('by') != authority or life.get('phase') != 'parked' \
            or (item.get('claim') or {}).get('name') != machine():
        fail('result notification requires exact same-host recorded acceptance authority and parked lifecycle')
    value = host.observe(life.get('key'))
    grant = project.observe(life.get('key'))
    criteria = raw.get('acceptance_criteria') or {}
    application = ((value or {}).get('drain') or {}).get('application') or {}
    source = next((e for e in raw.get('events', []) if e.get('id') == life.get('event')), None)
    artifact = Path(CONFIG['root']) / str(life.get('artifact', ''))
    target = recipient('worker', item['claim'])
    accepted = raw.get('acceptance_receipts', {}).get(life.get('key'))
    request = {**fingerprint,'generation':life['generation'],'age':life['age'],
               'priority':item['priority'],'demand':life['demand']}
    project_request = {**request,'demand':{k:life['demand'].get(k,0) for k in CONFIG['capacity']['project_caps']}}
    expected = {'kind':'answer-artifact','key':life.get('key'),'criteria':criteria,
                'authority':authority,'application':application}
    result = {k: expected[k] for k in ('kind','key','criteria','application')}
    if not value or value.get('phase') != 'released' or not host.drain_verified(value) \
            or value.get('request') != request or not grant or grant.get('phase') != 'released' or grant.get('request') != project_request \
            or not source or target not in source.get('recipients', []) or target not in source.get('acks', []) \
            or criteria.get('kind') != 'answer-artifact' or criteria.get('event') != life.get('event') \
            or criteria.get('artifact') != life.get('artifact') \
            or application.get('status') != 'ok' or application.get('session') != item['claim']['session'] \
            or application.get('event') != life.get('event') or application.get('artifact') != life.get('artifact') \
            or (raw.get('applications', {}).get(life.get('key')) or {}).get('proof') != application \
            or not artifact.is_file() or artifact.is_symlink() \
            or not artifact.parent.resolve().is_relative_to(Path(CONFIG['root']).resolve()) \
            or hashlib.sha256(artifact.read_bytes()).hexdigest() != criteria.get('sha256') \
            or hashlib.sha256(source['text'].encode()).hexdigest() != criteria.get('sha256') \
            or application.get('sha256') != criteria.get('sha256') or accepted != expected or raw.get('result') != result:
        fail('result notification acceptance/application/drain evidence is missing or changed')
    return accepted


def cmd_applied(args):
    """A recorded worker attests application using a scoped artifact and exact local commit."""
    matched = re.fullmatch(r'([1-9]\d*):([1-9]\d*)', args.event)
    if not matched or not re.fullmatch(r'[0-9a-f]{40}', args.sha):
        fail('applied requires N:ID and full local Git SHA')
    n, event_id = map(int, matched.groups())
    issue = read_issue(n)
    item = parse(issue)
    raw = item['raw'] if item else {}
    claim = raw.get('claim') or {}
    turn = (raw.get('model_turns') or {}).get('worker') or {}
    actor = origin()
    if not item or issue['state'] != 'open' or role(item) != 'worker' or actor != claim \
            or claim.get('runtime') != 'codex' or claim.get('name') != machine() \
            or (turn.get('runtime') or {}).get('session') != claim.get('session') \
            or event_id not in turn.get('event_ids', []) or turn.get('phase') not in ('bound', 'complete'):
        fail('applied requires exact native worker, owning host and bound answer turn')
    event = next((event for event in raw.get('events', []) if event['id'] == event_id), None)
    target = recipient('worker', claim)
    if not event or event['action'] != 'answer' or target not in event['recipients']:
        fail('applied event is not this worker answer')
    inputs = turn.get('event_inputs') or []
    if hashlib.sha256(json.dumps(inputs, sort_keys=True).encode()).hexdigest() != turn.get('event_digest') \
            or model_event_input(event) not in inputs:
        fail('answer input changed from the exact admitted turn; acknowledgement refused')
    relative = Path(args.artifact)
    root = CONFIG['root'].resolve()
    path = root / relative
    import fnmatch
    if relative.is_absolute() or '..' in relative.parts or not relative.name or path.is_symlink() \
            or not path.is_file() or not path.resolve().is_relative_to(root) \
            or not any(fnmatch.fnmatchcase(relative.as_posix(), scope) for scope in item['scope']):
        fail('applied artifact must be a real scoped file inside this task workspace')
    content = path.read_bytes()
    if len(content) > 1024 * 1024:
        fail('applied artifact exceeds 1 MiB bound')
    git = [shutil.which('git') or fail('git not found'), '-C', str(root)]
    head = subprocess.run([*git, 'rev-parse', 'HEAD'], capture_output=True, timeout=10)
    blob = subprocess.run([*git, 'show', f'{args.sha}:{relative.as_posix()}'], capture_output=True, timeout=10)
    if head.returncode or head.stdout.decode().strip() != args.sha or blob.returncode or blob.stdout != content:
        fail('applied artifact differs from exact current local Git commit')
    proof = {'worker': claim, 'turn_key': turn['key'], 'event': event_id,
             'input_sha256': hashlib.sha256(event['text'].encode()).hexdigest(),
             'artifact': relative.as_posix(), 'sha256': hashlib.sha256(content).hexdigest(), 'git_sha': args.sha}
    receipts = dict(raw.get('application_receipts') or {})
    previous = receipts.get(str(event_id))
    if previous:
        if previous != proof or target not in event['acks']:
            fail('applied receipt changed or acknowledgement missing; replay refused')
        print(json.dumps({'event': args.event, 'applied': True, 'repeated': True}))
        return
    if read_issue(n) != issue:
        fail('task changed before applied receipt; preserve newer state')
    receipts[str(event_id)] = proof
    raw['application_receipts'] = receipts
    event['acks'] = list(event['acks']) + ([target] if target not in event['acks'] else [])
    write_task_verified(issue, raw, event_labels(raw, issue['labels']))
    print(json.dumps({'event': args.event, 'applied': True, 'repeated': False}))


def cmd_apply_event(args):
    matched = re.fullmatch(r'([1-9]\d*):([1-9]\d*)', args.event)
    if not matched:
        fail('apply-event requires N:ID')
    n, event_id = map(int, matched.groups())
    issue = read_issue(n)
    raw = issue_data(issue)
    item, actor = parse(issue), session() or {}
    manager = args.role == 'manager'
    claim = raw.get('pm' if manager else 'claim') or {}
    if raw.get('event_schema') != 2 or not item or issue['state'] != 'open' or (not manager and item['state'] not in ('doing', 'ask')):
        fail('apply-event requires an open doing/ask schema2 task; no implicit migration')
    reason = schema2_execution_reason(item)
    if reason:
        fail(reason)
    if claim.get('session') != actor.get('session') or claim.get('runtime') != actor.get('runtime') \
            or not actor.get('session') or claim.get('name') != machine() or not mine(item) or not host_scope(item):
        fail('apply-event requires the exact recorded recipient on its owning host')
    target = recipient('manager' if manager else 'worker', claim)
    op = ReceiptOperation(item, event_id, target)
    action = op.input['event']['action']
    if action not in (('ask', 'result') if manager else ('answer',)):
        fail('this role supports only ask/result notifications' if manager else 'only answer artifact materialization is qualified by this handler')
    acceptance = verify_result_notification(item, op.input['event']) if action == 'result' else None
    relative = Path(args.artifact)
    root = CONFIG['root'].resolve()
    if relative.is_absolute() or '..' in relative.parts or not relative.name:
        fail('artifact must be a relative path without parent traversal')
    path = root / relative
    if not path.parent.is_dir() or not path.parent.resolve().is_relative_to(root) or path.is_symlink():
        fail('artifact parent must exist within the task workspace; symlinks refused')
    content = op.input['event']['text'].encode('utf-8')
    if len(content) > 1024 * 1024:
        fail('answer artifact exceeds the bounded 1 MiB action')
    digest = hashlib.sha256(content).hexdigest()
    evidence = {'source': 'native-manager-notification-artifact' if manager else 'native-answer-artifact', 'path': relative.as_posix(), 'sha256': digest}
    if acceptance:
        evidence['acceptance_sha256'] = hashlib.sha256(json.dumps(acceptance, sort_keys=True).encode()).hexdigest()
    def verifier(request, receipt):
        expected_actor = op.acceptor if request['stage'] == 'acceptance' else target
        if receipt.get('evidence') != evidence or request['actor'] != expected_actor:
            return False
        if acceptance:
            current = parse(read_issue(n))
            if not current or ReceiptOperation(current, event_id, target).id != op.id \
                    or verify_result_notification(current, op.input['event']) != acceptance:
                return False
        return request['stage'] != 'application' or path.is_file() and not path.is_symlink() and path.read_bytes() == content
    operations = raw.get('operations', {})
    if not isinstance(operations, dict):
        fail('malformed native operation map')
    saved = operations.get(op.id)
    if saved:
        if saved.get('artifact') != relative.as_posix():
            fail('operation artifact path changed; replay refused')
        op = ReceiptOperation.restore(saved['receipt'], item, verifier)
    def record(ack=False):
        fresh = read_issue(n)
        current = parse(fresh)
        if not current or fresh['state'] != 'open' or (not manager and current['state'] not in ('doing', 'ask')) \
                or ReceiptOperation(current, event_id, target).id != op.id:
            fail('task changed before native receipt write')
        if acceptance and verify_result_notification(current,op.input['event']) != acceptance:
            fail('result acceptance changed before native receipt write')
        payload = issue_data(fresh)
        payload.setdefault('operations', {})[op.id] = {'artifact': relative.as_posix(), 'receipt': op.snapshot()}
        if ack:
            event = next(e for e in payload['events'] if e['id'] == event_id)
            if target not in event['acks']:
                event['acks'].append(target)
        write_task_verified(fresh,payload,event_labels(payload,fresh['labels']))
    if not saved:
        for stage in ('delivery', 'handling'):
            op.receipt({**op.request(), 'status': 'ok', 'evidence': evidence}, verifier)
        record()  # durable intent before an external artifact effect
    if op.phase != 'complete':
        effect(materialize_artifact, path, content)
        op.receipt({**op.request(), 'status': 'ok', 'evidence': evidence}, verifier)
        if action == 'result':
            op.receipt({**op.request(), 'status': 'ok', 'evidence': evidence}, verifier)
        record(ack=True)
    print(json.dumps({'operation': op.id, 'phase': op.phase, 'artifact': relative.as_posix(), 'sha256': digest, 'task_accepted': False}))


def legacy_worker_message(issue, claim, supervised):
    return next(legacy_worker_messages(issue, claim, supervised), None)


def legacy_worker_messages(issue, claim, supervised):
    """Relevant signals only: ordinary comments cannot hide an answer or a nudge."""
    comments = issue.get('comments') or []
    cursor = issue_data(issue).get('worker_comment_cursor', {}).get(claim.get('session'), -1)
    if cursor < 0 and not issue_data(issue).get('event_schema'):
        cursor = max((i for i, text in enumerate(comments) if text.startswith('**nudge**')
                      and re.search(r'\n\nworker ' + re.escape(claim.get('session') or '') + r'(?:\s|$)', text)), default=-1)
    for i, text in enumerate(comments[cursor + 1:], cursor + 1):
        if text.startswith('**answer**') and not issue_data(issue).get('event_schema'):
            yield 'answer', text.partition('\n\n')[2], i
        if supervised and text.startswith('nudge:'):
            yield 'nudge', text.partition('nudge:')[2].strip(), i

def follow(item, kind, claim, supervised):
    """A live worker: an answer, a supervisor's `nudge:` comment or 120 silent minutes reach it once (step 2, step 4).
    None: this read shows the task ineligible (#576): the caller ends the task's step."""
    issue = read_issue(item['iid'])
    if not executable(issue, item):
        return None  # #576: a reassignment seen by this read: no send, the claim stays
    if (model_role_state(item, 'worker') or lead_state(kind, claim['session'])) != 'idle':
        return claim  # busy/unknown: no comment history fetch and no consumption
    raw = issue_data(issue)
    old = claim['session']
    target = recipient('worker', claim)
    events = event_pending(raw, target)
    if not events:
        issue = issue_history(issue)
    legacy = legacy_worker_message(issue, claim, supervised) if not events else None
    answer = '\n\n'.join(event['text'] for event in events) if events else legacy[1] if legacy and legacy[0] == 'answer' else None
    told = legacy[1] if legacy and legacy[0] == 'nudge' else None
    if answer is None and told is None and age(item) < 120:
        return claim
    text = f'The owner answered your question:\n\n{answer}' if answer is not None else told or 'continue: read your issue'
    if isinstance(kind, Codex):
        text += '\n\nApplication event IDs: ' + ', '.join(f'{item["iid"]}:{e["id"]}' for e in events)
        text += '\nDelivery is not acknowledgement. After applying the answer, commit your scoped artifact locally and run taskq applied N:ID --artifact RELPATH --sha FULLSHA before result. Repeat the same applied command to verify idempotence.'
        sid = owned_model_turn(item, kind, 'worker', text, events, resume=old)
        if not sid:
            return claim
        claim = {**claim, 'session': sid}
        raw = issue_data(read_issue(item['iid']))
    else:
        claim = {**claim, 'session': effect(kind.send, old, text)}
    if events and not isinstance(kind, Codex):
        for event in raw['events']:
            if event['id'] in [value['id'] for value in events]:
                event['acks'] = list(event.get('acks', [])) + [target]
    cursor = dict(raw.get('worker_comment_cursor') or {})
    if old in cursor:
        cursor[claim['session']] = cursor[old]  # a resumed copy is the same worker's delivery history
    if legacy and not events:
        cursor[claim['session']] = legacy[2]
    item['raw'] = raw
    move(item, item['state'], 'nudge', f'worker {claim["session"]}' + (f' replaces {old}' * (old != claim['session'])),
         claim=claim, worker_comment_cursor=cursor)
    item['claim'] = claim  # the nudge comment is now the last note: one send per answer
    return claim

def supervise(item, kinds, worker_allowed=True):
    """§ 7 step 4, a supervised task whose supervisor runs here: the pass is its hands, never its judge."""
    n, boss, claim = item['iid'], item['supervisor'], item['claim'] or {}
    lead = kinds.get(boss['runtime'])
    if lead is None:
        if boss['runtime'] == 'hermes':
            fail('Hermes native supervisor bridge is not configured')
        return
    fresh = executable(read_issue(n), item)  # #532: the list may lag a spawn; #545: or an assignee-only label or reassignment
    if not fresh:
        return  # ineligible: sessions, claim and order stay as they are
    admitted = True
    if boss['runtime'] == 'hermes':
        state = lead_state(lead, boss['session'])  # prove bridge/state before admitting any worker action
        admitted = state != 'dead'
    if admitted and worker_allowed and item['raw'].get('order') and claim.get('runtime') in kinds and fresh['raw'].get('order') and not (fresh['claim'] or {}).get('session'):  # run or a rework requeue: a new worker on branch taskq-<N> (#291)
        kind = kinds[claim['runtime']]
        if not replace(item, kind, claim['runtime'], 'worker', running=True):
            return
        sid = spawn_named(item, kind, 'T', brief(item, claim['runtime']))
        if not sid:
            return
        item['claim'], item['raw']['order'] = {**claim, 'session': sid}, None
        move(item, 'doing', 'spawn', note('worker', sid, kind), claim=item['claim'], order=None)
    elif admitted and item['state'] == 'doing' and claim.get('session') and claim.get('runtime') in kinds:
        kind = kinds[claim['runtime']]
        live = model_role_state(item, 'worker') or runtime_state(kind, claim['session'])
        if live == 'dead':  # the supervisor decides: rework or ask
            item['claim'] = {**claim, 'session': None}
            move(item, 'doing', 'gone', f'worker {claim["session"]} is gone', claim=item['claim'])
        elif live in ('running', 'idle'):
            if follow(item, kind, claim, True) is None:
                return  # #576: no supervisor send, resume or respawn either
    state = model_role_state(item, 'supervisor') or lead_state(lead, boss['session'])
    issue = read_issue(n)
    if not executable(issue, item):
        return  # #545: a reassignment seen by this second read stops every supervisor send, resume and respawn
    if state == 'dead':
        evidence = tail_of(lead, boss['session']) or 'no log'
        deaths = item['raw'].get('retry_counts', {}).get('supervisor')
        if deaths is None:
            deaths = lead_deaths(issue_history(issue)['comments'])
        if deaths:  # second death since result/answer
            move(item, 'ask', 'ask', f'supervisor {boss["session"]} is gone again: fix the runtime, then answer.\n\nLast log line: {evidence}')
            return
        move(item, item['state'], 'gone', f'supervisor {boss["session"]} is gone: {evidence}')
        if getattr(lead, 'resumable', lambda _: False)(boss['session']):  # Codex: the same thread, the same id
            text = f'restart #{n}: your last turn ended {evidence}; read your issue'
            owned_model_turn(item, lead, 'supervisor', text, resume=boss['session']) if isinstance(lead, Codex) else effect(lead.send, boss['session'], text)
        else:  # a new supervisor adopts the live worker from the board; the dead one is retired first
            if not replace(item, lead, boss['runtime'], 'supervisor'):
                return
            sid = spawn_named(item, lead, 'S', supervisor_brief(item, boss['runtime'], lead))
            if not sid:
                return
            item['supervisor'] = {**boss, 'session': sid}
            move(item, item['state'], 'spawn', note('supervisor', sid, lead), supervisor=item['supervisor'])
    elif state == 'idle':
        found, count = pending(issue if issue_data(issue).get('event_schema') else issue_history(issue), boss)
        if found:  # running: its own wait or turn-end pass delivers events; idle: resume with events
            text = f'{" ".join(found)}: read your issue'
            if isinstance(lead, Codex):
                events = event_pending(issue_data(issue), recipient('supervisor', boss))
                sid = owned_model_turn(item, lead, 'supervisor', text, events, resume=boss['session'])
                if not sid:
                    return
            else:
                sid = effect(lead.send, boss['session'], text)
                seen(n, sid, count)
            item['raw'] = issue_data(read_issue(n))
            if sid != boss['session']:  # Claude resumes under a new id (#284): record it; the old one is refused and retired
                item['supervisor'] = {**boss, 'session': sid}
                move(item, item['state'], 'nudge', f'supervisor {sid} replaces {boss["session"]}', supervisor=item['supervisor'])

def age(item):
    """Minutes since the issue last changed: a comment changes it too."""
    changed = datetime.fromisoformat((item['updated_at'] or '').replace('Z', '+00:00'))
    return (datetime.now(timezone.utc) - changed).total_seconds() / 60

def quick_deaths(n):
    """'requeue ... is gone' notes since the last result or answer."""
    issue = BOARD.get(n)
    raw = issue_data(issue)
    if 'worker' in raw.get('retry_counts', {}):
        return raw['retry_counts']['worker']
    count = 0
    for text in reversed(issue['comments'] or []):
        if text.startswith(('**result**', '**answer**')):
            break
        count += text.startswith('**requeue**') and ' is gone' in text
    return count

def cmd_tick(args, table=True):
    """One event, one fresh pass after bounded board-guard acquisition."""
    try:
        one_pass(args, table)
    except SystemExit as error:
        if table and not getattr(args, 'headless', False) and getattr(args, 'diagnose', False) and 'project guard' in str(error):
            print('Проблема проекта: guard — операция отказала; stop/drain и сверка исходов, без снятия блокировки.')
            cmd_status(args)
        raise


def dispatch_manager(args):
    """Explicit project delegation, not a fabricated native session or manager adoption."""
    if not getattr(args, 'headless', False):
        return None
    value = CONFIG.get('dispatch')
    if not isinstance(value, dict) or set(value) != {'pm', 'hosts', 'board_user'}:
        fail('headless tick requires explicit project dispatch authority')
    pm, hosts = value['pm'], value['hosts']
    if not isinstance(pm, dict) or set(pm) != {'runtime', 'session', 'name'} \
            or pm.get('runtime') != 'codex' or any(not isinstance(v, str) or not v.strip() for v in pm.values()) \
            or not isinstance(hosts, list) or not hosts \
            or any(not isinstance(h, str) or not h.strip() for h in hosts) \
            or len(hosts) != len(set(hosts)) or machine() not in hosts:
        fail('headless dispatch PM/participating host identity invalid')
    if not isinstance(value['board_user'], str) or not value['board_user'].strip() \
            or BOARD.user() != value['board_user']:
        fail('headless dispatch authenticated board user mismatch')
    return pm


def direct():
    """R6 (#521, #574): the client that finally renders the report: TASKQ_CLIENT, else the session running the command.
    Codex: direct codex:// links; Claude, DOT, a shell or anything else: the https wrapper.
    ponytail: CODEX_THREAD_ID cannot tell the Codex app from the CLI or IDE."""
    return (os.environ.get('TASKQ_CLIENT') or (session() or {}).get('runtime')) == 'codex'

def lead(item, kinds, admit=False):
    """R3 (#532): the supervisor follows the task's own manager (DOT: Codex); None: no manager can start it here."""
    runtime = (item['pm'] or {}).get('runtime')
    if runtime == 'hermes' and admit:
        if not item['pm'].get('session'):
            fail('Hermes manager needs genuine HERMES_SESSION_ID')
        if runtime not in kinds:
            fail('Hermes native supervisor bridge is not configured')
        kinds[runtime].check()
    runtime = {'dot': 'codex'}.get(runtime, runtime)
    return runtime if runtime in kinds else None

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
        fresh = parse(issues[n]) if issues[n] and issues[n]['state'] == 'open' else None
        if fresh and sid in ((fresh['claim'] or {}).get('session'), (fresh['supervisor'] or {}).get('session')):
            return False  # a fresh controller/worker is current even when the pass's earlier read held another id
        return bool(issues[n] and host_scope(issues[n]) and recorded(issues[n], name, sid))
    return gone

def row(item, kinds, here, state=None):
    """R6 (#489, #574): one markdown row, `[#N <title>](issue)` and `[<session[:8]>](link)`; a session with no link here stays
    plain text. `state`: the report's state of the task (`blocked (no manager)`, ...), else its label."""
    claim = item['claim'] if (item['claim'] or {}).get('session') else item.get('supervisor') or item['claim'] or {}  # no worker yet: its supervisor
    runtime, session = claim.get('runtime') or item['runtime'], claim.get('session') or ''
    url = session and claim.get('name') == here and runtime in kinds and kinds[runtime].link(session)
    if url and runtime == 'codex' and direct():  # #521: Codex opens its own thread link; the wrapper only loads a page first
        url = f'codex://threads/{session}'
    cell = f'[{session[:8]}]({url})' if url else session and f'{session[:8]} on {claim.get("name")}'
    return f'| {heading(item)} | {state or item["state"]} | {runtime} | {cell} |'

def cell(text):
    """R6 (#574): one table cell and one link text: a newline becomes a space; `|`, `[`, `]` and a backslash are escaped."""
    return re.sub(r'([\\[\]|])', r'\\\1', ' '.join(str(text).split()))

def heading(item):
    """`[#N <title>](url)`, the title cut to 60 (R6, #574)."""
    title = ' '.join((item.get('title') or '').split())
    text = f'#{item["iid"]} {cell(title if len(title) <= 60 else title[:59] + "…")}'.strip()
    return f'[{text}]({item["url"]})' if item.get('url') else text

def one_pass(args, table=True):
    """One pass: free waiting tasks, follow unsupervised workers, act for supervisors, spawn ready tasks' supervisors, print the table."""
    here, kinds = machine(), runtimes()
    delegated_pm = dispatch_manager(args)
    table = table and delegated_pm is None
    local = local_limits()
    limits = local if local is not None else CONFIG.get('limits') or {name: 1 for name in kinds}
    blind = bool(os.environ.get('CODEX_SANDBOX'))
    with contextlib.nullcontext() if blind else coordination():  # a sandbox pass remains read-only
        held = not blind
        tasks = getattr(args, 'tasks', None) or []  # #481: the list lags the event's own write (a new issue, a label): read those directly
        issues = [issue for issue in BOARD.list(None) if issue['iid'] not in tasks] + [issue for issue in map(read_issue, tasks) if issue['state'] == 'open']
        if held:
            board_schema_gate(issues)
        items = sorted(filter(None, map(parse, issues)), key=lambda item: (item['priority'], item['iid']))
        if delegated_pm is not None:
            items = [item for item in items if item['pm'] == delegated_pm]
        blind = bool(os.environ.get('CODEX_SANDBOX'))  # #502: a sandbox sees no other session alive: it would requeue live workers as gone
        if blind:
            print('taskq: inside a Codex sandbox: the pass only prints the table', file=sys.stderr)
        busy, ready = {}, items if held and not blind else []
        if ready:
            # Admission/accounting use fresh reservations, including work outside this host-label scope.
            # A lagging list can otherwise hide an active worker or overwrite a claim moved to another machine.
            fresh_issues = [read_issue(item['iid']) for item in ready]
            items = ready = sorted(filter(None, (parse(issue) for issue in fresh_issues if issue['state'] == 'open')),
                                   key=lambda item: (item['priority'], item['iid']))
            if delegated_pm is not None:
                items = ready = [item for item in ready if item['pm'] == delegated_pm]
        for item in ready:
            if 'legacy_recovery' in item['raw']:
                continue
            if item['raw'].get('event_schema') != 2 and compatibility_reason(item):
                fail(compatibility_reason(item))
            if item['raw'].get('model_turns'):
                model_reconcile(item)
        model_mode = isinstance(CONFIG.get('capacity'), dict) and 'model:codex' in CONFIG['capacity'].get('host_caps', {})
        worker_slots, occupied = set(), {}
        if local is not None:  # existing active workers consume capacity, even outside this invocation's host scope
            for item in ready:
                claim = item['claim'] or {}
                if model_mode:
                    for turn in (item['raw'].get('model_turns') or {}).values():
                        if turn['phase'] != 'complete':
                            occupied['codex'] = occupied.get('codex', 0) + 1
                elif item['state'] in ('doing', 'review', 'ask') and claim.get('name') == here and claim.get('session'):
                    name = claim.get('runtime')
                    occupied[name] = occupied.get(name, 0) + 1
            for item in ready:  # pending reservations retain their board claims; admit only within remaining capacity
                claim = item['claim'] or {}
                name = claim.get('runtime')
                if item['supervisor'] and item['state'] in ('doing', 'review', 'ask') and claim.get('name') == here \
                        and not claim.get('session') and (occupied.get(name, 0) < limits.get(name, 0) or model_mode \
                        and (item['raw'].get('model_turns') or {}).get('worker', {}).get('phase') == 'reserving'):
                    worker_slots.add(item['iid'])
                    occupied[name] = occupied.get(name, 0) + 1
        for item in ready:
            claim = item['claim'] or {}
            if 'legacy_recovery' in item['raw']:
                continue
            if item['raw'].get('event_schema') == 2:
                queue_pass(item)
                if claim.get('name') == here and claim.get('session') and item['state'] in ('doing','review','ask'):
                    busy[claim.get('runtime')] = busy.get(claim.get('runtime'),0)+1
                continue
            if not host_scope(item):
                if item['state'] in ('doing', 'review', 'ask') and claim.get('name') == here:
                    busy[claim.get('runtime')] = busy.get(claim.get('runtime'), 0) + 1
                continue
            if getattr(args, 'unknown_after', None) is not None and recovery_question(item, kinds, here, args.unknown_after):
                if item['state'] in ('doing', 'ask') and claim.get('name') == here:
                    busy[claim.get('runtime')] = busy.get(claim.get('runtime'), 0) + 1
                continue
            if item['state'] == 'waiting' and not open_deps(item['deps']):
                if not executable(read_issue(item['iid']), item):
                    continue
                move(item, 'ready', 'ready', 'dependencies closed')
                item['state'] = 'ready'
            if item['supervisor']:  # step 4; the worker's slot is held from the supervisor's spawn to close
                if item['supervisor'].get('name') == here:
                    supervise(item, kinds, model_mode or local is None or item['iid'] in worker_slots)
                if item['state'] in ('doing', 'review', 'ask') and claim.get('name') == here:
                    busy[claim.get('runtime')] = busy.get(claim.get('runtime'), 0) + 1
                continue
            if item['state'] != 'doing' or claim.get('name') != here or claim.get('runtime') not in kinds:
                continue  # another machine's, or a session no runtime here can see
            if not executable(read_issue(item['iid']), item):  # #545: the list's labels may lag; an ineligible task keeps its session and slot
                busy[claim['runtime']] = busy.get(claim['runtime'], 0) + 1
                continue
            runtime = kinds[claim['runtime']]  # step 2: unsupervised (R3 Transition: started before #525, or taken by hand)
            state = model_role_state(item, 'worker') or runtime_state(runtime, claim['session'])
            if state == 'dead':
                gone = f'session {claim["session"]} is gone'
                if quick_deaths(item['iid']):  # #393: the second death in a row with no result or answer asks, not respawns
                    move(item, 'ask', 'ask', f'{gone} again, the worker dies at once: fix the runtime, then answer.\n\n'
                                             f'Last log line: {tail_of(runtime, claim["session"]) or "none"}')
                    item['state'] = 'ask'
                    continue
                move(item, 'ready', 'requeue', gone, claim=None, result=None)
                item.update(state='ready', claim=None)
                continue
            if state in ('running', 'idle'):
                claim = follow(item, runtime, claim, False) or claim  # denied: the original claim keeps its slot
            busy[claim['runtime']] = busy.get(claim['runtime'], 0) + 1
        for item in ready:
            if 'legacy_recovery' in item['raw']:
                continue
            if item['raw'].get('event_schema') == 2 or item['state'] != 'ready' or not host_scope(item) or item['host'] not in (None, here) or not mine(item) or open_deps(item['deps']) \
                    or (item['pm'] and item['pm'].get('name') != here and item['pm'] != delegated_pm) or not lead(item, kinds, admit=True):
                continue  # no manager: the task waits, the table says so; another machine's manager: that machine starts it (§ 7 step 3)
            names = [item['runtime']] if item['runtime'] != 'any' else list(limits)
            free = next((name for name in names if name in kinds and limits.get(name, 0 if local is not None else 1) > 0
                         and (model_mode and name == 'codex' or busy.get(name, 0) < limits.get(name, 0 if local is not None else 1))), None)
            fresh = free and executable(read_issue(item['iid']), item)  # #357: the board may have moved since the list; #545: eligibility
            if fresh and fresh['state'] == 'ready' and fresh['pm'] == item['pm']:  # the tick claims the slot for the worker; the supervisor orders the worker (`run`)
                if local is not None and fresh['runtime'] not in ('any', free):
                    continue  # a stale list must not select a different or disabled worker runtime
                item.update(fresh)
                runtime = lead(item, kinds)
                if not replace(item, kinds[runtime], runtime, 'supervisor'):  # a requeued task's old S<N>
                    continue
                session = spawn_named(item, kinds[runtime], 'S', supervisor_brief(item, runtime, kinds[runtime]))
                if not session:
                    continue
                item.update(supervisor={'runtime': runtime, 'session': session, 'name': here}, claim={'runtime': free, 'session': None, 'name': here})
                move(item, 'doing', 'spawn', note('supervisor', session, kinds[runtime]), supervisor=item['supervisor'], claim=item['claim'],
                     result=None, order=None)
                item['state'], busy[free] = 'doing', busy.get(free, 0) + 1
        if held and not blind and delegated_pm is None and not any('legacy_recovery' in item['raw'] for item in items):
            retire(stale({item['iid']: item for item in items}), 'could not remove stopped sessions', running=False)
    if table:
        if getattr(args, 'diagnose', False):
            cmd_status(args)
        else:
            report(items, {issue['iid'] for issue in issues}, kinds, here)
    return held

def cmd_status(args):
    """`taskq status` (R6, #574): the report with no pass: one board list; no pull, write, dispatch or session."""
    try:
        issues = BOARD.list(None)
    except (Exception, SystemExit):
        if not getattr(args, 'diagnose', False):
            raise
        print(f'{CONFIG["root"].name}: доска недоступна; fresh-read-unavailable. PM: проверить доступ через штатный маршрут.\nMode: events · arm: <arm_tick>')
        return
    items = sorted(filter(None, map(parse, issues)), key=lambda item: (item['priority'], item['iid']))
    kinds, here = runtimes(), machine()
    problems = None
    if getattr(args, 'diagnose', False):
        report_guard()
        items, problems = observe_obligations(items, kinds, here, {issue['iid'] for issue in issues})
    report(items, {issue['iid'] for issue in issues}, kinds, here, problems)


class ReceiptOperation:
    """Dormant native receipt reducer; callers must persist on board and qualify the verifier separately."""
    def __init__(self, item, event_id, target):
        events = (item.get('raw') or {}).get('events', [])
        found = [event for event in events if event.get('id') == event_id]
        if type(item.get('iid')) is not int or item['iid'] <= 0 or type(event_id) is not int or event_id <= 0 or len(found) != 1:
            raise ValueError('one exact task/event required')
        event = found[0]
        targets = event.get('recipients')
        identities = [('worker', item.get('claim')), ('supervisor', item.get('supervisor')), ('manager', item.get('pm'))]
        current = [recipient(role, identity) for role, identity in identities
                   if isinstance(identity, dict) and all(isinstance(identity.get(key), str) and identity[key]
                                                        for key in ('runtime', 'session', 'name'))]
        if type(event.get('id')) is not int or not isinstance(targets, list) or any(not isinstance(t, str) for t in targets) \
                or not isinstance(target, str) or target not in targets or target not in current:
            raise ValueError('one exact recorded recipient required')
        if not isinstance(event.get('text'), str) or not isinstance(event.get('action'), str):
            raise ValueError('malformed event payload')
        self.input = json.loads(json.dumps({
            'repo': CONFIG.get('repo') or str(CONFIG['root']), 'task': item['iid'], 'target': target,
            'event': {key: event.get(key) for key in ('id', 'action', 'text', 'by', 'recipients')},
            'identities': {key: item.get(key) for key in ('pm', 'supervisor', 'claim')}}))
        self.id = hashlib.sha256(json.dumps(self.input, sort_keys=True, ensure_ascii=False).encode()).hexdigest()
        self.stages = ('delivery', 'handling', 'application')
        self.acceptor = None
        if event['action'] == 'result':
            identity = item.get('supervisor') or item.get('pm')
            if not identity or not identity.get('session'):
                raise ValueError('result needs exact recorded acceptance authority')
            self.acceptor = recipient('supervisor' if item.get('supervisor') else 'manager', identity)
            self.stages += ('acceptance',)
        self.receipts, self.phase = [], 'pending'

    def request(self):
        if self.phase in ('blocked', 'complete'):
            raise ValueError('operation has no pending stage')
        stage = self.stages[len(self.receipts)]
        return {'operation': self.id, 'stage': stage, 'target': self.input['target'],
                'actor': self.acceptor if stage == 'acceptance' else self.input['target']}

    def receipt(self, value, verifier):
        if not isinstance(value, dict) or not callable(verifier):
            raise ValueError('qualified verifier and structured receipt required')
        receipt = json.loads(json.dumps(value))
        prior = next((r for r in self.receipts if r.get('stage') == receipt.get('stage')), None)
        if prior:
            if receipt != prior:
                raise ValueError('conflicting duplicate receipt')
            return False  # no permission to repeat the external effect
        request = self.request()
        if any(receipt.get(key) != expected for key, expected in request.items()):
            raise ValueError('wrong identity/operation or stage out of order')
        if receipt.get('status') not in ('ok', 'unknown', 'declined') or not isinstance(receipt.get('evidence'), dict):
            raise ValueError('malformed receipt status/evidence')
        # Authentication belongs to the qualified adapter, never a self-declared source or boolean in JSON.
        if verifier(json.loads(json.dumps(request)), json.loads(json.dumps(receipt))) is not True:
            raise ValueError('receipt not authenticated by adapter')
        self.receipts.append(receipt)
        self.phase = ('blocked' if receipt['status'] != 'ok' else
                      'complete' if len(self.receipts) == len(self.stages) else 'pending')
        return True

    def snapshot(self):
        return json.loads(json.dumps({'schema': 1, 'input': self.input, 'operation': self.id,
                                     'receipts': self.receipts, 'phase': self.phase}))

    @classmethod
    def restore(cls, saved, item, verifier):
        if not isinstance(saved, dict) or saved.get('schema') != 1 or not isinstance(saved.get('input'), dict):
            raise ValueError('unsupported operation checkpoint')
        source = saved['input']
        op = cls(item, (source.get('event') or {}).get('id'), source.get('target'))
        if source != op.input or saved.get('operation') != op.id or not isinstance(saved.get('receipts'), list):
            raise ValueError('stale operation checkpoint')
        for value in saved['receipts']:
            op.receipt(value, verifier)
        if saved != op.snapshot():
            raise ValueError('checkpoint differs from authenticated replay')
        return op


def capacity_reserve(leases, caps, key, request):
    """One reservation reducer for native project anchors and atomic host transactions."""
    demand = request.get('demand') if isinstance(request, dict) else None
    if not isinstance(key, str) or not key or not isinstance(demand, dict) or not demand \
            or any(k not in caps or type(v) is not int or v < 0 or v > caps[k] for k,v in demand.items()) \
            or not any(demand.values()) or type(request.get('age')) not in (int, float) \
            or not 0 <= request['age'] < float('inf') or type(request.get('priority')) is not int:
        raise ValueError('invalid or unsatisfiable resource request')
    value = leases.get(key)
    if value and value['request'] != request:
        raise ValueError('reservation identity reused for a different request')
    if value and value['phase'] != 'waiting':
        return value
    if not value:
        ticket = 1 + max((v.get('ticket', 0) for v in leases.values()), default=0)
        value = leases[key] = {'request': request, 'phase': 'waiting', 'ticket': ticket}
    used = {r: sum(v['request']['demand'].get(r,0) for v in leases.values()
                   if v['phase'] not in ('waiting','released')) for r in caps}
    waiting = sorted(((k,v) for k,v in leases.items() if v['phase']=='waiting'),
                     key=lambda pair:(pair[1]['request']['priority'],pair[1]['ticket'],pair[1]['request']['age'],pair[0]))
    first = next((k for k,v in waiting if all(used[r]+v['request']['demand'].get(r,0)<=cap for r,cap in caps.items())),None)
    if first == key:
        value['phase'] = 'reserved'
    return value


class BoardCapacity:
    """Project-local preprovisioned issue; acknowledged native guard serializes every operation."""
    marker = re.compile(r'<!-- taskq:capacity -->\s*```json\s*(.*?)\s*```\s*<!-- /taskq:capacity -->', re.S)
    scope = 'project'

    def __init__(self, board, caps):
        self.board, self.caps, self.owner = board, caps, board.repo
        self.n = board.options.get('capacity_issue')
        project_id = board.options.get('capacity_project_id')
        host = board.host.lower().rstrip('.') if isinstance(board.host,str) else ''
        if not host or not re.fullmatch(r'[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?',host):
            raise ValueError('native capacity requires explicit server host; CLI defaults are not an authority')
        if type(self.n) is not int or self.n <= 0 or type(project_id) is not int or project_id <= 0 \
                or not isinstance(caps,dict) or not caps or any(not isinstance(k,str) or not k
                    or type(v) is not int or v < 0 for k,v in caps.items()):
            raise ValueError('explicit provisioned capacity_issue/project_id and finite caps required')
        self.guarded()
        metadata = board.api('GET','')
        if not isinstance(metadata,dict) or metadata.get('id') != project_id:
            raise ValueError('native capacity project identity mismatch')
        self.domain = {'board':'gitlab' if isinstance(board,GitLab) else 'github',
                       'host':host, 'project_id':project_id}

    def guarded(self):
        held = getattr(GUARD,'held',None)
        if not held or held['board'] is not self.board or held['pid'] != os.getpid() \
                or held['identity'] != origin() or held['poisoned']:
            raise ValueError('native project capacity requires this controller project guard')

    def load(self):
        self.guarded()
        issue = self.board.metadata(self.n)
        body = issue.get('body') or ''
        matches = list(self.marker.finditer(body))
        if issue.get('iid') != self.n or issue.get('state') != 'open' or len(matches) != 1 \
                or BLOCK.search(body) or any(l.startswith(PREFIX) for l in issue.get('labels',[])) or len(body.encode()) > 1024*1024:
            raise ValueError('capacity anchor must be one open non-task provisioned issue')
        value = json.loads(matches[0].group(1))
        expected = {'schema':1,'domain':self.domain,'caps':self.caps}
        if not isinstance(value,dict) or any(value.get(k)!=v for k,v in expected.items()) \
                or not isinstance(value.get('leases'),dict) or set(value) != {*expected,'leases'}:
            raise ValueError('capacity anchor ownership/caps/schema mismatch')
        for key,lease in value['leases'].items():
            if not isinstance(key,str) or not key or not isinstance(lease,dict) or set(lease)!={'request','phase','ticket'} \
                    or lease['phase'] not in ('waiting','reserved','released') or type(lease['ticket']) is not int or lease['ticket'] <= 0:
                raise ValueError('malformed native project lease')
            # Validate demand and immutable request without changing the stored lease.
            capacity_reserve({},self.caps,key,lease['request'])
            if lease['request'].get('project_domain') != self.domain or lease['request'].get('repo') != self.owner:
                raise ValueError('foreign reservation project domain')
        tickets = [lease['ticket'] for lease in value['leases'].values()]
        if len(tickets) != len(set(tickets)) or any(sum(lease['request']['demand'].get(resource,0)
                for lease in value['leases'].values() if lease['phase']=='reserved') > cap
                for resource,cap in self.caps.items()):
            raise ValueError('capacity anchor overcommitted or duplicate FIFO tickets; reconciliation required')
        return issue,value,matches[0]

    def observe(self,key):
        return self.load()[1]['leases'].get(key)

    def change(self,key,request=None):
        issue,value,match = self.load()
        before = json.dumps(value,sort_keys=True)
        if request is not None:
            if not isinstance(request,dict) or request.get('project_domain') != self.domain or request.get('repo') != self.owner:
                raise ValueError('foreign reservation project domain')
            result = capacity_reserve(value['leases'],self.caps,key,request)
        else:
            result = value['leases'].get(key)
            if not result:
                raise ValueError('unknown project grant cannot be released')
            result['phase']='released'
        if json.dumps(value,sort_keys=True) != before:
            replacement = '<!-- taskq:capacity -->\n```json\n'+json.dumps(value,sort_keys=True,ensure_ascii=False)+'\n```\n<!-- /taskq:capacity -->'
            body = issue['body'][:match.start()]+replacement+issue['body'][match.end():]
            if len(body.encode()) > 1024*1024:
                raise ValueError('capacity anchor full; explicit settled-lease compaction required')
            def write_verified():
                self.board.update(self.n,body=body)
                if self.load()[1] != value:
                    raise RuntimeError('capacity write readback unknown/conflicting; retain project guard')
            effect(write_verified)
        return json.loads(json.dumps(result))

    def reserve(self,key,request):
        return self.change(key,request)

    def release(self,key):
        return self.change(key)


def model_turn_receipt(runtime, child):
    """Model concurrency only: a terminal owned CLI turn is not resource/application drain."""
    if not isinstance(runtime, dict) or runtime.get('kind') != 'codex-model-turn' \
            or not isinstance(child, dict) or type(child.get('pid')) is not int or not child.get('birth') \
            or process_state(child['pid'], child['birth']) != 'dead':
        raise ValueError('owned model CLI is live or unknown')
    path = Path(runtime.get('log', ''))
    offset, sid = runtime.get('offset'), runtime.get('session')
    if not path.is_absolute() or path.is_symlink() or not path.is_file() \
            or type(offset) is not int or offset < 0 or not isinstance(sid, str) or not sid:
        raise ValueError('model log binding unavailable')
    with path.open('rb') as stream:
        if os.fstat(stream.fileno()).st_size > 16 * 1024 * 1024:
            raise ValueError('model log exceeds qualified bound')
        data = stream.read()
    if offset > len(data) or (offset and data[offset - 1:offset] != b'\n') \
            or hashlib.sha256(data[:offset]).hexdigest() != runtime.get('prefix_sha256'):
        raise ValueError('model log prefix/offset changed')
    segment = data[offset:]
    if not segment.endswith(b'\n'):
        raise ValueError('model log incomplete')
    try:
        entries = [json.loads(line) for line in segment.splitlines()]
    except (ValueError, UnicodeDecodeError) as error:
        raise ValueError('model log malformed') from error
    if not entries or any(not isinstance(entry, dict) for entry in entries) \
            or entries[0].get('type') != 'thread.started' or entries[0].get('thread_id') != sid \
            or len(entries) < 3 or entries[1].get('type') != 'turn.started' \
            or sum(entry.get('type') == 'thread.started' for entry in entries) != 1 \
            or sum(entry.get('type') == 'turn.started' for entry in entries) != 1:
        raise ValueError('model turn attribution unknown or duplicated')
    terminal = entries[-1].get('type')
    if terminal not in ('turn.completed', 'turn.failed') \
            or any(entry.get('type') in ('turn.completed', 'turn.failed', 'error') for entry in entries[:-1]) \
            or next(i for i, entry in enumerate(entries) if entry.get('type') == 'turn.started') >= len(entries) - 1:
        raise ValueError('model turn terminal receipt unavailable')
    return {'kind': 'model-turn', 'session': sid, 'terminal': terminal,
            'offset': offset, 'log_sha256': hashlib.sha256(data).hexdigest()}


class SQLiteCapacity:
    """Explicit single-owner capacity authority. Leases only, never a second task board."""
    def __init__(self, path, scope, owner, caps):
        import sqlite3
        if scope not in ('project', 'host') or not isinstance(owner, str) or not owner or not isinstance(caps, dict) or not caps \
                or any(not isinstance(k, str) or not k or type(v) is not int or v < 0 for k, v in caps.items()):
            raise ValueError('explicit resource ownership and finite caps required')
        self.path, self.scope, self.owner, self.caps = Path(path), scope, owner, caps
        if self.path.is_symlink() or not self.path.parent.is_dir():
            raise ValueError('capacity provider must be explicitly provisioned; symlink refused')
        self.sqlite = sqlite3
        meta = {'schema': 2, 'scope': scope, 'owner': owner, 'caps': caps,
                'domain': process_domain() if scope == 'host' else None}
        with self.transaction() as db:
            db.execute('CREATE TABLE IF NOT EXISTS capacity_meta (slot INTEGER PRIMARY KEY CHECK(slot=1), body TEXT NOT NULL)')
            db.execute('CREATE TABLE IF NOT EXISTS capacity_lease (id TEXT PRIMARY KEY, body TEXT NOT NULL)')
            prior = db.execute('SELECT body FROM capacity_meta WHERE slot=1').fetchone()
            if prior and json.loads(prior[0]) != meta:
                raise ValueError('capacity ownership/domain/caps changed; explicit reconciliation required')
            if not prior:
                db.execute('INSERT INTO capacity_meta VALUES (1,?)', (json.dumps(meta, sort_keys=True),))
        os.chmod(self.path, 0o600)

    @contextlib.contextmanager
    def transaction(self):
        db = self.sqlite.connect(str(self.path), timeout=5, isolation_level=None)
        try:
            db.execute('PRAGMA synchronous=FULL')
            db.execute('BEGIN IMMEDIATE')
            yield db
            db.execute('COMMIT')
        except BaseException:
            if db.in_transaction:
                db.execute('ROLLBACK')
            raise
        finally:
            db.close()

    def row(self, db, key):
        row = db.execute('SELECT body FROM capacity_lease WHERE id=?', (key,)).fetchone()
        return json.loads(row[0]) if row else None

    def put(self, db, key, value):
        db.execute('INSERT OR REPLACE INTO capacity_lease VALUES (?,?)', (key, json.dumps(value, sort_keys=True)))

    def observe(self, key):
        with self.transaction() as db:
            return self.row(db, key)

    def reserve(self, key, request):
        with self.transaction() as db:
            leases = {k: json.loads(body) for k,body in db.execute("SELECT id,body FROM capacity_lease")}
            before = json.dumps(leases,sort_keys=True)
            result = capacity_reserve(leases,self.caps,key,request)
            if json.dumps(leases,sort_keys=True) != before:
                self.put(db,key,result)
            return result

    def launch(self, key, runtime=None):
        if self.scope!='host':
            raise ValueError('native child launch belongs to host provider')
        with self.transaction() as db:
            value = self.row(db, key)
            if not value or value['phase'] != 'reserved':
                return False
            value['phase'] = 'launching'
            value['runtime'] = runtime
            self.put(db,key,value)
            return True

    def bind(self, key):
        if self.scope!='host':
            raise ValueError('native child binding belongs to host provider')
        pid = os.getpid()
        state, birth = process_identity(pid)
        if not birth:
            raise ValueError('native child birth unavailable')
        with self.transaction() as db:
            value = self.row(db,key)
            if not value or value['phase'] != 'launching':
                raise ValueError('child admission revoked or duplicate; no action permitted')
            value.update(phase='bound',child={'pid':pid,'birth':birth},stop=False)
            self.put(db,key,value)
            return value

    def stop(self, key, request=None):
        with self.transaction() as db:
            value=self.row(db,key)
            if value and request is not None and value['request']!=request:
                raise ValueError('stop request differs from exact admitted grant')
            if not value:
                if request is None:
                    raise ValueError('unknown grant')
                value={'request':request,'phase':'released','ticket':0,'drain':{'kind':'revoked-before-admission'}}
            if value['phase'] == 'launching' and (value.get('runtime') or {}).get('kind') == 'codex-model-turn':
                raise ValueError('model launch outcome unknown; reservation retained')
            if value['phase'] in ('waiting','reserved','launching'):
                value.update(phase='released',drain={'kind':'revoked-before-admission'})
            elif value['phase']=='bound':
                value['stop']=True
            self.put(db,key,value)
            return value

    def bind_model(self, key, pid, birth, runtime):
        """Bind the launched CLI, never the controller or an unqualified resource tree."""
        if self.scope != 'host' or type(pid) is not int or not birth \
                or process_identity(pid) != ('running', birth):
            raise ValueError('exact running model child identity required')
        with self.transaction() as db:
            value = self.row(db, key)
            demand = ((value or {}).get('request') or {}).get('demand') or {}
            if not value or value['phase'] != 'launching' or not demand \
                    or demand != {'model:codex': 1} \
                    or value.get('runtime') != runtime or not isinstance(runtime, dict) \
                    or runtime.get('kind') != 'codex-model-turn':
                raise ValueError('model-only launch binding required')
            value.update(phase='bound', child={'pid': pid, 'birth': birth}, stop=False)
            self.put(db, key, value)
            return value

    def settle_model(self, key, runtime):
        with self.transaction() as db:
            value = self.row(db, key)
            demand = ((value or {}).get('request') or {}).get('demand') or {}
            if self.scope != 'host' or not value or value['phase'] not in ('bound', 'drained', 'released') \
                    or value.get('runtime') != runtime or not demand \
                    or demand != {'model:codex': 1}:
                raise ValueError('model-only bound grant required')
            proof = model_turn_receipt(runtime, value.get('child'))
            if value['phase'] in ('drained', 'released') and value.get('drain') != proof:
                raise ValueError('settled model evidence changed')
            if value['phase'] == 'bound':
                value.update(phase='drained', drain=proof)
                self.put(db, key, value)
            return value

    def drained(self, key, application):
        with self.transaction() as db:
            value=self.row(db,key)
            if not value or value['phase']!='bound' or (value.get('runtime') or {}).get('kind') != 'controlled-artifact' \
                    or value['child']!={'pid':os.getpid(),'birth':process_identity(os.getpid())[1]}:
                raise ValueError('foreign child cannot settle a grant')
            value.update(phase='drained',drain={'kind':'controlled-artifact','application':application})
            self.put(db,key,value)

    def drain_verified(self, value):
        if not value:
            return False
        proof=value.get('drain') or {}
        if value['phase']=='released' and proof.get('kind')=='revoked-before-admission':
            return True
        child=value.get('child') or {}
        if proof.get('kind') == 'model-turn':
            demand = (value.get('request') or {}).get('demand') or {}
            if demand != {'model:codex': 1} \
                    or value['phase'] not in ('drained', 'released'):
                return False
            try:
                return model_turn_receipt(value.get('runtime'), child) == proof
            except (ValueError, OSError):
                return False
        return value['phase'] in ('drained','released') and proof.get('kind')=='controlled-artifact' \
            and (value.get('runtime') or {}).get('kind') == 'controlled-artifact' \
            and process_state(child.get('pid'),child.get('birth'))=='dead'

    def release(self, key):
        with self.transaction() as db:
            value=self.row(db,key)
            if not value:
                raise ValueError('unknown grant cannot be released')
            if value['phase']=='released':
                return value
            if self.scope=='host' and value['phase'] in ('launching','bound','drained') and not self.drain_verified(value):
                raise ValueError('host child is live/unknown; drain required')
            value['phase']='released'
            self.put(db,key,value)
            return value

    def reconcile_dead(self, key, runtime):
        with self.transaction() as db:
            value=self.row(db,key)
            child=(value or {}).get('child') or {}
            if not value or value['phase']!='bound' or value.get('runtime')!=runtime \
                    or runtime.get('kind')!='controlled-artifact' or process_state(child.get('pid'),child.get('birth'))!='dead':
                raise ValueError('unqualified dead-child reconciliation')
            value.update(phase='drained',drain={'kind':'controlled-artifact','application':{'status':'unknown'},
                         'cause':'verified finite child died without application receipt','runtime':runtime})
            self.put(db,key,value)
            return value


def controlled_child(input_path):
    """Qualified finite adapter: no arbitrary commands, no descendants, no network."""
    data=json.loads(Path(input_path).read_text())
    provider=SQLiteCapacity(data['provider'],'host',data['owner'],data['caps'])
    lease=provider.observe(data['key'])
    if not lease or lease['request'] != data['request']:
        raise ValueError('child request differs from grant')
    expected={'kind':'controlled-artifact','implementation_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'input_sha256':hashlib.sha256(Path(input_path).read_bytes()).hexdigest()}
    if lease.get('runtime')!=expected:
        raise ValueError('child code/input differs from the qualified launch')
    provider.bind(data['key'])
    path=Path(data['workspace']).resolve()/data['artifact']
    relative=Path(data['artifact'])
    if relative.is_absolute() or '..' in relative.parts or not path.parent.resolve().is_relative_to(Path(data['workspace']).resolve()):
        raise ValueError('controlled child artifact escapes workspace')
    application={'status':'failed'}
    try:
        content=data['text'].encode('utf-8')
        if len(content)>1024*1024:
            raise ValueError('controlled artifact exceeds 1 MiB')
        materialize_artifact(path,content)
        application={'status':'ok','artifact':relative.as_posix(),'sha256':hashlib.sha256(path.read_bytes()).hexdigest(),
                     'event':data['event'],'session':data['request']['identity']['session']}
        while not provider.observe(data['key']).get('stop'):
            time.sleep(0.02)
    finally:
        provider.drained(data['key'],application)
    return application


class ControlledArtifactRuntime:
    children = {}  # owned Popen objects only; durable authority remains the native grant

    def __init__(self, provider, root):
        self.provider,self.root=provider,Path(root).resolve()

    def start(self, key, request, event, artifact):
        directory=self.root
        for component in ('.taskq','controlled'):
            directory=directory/component
            if directory.is_symlink() or directory.exists() and not directory.is_dir():
                raise ValueError('controlled input directory is not an owned workspace directory')
            directory.mkdir(mode=0o700,exist_ok=True)
        data={'provider':str(self.provider.path.resolve()),'owner':self.provider.owner,'caps':self.provider.caps,
              'key':key,'request':request,'workspace':str(self.root),'event':event['id'],'text':event['text'],'artifact':artifact}
        path=directory/(key+'.json')
        materialize_artifact(path,json.dumps(data,sort_keys=True).encode())
        runtime={'kind':'controlled-artifact','implementation_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                 'input_sha256':hashlib.sha256(path.read_bytes()).hexdigest()}
        if not self.provider.launch(key,runtime):
            return self.provider.observe(key)  # never another spawn for a retained launch intent
        with (directory/(key+'.log')).open('ab') as output:
            process=subprocess.Popen([sys.executable,'-I','-S','-B',str(Path(__file__).resolve()),'capacity-child','--input',str(path)],
                                     cwd=self.root,env={k:v for k,v in os.environ.items() if k in ('PATH','SYSTEMROOT','TEMP','TMP','LANG')},
                                     stdin=subprocess.DEVNULL,stdout=output,stderr=output,
                                     **({'creationflags':0x208} if os.name=='nt' else {'start_new_session':True}))
        self.children[(str(self.provider.path),key)] = process
        deadline=time.monotonic()+2
        while time.monotonic()<deadline:
            value=self.provider.observe(key)
            if value['phase']!='launching' or process.poll() is not None:
                break
            time.sleep(.02)
        return self.provider.observe(key)

    def drain(self, key, request):
        self.provider.stop(key,request)
        deadline=time.monotonic()+2
        while True:
            value=self.provider.observe(key)
            child=(value or {}).get('child') or {}
            if value and value['phase']=='bound' and process_state(child.get('pid'),child.get('birth'))=='dead':
                path=self.root/'.taskq'/'controlled'/(key+'.json')
                runtime={'kind':'controlled-artifact','implementation_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                         'input_sha256':hashlib.sha256(path.read_bytes()).hexdigest()}
                value=self.provider.reconcile_dead(key,runtime)
            if self.provider.drain_verified(value):
                process=self.children.pop((str(self.provider.path),key),None)
                if process is not None:
                    process.wait(timeout=1)
                return value
            if time.monotonic()>=deadline:
                return None
            time.sleep(.02)


def capacity_context(item, refuse=fail):
    selected=CONFIG.get('capacity')
    if not isinstance(selected,dict) or not callable(getattr(BOARD,'capacity_provider',None)):
        refuse('qualified board capacity provider and explicit host/project caps required; no defaults')
    if (item['raw'].get('execution') or {}).get('kind')!='controlled-artifact' \
            or (item.get('claim') or {}).get('runtime')!='controlled-artifact':
        refuse('runtime addressed drain/admission unsupported; arbitrary tasks are not controlled-artifact')
    project=BOARD.capacity_provider(selected['project_caps'])
    if not all(callable(getattr(project,m,None)) for m in ('reserve','observe','release')) \
            or project.scope!='project' or project.owner!=CONFIG.get('repo') or project.caps!=selected['project_caps']:
        refuse('project provider protocol unavailable')
    host=SQLiteCapacity(selected['host_path'],'host',machine(),selected['host_caps'])
    return project,host,ControlledArtifactRuntime(host,CONFIG['root'])


def lifecycle_read(n, refuse=fail, manager=False):
    issue=read_issue(n);item=parse(issue)
    if not item or issue['state']!='open' or item['raw'].get('event_schema')!=2:
        refuse('lifecycle requires an open opt-in schema2 task')
    reason=schema2_execution_reason(item)
    if reason:
        refuse(reason)
    actor=origin();authority=item.get('pm') if manager else item.get('supervisor') or item.get('pm')
    if not actor or actor!=authority or not actor.get('session') or (item.get('claim') or {}).get('name')!=machine():
        refuse('lifecycle requires exact recorded authority on owning host; no reassignment')
    return issue,item


def write_task_verified(issue, raw, labels, preserve_text=False):
    """A successful transport response is not a receipt until fresh task readback matches."""
    if 'legacy_recovery' in issue_data(issue):
        fail(f'#{issue["iid"]}: legacy recovery hold; authoritative writes refused')
    body=(BLOCK.sub(lambda _: block('',raw).lstrip('\n'),issue['body'],count=1) if preserve_text
          else block(BLOCK.sub('',issue['body']).strip(),raw))
    def write_and_read():
        BOARD.update(issue['iid'],labels=labels,body=body)
        fresh=read_issue(issue['iid'])
        if fresh.get('iid')!=issue['iid'] or fresh.get('state')!=issue['state'] \
                or fresh.get('body')!=body or sorted(fresh.get('labels',[]))!=sorted(labels):
            raise RuntimeError('task write readback unknown/conflicting; retain project guard')
        return fresh
    return effect(write_and_read)


def lifecycle_write(issue, raw, state=None):
    labels=issue['labels'] if state is None else [l for l in issue['labels'] if not l.startswith(PREFIX)]+[PREFIX+state]
    write_task_verified(issue,raw,event_labels(raw,labels))


def lifecycle_prepare(n, refuse=fail, manager=False):
    issue,item=lifecycle_read(n,refuse,manager);raw=item['raw'];project,host,runtime=capacity_context(item,refuse)
    execution=raw['execution'];identity=item['claim'];life=raw.get('lifecycle')
    if life is not None and (not isinstance(life,dict) or type(life.get('generation')) is not int \
            or life['generation']<=0 or type(life.get('event')) is not int or life['event']<=0 \
            or life.get('phase') not in ('reserving','waiting','starting','active','draining','parked-releasing','parked')):
        refuse('malformed lifecycle intent; reconciliation required')
    if not isinstance(execution,dict) or not isinstance(execution.get('artifact'),str) \
            or type(execution.get('event')) is not int or execution['event']<=0 \
            or not isinstance(raw.get('events'),list):
        refuse('malformed execution/event intent; reconciliation required')
    for event_id in {execution['event'],(life or {}).get('event')} - {None}:
        events=[event for event in raw['events'] if isinstance(event,dict) and event.get('id')==event_id]
        if len(events)!=1 or events[0].get('action')!='answer' or not isinstance(events[0].get('text'),str) \
                or len(events[0]['text'].encode())>1024*1024 \
                or not isinstance(events[0].get('recipients'),list) or not isinstance(events[0].get('acks'),list) \
                or any(not isinstance(t,str) for t in events[0]['recipients']+events[0]['acks']) \
                or not set(events[0]['acks']).issubset(events[0]['recipients']) \
                or recipient('worker',identity) not in events[0]['recipients']:
            refuse('exact bounded worker answer event required')
    age=(life or {}).get('age') if life else raw.get('admission_age')
    demand=(life or {}).get('demand') if life else execution.get('demand')
    selected=CONFIG['capacity']
    if type(age) not in (int,float) or not -float('inf')<age<float('inf'):
        refuse('finite original admission age required')
    for vector in (demand,execution.get('demand')):
        if not isinstance(vector,dict) or not vector or any(k not in selected['host_caps'] \
                or type(v) is not int or v<0 or v>selected['host_caps'][k] for k,v in vector.items()) \
                or any(vector.get(k,0)>v for k,v in selected['project_caps'].items()):
            refuse('unsatisfiable or malformed demand; no grants acquired')
    fingerprint={'repo':CONFIG['repo'],'project_domain':getattr(project,'domain',
                 {'board':CONFIG['board'],'repo':CONFIG['repo']}),'task':item['iid'],'identity':identity,
                 'authorities':{k:item.get(k) for k in ('supervisor','pm')}}
    def artifact_for(event):
        name=execution['artifact'].replace('{event}',str(event))
        path=Path(name);root=Path(CONFIG['root']).resolve()
        if path.is_absolute() or '..' in path.parts or not path.name or not (root/path).parent.is_dir() \
                or not (root/path).parent.resolve().is_relative_to(root):
            refuse('controlled artifact must stay in the existing workspace directory')
        return name
    if life:
        expected_key=hashlib.sha256(json.dumps([fingerprint,life['generation']],sort_keys=True).encode()).hexdigest()
        if life.get('identity')!=fingerprint or life.get('artifact')!=artifact_for(life['event']) or life.get('key')!=expected_key:
            refuse('recorded identity/artifact/key changed; lifecycle reconciliation required')
    artifact_for(execution['event'])  # also validate a new intent before any task write
    return issue,item,project,host,runtime,fingerprint,artifact_for


def lifecycle(n, action, text=None):
    issue,item,project,host,runtime,fingerprint,artifact_for=lifecycle_prepare(n)
    raw=item['raw'];execution=raw['execution'];identity=item['claim'];life=raw.get('lifecycle')
    if raw.get('result') is not None and (action in ('admit','resume','answer') or
            action == 'reconcile' and (not life or life['phase'] not in ('draining','parked-releasing','parked'))):
        fail('accepted native result requires separately qualified rework; no new execution or answer')
    def request_for(lifecycle):
        return {**fingerprint,'generation':lifecycle['generation'],'age':lifecycle['age'],
                'priority':item['priority'],'demand':lifecycle['demand']}
    if action=='answer':
        if not life or life['phase'] not in ('draining','parked-releasing','parked') or text is None:
            fail('bounded parked answer requires text and an existing park intent')
        if len(raw.get('events',[]))>=EVENT_LIMIT:
            fail('event history full; qualified compaction required')
        event_id=raw.get('event_seq',0)+1
        raw['events'].append({'id':event_id,'action':'answer','text':text,'by':origin(),
                              'recipients':[recipient('worker',identity)],'acks':[]})
        raw['event_seq']=event_id;execution['event']=event_id
        lifecycle_write(issue,raw)
        return {'phase':life['phase'],'answer_event':event_id,'task_accepted':False}
    if action in ('park','reconcile') and life and life['phase'] in ('draining','parked-releasing','parked','active','starting','reserving','waiting'):
        if action=='reconcile' and life['phase'] in ('active','starting','reserving','waiting'):
            pass  # readback/re-entry uses admission below; never blindly repeat a spawn
        else:
            if life['phase']=='parked':
                if not host.drain_verified(host.observe(life['key'])) or project.observe(life['key']) is not None and project.observe(life['key'])['phase']!='released':
                    fail('parked grants are unsettled; no resume/release inferred')
                return {'phase':'parked','generation':life['generation'],'task_accepted':False}
            life['phase']='draining';lifecycle_write(issue,raw)
            drained=effect(runtime.drain,life['key'],request_for(life))
            if not drained:
                return {'phase':'draining','blocker':'addressed drain unproven; grants retained','task_accepted':False}
            life['drain']=drained['drain'];life['phase']='parked-releasing';lifecycle_write(issue,raw,'later')
            effect(host.release,life['key'])
            if project.observe(life['key']) is not None:
                effect(project.release,life['key'])
            life['phase']='parked'
            application=drained['drain'].get('application') or {}
            event=next((e for e in raw['events'] if e['id']==life['event']),None)
            path=Path(CONFIG['root'])/life['artifact']
            if application.get('status')=='ok' and event and recipient('worker',identity) in event['recipients'] \
                    and application.get('artifact')==life['artifact'] and application.get('event')==life['event'] \
                    and application.get('session')==identity['session'] and path.is_file() and not path.is_symlink() \
                    and application.get('sha256')==hashlib.sha256(path.read_bytes()).hexdigest()==hashlib.sha256(event['text'].encode()).hexdigest():
                target=recipient('worker',identity)
                if target not in event['acks']:
                    event['acks'].append(target)
                raw.setdefault('applications',{})[life['key']]={'event':life['event'],'identity':identity,'proof':application}
            lifecycle_write(issue,raw,'later')
            return {'phase':'parked','generation':life['generation'],'task_accepted':False}
    if action=='park':
        fail('no active lifecycle to park')
    if action=='accept':
        if not life or life['phase']!='parked':
            fail('result acceptance requires an already settled park; no automatic drain')
        native_ack(n,life['event'])
        criteria=raw.get('acceptance_criteria') or {}
        event=next((e for e in raw['events'] if e['id']==execution['event']),None)
        if criteria.get('kind')!='answer-artifact' or criteria.get('event')!=execution['event'] or criteria.get('artifact')!=artifact_for(execution['event']) \
                or not event or criteria.get('sha256')!=hashlib.sha256(event['text'].encode()).hexdigest():
            fail('task-specific acceptance criteria unavailable or changed; action/exit0 is not task success')
        path=Path(CONFIG['root'])/artifact_for(execution['event']);value=host.observe(life['key']) if life else None
        application=((value or {}).get('drain') or {}).get('application') or {}
        if not host.drain_verified(value) or application.get('status')!='ok' or application.get('session')!=identity['session'] or application.get('event')!=execution['event'] \
                or application.get('sha256')!=criteria['sha256'] or not path.is_file() or path.is_symlink() \
                or hashlib.sha256(path.read_bytes()).hexdigest()!=criteria['sha256']:
            fail('exact independently verifiable application receipt/artifact missing')
        if not life or life['phase'] != 'parked' or value['phase'] != 'released' \
                or project.observe(life['key']) is not None and project.observe(life['key'])['phase'] != 'released' \
                or recipient('worker',identity) not in event['acks'] \
                or (raw.get('applications',{}).get(life['key']) or {}).get('proof') != application:
            fail('application ack or predecessor grants are not settled on the task board')
        receipt={'kind':'answer-artifact','key':life['key'],'criteria':criteria,'authority':origin(),'application':application}
        receipts=raw.setdefault('acceptance_receipts',{})
        if receipts.get(life['key']) and receipts[life['key']]!=receipt:
            fail('conflicting task acceptance receipt')
        if not receipts.get(life['key']):
            receipts[life['key']]=receipt;lifecycle_write(issue,raw)
        return {'phase':life['phase'],'task_accepted':True,'criterion':'answer-artifact'}
    if action not in ('admit','resume','reconcile'):
        fail('unsupported lifecycle action')
    if life and life['phase'] in ('draining','parked-releasing'):
        fail('drain/release unfinished; reconcile park before resume')
    if life and life['phase']=='parked' and action!='resume':
        fail('parked task requires explicit resume')
    if not life or life['phase']=='parked':
        if life and (not host.drain_verified(host.observe(life['key'])) or project.observe(life['key']) is not None and project.observe(life['key'])['phase']!='released'):
            fail('predecessor drain changed; resume refused')
        generation=life['generation']+1 if life else 1
        age=life['age'] if life else raw.get('admission_age')
        if type(age) not in (int,float):
            fail('original admission age required')
        key=hashlib.sha256(json.dumps([fingerprint,generation],sort_keys=True).encode()).hexdigest()
        life={'identity':fingerprint,'generation':generation,'age':age,'phase':'reserving','key':key,
              'demand':execution['demand'],'event':execution['event'],'artifact':artifact_for(execution['event'])}
        raw['lifecycle']=life;lifecycle_write(issue,raw)
    if life['event']!=execution['event'] or life['artifact']!=artifact_for(execution['event']) or life['demand']!=execution['demand']:
        fail('active execution changed; park/reconcile before continuation')
    request=request_for(life)
    selected=CONFIG['capacity']
    demand=life['demand']
    if not isinstance(demand,dict) or not demand or any(k not in selected['host_caps'] or type(v) is not int or v<0 or v>selected['host_caps'][k] for k,v in demand.items()) \
            or any(demand.get(k,0)>v for k,v in selected['project_caps'].items()):
        fail('unsatisfiable demand; no grants acquired')
    project_request={**request,'demand':{k:demand.get(k,0) for k in selected['project_caps']}}
    p=effect(project.reserve,life['key'],project_request)
    if p['phase']!='reserved':
        life['phase']='waiting';lifecycle_write(issue,raw)
        return {'phase':'waiting','resource':'project','task_accepted':False}
    h=effect(host.reserve,life['key'],request)
    if h['phase']=='waiting':
        life['phase']='waiting';lifecycle_write(issue,raw)
        return {'phase':'waiting','resource':'host','task_accepted':False}
    if h['phase']=='released':
        fail('revoked admission cannot spawn; reconcile park first')
    event=next(e for e in raw['events'] if e['id']==life['event'])
    life['phase']='starting';lifecycle_write(issue,raw)
    h=effect(runtime.start,life['key'],request,event,life['artifact'])
    life['phase']='active' if h['phase']=='bound' else 'starting'
    lifecycle_write(issue,raw,'doing')
    return {'phase':life['phase'],'generation':life['generation'],'child':h.get('child'),'task_accepted':False}


def native_ack(n, event_id):
    issue,item=lifecycle_read(n)
    life=item['raw'].get('lifecycle') or {}
    if life.get('event')!=event_id:
        fail('native ack requires the admitted application event')
    outcome=lifecycle(n,'park')
    if outcome['phase']!='parked':
        fail('native application/drain unproven; ack unchanged')
    issue,item=lifecycle_read(n);project,host,runtime=capacity_context(item)
    raw=item['raw'];life=raw['lifecycle'];value=host.observe(life['key'])
    proof=(raw.get('applications',{}).get(life['key']) or {}).get('proof') or {}
    event=next((e for e in raw['events'] if e['id']==event_id),None)
    path=Path(CONFIG['root'])/life['artifact']
    if not host.drain_verified(value) or value['phase']!='released' \
            or project.observe(life['key']) is not None and project.observe(life['key'])['phase']!='released' \
            or proof != ((value.get('drain') or {}).get('application') or {}) \
            or proof.get('status')!='ok' or proof.get('event')!=event_id \
            or proof.get('session')!=item['claim']['session'] or proof.get('artifact')!=life['artifact'] \
            or not event or recipient('worker',item['claim']) not in event['recipients'] \
            or recipient('worker',item['claim']) not in event['acks'] \
            or not path.is_file() or path.is_symlink() \
            or proof.get('sha256')!=hashlib.sha256(path.read_bytes()).hexdigest() \
            or proof.get('sha256')!=hashlib.sha256(event['text'].encode()).hexdigest():
        fail('native application acknowledgement cannot be independently verified')
    return proof


def cmd_lifecycle(args):
    print(json.dumps(lifecycle(args.n,args.action,getattr(args,'text',None))))


def queue_move(args, current):
    """Normal commands share the native service; never fall through to destructive legacy moves."""
    n=current['iid']
    issue,current=lifecycle_read(n)
    if args.command in ('later','ask'):
        outcome=lifecycle(n,'park')
        if args.command=='ask':
            issue,current=lifecycle_read(n);raw=current['raw']
            raw['decision']=decision(args)
            native_event(raw,'ask',args.text,event_targets(raw,'ask',args.text))
            lifecycle_write(issue,raw,'ask')
        print(json.dumps(outcome));return
    if args.command=='answer':
        if current.get('result'):
            fail('native result rework unsupported; preserve accepted result')
        if not args.text:
            fail('schema2 answer requires exact text')
        life=current['raw'].get('lifecycle') or {}
        if life.get('phase') not in ('draining','parked-releasing','parked'):
            lifecycle(n,'park')
        outcome=lifecycle(n,'answer',args.text)
        issue,current=lifecycle_read(n);raw=current['raw'];raw['decision']=None
        lifecycle_write(issue,raw,'ready' if raw['lifecycle']['phase']=='parked' else 'ask')
        print(json.dumps(outcome));return
    if args.command=='result':
        if args.sha or args.checks:
            fail('bounded artifact result cannot assert commit/CI publication checks')
        accepted=lifecycle(n,'accept')
        issue,current=lifecycle_read(n);raw=current['raw'];life=raw['lifecycle']
        accepted_receipt=raw['acceptance_receipts'][life['key']]
        result={'kind':'answer-artifact','key':life['key'],'criteria':accepted_receipt['criteria'],
                'application':accepted_receipt['application']}
        if raw.get('result') is not None and raw['result']!=result:
            fail('conflicting bounded task result')
        if raw.get('result') is None:
            raw['result']=result
            native_event(raw,'result',args.text or '',[recipient('manager',raw['pm'])] if raw.get('pm') else [])
            lifecycle_write(issue,raw,'review')
        print(json.dumps({**accepted,'state':'review','result':result}));return
    fail('schema2 command unsupported; no legacy mutation or implicit migration')


def native_event(raw, action, text, targets):
    if len(raw.get('events',[]))>=EVENT_LIMIT:
        fail('native event history full; qualified compaction required')
    number=raw.get('event_seq',0)+1
    raw.setdefault('events',[]).append({'id':number,'action':action,'text':text,'by':origin(),
                                      'recipients':targets,'acks':[]})
    raw['event_seq']=number
    return number


class NativePreflightRefusal(SystemExit):
    """Known validation refusal before effects; never changes guard poison from prior tasks."""


def refuse_preflight(message):
    raise NativePreflightRefusal(message)


def queue_pass(item):
    """Act only as this task's exact controller; unsupported tasks keep their slots."""
    reason=schema2_execution_reason(item)
    authority=item.get('supervisor') or item.get('pm')
    if reason or origin()!=authority or (item.get('claim') or {}).get('name')!=machine():
        print(f'taskq: #{item["iid"]}: {reason or "native controller unavailable; task and grants retained"}',file=sys.stderr)
        return
    if not isinstance(CONFIG.get('capacity'),dict) or not callable(getattr(BOARD,'capacity_provider',None)) \
            or (item['raw'].get('execution') or {}).get('kind')!='controlled-artifact' \
            or (item.get('claim') or {}).get('runtime')!='controlled-artifact':
        print(f'taskq: #{item["iid"]}: addressed runtime/capacity unsupported; task and grants retained',file=sys.stderr)
        return
    try:
        prepared=lifecycle_prepare(item['iid'],refuse_preflight)  # no grant/task/runtime effects
        item=prepared[1]
    except (NativePreflightRefusal,ValueError) as error:
        if (getattr(GUARD,'held',None) or {}).get('poisoned'):
            raise  # never swallow an earlier effect failure or reset its poison
        print(f'taskq: #{item["iid"]}: native preflight refused ({error}); task and grants retained',file=sys.stderr)
        return
    life=item['raw'].get('lifecycle') or {}
    if item['state']=='review' or item.get('result') or item['state']=='later' and life.get('phase')=='parked':
        return
    if item['state']=='ready' and not open_deps(item['deps']):
        outcome=lifecycle(item['iid'],'resume' if life.get('phase')=='parked' else 'admit')
        if outcome['phase']!='active':
            return
    elif life.get('phase') not in ('active','starting','draining','parked-releasing'):
        return
    outcome=lifecycle(item['iid'],'park')
    # A pending answer arriving during an unproven drain becomes eligible only after settled park.
    issue,current=lifecycle_read(item['iid']);raw=current['raw'];life=raw['lifecycle']
    if outcome['phase']=='parked' and raw.get('decision'):
        lifecycle_write(issue,raw,'ask')
    elif outcome['phase']=='parked' and raw['execution']['event']!=life['event']:
        lifecycle_write(issue,raw,'ready')




class RecoveryCorrelation:
    """Read-only identity/payload correlation; no receipt reducer or execution authority."""
    def __init__(self, item, role):
        if role not in ('worker', 'supervisor'):
            raise ValueError('unsupported recovery role')
        identity = item.get('claim' if role == 'worker' else 'supervisor') or {}
        self.target = {'task': item['iid'], 'role': role, **{key: identity.get(key) for key in ('session', 'runtime', 'name')}}
        if type(self.target['task']) is not int or self.target['task'] <= 0 or any(
                not isinstance(self.target[key], str) or not self.target[key] for key in ('session', 'runtime', 'name')):
            raise ValueError('complete recorded role identity required')
        self.repo = CONFIG.get('repo') or str(CONFIG['root'])
        raw = item.get('raw') or {}
        payload = raw  # exact authoritative payload, including answers, events and recipient acknowledgements
        self.payload = hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()
        self.id = hashlib.sha256(json.dumps([self.repo, self.target, self.payload], sort_keys=True).encode()).hexdigest()[:20]

    def snapshot(self):
        return json.loads(json.dumps({'repo':self.repo,'target':self.target,
                                     'operation':self.id,'payload':self.payload}))


def recovery_plan(item, role, kinds, here):
    operation = RecoveryCorrelation(item, role)
    remote = operation.target['name'] != here
    plan = {'operation': operation.snapshot(), 'state': 'unsupported',
            'capability': 'owning-host-read-required' if remote else 'addressed-writer-controller-drain-unqualified',
            'automatic_actions': [], 'slot': 'reserved',
            'manual_steps': [
                'Keep this exact role/session, handle, branch, result, question and unacknowledged events.',
                'On the owning host identify the actual runtime/controller and in-flight operation via a supported read.',
                'Obtain addressed session unload/drain and operation reconciliation receipts; never stop a shared server or delete a lock.',
                'If the platform has no addressed control, leave recovery unsupported and hand the exact identity to its operator.',
                'After verified drain and fresh board identity, qualify same-session continuation with a new verified native process birth.',
                'Verify board record then application receipt for exact pending payloads before acknowledgement or normal queue resume.']}
    return plan


def cmd_recovery_plan(args):
    kinds, here = runtimes(), machine()
    for n in ([args.n] if isinstance(args.n, int) else args.n):
        issue = read_issue(n)
        item = parse(issue) if issue['state'] == 'open' else None
        if not item or not mine(item):
            fail(f'#{n}: fresh selected open task required')
        try:
            plan = recovery_plan(item, args.role, kinds, here)
        except ValueError as error:
            fail(f'#{n}: {error}')
        if args.json:
            print(json.dumps(plan, ensure_ascii=False))
        else:
            target = plan['operation']['target']
            print(f'#{n} {target["role"]} {target["runtime"]}:{target["session"]} on {target["name"]}: {plan["state"]}; {plan["capability"]}; slot reserved')
            for step in plan['manual_steps']:
                print(f'- {step}')


def positive_minutes(value):
    try:
        number = float(value)
    except (ValueError, TypeError):
        raise argparse.ArgumentTypeError('finite positive minutes required')
    if not 0 < number < float('inf'):
        raise argparse.ArgumentTypeError('finite positive minutes required')
    return number


def recovery_role_state(kind, session):
    """Use ordinary state only when structured observation is absent; failed evidence stays unknown."""
    observe = getattr(kind, 'observe', None)
    try:
        if callable(observe):
            evidence = observe(session)
            return evidence.get('state', 'unknown') if isinstance(evidence, dict) else 'unknown'
        return runtime_state(kind, session) if kind is not None else 'unknown'
    except (Exception, SystemExit):
        return 'unknown'


def recovery_question(item, kinds, here, minutes):
    """Opt-in isolated candidate: one persisted diagnosis, never takeover; current native payloads preserved."""
    if not mine(item) or item['state'] not in ('doing', 'ask') or item.get('result'):
        return False
    actor, pm = origin() or {}, item.get('pm') or {}
    if pm and (pm.get('runtime'), pm.get('session'), pm.get('name')) != (actor.get('runtime'), actor.get('session'), here):
        return False
    for role, field in (('supervisor', 'supervisor'), ('worker', 'claim')):
        identity = item.get(field) or {}
        if not identity.get('session') or identity.get('name') != here:
            continue
        kind = kinds.get(identity.get('runtime'))
        if recovery_role_state(kind, identity['session']) in ('running', 'idle'):
            continue
        # Even before the timeout, this role cannot be resumed/replaced by this candidate pass.
        operation = RecoveryCorrelation(item, role)
        identity_key = hashlib.sha256(json.dumps([operation.repo, operation.target], sort_keys=True).encode()).hexdigest()[:20]
        marker = f'recovery:{identity_key}'
        counts = dict(item['raw'].get('retry_counts') or {})
        if item['state'] == 'ask' or counts.get(marker):
            return True
        try:
            overdue = age(item) >= minutes
        except (ValueError, TypeError, OverflowError):
            overdue = False  # unknown clock is not elapsed time
        if not overdue:
            return True
        fresh = executable(read_issue(item['iid']), item)
        if not fresh:
            return True
        # Preserve payload changes from the authoritative read, including pending answers/events.
        item = fresh
        if item.get('result') or item['state'] != 'doing':
            return True
        try:
            if age(item) < minutes:
                return True
        except (ValueError, TypeError, OverflowError):
            return True
        if recovery_role_state(kind, identity['session']) in ('running', 'idle'):
            return True  # defer normal handling until the next fresh pass
        counts = dict(item['raw'].get('retry_counts') or {})
        if counts.get(marker):
            return True
        counts[marker] = 1
        text = (f'{role} {identity["session"]}: state unknown after board inactivity >= {minutes:g} minutes. '
                'Timeout does not prove death or release ownership. Addressed writer/controller drain is unsupported here. '
                'Preserve this role, pending answers, branch and result; obtain supported exact-session drain/reconciliation evidence. '
                f'[{marker}]')
        move(item, 'ask', 'ask', text, retry_counts=counts,
             decision={'summary': 'Состояние сессии неизвестно; требуется адресное восстановление без потери результата',
                       'options': ['Сохранить блокировку до supported drain receipt',
                                   'Передать exact-session recovery оператору owning runtime'], 'recommend': 1})
        return True
    return False


def report_guard():
    observe = getattr(BOARD, 'observe_guard', None)
    try:
        evidence = observe() if callable(observe) else {'state': 'unknown'}
    except (Exception, SystemExit):
        evidence = {'state': 'unknown'}
    state = evidence.get('state') if isinstance(evidence, dict) else 'unknown'
    if state == 'absent':
        print('Project guard: absent (GET snapshot; не grant).')
    elif state == 'held':
        token = hashlib.sha256(str(evidence.get('identity')).encode()).hexdigest()[:12]
        owner = re.fullmatch(r'([\w.-]+) (claude|codex|dot|hermes|grok):([\w-]+) pid=\d+ [\w-]+', evidence.get('owner') or '')
        identity = f'{owner[1]} {owner[2]}:{owner[3]}' if owner else 'unknown'
        print(f'Project guard: held G-{token}; controller {identity}; controller accessibility/operation unknown. PM: supported drain/readback; guard не снимать по наблюдению.')
    else:
        print('Project guard: unknown; PM: свежий supported read; отсутствие grant не доказано.')

DIAGNOSES = {
    'legacy-handle': ('Legacy PID/session без birth identity; crash не доказан', 'owning-runtime', 'сохранить handle/ответы; квалифицировать exact-session drain и новый identity-verified handle, не выдумывать birth'),
    'history-incomplete': ('История наблюдений неполна', 'owning-runtime', 'сверить durable receipts; отсутствие свидетельства не означает завершение'),
    'blocked-prerequisite': ('Prerequisite остаётся открытым', 'supervisor', 'проверить зависимость; не обходить gate'),
    'result-receipt-unknown': ('Board receipt результата отсутствует; производство неизвестно', 'worker', 'сверить сохранённый кандидат и supported result receipt; не повторять работу'),
    'fresh-read-unavailable': ('Свежие данные недоступны', 'PM', 'проверить штатный доступ; старые роли не использовать'),
    'owning-host': ('Удалённое состояние не проверено', 'owning-host', 'проверить текущую роль на её хосте'),
    'unknown-runtime': ('Состояние сессии неизвестно', 'owning-runtime', 'получить поддержанное read-only свидетельство'),
    'active-writer': ('Writer удерживается или отказал resume', 'owning-runtime', 'адресно проверить holder/release; замена пока не доказана'),
    'surface-auth': ('Последняя команда отказала по auth', 'owning-runtime', 'сравнить контекст запуска; credentials не переносить'),
    'after-hook': ('Последний hook отказал', 'runtime', 'проверить завершение hook и receipt следующего pass'),
    'transport-loss': ('Исход доставки неизвестен', 'ARM', 'сверить receipt и наличие прежнего wait'),
    'result-unsubmitted': ('Результат не передан', 'worker', 'сверить board receipt; сохранить кандидат, не повторять работу'),
    'ack-contention': ('Последняя команда встретила guard', 'ack-actor', 'сверить точный event/ack; отказ не означает провал доставки'),
    'native-windows': ('Native tooling отказал', 'supervisor', 'разобрать exact native failure; WSL и trust не включать молча'),
    'runtime-rejected': ('Последний tool/runtime отказал', 'owning-runtime', 'диагностировать штатный маршрут; без обхода rejection'),
    'interrupted-turn': ('CLI turn прерван; app неизвестен', 'owning-runtime', 'сверить текущую роль и исход операции'),
    'missing-order': ('Worker/order отсутствуют', 'supervisor', 'сверить обязательство и принять штатное решение об order'),
    'pending-event': ('Событие ожидает receipt', 'recipient', 'сверить exact recipient/event; не ack по наблюдению'),
    'owner-question': ('Текущий вопрос ожидает board receipt', 'owner/PM', 'сверить текущий вопрос и уже данный ответ; не запрашивать повторное разрешение'),
}

def observe_obligations(items, kinds, here, listed=None):
    """Fresh read-only diagnosis; unsupported evidence stays unknown, no recovery or storage."""
    fresh, problems = [], {}
    def add(item, role_name, code, state='unknown', evidence='board'):
        identity = item.get('supervisor' if role_name == 'supervisor' else 'claim') or {}
        raw = item['raw']
        ask = (raw.get('action_payloads') or {}).get('ask') or {}
        version = (ask.get('id') or hashlib.sha256(json.dumps(raw.get('decision') or {}, sort_keys=True).encode()).hexdigest()[:12]) if code == 'owner-question' else None
        key = [CONFIG.get('repo'), item['iid'], role_name, identity.get('session'), version, code, evidence]
        token = hashlib.sha256(json.dumps(key, sort_keys=True).encode()).hexdigest()[:12]
        title, actor, action = DIAGNOSES[code]
        problems[(item['iid'], role_name, code, evidence)] = {
            'ref': f'P{item["iid"]}-{token}', 'iid': item['iid'], 'role': role_name,
            'state': state, 'code': code, 'reason': title, 'actor': actor, 'next': action,
            'evidence': f'{evidence}; ask:{version}' if code == 'owner-question' else evidence,
        }

    for stale_item in items:
        if not mine(stale_item):
            fresh.append(stale_item)
            continue
        try:
            issue = read_issue(stale_item['iid'])
            item = parse(issue) if issue['state'] == 'open' else None
        except (Exception, SystemExit):
            item = {**stale_item, 'state': 'unknown (fresh-read-unavailable)'}
            fresh.append(item)
            add(item, 'PM', 'fresh-read-unavailable')
            continue
        if not item:
            continue
        fresh.append(item)
        if not mine(item):
            continue  # a fresh transfer is not authority to inspect someone else's runtime
        if item['state'] == 'later':
            continue
        raw = item['raw']
        open_ids = listed if listed is not None else {entry['iid'] for entry in items}
        for dependency in item.get('deps') or []:
            if dependency in open_ids:
                add(item, 'supervisor', 'blocked-prerequisite', 'waiting', f'dependency:#{dependency};list-snapshot')
        if item['state'] in ('doing', 'review') and (item.get('claim') or {}).get('session') and not item.get('result'):
            add(item, 'worker', 'result-receipt-unknown', evidence='board:result-absent;production-unverified')
        if item['state'] == 'ask' or item['state'] == 'review' and (raw.get('decision') or {}).get('options'):
            add(item, 'owner', 'owner-question', 'waiting')
        for event in raw.get('events') or []:
            for target in sorted(set(event.get('recipients', [])) - set(event.get('acks', []))):
                target_id = hashlib.sha256(target.encode()).hexdigest()[:12]
                add(item, 'recipient', 'pending-event', 'waiting', f'event:{event["id"]};recipient:{target_id}')
        worker = item.get('claim') or {}
        if item['state'] == 'doing' and item.get('supervisor') and not worker.get('session') and not item.get('order'):
            add(item, 'supervisor', 'missing-order')
        for role_name, field in (('supervisor', 'supervisor'), ('worker', 'claim')):
            identity = item.get(field) or {}
            sid = identity.get('session')
            if not sid:
                continue
            if identity.get('name') != here:
                add(item, role_name, 'owning-host')
                continue
            kind = kinds.get(identity.get('runtime'))
            observe = getattr(kind, 'observe', None)
            if not callable(observe):
                add(item, role_name, 'unknown-runtime')
                continue
            try:
                evidence = observe(sid)
                if not isinstance(evidence, dict):
                    raise ValueError('unsupported observation')
                state = evidence.get('state')
                state = state if state in ('running', 'idle') else 'unknown'
                records = evidence.get('problems') or [evidence]
                for record in records:
                    code = record.get('code')
                    if code == 'result-unsubmitted' and item.get('result'):
                        continue
                    source = record.get('source')
                    source = source if source in ('cli-turn', 'writer-holder', 'writer-unverified', 'handle-changed') else 'runtime-observe'
                    version = hashlib.sha256(str(record.get('evidence_id') or 'unavailable').encode()).hexdigest()[:12]
                    provenance = f'{source};evidence:{version}'
                    if code in DIAGNOSES:
                        add(item, role_name, code, state, provenance)
                    elif state == 'unknown':
                        add(item, role_name, 'unknown-runtime', evidence=provenance)
            except (Exception, SystemExit):
                add(item, role_name, 'unknown-runtime')
    return fresh, list(problems.values())

def report(items, listed, kinds, here, problems=None):
    """R6 (#574): the one report of tick and status. Counters and rows come from the same `items` and the same filter; a dep
    is open when `listed`, the iids of that same board list (every open issue, a task or not), holds it: no BOARD.get."""
    items = list(filter(mine, items))

    def state(item):  # what the row says; only a plain `ready` is counted ready
        deps = ', '.join(f'#{n}' for n in item['deps'] or [] if n in listed)
        if item['state'] == 'waiting':
            return f'waiting ({deps})' if deps else 'waiting'
        if item['state'] == 'ready' and not lead(item, kinds):
            return 'blocked (no manager)'
        return f'blocked ({deps} open)' if item['state'] == 'ready' and deps else item['state']
    states = {item['iid']: state(item) for item in items}
    count = lambda *wanted: sum(value in wanted for value in states.values())
    host, repo = CONFIG.get('host'), CONFIG.get('repo')
    url = CONFIG.get('board_url') or {'github': f'https://{host or "github.com"}/{repo}/issues',
                                      'gitlab': f'https://{host or "gitlab.com"}/{repo}/-/issues'}.get(CONFIG['board'])
    lines = [CONFIG['root'].name + (f' · [board]({url})' if url else ''),  # a board file names its page in `board_url`
             f'In work {count("doing", "review")} · Waiting for answer {count("ask")} · Ready {count("ready")}', '']
    rows = [row(item, kinds, here, states[item['iid']]) for item in items if item['state'] not in ('ask', 'later')]
    lines += ['| Task | State | Runtime | Session |', '|---|---|---|---|', *rows, ''] if rows else []  # empty: left out
    cards = decisions(items)
    lines += ['Questions (answer N.M):', '', '| Question | Brief reason | Options |', '|---|---|---|', *cards, ''] if cards else []
    later = [heading(item) for item in items if item['state'] == 'later']
    lines += ['Later: ' + ', '.join(later), ''] if later else []
    if problems:
        lines += ['Проблемы сессий (read-only; references не являются answer tokens):', '',
                  '| Reference / task | Роль / состояние | Свидетельство / проблема | Ответственный / Следующее действие |',
                  '|---|---|---|---|']
        for problem in problems:
            lines.append(f'| {problem["ref"]} / #{problem["iid"]} | {cell(problem["role"])} / {problem["state"]} | '
                         f'{problem["code"]}: {problem["reason"]}; {cell(problem["evidence"])} | '
                         f'{problem["actor"]}: {problem["next"]} |')
        lines += ['', 'Диагностика: per-task re-read; зависимости — list snapshot; runtime evidence не доказывает application/result receipt.', '']
    print('\n'.join([*lines, 'Mode: events · arm: <arm_tick>']))  # R6 item 6: the executing PM fills the one field

MEDIA = re.compile(r'\.(png|jpe?g|gif|webp|svg)(\?.*)?$', re.I)

def decisions(items):
    """#490, R6 Questions: one table row per task waiting on the owner: an ask, or a review with options; ★ the recommended one.
    Images inline unless `inline_media` is false."""
    lines = []
    for item in items:
        card = item['raw'].get('decision') or {}
        if not (item['state'] == 'ask' or item['state'] == 'review' and card.get('options')):
            continue
        n, inline = item['iid'], CONFIG.get('inline_media', True)
        links = [f'![{n}]({link})' if inline and MEDIA.search(link) else link for link in card.get('links') or []]
        options = [f'{n}.{k} {cell(text)}' + ' ★' * (k == card.get('recommend')) for k, text in enumerate(card.get('options') or [], 1)]
        question = heading(item) + ' review' * (item['state'] == 'review')
        lines.append(f'| {question} | {" · ".join([cell(card.get("summary") or item["title"]), *links])} | {" · ".join(options)} |')
    return lines

EVENTS = ('add', 'answer', 'run', 'result', 'requeue', 'close')  # R4 (#333): each starts one pass after its move

def dispatch(command, tasks, after=None, after_birth=None):
    """R4 (#405): the event pass runs in a detached `tick --quiet` child, its output in .taskq/dispatch.log; the event returns at once."""
    reason = release_reason()
    if reason:
        print(f'taskq: dispatch stopped: {reason}', file=sys.stderr)
        return
    if os.environ.get('CODEX_SANDBOX'):  # a sandboxed Codex worker can neither start codex nor see other sessions' pids:
        return  # its pass would requeue live tasks as gone and spawn workers that die at once (#269 run 4b)
    try:
        (CONFIG['root'] / '.taskq').mkdir(exist_ok=True)
        detach = {'creationflags': 0x208} if os.name == 'nt' else {'start_new_session': True}
        tasks = [str(n) for n in (tasks if isinstance(tasks, list) else [tasks])]
        with open(CONFIG['root'] / '.taskq' / 'dispatch.log', 'ab') as out:
            out.write(f'{datetime.now():%Y-%m-%d %H:%M:%S} {command} {" ".join(f"#{n}" for n in tasks) or f"pid {after}"}\n'.encode())
            out.flush()  # the event, then the child's lines
            start_pass([sys.executable, str(Path(__file__).resolve()), 'tick', '--quiet', *(['--after', str(after), '--after-birth', after_birth or '-'] if after else []), '--tasks', *tasks],
                       cwd=CONFIG['root'],
                       stdin=subprocess.DEVNULL, stdout=out, stderr=subprocess.STDOUT, **detach)
    except Exception as error:  # parent board mutation is already settled; report failure to launch its event
        print(f'taskq: dispatch stopped: {error}; resolve the blocker, then run a fresh tick', file=sys.stderr)

start_pass = subprocess.Popen  # tests run the child's pass in process

def event_pass(args):
    """`tick --quiet`: the tick pass without the table. `--after PID --after-birth ID`: first wait for that Codex turn to end (R4 #525).
    A failed pass exits nonzero and logs its blocker; the detached parent mutation remains settled."""
    try:
        while args.after:
            state = process_state(args.after, getattr(args, 'after_birth', None))
            if state == 'unknown':
                raise RuntimeError('turn-end process identity unknown; pass refused')
            if state == 'dead':
                break
            time.sleep(5)
        cmd_tick(args, table=False)
    except (SystemExit, Exception) as error:
        print(f'taskq: dispatch stopped: {str(error).removeprefix("taskq: ")}; resolve the blocker, then run a fresh tick', file=sys.stderr)
        raise SystemExit(1) from error

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
    """Observe versioned board events; only successful native delivery or explicit ack consumes them."""
    if args.task:
        return wait_task(args)
    identity, kinds, here = session() or {}, runtimes(), machine()
    me = args.pm or identity.get('session')
    def selected(raw):
        pm = raw.get('pm') or {}
        return (not me or pm.get('session') in (None, me)) and \
            (bool(args.pm) or not me or not pm or pm.get('runtime') == identity.get('runtime'))
    path = CONFIG['root'] / '.taskq' / (f'wait-{me}.json' if me else 'wait.json')
    legacy_seen = json.loads(path.read_text('utf-8')) if path.is_file() else {}
    end = time.time() + args.window * 60
    while True:
        events, legacy_now = [], {}
        issues = BOARD.list(None) + list(getattr(BOARD, 'closed', lambda: [])())
        for issue in issues:
            raw = issue_data(issue)
            if not raw or not selected(raw) or 'legacy_recovery' in raw:
                continue
            item = parse(issue) if issue['state'] == 'open' else parse({**issue, 'labels': [PREFIX + 'ready']})
            if not item or not mine(item):
                continue
            if raw.get('event_schema'):
                pm = raw.get('pm') or {'runtime': identity.get('runtime', 'owner'), 'session': me}
                target = recipient('manager', pm)
                held = raw.get('supervisor') or raw.get('claim') or {}
                sid = held.get('session')
                if issue['state'] == 'open' and (raw.get('supervisor') or item['state'] == 'doing') \
                        and sid and held.get('name') == here and held.get('runtime') in kinds \
                        and not os.environ.get('CODEX_SANDBOX') and sid not in raw.get('observed_dead', []) \
                        and lead_state(kinds[held['runtime']], sid) == 'dead':
                    with coordination():
                        fresh_issue = read_issue(issue['iid'])
                        fresh = issue_data(fresh_issue)
                        fresh_item = parse(fresh_issue) if fresh_issue['state'] == 'open' else None
                        if fresh.get('pm') != raw.get('pm') or not selected(fresh):
                            continue  # a new owner is not this observation's delivery recipient
                        if fresh_item and (fresh.get('supervisor') or fresh_item['state'] == 'doing') \
                                and (fresh.get('supervisor') or fresh.get('claim')) == held and sid not in fresh.get('observed_dead', []):
                            append_event(fresh, 'observed-gone', '', [target])
                            fresh['observed_dead'] = [*fresh.get('observed_dead', []), sid]
                            effect(BOARD.update, issue['iid'], labels=event_labels(fresh, fresh_issue['labels']), body=block(BLOCK.sub('', fresh_issue['body']).strip(), fresh))
                        raw = fresh
                for event in event_pending(raw, target):
                    events.append({'id': f'{issue["iid"]}:{event["id"]}', 'text': event_line(issue['iid'], event), 'target': target})
                continue
            if issue['state'] != 'open':
                continue
            claim, boss, state = item['claim'] or {}, item['supervisor'] or {}, item['state']
            if boss:
                state = 'gone' if not os.environ.get('CODEX_SANDBOX') and boss.get('name') == here and boss.get('runtime') in kinds \
                    and lead_state(kinds[boss['runtime']], boss['session']) == 'dead' else 'review*' if state == 'review' else state
            elif state == 'doing' and not os.environ.get('CODEX_SANDBOX') and claim.get('name') == here and claim.get('runtime') in kinds \
                    and lead_state(kinds[claim['runtime']], claim['session']) == 'dead':
                state = 'gone'
            n = str(item['iid'])
            legacy_now[n] = state
            if state in ('review', 'ask', 'gone') and legacy_seen.get(n) != state:
                events.append({'id': None, 'text': f'{state} #{n}', 'target': recipient('manager', raw.get('pm') or identity)})
        for n in legacy_seen:
            if n.isdigit() and n not in legacy_now:
                old = issue_data(read_issue(int(n)))
                if not old.get('event_schema') and selected(old) and (line := closed(n)):
                    events.append({'id': None, 'text': line, 'target': recipient('manager', old.get('pm') or identity)})
        if events or time.time() >= end:
            native = kinds.get('hermes') if identity.get('runtime') == 'hermes' and identity.get('session') == me else None
            native_target = recipient('manager', identity)
            if any(event.get('target') == native_target for event in events) and callable(getattr(native, 'owned', None)) and native.owned(me):
                with coordination():
                    # Re-read receipts under the board grant; concurrent waits cannot both consume an acked batch.
                    pending_events = []
                    for event in events:
                        if event.get('target') != native_target:
                            continue  # explicit sender --pm may observe another runtime, never wake this bridge for it
                        if not event['id']:
                            pending_events.append(event)
                            continue
                        n, number = map(int, event['id'].split(':'))
                        fresh = issue_data(read_issue(n))
                        if (not fresh.get('pm') or recipient('manager', fresh['pm']) == native_target) and \
                                any(value['id'] == number for value in event_pending(fresh, event['target'])):
                            pending_events.append(event)
                    if pending_events:
                        native.check()
                        if native.state(me) != 'idle':
                            fail('Hermes manager not confirmed idle; event receipt unchanged')
                        try:
                            if effect(native.wake_manager, me, '\n'.join(event['text'] for event in pending_events)) != me:
                                fail('Hermes manager wake changed identity; event receipt unchanged')
                        except Exception as error:
                            fail(f'Hermes manager wake failed ({type(error).__name__}); event receipt unchanged')
                        for event in pending_events:
                            if event['id']:
                                n, number = map(int, event['id'].split(':'))
                                acknowledge(read_issue(n), event['target'], [number])
                    events = [event for event in events if event.get('target') != native_target] + pending_events
            # Legacy installations retain their historical receipt path until explicit migration.
            if legacy_now or any(not issue_data(read_issue(int(n))).get('event_schema') for n in legacy_seen if n.isdigit()):
                path.parent.mkdir(exist_ok=True)
                path.write_text(json.dumps(legacy_now), 'utf-8')
            if getattr(args, 'json', False):
                print(json.dumps({'events': [{key: value for key, value in event.items() if key != 'target'} for event in events], 'tick': not events}))
            else:
                print('\n'.join(event['text'] for event in events) or 'tick')
            return
        legacy_seen = legacy_now
        time.sleep(args.every)


def wait_task(args):
    """The supervisor's wait (§ 7 Supervisor): block until its task needs it, print its events (`pending`), `stop #N` when the
    task is closed or no longer this session's, or `tick` after the window. Versioned events require explicit ack."""
    n, me, end = args.task, (session() or {}).get('session'), time.time() + args.window * 60
    while True:
        issue = read_issue(n)
        current = parse(issue) if issue['state'] == 'open' else None
        if current and 'legacy_recovery' in current['raw']:
            return print(f'stop #{n}: legacy recovery hold')
        boss = (current or {}).get('supervisor') or {}
        if not me or boss.get('session') != me:
            return print(f'stop #{n}')
        found, count = pending(issue if issue_data(issue).get('event_schema') else issue_history(issue), boss)
        if found or time.time() >= end:
            if found and not isinstance(count, list):
                seen(n, me, count)  # legacy compatibility only; versioned observation never consumes
            if getattr(args, 'json', False):
                return print(json.dumps({'events': [{'id': f'{n}:{number}', 'text': text} for number, text in zip(count, found)] if isinstance(count, list) else [], 'tick': not found}))
            return print('\n'.join(found) or 'tick')
        time.sleep(args.every)

SENDERS = {'claude': 'SendMessage'}
CLONE = Path(__file__).resolve().parent  # the taskq clone: its taskq.md is the manager contract (#430)

def contract():
    """The short hash of the clone's taskq.md, None without one."""
    path = CLONE / 'taskq.md'
    return hashlib.sha256(path.read_bytes()).hexdigest()[:12] if path.is_file() else None

def release_reason():
    """Managed launchers select once; older cooperative processes become read-only after pointer switch."""
    folder = os.environ.get('TASKQ_INSTALL_DIR')
    if not folder:
        return None  # direct source invocation is not a managed installation
    try:
        selected = json.loads((Path(folder) / 'current.json').read_text('utf-8'))
        path = Path(selected['path']).resolve()
        if path == CLONE.resolve() and selected['commit'] == CLONE.name and re.fullmatch(r'[0-9a-f]{40}', selected['commit']):
            return None
    except (OSError, ValueError, KeyError, TypeError):
        pass
    return 'stale or invalid installed release; use the taskq launcher and re-read taskq pm before further mutations'


def release_context():
    return f'Loaded TaskQ release: {CLONE}; contract {contract() or "unavailable"}. For a new turn use the taskq launcher and re-read its contract.'


def cmd_launch(args):
    """Stable bootstrap: validate and pin one release, then start its code and contract in a fresh interpreter."""
    if not args.install_dir:
        fail('launch needs --install-dir or TASKQ_INSTALL_DIR after a qualified explicit update')
    root = Path(args.install_dir).expanduser().resolve()
    try:
        selected = json.loads((root / 'current.json').read_text('utf-8'))
        commit, source = selected['commit'], selected['path']
        if not isinstance(commit, str) or not re.fullmatch(r'[0-9a-f]{40}', commit) or not isinstance(source, str):
            raise ValueError('pointer needs a full lowercase commit SHA and absolute source path')
        expected = root / 'releases' / commit
        path = Path(source)
        if not path.is_absolute() or path != expected or path.resolve() != expected:
            raise ValueError('release path is not the canonical installation release directory')
        for name in ('taskq.py', 'taskq.md'):
            target = expected / name
            if target.is_symlink() or not target.is_file():
                raise ValueError(f'release has no regular {name}')
    except (OSError, ValueError, KeyError, TypeError) as error:
        fail(f'launch invalid installation pointer/release: {error}')
    argv = args.arguments[1:] if args.arguments[:1] == ['--'] else args.arguments
    os.environ['TASKQ_INSTALL_DIR'] = str(root)
    os.environ['TASKQ_RELEASE_COMMIT'] = commit
    command = [sys.executable, str(expected / 'taskq.py'), *argv]
    if os.name == 'nt':  # Windows execv can exit the observed parent before the spawned interpreter finishes.
        raise SystemExit(subprocess.run(command).returncode)
    os.execv(sys.executable, command)


def cmd_version(args):
    git = shutil.which('git')
    data = {'source': str(CLONE), 'commit': None, 'dirty': None, 'contract': contract(), 'event_schema': EVENT_SCHEMA,
            'install': os.environ.get('TASKQ_INSTALL_DIR'), 'stale': bool(release_reason())}
    if git:
        for key, command in (('commit', ['rev-parse', 'HEAD']), ('dirty', ['status', '--porcelain', '--untracked-files=all'])):
            done = subprocess.run([git, '-C', str(CLONE), *command], capture_output=True, **no_window(), text=True, encoding='utf-8')
            if not done.returncode:
                data[key] = bool(done.stdout.strip()) if key == 'dirty' else done.stdout.strip()
    if data['install']:
        try:
            data['selected'] = json.loads((Path(data['install']) / 'current.json').read_text('utf-8'))
        except (OSError, ValueError):
            data['selected'] = None
    print(json.dumps(data, ensure_ascii=False))


def freshness_notice():
    """Managed-install availability only: bounded remote read, disposable local cache, never an update."""
    folder = os.environ.get('TASKQ_INSTALL_DIR')
    if not folder or not re.fullmatch(r'[0-9a-f]{40}', CLONE.name):
        return
    path, now = Path(folder) / '.freshness.json', time.time()
    cached = None
    try:
        candidate = json.loads(path.read_text('utf-8'))
        stamp, sha = candidate['checked_at'], candidate.get('upstream_sha')
        if type(stamp) in (int, float) and 0 <= now - stamp < 300 and (
                isinstance(sha, str) and re.fullmatch(r'[0-9a-f]{40}', sha)
                or sha is None and isinstance(candidate.get('error'), str)):
            cached = candidate
    except (OSError, ValueError, TypeError, KeyError):
        pass
    if cached is None:
        cached = {'checked_at': now, 'upstream_sha': None}
        try:
            gh = shutil.which('gh')
            if not gh:
                raise ValueError('gh not found')
            done = subprocess.run([gh, 'api', '--hostname', 'github.com', 'repos/alexkirs/taskq/git/ref/heads/main'],
                                  capture_output=True, **no_window(), text=True, encoding='utf-8', timeout=10)
            if done.returncode:
                raise ValueError('upstream query failed')
            sha = json.loads(done.stdout)['object']['sha']
            if not isinstance(sha, str) or not re.fullmatch(r'[0-9a-f]{40}', sha):
                raise ValueError('invalid upstream SHA')
            cached['upstream_sha'] = sha
        except (OSError, ValueError, TypeError, KeyError, subprocess.TimeoutExpired) as error:
            cached['error'] = 'query timed out' if isinstance(error, subprocess.TimeoutExpired) else str(error)[:160]
        try:
            path.write_text(json.dumps(cached) + '\n', encoding='utf-8')
        except OSError:
            print('taskq: update availability cache unavailable; later commands may check again', file=sys.stderr)
    if cached['upstream_sha'] is None:
        print(f'taskq: update availability unknown ({cached["error"]}); retry after the five-minute cache window', file=sys.stderr)
    elif cached['upstream_sha'] != CLONE.name:
        print(f'TaskQ upstream revision {cached["upstream_sha"]} differs from loaded {CLONE.name}; '
              'run taskq update to preview. Qualification and installation remain explicit.', file=sys.stderr)


def refresh(pm=False):
    """Compare the running release contract; managed installs also report bounded upstream availability."""
    path = CONFIG['root'] / '.taskq' / 'pm.json'
    known = json.loads(path.read_text('utf-8')).get('contract') if path.is_file() else None
    if not pm and contract() and known and known != contract():  # no pm.json: this session never took the role
        print('The manager contract changed: run taskq pm and follow it from now on.', file=sys.stderr)
    freshness_notice()

def legacy_reconcile_plan(issue, here, observers=None):
    """Read-only correlation, never an ownership transfer or operator drain attestation."""
    raw = issue_data(issue)
    digest = hashlib.sha256(issue['body'].encode('utf-8')).hexdigest()
    owners = []
    for role in ('pm', 'supervisor', 'claim'):
        identity = raw.get(role)
        if identity is None:
            continue
        expected = {'task': issue['iid'], 'role': role, 'identity': identity, 'source_sha256': digest}
        reason = None
        if not isinstance(identity, dict) or not all(isinstance(identity.get(k), str) and identity[k] for k in ('runtime', 'session', 'name')):
            reason = 'incomplete identity'
        elif identity['name'] != here:
            reason = 'foreign host: observe on recorded host'
        else:
            observer = (observers or {}).get(identity['runtime'])
            if not callable(observer):
                reason = 'qualified addressed drain observer unavailable'
            else:
                try:
                    observation = observer(expected)
                except (Exception, SystemExit):
                    observation = None
                if not isinstance(observation, dict) or any(observation.get(k) != v for k, v in expected.items()):
                    reason = 'unknown/stale observation'
                elif any(observation.get(k) is not False for k in ('writers', 'inflight', 'resources')) or observation.get('addressed_drain') is not True:
                    reason = 'active or unverified drain'
        owners.append({**expected, 'state': 'blocked' if reason else 'observed', 'reason': reason})
    return {'task': issue['iid'], 'source_sha256': digest, 'state': 'blocked' if any(o['reason'] for o in owners) else 'no unresolved observations',
            'owners': owners, 'apply_supported': False, 'preserve': 'entire source payload including pending answers/results/receipts; no effects'}


def cmd_reconcile(args):
    # No native legacy adapter is qualified. Never accept a caller file/flag as drain.
    plan = legacy_reconcile_plan(BOARD.get(args.n), machine())
    print(json.dumps(plan, sort_keys=True) if args.json else f'#{args.n}: {plan["state"]}; read-only; no reconcile apply adapter')


def migration_identity(item):
    """Qualify only existing identities; never synthesize births or replace sessions."""
    for role in ('pm','supervisor','claim'):
        identity = item.get(role)
        if identity is None:
            if role == 'pm':
                return 'missing PM; owner must explicitly select a legitimate PM before native migration'
            continue
        if not isinstance(identity,dict) or not all(isinstance(identity.get(k),str) and identity[k] for k in ('runtime','session','name')):
            return f'{role} identity incomplete; reconcile its original session/host, no implicit adoption'
        if identity['name'] != machine():
            return f'{role} {identity["runtime"]}:{identity["session"]} is foreign-host; reconcile on {identity["name"]}'
        if role == 'pm':
            if identity != origin():
                return f'PM {identity["runtime"]}:{identity["session"]} must invoke native migration itself'
            continue
        if identity['runtime'] != 'codex':
            return f'{role} runtime {identity["runtime"]} has no qualified migration identity/drain adapter'
        paths = [path for path in (Path(CONFIG['root'])/'.taskq').glob('*.pid')
                 if read_process(path)[1] == identity['session']]
        if len(paths) != 1:
            return f'{role} {identity["session"]} has no unique addressed native handle; owner must reconcile original runtime'
        pid,sid,birth = read_process(paths[0])
        if sid != identity['session'] or not birth or birth_domain(birth) != process_domain() or process_state(pid,birth) != 'dead':
            return f'{role} {identity["session"]} handle unknown/running; preserve it and pending data; obtain birth-qualified drain on original host'
        try:
            observation = Codex().observe(identity['session'])
        except (Exception, SystemExit):
            observation = {}
        writer = any(p.get('code') == 'active-writer' for p in observation.get('problems', [])) or observation.get('code') == 'active-writer'
        return f'{role} {identity["session"]} app ownership/drain ' + ('active writer' if writer else 'unqualified/unknown') + '; dead CLI handle and flags are insufficient; native migration refused'
    return None


def cmd_migrate(args):
    """Explicit board-only transition. Preflight the whole fresh snapshot before the first write."""
    if args.apply and not args.controllers_stopped:
        fail('migrate --apply requires --controllers-stopped after stopping/draining old controllers on every host')
    with coordination() if args.apply else contextlib.nullcontext():
        issues = [BOARD.get(issue['iid']) for issue in BOARD.list(None) if BLOCK.search(issue.get('body') or '')]
        changes = []
        native = args.native_receipts
        for issue in issues:
            item = parse(issue) if issue['state'] == 'open' else None
            if not item:
                continue
            version = item['raw'].get('event_schema', 0)
            if type(version) is not int or version not in ((0,1,2,EVENT_SCHEMA) if native else (0,1,EVENT_SCHEMA)):
                fail(f'#{item["iid"]}: unsupported event_schema {version!r}; migration refused')
            if native and version != 2:
                blocker = migration_identity(item)
                if blocker:
                    fail(f'#{item["iid"]}: native migration refused: {blocker}')
                fail(f'#{item["iid"]}: model execution adapter unqualified; notification-only native migration refused; keep schema1')
            elif version == 0:
                changes.append((issue, initialize_events(issue)))
        if args.apply:
            ensure = getattr(BOARD, 'ensure_event_label', None)
            if not callable(ensure):
                fail('board adapter needs ensure_event_label for explicit event-index setup')
            ensure()
        else:
            print('Setup requires reserved taskq-events label (checked/provisioned only on apply)')
        for issue, raw in changes:
            print(f'#{issue["iid"]} event_schema {issue_data(issue).get("event_schema",0)} -> {raw["event_schema"]}' + (' (apply)' if args.apply else ' (preview)'))
            if args.apply:
                # Replace only the matched block, preserving surrounding human prose byte-for-byte.
                write_task_verified(issue,raw,event_labels(raw,issue['labels']),preserve_text=True)
        print(f'{len(changes)} task(s) ' + ('migrated' if args.apply else 'would migrate; apply requires --controllers-stopped'))


SCHEMA_TRANSITIONS = {0: {'to': EVENT_SCHEMA, 'summary': 'Import pending messages; separate model turns from task/resource ownership.'},
                      1: {'to': EVENT_SCHEMA, 'summary': 'Preserve existing events/acks; separate model turns from task/resource ownership.'}}


def repair_plan(hold_legacy=False):
    """Shipped transitions only. Preflight the entire fresh board before any effect."""
    changes = []
    for listed in BOARD.list(None):
        if not BLOCK.search(listed.get('body') or ''):
            continue
        issue = BOARD.get(listed['iid'])
        if issue.get('state') != 'open':
            fail(f'#{issue["iid"]}: task changed since repair snapshot; re-plan without writes')
        raw = issue_data(issue)
        version = raw.get('event_schema', 0)
        if version == EVENT_SCHEMA:
            continue
        transition = SCHEMA_TRANSITIONS.get(version)
        if not transition or transition['to'] != EVENT_SCHEMA:
            fail(f'#{issue["iid"]}: no qualified repair from schema {version} to {EVENT_SCHEMA}; board unchanged')
        for role in ('claim', 'supervisor'):
            if (raw.get(role) or {}).get('session') and not hold_legacy:
                fail(f'#{issue["iid"]}: {role} ownership unresolved; repair cannot retire or replace a session')
        converted = initialize_events(issue)
        if 'schema_repair' in raw:
            fail(f'#{issue["iid"]}: conflicting repair receipt; preserve it for reconciliation')
        if any(converted.get(key) != value for key, value in raw.items() if key != 'event_schema'):
            fail(f'#{issue["iid"]}: repair would overwrite existing data; qualified transition required')
        converted['schema_repair'] = {'from': version, 'to': EVENT_SCHEMA,
                                      'source_sha256': hashlib.sha256(issue['body'].encode('utf-8')).hexdigest()}
        if hold_legacy and any((raw.get(role) or {}).get('session') for role in ('claim', 'supervisor')):
            if 'legacy_recovery' in raw:
                fail(f'#{issue["iid"]}: conflicting legacy recovery record; preserve it')
            converted['legacy_recovery'] = {'kind': 'unresolved-ownership',
                'source_sha256': converted['schema_repair']['source_sha256'],
                'from': version, 'drain_proven': False}
        changes.append((issue, converted))
    return changes


def cmd_repair(args):
    """Explicit consent, exact readback and resumable per-issue stamps; no worker effects."""
    if args.apply and not args.yes:
        fail('repair --apply requires --yes after reviewing taskq repair; no board changes')
    with coordination() if args.apply else contextlib.nullcontext():
        changes = repair_plan(getattr(args, 'hold_legacy', False))
        for issue, raw in changes:
            print(f'#{issue["iid"]}: schema {raw["schema_repair"]["from"]} -> {EVENT_SCHEMA}: '
                  + SCHEMA_TRANSITIONS[raw['schema_repair']['from']]['summary'])
        if args.apply and changes:
            ensure = getattr(BOARD, 'ensure_event_label', None)
            if not callable(ensure):
                fail('board adapter lacks event-label provisioning; repair refused')
            effect(ensure)
            for issue, raw in changes:
                if BOARD.get(issue['iid']) != issue:
                    fail(f'#{issue["iid"]}: repair source changed; preserved newer state, re-plan')
                write_task_verified(issue, raw, event_labels(raw, issue['labels']), preserve_text=True)
            if repair_plan():
                fail('repair verification incomplete; do not start workers')
        print(f'{len(changes)} task(s) ' + ('repaired and verified' if args.apply else 'require repair; confirm with taskq repair --apply --yes'))


def board_schema_gate(issues=None):
    """Any incompatible open task blocks ordinary board execution, including mixed boards."""
    for issue in BOARD.list(None) if issues is None else issues:
        if not BLOCK.search(issue.get('body') or ''):
            continue
        version = issue_data(issue).get('event_schema', 0)
        if version != EVENT_SCHEMA:
            fail(f'#{issue["iid"]}: board schema {version} differs from required {EVENT_SCHEMA}; run taskq repair; no work started')


def update_git(folder, *argv):
    try:
        done = subprocess.run([shutil.which('git') or fail('git not found'), '-C', str(folder), *argv],
                              capture_output=True, **no_window(), text=True, encoding='utf-8', timeout=120 if argv[0] == 'clone' else 30)
    except subprocess.TimeoutExpired:
        fail(f'update git {argv[0]} timed out; selected pointer unchanged')
    if done.returncode:
        fail(f'update git {argv[0]}: {last_line(done.stderr + done.stdout)}')
    return done.stdout.strip()


def qualified_checks(commit, upstream):
    """An operator record is not CI. Require the actual exact-SHA TaskQ upstream tests independently."""
    if not re.fullmatch(r'(?:https://github\.com/|git@github\.com:)alexkirs/taskq(?:\.git)?/?', upstream):
        fail('update requires the canonical github.com/alexkirs/taskq upstream')
    try:
        done = subprocess.run([shutil.which('gh') or fail('gh not found'), 'api', '--hostname', 'github.com', '--paginate', '--slurp',
                               f'repos/alexkirs/taskq/commits/{commit}/check-runs'],
                              capture_output=True, **no_window(), text=True, encoding='utf-8', timeout=30)
    except subprocess.TimeoutExpired:
        fail('update exact-SHA CI timed out; selected pointer unchanged')
    if done.returncode:
        fail(f'update exact-SHA CI unavailable: {last_line(done.stderr + done.stdout)}')
    try:
        pages = json.loads(done.stdout)
        checks = [check for page in pages for check in page['check_runs'] if check['name'] == 'tests']
        green = bool(checks) and all(check['head_sha'] == commit and check['status'] == 'completed'
                                     and check['conclusion'] == 'success' for check in checks)
    except (ValueError, KeyError, TypeError):
        green = False
    if not green:
        fail('update requires completed successful tests checks for the exact qualified SHA')


@contextlib.contextmanager
def installation_lock(root):
    """Local installation serialization only, never task state or a board coordination substitute."""
    root.mkdir(parents=True, exist_ok=True)
    path, token = root / '.update.lock', uuid.uuid4().hex
    try:
        with path.open('x', encoding='utf-8') as handle:
            handle.write(token)
    except FileExistsError:
        fail(f'update installation busy: {path}; no expiry/steal; reconcile a stopped installer explicitly')
    try:
        yield
    finally:
        if path.read_text('utf-8') != token:
            fail(f'update installation lock changed; retaining {path} for reconciliation')
        path.unlink()  # exact owned file only; never delete a release or another installer's lock


def verify_release(release, commit, upstream):
    if update_git(release, 'remote', 'get-url', 'origin') != upstream:
        fail('update release origin differs; preserving it and the pointer')
    update_git(release, 'merge-base', '--is-ancestor', commit, 'refs/remotes/origin/main')
    if update_git(release, 'rev-parse', 'HEAD') != commit or update_git(release, 'status', '--porcelain', '--untracked-files=all'):
        fail('update release verification failed; preserving it and the pointer')
    for name in ('taskq.py', 'taskq.md'):
        entry = update_git(release, 'ls-tree', 'HEAD', '--', name)
        if not entry.startswith(('100644 ', '100755 ')) or not (release / name).is_file():
            fail(f'update release has no regular {name}; pointer unchanged')


def cmd_update(args):
    """Install a qualified exact upstream revision without changing any running source checkout."""
    if not args.install_dir:
        fail('unmanaged installation: use update --install-dir <directory>; the managed launcher supplies it thereafter')
    upstream = update_git(CLONE, 'remote', 'get-url', 'origin')
    candidate = args.commit
    if candidate is None:
        lines = update_git(CLONE, 'ls-remote', '--exit-code', 'origin', 'refs/heads/main').splitlines()
        if len(lines) != 1 or len(lines[0].split()) != 2 or lines[0].split()[1] != 'refs/heads/main':
            fail('update could not resolve one exact upstream main SHA')
        candidate = lines[0].split()[0]
    if not re.fullmatch(r'[0-9a-f]{40}', candidate):
        fail('update --commit requires a full lowercase 40-character commit SHA')
    root = Path(args.install_dir).expanduser().resolve()
    release = root / 'releases' / candidate
    print(f'Update {candidate} from {upstream}\nRelease: {release}\nPointer: {root / "current.json"}')
    if not args.apply:
        return print(f'Preview only; next: taskq update --install-dir {shlex.quote(str(root))} --commit {candidate} --apply --qualification <reviewed-evidence.json>')
    if not args.qualification:
        fail('update --apply requires --qualification with reviewed exact-SHA test evidence; upstream CI is checked independently')
    try:
        evidence = json.loads(Path(args.qualification).read_text('utf-8'))
    except (OSError, ValueError) as error:
        fail(f'update qualification unreadable: {error}')
    if not isinstance(evidence, dict) or any(evidence.get(key) != value for key, value in
            dict(commit=candidate, upstream=upstream, tests='passed', review='accepted').items()):
        fail('update qualification must record this commit/upstream, tests passed and review accepted')
    qualified_checks(candidate, upstream)
    with installation_lock(root):
        previous = root / 'current.json'
        selected = None
        if previous.is_file():
            try:
                selected = json.loads(previous.read_text('utf-8'))
                if not re.fullmatch(r'[0-9a-f]{40}', selected['commit']) or not isinstance(selected['path'], str):
                    raise ValueError('invalid pointer')
            except (OSError, ValueError, KeyError, TypeError) as error:
                fail(f'update previous pointer invalid; reconcile explicitly: {error}')
        if not release.exists():
            release.parent.mkdir(parents=True, exist_ok=True)
            # Failure leaves evidence; neither existing source nor active pointer is changed.
            update_git(root, 'clone', '--no-checkout', '--single-branch', '--branch', 'main', '--', upstream, str(release))
            update_git(release, 'merge-base', '--is-ancestor', candidate, 'refs/remotes/origin/main')
            update_git(release, 'checkout', '--detach', candidate)
        verify_release(release, candidate, upstream)  # retries may reuse only an intact exact qualified release
        if selected:
            update_git(release, 'merge-base', '--is-ancestor', selected['commit'], candidate)
        temporary = root / f'.current-{uuid.uuid4().hex}.json'
        temporary.write_text(json.dumps({'commit': candidate, 'path': str(release)}) + '\n', encoding='utf-8')
        os.replace(temporary, previous)
    print('Installed for new launcher processes; running releases unchanged. Board migration remains explicit. '
          'Before work, the selected release checks board schema; review taskq repair if incompatible.')


def adopt(numbers, me):
    """R3 Transition (#532): record this session as the `pm` of tasks with none. The project guard and a fresh read keep
    cooperating adoptions across checkouts and machines from overwriting each other (R4)."""
    if not origin():
        fail('cannot adopt: a plain shell needs TASKQ_RUNTIME')
    with coordination():
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
    runtime = me.get('runtime') or os.environ.get('TASKQ_RUNTIME')
    tq = queue_tool(runtime)
    with coordination() if args.adopt else contextlib.nullcontext():
        snapshot = [item for item in map(parse, BOARD.list(None)) if item]
        if args.adopt:
            snapshot = list(filter(None, (parse(read_issue(item['iid'])) for item in snapshot)))
        if me and any(role(item) in ('supervisor', 'worker') for item in snapshot):
            fail('a recorded supervisor or worker cannot take the manager role (R3 one controller)')
        if args.adopt:
            adopt(args.adopt, me)
    text, digest = (CLONE / 'taskq.md').read_text('utf-8'), contract()
    sections = re.findall(r'^## (?:Principles|7\. Manager)\b.*?(?=^## )', text, re.M | re.S)
    (CONFIG['root'] / '.taskq').mkdir(exist_ok=True)
    (CONFIG['root'] / '.taskq' / 'pm.json').write_text(json.dumps({'contract': digest}), 'utf-8')
    print(f'taskq pm contract {digest}\nYou are the taskq manager of {CONFIG["root"]}. Follow this role from now on; '
          f'`taskq` is `{tq}`.' + (f' {shell_instructions(runtime)}' if powershell(runtime) else '') + '\n\n' + '\n'.join(sections))
    for item in sorted(snapshot, key=lambda item: (item['priority'], item['iid'])):
        if not item['pm'] and item['iid'] not in args.adopt:
            print(f'Unassigned manager: #{item["iid"]} {item["title"]} ({item["state"]}). '
                  f'Triage explicitly; to adopt in project {CONFIG["root"].name}: '
                  f'`' + (f'Set-Location -LiteralPath {shell_quote(CONFIG["root"], True)} -ErrorAction Stop; {tq} pm --adopt {item["iid"]}'
                          if powershell(runtime) else f'cd {shlex.quote(str(CONFIG["root"]))} && taskq pm --adopt {item["iid"]}')
                  + '`. No ownership or claims changed.')
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
    native = powershell(runtime)
    tq = queue_tool(runtime)
    wait = f'{tq} wait'
    action = ('Explicit owner arm: execute the proven route, not just this prompt. Reuse the existing monitor and targeted wait; '
             'repeated arm must not create duplicates. Keep paused projects paused. Prove an idle-manager wake and the next wait; '
             'printed output is not proof. No supported access to the existing sender: report one blocker through the existing task, '
              'never create a replacement sender or bridge. See § 7 Arm the tick.\n')
    if native:
        action += shell_instructions(runtime) + f' `taskq` means `{tq}`.\n'
    if runtime == 'hermes':
        return print(action + 'Hermes manager wake/event delivery requires the configured native external bridge. '
                     'No background wake, restart durability or unattended event delivery is verified; '
                     'outcomes remain on the board until read. No Codex resume route.')
    start = action + ('Start: run one pass now (`taskq tick`, outside a Codex sandbox). From then on the approved queue runs by itself (R4): '
             'supervisors, workers, reviews, reworks, closes and the next task start on queue events and Codex turn ends; '
             'no sender, timer or extension. Arming below only brings you the short outcomes.\n')
    if not args.target and runtime == 'codex':  # #510: a Codex session is not woken when a background command ends
        thread = os.environ.get('CODEX_THREAD_ID') or '<this thread>'
        manager = native_command([shutil.which('codex') or 'codex', *shlex.split(CODEX_COMPACT)]) if native else f'codex {CODEX_COMPACT}'
        return print(f'''{start}Arm the tick in this session. Codex is not woken when a background command ends, so tick in the foreground:
loop {{ run `{wait}`; on its output (`ask #N`, `closed #N <verdict>`, `review #N`, `gone #N` or `tick`) run one pass (`taskq tick`) and do
§ 7 After each pass for those tasks; for schema1 acknowledge handled [event N:ID] with `taskq ack N:ID`; for schema2 apply supported ask/result through `{tq} apply-event N:ID --role manager --artifact taskq-event-N-ID.txt` }}. Between turns the outcomes wait on the board for your next pass; the queue does not.
Optional, only to be woken between turns: `{tq} arm tick {shell_quote(thread, native) if native else thread}` prints a sender prompt for a
thread with a local rollout only; no wake of a Codex app thread is promised (#522).
Codex manager: start it with `{manager}`.''')
    if not args.target:  # no target: this session ticks itself (Claude: a background command wakes the session on exit)
        return print(f'''{start}Arm the tick in this session. Run `{wait}` as a background command (Claude Code: run_in_background).
When it ends you are woken with its output (`ask #N`, `closed #N <verdict>`, `review #N`, `gone #N` or `tick`): run one pass (`taskq tick`),
do § 7 After each pass for those tasks, for schema1 acknowledge handled [event N:ID] with `taskq ack N:ID`; for schema2 apply supported ask/result through `{tq} apply-event N:ID --role manager --artifact taskq-event-N-ID.txt`, then start `{wait}` in the background again. Keep exactly one wait running.
Codex manager: start it with `codex {CODEX_COMPACT}` (Claude: .claude/settings.json autoCompactWindow 200000).''')
    pm = re.split(r'session_|threads/|/', args.target)[-1]  # #532: the sender observes its manager, without acknowledging
    resume = f'codex exec {shlex.join(codex_options())} resume {shlex.quote(pm)}'  # the options a worker turn gets
    if native:
        resume = native_command([shutil.which('codex') or 'codex', 'exec', *codex_options(), 'resume', pm, '-'])
    wait = f'{wait} --pm {shell_quote(pm, native)}'
    send, shell, note = f'with {SENDERS.get(runtime, "your messaging tool")}', '', ''
    where = rollout(pm) if runtime == 'codex' else None
    if where == 'local':  # a CLI thread: exec resume finds it
        send = f'by running `{resume} "<its output>"`: a new turn on that thread wakes it'
        if native:
            send = ('by setting `[Console]::OutputEncoding = $OutputEncoding = [System.Text.UTF8Encoding]::new($false)` '
                    f'and piping its output to `{resume}` as stdin: a new turn on that thread wakes it')
        shell = (f'\nNo agent needed: `cd {CONFIG["root"]} && while e=$({wait}) && {resume} "$e"; do :; done; '
                 'echo "taskq sender stopped"` in a terminal.')
        if native:
            shell = (f'\nNo agent needed: `$ErrorActionPreference=\'Stop\'; Set-Location -LiteralPath {shell_quote(CONFIG["root"], True)} -ErrorAction Stop; '
                     '[Console]::OutputEncoding = $OutputEncoding = [System.Text.UTF8Encoding]::new($false); '
                     f'while ($true) {{ $e = {wait}; if ($LASTEXITCODE -ne 0) {{ break }}; '
                     f'$e | {resume}; if ($LASTEXITCODE -ne 0) {{ break }}; '
                     '}; '
                     'Write-Output "taskq sender stopped"` in a terminal.')
    elif where == 'archived':  # #522: exec resume of an archived thread is unverified (R12): no route
        return print(f'taskq: {pm} is archived in Codex; `exec resume` of an archived thread is unverified, so no sender.\n'
                     f'Run `codex unarchive {pm}`, then `taskq arm tick {pm}` again.')
    elif runtime == 'codex':  # #522: unknown target; an app thread fails `no rollout found` (#269)
        send = 'with `send_message_to_thread`'
        note = (f'taskq: no local Codex rollout of {pm}, so no `codex exec resume` and no promised wake. Unknown what it is: '
                'a Codex app thread (exec resume fails `no rollout found`), a thread name, a typo or another machine\'s thread.\n'
                f'Only if {pm} is a known Codex app thread: run this prompt in an independent, user-visible Codex app session '
                'whose send_message_to_thread reaches it. Not a collaboration subagent of the manager: it cannot send to its ancestor '
                'and starts no turn. A session without that tool (a CLI worker) hands this prompt to the owner or the app manager.\n'
                'Workers still dispatch without a sender (R4 event chain); only review, ask and gone wait for the manager.\n\n')
    if not any((item['pm'] or {}).get('session') == pm for item in map(parse, BOARD.list(None)) if item):
        note += f'taskq: no open task records {pm} as its pm; this wait shows only tasks with no manager until one does.\n'
    print(f'''{action}{note}You are the taskq tick sender for the manager session {pm}. Do no task work; use only wait and delivery. Never acknowledge manager events.
Stay in this one turn and repeat, from {CONFIG["root"]}; do not end the turn between events (an ended turn forwards nothing):
1. Run `{wait}`. It blocks until the manager is needed (at most 10 minutes) and prints one line per event.
2. Send its output, verbatim, to {pm} {send}.
3. Do not run ack: the recorded manager owns application acknowledgement. Delivery success is not application.
   Schema2 ask/result requires the manager native apply-event handler; unsupported handling remains pending on the board.
4. Go back to 1 at once. A failed wait, a failed send or no such send tool: stop, say here
   `taskq sender stopped: <error>` once; never retry, never another route.{shell}''')

def main(argv=None):
    global CONFIG, BOARD
    if os.name == 'nt':  # redirected Windows streams otherwise use a legacy code page that rejects task titles/contract
        for stream in (sys.stdout, sys.stderr):
            if hasattr(stream, 'reconfigure'):
                stream.reconfigure(encoding='utf-8')
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
            (('--priority',), {'type': int, 'choices': (1, 2), 'default': 2}), (('--host',), {}),
            (('--later',), {'action': 'store_true'}), n=False)
    command('list', cmd_list, (('state',), {'nargs': '?', 'choices': STATES}), n=False)
    command('take', cmd_take)
    card = ((('--option',), {'action': 'append', 'default': []}), (('--recommend',), {'type': int, 'default': 1}),
            (('--link',), {'action': 'append', 'default': []}))  # #490: the decision card
    command('ask', cmd_move, *card, text='required')
    command('answer', cmd_answer, (('n',), {'nargs': '+'}), n=False, text=True)
    command('result', cmd_move, (('--sha',), {'type': commit}), (('--checks',), {'default': ''}), *card, text=True)
    command('requeue', cmd_requeue, text=True)
    command('run', cmd_run, text=True)
    command('later', cmd_move, text=True)
    command('close', cmd_close, (('n',), {'nargs': '+', 'type': int}), n=False, text=True)
    command('status', cmd_status, (('--diagnose',), {'action': 'store_true'}), n=False)
    command('recovery-plan', cmd_recovery_plan, (('--role',), {'choices': ('worker', 'supervisor'), 'required': True}), (('--json',), {'action': 'store_true'}))
    command('reconcile', cmd_reconcile, (('--json',), {'action': 'store_true'}))
    command('tick', lambda args: (event_pass if args.quiet else cmd_tick)(args), (('--quiet',), {'action': 'store_true'}),
            (('--headless',), {'action': 'store_true'}),
            (('--tasks',), {'nargs': '*', 'type': int, 'default': []}), (('--after',), {'type': int}), (('--after-birth',), {}),
            (('--diagnose',), {'action': 'store_true'}), (('--unknown-after',), {'type': positive_minutes, 'nargs': '?', 'const': 30}), n=False)
    command('wait', cmd_wait, (('--window',), {'type': float, 'default': 10}), (('--every',), {'type': float, 'default': 25}),
            (('--task',), {'type': int}), (('--pm',), {}), (('--json',), {'action': 'store_true'}), n=False)
    command('capacity-child', lambda args: controlled_child(args.input), (('--input',), {'required':True}), n=False)
    command('lifecycle', cmd_lifecycle, (('action',), {'choices':('admit','resume','park','reconcile','answer','accept')}), (('--text',), {}))
    command('applied', cmd_applied, (('event',), {}), (('--artifact',), {'required': True}), (('--sha',), {'required': True}), n=False)
    command('apply-event', cmd_apply_event, (('event',), {}), (('--artifact',), {'required': True}), (('--role',), {'choices': ('worker', 'manager'), 'default': 'worker'}), n=False)
    command('ack', cmd_ack, (('events',), {'nargs': '*'}), (('--pm',), {}), (('--role',), {'choices': ('manager','supervisor')}), (('--stdin',), {'action': 'store_true'}), n=False)
    command('arm', cmd_arm, (('what',), {'choices': ('tick',)}), (('target',), {'nargs': '?'}), n=False)
    command('pm', cmd_pm, (('--adopt',), {'nargs': '+', 'type': int, 'default': []}), n=False)
    command('cleanup', cmd_cleanup, (('--dry-run',), {'action': 'store_true'}), n=False)
    command('launch', cmd_launch, (('--install-dir',), {'default': os.environ.get('TASKQ_INSTALL_DIR')}),
            (('arguments',), {'nargs': argparse.REMAINDER}), n=False)
    command('version', cmd_version, n=False)
    command('contract', lambda args: print(f'{CLONE / "taskq.md"} {contract() or "unavailable"}'), n=False)
    command('migrate', cmd_migrate, (('--native-receipts',), {'action': 'store_true'}), (('--apply',), {'action': 'store_true'}),
            (('--controllers-stopped',), {'action': 'store_true'}), n=False)
    command('repair', cmd_repair, (('--apply',), {'action': 'store_true'}), (('--yes',), {'action': 'store_true'}),
            (('--hold-legacy',), {'action': 'store_true'}), n=False)
    command('update', cmd_update, (('--commit',), {}), (('--install-dir',), {'default': os.environ.get('TASKQ_INSTALL_DIR')}),
            (('--qualification',), {}), (('--apply',), {'action': 'store_true'}), n=False)
    args = parser.parse_args(argv)
    if args.command in ('launch', 'update', 'version', 'contract', 'capacity-child'):
        return args.function(args)  # source installation needs no consumer project or board adapter
    if BOARD is None:
        CONFIG = load_config()
        BOARD = make_board(CONFIG)
    local_limits()  # fail before any command writes, refreshes, starts an event or admits work
    if args.command=='lifecycle' and (not isinstance(CONFIG.get('capacity'),dict) or not callable(getattr(BOARD,'capacity_provider',None))):
        fail('qualified board capacity provider and explicit host/project caps required; no defaults')
    if 'TASKQ_HOST_ONLY' in os.environ and (not os.environ['TASKQ_HOST_ONLY'] or os.environ['TASKQ_HOST_ONLY'] != machine()):
        fail('TASKQ_HOST_ONLY must equal this machine name (TASKQ_HOST / hosts)')
    if args.command == 'wait' and not args.task or args.command == 'tick' and not args.quiet or args.command == 'pm':
        refresh(args.command == 'pm')
    writes = args.command in ('add', 'take', 'ask', 'answer', 'result', 'requeue', 'run', 'later', 'close', 'ack', 'applied', 'apply-event', 'lifecycle') or \
        args.command == 'cleanup' and not args.dry_run
    if (writes or args.command in ('migrate','repair') and args.apply or args.command == 'tick' or args.command == 'pm' and args.adopt) and getattr(GUARD, 'held', None):
        raise SystemExit('taskq: another command holds the project guard; a new command must acquire independently')
    if (writes or args.command in ('tick', 'wait') or args.command in ('migrate','repair') and args.apply
            or args.command == 'pm' and args.adopt) and release_reason():
        fail(release_reason())
    try:
        with coordination() if writes else contextlib.nullcontext():
            if args.command in ('add','take','ask','answer','result','requeue','run','later','close','cleanup','ack','applied','apply-event','lifecycle') or args.command=='pm' and args.adopt:
                board_schema_gate()
            done = args.function(args)
    except SettledError as error:
        if error.tasks and not (args.command == 'add' and args.later):
            dispatch(args.command, error.tasks)
        raise
    if args.command in EVENTS and not (args.command == 'add' and args.later):
        dispatch(args.command, done if args.command in ('add', 'answer') else args.n)

if __name__ == '__main__':
    main()
