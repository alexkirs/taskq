"""#197 local pilot: when cleanup is due on an existing tick, and how one attempt is recorded.

Proposed fixture behavior, not an accepted production spec (docs/cleanup-schedule.md). Nothing here is wired
into tick, worker, the CLI or live configuration: `run` takes the native plan and the per-item apply as callables,
acts only on "Remove" items, leaves "Ask" items pending and never replays stored commands.
"""
from datetime import datetime, timedelta, timezone
import fcntl
import json
import os
from pathlib import Path
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

UTC = timezone.utc
SCHEDULES = ('hourly', 'daily', 'weekly', 'custom')
WEEKDAYS = ('mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun')
DEFAULTS = {'enabled': True, 'schedule': 'hourly', 'timezone': 'Etc/UTC'}
BACKOFF = timedelta(hours=1)
# Which optional keys each schedule takes; any other present key is a contradiction.
ALLOWED = {'hourly': set(), 'daily': {'at'}, 'weekly': {'at', 'weekday'},
           'custom': {'interval_minutes', 'weekdays', 'at'}}


def validate(raw):
    """`[cleanup]` table -> resolved config; every invalid or contradictory field in one ValueError.

    Missing keys take DEFAULTS (enabled hourly, Etc/UTC) and are listed in `defaulted`, so the report can say
    the timezone is the fallback; an explicit `enabled = false` or custom choice is kept as written."""
    raw = dict(raw or {})
    errors = []
    unknown = set(raw) - set(DEFAULTS) - {'at', 'weekday', 'interval_minutes', 'weekdays'}
    errors += [f'unknown key {k!r}' for k in sorted(unknown)]
    cfg = {**DEFAULTS, **{k: raw[k] for k in DEFAULTS if k in raw}}
    cfg['defaulted'] = sorted(k for k in DEFAULTS if k not in raw)
    if not isinstance(cfg['enabled'], bool):
        errors.append('enabled must be true or false')
    schedule = cfg['schedule']
    if schedule not in SCHEDULES:
        errors.append(f'schedule must be one of {", ".join(SCHEDULES)}, not {schedule!r}')
    try:
        cfg['zone'] = ZoneInfo(cfg['timezone'])
    except (ZoneInfoNotFoundError, ValueError, TypeError):
        errors.append(f'timezone {cfg["timezone"]!r} is not an IANA zone')
    extra = {'at', 'weekday', 'interval_minutes', 'weekdays'} & set(raw)
    for k in sorted(extra - ALLOWED.get(schedule, extra)):
        errors.append(f'{k} does not apply to schedule {schedule!r}')
    if 'at' in raw:
        try:
            hour, minute = (int(p) for p in str(raw['at']).split(':'))
            assert len(str(raw['at'])) == 5 and 0 <= hour < 24 and 0 <= minute < 60
            cfg['at'] = (hour, minute)
        except (ValueError, AssertionError):
            errors.append(f'at must be "HH:MM", not {raw["at"]!r}')
    if schedule in ('daily', 'weekly') and 'at' not in raw:
        errors.append(f'schedule {schedule!r} needs at = "HH:MM"')
    if schedule == 'weekly':
        if raw.get('weekday') not in WEEKDAYS:
            errors.append(f'weekday must be one of {", ".join(WEEKDAYS)}')
        else:
            cfg['days'] = {WEEKDAYS.index(raw['weekday'])}
    if schedule == 'daily':
        cfg['days'] = set(range(7))
    if schedule == 'custom':
        interval, days = raw.get('interval_minutes'), raw.get('weekdays')
        if (interval is None) == (days is None):
            errors.append('custom needs exactly one of interval_minutes or weekdays')
        elif interval is not None:
            # Any positive whole number of minutes: the draft's 60-minute minimum is dropped on purpose.
            if type(interval) is not int or interval <= 0:
                errors.append(f'interval_minutes must be a positive integer, not {interval!r}')
            if 'at' in raw:
                errors.append('at does not apply to custom interval_minutes')
        else:
            if (not isinstance(days, list) or not days or len(set(days)) != len(days)
                    or any(d not in WEEKDAYS for d in days)):
                errors.append(f'weekdays must be a non-empty list of distinct {", ".join(WEEKDAYS)}')
            else:
                cfg['days'] = {WEEKDAYS.index(d) for d in days}
            if 'at' not in raw:
                errors.append('custom weekdays needs at = "HH:MM"')
    if errors:
        raise ValueError('; '.join(errors))
    cfg['interval'] = (timedelta(hours=1) if schedule == 'hourly'
                       else timedelta(minutes=raw['interval_minutes']) if 'interval_minutes' in raw else None)
    return cfg


def wall_to_utc(day, hour, minute, zone):
    """Local wall time on `day` -> UTC instant. Repeated (fall back): the first one.
    Skipped (spring forward): the first valid instant after it, i.e. the transition."""
    wall = datetime(day.year, day.month, day.day, hour, minute)
    first = wall.replace(tzinfo=zone, fold=0).astimezone(UTC)
    if first.astimezone(zone).replace(tzinfo=None) == wall:
        return first
    # In a gap fold=1 maps before the transition and fold=0 after it; step to the first minute past it.
    # ponytail: minute steps, at most the gap length (60 min in tzdb today); bisect if a zone ever has longer gaps.
    instant = wall.replace(tzinfo=zone, fold=1).astimezone(UTC)
    while instant.astimezone(zone).replace(tzinfo=None) < wall:
        instant += timedelta(minutes=1)
    return instant.replace(second=0, microsecond=0)


