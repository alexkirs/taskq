"""Linux project-local Hermes TUI JSON-RPC over stdio; no board authority.

Supply isolated HERMES_HOME and TASKQ_HERMES_COMMAND (JSON argv for the
installed Python's -m tui_gateway.entry). Files in .taskq are transport handles
and evidence only. Owners never restart or attach to a different gateway.
"""
import contextlib
import hashlib
import json
import os
from pathlib import Path
import queue
import signal
import subprocess
import sys
import threading
import time
import uuid

WAIT = 60


def available():
    home = os.environ.get('HERMES_HOME')
    command = json.loads(os.environ.get('TASKQ_HERMES_COMMAND', '[]'))
    return bool(sys.platform == 'linux' and hasattr(os, 'pidfd_open') and
                hasattr(signal, 'pidfd_send_signal') and home and Path(home).is_dir() and
                isinstance(command, list) and command and all(isinstance(x, str) and x for x in command))


def host():
    for module in list(sys.modules.values()):
        if getattr(module, '__file__', None) and Path(module.__file__).name == 'taskq.py':
            return module
    raise ValueError('TaskQ project configuration is not loaded')


def folder():
    return Path(host().CONFIG['root']) / '.taskq'


def handle(sid):
    return folder() / ('h-' + hashlib.sha256(sid.encode()).hexdigest()[:12] + '.handle.json')


def atomic(path, value):
    temporary = path.with_name(path.name + '.' + uuid.uuid4().hex + '.tmp')
    try:
        with open(temporary, 'x') as stream:
            os.chmod(temporary, 0o600)
            json.dump(value, stream)
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def birth(pid):
    """Linux starttime, including zombies as dead; a PID alone is never an owner."""
    try:
        fields = Path(f'/proc/{pid}/stat').read_text().rpartition(')')[2].split()
        return fields[19] if fields[0] not in ('Z', 'X') else None
    except (FileNotFoundError, ProcessLookupError):
        return None


def owner_fd(owner):
    """Pin the process before checking its birth, so later signals cannot hit a reused PID."""
    try:
        fd = os.pidfd_open(owner['pid'])
    except ProcessLookupError:
        raise ConnectionRefusedError('Hermes owner is gone')
    if (owner.get('boot') != Path('/proc/sys/kernel/random/boot_id').read_text().strip()
            or birth(owner['pid']) != owner['birth']):
        os.close(fd)
        raise ConnectionRefusedError('Hermes owner identity changed')
    return fd


def request(path, method, params=None, expected=None):
    path = Path(path)
    owner = json.loads((path / 'owner.json').read_text())
    if expected is not None and owner != expected:
        raise ConnectionRefusedError('Hermes endpoint ownership changed')
    fd = owner_fd(owner)
    token = uuid.uuid4().hex
    pending, response = path / (token + '.req'), path / (token + '.reply')
    try:
        atomic(pending, {'owner': owner, 'method': method, 'params': params or {}})
        deadline = time.monotonic() + WAIT
        while not response.exists():
            if birth(owner['pid']) != owner['birth']:
                raise ConnectionRefusedError('Hermes owner exited')
            if time.monotonic() >= deadline:
                raise TimeoutError('Hermes RPC deadline exceeded; delivery unknown')
            time.sleep(.02)
        reply = json.loads(response.read_text())
        if 'error' in reply:
            raise ValueError('Hermes RPC failed: ' + str(reply['error']))
        return reply['result']
    finally:
        pending.unlink(missing_ok=True)
        response.unlink(missing_ok=True)
        os.close(fd)


def data_for(sid):
    data = json.loads(handle(sid).read_text())
    if data['session'] != sid:
        raise ValueError('Hermes handle stored key mismatch')
    return data


def call(sid, method, **params):
    data = data_for(sid)
    return request(data['endpoint'], method, {'session_id': data['runtime_id'], **params}, data['owner'])


