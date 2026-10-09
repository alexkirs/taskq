"""Project-local POSIX Hermes TUI stdio bridge. No board state or credentials.

Configure runtimes.hermes = runtimes/hermes.py, TASKQ_HERMES_COMMAND as a JSON
argv (e.g. ["/path/to/python", "-m", "tui_gateway.entry"]), and an explicitly
isolated HERMES_HOME. The interpreter must have Hermes installed/importable.
"""
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time

WAIT = 60


def available():
    home = os.environ.get('HERMES_HOME')
    command = json.loads(os.environ.get('TASKQ_HERMES_COMMAND', '[]'))
    return bool(os.name == 'posix' and home and Path(home).is_dir()
                and isinstance(command, list) and command and all(isinstance(x, str) for x in command))


def folder():
    for module in list(sys.modules.values()):
        if getattr(module, '__file__', None) and Path(module.__file__).name == 'taskq.py':
            return Path(module.CONFIG['root']) / '.taskq'
    raise ValueError('TaskQ project configuration is not loaded')


def handle(sid):
    return folder() / ('h-' + hashlib.sha256(sid.encode()).hexdigest()[:12] + '.json')


def request(path, method, params=None):
    import uuid
    path = Path(path)
    if not path.is_dir():
        raise ConnectionRefusedError('Hermes owner is gone')
    owner = json.loads((path / 'owner.json').read_text())
    try:
        os.kill(owner['pid'], 0)
    except ProcessLookupError:
        raise ConnectionRefusedError('Hermes owner is gone')
    token = uuid.uuid4().hex
    pending, response = path / (token + '.req'), path / (token + '.reply')
    temporary = path / (token + '.tmp')
    temporary.write_text(json.dumps({'method': method, 'params': params or {}}))
    temporary.rename(pending)
    deadline = time.monotonic() + WAIT
    while not response.exists():
        if not path.exists():
            raise ConnectionRefusedError('Hermes owner is gone')
        if time.monotonic() >= deadline:
            pending.unlink(missing_ok=True)
            raise TimeoutError('Hermes RPC deadline exceeded')
        time.sleep(.02)
    reply = json.loads(response.read_text())
    response.unlink()
    if 'error' in reply:
        raise ValueError(reply['error'])
    return reply['result']


def call(sid, method, **params):
    data = json.loads(handle(sid).read_text())
    return request(data['endpoint'], method, {'session_id': data['runtime_id'], **params})


def start(endpoint, cwd, session=None):
    """Start one stdio gateway. A resumed owner receives only its own stored ID in child tools."""
    env = dict(os.environ)
    for key in ('HERMES_SESSION_ID', 'CODEX_THREAD_ID', 'CLAUDE_CODE_SESSION_ID'):
        env.pop(key, None)
    if session:
        env['HERMES_SESSION_ID'] = session
    with open(endpoint.with_suffix('.log'), 'ab') as log:
        return subprocess.Popen([sys.executable, str(Path(__file__).resolve()), str(endpoint), str(cwd)],
                                cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                start_new_session=True)


def wait_owner(endpoint, child):
    deadline = time.monotonic() + WAIT
    while not (endpoint / 'owner.json').exists():
        if child.poll() is not None or time.monotonic() >= deadline:
            log = endpoint.with_suffix('.log')
            detail = log.read_text('utf-8', errors='replace') if log.exists() else 'no bridge log'
            raise ValueError('Hermes transport failed to start: ' + detail[-4000:])
        time.sleep(.05)


def spawn(name, prompt, cwd):
    if not available():
        raise ValueError('Hermes needs explicit isolated HERMES_HOME and TASKQ_HERMES_COMMAND')
    folder().mkdir(exist_ok=True)
    old_mask = os.umask(0o077)
    import uuid
    endpoint = folder() / ('h-' + uuid.uuid4().hex[:8] + '.ipc')
    try:
        child = start(endpoint, cwd)
    finally:
        os.umask(old_mask)
    try:
        wait_owner(endpoint, child)
        created = request(endpoint, 'session.create', {'cwd': str(cwd)})
        sid, runtime = created['stored_session_id'], created['session_id']
        if not isinstance(sid, str) or not sid.strip():
            raise ValueError('Hermes returned no stored session ID')
        request(endpoint, '_stop')
        child.wait(timeout=WAIT)
        endpoint = folder() / ('h-' + uuid.uuid4().hex[:8] + '.ipc')
        child = start(endpoint, cwd, sid)
        wait_owner(endpoint, child)
        resumed = request(endpoint, 'session.resume', {'session_id': sid})
        runtime = resumed.get('session_id')
        if not isinstance(runtime, str) or not runtime:
            raise ValueError('Hermes did not resume the stored session')
        handle(sid).write_text(json.dumps({'endpoint': str(endpoint), 'runtime_id': runtime, 'session': sid, 'name': name}))
        handle(sid).chmod(0o600)
        try:
            call(sid, 'session.title', title=name)
            if call(sid, 'session.title').get('title') != name:
                raise ValueError('Hermes native title not confirmed')
        except (OSError, ValueError) as error:
            for module in list(sys.modules.values()):
                if getattr(module, '__file__', None) and Path(module.__file__).name == 'taskq.py':
                    raise module.Unnamed(sid, error)
            raise
        send(sid, prompt)
        threading.Thread(target=child.wait, daemon=True).start()
        return sid
    except BaseException:
        child.terminate()
        child.wait(timeout=WAIT)
        raise


