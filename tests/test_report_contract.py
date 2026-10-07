"""Mocked schema/readback conformance; runtime labels do not qualify transport adapters."""
import argparse
import contextlib
import copy
from datetime import datetime, timezone
import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import taskq as core

tick = sys.modules['taskq.tick']


class ReportContractTests(unittest.TestCase):
    def payload(self):
        now = datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z')
        return {'contract': tick.report_contract(), 'repository': 'https://example.com/org/repo',
                'profile': {'filter': '', 'mine': False, 'limits': {'codex': 1}},
                'board': 'https://example.com/board/1', 'observed_at': now, 'outcome': 'ok',
                'actions': [], 'refusals': [], 'source_status': 'available', 'validation': [],
                'workers': [{'task': '[#1](https://example.com/issues/1)', 'title': 'Work',
                             'state': 'doing', 'runtime': 'codex', 'machine': 'host',
                             'session': '[session](https://example.com/session/1)',
                             'last_activity': 'active', 'event_at': now, 'commit': 'unavailable'}]}

    def verify(self, report, runtime='codex', rendered=None):
        now = datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z')
        evidence = {'runtime': runtime, 'session': 'pm-session', 'source': 'https://example.com/messages/1',
                    'received_at': now, 'applied_at': now, 'report': report,
                    'rendered': rendered if rendered is not None else tick.render_report(report)}
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'readback.json'
            path.write_text(json.dumps(evidence))
            with contextlib.redirect_stdout(io.StringIO()) as output:
                try:
                    tick.verify_report(argparse.Namespace(file=path))
                except SystemExit as error:
                    self.assertEqual(error.code, 1)
            return json.loads(output.getvalue())

    def test_common_fresh_and_existing_labeled_readback_scenarios(self):
        for runtime in ('claude', 'codex', 'dot'):
            for lifecycle in ('fresh', 'already-running'):
                with self.subTest(runtime=runtime, lifecycle=lifecycle):
                    payload = self.payload()
                    self.assertEqual(tick.validate_report(payload), [])
                    self.assertEqual(self.verify(payload, runtime)['status'], 'applied')
                    self.assertEqual(self.verify(payload, runtime)['status'], 'applied')  # duplicate readback, no dispatch
                    omitted = copy.deepcopy(payload)
                    del omitted['workers'][0]['state']
                    self.assertTrue(tick.validate_report(omitted))
                    omitted = copy.deepcopy(payload)
                    omitted['workers'][0]['session'] = 'unavailable'
                    self.assertTrue(tick.validate_report(omitted))
                    changed = copy.deepcopy(payload)
                    changed['contract']['version'] += 1
                    self.assertEqual(self.verify(changed, runtime)['status'], 'unknown')
                    changed['contract'] = tick.report_contract()  # next safe pass
                    self.assertEqual(self.verify(changed, runtime)['status'], 'applied')
                    stale = copy.deepcopy(payload)
                    stale['observed_at'] = '2000-01-01T00:00:00Z'
                    self.assertTrue(tick.validate_report(stale))
                    unavailable = copy.deepcopy(payload)
                    unavailable['source_status'] = 'unavailable'
                    unavailable['board'] = 'unavailable'
                    self.assertEqual(self.verify(unavailable, runtime)['status'], 'unknown')
                    # Interrupted update keeps old evidence; it cannot silently count as new application.
                    with patch.object(tick, 'REPORT_VERSION', 2):
                        self.assertEqual(self.verify(payload, runtime)['status'], 'unknown')
                    self.assertEqual(self.verify(payload, runtime, rendered='Done')['status'], 'unknown')

    def test_event_during_pass_is_valid_but_future_event_is_not(self):
        report = self.payload()
        now = datetime.now(timezone.utc).timestamp()
        utc = lambda at: datetime.fromtimestamp(at, timezone.utc).isoformat().replace('+00:00', 'Z')
        report['observed_at'] = utc(now - 30)
        report['workers'][0]['event_at'] = utc(now - 10)
        self.assertEqual(tick.validate_report(report, now=now), [])
        self.assertEqual(self.verify(report)['status'], 'applied')
        report['workers'][0]['event_at'] = utc(now + 60)
        self.assertIn('worker event time unknown/invalid', tick.validate_report(report, now=now))
        self.assertEqual(self.verify(report)['status'], 'unknown')

    def test_long_pass_does_not_refresh_source_observation(self):
        report = self.payload()
        observed = datetime.fromtimestamp(1000, timezone.utc).isoformat().replace('+00:00', 'Z')
        report['observed_at'] = observed
        report['workers'][0]['event_at'] = observed
        args = argparse.Namespace()
        emitted = datetime.fromtimestamp(1901, timezone.utc)
        with patch.object(tick, 'datetime') as clock, patch.object(tick.time, 'time', return_value=1901), \
                contextlib.redirect_stdout(io.StringIO()) as output:
            clock.now.return_value = emitted
            tick.emit_report(args, report)
        self.assertEqual(report['observed_at'], observed)
        self.assertIn('invalid/stale observed_at; obtain a fresh tick', report['validation'])
        self.assertIn(observed, output.getvalue())

    def test_verifier_rejects_missing_validation_and_rendered_status(self):
        report = self.payload()
        missing = copy.deepcopy(report)
        del missing['validation']
        self.assertEqual(self.verify(missing, rendered=tick.render_report(report))['status'], 'unknown')
        rendered = tick.render_report(report)
        hidden_status = '\n'.join(line for line in rendered.splitlines() if not line.startswith('Source status:'))
        self.assertEqual(self.verify(report, rendered=hidden_status)['status'], 'unknown')
        hidden_validation = '\n'.join(line for line in rendered.splitlines() if not line.startswith('Validation:'))
        self.assertEqual(self.verify(report, rendered=hidden_validation)['status'], 'unknown')

    def test_verifier_rejects_hidden_or_invalid_validation(self):
        report = self.payload()
        rendered = tick.render_report(report)
        report['validation'] = ['source-specific blocker']
        self.assertEqual(self.verify(report, rendered=rendered)['status'], 'unknown')
        self.assertEqual(self.verify(report)['status'], 'unknown')  # visible blockers cannot acknowledge success
        for invalid in (None, 'no blockers', [None], [1], ['']):
            report['validation'] = invalid
            self.assertEqual(self.verify(report, rendered=rendered)['status'], 'unknown')

    def test_template_provenance_and_bootstrap(self):
        contract = tick.report_contract()
        self.assertEqual(contract['sha256'], hashlib.sha256(contract['template'].encode()).hexdigest())
        self.assertIn('05cf6aab1c6ed5fc9589b9e4673365cec34c58e6', contract['source'])
        with contextlib.redirect_stdout(io.StringIO()) as output:
            core.contract(argparse.Namespace(report=True))
        self.assertIn(contract['sha256'], output.getvalue())
        self.assertIn(contract['template'], output.getvalue())
        self.assertIn('unknown', output.getvalue())

    def test_changed_template_cannot_reuse_the_version_hash(self):
        with tempfile.TemporaryDirectory() as folder, patch.object(core, 'CONTRACTS', Path(folder)):
            (Path(folder) / 'pm-report-v1.md').write_text('changed without new version')
            with self.assertRaisesRegex(ValueError, 'hash mismatch'):
                tick.report_contract()

    def test_empty_workers_and_table_free_channel(self):
        payload = self.payload()
        payload['workers'] = []
        rendered = tick.render_report(payload)
        self.assertIn('Workers: none', rendered)
        self.assertEqual(self.verify(payload)['status'], 'applied')
        payload = self.payload()
        table_free = tick.render_report(payload).replace('|', '\n')
        self.assertEqual(self.verify(payload, rendered=table_free)['status'], 'applied')

    def test_omitted_payload_fields_and_links(self):
        payload = self.payload()
        for key in payload:
            omitted = copy.deepcopy(payload)
            del omitted[key]
            self.assertTrue(tick.validate_report(omitted), key)
        for key in ('board', 'repository'):
            omitted = copy.deepcopy(payload)
            omitted[key] = 'unavailable'
            self.assertTrue(tick.validate_report(omitted), key)
        omitted = copy.deepcopy(payload)
        omitted['workers'][0]['task'] = '#1'
        self.assertTrue(tick.validate_report(omitted))

    def test_missing_invalid_future_and_old_activity(self):
        self.assertTrue(tick.validate_report({}))
        for value in (None, [], 'nonsense', '2000-01-01T00:00:00', '2999-01-01T00:00:00Z'):
            payload = self.payload()
            payload['observed_at'] = value
            self.assertTrue(tick.validate_report(payload))
        payload = self.payload()
        payload['workers'][0]['event_at'] = '2000-01-01T00:00:00Z'
        self.assertEqual(tick.validate_report(payload), [])  # old activity is not stale observation
        payload['workers'] = None
        self.assertTrue(tick.validate_report(payload))

    def test_failure_and_quiet_pass_always_emit_report_without_receipt(self):
        args = argparse.Namespace(output={'actions': [], 'refusals': []})
        with patch.object(tick, 'queue_pass', return_value=[]), contextlib.redirect_stdout(io.StringIO()) as output:
            tick.tick_pass(args)
        self.assertEqual(output.getvalue().count('Board:'), 1)
        self.assertEqual(output.getvalue().count('## Workers'), 1)
        self.assertIn('unknown', output.getvalue())
        self.assertIn('report', args.output)
        with patch.object(tick, 'queue_pass', side_effect=OSError('source down')), \
                contextlib.redirect_stdout(io.StringIO()) as output, self.assertRaises(OSError):
            tick.tick_pass(args)
        self.assertIn('outcome: failure', output.getvalue())
        self.assertIn('Workers: unknown', output.getvalue())


if __name__ == '__main__':
    unittest.main()
