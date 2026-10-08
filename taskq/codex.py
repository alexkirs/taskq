"""The Codex app server: worker threads, their live state, archive."""
import base64
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import socket
import time

import taskq as core


# --- Codex app server (the desktop app's shared one); JSON-RPC over a WebSocket on a unix socket ---

# The Codex CLI's app-server daemon owns this socket; the desktop app only starts that daemon. Headless (#160):
# `codex app-server daemon start` (`bootstrap` keeps it across reboots) serves the same socket without the app.
CODEX_SOCKET = Path.home() / '.codex/app-server-control/app-server-control.sock'
CODEX_HEADLESS = 'codex login --device-auth && codex app-server daemon start'
CODEX_IPC = Path.home() / '.codex/ipc/ipc.sock'
# A loaded thread's writer lock; the file goes away when its server releases the thread (#158).
CODEX_LOCKS = Path.home() / '.codex/thread-writer-locks'
# Design decision 2026-10-07 (#57 probe, #149): a Codex worker runs in workspace-write with network on
# and never asks; every step of a worker passed there. The worktree lives in ROOT/.worktrees, but git
# writes its refs, objects and worktree admin files into the main checkout's .git, so .git is a root.
# The taskq state dir holds the machine id and update stamp. taskq.toml [codex] writable adds the project's
# own roots outside the checkout (#154, csgo's media root); a missing one is skipped, doctor warns.
# Owner decision 2026-10-07 (#157): workspace-write denies the Apple GPU (IOKit AGXDeviceUserClient) and Codex has
# no GPU-only setting, so a task labelled FULL_ACCESS runs its Codex turns with danger-full-access instead.
def codex_turn_policy(full_access=False):
    if full_access:
        return {'approvalPolicy': 'never', 'sandboxPolicy': {'type': 'dangerFullAccess'}}
    # A missing root grants nothing, and on Linux one under the thread's cwd (.worktrees before the first
    # tree) makes bwrap refuse every command (#164), so only existing roots are listed.
    roots = [root for root in (core.ROOT / '.git', core.ROOT / '.worktrees', core.UPDATE_STAMP.parent,
                               *core.codex_writable()) if root.exists()]
    # #163: on Linux Codex mounts the gitdir of a root that is a linked worktree read-only after the writable
    # roots (openai/codex#14338) unless that exact gitdir is a root too. Only a gitdir inside this .git, already
    # writable, is added: no new access.
    common = (core.ROOT / '.git').resolve()
    roots += [gitdir for gitdir in map(worktree_gitdir, roots) if gitdir and common in gitdir.resolve().parents]
    return {'approvalPolicy': 'never', 'sandboxPolicy': {
        'type': 'workspaceWrite', 'networkAccess': True, 'writableRoots': [str(root) for root in roots]}}


def worktree_gitdir(tree):
    """The existing gitdir a linked worktree's `.git` file names, spelled as Codex resolves it; else None."""
    try:
        text = (tree / '.git').read_text().strip()
    except OSError:  # no .git, or a directory
        return None
    gitdir = Path(os.path.normpath(tree / text.removeprefix('gitdir:').strip())) if text.startswith('gitdir:') else None
    return gitdir if gitdir and gitdir.is_dir() else None


# thread/start and thread/resume use the CLI spelling; turn/start carries the full policy.
CODEX_ACCESS = {'sandbox': 'workspace-write', 'approvalPolicy': 'never'}


def codex_access(full_access=False):
    return {**CODEX_ACCESS, 'sandbox': 'danger-full-access'} if full_access else CODEX_ACCESS