def start(endpoint, cwd):
    env = dict(os.environ)
    for key in ('HERMES_SESSION_ID', 'CODEX_THREAD_ID', 'CLAUDE_CODE_SESSION_ID'):
        env.pop(key, None)
    with open(endpoint.with_suffix('.log'), 'ab') as log:
        os.chmod(endpoint.with_suffix('.log'), 0o600)
        return subprocess.Popen([sys.executable, str(Path(__file__).resolve()), str(endpoint), str(cwd)],
                                cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                start_new_session=True)


def wait_owner(endpoint, child):
    deadline = time.monotonic() + WAIT
    while not (endpoint / 'owner.json').exists():
        if child.poll() is not None or time.monotonic() >= deadline:
            raise ValueError('Hermes owner startup failed; see ' + str(endpoint.with_suffix('.log')))
        time.sleep(.02)
    owner = json.loads((endpoint / 'owner.json').read_text())
    if owner['pid'] != child.pid:
        raise ValueError('Hermes startup owner PID mismatch')
    fd = owner_fd(owner)
    os.close(fd)
    return owner


def reap(child):
    """Direct child identity remains pinned until wait; SIGTERM runs the owner's finally block."""
    if child.poll() is None:
        child.terminate()
    try:
        child.wait(timeout=8)
    except subprocess.TimeoutExpired:
        # Gateway has its own parent-death signal; no signalling an unverified PID.
        child.kill()
        child.wait(timeout=5)


def spawn(name, prompt, cwd):
    if not available():
        raise ValueError('Hermes needs Linux pidfds, explicit isolated HERMES_HOME and TASKQ_HERMES_COMMAND')
    folder().mkdir(exist_ok=True)
    endpoint = folder() / ('h-' + uuid.uuid4().hex + '.ipc')
    child, sid, runtime, owner = start(endpoint, cwd), None, None, None
    try:
        owner = wait_owner(endpoint, child)
        created = request(endpoint, 'session.create', {'cwd': str(cwd)}, owner)
        sid, runtime = created.get('stored_session_id'), created.get('session_id')
        if not all(isinstance(x, str) and x.strip() for x in (sid, runtime)):
            raise ValueError('Hermes returned no genuine stored/runtime IDs')
        atomic(handle(sid), {'endpoint': str(endpoint), 'owner': owner, 'runtime_id': runtime,
                             'session': sid, 'name': name})
        try:
            titled = call(sid, 'session.title', title=name)
            read = call(sid, 'session.title')
            # methods_session.session.title persists the draft iff pending is false.
            if titled.get('pending') is not False or (read.get('session_key'), read.get('title')) != (sid, name):
                raise ValueError('Hermes stored row/native title persistence not confirmed')
        except (OSError, ValueError) as error:
            raise host().Unnamed(sid, error)
        # Never restart: native context binds the actual stored ID for subprocess tools.
        send(sid, prompt)
        threading.Thread(target=child.wait, daemon=True).start()
        return sid
    except BaseException:
        if runtime and owner:
            with contextlib.suppress(Exception):
                request(endpoint, 'session.close', {'session_id': runtime}, owner)
        reap(child)
        raise


def snapshot_state(snapshot, sid, runtime):
    if (snapshot.get('session_id'), snapshot.get('session_key')) != (runtime, sid):
        return None
    if snapshot.get('hydrating') or snapshot.get('queued') or snapshot.get('auto_continue'):
        return None
    running = snapshot.get('running')
    return ('running' if running else 'idle') if type(running) is bool else None


def state(sid):
    try:
        data = data_for(sid)
        return snapshot_state(call(sid, 'session.activate', omit_messages=True), sid, data['runtime_id'])
    except (FileNotFoundError, ConnectionRefusedError):
        return 'dead'
    except (OSError, ValueError, KeyError, TypeError):
        return None


def owned(sid):
    """Local handle existence only; a stale/invalid owned handle must fail visibly, not fall back."""
    return handle(sid).is_file()


