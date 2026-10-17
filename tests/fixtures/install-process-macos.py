"""Installer and real project-loop crash/replay proof; manager and boards are isolated substitutes."""
import hashlib
import argparse
import json
import os
import pathlib
import plistlib
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import traceback


def manager():
    install = pathlib.Path(os.environ['TASKQ_INSTALL_PROCESS_CASE'])
    assert install.parent.name.startswith('taskq-install-process-')
    args = sys.argv[2:]
    active = install / 'manager-active'
    legacy = json.loads((install / 'legacy-native.json').read_text()) if (install / 'legacy-native.json').exists() else None
    legacy_active = install / 'legacy-active'
    legacy_label = 'com.taskq.' + legacy['project'] if legacy else None
    if args[0] == 'print':
        service = json.loads((install / 'service').read_text()) if (install / 'service').exists() else {}
        pid = service.get('pid', 0) if isinstance(service, dict) else 0
        if args[1].count('/') == 1:
            if active.exists():
                print(f'{pid} 0 com.taskq.service')
            if legacy and legacy_active.exists():
                print(f"{legacy['pid']} 0 {legacy_label}")
        else:
            label = args[1].split('/', 2)[2]
            is_legacy = legacy and label == legacy_label and legacy_active.exists()
            if not is_legacy and not (label == 'com.taskq.service' and active.exists()):
                return 0
            root = legacy['root'] if is_legacy else str(install)
            pid = legacy['pid'] if is_legacy else pid
            definition = pathlib.Path(os.environ['HOME']) / ('Library/LaunchAgents/' + label + '.plist')
            print(f'program = {install}/taskq\nworking directory = {root}\npath = {definition}\npid = {pid}\narguments = {{\n{install}/taskq\nservice\nrun\n}}')
        return 0
    if args[0] in ('bootstrap', 'kickstart'):
        with (install / 'native-submissions').open('a') as stream:
            stream.write(json.dumps(args) + '\n')
        active.touch()
        return 0
    if args[0] == 'bootout':
        if legacy and (args[-1].endswith('/' + legacy_label) or pathlib.Path(args[-1]).name == legacy_label + '.plist'):
            if legacy_active.exists():
                legacy_active.unlink()
                temporary = install / 'stop-legacy.pending'
                temporary.write_text(json.dumps(legacy))
                temporary.replace(install / 'stop-legacy.json')
            return 0
        active.unlink(missing_ok=True)
        return 0
    raise RuntimeError('unexpected substituted native action: ' + repr(args))


if len(sys.argv) > 1 and sys.argv[1] == '--manager':
    sys.exit(manager())

parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=pathlib.Path, required=True)
parser.add_argument('--output', type=pathlib.Path, required=True)
parser.add_argument('phases', nargs='*')
options = parser.parse_args()
repo = pathlib.Path(__file__).resolve().parents[2]
binary = options.binary.resolve()
assert sys.platform == 'darwin' and binary.is_file()
fixture_test = 'host::installation::tests::installation_process_fixture'
results = []


