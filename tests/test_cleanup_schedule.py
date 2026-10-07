"""#197: deterministic schedule fixtures (validation, due, DST, retry, dedup, lock). The native cleanup, tick
and idle paths run in tests/test_taskq.py `Cleanup`; neither is a receipt of a real existing-tick cleanup."""
from datetime import datetime, timedelta, timezone
import fcntl
import json
from pathlib import Path
import tempfile
import unittest

from taskq import cleanup_schedule as cs

UTC = timezone.utc
T0 = datetime(2026, 10, 8, 12, 0, tzinfo=UTC)


def at(text, zone='UTC'):
    return datetime.fromisoformat(text).replace(tzinfo=cs.ZoneInfo(zone)).astimezone(UTC)


class Validate(unittest.TestCase):
    def test_missing_settings_resolve_to_enabled_hourly_utc_fallback(self):
        for raw in (None, {}):
            cfg = cs.validate(raw)
            self.assertEqual((cfg['enabled'], cfg['schedule'], cfg['timezone']), (True, 'hourly', 'Etc/UTC'))
            self.assertEqual(cfg['interval'], timedelta(hours=1))
            self.assertEqual(cfg['defaulted'], ['enabled', 'schedule', 'timezone'])

    def test_explicit_disable_and_custom_survive(self):
        cfg = cs.validate({'enabled': False, 'schedule': 'custom', 'interval_minutes': 7, 'timezone': 'Europe/Berlin'})
        self.assertEqual((cfg['enabled'], cfg['interval'], cfg['defaulted']), (False, timedelta(minutes=7), []))
        cfg = cs.validate({'schedule': 'custom', 'weekdays': ['mon', 'fri'], 'at': '09:30'})
        self.assertEqual((cfg['days'], cfg['at']), ({0, 4}, (9, 30)))

    def test_any_positive_interval_no_sixty_minute_minimum(self):
        for minutes in (1, 15, 59, 60, 1440):
            self.assertEqual(cs.validate({'schedule': 'custom', 'interval_minutes': minutes})['interval'],
                             timedelta(minutes=minutes))

    def test_invalid_and_contradictory_fields_are_refused_visibly(self):
        bad = {
            'enabled': {'enabled': 'yes'},
            'one of hourly': {'schedule': 'monthly'},
            'IANA': {'timezone': 'Mars/Base'},
            'does not apply to schedule \'hourly\'': {'at': '10:00'},
            'needs at': {'schedule': 'daily'},
            'HH:MM': {'schedule': 'daily', 'at': '25:00'},
            'weekday must be': {'schedule': 'weekly', 'at': '10:00', 'weekday': 'funday'},
            'exactly one': {'schedule': 'custom'},
            'exactly one of': {'schedule': 'custom', 'interval_minutes': 60, 'weekdays': ['mon'], 'at': '10:00'},
            'positive integer': {'schedule': 'custom', 'interval_minutes': 0},
            'positive integer, not -5': {'schedule': 'custom', 'interval_minutes': -5},
            'positive integer, not True': {'schedule': 'custom', 'interval_minutes': True},
            'positive integer, not 1.5': {'schedule': 'custom', 'interval_minutes': 1.5},
            'at does not apply to custom': {'schedule': 'custom', 'interval_minutes': 5, 'at': '10:00'},
            'distinct': {'schedule': 'custom', 'weekdays': ['mon', 'mon'], 'at': '10:00'},
            'custom weekdays needs at': {'schedule': 'custom', 'weekdays': ['mon']},
            'unknown key': {'every': '1h'},
            'not \'1:002\'': {'schedule': 'daily', 'at': '1:002'},
            'not \'9:30\'': {'schedule': 'daily', 'at': '9:30'},
            'not \' 09:30\'': {'schedule': 'daily', 'at': ' 09:30'},
            'not 930': {'schedule': 'daily', 'at': 930},
            r'not \[\]': {'schedule': []},
            'not None': {'schedule': None},
            'non-empty list': {'schedule': 'custom', 'weekdays': [[]], 'at': '10:00'},
            'non-empty list of': {'schedule': 'custom', 'weekdays': 'mon', 'at': '10:00'},
            'too large to represent': {'schedule': 'custom', 'interval_minutes': 10 ** 18},
            'IANA zone': {'timezone': ['UTC']},
        }
        for message, raw in bad.items():
            with self.subTest(raw=raw), self.assertRaisesRegex(ValueError, message):
                cs.validate(raw)

    def test_all_errors_in_one_message(self):
        with self.assertRaises(ValueError) as caught:
            cs.validate({'enabled': 1, 'timezone': 'Nope/Nope'})
        self.assertIn('enabled', str(caught.exception))
        self.assertIn('IANA', str(caught.exception))


    def test_huge_but_representable_interval_is_allowed_and_overflow_is_visible(self):
        cfg = cs.validate({'schedule': 'custom', 'interval_minutes': 10 ** 12})
        with self.assertRaisesRegex(ValueError, 'past the last representable date'):
            cs.next_due(cfg, T0)