class Codex:
    def __init__(self, timeout=60):
        from taskq.worker import no_live_in_tests
        no_live_in_tests('Codex app server')
        import base64
        import socket
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(timeout)
        self.socket.connect(str(CODEX_SOCKET))
        key = base64.b64encode(os.urandom(16)).decode()
        self.socket.sendall(('GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
                             f'Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n').encode())
        head = b''
        while b'\r\n\r\n' not in head:
            head += self.exact(1)
        if b' 101 ' not in head.split(b'\r\n')[0]:
            core.fail(f'Codex app server refused the connection: {head[:80]}')
        self.counter = 0
        self.permission_requests = {}
        self.call('initialize', {'clientInfo': {'name': 'taskq', 'version': '1'}, 'capabilities': {'experimentalApi': True}})
        self.send({'method': 'initialized'})

    def exact(self, size):
        data = b''
        while len(data) < size:
            data += self.socket.recv(size - len(data)) or core.fail('Codex app server closed the connection')
        return data

    def send(self, value):
        import struct
        data, mask = json.dumps(value).encode(), os.urandom(4)
        size = len(data)
        head = bytes([0x81]) + (bytes([0x80 | size]) if size < 126 else bytes([0xfe]) + struct.pack('>H', size)
                                if size < 65536 else bytes([0xff]) + struct.pack('>Q', size))
        self.socket.sendall(head + mask + bytes(byte ^ mask[index % 4] for index, byte in enumerate(data)))

    def receive(self):
        import struct
        message = b''
        while True:
            first, second = self.exact(2)
            size = second & 0x7f
            size = struct.unpack('>H', self.exact(2))[0] if size == 126 else struct.unpack('>Q', self.exact(8))[0] if size == 127 else size
            payload, opcode = self.exact(size), first & 0x0f
            if opcode == 9:
                self.socket.sendall(bytes([0x8a, 0x80]) + b'\0\0\0\0')
                continue
            if opcode == 8:
                core.fail('Codex app server closed the connection')
            message += payload
            if first & 0x80:
                return json.loads(message)

    def call(self, method, params):
        self.counter += 1
        self.send({'id': self.counter, 'method': method, 'params': params})
        while True:
            value = self.receive()
            self.observe(value)
            if value.get('id') == self.counter and 'method' not in value:
                if 'error' in value:
                    core.fail(f'Codex {method}: {value["error"]}')
                return value['result']

    def observe(self, value):
        """Keep supported approval metadata, never reply to or accept a server request."""
        method, params = value.get('method'), value.get('params') or {}
        key = (params.get('threadId'), value.get('id'))
        if method in ('item/commandExecution/requestApproval', 'item/fileChange/requestApproval',
                      'item/permissions/requestApproval') and key[0] and key[1] is not None:
            self.permission_requests[key] = {
                'request_id': key[1], 'turn_id': params.get('turnId'), 'item_id': params.get('itemId'),
                'event_at': params['startedAtMs'] / 1000 if params.get('startedAtMs') is not None else None, 'method': method}
        elif method == 'serverRequest/resolved':
            self.permission_requests.pop((params.get('threadId'), params.get('requestId')), None)

    def wait_turn(self, thread, seconds):
        end = time.time() + seconds
        while time.time() < end:
            value = self.receive()
            self.observe(value)
            if value.get('method') == 'turn/completed' and value['params'].get('threadId') == thread:
                return
        core.fail(f'Codex thread {thread}: no turn/completed in {seconds} s')


def codex_observation(codex, thread, status, turns, stamp):
    """Unknown includes empty flags and unobserved approvals; active requires a running typed item."""
    pending = [value for (session, _), value in getattr(codex, 'permission_requests', {}).items() if session == thread]
    flags = status.get('activeFlags') or []
    latest = turns[0] if turns else {}
    running = [entry['item'] for entry in latest.get('entries', [])
               if entry['item'].get('status') == 'inProgress' and
               entry['item'].get('type') in ('commandExecution', 'mcpToolCall', 'dynamicToolCall', 'liveToolCall')]
    waiting = bool(pending) or 'waitingOnApproval' in flags
    terminal = status.get('type') != 'active' and latest.get('status') in ('completed', 'failed', 'interrupted')
    state = ('waiting_permission' if waiting else 'terminal' if terminal else
             'active' if latest.get('status') == 'inProgress' and running else 'unknown')
    return {'runtime': 'codex', 'session': thread, 'status': state,
            'observed_at': datetime.now(timezone.utc).isoformat(), 'event_at': stamp,
            'session_link': f'{core.PAGES.rstrip("/")}/open.html#codex://threads/{thread}',
            'exact_blocker': ('Owner approval required in runtime UI' if waiting else
                              'Owner input required in runtime UI' if 'waitingOnUserInput' in flags else
                              'No current execution or terminal evidence; approval visibility incomplete' if state == 'unknown' else None),
            'source': 'codex app-server', 'conflict': False,
            'permission_requests': pending,
            'approval_visibility': 'pending' if waiting else 'unknown',
            'notify_dedup': (f'permission codex {thread} ' + ','.join(sorted(str(item['request_id']) for item in pending))
                             if pending else f'permission codex {thread} flag' if waiting else None)}


