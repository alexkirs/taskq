"""#197: when native cleanup is due on an existing tick, and how one attempt is recorded.

Spec: https://github.com/alexkirs/taskq/wiki/Cleanup-schedule (docs/cleanup-schedule.md). `cleanup --apply`
(manual), the owner's tick (scheduled) and the idle stop (idle) all apply through `run`: one lock, one state
file in the checkout's `.local/`, a fresh native plan per attempt; stored commands are never replayed.
"""
from datetime import datetime, timedelta, timezone
import errno
import json
import os
import re
import subprocess
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
    if not isinstance(schedule, str) or schedule not in SCHEDULES:
        errors.append(f'schedule must be one of {", ".join(SCHEDULES)}, not {schedule!r}')
        schedule = None
    try:
        cfg['zone'] = ZoneInfo(cfg['timezone'])
    except (ZoneInfoNotFoundError, ValueError, TypeError):
        errors.append(f'timezone {cfg["timezone"]!r} is not an IANA zone')
    extra = {'at', 'weekday', 'interval_minutes', 'weekdays'} & set(raw)
    for k in sorted(extra - ALLOWED.get(schedule, extra)):  # unknown schedule: already refused above
        errors.append(f'{k} does not apply to schedule {schedule!r}')
    if 'at' in raw:
        match = isinstance(raw['at'], str) and re.fullmatch(r'([01][0-9]|2[0-3]):([0-5][0-9])', raw['at'])
        if match:
            cfg['at'] = (int(match[1]), int(match[2]))
        else:
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
            else:
                try:
                    cfg['interval'] = timedelta(minutes=interval)
                except OverflowError:
                    errors.append(f'interval_minutes {interval} is too large to represent')
            if 'at' in raw:
                errors.append('at does not apply to custom interval_minutes')
        else:
            if (not isinstance(days, list) or not days or any(not isinstance(d, str) or d not in WEEKDAYS for d in days)
                    or len(set(days)) != len(days)):
                errors.append(f'weekdays must be a non-empty list of distinct {", ".join(WEEKDAYS)}')
            else:
                cfg['days'] = {WEEKDAYS.index(d) for d in days}
            if 'at' not in raw:
                errors.append('custom weekdays needs at = "HH:MM"')
    if errors:
        raise ValueError('; '.join(errors))
    if schedule == 'hourly':
        cfg['interval'] = timedelta(hours=1)
    cfg.setdefault('interval', None)
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
        try:
            return last_success + cfg['interval']
        except OverflowError:
            raise ValueError(f'interval of {cfg["interval"]} after {stamp(last_success)} is past the last representable date')
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


def aware(instant):
    if instant.utcoffset() is None:
        raise ValueError(f'{instant!r} has no timezone')
    return instant


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


def settings():
    """The resolved [cleanup]: the personal taskq.local.toml's table when it has one (it replaces the shared one
    whole, so schedule keys never mix across files), else taskq.toml's, else the defaults. Both are validated
    when read (`configure`, `checked`); nothing is written back."""
    import taskq as core
    local = core.personal()
    raw, source = ((local['cleanup'], str(core.LOCAL)) if 'cleanup' in local
                   else (core.CLEANUP, 'taskq.toml') if core.CLEANUP else ({}, 'default'))
    return {**validate(raw), 'source': source}


def state_path():
    """Per checkout, beside the tick's beat and idle count in `<main checkout>/.local/`."""
    import taskq as core
    path = core.TICK_BEAT.with_name('taskq-cleanup.json')
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def locked(handle):
    """Non-blocking exclusive lock on an open file; False when another process holds it (as `tick` locks)."""
    try:
        if os.name == 'nt':
            import msvcrt
            handle.seek(0)
            msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return True
    except OSError as error:
        if error.errno not in (errno.EACCES, errno.EAGAIN):
            raise
        return False


def local_time(instant, cfg):
    return instant.astimezone(cfg['zone']).isoformat() if instant else None


def run(cfg, state_path, now, execute, trigger='tick', clock=lambda: datetime.now(UTC)):
    """One attempt under the shared cleanup lock; returns the report dict.

    `trigger`: 'tick' and 'idle' respect `enabled` and the schedule; 'manual' is the owner's explicit cleanup
    and runs regardless, but takes the same lock and records into the same state, so the next tick dedups.
    `execute()` builds a fresh native plan, applies only its Remove items with a recheck each, and returns
    {'attempted', 'succeeded', 'refused', 'errors', 'pending_asks'}: lists. An exception from it is an error.
    `now` is the attempt start; `clock()` (aware) is read once the attempt ends, and last_success, retry_at
    and the next due count from that completion, so a long run is not due again the moment it ends."""
    aware(now)
    report = {'observed': stamp(now), 'trigger': trigger, 'timezone': cfg['timezone'],
              'timezone_fallback': 'timezone' in cfg['defaulted'], 'settings': cfg.get('source', 'default')}
    with open(f'{state_path}.lock', 'a+b') as lock:
        if not locked(lock):
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
            reason, due_at = 'manual', None
        else:
            due, reason, due_at = check(cfg, state, now)
            if not due:
                return {**report, 'outcome': 'skipped', 'reason': reason, 'last_success': state.get('last_success'),
                        'next_due': stamp(due_at), 'next_due_local': local_time(due_at, cfg)}
        state['running'] = {'started': stamp(now), 'trigger': trigger}
        save(state_path, state)
        counts = {'attempted': [], 'succeeded': [], 'refused': [], 'errors': [], 'pending_asks': []}
        try:
            counts.update(execute())
        except (OSError, ValueError, SystemExit, RuntimeError, subprocess.SubprocessError) as error:
            # Anything else propagates and leaves `running`: the next attempt records it as interrupted.
            counts['errors'] = counts['errors'] + [{'item': 'plan', 'error': str(error)}]
        outcome = ('success' if not counts['errors']
                   else 'partial' if counts['succeeded'] else 'failed')
        # A naive clock raises here and leaves `running`, so the next attempt records this one as interrupted.
        # max(): a clock stepped back never puts completion before the start.
        finished = max(aware(clock()), now)
        del state['running']
        state.update(last_attempt=stamp(now), last_finished=stamp(finished), last_outcome=outcome)
        if outcome == 'success':
            state.update(last_success=stamp(finished), retry_at=None)
        else:
            state['retry_at'] = stamp(finished + BACKOFF)
        save(state_path, state)
        upcoming = check(cfg, state, finished)[2] if cfg['enabled'] else None
        return {**report, 'due_at': stamp(due_at), 'finished': stamp(finished), 'outcome': outcome, 'reason': reason,
                **counts, 'last_success': state.get('last_success'), 'next_due': stamp(upcoming),
                'next_due_local': local_time(upcoming, cfg)}