def send(sid, text):
    data = data_for(sid)
    if state(sid) != 'idle':
        raise ValueError('Hermes session is not confirmed idle')
    resumed = request(data['endpoint'], 'session.resume', {'session_id': sid, 'omit_messages': True}, data['owner'])
    if snapshot_state(resumed, sid, data['runtime_id']) != 'idle':
        raise ValueError('Hermes resume changed ownership or is not idle')
    admission = call(sid, '_submit', stored_id=sid, text=text)
    if admission['reply'].get('status') != 'streaming':
        raise ValueError('Hermes did not admit a streaming turn')
    data['turn'] = admission['turn']  # local request correlation, never a Hermes ID or board event receipt
    atomic(handle(sid), data)
    return sid


def history_watermark(history):
    """History is a display projection: tool/hidden rows need not carry row IDs.
    Idle persisted user/assistant rows do; DB IDs are AUTOINCREMENT (Hermes source).
    Empty history is valid only when the native raw count is explicitly zero.
    """
    rows = history.get('messages')
    if not isinstance(rows, list) or type(history.get('count')) is not int:
        raise ValueError('Hermes persisted history boundary unavailable')
    ids = [row.get('row_id') for row in rows if row.get('role') in ('user', 'assistant')]
    if (not ids and history['count'] != 0) or any(type(i) is not int or i <= 0 for i in ids):
        raise ValueError('Hermes persisted history boundary unavailable')
    if ids != sorted(set(ids)):
        raise ValueError('Hermes persisted history boundary is unordered')
    return max(ids, default=0)


def fresh_receipt(payload, boundary):
    receipt = payload.get('persisted_turn') or {}
    ids = receipt.get('row_ids')
    return (isinstance(ids, list) and len(ids) >= 2
            and all(type(i) is int and i > boundary for i in ids)
            and ids == sorted(set(ids))
            and receipt.get('user_row_id') == ids[0]
            and receipt.get('final_assistant_row_id') == ids[-1])


def completed(sid, timeout=None):
    """Prove this submitted turn by terminal event AND committed user/final-assistant row identities."""
    data = data_for(sid)
    deadline = time.monotonic() + (WAIT if timeout is None else timeout)
    while time.monotonic() < deadline:
        turn = call(sid, '_turn', turn=data['turn'])
        if turn.get('complete') is not None:
            payload = turn['complete']
            receipt = payload.get('persisted_turn') or {}
            if (turn.get('started') is not True or payload.get('status') != 'complete'
                    or payload.get('error') or receipt.get('complete') is not True
                    or not fresh_receipt(payload, turn['boundary'])):
                raise ValueError('Hermes turn has no successful persisted message.complete proof')
            rows = {row.get('row_id'): row for row in call(sid, 'session.history').get('messages', [])}
            user = rows.get(receipt.get('user_row_id'), {})
            assistant = rows.get(receipt.get('final_assistant_row_id'), {})
            if (user.get('role') != 'user' or user.get('text') != turn['prompt']
                    or assistant.get('role') != 'assistant' or assistant.get('text') != payload.get('text')
                    or not payload.get('text')):
                raise ValueError('Hermes persisted turn rows do not prove this prompt/model response')
            if state(sid) == 'idle':
                return payload
        if turn.get('error'):
            raise ValueError('Hermes submitted turn failed; see retained event evidence')
        time.sleep(.05)
    raise TimeoutError('Hermes message.complete/idle proof deadline exceeded')


def wake_manager(sid, text):
    send(sid, text)
    completed(sid)
    return sid


def alive(sid):
    value = state(sid)
    # Idle is live for worker liveness; supervisor state distinguishes it from running.
    return None if value is None else value != 'dead'


def link(sid):
    return None


def stop(sid):
    """Only the exact owner may stop; retained logs prove shutdown. No restart/recovery."""
    data = data_for(sid)
    if state(sid) is None:
        raise ValueError('Hermes retirement state unknown')
    if state(sid) != 'dead':
        if call(sid, 'session.close').get('closed') is not True:
            raise ValueError('Hermes did not confirm close')
        request(data['endpoint'], '_stop', expected=data['owner'])
        deadline = time.monotonic() + 8
        while birth(data['owner']['pid']) == data['owner']['birth']:
            if time.monotonic() >= deadline:
                raise TimeoutError('Hermes owner did not terminate after close')
            time.sleep(.02)
    handle(sid).unlink()


