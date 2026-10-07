"""The Codex app server: worker threads, their live state, archive."""
import base64
from datetime import datetime
import json
import os
from pathlib import Path
import socket
import time

import taskq as core


# --- Codex app server (the desktop app's shared one); JSON-RPC over a WebSocket on a unix socket ---

CODEX_SOCKET = Path.home() / '.codex/app-server-control/app-server-control.sock'
CODEX_IPC = Path.home() / '.codex/ipc/ipc.sock'
# Design decision 2026-10-07 (#57 probe, #149): a Codex worker runs in workspace-write with network on
# and never asks; every step of a worker passed there. The worktree lives in ROOT/.worktrees, but git
# writes its refs, objects and worktree admin files into the main checkout's .git, so .git is a root.
# The taskq state dir holds the machine id and update stamp.
def codex_turn_policy():
    roots = [core.ROOT / '.git', core.ROOT / '.worktrees', core.UPDATE_STAMP.parent]
    return {'approvalPolicy': 'never', 'sandboxPolicy': {
        'type': 'workspaceWrite', 'networkAccess': True, 'writableRoots': [str(root) for root in roots]}}


# thread/start and thread/resume use the CLI spelling; turn/start carries the full policy.
CODEX_ACCESS = {'sandbox': 'workspace-write', 'approvalPolicy': 'never'}


