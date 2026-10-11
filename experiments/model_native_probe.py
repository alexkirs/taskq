"""Opt-in isolated native Codex proof: ask, answer, late steer, application, result.
No production board, install, ARM, publication, replacement or automatic recovery.
"""
import argparse
import hashlib
import importlib.util
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import time
import traceback
import uuid

TASKQ = pathlib.Path(__file__).resolve().parents[1] / 'taskq.py'
spec = importlib.util.spec_from_file_location('model_probe_taskq', TASKQ)
q = importlib.util.module_from_spec(spec); spec.loader.exec_module(q)
fixture_spec = importlib.util.spec_from_file_location('board_fixture', pathlib.Path(__file__).with_name('lifecycle_native_probe.py'))
fixture = importlib.util.module_from_spec(fixture_spec); fixture_spec.loader.exec_module(fixture)

# Records a real worker CLI refusal, without supplying identity or applying an event.
GATE = '''import json,os,pathlib,subprocess,sys,time
ROOT=pathlib.Path(__file__).resolve().parent
def save(name,value):
 path=ROOT/name;temporary=path.with_suffix('.tmp')
 temporary.write_text(json.dumps(value),encoding='utf-8');os.replace(temporary,path)
sha=sys.argv[1]
save('probe-ready.json',{'sha':sha})
deadline=time.monotonic()+120
while not (ROOT/'probe-steer.json').exists():
 if time.monotonic()>=deadline:raise SystemExit('fixture steer wait expired; no retry')
 time.sleep(.1)
event=json.loads((ROOT/'probe-steer.json').read_text('utf-8'))['event']
argv=[sys.executable,'-B',TASKQ,'result','1','--sha',sha,
      '--checks','exact answer artifact and native receipt','--text','late control must refuse']
started=time.time()
result=subprocess.run(argv,cwd=ROOT,capture_output=True,text=True,encoding='utf-8',timeout=90)
save('probe-refusal.json',{'argv':argv,'event':event,'started_at':started,'finished_at':time.time(),
                         'exit':result.returncode,'stdout':result.stdout,'stderr':result.stderr})
if result.returncode==0 or f'unapplied worker control events: 1:{event}' not in result.stderr:
 raise SystemExit('required native pending-control refusal was not observed')
print('Native late-control refusal recorded. End this model turn now.')
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', nargs='?', help='new disposable local fixture directory')
    parser.add_argument('--execute', action='store_true', help='explicitly authorize this isolated three-turn probe')
    parser.add_argument('--expected-head', help='reviewed full candidate SHA; tracked source must be clean')
    args = parser.parse_args()
    if not args.execute:
        print('INERT: supply a fresh output directory, --execute and --expected-head FULLSHA only after review.')
        return
    if not args.output or not re.fullmatch(r'[0-9a-f]{40}', args.expected_head or ''):
        parser.error('--execute requires output and --expected-head FULLSHA')
    source_git = ['git', '-C', str(TASKQ.parent)]
    head = subprocess.check_output(source_git + ['rev-parse', 'HEAD'], text=True).strip()
    if head != args.expected_head or subprocess.check_output(source_git + ['status', '--porcelain', '--untracked-files=no'], text=True).strip():
        raise SystemExit('candidate SHA differs or tracked source is dirty; no fixture created')
    source_hashes = {name: hashlib.sha256((TASKQ.parent/name).read_bytes()).hexdigest()
                     for name in ('taskq.py', 'taskq.md', 'experiments/model_native_probe.py', 'experiments/lifecycle_native_probe.py')}
    actor = q.origin()
    if not actor or actor.get('runtime') != 'codex' or not actor.get('session'):
        raise SystemExit('genuine native Codex controller required; never spoof session identity')
    env = {**os.environ, 'TASKQ_RUNTIME': 'codex', 'TASKQ_LIMITS': '{"claude":0,"codex":1}'}
    for key in ('TASKQ_INSTALL_DIR', 'TASKQ_RELEASE_COMMIT'):
        env.pop(key, None)  # fixture commands must never select the production launcher
    versions = {'python': sys.version, 'python_executable': sys.executable,
                'codex': subprocess.check_output([shutil.which('codex') or 'codex', '--version'], env=env, text=True).strip()}
    root = pathlib.Path(args.output).resolve(); root.mkdir(parents=True, exist_ok=False); os.chmod(root, 0o700)
    answer = 'ANSWER_' + uuid.uuid4().hex + '\n'
    steering = 'STEER_' + uuid.uuid4().hex + '\n'
    source = 'TASKQ='+repr(str(TASKQ))+'\n'+fixture.BOARD_SOURCE + '''
