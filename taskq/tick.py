"""`tick`: the coordinator's pass over the queue, board moves, the Workers table, the beat stamp."""
import argparse
from datetime import datetime, timezone
import contextlib
import errno
import hashlib
import io
import json
import os
from pathlib import Path
import plistlib
import re
import shlex
import subprocess
import sys
import time

import taskq as core
from taskq.worker import CLAUDE_ENDED


def clone_warning():
    """One line when this install is a clone off clean `main`: every session on the machine runs its working tree."""
    kind, where = core.install()
    if kind != 'clone':
        return None
    branch, dirty = core.git('rev-parse', '--abbrev-ref', 'HEAD', cwd=where), core.git('status', '--porcelain', '--untracked-files=no', cwd=where)
    faults = [f'on {branch or "an unknown branch"}, not main'] * (branch != 'main') + ['has uncommitted changes'] * bool(dirty)
    if faults:
        return f'Warning: the taskq clone {where} {" and ".join(faults)}; every session here runs it, edit in a worktree (README: Develop).'


def auto_update():
    """[update] auto: at most once per `every`; a pass that updated goes on as the new version (exec)."""
    if not core.UPDATE['auto'] or (core.UPDATE_STAMP.exists() and time.time() - core.UPDATE_STAMP.stat().st_mtime < core.seconds(core.UPDATE['every'])):
        return
    core.UPDATE_STAMP.parent.mkdir(parents=True, exist_ok=True)
    core.UPDATE_STAMP.touch()
    if core.update(argparse.Namespace(verbose=False)):
        sys.stdout.flush()
        os.execv(sys.executable, [sys.executable, '-m', 'taskq', *sys.argv[1:]])


def question(iid):
    """The latest question of an `ask` task, when `tick` last showed it (None: not yet) and the question note's id
    (#223: its revision). The newest page is enough: while a task waits in ask, only `shown` notes follow its question."""
    shown = None
    for item in core.collaborators(core.api('GET', f'issues/{iid}/notes?sort=desc&per_page=100&activity_filter=only_comments')):
        if item['body'].startswith('**shown**') and shown is None:
            shown = core.stamp(item['created_at'])
        elif item['body'].startswith('**ask**'):
            return item['body'].split('\n\n', 1)[-1], shown, item['id']
    return 'no question note', shown, None


def revision(item):
    """#223: the revision of a pending item: `<kind> <note id>` of the newest trusted note that put it where it is (the
    claim session's `result` for review, anyone's `ask` as `question` reads it, the claim session's `result`, `ask` or
    `problem` for doing), then the result block itself (`sha` and `checks`). The block is always part of it: a PUT that
    landed with a new `checks` and the same SHA while its note failed is a new revision with the old note.
    The note's body is kept on the item (`_note`): what the coordinator reads is the note the revision names, from
    the same read, never a second read that a newer note could have reached in between."""
    claim = item['claim'] or {}
    who = f'{claim.get("runtime")}:{(claim.get("session") or "")[:8]}'
    heads = {'review': [f'**result** · {who}'], 'ask': ['**ask**'],
             'doing': [f'**{kind}** · {who}' for kind in ('result', 'ask', 'problem')]}.get(item['state'], [])
    block = json.dumps(item.get('result'), sort_keys=True)
    item['_note'] = 'none'
    for note in reversed(core.comments(item['iid'])):
        head = note['body'].split('\n', 1)[0]
        if any(head.startswith(found) for found in heads):
            item['_note'] = note['body']
            return f'{head[2:].split("**", 1)[0]} {note["id"]} {block}'
    return f'block {block}'


def pending(item):
    """#223: the pending tuple of an item as one line: state, task, claim and revision, read once per item and kept
    (`_pending`) with the note it names. The wake key hashes these lines; the item is re-read against its line
    immediately before the send."""
    if '_pending' not in item:
        claim = item['claim'] or {}
        item['_pending'] = f'{item["state"]} {item["iid"]} {claim.get("runtime")}:{claim.get("session")} {revision(item)}'
    return item['_pending']


def still_pending(item, line):
    """A fresh read of the issue and its trusted notes gives the same pending line: state, claim, the note the
    revision names and the result block unchanged; any later result, ask or problem note or block write is a new line.
    A gone line means resolved or superseded, never that the coordinator received or applied the earlier wake."""
    issue = core.api('GET', f'issues/{item["iid"]}')
    fresh = core.parse(issue) if issue['state'] == 'opened' else None
    return bool(fresh) and pending(fresh) == line


TICK_MINUTES = 5  # the coordinator timer's interval (manager contract § 2)
TICK_LIVE_MINUTES = 3 * TICK_MINUTES  # a younger tick means another coordinator is armed; an older one, a stalled timer