def next_due(cfg, last_success):
    """First due instant (UTC) after the last successful completion; None = never run, due at once.

    Interval schedules count elapsed UTC time from the last success. Calendar schedules take the first
    occurrence of `at` on an allowed weekday in the configured zone strictly after the last success, so
    after any downtime there is exactly one due instant, never a backlog."""
    if last_success is None:
        return None
    if cfg['interval']:
        return last_success + cfg['interval']
    day = last_success.astimezone(cfg['zone']).date()
    for offset in range(15):
        candidate = day + timedelta(days=offset)
        if candidate.weekday() in cfg['days']:
            instant = wall_to_utc(candidate, *cfg['at'], cfg['zone'])
            if instant > last_success:
                return instant
    raise AssertionError('no occurrence within two weeks')


def check(cfg, state, now):
    """(due, reason, next instant) for a tick at `now` given the persisted state; reads nothing else."""
    if not cfg['enabled']:
        return False, 'disabled', None
    last = parse(state.get('last_success'))
    due = next_due(cfg, last)
    retry = parse(state.get('retry_at'))
    if retry and (due is None or retry > due):
        # A failed or partial attempt does not advance last_success, so the schedule alone would say "due"
        # on every tick; the one-hour backoff holds it to one retry per hour.
        return now >= retry, 'retry after backoff' if now >= retry else 'backoff', retry
    if due is None:
        return True, 'first run', now
    return now >= due, 'due' if now >= due else 'not due', due


def parse(text):
    return datetime.fromisoformat(text) if text else None


def stamp(instant):
    return instant.astimezone(UTC).isoformat() if instant else None


def load(path):
    try:
        return json.loads(Path(path).read_text())
    except FileNotFoundError:
        return {}


def save(path, state):
    tmp = Path(f'{path}.tmp')
    tmp.write_text(json.dumps(state, indent=1, sort_keys=True))
    os.replace(tmp, path)


def run(cfg, state_path, now, plan, apply, trigger='tick'):
    """One attempt under the shared cleanup lock; returns the report dict.

    `trigger`: 'tick' and 'idle' respect `enabled` and the schedule; 'manual' is the owner's explicit cleanup
    and runs regardless, but takes the same lock and records into the same state, so the next tick dedups.
    `plan()` returns a fresh native plan: items {'section': 'Remove'|'Ask the owner'|'Kept', ...}.
    `apply(item)` rechecks one Remove item at action time and returns True (removed) or False (refused);
    an exception is an error. Ask items stay pending and nothing else is ever applied."""
    report = {'observed': stamp(now), 'trigger': trigger, 'timezone': cfg['timezone'],
              'timezone_fallback': 'timezone' in cfg['defaulted']}
    with open(f'{state_path}.lock', 'a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return {**report, 'outcome': 'busy', 'reason': 'another cleanup holds the lock'}
        # Recheck under the lock: a run that finished while we waited has already moved the state.
        state = load(state_path)
        if state.get('running'):
            # The holder of `running` died (we hold the lock now): record it as failed, and the retry builds
            # a fresh plan; the interrupted attempt's items are never replayed.
            started = parse(state['running']['started'])
            state.update(last_attempt=stamp(started), last_outcome='interrupted', retry_at=stamp(started + BACKOFF))
            del state['running']
            save(state_path, state)
        if trigger == 'manual':
            reason = 'manual'
        else:
            due, reason, upcoming = check(cfg, state, now)
            if not due:
                return {**report, 'outcome': 'skipped', 'reason': reason, 'last_success': state.get('last_success'),
                        'next_due': stamp(upcoming)}
        state['running'] = {'started': stamp(now), 'trigger': trigger}
        save(state_path, state)
        counts = {'attempted': [], 'succeeded': [], 'refused': [], 'errors': [], 'pending_asks': []}
        try:
            items = plan()
        except Exception as error:
            items, counts['errors'] = [], [{'item': 'plan', 'error': str(error)}]
        for item in items:
            if item.get('section') == 'Ask the owner':
                counts['pending_asks'].append(item)
            elif item.get('section') == 'Remove':
                counts['attempted'].append(item)
                try:
                    counts['succeeded' if apply(item) else 'refused'].append(item)
                except Exception as error:
                    counts['errors'].append({'item': item, 'error': str(error)})
        outcome = ('success' if not counts['errors']
                   else 'partial' if counts['succeeded'] else 'failed')
        del state['running']
        state.update(last_attempt=stamp(now), last_outcome=outcome)
        if outcome == 'success':
            state.update(last_success=stamp(now), retry_at=None)
        else:
            state['retry_at'] = stamp(now + BACKOFF)
        save(state_path, state)
        upcoming = check(cfg, state, now)[2] if cfg['enabled'] else None
        return {**report, 'outcome': outcome, 'reason': reason, **counts,
                'last_success': state.get('last_success'), 'next_due': stamp(upcoming)}
