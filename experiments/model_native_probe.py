"""One authorized fresh TaskQ-owned Codex worker, two synthetic turns, local board/Git.
No production projects, ARM, publication, security changes, or replacement on failure.
"""
import hashlib
import importlib.util
import json
import os
import pathlib
import sqlite3
import subprocess
import sys
import time

TASKQ = pathlib.Path(__file__).resolve().parents[1] / 'taskq.py'
spec = importlib.util.spec_from_file_location('model_probe_taskq', TASKQ)
q = importlib.util.module_from_spec(spec); spec.loader.exec_module(q)
fixture_spec = importlib.util.spec_from_file_location('board_fixture', pathlib.Path(__file__).with_name('lifecycle_native_probe.py'))
fixture = importlib.util.module_from_spec(fixture_spec); fixture_spec.loader.exec_module(fixture)


def main():
    if len(sys.argv) != 2: raise SystemExit('pass one fresh private output directory')
    root = pathlib.Path(sys.argv[1]).resolve(); root.mkdir(parents=True, exist_ok=False); os.chmod(root, 0o700)
    actor = q.origin()
    if not actor or actor.get('runtime') != 'codex' or not actor.get('session'):
        raise SystemExit('genuine native Codex controller required; never spoof session identity')
    value = 'MODEL_PROBE_VALUE_20261010\n'
    source = 'TASKQ='+repr(str(TASKQ))+'\n'+fixture.BOARD_SOURCE + '''
def comment(n,text):
 with connect() as db:
  value=json.loads(db.execute('SELECT body FROM issues WHERE id=?',(n,)).fetchone()[0])
  value.setdefault('comments',[]).append(text)
  db.execute('UPDATE issues SET body=? WHERE id=?',(json.dumps(value),n))
def history(n):return get(n)
def ensure_event_label():pass
'''
    (root/'board.py').write_text(source)
    (root/'taskq.json').write_text(json.dumps({'board': 'board.py', 'repo': 'isolated/model-two-turn', 'publish': 'direct',
        'update': False, 'workspace': 'external', 'codex': ['-s', 'workspace-write'], 'limits': {'claude': 0, 'codex': 1},
        'capacity': {'project_caps': {'model:codex': 1}, 'host_caps': {'model:codex': 1, 'heavy': 1},
                     'host_path': str(root/'host.sqlite')}}))
    (root/'AGENTS.md').write_text('''This is an owner-authorized isolated TaskQ model qualification, not production.
Use this existing local Git checkout directly. No origin/remote exists; do not fetch,
rebase, push, publish, create worktrees, launch monitors, or modify global settings.
Only artifact.txt is an output. First turn asks the owner the prescribed synthetic
question via native taskq and stops. The answer turn writes exact owner text to the
artifact, commits locally, calls native applied twice and native result, then stops.
No background commands or unrelated API calls. Generic publication instructions do
not authorize publication in this fixture. Existing sandbox and account are preserved.
''')
    git = ['git', '-C', str(root)]
    for args in (['init', '-q'], ['config', 'user.name', 'TaskQ fixture'], ['config', 'user.email', 'fixture@example.invalid'],
                 ['add', 'AGENTS.md', 'taskq.json', 'board.py'], ['commit', '-qm', 'Initialize isolated model fixture']):
        subprocess.run(git+args, check=True, capture_output=True)
    board = q.load_file('board.py', root)
    goal = '''Owner-authorized isolated two-turn proof. Work only in this existing checkout,
with no remote operations, extra workers or background jobs. Initial turn MUST ask
exactly one native TaskQ question: "Which exact synthetic content should artifact.txt contain?"
with two options "owner supplies text" and "stop", then end the turn without writing artifact.txt.
On the owner's answer, write artifact.txt exactly as the supplied text including its
final newline; make one local Git commit. Read the admitted answer event ID from
the resume prompt/board. Run native taskq applied N:ID --artifact artifact.txt --sha
the full commit SHA, repeat the exact applied command once, then native taskq result
with that SHA, checks "local exact artifact and native receipt", and stop. Never
push/fetch/rebase/publish; the manager verifies and handles the result locally.
Keep every TaskQ command in this checkout and use your genuine native engine identity.
'''
    raw = {'event_schema': q.EVENT_SCHEMA, 'event_seq': 0, 'events': [], 'scope': ['artifact.txt'], 'deps': [],
           'claim': {'runtime': 'codex', 'session': None, 'name': actor['name']}, 'supervisor': actor, 'pm': actor,
           'result': None, 'order': None}
    issue = {'iid': 1, 'title': 'Isolated two-turn Codex proof', 'labels': ['q-doing', 'priority-1', 'code', 'run:codex'],
             'state': 'open', 'body': q.block('## Goal\n\n'+goal, raw), 'comments': [],
             'updated_at': '2026-10-10T00:00:00Z'}
    with board.connect() as db: db.execute('INSERT INTO issues VALUES (1,?)', (json.dumps(issue),))
    host = q.SQLiteCapacity(root/'host.sqlite', 'host', actor['name'], {'model:codex': 1, 'heavy': 1})
    host.reserve('unused-heavy-control', {'age': time.time(), 'priority': 1, 'demand': {'heavy': 1}})
    commands = []; checkpoints = []
    env = {**os.environ, 'TASKQ_RUNTIME': 'codex', 'TASKQ_LIMITS': '{"claude":0,"codex":1}', 'TASKQ_COMPRESS': 'off'}
    def command(*arguments):
        start = time.time()
        result = subprocess.run([sys.executable, str(TASKQ), *arguments], cwd=root, env=env,
                                capture_output=True, text=True, timeout=90)
        commands.append({'arguments': list(arguments), 'exit': result.returncode, 'elapsed': time.time()-start,
                         'stdout': result.stdout, 'stderr': result.stderr})
        (root/'commands.json').write_text(json.dumps(commands, indent=2))
        if result.returncode: raise RuntimeError('native command failed: '+ ' '.join(arguments[:2])+'; see commands.json')
    def current(): return q.issue_data(board.get(1))
    def wait(predicate, label):
        deadline = time.monotonic()+360
        while time.monotonic() < deadline:
            data = current()
            (root/'progress.json').write_text(json.dumps({'label': label, 'model_turns': data.get('model_turns'),
                'state': board.get(1)['labels'], 'result': data.get('result')}, indent=2))
            if predicate(data): return data
            time.sleep(1)
        raise RuntimeError('bounded wait expired: '+label+'; no replacement or model retry')
    def checkpoint(label, data):
        turn = data['model_turns']['worker']; lease = host.observe(turn['key'])
        project = board.capacity_provider({'model:codex': 1})
        assert lease['phase'] == project.observe(turn['key'])['phase'] == 'released'
        assert q.process_state(turn['child']['pid'], turn['child']['birth']) == 'dead'
        assert host.observe('unused-heavy-control')['phase'] == 'reserved'
        checkpoints.append({'label': label, 'turn': turn, 'claim': data['claim'], 'model_released': True,
                            'heavy_control_retained': True})
    try:
        command('run', '1', '--text', 'Perform only the prescribed isolated first question, then stop')
        first = wait(lambda data: any(e['action']=='ask' for e in data.get('events', [])) and
                     data.get('model_turns', {}).get('worker', {}).get('phase')=='complete', 'first ask and model settlement')
        sid = first['claim']['session']; checkpoint('first-question', first)
        assert not (root/'artifact.txt').exists()
        ask = next(e for e in first['events'] if e['action']=='ask')
        command('ack', f'1:{ask["id"]}', '--role', 'manager')
        command('ack', f'1:{ask["id"]}', '--role', 'supervisor')
        command('answer', '1', '--text', value)
        second = wait(lambda data: bool(data.get('result')) and
                      data.get('model_turns', {}).get('worker', {}).get('phase')=='complete', 'second artifact/result and model settlement')
        assert second['claim']['session'] == sid
        assert second['model_turns']['worker']['generation'] == 2
        checkpoint('second-result', second)
        assert (root/'artifact.txt').read_bytes() == value.encode()
        sha = subprocess.check_output(git+['rev-parse', 'HEAD'], text=True).strip()
        assert second['result']['sha'] == sha
        assert subprocess.check_output(git+['show', sha+':artifact.txt']) == value.encode()
        answer_id = second['model_turns']['worker']['event_ids'][0]
        proof = second['application_receipts'][str(answer_id)]
        assert proof['worker']['session'] == sid and proof['git_sha'] == sha
        assert proof['sha256'] == hashlib.sha256(value.encode()).hexdigest()
        log = root/'.taskq'/'T1.log'; before = log.read_bytes()
        assert sum(json.loads(line).get('type')=='turn.started' for line in before.splitlines()) == 2
        for event in second.get('events', []):
            if q.recipient('supervisor', actor) in set(event['recipients'])-set(event['acks']):
                command('ack', f'1:{event["id"]}', '--role', 'supervisor')
        command('answer', '1', '--text', value)
        command('tick', '--quiet'); command('tick', '--quiet')
        time.sleep(2)
        assert log.read_bytes() == before
        assert (root/'artifact.txt').read_bytes() == value.encode()
        host.release('unused-heavy-control')  # reserved only, never launched a resource process
        final = current()
        assert final['claim']['session'] == sid and final['result']['sha'] == sha
        assert all(set(e['recipients']) <= set(e['acks']) for e in final.get('events', []))
        with board.connect() as db: assert db.execute('SELECT count(*) FROM guard').fetchone()[0] == 0
        with host.transaction() as db: grants = [json.loads(row[0]) for row in db.execute('SELECT body FROM capacity_lease')]
        assert all(grant['phase']=='released' for grant in grants)
        result = {'passed': True, 'head': subprocess.check_output(['git','rev-parse','HEAD'],cwd=TASKQ.parent,text=True).strip(),
            'source_sha256': hashlib.sha256(TASKQ.read_bytes()).hexdigest(), 'actor': actor, 'worker_session': sid,
            'commands': commands, 'checkpoints': checkpoints, 'final_board': board.get(1), 'host_grants': grants,
            'model_turns': 2, 'same_native_worker': True, 'native_application_receipt': proof,
            'limits': {'model:codex': 1, 'heavy': 1}, 'all_model_children_dead': True,
            'scope': 'fresh owned Codex CLI; local SQLite/Git; no generic tool/app/resource drain or production activation'}
        (root/'results.json').write_text(json.dumps(result, indent=2))
        print(json.dumps({'passed': True, 'turns': 2, 'worker': sid, 'results': str(root/'results.json')}))
    except BaseException as error:
        (root/'failure-results.json').write_text(json.dumps({'passed': False, 'error': str(error),
            'commands': commands, 'checkpoints': checkpoints, 'board': board.get(1)}, indent=2))
        raise


if __name__ == '__main__': main()