class Codex:
    def __init__(self, timeout=60):
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
            if value.get('id') == self.counter and 'method' not in value:
                if 'error' in value:
                    core.fail(f'Codex {method}: {value["error"]}')
                return value['result']

    def wait_turn(self, thread, seconds):
        end = time.time() + seconds
        while time.time() < end:
            value = self.receive()
            if value.get('method') == 'turn/completed' and value['params'].get('threadId') == thread:
                return
        core.fail(f'Codex thread {thread}: no turn/completed in {seconds} s')


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
    `thread-archived` (version 2 in the app's table) takes it out the same way."""
    ipc = CodexIpc(timeout=10)
    try:
        ipc.broadcast(method, {'hostId': 'local', 'conversationId': thread}, version)
    finally:
        ipc.socket.close()


def codex_release(codex, thread):
    """Only an explicit unsubscribe lets the shared server unload a thread: 60 s after its turn ends it drops
    the writer lock (~/.codex/thread-writer-locks), and the app may take the thread (2026-10-06).
    A closed connection alone keeps it loaded, and the app shows 'This is open in another app'."""
    codex.call('thread/unsubscribe', {'threadId': thread})


def codex_send_app(codex, thread, metadata, text):
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
        if turns and codex_app_running(metadata, turns[0]):
            cwd = metadata.get('cwd') or str(core.ROOT)
            reply = ipc.request('thread-follower-steer-turn', {'conversationId': thread, 'input': item, 'restoreMessage':
                                {'text': text, 'cwd': cwd, 'context': {'workspaceRoots': [cwd]}}}, 1, owner)
            if reply['resultType'] == 'success':
                return 'steered the active turn in the Codex app'
        reply = ipc.request('thread-follower-start-turn', {'conversationId': thread, 'turnStart': {
            'request': {'threadId': thread, 'input': item, **codex_turn_policy()}, 'context': {}}}, 2, owner)
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


def codex_spawn(name, prompt=None):
    """A persistent thread of the app's project (`codex_project`) in section `CODEX_SECTION`, announced to the app and
    released by the shared server, so the owner can write in it. It runs with `CODEX_ACCESS`. Its first turn is `prompt`,
    left running as `codex-send` leaves it; without one, a finished 'ready' turn."""
    codex = Codex(timeout=300)
    thread = codex.call('thread/start', {'cwd': str(core.ROOT), 'projectId': codex_project(codex),
                                         'ephemeral': False, **CODEX_ACCESS})['thread']['id']
    codex.call('thread/name/set', {'threadId': thread, 'name': name})
    if section := core.codex_override('section'):
        codex.call('thread/section/move', {'threadId': thread, 'sectionId': section})
    codex.call('turn/start', {'threadId': thread, **codex_turn_policy(),
                            'input': [{'type': 'text', 'text': prompt or 'Reply with the single word: ready'}]})
    if not prompt:
        codex.wait_turn(thread, 280)
    codex_release(codex, thread)
    codex_announce(thread)
    return thread


def codex_send(args):
    """Pin every new turn's policy; deliver active input through steer without a new turn. A thread the
    shared server has not loaded may be the app's: then the app delivers it."""
    codex = Codex()
    metadata = codex.call('thread/read', {'threadId': args.thread})['thread']
    status = metadata['status']['type']
    route = codex_send_app(codex, args.thread, metadata, args.text) if status == 'notLoaded' else None
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
            codex.call('thread/resume', {'threadId': args.thread, **CODEX_ACCESS})
        except SystemExit as error:
            if 'active writer' in str(error):
                core.fail(f'Codex thread {args.thread} is held by the Codex app, but no app window owns it; '
                     f'open it there (`open -g codex://threads/{args.thread}`) and send again')
            raise
        codex.call('turn/start', {'threadId': args.thread, **codex_turn_policy(),
                                'input': [{'type': 'text', 'text': args.text}]})
        codex_release(codex, args.thread)
    print(f'delivered to {args.thread}' + (' (steered active turn)' if status == 'active' else ''))


# Bounded diagnostic output, independent of the number of items in a long worker turn.
CODEX_ITEM_LIMIT = 100
CODEX_TAIL_BYTES = 256 * 1024  # bounded live diagnostic, not a second session log


def codex_live_entries(path, turn):
    """The paginated store omits running exec commands. Read only this thread's bounded rollout tail."""
    from datetime import datetime
    if not path:
        return [], None
    try:
        with open(path, 'rb') as handle:
            handle.seek(0, 2)
            offset = max(0, handle.tell() - CODEX_TAIL_BYTES)
            handle.seek(offset)
            lines = handle.read(CODEX_TAIL_BYTES).splitlines()
        if offset:
            lines = lines[1:]  # possibly partial first record
    except OSError:
        return [], None
    calls, processes, last = {}, {}, None
    recorded = {str(entry['item'].get('processId')) for entry in turn['entries']
                if entry['item'].get('type') == 'commandExecution'}
    for line in lines:
        try:
            record = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            continue  # partial last write is normal while the thread works
        payload = record.get('payload', {})
        if record.get('type') != 'response_item':
            continue
        meta = payload.get('internal_chat_message_metadata_passthrough') or {}
        if meta.get('turn_id') != turn['id']:
            continue
        kind, call = payload.get('type'), payload.get('call_id')
        if kind not in ('custom_tool_call', 'function_call', 'custom_tool_call_output', 'function_call_output'):
            continue
        stamp = datetime.fromisoformat(record['timestamp'].replace('Z', '+00:00')).timestamp()
        last = max(last or stamp, stamp)
        if kind in ('custom_tool_call', 'function_call'):
            calls[call] = {'turnId': turn['id'], 'startedAtMs': stamp * 1000,
                           'item': {'type': 'liveToolCall', 'id': call, 'status': 'inProgress',
                                    'tool': payload['name'], 'arguments': payload.get('input', payload.get('arguments', ''))}}
        else:
            entry = calls.pop(call, None)
            output = payload.get('output')
            blocks = output if isinstance(output, list) else [{'text': output}]
            for block in blocks:
                try:
                    result = json.loads(block.get('text', ''))
                except (ValueError, TypeError):
                    continue
                if isinstance(result, dict) and result.get('session_id') is not None and entry is not None:
                    sid = str(result['session_id'])
                    if sid not in processes and 'exec_command' in entry['item']['arguments']:
                        processes[sid] = {**entry, 'item': {'type': 'commandExecution', 'status': 'inProgress',
                                                          'command': entry['item']['arguments'], 'processId': sid}}
            # A commandExecution completion in the API is authoritative; tool output alone may be a polling result.
    return [entry for sid, entry in processes.items() if sid not in recorded] + list(calls.values()), last


def recorded_turn_policy(path, turn_id):
    """Report this turn's recorded policy, never its thread defaults or our desired policy."""
    policy = None
    if not path:
        return policy
    try:
        with open(path, 'rb') as handle:
            # ponytail: bounded-memory history scan; add a reverse seek if large rollouts make this costly.
            while line := handle.readline(CODEX_TAIL_BYTES):
                if b'"turn_context"' not in line:
                    continue
                try:
                    record = json.loads(line)
                except (ValueError, UnicodeDecodeError):
                    continue
                payload = record.get('payload', {})
                if record.get('type') == 'turn_context' and payload.get('turn_id') == turn_id:
                    policy = {'sandbox': payload.get('sandbox_policy'), 'approval': payload.get('approval_policy')}
    except OSError:
        pass
    return policy


def codex_app_running(metadata, turn):
    """The shared server reads a turn the app is running from its rollout and calls it `interrupted`, since
    the turn has no end yet. The end (`task_complete` or `turn_aborted`) is always the turn's last record."""
    if metadata['status']['type'] != 'notLoaded' or turn['status'] not in ('interrupted', 'inProgress'):
        return False
    try:
        with open(metadata.get('path') or '', 'rb') as handle:
            handle.seek(max(0, handle.seek(0, 2) - CODEX_TAIL_BYTES))
            tail = handle.read(CODEX_TAIL_BYTES)
    except OSError:
        return False
    return not any(f'"type":"{end}","turn_id":"{turn["id"]}"'.encode() in tail for end in ('task_complete', 'turn_aborted'))


def codex_snapshot(codex, thread, limit=3, include_policy=False):
    """Read persisted item lifecycles, including in-progress commands, without resuming the worker."""
    metadata = codex.call('thread/read', {'threadId': thread})['thread']
    turns = codex.call('thread/turns/list', {'threadId': thread, 'limit': limit, 'itemsView': 'notLoaded'})['data']
    if turns and codex_app_running(metadata, turns[0]):
        turns[0].update(status='inProgress', app=True)
    for turn in turns:
        page = codex.call('thread/items/list', {'threadId': thread, 'turnId': turn['id'],
                                               'limit': CODEX_ITEM_LIMIT, 'sortDirection': 'desc'})
        turn['entries'] = list(reversed(page['data']))
        turn['olderItems'] = bool(page.get('nextCursor'))
        if turn['status'] == 'inProgress':
            live, last = codex_live_entries(metadata.get('path'), turn)
            turn['entries'] += live
            turn['liveStamp'] = last
        turn['entries'].sort(key=lambda entry: entry.get('startedAtMs') or 0)
    # updatedAt is thread metadata recency, not the last item event. Never substitute it silently.
    stamps = [entry[key] / 1000 for turn in turns for entry in turn['entries']
              for key in ('startedAtMs', 'completedAtMs') if entry.get(key) is not None]
    stamps += [turn[key] for turn in turns for key in ('startedAt', 'completedAt') if turn.get(key) is not None]
    stamps += [turn['liveStamp'] for turn in turns if turn.get('liveStamp') is not None]
    if include_policy and turns:
        turns[0]['policy'] = recorded_turn_policy(metadata.get('path'), turns[0]['id'])
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
    held = turns and turns[0].get('app')
    print(f'status: {status["type"]}' + (f' {status.get("activeFlags")}' if status['type'] == 'active' else '') +
          (' (turn running in the Codex app, which holds the session)' if held else ''))
    print(f'last event: {codex_age(stamp)}')
    policy = (turns[0].get('policy') if turns else None) or {}
    sandbox = policy.get('sandbox') or {}
    shown = {key: sandbox[key] for key in ('type', 'network_access', 'writable_roots') if key in sandbox}
    print('last turn sandbox: ' + (json.dumps(shown, ensure_ascii=False) if shown else 'unknown (no turn_context)') +
          f'; approvalPolicy: {policy.get("approval") or "unknown"}')
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
    if metadata['status']['type'] == 'active' or (turns and codex_app_running(metadata, turns[0])):
        core.fail(f'Codex thread {args.thread} is working; not archived')
    try:
        codex.call('thread/archive', {'threadId': args.thread})
    except SystemExit as error:
        # 2026-10-06: a thread the app has opened stays loaded in the app's private app server (stdio, not
        # reachable) for 3 h after it leaves view, or until more than 10 such threads; no IPC request archives
        # or releases it (`thread-archived` only hides the row), and osascript has no assistive access. The
        # app's own context menu archives it; an agent does that with computer-use, not the owner.
        if 'active writer' in str(error):
            core.fail(f'Codex thread {args.thread} is held open by the Codex app. Archive it there with computer-use '
                 f'(com.openai.codex, full-screen control): `open -g codex://threads/{args.thread}`, activate the app, '
                 f'click the chat body and press Cmd+Shift+A (Archive chat); then run codex-archive again to confirm and '
                 f'open the session the window showed before the same way (the link switches the window)')
        raise
    codex_announce(args.thread, 'thread-archived', 2)
    print(f'archived {args.thread}')
