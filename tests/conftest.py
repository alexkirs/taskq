"""Test-only contract selection and redacted run receipts."""
import datetime
import hashlib
import json
from pathlib import Path
import pytest
from tests.portfolio import CONTRACTS, affected, contracts_for, fingerprint


def pytest_addoption(parser):
    parser.addoption('--contract', action='append', choices=sorted(CONTRACTS), default=[])
    parser.addoption('--changed', action='append', default=[], help='Repository-relative changed path; unknown selects all')
    parser.addoption('--receipt', help='Write redacted run metadata to this JSON path')


def pytest_configure(config):
    config._contract_receipt = {'started_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                                'failures': [], 'selected': 0, 'deselected': 0, 'outcomes': {}}
    config._contract_receipt_path = config.getoption('--receipt')
    if config._contract_receipt_path:
        config._contract_receipt.update(fingerprint(Path(config.rootpath)))
        config._contract_receipt['hypothesis_seed'] = config.getoption('hypothesis_seed', default=None)


@pytest.hookimpl(hookwrapper=True, tryfirst=True)
def pytest_collection_modifyitems(config, items):
    original_count = len(items)
    requested = set(config.getoption('--contract')) | affected(config.getoption('--changed'))
    all_tests = not requested or requested == set(CONTRACTS)
    selected, excluded = [], []
    for item in items:
        groups = contracts_for(item.nodeid)
        (selected if all_tests or not groups or groups & requested else excluded).append(item)
    items[:] = selected
    if excluded:
        config.hook.pytest_deselected(items=excluded)
    yield  # Other selectors (-k/-m) must finish before recording the tested node set.
    node_digest = hashlib.sha256(json.dumps(sorted(item.nodeid for item in items)).encode()).hexdigest()
    collection_filters = ('keyword', 'markexpr', 'ignore', 'ignore_glob', 'deselect',
                          'lf', 'collectonly', 'override_ini')
    full = (len(items) == original_count and list(config.args) == ['tests']
            and not any(config.getoption(option, default=None) for option in collection_filters))
    options = {option: config.getoption(option, default=None) for option in collection_filters}
    options['args'] = list(config.args)
    options_digest = hashlib.sha256(json.dumps(options, sort_keys=True).encode()).hexdigest()
    config._contract_receipt.update(selected=len(items), deselected=original_count-len(items),
                                    selected_nodes_sha256=node_digest, full_suite=full,
                                    selection_options_sha256=options_digest,
                                    contracts=sorted(requested) if not all_tests else ['all'])


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    report = (yield).get_result()
    data = item.config._contract_receipt
    if report.when == 'call' or report.failed or report.skipped:
        key = report.when + ':' + report.outcome
        data['outcomes'][key] = data['outcomes'].get(key, 0) + 1
    if report.failed:
        # Node identity only: never capture parameter values, exceptions, locals or task text.
        data['failures'].append({'node': report.nodeid.split('[')[0], 'phase': report.when,
                                 'contracts': sorted(contracts_for(report.nodeid))})


def pytest_sessionfinish(session, exitstatus):
    config = session.config
    if config._contract_receipt_path:
        data = config._contract_receipt
        data['artifact_unchanged'] = fingerprint(Path(config.rootpath))['fingerprint'] == data['fingerprint']
        data.update(exitstatus=int(exitstatus), finished_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    collection_only=config.getoption('collectonly'))
        path = Path(config._contract_receipt_path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(data, indent=2) + '\n', encoding='utf-8')