class Due(unittest.TestCase):
    def test_disabled_never_due(self):
        self.assertEqual(cs.check(cs.validate({'enabled': False}), {}, T0), (False, 'disabled', None))

    def test_first_use_due_immediately(self):
        self.assertEqual(cs.check(cs.validate({}), {}, T0), (True, 'first run', T0))

    def test_hourly_not_due_then_due(self):
        cfg, state = cs.validate({}), {'last_success': cs.stamp(T0)}
        self.assertEqual(cs.check(cfg, state, T0 + timedelta(minutes=59))[:2], (False, 'not due'))
        self.assertEqual(cs.check(cfg, state, T0 + timedelta(minutes=60)), (True, 'due', T0 + timedelta(hours=1)))

    def test_downtime_gives_one_due_not_a_backlog(self):
        cfg = cs.validate({})
        due, reason, instant = cs.check(cfg, {'last_success': cs.stamp(T0)}, T0 + timedelta(days=3))
        self.assertEqual((due, reason, instant), (True, 'due', T0 + timedelta(hours=1)))
        # After the single run, the next due is counted from it, not from the missed hours.
        self.assertEqual(cs.next_due(cfg, T0 + timedelta(days=3)), T0 + timedelta(days=3, hours=1))
        daily = cs.validate({'schedule': 'daily', 'at': '03:00'})
        self.assertEqual(cs.next_due(daily, at('2026-10-20T10:00')), at('2026-10-21T03:00'))

    def test_interval_counts_utc_elapsed_across_dst(self):
        cfg = cs.validate({'schedule': 'custom', 'interval_minutes': 90, 'timezone': 'America/New_York'})
        last = at('2026-03-08T01:30', 'America/New_York')
        self.assertEqual(cs.next_due(cfg, last) - last, timedelta(minutes=90))

    def test_daily_in_zone(self):
        cfg = cs.validate({'schedule': 'daily', 'at': '09:00', 'timezone': 'Asia/Tokyo'})
        self.assertEqual(cs.next_due(cfg, at('2026-10-08T08:59', 'Asia/Tokyo')), at('2026-10-08T09:00', 'Asia/Tokyo'))
        self.assertEqual(cs.next_due(cfg, at('2026-10-08T09:00', 'Asia/Tokyo')), at('2026-10-09T09:00', 'Asia/Tokyo'))

    def test_weekly_and_custom_weekdays(self):
        weekly = cs.validate({'schedule': 'weekly', 'weekday': 'mon', 'at': '06:00'})
        self.assertEqual(cs.next_due(weekly, T0), at('2026-10-12T06:00'))  # T0 is a Thursday
        custom = cs.validate({'schedule': 'custom', 'weekdays': ['tue', 'fri'], 'at': '06:00'})
        self.assertEqual(cs.next_due(custom, T0), at('2026-10-09T06:00'))
        self.assertEqual(cs.next_due(custom, at('2026-10-09T06:00')), at('2026-10-13T06:00'))

    def test_dst_skipped_time_runs_at_first_valid_instant(self):
        cfg = cs.validate({'schedule': 'daily', 'at': '02:30', 'timezone': 'America/New_York'})
        due = cs.next_due(cfg, at('2026-03-07T12:00', 'America/New_York'))
        self.assertEqual(due, datetime(2026, 3, 8, 7, 0, tzinfo=UTC))  # 03:00 EDT, the transition
        self.assertEqual(due.astimezone(cfg['zone']).strftime('%H:%M %Z'), '03:00 EDT')

    def test_dst_repeated_time_runs_once_at_first_occurrence(self):
        cfg = cs.validate({'schedule': 'daily', 'at': '01:30', 'timezone': 'America/New_York'})
        first = cs.next_due(cfg, at('2026-10-31T12:00', 'America/New_York'))
        self.assertEqual(first, datetime(2026, 11, 1, 5, 30, tzinfo=UTC))  # 01:30 EDT, not 01:30 EST
        self.assertEqual(cs.next_due(cfg, first), datetime(2026, 11, 2, 6, 30, tzinfo=UTC))