class CodexIpc:
    """The app's IPC router (4-byte little-endian length, then JSON); versions are the app's own table."""
    def __init__(self, timeout=60):
        import socket
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(timeout)
        self.socket.connect(str(CODEX_IPC))
        self.client = 'initializing-client'
        self.client = self.request('initialize', {'clientType': 'taskq'}, 0)['result']['clientId']

    def put(self, value):
        import struct
        data = json.dumps(value).encode()
        self.socket.sendall(struct.pack('<I', len(data)) + data)

    def exact(self, size):
        data = b''
        while len(data) < size:
            data += self.socket.recv(size - len(data)) or core.fail('Codex app IPC closed the connection')
        return data

    def request(self, method, params, version, target=None):
        import struct
        import uuid
        request = str(uuid.uuid4())
        self.put({'type': 'request', 'requestId': request, 'sourceClientId': self.client, 'version': version,
                  'method': method, 'params': params, **({'targetClientId': target} if target else {})})
        while True:
            reply = json.loads(self.exact(struct.unpack('<I', self.exact(4))[0]))
            if reply.get('type') == 'client-discovery-request':
                # The router asks every client; answering at once makes "nobody owns it" a 0 s, not 10 s, error.
                self.put({'type': 'client-discovery-response', 'requestId': reply['requestId'],
                          'response': {'canHandle': False}})
            elif reply.get('type') == 'response' and reply.get('requestId') == request:
                return reply

    def broadcast(self, method, params, version):
        self.put({'type': 'broadcast', 'method': method, 'sourceClientId': self.client, 'params': params,
                  'version': version})
        time.sleep(0.5)


def codex_announce(thread, method='thread-unarchived', version=1):
    """The way 'taskq probe 1' (2026-10-06) got into the app's sidebar: one `thread-unarchived` broadcast.
    `thread-archived` (version 2 in the app's table) takes it out the same way. No app (headless, #160): no sidebar."""
    try:
        ipc = CodexIpc(timeout=10)
    except OSError:
        return
    try:
        ipc.broadcast(method, {'hostId': 'local', 'conversationId': thread}, version)
    finally:
        ipc.socket.close()


def codex_release(codex, thread):
    """Only an explicit unsubscribe lets the shared server unload a thread: 60 s after its turn ends it drops
    the writer lock (~/.codex/thread-writer-locks), and the app may take the thread (2026-10-06).
    A closed connection alone keeps it loaded, and the app shows 'This is open in another app'."""
    codex.call('thread/unsubscribe', {'threadId': thread})


def codex_send_app(codex, thread, metadata, text, full_access=False):
    """Deliver through the app when its window owns the thread, as a second app window would
    (`thread-follower-*`). None: the app does not own it. The app applies the request's policy."""
    try:
        ipc = CodexIpc()
    except OSError:
        return None  # the app is not running
    try:
        found = ipc.request('thread-owner-discovery', {'hostId': 'local', 'conversationId': thread}, 1)
        if found['resultType'] != 'success':
            return None
        owner, item = found['handledByClientId'], [{'type': 'text', 'text': text, 'text_elements': []}]
        turns = codex.call('thread/turns/list', {'threadId': thread, 'limit': 1, 'itemsView': 'notLoaded'})['data']
        if turns and turns[0]['status'] == 'inProgress':
            cwd = metadata.get('cwd') or str(core.ROOT)
            reply = ipc.request('thread-follower-steer-turn', {'conversationId': thread, 'input': item, 'restoreMessage':
                                {'text': text, 'cwd': cwd, 'context': {'workspaceRoots': [cwd]}}}, 1, owner)
            if reply['resultType'] == 'success':
                return 'steered the active turn in the Codex app'
        reply = ipc.request('thread-follower-start-turn', {'conversationId': thread, 'turnStart': {
            'request': {'threadId': thread, 'input': item, **codex_turn_policy(full_access)}, 'context': {}}}, 2, owner)
        if reply['resultType'] != 'success':
            core.fail(f'Codex app refused the message for {thread}: {reply.get("error")}')
        return 'new turn in the Codex app'
    finally:
        ipc.socket.close()


def codex_project(codex):
    """The app's project whose root is the main checkout, created when none is; `[codex] project` overrides it.
    Found anew on every spawn: an id is per machine, the checkout path is what every machine shares."""
    if project := core.codex_override('project'):
        return project
    root = os.path.realpath(core.ROOT)
    for project in codex.call('project/list', {}).get('data', []):
        if any(os.path.realpath(item['path']) == root for item in project.get('roots') or []):
            return project['id']
    import uuid
    created = codex.call('project/create', {'idempotencyKey': str(uuid.uuid4()), 'name': Path(root).name,
                                            'roots': [{'path': root}]})
    return created.get('project', created)['id']