def send(sid, text):
    if state(sid) != 'idle':
        raise ValueError('Hermes session is not idle; refusing queued/steered delivery')
    data = json.loads(handle(sid).read_text())
    resumed = request(data['endpoint'], 'session.resume', {'session_id': sid})
    if resumed.get('session_id') != data['runtime_id']:
        raise ValueError('Hermes resume changed live ownership')
    result = call(sid, 'prompt.submit', text=text)
    if result.get('status') != 'streaming':
        raise ValueError('Hermes did not admit the turn: ' + repr(result))
    return sid


def state(sid):
    try:
        output = call(sid, 'session.status').get('output', '')
    except (FileNotFoundError, ConnectionRefusedError):
        return 'dead'
    except (OSError, ValueError):
        return None
    lines = output.splitlines()
    if 'Agent Running: Yes' in lines:
        return 'running'
    if 'Agent Running: No' in lines:
        return 'idle'
    return None


def alive(sid):
    value = state(sid)
    return None if value is None else value == 'running'


def link(sid):
    return None


def wake_manager(sid, text):
    """Only a manager owned by this transport can receive a native idle turn."""
    return send(sid, text)


def retire(gone, running=True):
    import re
    for path in folder().glob('h-*.json'):
        data = json.loads(path.read_text())
        match = re.match(r'[TS](\d+)\b', data['name'])
        if not match:
            continue
        sid = data['session']
        active = state(sid) == 'running'
        if gone(int(match[1]), sid, active) and (running or not active):
            if state(sid) == 'dead':
                path.unlink()
                continue
            result = call(sid, 'session.close')
            if result.get('closed') is not True:
                raise ValueError('Hermes did not confirm close')
            request(data['endpoint'], '_stop')
            path.unlink()


def serve(endpoint, cwd):
    """One persistent gateway owner; reconnecting TaskQ commands use only its handle."""
    os.umask(0o077)
    replies = queue.Queue()
    process = subprocess.Popen(json.loads(os.environ['TASKQ_HERMES_COMMAND']), cwd=cwd,
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    def read():
        for line in process.stdout:
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if 'id' in message and 'method' not in message:
                replies.put(message)
            elif 'id' in message:  # unattended approval/secret requests fail closed
                process.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': message['id'],
                    'error': {'code': -32601, 'message': 'TaskQ has no interactive approval host'}}) + '\n')
                process.stdin.flush()
        replies.put(None)
    threading.Thread(target=read, daemon=True).start()
    endpoint = Path(endpoint)
    try:
        endpoint.mkdir(mode=0o700)
        (endpoint / 'owner.json').write_text(json.dumps({'pid': os.getpid()}))
        serial = 0
        stop = False
        while process.poll() is None and not stop:
            for pending in endpoint.glob('*.req'):
                req = json.loads(pending.read_text())
                pending.unlink()
                response = pending.with_suffix('.reply')
                if req['method'] == '_stop':
                    response.write_text('{"result":true}')
                    stop = True
                    # Let the caller observe the acknowledgement before removing IPC.
                    deadline = time.monotonic() + 2
                    while response.exists() and time.monotonic() < deadline:
                        time.sleep(.02)
                    break
                serial += 1
                process.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': serial, **req}) + '\n')
                process.stdin.flush()
                try:
                    deadline = time.monotonic() + WAIT
                    while True:
                        reply = replies.get(timeout=max(.01, deadline - time.monotonic()))
                        if reply is None:
                            raise ValueError('Hermes gateway exited')
                        if reply.get('id') == serial:
                            break
                        if time.monotonic() >= deadline:
                            raise queue.Empty()
                except (queue.Empty, ValueError) as error:
                    reply = {'error': str(error) or 'Hermes RPC timeout'}
                temporary = response.with_suffix('.tmp')
                temporary.write_text(json.dumps(reply))
                temporary.rename(response)
            time.sleep(.02)
    finally:
        import shutil
        shutil.rmtree(endpoint, ignore_errors=True)
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stdin.close()
        process.stdout.close()


if __name__ == '__main__':
    serve(sys.argv[1], sys.argv[2])