def comment(n,text):
 with connect() as db:
  value=json.loads(db.execute('SELECT body FROM issues WHERE id=?',(n,)).fetchone()[0])
  value.setdefault('comments',[]).append(text)
  db.execute('UPDATE issues SET body=? WHERE id=?',(json.dumps(value),n))
def history(n):return get(n)
def ensure_event_label():pass
'''
    (root/'board.py').write_text(source, encoding='utf-8')
    (root/'probe-gate.py').write_text('TASKQ='+repr(str(TASKQ))+'\n'+GATE, encoding='utf-8')
    (root/'taskq.json').write_text(json.dumps({'board': 'board.py', 'repo': 'isolated/board-control-three-turn',
        'publish': 'direct', 'update': False, 'workspace': 'external', 'codex': ['-s', 'workspace-write'],
        'limits': {'claude': 0, 'codex': 1}, 'capacity': {'project_caps': {'model:codex': 1},
        'host_caps': {'model:codex': 1, 'heavy': 1}, 'host_path': str(root/'host.sqlite')}}), encoding='utf-8')
    goal = '''Owner-authorized isolated three-turn proof. Work only in this existing checkout.
No remote operations, extra workers, background jobs, global changes or worktrees.
Only artifact.txt is the task output. Do not edit probe-gate.py, board.py or taskq.json.
Use genuine native identity and the absolute TaskQ entrypoint from your brief.
Read `taskq inbox 1 --role worker` at the start of each resumed turn and before result.
First turn: ask exactly one native TaskQ question, "Which exact synthetic content should artifact.txt contain?",
with options "owner supplies text" and "stop". End without creating artifact.txt.
Answer turn: read the full pending answer from the inbox. Write artifact.txt with exactly
that UTF-8 text including the final newline. Stage only artifact.txt and commit locally.
Call `taskq applied 1:ID --artifact artifact.txt --sha FULLSHA` for the answer, twice identically.
Then run `python -B probe-gate.py FULLSHA` synchronously. This bounded fixture helper waits
for late steering and attempts the real native result command with your genuine identity.
It succeeds only when result is refused because the late control is still unapplied.
After the helper finishes, END THIS TURN. Do not apply that late steer in the answer turn.
Steer turn: read the new pending steer from the inbox. Replace artifact.txt with exactly
its UTF-8 text including the final newline, commit only artifact.txt, call native applied
for this steer twice identically, read inbox again, and submit native result with the full
new commit SHA and checks "exact steer artifact and native receipt". End the turn.
Never claim a command succeeded from item.started alone: collect its terminal exit/output.
Do not fetch, push, publish or close; the controller verifies the result locally.
'''
    (root/'AGENTS.md').write_text(goal, encoding='utf-8')
    infrastructure = {name: hashlib.sha256((root/name).read_bytes()).hexdigest()
                      for name in ('AGENTS.md', 'taskq.json', 'board.py', 'probe-gate.py')}
    git = ['git', '-C', str(root)]
    for arguments in (['init', '-q'], ['config', 'user.name', 'TaskQ fixture'],
                      ['config', 'user.email', 'fixture@example.invalid'],
                      ['add', 'AGENTS.md', 'taskq.json', 'board.py', 'probe-gate.py'],
                      ['commit', '-qm', 'Initialize isolated board-control fixture']):
        subprocess.run(git+arguments, check=True, capture_output=True)
    board = q.load_file('board.py', root)
    raw = {'event_schema': q.EVENT_SCHEMA, 'event_seq': 0, 'events': [], 'scope': ['artifact.txt'], 'deps': [],
           'claim': {'runtime': 'codex', 'session': None, 'name': actor['name']}, 'supervisor': actor, 'pm': actor,
           'result': None, 'order': None}
    issue = {'iid': 1, 'title': 'Isolated board-only Codex proof', 'labels': ['q-doing', 'priority-1', 'code', 'run:codex'],
             'state': 'open', 'body': q.block('## Goal\n\n'+goal, raw), 'comments': [],
             'updated_at': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}
    with board.connect() as db: db.execute('INSERT INTO issues VALUES (1,?)', (json.dumps(issue),))
    host = q.SQLiteCapacity(root/'host.sqlite', 'host', actor['name'], {'model:codex': 1, 'heavy': 1})
    host.reserve('unused-heavy-control', {'age': time.time(), 'priority': 1, 'demand': {'heavy': 1}})
    commands = []; checkpoints = []
    def save(name, value):
        path = root/name; temporary = path.with_suffix('.tmp')
        temporary.write_text(json.dumps(value, indent=2), encoding='utf-8'); os.replace(temporary, path)
    def command(*arguments):
        started = time.time()
        result = subprocess.run([sys.executable, '-B', str(TASKQ), *arguments], cwd=root, env=env,
                                capture_output=True, text=True, encoding='utf-8', timeout=90)
        commands.append({'arguments': list(arguments), 'exit': result.returncode, 'started_at': started,
                         'finished_at': time.time(), 'stdout': result.stdout, 'stderr': result.stderr})
        save('commands.json', commands)
        if result.returncode: raise RuntimeError('native command failed; see commands.json')
        return result.stdout
    def current(): return q.issue_data(board.get(1))
    def wait(predicate, label):
        deadline = time.monotonic()+360
        while time.monotonic() < deadline:
            data = current()
            save('progress.json', {'label': label, 'model_turns': data.get('model_turns'),
                                  'state': board.get(1)['labels'], 'result': data.get('result')})
            if predicate(data): return data
            time.sleep(1)
        raise RuntimeError('bounded wait expired: '+label+'; no replacement or model retry')
    def acknowledge(data):
        for event in data.get('events', []):
            for role in ('manager', 'supervisor'):
                if q.recipient(role, actor) in set(event['recipients'])-set(event['acks']):
                    command('ack', f'1:{event["id"]}', '--role', role)
    try:
        command('run', '1', '--text', 'Perform only the prescribed isolated first question, then stop')
        first = wait(lambda data: any(e['action']=='ask' for e in data.get('events', [])) and
                     data.get('model_turns', {}).get('worker', {}).get('phase')=='complete', 'first question and settlement')
        sid = first['claim']['session']
        assert not (root/'artifact.txt').exists()
        acknowledge(first)
        command('answer', '1', '--text', answer)
        active = wait(lambda data: (root/'probe-ready.json').is_file(), 'answer applied; worker awaits late steer')
        second_turn = active['model_turns']['worker']
        assert second_turn['generation'] == 2 and second_turn['phase'] == 'bound'
        assert active['claim']['session'] == sid and not active.get('result')
        assert q.process_state(second_turn['child']['pid'], second_turn['child']['birth']) == 'running'
        answer_id, = second_turn['event_ids']
        answer_proof = active['application_receipts'][str(answer_id)]
        assert answer_proof['worker']['session'] == sid
        assert answer_proof['sha256'] == hashlib.sha256(answer.encode()).hexdigest()
        assert (root/'artifact.txt').read_bytes() == answer.encode()
        assert json.loads((root/'probe-ready.json').read_text('utf-8'))['sha'] == answer_proof['git_sha']
        inbox = json.loads(command('inbox', '1', '--role', 'manager'))
        steer_args = ('steer', '1', '--revision', inbox['revision'], '--command-id', 'fixture-late-steer', '--text', steering)
        receipt = json.loads(command(*steer_args))
        pending = current(); steer_id = pending['control_commands']['fixture-late-steer']['event']
        assert steer_id not in second_turn['event_ids'] and not receipt['repeated']
        assert pending['model_turns']['worker'] == second_turn
        event = next(e for e in pending['events'] if e['id'] == steer_id)
        assert q.recipient('worker', pending['claim']) not in event['acks']
        before_replay = json.dumps(board.get(1), sort_keys=True)
        assert json.loads(command(*steer_args))['repeated']
        assert json.dumps(board.get(1), sort_keys=True) == before_replay
        assert q.process_state(second_turn['child']['pid'], second_turn['child']['birth']) == 'running'
        save('probe-steer.json', {'event': steer_id})
        final = wait(lambda data: bool(data.get('result')) and
                     data.get('model_turns', {}).get('worker', {}).get('phase')=='complete', 'steer application, result and settlement')
        refusal = json.loads((root/'probe-refusal.json').read_text('utf-8'))
        assert refusal['exit'] != 0 and f'unapplied worker control events: 1:{steer_id}' in refusal['stderr']
        assert final['claim']['session'] == sid
        turns = final.get('model_turn_history', []) + [final['model_turns']['worker']]
        assert [turn['generation'] for turn in turns] == [1, 2, 3]
        assert turns[1]['event_ids'] == [answer_id] and turns[2]['event_ids'] == [steer_id]
        project = board.capacity_provider({'model:codex': 1})
        for turn in turns:
            assert turn['phase'] == 'complete' and turn['runtime']['session'] == sid
            assert host.observe(turn['key'])['phase'] == project.observe(turn['key'])['phase'] == 'released'
            assert q.process_state(turn['child']['pid'], turn['child']['birth']) == 'dead'
            checkpoints.append({'turn': turn, 'model_released': True, 'native_root_dead': True})
        assert host.observe('unused-heavy-control')['phase'] == 'reserved'
        sha = subprocess.check_output(git+['rev-parse', 'HEAD'], text=True).strip()
        proof = final['application_receipts'][str(steer_id)]
        assert proof['worker']['session'] == sid and proof['git_sha'] == final['result']['sha'] == sha
        assert proof['sha256'] == hashlib.sha256(steering.encode()).hexdigest()
        assert (root/'artifact.txt').read_bytes() == subprocess.check_output(git+['show', sha+':artifact.txt']) == steering.encode()
        log = root/'.taskq'/'T1.log'; before = log.read_bytes()
        entries = [json.loads(line) for line in before.splitlines()]
        assert sum(entry.get('type')=='turn.started' for entry in entries) == 3
        inbox_commands = [entry for entry in entries if entry.get('type') == 'item.completed'
                          and entry.get('item', {}).get('type') == 'command_execution'
                          and entry['item'].get('exit_code') == 0 and 'inbox' in entry['item'].get('command', '')
                          and 'taskq.inbox' in entry['item'].get('aggregated_output', '')]
        for payload in (answer.strip(), steering.strip()):
            assert any(payload in entry['item']['aggregated_output'] for entry in inbox_commands)
        applied_replays = []
        for entry in entries:
            item = entry.get('item', {})
            if entry.get('type') != 'item.completed' or item.get('type') != 'command_execution' or item.get('exit_code') != 0:
                continue
            for line in item.get('aggregated_output', '').splitlines():
                try: output = json.loads(line)
                except ValueError: continue
                if isinstance(output, dict) and output.get('applied') is True and output.get('repeated') is True:
                    applied_replays.append(output)
        assert {f'1:{answer_id}', f'1:{steer_id}'} <= {output.get('event') for output in applied_replays}
        home = pathlib.Path(os.environ.get('CODEX_HOME') or pathlib.Path.home()/'.codex')
        rollouts = list((home/'sessions').rglob(f'rollout-*-{sid}.jsonl'))
        assert len(rollouts) == 1 and rollouts[0].stat().st_size <= 32*1024*1024
        rollout_bytes = rollouts[0].read_bytes(); messages = []
        for line in rollout_bytes.splitlines():
            entry = json.loads(line); payload = entry.get('payload', {})
            if entry.get('type') == 'response_item' and payload.get('role') == 'user':
                messages.append(''.join(part.get('text', '') for part in payload.get('content', [])))
        wakes = []
        for turn in turns[1:]:
            matches = [text for text in messages if hashlib.sha256(text.encode()).hexdigest() == turn['request']['prompt_sha256']]
            assert len(matches) == 1, 'native rollout must contain exact admitted wake bytes'
            prompt = matches[0]
            assert 'inbox 1 --role worker' in prompt and f'1:{turn["event_ids"][0]}' in prompt
            assert answer.strip() not in prompt and steering.strip() not in prompt
            wakes.append({'generation': turn['generation'], 'sha256': turn['request']['prompt_sha256'], 'text': prompt})
        acknowledge(final)
        replay_state = current(); replay_artifact = (root/'artifact.txt').stat().st_mtime_ns
        assert json.loads(command(*steer_args))['repeated']
        command('answer', '1', '--text', answer)
        command('tick', '--quiet'); command('tick', '--quiet')
        assert current() == replay_state and log.read_bytes() == before
        assert (root/'artifact.txt').stat().st_mtime_ns == replay_artifact
        host.release('unused-heavy-control')
        final = current()
        assert all(set(event['recipients']) <= set(event['acks']) for event in final.get('events', []))
        with board.connect() as db: assert db.execute('SELECT count(*) FROM guard').fetchone()[0] == 0
        with host.transaction() as db: grants = [json.loads(row[0]) for row in db.execute('SELECT body FROM capacity_lease')]
        assert len(grants) == 4 and all(grant['phase']=='released' for grant in grants)
        assert source_hashes == {name: hashlib.sha256((TASKQ.parent/name).read_bytes()).hexdigest() for name in source_hashes}
        assert infrastructure == {name: hashlib.sha256((root/name).read_bytes()).hexdigest() for name in infrastructure}
        result = {'passed': True, 'head': head, 'source_hashes': source_hashes, 'versions': versions,
            'actor': actor, 'worker_session': sid, 'commands': commands, 'checkpoints': checkpoints,
            'final_board': board.get(1), 'host_grants': grants, 'model_turns': 3, 'same_native_worker': True,
            'application_receipts': {'answer': answer_proof, 'steer': proof}, 'late_result_refusal': refusal,
            'pointer_wakes': wakes, 'native_inbox_commands': inbox_commands,
            'applied_replays': applied_replays, 'fixture_infrastructure_hashes': infrastructure,
            'rollout': {'path': str(rollouts[0]), 'sha256': hashlib.sha256(rollout_bytes).hexdigest()},
            'limits': {'model:codex': 1, 'heavy': 1}, 'all_model_children_dead': True,
            'scope': 'native Codex CLI; disposable SQLite/Git; no external provider, installation compatibility, generic drain or production activation'}
        save('results.json', result)
        print(json.dumps({'passed': True, 'turns': 3, 'worker': sid, 'results': str(root/'results.json')}))
    except BaseException as error:
        save('failure-results.json', {'passed': False, 'head': head, 'source_hashes': source_hashes,
            'error': str(error), 'traceback': traceback.format_exc(), 'commands': commands, 'checkpoints': checkpoints, 'board': board.get(1),
            'recovery': 'preserve fixture and exact native handles; no automatic retry, kill, release or replacement'})
        raise


if __name__ == '__main__': main()