def codex_spawn(name, prompt=None, full_access=False):
    """A persistent thread of the app's project (`codex_project`) in section `CODEX_SECTION`, announced to the app and
    released by the shared server, so the owner can write in it. It runs with `codex_access(full_access)`. Its first turn is `prompt`,
    left running as `codex-send` leaves it; without one, a finished 'ready' turn."""
    codex = Codex(timeout=300)
    thread = codex.call('thread/start', {'cwd': str(core.ROOT), 'projectId': codex_project(codex),
                                         'ephemeral': False, **codex_access(full_access)})['thread']['id']
    codex.call('thread/name/set', {'threadId': thread, 'name': name})
    if section := core.codex_override('section'):
        codex.call('thread/section/move', {'threadId': thread, 'sectionId': section})
    codex.call('turn/start', {'threadId': thread, **codex_turn_policy(full_access),
                            'input': [{'type': 'text', 'text': prompt or 'Reply with the single word: ready'}]})
    if not prompt:
        codex.wait_turn(thread, 280)
    codex_release(codex, thread)
    codex_announce(thread)
    return thread


def thread_full_access(thread, name):
    from taskq.tick import supervisor_iid, worker_iid
    if supervisor_iid(name):
        return True
    iid = worker_iid(name)
    return any(item['full_access'] and (item['iid'] == iid or (item['claim'] or {}).get('session') == thread)
               for item in core.load()[0])


def codex_send(args):
    """Pin every new turn's policy; deliver active input through steer without a new turn. A thread the
    shared server has not loaded may be the app's: then the app delivers it. Full access: `args.full_access`, else
    decided by the thread itself (#270), not the claim a reject clears: a supervisor (`S<N> `, it drives the
    app server, which workspace-write denies) or the `T<N> ` worker of a FULL_ACCESS task."""
    codex = Codex()
    metadata = codex.call('thread/read', {'threadId': args.thread})['thread']
    full = getattr(args, 'full_access', None) or thread_full_access(args.thread, metadata.get('name'))
    status = metadata['status']['type']
    route = codex_send_app(codex, args.thread, metadata, args.text, full) if status == 'notLoaded' else None
    if route:
        return print(f'delivered to {args.thread} ({route})')
    if status == 'active':
        turns = codex.call('thread/turns/list', {'threadId': args.thread, 'limit': 1, 'itemsView': 'notLoaded'})['data']
        if not turns or turns[0]['status'] != 'inProgress':
            core.fail(f'Codex thread {args.thread}: active turn id unavailable; no message sent')
        codex.call('turn/steer', {'threadId': args.thread, 'expectedTurnId': turns[0]['id'],
                                 'input': [{'type': 'text', 'text': args.text}]})
    else:
        try:
            # Also for a loaded idle thread: resume subscribes this connection, so the release below unloads it.
            codex.call('thread/resume', {'threadId': args.thread, **codex_access(full)})
        except SystemExit as error:
            if 'active writer' in str(error):
                core.fail(f'Codex thread {args.thread} is held by the Codex app, but no app window owns it; '
                     f'open it there (`open -g codex://threads/{args.thread}`) and send again')
            raise
        codex.call('turn/start', {'threadId': args.thread, **codex_turn_policy(full),
                                'input': [{'type': 'text', 'text': args.text}]})
        codex_release(codex, args.thread)
    print(f'delivered to {args.thread}' + (' (steered active turn)' if status == 'active' else ''))


# Bounded diagnostic output, independent of the number of items in a long worker turn.
CODEX_ITEM_LIMIT = 100


def codex_snapshot(codex, thread, limit=3, include_policy=False):
    """Read persisted item lifecycles, including in-progress commands, without resuming the worker."""
    metadata = codex.call('thread/read', {'threadId': thread})['thread']
    turns = codex.call('thread/turns/list', {'threadId': thread, 'limit': limit, 'itemsView': 'notLoaded'})['data']
    for turn in turns:
        page = codex.call('thread/items/list', {'threadId': thread, 'turnId': turn['id'],
                                               'limit': CODEX_ITEM_LIMIT, 'sortDirection': 'desc'})
        turn['entries'] = list(reversed(page['data']))
        turn['olderItems'] = bool(page.get('nextCursor'))
        turn['entries'].sort(key=lambda entry: entry.get('startedAtMs') or 0)
    # updatedAt is thread metadata recency, not the last item event. Never substitute it silently.
    stamps = [entry[key] / 1000 for turn in turns for entry in turn['entries']
              for key in ('startedAtMs', 'completedAtMs') if entry.get(key) is not None]
    stamps += [turn[key] for turn in turns for key in ('startedAt', 'completedAt') if turn.get(key) is not None]
    return metadata['status'], turns, max(stamps, default=None)