def retire(gone, running=True):
    import re
    for path in folder().glob('h-*.handle.json'):
        data = json.loads(path.read_text())
        match = re.match(r'[TS](\d+)\b', data['name'])
        if not match:
            continue
        sid, status = data['session'], state(data['session'])
        if status is None:
            raise ValueError('Hermes retirement state unknown')
        active = status == 'running'
        if gone(int(match[1]), sid, active) and (running or not active):
            stop(sid)


def parent_death(parent):
    """Gateway dies if its owner is killed even before Python can run finally."""
    import ctypes
    if ctypes.CDLL(None, use_errno=True).prctl(1, signal.SIGKILL, 0, 0, 0) != 0:
        os._exit(126)
    if os.getppid() != parent:
        os._exit(126)


def reap_adopted():
    """Owner is a Linux subreaper: kill/reap only direct adopted gateway children, even
    if a tool made its own process group. Pin each child before testing its parent."""
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        children = Path(f'/proc/{os.getpid()}/task/{os.getpid()}/children').read_text().split()
        for child in children:
            try:
                fd = os.pidfd_open(int(child))
            except ProcessLookupError:
                continue
            try:
                fields = Path(f'/proc/{child}/stat').read_text().rpartition(')')[2].split()
                if fields[1] == str(os.getpid()):
                    with contextlib.suppress(ProcessLookupError):
                        signal.pidfd_send_signal(fd, signal.SIGKILL)
            except FileNotFoundError:
                pass
            finally:
                os.close(fd)
        try:
            while os.waitpid(-1, os.WNOHANG)[0]:
                pass
        except ChildProcessError:
            return
        time.sleep(.02)
    raise TimeoutError('Hermes adopted gateway children did not terminate')