class Run(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.path = Path(self.dir.name, 'cleanup-state.json')
        self.cfg = cs.validate({})
        self.applied = []

    def tearDown(self):
        self.dir.cleanup()

    def plan(self, *items):
        """A stand-in for the native `apply_plan`: Remove items go to `apply`, Ask items stay pending, the rest is
        untouched. The real one runs in tests/test_taskq.py `Cleanup` against disposable repositories."""
        def execute():
            counts = {'attempted': [], 'succeeded': [], 'refused': [], 'errors': [], 'pending_asks': []}
            for item in items:
                if item['section'] == 'Ask the owner':
                    counts['pending_asks'].append(item)
                elif item['section'] == 'Remove':
                    counts['attempted'].append(item)
                    try:
                        counts['succeeded' if self.apply(item) else 'refused'].append(item)
                    except OSError as error:
                        counts['errors'].append({'item': item, 'error': str(error)})
            return counts
        return execute

    def apply(self, item):
        self.applied.append(item['target'])
        if item.get('raise'):
            raise OSError(item['raise'])
        return not item.get('refuse')

    def run_at(self, now, plan, trigger='tick', cfg=None, took=timedelta(0)):
        """An attempt started at `now` whose clock reads `now + took` when it ends."""
        return cs.run(cfg or self.cfg, self.path, now, plan, trigger, clock=lambda: now + took)

    def test_only_remove_applied_ask_pending_success(self):
        report = self.run_at(T0, self.plan({'section': 'Remove', 'target': 'tree-1'},
                                           {'section': 'Remove', 'target': 'dirty', 'refuse': True},
                                           {'section': 'Ask the owner', 'target': 'origin/taskq-9'},
                                           {'section': 'Kept', 'target': 'active'},
                                           {'section': 'other', 'target': 'unknown'}))
        self.assertEqual(self.applied, ['tree-1', 'dirty'])
        self.assertEqual(report['outcome'], 'success')
        self.assertEqual([i['target'] for i in report['refused']], ['dirty'])
        self.assertEqual([i['target'] for i in report['pending_asks']], ['origin/taskq-9'])
        self.assertEqual((report['last_success'], report['next_due']), (cs.stamp(T0), cs.stamp(T0 + timedelta(hours=1))))
        self.assertTrue(report['timezone_fallback'])

    def test_restart_dedup_and_manual_shares_state(self):
        self.run_at(T0, self.plan())
        # A new process (state reread from disk) inside the hour does nothing.
        report = self.run_at(T0 + timedelta(minutes=30), self.plan({'section': 'Remove', 'target': 'x'}))
        self.assertEqual((report['outcome'], report['reason'], self.applied), ('skipped', 'not due', []))
        self.run_at(T0 + timedelta(minutes=40), self.plan(), trigger='manual')
        # The manual run moved last_success, so the tick an hour after T0 is still not due.
        report = self.run_at(T0 + timedelta(minutes=61), self.plan())
        self.assertEqual((report['outcome'], report['next_due']),
                         ('skipped', cs.stamp(T0 + timedelta(minutes=100))))

    def test_disabled_tick_and_idle_skip_manual_runs(self):
        cfg = cs.validate({'enabled': False})
        for trigger in ('tick', 'idle'):
            self.assertEqual(self.run_at(T0, self.plan(), trigger, cfg)['reason'], 'disabled')
        self.assertEqual(self.run_at(T0, self.plan(), 'manual', cfg)['outcome'], 'success')
        self.assertIsNone(self.run_at(T0, self.plan(), 'manual', cfg)['next_due'])

    def test_failure_and_partial_keep_last_success_and_back_off(self):
        self.run_at(T0, self.plan())
        later = T0 + timedelta(hours=2)
        report = self.run_at(later, self.plan({'section': 'Remove', 'target': 'a'},
                                              {'section': 'Remove', 'target': 'b', 'raise': 'busy'}))
        self.assertEqual((report['outcome'], report['last_success']), ('partial', cs.stamp(T0)))
        self.assertEqual(report['next_due'], cs.stamp(later + cs.BACKOFF))
        # Ticks inside the backoff do not loop, even though the schedule alone says due.
        self.assertEqual(self.run_at(later + timedelta(minutes=5), self.plan())['reason'], 'backoff')

        def broken():
            raise RuntimeError('glab down')
        report = self.run_at(later + cs.BACKOFF, broken)
        self.assertEqual((report['reason'], report['outcome'], report['last_success']),
                         ('retry after backoff', 'failed', cs.stamp(T0)))
        report = self.run_at(later + 2 * cs.BACKOFF, self.plan())
        self.assertEqual((report['outcome'], report['last_success']), ('success', cs.stamp(later + 2 * cs.BACKOFF)))
        self.assertIsNone(json.loads(self.path.read_text())['retry_at'])

    def test_long_run_counts_from_completion_not_start(self):
        cfg = cs.validate({'schedule': 'custom', 'interval_minutes': 5})
        report = self.run_at(T0, self.plan({'section': 'Remove', 'target': 'slow'}), cfg=cfg, took=timedelta(hours=3))
        end = T0 + timedelta(hours=3)
        self.assertEqual((report['observed'], report['finished']), (cs.stamp(T0), cs.stamp(end)))
        self.assertEqual((report['last_success'], report['next_due']), (cs.stamp(end), cs.stamp(end + timedelta(minutes=5))))
        state = json.loads(self.path.read_text())
        self.assertEqual((state['last_attempt'], state['last_finished']), (cs.stamp(T0), cs.stamp(end)))
        # A tick right at the end is not due again; five minutes later it is.
        self.assertEqual(self.run_at(end, self.plan(), cfg=cfg)['reason'], 'not due')
        self.assertEqual(self.run_at(end + timedelta(minutes=5), self.plan(), cfg=cfg)['reason'], 'due')

    def test_failed_and_partial_retry_counts_from_completion(self):
        for target, took in (({'section': 'Remove', 'target': 'a', 'raise': 'x'}, timedelta(hours=2)),
                             ({'section': 'Remove', 'target': 'b'}, timedelta(minutes=90))):
            with self.subTest(took=took):
                self.path.unlink(missing_ok=True)
                items = [target, {'section': 'Remove', 'target': 'c', 'raise': 'y'}]
                report = self.run_at(T0, self.plan(*items), took=took)
                self.assertIn(report['outcome'], ('failed', 'partial'))
                self.assertEqual(report['next_due'], cs.stamp(T0 + took + cs.BACKOFF))
                self.assertEqual(self.run_at(T0 + took + timedelta(minutes=59), self.plan())['reason'], 'backoff')
                self.assertEqual(self.run_at(T0 + took + cs.BACKOFF, self.plan())['reason'], 'retry after backoff')

    def test_naive_times_are_refused(self):
        with self.assertRaisesRegex(ValueError, 'no timezone'):
            self.run_at(T0.replace(tzinfo=None), self.plan())
        with self.assertRaisesRegex(ValueError, 'no timezone'):
            cs.run(self.cfg, self.path, T0, self.plan(), clock=lambda: T0.replace(tzinfo=None))
        # The marker stays, so the next attempt records that one as interrupted.
        self.assertIn('running', json.loads(self.path.read_text()))

    def test_overlap_second_run_is_busy(self):
        with open(f'{self.path}.lock', 'a') as held:
            fcntl.flock(held, fcntl.LOCK_EX)
            report = self.run_at(T0, self.plan({'section': 'Remove', 'target': 'x'}))
        self.assertEqual((report['outcome'], self.applied), ('busy', []))
        self.assertFalse(self.path.exists())

    def test_interrupted_attempt_backs_off_then_uses_fresh_plan(self):
        cs.save(self.path, {'last_success': cs.stamp(T0 - timedelta(hours=5)),
                            'running': {'started': cs.stamp(T0), 'trigger': 'tick'}})
        report = self.run_at(T0 + timedelta(minutes=10), self.plan({'section': 'Remove', 'target': 'stale'}))
        self.assertEqual((report['reason'], self.applied), ('backoff', []))
        state = json.loads(self.path.read_text())
        self.assertEqual((state['last_outcome'], 'running' in state), ('interrupted', False))
        report = self.run_at(T0 + cs.BACKOFF, self.plan({'section': 'Remove', 'target': 'fresh'}))
        self.assertEqual((report['outcome'], self.applied), ('success', ['fresh']))


if __name__ == '__main__':
    unittest.main()