def codex_age(stamp):
    return f'{max(0, time.time() - stamp):.0f}s ago' if stamp is not None else 'unknown (no event timestamp)'


def codex_line(value):
    """One short line per event; never dump an entire command output or tool result."""
    text = ' '.join(str(value or '').split())
    return text[:300] + ('…' if len(text) > 300 else '')


def codex_item(item):
    kind = item['type']
    if kind == 'userMessage':
        return 'user: ' + codex_line(' '.join(part.get('text', f'[{part["type"]}]') for part in item['content']))
    if kind in ('agentMessage', 'plan'):
        return f'{kind}: ' + codex_line(item['text'])
    if kind == 'commandExecution':
        return (f'command {item["status"]} exit={item.get("exitCode")} '
                f'{codex_line(item["command"])} | {codex_line(item.get("aggregatedOutput"))}')
    if kind in ('mcpToolCall', 'dynamicToolCall'):
        return f'{kind} {item["status"]} {item.get("server", item.get("namespace", ""))}/{item["tool"]}'
    if kind == 'liveToolCall':
        return f'live tool {item["tool"]} inProgress: {codex_line(item["arguments"])}'
    return f'{kind}: {codex_line(item.get("status", item.get("query", item.get("path", ""))))}'


def codex_read(args):
    """Print recent turns and timestamped events, with the active operation and event age."""
    codex = Codex()
    status, turns, stamp = codex_snapshot(codex, args.thread, args.limit, include_policy=True)
    print(f'status: {status["type"]}' + (f' {status.get("activeFlags")}' if status['type'] == 'active' else '') +
          '')
    print(f'last event: {codex_age(stamp)}')
    for turn in reversed(turns):
        print(f'turn {turn["id"]}: {turn["status"]}')
        if turn['olderItems']:
            print(f'  older events omitted; showing last {CODEX_ITEM_LIMIT}')
        for entry in turn['entries']:
            when = entry.get('startedAtMs')
            stamp_text = time.strftime('%Y-%m-%d %H:%M:%S UTC', time.gmtime(when / 1000)) if when is not None else 'time unknown'
            print(f'  {stamp_text} {codex_item(entry["item"])}')
    if status['type'] == 'active':
        running = [entry['item'] for turn in turns if turn['status'] == 'inProgress'
                   for entry in turn['entries'] if entry['item'].get('status') == 'inProgress']
        print('now: ' + ('; '.join(codex_item(item) for item in running) if running else 'active; no running tool recorded'))


def codex_archived(metadata):
    return '/archived_sessions/' in (metadata.get('path') or '')


def codex_is_archived(thread):
    """#127: True only when this machine's app server reads the thread as archived; unknown (no socket, another
    machine's thread) is False, so the tick keeps listing it."""
    try:
        codex = Codex()
        try:
            return codex_archived(codex.call('thread/read', {'threadId': thread})['thread'])
        finally:
            codex.socket.close()
    except (OSError, SystemExit, ValueError):
        return False


def codex_archive(args):
    """Archive an idle Codex thread (reversible: `thread/unarchive`) and take it out of the app's sidebar."""
    codex = Codex()
    metadata = codex.call('thread/read', {'threadId': args.thread})['thread']
    if codex_archived(metadata):
        return print(f'already archived {args.thread}')
    turns = codex.call('thread/turns/list', {'threadId': args.thread, 'limit': 1, 'itemsView': 'notLoaded'})['data']
    if metadata['status']['type'] == 'active':
        core.fail(f'Codex thread {args.thread} is working; not archived')
    try:
        codex.call('thread/archive', {'threadId': args.thread})
    except SystemExit as error:
        if 'active writer' not in str(error):
            raise
        core.fail(f'Codex thread {args.thread} is held by the Codex app: archive it there')
    # Only after the archive: to an app still holding the thread, `thread-archived` drops it from the app's
    # inactive-thread unsubscriber without an unsubscribe, so it stays held until the app restarts (#165).
    codex_announce(args.thread, 'thread-archived', 2)
    print(f'archived {args.thread}')
