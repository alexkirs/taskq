"""Explicit behavioral selection; unknown/shared changes deliberately require all tests."""
import hashlib
import json
import locale
from pathlib import Path
import platform
import subprocess
import sys

CONTRACTS = {
    'admission': ('test_board_control.py', 'Tick', 'Commands', 'HermesAdmission', 'TightAssignmentDesign'),
    'lifecycle': ('test_lifecycle.py', 'test_model_integration.py', 'StateKernel', 'TestStateKernel', 'Commands', 'RealChild'),
    'answers': ('SubmittedResultRejection', 'test_receipts.py', 'test_subscriptions.py', 'test_native_action.py', 'test_sender_handshake.py',
                'test_board_control.py', 'EventDelivery', 'Wait', 'MultiPM'),
    'recovery': ('RecoveryQualification', 'test_legacy_reconcile.py', 'test_repair_reset.py', 'test_repair_owner_risk.py',
                 'Failed', 'RecoveryTimeout', 'Cleanup', 'SchemaRepair', 'StateKernel', 'TestStateKernel'),
    'capacity': ('test_board_capacity.py', 'test_model_capacity.py', 'Model', 'Tick', 'Coordination',
                 'RealChild', 'TestStateKernel'),
    'restart': ('test_host_service.py', 'test_model_integration.py', 'RuntimeProcessBoundary', 'Wait', 'RealChild'),
    'upgrade': ('Contract', 'DirectPublication', 'PullRequests', 'ProjectBootstrap', 'test_portfolio.py'),
}


def contracts_for(nodeid):
    parts = nodeid.replace('\\', '/').split('::')
    filename = Path(parts[0]).name
    cls = parts[1] if len(parts) > 2 else ''
    return {name for name, targets in CONTRACTS.items() if filename in targets or cls in targets}


def affected(paths):
    selected = set()
    for path in paths:
        path = path.replace('\\', '/')
        if path.startswith('./'):
            path = path[2:]
        if path.startswith('tests/test_') and path.count('/') == 1:
            groups = contracts_for(path)
            if not groups or path == 'tests/test_single.py':
                return set(CONTRACTS)
            selected.update(groups)
        elif path == 'experiments/state_replay.py':
            selected.update(('lifecycle', 'recovery'))
        else:
            return set(CONTRACTS)
    return selected


def fingerprint(root):
    """Hash actual tracked bytes, not just a Git SHA; no file contents enter receipts."""
    def git(*args):
        return subprocess.check_output(['git', '-C', str(root), *args], text=True, encoding='utf-8').strip()
    files = git('ls-files', '-z', '--cached', '--others', '--exclude-standard').split('\0')
    hashes = {}
    for name in files:
        if name:
            path = root / name
            hashes[name] = hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else 'missing'
    environment = {'platform': platform.system(), 'machine': platform.machine(),
                   'python': platform.python_version(), 'encoding': locale.getpreferredencoding(False),
                   'filesystem_encoding': sys.getfilesystemencoding(),
                   'interpreter_sha256': hashlib.sha256(Path(sys.executable).read_bytes()).hexdigest(),
                   'virtual_environment': sys.prefix != sys.base_prefix}
    from importlib.metadata import distributions
    environment['dependencies'] = {dist.metadata['Name']: dist.version for dist in distributions()}
    value = {'files': hashes, 'environment': environment}
    digest = hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()
    return {'fingerprint': digest, 'commit': git('rev-parse', 'HEAD'),
            'tree': git('rev-parse', 'HEAD^{tree}'), **environment}