def serve(endpoint, cwd):
    """A single gateway owner, serial RPC submission, runtime event evidence; no task authority."""
    os.umask(0o077)
    endpoint = Path(endpoint)
    owner = {'pid': os.getpid(), 'birth': birth(os.getpid()), 'nonce': uuid.uuid4().hex,
             'boot': Path('/proc/sys/kernel/random/boot_id').read_text().strip()}
    process, replies = None, queue.Queue()
    write_lock, turn_lock = threading.Lock(), threading.Lock()
    turns, active, counter = {}, [None], [0]
    evidence = endpoint.with_suffix('.events.jsonl')

    def interrupted(signum, frame):
        raise SystemExit(128 + signum)

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)

    def write(frame):
        with write_lock:
            process.stdin.write(json.dumps({'jsonrpc': '2.0', **frame}) + '\n')
            process.stdin.flush()

    def rpc(method, params):
        counter[0] += 1
        serial = counter[0]
        write({'id': serial, 'method': method, 'params': params})
        deadline = time.monotonic() + WAIT
        while True:
            reply = replies.get(timeout=max(.01, deadline - time.monotonic()))
            if reply is None:
                raise ValueError('Hermes gateway exited')
            if reply.get('id') == serial:
                if 'error' in reply:
                    raise ValueError('Hermes ' + method + ' refused (code ' + str(reply['error'].get('code')) + ')')
                return reply['result']
            if time.monotonic() >= deadline:
                raise TimeoutError('Hermes RPC reply deadline exceeded')

    def read():
        try:
            for line in process.stdout:
                try:
                    message = json.loads(line)
                except ValueError:
                    continue
                if 'id' in message and 'method' not in message:
                    replies.put(message)
                elif 'id' in message:
                    write({'id': message['id'], 'error': {'code': -32601, 'message': 'TaskQ has no interactive approval host'}})
                elif message.get('method') == 'event':
                    event = message.get('params', {})
                    if event.get('type') not in ('message.start', 'message.complete', 'error', 'tool.start', 'tool.complete'):
                        continue
                    with turn_lock:
                        current = turns.get(active[0])
                        if current and event.get('session_id') == current['runtime']:
                            if event['type'] == 'message.start':
                                current['started'] = True
                            elif event['type'] == 'message.complete' and current['started']:
                                payload = event.get('payload', {})
                                if fresh_receipt(payload, current['boundary']):
                                    current['complete'] = payload
                            elif event['type'] in ('tool.start', 'tool.complete') and current['started'] and current['complete'] is None:
                                current['tools'].append(event)
                            elif event['type'] == 'error':
                                current['error'] = True
                            with open(evidence, 'a') as out:
                                out.write(json.dumps({'submit': active[0], 'event': event}) + '\n')
        finally:
            replies.put(None)

    import ctypes
    if ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) != 0:
        raise ValueError('Hermes owner cannot reap its gateway children')
    parent_pid = os.getpid()
    try:
        process = subprocess.Popen(json.loads(os.environ['TASKQ_HERMES_COMMAND']), cwd=cwd,
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
                                   start_new_session=True, preexec_fn=lambda: parent_death(parent_pid))
        owner['gateway'] = {'pid': process.pid, 'birth': birth(process.pid)}
        threading.Thread(target=read, daemon=True).start()
        endpoint.mkdir(mode=0o700)
        atomic(endpoint / 'owner.json', owner)
        stopping = False
        while birth(process.pid) is not None and not stopping:
            for pending in endpoint.glob('*.req'):
                response = pending.with_suffix('.reply')
                try:
                    req = json.loads(pending.read_text())
                    pending.unlink()
                    if req.get('owner') != owner:
                        raise ValueError('Hermes IPC owner token mismatch')
                    method, params = req['method'], req['params']
                    if method == '_stop':
                        result, stopping = True, True
                    elif method == '_turn':
                        with turn_lock:
                            result = dict(turns[params['turn']])
                    elif method == '_submit':
                        runtime, sid = params['session_id'], params['stored_id']
                        snapshot = rpc('session.activate', {'session_id': runtime, 'omit_messages': True})
                        if snapshot_state(snapshot, sid, runtime) != 'idle':
                            raise ValueError('Hermes submit refuses busy/unknown ownership')
                        boundary = history_watermark(rpc('session.history', {'session_id': runtime}))
                        token = uuid.uuid4().hex  # LOCAL submit correlation, not a native ID
                        with turn_lock:
                            if active[0] and turns[active[0]]['complete'] is None:
                                raise ValueError('Hermes previous turn has no terminal event')
                            turns.clear()  # one session, one current native turn; evidence stays in JSONL
                            turns[token] = {'runtime': runtime, 'prompt': params['text'], 'started': False, 'complete': None, 'boundary': boundary, 'tools': []}
                            active[0] = token
                        reply = rpc('prompt.submit', {'session_id': runtime, 'text': params['text']})
                        if reply.get('status') != 'streaming':
                            raise ValueError('Hermes prompt was queued/steered/refused, not admitted')
                        result = {'reply': reply, 'turn': token}
                    else:
                        result = rpc(method, params)
                    atomic(response, {'result': result})
                except (Exception, queue.Empty) as error:
                    # Never print provider/config exception bodies or request text.
                    atomic(response, {'error': str(error) if isinstance(error, (ValueError, TimeoutError)) else type(error).__name__})
                if stopping:
                    deadline = time.monotonic() + 2
                    while response.exists() and time.monotonic() < deadline:
                        time.sleep(.02)
                    break
            time.sleep(.02)
    finally:
        # We have not reaped the gateway yet: its PID/group cannot be reused. Kill the
        # owned group (including same-group children) before wait, even if leader is a zombie.
        if process is not None:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGKILL)
            code = process.wait(timeout=5)
            atomic(endpoint.with_suffix('.exit.json'), {'owner': owner, 'gateway_returncode': code})
            reap_adopted()
            process.stdin.close()
            process.stdout.close()
        # Keep logs/event evidence; remove only this owner instance's transient IPC.
        if (endpoint / 'owner.json').is_file() and json.loads((endpoint / 'owner.json').read_text()) == owner:
            import shutil
            shutil.rmtree(endpoint)


if __name__ == '__main__':
    serve(sys.argv[1], sys.argv[2])