def run_case(phase):
    started = time.monotonic()
    base = pathlib.Path(tempfile.mkdtemp(prefix='taskq-install-process-')).resolve()
    install, root, home, tools = [base / name for name in ('install', 'first project', 'home', 'bin')]
    for path in (install, root, home, tools):
        path.mkdir()
    (root / 'taskq.json').write_text('{}\n')
    (install / 'host.json').write_text('{"limits":{"codex":1},"load_gate":{"enabled":false}}\n')
    (install / 'token').write_text('owned-fixture-not-a-credential\n')
    (install / 'token').chmod(0o600)
    (home / 'Library/LaunchAgents').mkdir(parents=True)
    candidate = install / 'releases/4.5.0/taskq'
    candidate.parent.mkdir(parents=True)
    shutil.copy2(binary, candidate)
    executable = install / 'fixture test'
    shutil.copy2(binary, executable)
    shim = tools / 'launchctl'
    shim.write_text('#!/bin/sh\nexec ' + shlex.quote(sys.executable) + ' ' + shlex.quote(str(pathlib.Path(__file__).resolve())) + ' --manager "$@"\n')
    shim.chmod(0o700)
    environment = {key: os.environ[key] for key in ('TMPDIR', 'USER', 'LOGNAME', 'LANG') if key in os.environ}
    environment.update(HOME=str(home), PATH=str(tools) + ':/usr/bin:/bin:/usr/sbin:/sbin', TASKQ_INSTALL=str(install), TASKQ_INSTALL_PROCESS_CASE=str(install), TASKQ_INSTALL_PROCESS_ROOT=str(root), TASKQ_INCIDENT_REPORTS='off')
    children, roles, logs, interrupted = [], [], [], set()
    item = {'phase': phase, 'scope': 'real installer and project loops; substituted native manager and in-memory boards', 'fixture': str(base)}

    def start(role, interrupt=None, project=root):
        env = dict(environment, TASKQ_INSTALL_PROCESS_ROLE=role, TASKQ_INSTALL_PROCESS_ROOT=str(project))
        if interrupt:
            env['TASKQ_INSTALL_INTERRUPT'] = interrupt
        path = install / f'{len(children)}-{role}.log'
        with path.open('wb') as stream:
            child = subprocess.Popen([str(executable), '--exact', fixture_test, '--ignored', '--nocapture'], env=env, stdin=subprocess.DEVNULL, stdout=stream, stderr=stream)
        children.append(child)
        roles.append(role)
        logs.append(path)
        return child

    host, legacy_actor = None, None
    rollback_case = phase in ('later-rollback', 'later-private-rollback', 'foreign-rollback', 'interrupted-restore')

    def selection(version, previous, label):
        for release in (version, previous):
            path = install / 'releases' / release / 'taskq'
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(('owned retained release ' + release).encode())
        (install / 'taskq').write_bytes((label + ' launcher').encode())
        (install / 'taskq').chmod(0o751)
        (install / 'current').write_text(version + '\n')
        (install / 'previous').write_text(previous + '\n')

    def ensure_host():
        nonlocal host
        if legacy_actor is not None:
            legacy_actor.poll()
        if host is None and (install / 'manager-active').exists():
            host = start('host')

    def until(predicate, seconds=30, actor=None):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            ensure_host()
            if predicate():
                return
            if actor is not None and actor.poll() is not None:
                raise RuntimeError('fixture actor exited before its completion signal')
            time.sleep(.02)
        raise TimeoutError('fixture completion signal expired')

    try:
        if phase == 'legacy-handoff':
            definition = home / ('Library/LaunchAgents/com.taskq.' + root.name + '.plist')
            definition.write_bytes(plistlib.dumps({'Label': 'com.taskq.' + root.name, 'WorkingDirectory': str(root), 'ProgramArguments': [str(install / 'taskq'), 'service', 'run']}))
            (install / 'taskq').write_bytes(b'owned original legacy launcher')
            (root / 'taskq.json').write_text('{"limits":{"codex":1}}\n')
            legacy_actor = start('legacy')
            cursor = install / 'projects.installing'
            until(lambda: cursor.exists() and json.loads(cursor.read_text())['phase'] == 'registered', actor=legacy_actor)
            pending = json.loads(cursor.read_text())
            baseline = json.loads((install / 'host-policy-migration/install.json').read_text())
            floor = [saved for saved in baseline['files'] if saved['path'] == str(install / 'rollback-floor')]
            assert len(floor) == 1 and floor[0]['digest'] is None, 'pre-host rollback-floor absence was not preserved'
            original_id = pending['id']
            child = json.loads((install / 'owned-work.json').read_text())
            until(lambda: (root / '.taskq/status.json').exists() and bool(json.loads((root / '.taskq/status.json').read_text()).get('kids')), actor=legacy_actor)
            before = json.loads((root / '.taskq/status.json').read_text())
            assert before['pid'] == legacy_actor.pid and before['kids'][0]['pid'] == child['pid']
            assert pending['legacy']['phase'] == 'draining' and not (install / 'controller-ready.json').exists()
            assert host is not None and host.poll() is None and host.pid != legacy_actor.pid
            until(lambda: json.loads((root / '.taskq/status.json').read_text())['at'] > before['at'], actor=legacy_actor)
            during = json.loads((root / '.taskq/status.json').read_text())
            assert during['pid'] == legacy_actor.pid and during['kids'][0]['pid'] == child['pid']
            assert not during.get('host_install') and not (install / 'controller-ready.json').exists()
            assert json.loads(cursor.read_text())['legacy']['phase'] == 'draining'
            item['owned_work_preserved_during_host_start'] = child
            item['work_before'] = before
            item['work_during'] = during
            (install / 'finish-owned-work').touch()
            until(lambda: (install / 'controller-ready.json').exists(), actor=legacy_actor)
            assert json.loads((install / 'controller-ready.json').read_text())['id'] == original_id
            assert json.loads(cursor.read_text())['legacy']['phase'] == 'ready'
            prepared = json.loads((root / '.taskq/status.json').read_text())
            assert prepared['pid'] == legacy_actor.pid and not prepared['kids']
            assert not prepared.get('host_install') and definition.exists()
            item['prepared_status'] = prepared
            item['controller'] = json.loads((install / 'controller-ready.json').read_text())
            (install / 'allow-retirement').touch()
            until(lambda: (install / 'controller-finished').exists())
            until(lambda: legacy_actor.poll() is not None)
            assert legacy_actor.returncode == 0
            until(lambda: json.loads((root / '.taskq/status.json').read_text()).get('pid') == host.pid)
            recovered = start('recover')
            until(lambda: recovered.poll() is not None)
            outcome = json.loads((install / 'installer-result.json').read_text())
            assert outcome['ok'] and outcome['status']['id'] == original_id and outcome['status']['state'] == 'observed', outcome
            assert not cursor.exists() and not definition.exists() and not (install / 'legacy-active').exists()
            item['new_host_status'] = json.loads((root / '.taskq/status.json').read_text())
            item.update(original_command=original_id, outcome=outcome, native_submission_count=len((install / 'native-submissions').read_text().splitlines()), passed=True)
            assert item['native_submission_count'] == 1
            return item
        if rollback_case:
            selection('3.0.0', '2.0.0', 'first')
        if phase == 'interrupted-restore':
            (install / 'rollback-floor').write_text('2.0.0\n')
        initial = start('install', 'snapshot' if rollback_case else phase)
        until(lambda: (install / 'interrupted-at').exists(), actor=initial)
        cursor = install / 'projects.installing'
        original = json.loads(cursor.read_text()) if cursor.exists() else None
        item['original_command'] = original['id'] if original else None
        item['registry_before_death'] = (install / 'projects').read_text() if (install / 'projects').exists() else None
        initial.kill()
        interrupted.add(initial.pid)
        item['installer_exit'] = initial.wait(timeout=5)
        (install / 'interrupted-at').unlink()
        if rollback_case:
            manifest = install / 'host-policy-migration/install.json'
            original_manifest = manifest.read_bytes()
            original_files = {path.name: path.read_bytes() for path in (manifest.parent / 'install-originals').iterdir() if path.is_file()}
            if phase == 'interrupted-restore':
                saved = json.loads(original_manifest)['files']
                floors = [(index, entry) for index, entry in enumerate(saved) if entry['path'] == str(install / 'rollback-floor')]
                assert len(floors) == 1, 'original rollback-floor missing from topology restore evidence'
                index, floor = floors[0]
                assert floor['digest'] == hashlib.sha256(b'2.0.0\n').hexdigest()
                assert original_files[str(index)] == b'2.0.0\n'
                item['pre_host_rollback_floor_preserved'] = True
            selection('4.4.0', '4.3.0', 'latest')
            if phase == 'later-private-rollback':
                for name in ('current', 'previous'):
                    (install / name).chmod(0o600)
            definition = home / 'Library/LaunchAgents/com.taskq.service.plist'
            definition.write_bytes(plistlib.dumps({'Label': 'com.taskq.service', 'WorkingDirectory': str(install), 'ProgramArguments': [str(install / 'taskq'), 'service', 'run'], 'FixtureOwnerNote': 'latest owner definition'}))
            definition.chmod(0o640)
            paths = [install / name for name in ('taskq', 'current', 'previous')] + [definition]
            before = {str(path): (path.read_bytes(), path.stat().st_mode & 0o777) for path in paths}
            environment['TASKQ_INSTALL_FAIL_ACTIVATION'] = '1'
            recovered = start('install', 'activation' if phase == 'foreign-rollback' else 'restored' if phase == 'interrupted-restore' else None)
            if phase == 'foreign-rollback':
                until(lambda: (install / 'interrupted-at').exists(), actor=recovered)
                definition.write_bytes(definition.read_bytes().replace(b'latest owner definition', b'newer owner definition'))
                after_edit = {str(path): (path.read_bytes(), path.stat().st_mode & 0o777) for path in paths}
                (install / 'release-interruption').touch()
            if phase == 'interrupted-restore':
                until(lambda: (install / 'interrupted-at').exists(), actor=recovered)
                restore_id = json.loads(cursor.read_text())['id']
                assert (install / 'taskq').read_bytes() == before[str(install / 'taskq')][0]
                assert (install / 'current').read_bytes() != before[str(install / 'current')][0]
                recovered.kill()
                interrupted.add(recovered.pid)
                recovered.wait(timeout=5)
                (install / 'interrupted-at').unlink()
                del environment['TASKQ_INSTALL_FAIL_ACTIVATION']
                recovered = start('recover')
                item['interrupted_restore_command'] = restore_id
            until(lambda: recovered.poll() is not None, actor=None)
            assert recovered.returncode == 0
            outcome = json.loads((install / 'installer-result.json').read_text())
            item['outcome'] = outcome
            item['native_submission_count'] = len((install / 'native-submissions').read_text().splitlines()) if (install / 'native-submissions').exists() else 0
            assert not outcome['ok'] and item['native_submission_count'] == 0
            after = {str(path): (path.read_bytes(), path.stat().st_mode & 0o777) if path.exists() else None for path in paths}
            item['latest_state_restored'] = before == after
            item['first_originals_preserved'] = manifest.read_bytes() == original_manifest and all((manifest.parent / 'install-originals' / name).read_bytes() == data for name, data in original_files.items())
            assert item['first_originals_preserved']
            if phase in ('later-rollback', 'later-private-rollback', 'interrupted-restore'):
                assert item['latest_state_restored'], 'later stopped-host selection was not restored'
                assert outcome['status']['state'] == 'failed' and not cursor.exists()
                if phase == 'interrupted-restore':
                    assert outcome['status']['id'] == restore_id, 'restore recovery changed command identity'
            else:
                assert after == after_edit, 'rollback overwrote a newer owner edit or partially restored another file'
                assert cursor.exists() and outcome['status']['state'] != 'observed'
                assert 'input changed before restore' in outcome['error'], 'foreign-edit refusal was not attributable to the changed input'
            item['passed'] = True
            return item
        recovered = start('recover' if original else 'install')
        until(lambda: recovered.poll() is not None, seconds=70, actor=None)
        assert recovered.returncode == 0
        outcome = json.loads((install / 'installer-result.json').read_text())
        item['outcome'] = outcome
        submissions = (install / 'native-submissions').read_text().splitlines() if (install / 'native-submissions').exists() else []
        item['native_submission_count'] = len(submissions)
        if phase == 'starting':
            assert not outcome['ok'] and 'unknown' in outcome['error']
            assert cursor.exists() and json.loads(cursor.read_text())['id'] == original['id']
            assert not submissions, 'uncertain replay submitted a replacement'
            assert outcome['status']['state'] == 'queued' and outcome['status'].get('reason')
        else:
            assert outcome['ok'], outcome
            assert outcome['status']['state'] == 'observed' and not cursor.exists()
            if original:
                assert outcome['status']['id'] == original['id'], 'recovery created another command'
            assert len(submissions) == 1, 'install/recovery repeated native submission'
            assert outcome['operation_count'] == 1, 'unresolved orphan command remains after retry'
            if phase == 'observed':
                second = base / 'second project'
                second.mkdir()
                (second / 'taskq.json').write_text('{}\n')
                first_status = json.loads((root / '.taskq/status.json').read_text())
                assert first_status['complete'] and first_status['host_instance']
                assert first_status['pid'] == host.pid
                before = {name: (install / name).read_bytes() if (install / name).exists() else None for name in ('taskq', 'current', 'previous')}
                (install / 'hold-project-ack').touch()
                attach = start('install', 'initial', project=second)
                until(lambda: (install / 'interrupted-at').exists(), actor=attach)
                attach_id = json.loads(cursor.read_text())['id']
                attach.kill()
                interrupted.add(attach.pid)
                attach.wait(timeout=5)
                (install / 'interrupted-at').unlink()
                attach = start('recover', project=second)
                until(lambda: json.loads(cursor.read_text())['phase'] == 'registered')
                until(lambda: 'project registration acknowledgement pending' in (install / 'resources/state.json').read_text())
                assert attach.poll() is None and not (second / '.taskq/status.json').exists()
                until(lambda: json.loads((root / '.taskq/status.json').read_text())['at'] > first_status['at'])
                sibling = json.loads((root / '.taskq/status.json').read_text())
                assert sibling['pid'] == host.pid and sibling['host_instance'] == first_status['host_instance']
                assert sibling['complete'] and not sibling.get('error') and not sibling['kids']
                assert all(task['state'] == 'ready' for task in sibling['tasks'])
                item['sibling_continued_while_second_ack_pending'] = True
                (install / 'hold-project-ack').unlink()
                until(lambda: attach.poll() is not None, actor=None)
                attached = json.loads((install / 'installer-result.json').read_text())
                item['second_root_result'] = attached
                item['second_root_command'] = attach_id
                assert attached['ok'] and attached['operation_count'] == 2 and attached['status']['id'] == attach_id, attached
                assert len((install / 'native-submissions').read_text().splitlines()) == 1
                assert before == {name: (install / name).read_bytes() if (install / name).exists() else None for name in before}
                assert set((install / 'projects').read_text().splitlines()) == {str(root), str(second)}
                statuses = [json.loads((project / '.taskq/status.json').read_text()) for project in (root, second)]
                assert all(status['pid'] == host.pid and status['host_instance'] == first_status['host_instance'] and status['complete'] for status in statuses)
                assert statuses[1]['host_install'] == attach_id
                item['project_statuses'] = statuses
                item['second_root_attach_without_reload'] = True
        item['passed'] = True
    except BaseException as error:
        item['passed'] = False
        item['error'] = repr(error)
        item['traceback'] = traceback.format_exc()[-6000:]
        item['failure_snapshot'] = {str(path.relative_to(base)): path.read_text(errors='replace')[-16000:] for path in (install / 'projects.installing', root / '.taskq/status.json', install / 'controller-error.txt', install / 'resources/state.json') if path.is_file()}
    finally:
        (install / 'finish-owned-work').touch()
        (install / 'allow-retirement').touch()
        (install / 'exit-host').touch()
        for child in children:
            if child.poll() is None:
                try:
                    child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)
        native_expected = (install / 'owned-work.json').exists() or (install / 'controller-ready.json').exists()
        if native_expected:
            verifier = start('verify-exit')
            verifier.wait(timeout=20)
            readback = json.loads((install / 'native-exit-readback.json').read_text()) if (install / 'native-exit-readback.json').exists() else None
            item['native_exit_readback'] = readback
            if not readback or readback['forced']:
                item['passed'] = False
                item['native_cleanup_error'] = 'owned native exit was not observed without forced teardown'
        item['actor_exits'] = [child.returncode for child in children]
        item['actors'] = [{'role': role, 'exit': child.returncode, 'expected_exit': -9 if child.pid in interrupted else 0} for role, child in zip(roles, children)]
        unexpected = [actor for actor in item['actors'] if actor['exit'] != actor['expected_exit']]
        if unexpected:
            item['passed'] = False
            item['teardown_error'] = 'unexpected actor exit: ' + repr(unexpected)
        item['logs'] = {path.name: path.read_text(errors='replace')[-6000:] for path in logs}
        if not native_expected or (item.get('native_exit_readback') or {}).get('all_absent', False):
            shutil.rmtree(base)
        item['fixture_storage_removed'] = not base.exists()
        item['elapsed_seconds'] = round(time.monotonic() - started, 3)
    return item


for phase in options.phases or ['snapshot', 'initial', 'received', 'prepared', 'starting', 'owned', 'registered', 'registry', 'applied', 'observed', 'later-rollback', 'later-private-rollback', 'foreign-rollback']:
    result = run_case(phase)
    results.append(result)
    print(json.dumps({key: result.get(key) for key in ('phase', 'passed', 'error', 'native_submission_count', 'second_root_attach_without_reload')}), flush=True)
    options.output.write_text(json.dumps({'scope': 'installer/project-loop process proof; manager and boards substituted; not native product or installed qualification', 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'source_files': {name: hashlib.sha256((repo / name).read_bytes()).hexdigest() for name in ('src/host_install.rs', 'src/host.rs', 'src/host_control.rs', 'src/main.rs', 'src/service.rs', 'Cargo.lock', 'tests/fixtures/install-process-macos.py')}, 'cases': results}, indent=2) + '\n')
sys.exit(0 if all(case['passed'] for case in results) else 1)