def tick_beat():
    """Record this tick and report the previous one, so a second session does not arm a second tick and an armed
    timer that stopped firing is named (#91: seen live 2026-10-07, a */5 job stayed in CronList ~36 min without a tick)."""
    before = core.TICK_BEAT.stat().st_mtime if core.TICK_BEAT.exists() else None
    core.TICK_BEAT.parent.mkdir(parents=True, exist_ok=True)
    core.TICK_BEAT.touch()
    if before is None:
        return print('Last tick: none.')
    minutes = int((time.time() - before) // 60)
    live = ' (another coordinator is armed: do not CronCreate a second tick)' if minutes < TICK_LIVE_MINUTES else ''
    print(f'Last tick: {minutes} min ago{live}.')
    if minutes >= TICK_LIVE_MINUTES:
        print(f'If a timer is armed: no tick for {minutes} min (expected every {TICK_MINUTES}): check CronList, '
              'end a long turn or background loops in the coordinator session, re-arm (manager contract § 2).')


TICK_PROMPT_VERSION = 3  # raise with every change of TICK_PROMPT: an older --prompt-version gets the re-arm line
# The coordinator timer's prompt, word for word as in manager contract § 2 (a test keeps them equal).
TICK_PROMPT = f"""taskq tick prompt v{TICK_PROMPT_VERSION}. Run `cd <main checkout> && taskq update; taskq tick --prompt-version {TICK_PROMPT_VERSION}`
and do the coordinator pass by taskq-manager.md § 3 (`taskq contract` prints its path). Reply in the owner's language,
include the generated PM report even when nothing changed; apply its version/hash on this safe pass."""


def contract_seen():
    return core.TICK_BEAT.with_name('taskq-contract-seen')


def contract_news(prompt_version):
    """#110: a coordinator reads taskq-manager.md once when armed and its prompt stays as armed. Name a changed
    contract once per checkout (its hash next to the tick stamp) and a prompt older than TICK_PROMPT."""
    manager, seen = core.CONTRACTS / 'taskq-manager.md', contract_seen()
    new = hashlib.sha256(manager.read_bytes()).hexdigest()[:7]
    old, since = (seen.read_text().split() + ['', ''])[:2] if seen.exists() else ('', '')
    if old != new:
        seen.parent.mkdir(parents=True, exist_ok=True)
        seen.write_text(f'{new} {core.version()}\n')
        print(f'The coordinator contract changed since your last tick ({old or "none"}→{new}): re-read § 3 now '
              f'({manager}, {core.CONTRACTS / "taskq.md"}).')
        # ponytail: digest only for a clone install, whose version is a commit
        if since and (log := core.git('log', '-3', '--format=  %h %s', f'{since}..HEAD', '--', str(manager), cwd=core.CONTRACTS)):
            print(log)
    if (prompt_version or 1) < TICK_PROMPT_VERSION:
        prompt = TICK_PROMPT.replace('<main checkout>', str(core.ROOT))
        print(f'Your tick prompt is outdated (v{prompt_version or 1}, current v{TICK_PROMPT_VERSION}): re-arm with this prompt '
              '(CronDelete the old timer, CronCreate this one; manager contract § 2):\n\n'
              + ''.join(f'    {line}\n' for line in prompt.splitlines()))


REPORT_VERSION = 1
REPORT_SHA256 = 'a97cc42337da74014530a25091802dc3567f65d644426db93d2bd50079b82ff1'
REPORT_SOURCE = 'https://github.com/alexkirs/taskq/wiki/Home/05cf6aab1c6ed5fc9589b9e4673365cec34c58e6#versioned-pm-tick-report-contract-191'


def report_contract():
    template = (core.CONTRACTS / 'pm-report-v1.md').read_bytes()
    if hashlib.sha256(template).hexdigest() != REPORT_SHA256:
        raise ValueError('PM report template hash mismatch; restore/update the package before publishing a report')
    return {'version': REPORT_VERSION, 'sha256': REPORT_SHA256,
            'source': REPORT_SOURCE, 'template': template.decode()}


def report_bootstrap():
    contract = report_contract()
    print(f'PM report contract v{contract["version"]} sha256:{contract["sha256"]}\nSource: {contract["source"]}\n'
          + contract['template'] + '\nReceived/applied: unknown until a supported-channel report is verified; '
          'checkout markers and sends are not receipts.')


def report_row(item, agents):
    claim = item['claim'] or {}
    activity = item.get('_report_activity', 'unknown')
    sha = (item.get('result') or {}).get('sha')
    return {'task': core.ref(item), 'title': ' '.join(item['title'].replace('|', '/').split()), 'state': item['state'],
            'runtime': claim.get('runtime') or 'unknown', 'machine': core.where(claim).strip().lstrip('@') or 'unknown',
            'session': session_link(claim, agents.get(claim['session'])) if claim.get('session') else 'unavailable', 'last_activity': activity,
            'event_at': item.get('updated_at') or 'unknown',
            'commit': f'[{sha}]({core.commit_url(item, sha)})' if sha and item.get('web_url') else 'unavailable'}


def render_report(report):
    contract = report['contract']
    lines = [f'PM report v{contract["version"]} sha256:{contract["sha256"]}',
             f'Source: {contract["source"]}', f'Repository: {report["repository"]}',
             'Profile: ' + json.dumps(report['profile'], sort_keys=True),
             f'Observed: {report["observed_at"]}; outcome: {report["outcome"]}',
             f'Board: {report["board"]}', '## Workers',
             '| Task | State | Runtime | Session | Last activity | Event time | Commit |',
             '|---|---|---|---|---|---|---|']
    for row in report['workers']:
        lines.append(f'| {row["task"]} {row["title"]} | {row["state"]} | {row["runtime"]} @{row["machine"]} '
                     f'| {row["session"]} | {row["last_activity"]} | {row["event_at"]} | {row["commit"]} |')
    if not report['workers']:
        lines.append('Workers: none' if report['source_status'] == 'available' else 'Workers: unknown (source unavailable)')
    lines += ['Actions: ' + json.dumps(report['actions'], sort_keys=True),
              'Refusals: ' + json.dumps(report['refusals']),
              f'Source status: {report["source_status"]}; received/applied: unknown (verify supported-channel output).',
              'Validation: ' + json.dumps(report['validation'])]
    return '\n'.join(lines)


def report_timestamp(value):
    try:
        at = datetime.fromisoformat(value.replace('Z', '+00:00'))
        return at.timestamp() if at.tzinfo is not None and at.utcoffset().total_seconds() == 0 else None
    except (AttributeError, TypeError, ValueError, OverflowError):
        return None


def validate_report(report, now=None):
    """Validate data, never infer a session receipt from successful delivery."""
    errors = []
    required = ('contract', 'repository', 'profile', 'board', 'observed_at', 'outcome',
                'actions', 'refusals', 'workers', 'source_status', 'validation')
    if not isinstance(report, dict) or any(key not in report for key in required):
        return ['missing report fields']
    if (not isinstance(report['validation'], list)
            or any(not isinstance(blocker, str) or not blocker for blocker in report['validation'])):
        return ['invalid validation blockers']
    expected = report_contract()
    if not isinstance(report['contract'], dict) or any(report['contract'].get(key) != expected[key]
                                                      for key in ('version', 'sha256', 'source')):
        errors.append('unsupported report contract; run taskq update and apply the next safe tick')
    def url(value):
        return isinstance(value, str) and bool(re.search(r'https?://[^\s)]+', value))
    observed = report_timestamp(report['observed_at'])
    now = time.time() if now is None else now
    if observed is None or not 0 <= now - observed <= TICK_LIVE_MINUTES * 60:
        errors.append('invalid/stale observed_at; obtain a fresh tick')
    if report['outcome'] not in ('ok', 'judgement_needed', 'failure', 'unknown', 'refused'):
        errors.append('invalid outcome')
    profile = report['profile']
    if (not url(report['repository']) or not isinstance(profile, dict)
            or any(key not in profile for key in ('filter', 'mine', 'limits'))):
        errors.append('invalid repository/profile identity')
    if not url(report['board']):
        errors.append('board unavailable; verify board source before board decisions')
    if report['source_status'] != 'available':
        errors.append('source unavailable; preserve current work and retry next safe tick')
    if not all(isinstance(report[key], list) for key in ('workers', 'actions', 'refusals')):
        return errors + ['invalid report lists']
    for row in report['workers']:
        fields = ('task', 'title', 'state', 'runtime', 'machine', 'session', 'last_activity', 'event_at', 'commit')
        if not isinstance(row, dict) or any(not isinstance(row.get(key), str) or not row[key] for key in fields):
            errors.append('missing worker fields')
            continue
        if row['state'] not in (*core.STATES, 'unknown'):
            errors.append('invalid worker state')
        if not url(row['task']):
            errors.append('worker task link missing')
        if not url(row['session']):
            errors.append('worker session link unavailable; use the printed attach/app reference')
        if row['commit'] != 'unavailable' and not url(row['commit']):
            errors.append('worker commit link missing')
        event = report_timestamp(row['event_at'])
        if event is None or event > now:
            errors.append('worker event time unknown/invalid')
        # An old issue update is truthful activity, not a stale observation of current state.
    return errors


def verify_report(args):
    """Read an actual channel readback supplied by the adapter; no receipt store or transport."""
    evidence = json.loads(Path(args.file).read_text())
    if not isinstance(evidence, dict):
        raise SystemExit('channel readback must be an object')
    report = evidence.get('report')
    errors = validate_report(report)
    for key in ('runtime', 'session', 'source', 'received_at', 'applied_at', 'rendered'):
        if not isinstance(evidence.get(key), str) or not evidence[key] or evidence[key] in ('unknown', 'unavailable'):
            errors.append(f'channel acknowledgement missing {key}')
    if isinstance(evidence.get('source'), str) and not re.match(r'https?://', evidence['source']):
        errors.append('channel source link unavailable')
    if not errors:
        rendered = evidence['rendered']
        # A table-free channel may copy every datum as labeled lines.
        for value in (report['board'], report['repository'], report['observed_at'],
                      str(report['contract']['version']), report['contract']['sha256'], report['contract']['source'],
                      report['outcome'], json.dumps(report['profile'], sort_keys=True),
                      json.dumps(report['actions'], sort_keys=True), json.dumps(report['refusals']),
                      f'Source status: {report["source_status"]}',
                      'Validation: ' + json.dumps(report['validation'])):
            if value not in rendered:
                errors.append('required report datum absent from channel readback')
        for row in report['workers']:
            if any(value not in rendered for value in row.values()):
                errors.append('worker datum absent from channel readback')
        if not report['workers'] and 'Workers: none' not in rendered:
            errors.append('empty workers not explicit')
        for key in ('received_at', 'applied_at'):
            try:
                at = report_timestamp(evidence[key])
                if at is None or not report_timestamp(report['observed_at']) <= at <= time.time():
                    raise ValueError()
            except (TypeError, ValueError):
                errors.append(f'invalid {key}')
    if not errors and report_timestamp(evidence['received_at']) > report_timestamp(evidence['applied_at']):
        errors.append('applied_at precedes received_at')
    if not errors:
        errors.extend(report['validation'])
    result = {'status': 'applied' if not errors else 'unknown', 'errors': errors,
              'contract': report_contract(), 'source': evidence.get('source'),
              'runtime': evidence.get('runtime'), 'session': evidence.get('session'),
              'qualification': 'caller-supplied channel readback; transport authenticity requires separate evidence'}
    print(json.dumps(result))
    if errors:
        raise SystemExit(1)


def profile_arguments(args):
    """Only this invocation's explicit flags, false, empty and zero included: the rest each worker reads itself."""
    flags = ' --filter ' + shlex.quote(args.filter) if args.filter is not None else ''
    flags += {True: ' --mine', False: ' --no-mine', None: ''}[args.mine]
    if args.limit:
        flags += ' --limit ' + ','.join(f'{name}={count}' for name, count in args.limit.items())
    return flags


def worker_prompt(args):
    return core.WORKER.replace(f'{core.TOOL} worker`', f'{core.TOOL} worker{profile_arguments(args)}`')


# The owner's card moves on GitHub's board the queue executes, by (label, Status): the command it runs.
BOARD_MOVES = {('ready', 'later'): 'later', ('waiting', 'later'): 'later', ('later', 'ready'): 'answer',
               ('later', 'waiting'): 'answer', ('review', 'ready'): 'reject'}


def board_fix(state, target, iid):
    if target == 'ready' and state in ('ask', 'doing'):
        return f'`{core.TOOL} {"answer" if state == "ask" else "release"} {iid} --text "<why>"`'
    if target == 'later' and state == 'ask' or target == 'ask' and state in ('ready', 'waiting', 'later'):
        return f'`{core.TOOL} {target} {iid} --text "<why>"`'
    return f'none: only a worker or `{core.TOOL} tick` moves a task from {state} to {target}'


def board_moves(everything, selected):
    """GitHub's board: Status is the owner's intent, the q-* label the queue's state. A move the queue can execute
    is executed with a note; any other goes back to the label's column (a manual ready<->waiting silently, as
    on GitLab) and is named. Returns how many moves ran and the lines for 'Board mismatch'."""
    cards, executed, misplaced, text = core.api('GET', 'board/items'), 0, [], 'moved on the board'
    for item in everything:
        iid, state = item['iid'], item['state']
        target = cards.get(iid, state)
        if iid not in selected or target == state or not (item := core.unchanged(item)):
            continue
        action = BOARD_MOVES.get((state, target))
        if action == 'later':
            core.save(item, 'later', 'later', text, waiting_for=text)
        elif action:
            core.requeue(argparse.Namespace(iid=iid, action=action, text=text))
        else:
            core.api('PUT', f'board/items/{iid}', {'status': state})
            if target and {state, target} != {'ready', 'waiting'}:
                misplaced.append(f'{core.ref(item)} was moved on the board from {state} to {target}: put back to {state}. Fix: {board_fix(state, target, iid)}')
            continue
        executed += 1
        print(f'Board move of {core.ref(item)} executed: {state} → {target}.')
    return executed, misplaced


def session_link(claim, agent=None):
    """#83: how the owner opens a worker session. Claude: its Remote Control https URL; without one the
    terminal command (background) or the app's id. Codex has no https form: docs/open.html redirects to its app link (#111)."""
    session = claim['session']
    if claim.get('runtime') == 'claude':
        url = core.claude_url(session)
        return f'[session]({url})' if url else f'`claude attach {agent["id"]}`' if agent else f'app session `local_{session}`'
    return f'[session]({core.PAGES.rstrip("/")}/open.html#codex://threads/{session})' if claim.get('runtime') == 'codex' else f'`{session}`'


def liveness(item, agents):
    """#43: (state, activity) of a worker session as this machine sees it. state: 'busy'; 'idle' (alive, its turn
    ended: nudge it); 'dead' (stopped: release a doing task now); None (no status here: another machine, no CLI,
    or a Codex task not in doing). A listed Claude job without pid is dead only in a terminal state; ponytail: an
    unlisted one is unknown, since `claude agents` failing also lists nothing; the 120-minute stale release covers it."""
    session, runtime = item['claim']['session'], item['claim'].get('runtime')
    if runtime == 'claude':
        agent = agents.get(session)
        if not agent:
            return None, f'issue {core.age(item)} min ago'
        # #176: `blocked` waits on the owner (a decision or an approval): busy with or without pid, never nudged or
        # released. No pid is dead only with a terminal state; any other state is unknown (stale release only).
        state = ('busy' if agent.get('state') == 'blocked' else
                 ('dead' if agent.get('state') in CLAUDE_ENDED else None) if not agent.get('pid') else
                 'busy' if agent.get('status') == 'busy' else 'idle')
        return state, f'{"running" if agent.get("pid") else "stopped"}, issue {core.age(item)} min ago'
    if runtime != 'codex' or item['state'] != 'doing':
        return None, f'issue {core.age(item)} min ago'
    try:
        codex = core.Codex()
        try:
            status, turns, last = core.codex_snapshot(codex, session, 1)
            item['_runtime_observation'] = core.codex_observation(codex, session, status, turns, last)
            if not item['_runtime_observation'].get('conflict'):  # #223: a conflict names the source that decided
                item['_runtime_observation']['source'] = 'codex app-server with existing rollout fallback'
        finally:
            codex.socket.close()
    except (OSError, SystemExit, ValueError) as error:
        return None, f'status unknown: {core.codex_line(error)}'
    # notLoaded with a running last turn: the app holds the session and works in it.
    working = bool(turns) and turns[0].get('app', False)
    state = ('busy' if working else 'idle' if status['type'] in ('idle', 'notLoaded')
             else 'dead' if status['type'] == 'systemError' else 'busy')
    observation = item['_runtime_observation']
    if observation['status'] in ('waiting_permission', 'active', 'unknown') and state != 'dead':
        state = 'busy'  # No positive terminal evidence: never nudge or release by age.
    return state, (f'{status["type"]}{" (turn running in the app)" if working else ""}, last event {core.codex_age(last)}; '
                   f'{observation["status"]}: {observation["exact_blocker"] or "typed runtime evidence"}')


def inbox_line(inbox):
    """Issues by non-collaborators, named so the manager sees them; taskq never acts on them. A collaborator makes
    one a task with `add` (a new task that links it)."""
    return f'Inbox: {len(inbox)} issues by non-collaborators ({", ".join(map(core.ref, sorted(inbox, key=lambda issue: issue["iid"])))})\n\n' if inbox else ''


NUDGE = 'Continue the assigned task; hand in result or ask the owner through taskq.'
# #42: the launchd timer's turn of the coordinator session, before the tick's output.
WAKE_PROMPT = ('taskq tick --act (the launchd timer) found what needs judgement; it already did the mechanical steps '
               '(spawn, retire, nudges). Do the coordinator pass by taskq-manager.md § 3 on the output below; do not run '
               '`taskq tick` again in this turn. Reply in the owner\'s language.\n\n')


def local(agent):
    """A `claude agents` job of this checkout: names are only a correlation, the cwd binds it to the project."""
    return bool(agent.get('cwd')) and Path(agent['cwd']).resolve() == core.ROOT.resolve()


def alive(agent):
    """Working or waiting on the owner (#176: blocked counts without pid; `status` is optional in the CLI's rows),
    or running in a state that is not terminal. A terminal state, or no pid without such a state, is not."""
    return agent.get('state') in ('working', 'blocked') or bool(agent.get('pid')) and agent.get('state') not in CLAUDE_ENDED


def busy(agent):
    return agent.get('state') in ('working', 'blocked') or agent.get('status') == 'busy'


def worker_iid(name):
    found = re.match(r'T(\d+) ', name or '')  # spawn names a worker `T<N> <title>`
    return found and int(found[1])


def codex_workers():
    """#185: (iid, thread id) of this checkout's live `T<N> ` Codex threads (active, idle or notLoaded; archived ones
    are not listed), from the read-only `thread/list` that cleanup and the archive pass read. None: unreachable."""
    if not core.CODEX_SOCKET.exists():
        return []  # no app server here: no Codex worker of this machine
    from taskq.cleanup import cleanup_codex
    root = core.ROOT.resolve()
    try:
        threads = cleanup_codex({root})
    except (OSError, SystemExit, ValueError):
        return None
    return [(iid, sid) for sid, thread in threads.items() if Path(thread.get('cwd') or '/').resolve() == root
            and (thread.get('status') or {}).get('type') != 'systemError' and (iid := worker_iid(thread.get('name')))]


def launched(iid, runtime):
    """#208: session ids of this checkout's live `T<iid>` workers of `runtime`, the evidence a reservation is matched
    and settled by; None when that inventory is unreadable or the runtime has none (a [runtimes] app)."""
    if runtime == 'claude':
        agents = core.claude_agents(strict=True)
        return None if agents is None else [sid for sid, agent in agents.items() if local(agent) and alive(agent) and worker_iid(agent.get('name')) == iid]
    if runtime == 'codex':
        threads = codex_workers()
        return None if threads is None else [sid for found, sid in threads if found == iid]
    return None


def starts(args, loaded, selected):
    """The tasks to start on this machine now, each with its runtime, within this machine's free places."""
    free, start = core.room(loaded[0], args.profile['limits']), []
    preferred = args.profile['preferred_runtime']
    ready = [item for item in core.startable(loaded=loaded) if item['iid'] in selected]
    # #185: a worker spawned by an earlier pass that has not taken its task yet (a manual TICK, a restart) is not
    # spawned again, and holds its runtime's place like a claim. An unreadable inventory holds what it could hide.
    agents, codex = (core.claude_agents(strict=True), codex_workers()) if ready else ({}, [])
    live = ([('claude', iid, sid) for sid, agent in (agents or {}).items() if local(agent) and alive(agent) and (iid := worker_iid(agent.get('name')))]
            + [('codex', iid, sid) for iid, sid in codex or []])
    # Exactly what room counted: a local claim of a doing task. A live worker of a review/ask task still holds a place.
    counted = {((item['claim'] or {}).get('runtime'), (item['claim'] or {}).get('session')) for item in loaded[0]
               if item['state'] == 'doing' and core.local_claim(item['claim'] or {})}
    reserved = {item['iid'] for item in loaded[0] if core.reserved_here(item)}  # #208: room counted its place
    for runtime, iid, session in live:
        if (runtime, session) not in counted and iid not in reserved and runtime in free:
            free[runtime] -= 1
    spawned = {iid for _, iid, _ in live}
    unknown = {'claude': agents is None, 'codex': codex is None}
    for item in ready:
        if item['iid'] in spawned:
            print(f'{core.ref(item)}: its worker is already running here, not taken yet; not started again.')
            continue
        hidden = [name for name, gone in unknown.items() if gone and item['runtime'] in (None, name)]
        if hidden:
            print(f'{core.ref(item)}: {" and ".join(hidden)} workers unreadable, one may be running before its take; not started this pass.')
            continue
        # The preferred runtime only breaks the tie for the user's own `any` task, and only while it has a free slot.
        own = item.get('assignees') == [args.profile['uid']] and preferred and free.get(preferred, 0) > 0
        who = item['runtime'] or (preferred if own else max(free, key=free.get))
        if free[who] > 0:
            free[who] -= 1
            start.append({**item, 'runtime': who})
    return start


def launch(args, start, act, step):
    """Start the workers: `act` spawns each itself (a step), else the commands for the coordinator."""
    if start and act:
        for item in start:
            step(f'spawn a {item["runtime"]} worker for {core.ref(item)}', lambda item=item: core.spawn(argparse.Namespace(
                runtime=item['runtime'], name=f'T{item["iid"]} {item["title"][:40]}', remote_control=True, text=worker_prompt(args),
                full_access=item['full_access'], limits=args.profile['limits'])), item=item)
    elif start:
        for item in start:
            core.record(args, 'spawn', status='proposed', task=item['iid'], runtime=item['runtime'])
        # One command per worker: the session starts on the prompt, no second message (#41).
        # An indented block, not inline code: the prompt itself holds backticks.
        print(f'## Start {len(start)} worker session(s)\n\n' + ''.join(f'- {core.ref(item)} {item["title"]}: {item["runtime"]}\n' for item in start)
              + '\nRun each command once; the worker starts on the brief at once:\n\n' + ''.join('    ' + shlex.join([core.TOOL, 'spawn', '--runtime', item['runtime'], '--name',
                                             f'T{item["iid"]} {item["title"][:40]}', '--text', worker_prompt(args)]
                                            + ['--codex-full-access'] * (item['full_access'] and item['runtime'] == 'codex')) + '\n'
                        for item in start))



def tick(args):
    if args.install_timer or args.uninstall_timer:
        return timer(args.install_timer)
    # Keep the inode: unlinking the file could let a third pass lock a different file.
    lock = core.TICK_BEAT.with_name('taskq-tick.lock')
    lock.parent.mkdir(parents=True, exist_ok=True)
    with lock.open('a+b') as held:
        try:
            if os.name == 'nt':
                import msvcrt
                held.seek(0)
                msvcrt.locking(held.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            if error.errno not in (errno.EACCES, errno.EAGAIN):
                raise
            core.record(args, 'tick', status='refused', reason='another tick pass is running')
            if hasattr(args, 'output'):
                args.output['refusals'].append('another tick pass is running')
            report = new_report(args)
            report['outcome'] = 'refused'
            report['refusals'] = ['another tick pass is running; do not repeat dispatch']
            emit_report(args, report)
            print('Skipped: another tick pass is running.', file=sys.stderr)
            return
        if args.act:
            return act(args)
        judgement = tick_pass(args)
        if hasattr(args, 'output') and judgement:
            args.output['outcome'] = 'judgement_needed'
            args.output['refusals'] = judgement



def act(args):
    """#42 `tick --act`: mechanical steps and a report every pass; exit 1/wake only for judgement.
    `--wake` (the launchd timer) also gives that output to the coordinator session as one turn."""
    with contextlib.redirect_stdout(io.StringIO()) as said:
        judgement = tick_pass(args, act=True)
    if hasattr(args, 'output') and judgement:
        args.output['outcome'] = 'failure' if any(event.get('status') == 'failed' for event in args.output['actions']) else 'judgement_needed'
        args.output['refusals'] = judgement
    print(said.getvalue(), end='')
    pending_set = getattr(args, 'pending', ())
    if args.wake and (judgement or pending_set):
        # #223: a shown question has no judgement line for a day, but it is pending: the bounded re-wake covers it.
        # Delivery is a fact; receipt and application are not read from it. The pending lines name what was sent.
        status = wake(said.getvalue(), judgement, pending_set)
        core.record(args, 'wake', status=status, acknowledged='unknown', pending=[line for _, line in pending_set])
    if not judgement:
        return
    sys.stdout.flush()
    sys.exit(1)


def woken():
    return core.TICK_BEAT.with_name('taskq-tick-woken')


def wake(output, judgement, pending=()):
    """One turn of the coordinator ([coordinator] session of taskq.local.toml) per new set of items: a review
    still open five minutes later wakes nobody again; a busy coordinator gets it on the next tick. #223: the key
    holds each item's pending line (state, claim, revision), so a resubmitted result or a second question is a new
    set; every item is re-read against its line immediately before the send; the same set is sent once more after
    TICK_LIVE_MINUTES to an idle coordinator (delivered is not applied), and nothing here reads an acknowledgement."""
    session = core.personal().get('coordinator', {}).get('session')
    if not session:
        print(f'\nNo coordinator to wake: no [coordinator] session in {core.LOCAL} (manager contract § 2).')
        return 'no coordinator'
    # #185: a handoff wakes the new owner. #223: a review, ask or stuck line of an item with a pending line is covered
    # by that finer line; a question leaving the list when shown is the same set, not a new one.
    covered = {str(item['iid']) for item, _ in pending}
    rest = [line for line in judgement if not (line.split()[0] in ('review', 'ask', 'stuck') and line.split()[1] in covered)]
    key = hashlib.sha256('\n'.join([session, *rest, *(line for _, line in pending)]).encode()).hexdigest()[:12]
    sent = woken().read_text().split() if woken().exists() else []
    again = bool(sent) and sent[0] == key
    if again and time.time() - float(sent[1] if len(sent) > 1 else 0) < TICK_LIVE_MINUTES * 60:
        print('\nThe coordinator was already woken for these items.')
        return 'already delivered'
    agents = core.claude_agents()
    if session not in agents:
        # Unlisted (`claude agents` failed, or not a background session here): a resume could run beside a busy PM.
        print(f'\nThe coordinator {session} is not in `claude agents` here: its state is unknown, not woken; '
              'the next tick tries again.')
        return 'coordinator unknown'
    name = agents[session].get('name')
    # #185: `claude --bg --resume` goes on under a new session id with the same name (#182): a busy or blocked
    # job of that name in this checkout is the coordinator too. Another checkout's job of the same name is not.
    if any(busy(agent) for sid, agent in agents.items() if sid == session or name and agent.get('name') == name and local(agent)):
        print('\nThe coordinator is busy: the next tick wakes it.')
        return 'coordinator busy'
    if moved := [item['iid'] for item, line in pending if not still_pending(item, line)]:
        print(f'\nNot woken: {", ".join(f"#{iid}" for iid in moved)} changed since this tick read it; the next tick reads again.')
        return 'pending changed'
    if again or not judgement:
        # A re-delivery, or a set with no judgement line (questions already shown): the pending items and their
        # notes, from the same read the lines were made of, so the turn has what it decides on.
        output += '\n## Pending, acknowledgement unknown\n\n' + ''.join(
            f'- {line}\n{core.data(item.get("_note", "none"))}' for item, line in pending)
    core.claude_wake(session, f'Project {core.PROJECT_PATH}, main checkout {core.ROOT}. ' + WAKE_PROMPT + output)
    woken().write_text(f'{key}\n{time.time():.0f}\n')
    print(f'\nWoke the coordinator {session}' + (' again: the same items are still pending; acknowledgement unknown.' if again else '.'))
    return 'delivered again' if again else 'delivered'


def timer(install):
    """#42: a launchd agent runs `tick --act --wake` from the main checkout every TICK_MINUTES; an optional
    macOS mode instead of the default in-session CronCreate timer (no LLM turn per fire, no app session, no 7-day limit)."""
    if sys.platform != 'darwin':
        return print('skipped: the launchd tick timer is macOS only; use the in-session timer (the default tick)')
    label = f'taskq.{core.ROOT.name}'
    plist, domain = Path.home() / 'Library/LaunchAgents' / f'{label}.plist', f'gui/{os.getuid()}'
    subprocess.run(['launchctl', 'bootout', f'{domain}/{label}'], capture_output=True)  # not loaded: nothing to do
    if not install:
        plist.unlink(missing_ok=True)
        return print(f'Removed the tick timer {label} ({plist}).')
    session = core.personal().get('coordinator', {}).get('session')
    if not session and (session := os.environ.get(core.RUNTIMES['claude'])):
        # Run inside the coordinator session: it records itself as the session the timer wakes.
        with core.LOCAL.open('a') as local:
            local.write(f'\n[coordinator]\nsession = {json.dumps(session)}\n')
    log = core.TICK_BEAT.with_name('taskq-tick.log')
    log.parent.mkdir(parents=True, exist_ok=True)
    plist.parent.mkdir(parents=True, exist_ok=True)
    # The shell's PATH finds git, gh/glab and claude; no token goes into the file (they come from the keychain).
    env = {key: os.environ[key] for key in ('PATH', 'TASKQ_HOST') if os.environ.get(key)}
    plist.write_bytes(plistlib.dumps({
        'Label': label, 'ProgramArguments': [sys.executable, '-m', 'taskq', 'tick', '--act', '--wake'],
        'WorkingDirectory': str(core.ROOT), 'StartInterval': TICK_MINUTES * 60, 'RunAtLoad': True,
        'EnvironmentVariables': env, 'StandardOutPath': str(log), 'StandardErrorPath': str(log)}))
    subprocess.run(['launchctl', 'bootstrap', domain, str(plist)], check=True)
    print(f'Installed the tick timer {label} ({plist}): `taskq tick --act --wake` every {TICK_MINUTES} min, log {log}.\n'
          + (f'It wakes the coordinator session {session}.' if session else
             f'No coordinator to wake: run this inside the coordinator session, or write [coordinator] session in {core.LOCAL}.')
          + '\nDelete an in-session CronCreate tick timer: one coordinator timer per checkout.')


def idle_ticks():
    return core.TICK_BEAT.with_name('taskq-tick-idle')


def idle_stop(act, step, failed):
    """#153: count this empty pass; on the [idle] stop-th in a row (taskq.local.toml, default 5, 0 = never) the line
    that stops the timer, and the count starts over. `act` (launchd) stops its own timer and runs cleanup here."""
    idle = core.personal().get('idle', {})
    from taskq.cleanup_schedule import settings
    # #197: [cleanup] enabled = false stops idle cleanup as well; [idle] cleanup = false stays the idle-only opt-out.
    stop, clean = idle.get('stop', 5), idle.get('cleanup', True) and settings()['enabled']
    count = int(idle_ticks().read_text()) + 1 if idle_ticks().exists() else 1
    if not stop or count < stop:
        idle_ticks().write_text(f'{count}\n')
        return None
    idle_ticks().unlink()
    cleanup = f'`{core.TOOL} cleanup --apply`'
    if not act:
        return (f'Idle {count} ticks: stop the timer (CronDelete / --uninstall-timer), ' + f'run {cleanup}, ' * clean
                + 'report to the owner; rearm with "arm the tick" (manager contract § 3).')
    if sys.platform == 'darwin':
        step('stop the tick timer', lambda: timer(False))
    if clean:
        try:
            # Under the #197 lock and state: skipped when the owner's tick already cleaned within the schedule.
            core.cleanup(argparse.Namespace(apply=True, trigger='idle'))  # its Remove and Ask the owner sections go to the coordinator
        except (SystemExit, OSError, subprocess.SubprocessError) as error:
            failed.append(f'run {cleanup}: {core.codex_line(str(error))}')
    stopped = 'the tick stopped its launchd timer' if sys.platform == 'darwin' else 'stop the external scheduler'
    return (f'\nIdle {count} ticks: {stopped}' + f' and ran {cleanup}' * clean
            + '; report to the owner; rearm with "arm the tick" (manager contract § 3).')


def retire_closed(log, args=None):
    """--act: a local Claude worker of a task closed in the last hour without this machine's `close` (closed on
    the board or by hand) is retired as `close` would. ponytail: sessions only; trees and branches: `cleanup`."""
    agents = core.claude_agents()
    if not agents:
        return
    after = time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(time.time() - 3600))
    for issue in core.issues(f'state=closed&updated_after={after}'):
        block = core.BLOCK.search(issue.get('description') or '')
        try:
            claim = (json.loads(block.group(1)) if block else {}).get('claim') or {}
        except (ValueError, AttributeError):
            continue
        agent = agents.get(claim.get('session'))
        if agent and agent.get('status') != 'busy' and claim.get('runtime') == 'claude' and core.local_claim(claim):
            core.claude_stop(claim['session'], remove=True)
            core.record(args, 'retire', status='done', task=issue['iid'], session=claim['session'])
            log(f'Retired {claim["session"]}: the worker of closed {core.ref(issue)}.')


def archive_finished_codex(tasks, log, args=None):
    """#165: every pass archives this checkout's Codex worker threads (`T<N> …`, as spawn names them) that are no
    open task's claim: the task closed, or went ask -> answer -> ready and a new session continues it. A thread the
    owner viewed is held by the app, and codex-archive has the app archive it (#165). Reversible (`thread/unarchive`), so no --act needed."""
    if not core.CODEX_SOCKET.exists():
        return
    from taskq.cleanup import cleanup_codex
    claimed, root = {(item['claim'] or {}).get('session') for item in tasks}, core.ROOT.resolve()
    try:
        threads = cleanup_codex({root})
    except (OSError, SystemExit, ValueError) as error:
        core.record(args, 'archive_inventory', status='refused', reason=str(error))
        return log(f'Codex threads not checked: {core.codex_line(str(error))[:120]}')
    for thread in threads.values():
        name, sid = thread.get('name') or '', thread['id']
        # 10 min: a worker spawned this pass may not have taken its task yet.
        # cleanup_codex also lists the app project's threads; only this checkout's are its workers.
        if (not re.match(r'T\d+ ', name) or Path(thread.get('cwd') or '/').resolve() != root or sid in claimed or (thread.get('status') or {}).get('type') not in ('idle', 'notLoaded')
                or time.time() - (thread.get('updatedAt') or time.time()) < 600):
            continue
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                core.codex_archive(argparse.Namespace(thread=sid))
            core.record(args, 'archive', status='done', session=sid)
            log(f'Archived {sid} ({name}): no open task holds it.')
        except (SystemExit, OSError) as error:
            core.record(args, 'archive', status='refused', session=sid, reason=str(error))
            log(f'Kept {sid} ({name}) for a later pass: {core.codex_line(str(error))[:120]}')


def new_report(args):
    contract = report_contract()
    host = core.HOST or ('github.com' if not core.BOARDS else None)
    repo = f'https://{host}/{core.PROJECT_PATH}' if host else 'unavailable'
    report = {'contract': contract, 'repository': repo, 'profile': {}, 'board': 'unavailable',
              'observed_at': datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z'), 'outcome': 'unknown',
              'actions': args.output['actions'] if hasattr(args, 'output') else [],
              'refusals': [], 'workers': [], 'source_status': 'unavailable', 'validation': []}
    args.pm_report = report
    return report


def tick_pass(args, act=False):
    report = new_report(args)
    try:
        judgement = queue_pass(args, act)
        report['outcome'] = ('failure' if any(event.get('status') == 'failed' for event in report['actions'])
                             else 'judgement_needed' if judgement else 'ok')
        report['refusals'] = judgement or []
        return judgement
    except (SystemExit, OSError, ValueError, subprocess.SubprocessError):
        report['outcome'] = 'failure'
        report['source_status'] = 'unavailable'
        raise
    finally:
        emit_report(args, report)


def emit_report(args, report):
    if hasattr(args, 'output'):
        report['actions'] = args.output['actions']
        if args.output['refusals']:
            report['refusals'] = args.output['refusals']
        args.output['report'] = report
    # Preserve the snapshot's conservative pass-start timestamp through long work and emission.
    report['validation'] = validate_report(report)
    print(render_report(report))
    if report['validation']:
        print('Report blockers: ' + '; '.join(report['validation']))
    print('Apply this version on this safe pass. Preserve claims/current work; do not repeat dispatch '
          'on duplicate delivery. Return version/hash plus the rendered report through the supported channel; '
          'taskq report-verify <readback.json> validates the acknowledgement. Unsupported contract: '
          'block obsolete publication, keep execution, run taskq update and retry next safe tick.')


def queue_pass(args, act=False):
    """One pass of the coordinator: release dead claims itself, then print exactly what to do. `act` (#42): also
    spawn, retire and nudge here instead of printing those steps. Returns the items that need judgement."""
    log = lambda line: print(line, file=sys.stderr)  # an act step: the timer's log, never the coordinator's turn
    failed = []

    def step(what, action, item=None):
        try:
            with contextlib.redirect_stdout(sys.stderr):
                result = action()
            core.record(args, what.split()[0], detail=what, status='done', task=(item or {}).get('iid'),
                        session=result if isinstance(result, str) else ((item or {}).get('claim') or {}).get('session'))
            log(f'Done: {what}.')
        except (SystemExit, OSError, ValueError, subprocess.SubprocessError) as error:
            failed.append(f'{what}: {core.codex_line(str(error))}')
            core.record(args, what.split()[0], detail=what, status='failed', reason=str(error), task=(item or {}).get('iid'))
    auto_update()
    print(f'taskq {core.version()}')
    if warning := clone_warning():
        print(warning)
    if not core.CODEX_SOCKET.exists():
        print(f'Codex workers unavailable on this machine: no Codex app server socket {core.CODEX_SOCKET}; '
              f'start it: `{core.CODEX_HEADLESS}`.')
    tick_beat()
    contract_news(args.prompt_version)
    loaded, candidates = core.profile(args)
    args.pm_report['profile'] = args.profile
    if issue := next((item for item in candidates if item.get('web_url')), None):
        args.pm_report['repository'] = re.split(r'/(?:-/)?issues/', issue['web_url'])[0]
    args.pm_report['source_status'] = 'available'
    board = None
    try:
        if core.BOARDS:
            board = next((board for board in core.api('GET', 'boards') if board['name'] == core.BOARD), None)
            if board and args.pm_report['repository'] != 'unavailable':
                args.pm_report['board'] = f'{args.pm_report["repository"]}/-/boards/{board["id"]}'
        else:
            board = core.api('GET', 'board')
            if board:
                args.pm_report['board'] = board['url']
    except (SystemExit, OSError, ValueError, subprocess.SubprocessError) as error:
        core.record(args, 'report_source', status='unknown', reason=f'board unavailable: {error}')
    selected = {item['iid'] for item in candidates}
    if hasattr(args, 'output'):
        args.output['profile'] = args.profile
        args.output['tasks'] = [{'id': item['iid'], 'state': item['state'], 'claim': item['claim']} for item in candidates]
        args.output['sessions'] = [item['claim']['session'] for item in candidates if (item['claim'] or {}).get('session')]
    # #145: only the coordinator machine ([coordinator] machine of taskq.toml; none set: every machine) starts shared
    # workers, accepts reviews and shows questions; another machine releases its own work and starts its host-* tasks.
    holder = core.COORDINATOR in (None, core.machine())
    # #43: a session seen here decides at once: dead is released now, alive (busy or idle) never by age.
    doing = [item for item in loaded[0] if item['iid'] in selected and item['state'] == 'doing' and (item['claim'] or {}).get('session')]
    agents = core.claude_agents() if any(item['claim'].get('runtime') == 'claude' for item in loaded[0]
                                         if item['iid'] in selected and (item['claim'] or {}).get('session')) else {}
    alive = {item['iid']: liveness(item, agents) for item in doing}
    for item in candidates:
        if item['state'] in ('doing', 'ask', 'review'):
            item['_report_activity'] = alive.get(item['iid'], (None, f'issue {core.age(item)} min ago'))[1]
    args.pm_report['workers'] = [report_row(item, agents) for item in candidates
                                 if item['state'] in ('doing', 'ask', 'review')]
    dead = [item for item in doing if alive[item['iid']][0] == 'dead' and not item.get('result')]
    stalled = [item for item in loaded[0] if item['iid'] in selected and item['state'] == 'doing' and alive.get(item['iid'], (None,))[0] is None
               and core.age(item) > core.STALE_MINUTES and (holder or core.local_claim(item['claim'] or {}))]
    for item in dead + stalled:
        if not core.unchanged(item):
            continue
        why = f'its {item["claim"]["runtime"]} session {item["claim"]["session"]} has stopped' if item in dead else \
            f'no change on the issue for {core.age(item)} minutes'
        args.iid, args.action, args.text = item['iid'], 'release', why
        core.requeue(args)
        core.record(args, 'release', task=item['iid'], reason=why)
        print(f'Released {"dead" if item in dead else "stalled"} {core.ref(item)}.')
    # #208: reservations found again (restart, a launch that died) are settled by evidence on their machine, never by age.
    released = [item for item in loaded[0] if item['state'] == 'ready' and item.get('reservation') and core.reconcile(item, args)]
    stalled = dead + stalled + released
    if not holder:
        # A task pinned to this machine (`host-<name>`) starts only here: the coordinator elsewhere cannot start it.
        start = starts(args, core.load() if stalled else loaded, {item['iid'] for item in candidates if item.get('host') == core.machine()})
        if not start:
            return print(f'coordinator is {core.COORDINATOR}: this tick released only its own stalled work. Say so and stop.')
        print(f'coordinator is {core.COORDINATOR}: start only the workers pinned to this machine.\n')
        launch(args, start, act, step)
        return failed
    loaded = core.load() if stalled else loaded
    # A lock on a task nobody holds: a take that died between the lock and the move, or a card moved by hand.
    held = {item['iid'] for item in loaded[0] if item['state'] not in ('ready', 'waiting') or item.get('reservation')}  # #208: no TTL steal
    for issue in core.issues(f'state=opened&my_reaction_emoji={core.LOCK}'):
        if issue['iid'] in selected and issue['iid'] not in held and all(time.time() - core.stamp(item['created_at']) > core.LOCK_SECONDS for item in core.locks(issue['iid'])):
            core.unlock(issue['iid'])
            core.record(args, 'unlock', task=issue['iid'])
            print(f'Unlocked {core.ref(issue)}: nobody holds it.')
    misplaced = []
    if not core.BOARDS and board:
        executed, misplaced = board_moves(loaded[0], selected)
        loaded = core.load() if executed else loaded
    # Only tick moves ready<->waiting: a card a hand moved between them goes back here.
    moved = 0
    for item in loaded[0]:
        if item['iid'] not in selected:
            continue
        open_deps = sorted(set(item['deps']) & loaded[1])
        if (item['state'], bool(open_deps)) not in (('ready', True), ('waiting', False)) or not (item := core.unchanged(item)):
            continue
        if item['state'] == 'ready':
            core.save(item, 'waiting', 'waiting', f'open dependencies {open_deps}')
        else:
            core.save(item, 'ready', 'ready', 'dependencies closed')
        core.record(args, 'move', task=item['iid'], state='ready' if item['state'] == 'waiting' else 'waiting')
        moved += 1
        print(f'Moved {core.ref(item)} {item["state"]} → {"ready" if item["state"] == "waiting" else "waiting"}.')
    everything, _, odd, problems, inbox = loaded = core.load() if moved else loaded
    everything = [item for item in everything if item['iid'] in selected]
    review = [item for item in everything if item['state'] == 'review' and item['result']]
    # A question reaches the owner once, when it is new; the ones already shown come back as a daily summary.
    asked = [(item, *question(item['iid'])) for item in everything if item['state'] == 'ask']
    fresh = [(item, text) for item, text, shown, _ in asked if shown is None]
    summary = [(item, text) for item, text, shown, _ in asked if shown and time.time() - shown >= core.SUMMARY_SECONDS]
    codex_stopped = [item for item in everything if item['state'] in ('ask', 'later')
                     and (item['claim'] or {}).get('runtime') == 'codex' and not core.codex_is_archived(item['claim']['session'])]
    # A card moved by hand on the board into a state its data does not support.
    odd = [f'{core.ref(issue)} labels {issue["labels"]}: give it exactly one state label' for issue in odd] + [
        f'{core.ref(item)} is doing without a worker: move it back to ready or `release {item["iid"]}`'
        for item in everything if item['state'] == 'doing' and not (item['claim'] or {}).get('session')] + [
        f'{core.ref(item)} is in review without a result: `reject {item["iid"]}` or close it by hand'
        for item in everything if item['state'] == 'review' and not item['result']] + misplaced
    start = starts(args, loaded, selected)
    if act:
        retire_closed(log, args)
    archive_finished_codex(loaded[0], log, args)
    # #197: the owner's tick (this machine coordinates) applies native cleanup when due; never a timer of its own.
    sys.modules['taskq.cleanup'].scheduled(args)  # the module: `core.cleanup` is the command function
    # #223: one read per item gives its pending line and the note it names; what is printed below comes from that
    # same read. A shown question is pending without a judgement line, so the lines are made before the idle return.
    args.pending = [(item, pending(item)) for item in review + [item for item, *_ in asked]]
    if not (review or fresh or summary or start or codex_stopped or odd or problems) and not any(item['state'] == 'doing' for item in everything):
        # #153: an ask or review task waits for someone, so it is no idle pass.
        if any(item['state'] in ('ask', 'review') for item in everything):
            idle_ticks().unlink(missing_ok=True)
        elif stop := idle_stop(act, step, failed):
            print(inbox_line(inbox) + stop)
            return ['idle stop'] + [f'inbox {issue["iid"]}' for issue in inbox] + failed
        print(inbox_line(inbox) + 'Nothing to do. Say so and stop.')
        return [f'inbox {issue["iid"]}' for issue in inbox]
    idle_ticks().unlink(missing_ok=True)
    print(f'You are the coordinator of the task queue for this one pass. Queue tool: `{core.TOOL}`\n')
    print(inbox_line(inbox), end='')
    # #83: one table of every worker; the owner's chat opens only http(s) links.
    workers = [item for item in everything if item['state'] in ('doing', 'ask', 'review') and (item['claim'] or {}).get('session')]
    idle, claude_idle, stuck = [], [], []
    for item in workers:
        session, runtime = item['claim']['session'], item['claim'].get('runtime')
        state, activity = alive.get(item['iid']) or liveness(item, agents)
        item['_report_activity'] = activity
        found = pending(item) if item['state'] == 'doing' and not item.get('result') and state != 'busy' else ''
        if found.split(' ')[3:4] == ['problem']:
            # #223: the worker ended its turn on a `problem` note, no result or ask: judgement, never a nudge or a
            # release; its claim stays. Unknown (another machine, no CLI) is listed too: a listing has no effect.
            stuck.append(item)
        elif state == 'idle' and item['state'] == 'doing' and not item.get('result'):
            (claude_idle if runtime == 'claude' else idle).append(item)
    args.pm_report['workers'] = [report_row(item, agents) for item in everything if item['state'] in ('doing', 'ask', 'review')]
    permissions = [item['_runtime_observation'] for item in workers if item.get('_runtime_observation', {}).get('status') == 'waiting_permission']
    for item in workers:
        if observation := item.get('_runtime_observation'):
            core.record(args, 'runtime_observation', task=item['iid'], **observation)
    if permissions:
        print('## Runtime permissions\n\nThe owner approves in the linked runtime UI. Keep the same worker; do not answer, nudge or spawn another.\n')
        for observation in permissions:
            print(f'- [{observation["session"]}]({observation["session_link"]}): {observation["exact_blocker"]}')
    if idle and act:
        for item in idle:
            step(f'nudge idle Codex {core.ref(item)}', lambda item=item: core.codex_send(
                argparse.Namespace(thread=item['claim']['session'], text=NUDGE, full_access=item['full_access'])), item=item)
    elif idle:
        print('## Codex idle\n\nTask is doing without result/ask, but its session has stopped. Intervene now:\n')
        for item in idle:
            print(f'- {core.ref(item)}: `{core.TOOL} codex-send {item["claim"]["session"]} '
                  f'--text "{NUDGE}"`')
        print()
    if claude_idle and act:
        for item in claude_idle:
            step(f'nudge idle Claude {core.ref(item)}', lambda item=item: core.claude_wake(item['claim']['session'], NUDGE), item=item)
    elif claude_idle:
        print('## Claude idle\n\nTask is doing without result/ask, but its session has ended its turn. Intervene now:\n')
        for item in claude_idle:
            print(f'- {core.ref(item)}: `claude --bg --resume {item["claim"]["session"]} "{NUDGE}"`')
        print()
    # An app without a status API: silence on the issue is the only sign its turn ended without a hand-in.
    quiet = [item for item in everything if item['state'] == 'doing' and (item['claim'] or {}).get('runtime') in core.EXECUTORS
             and not item.get('result') and item not in stuck and core.QUIET_MINUTES <= core.age(item) < core.QUIET_MINUTES + 5]
    if quiet and act:
        for item in quiet:
            step(f'nudge quiet {core.ref(item)}', lambda item=item: core.executor_run(
                item['claim']['runtime'], 'send', session=item['claim']['session'], text=NUDGE), item=item)
    elif quiet:
        print(f'## Quiet workers\n\nNo change on the issue for {core.QUIET_MINUTES} minutes. Nudge each (this tick only):\n')
        for item in quiet:
            print(f'- {core.ref(item)}: `{core.TOOL} send --runtime {item["claim"]["runtime"]} {item["claim"]["session"]} '
                  f'--text "{NUDGE}"`')
        print()
    if stuck:
        print('## Worker problems\n\nTask is doing; its worker ended its turn on a problem note, no result or ask. Its claim stays: '
              'send the next step to that same session (Claude: `claude --bg --resume <session> "<next step>"`; Codex: '
              f'`{core.TOOL} codex-send <session> --text "<next step>"`), or `{core.TOOL} release <N>` only for a stopped session. '
              'Do not answer: the task is not in ask.\n')
        for item in stuck:
            print(f'- {core.ref(item)} {item["claim"]["runtime"]} {item["claim"]["session"]}:\n' + core.data(item['_note'].split('\n\n', 1)[-1]))
    if odd:
        print('## Board mismatch\n\nThese issues are not in a state taskq can run. Fix each:\n')
        print(core.data(''.join(f'- {line}\n' for line in odd).rstrip()))
    if problems:
        print('## Problems without a task\n\nRead each. Fix it now if small, else `add` a task for it; then close the '
              'issue with a note of what was done.\n')
        print(core.data(''.join(f'- {core.ref(issue)} {issue["title"]}\n' for issue in problems).rstrip()))
    for item in review:
        sha = item['result'].get('sha')
        # #223: the hand-in is the note the pending line names, from the same read: never a newer note under an old line.
        print(f'## Review {core.ref(item)}: {item["title"]}\n\n{item["text"]}\n\nHanded in:\n\n{core.data(item["_note"])}\n'
              + (f'Commit: [{sha}]({core.commit_url(item, sha)})\n' if sha and item.get('web_url') else '') +
              f'Check the result against the Acceptance above (for code and docs read the commit).\n'
              f'Accepted: `{core.TOOL} close {item["iid"]} --text "<what you checked>"`. '
              f'Not accepted: `{core.TOOL} reject {item["iid"]} --text "<what to fix>"`.\n')
        if (item['claim'] or {}).get('runtime') == 'codex' and not core.local_claim(item['claim']):
            print(f'After close, archive its Codex session on its machine: `{core.TOOL} codex-archive {item["claim"]["session"]}`.\n')
    launch(args, start, act, step)
    if fresh:
        print('## Waiting for the owner\n\nNew questions. Do not answer these yourself. End your reply with this list, verbatim:\n')
        print(core.data('\n'.join(f'- {core.ref(item)} {item["title"]}: {text}' for item, text in fresh)))
        for item, _ in fresh:
            if core.unchanged(item):
                core.note(item['iid'], 'shown')
    if summary:
        print('## Still waiting for the owner (daily summary)\n\nEnd your reply with this list, verbatim:\n')
        print(core.data('\n'.join(f'- {core.ref(item)} {item["title"]}: {text.splitlines()[0] if text else "no note"}'
                          for item, text in summary)))
        for item, _ in summary:
            if core.unchanged(item):
                core.note(item['iid'], 'shown')
    if fresh or summary:
        print(f'\nThe owner answers with: `{core.TOOL} answer <N> --text "<answer>"`.')
    if codex_stopped and act:
        for item in codex_stopped:
            step(f'archive stopped Codex {core.ref(item)}', lambda item=item: core.codex_archive(
                argparse.Namespace(thread=item['claim']['session'])), item=item)
    elif codex_stopped:
        print('\n## Archive stopped Codex workers\n\nTasks in ask or later continue in a new session after answer; archive when idle:\n')
        for item in codex_stopped:
            print(f'- {core.ref(item)}: `{core.TOOL} codex-archive {item["claim"]["session"]}`')
    if failed:
        print('\n## Steps that failed\n\nThe tick could not do these itself; do each by hand (§ 3) or tell the owner:\n')
        print(core.data(''.join(f'- {line}\n' for line in failed).rstrip()))
    # #223: every unresolved review, question (shown or not) and worker problem is pending, with its revision; the
    # judgement lines below stay the coordinator's list and the report's refusals.
    args.pending += [(item, pending(item)) for item in stuck]
    return ([f'review {item["iid"]} {item["result"].get("sha")}' for item in review] + [f'ask {item["iid"]}' for item, _ in fresh + summary]
            + [f'stuck {item["iid"]} {pending(item).split(" ")[4]}' for item in stuck]
            + [observation['notify_dedup'] for observation in permissions]
            + odd + [f'problem {issue["iid"]}' for issue in problems] + [f'inbox {issue["iid"]}' for issue in inbox] + failed)
