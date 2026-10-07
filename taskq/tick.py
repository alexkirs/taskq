"""`tick`: the coordinator's pass over the queue, board moves, the Workers table, the beat stamp."""
import argparse
import contextlib
import fcntl
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
    """The latest question of an `ask` task and when `tick` last showed it (None: not yet). The newest page
    is enough: while a task waits in ask, only `shown` notes follow its question."""
    shown = None
    for item in core.collaborators(core.api('GET', f'issues/{iid}/notes?sort=desc&per_page=100&activity_filter=only_comments')):
        if item['body'].startswith('**shown**') and shown is None:
            shown = core.stamp(item['created_at'])
        elif item['body'].startswith('**ask**'):
            return item['body'].split('\n\n', 1)[-1], shown
    return 'no question note', shown


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


TICK_PROMPT_VERSION = 2  # raise with every change of TICK_PROMPT: an older --prompt-version gets the re-arm line
# The coordinator timer's prompt, word for word as in manager contract § 2 (a test keeps them equal).
TICK_PROMPT = f"""taskq tick prompt v{TICK_PROMPT_VERSION}. Run `cd <main checkout> && taskq update; taskq tick --prompt-version {TICK_PROMPT_VERSION}`
and do the coordinator pass by taskq-manager.md § 3 (`taskq contract` prints its path). Reply in the owner's language,
one or two lines when nothing changed."""


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
    or a Codex task not in doing). A listed Claude job without pid is dead; ponytail: an unlisted one is unknown,
    since `claude agents` failing also lists nothing; the 120-minute stale release covers it."""
    session, runtime = item['claim']['session'], item['claim'].get('runtime')
    if runtime == 'claude':
        agent = agents.get(session)
        if not agent:
            return None, f'issue {core.age(item)} min ago'
        state = 'dead' if not agent.get('pid') else 'busy' if agent.get('status') == 'busy' else 'idle'
        return state, f'{"running" if agent.get("pid") else "stopped"}, issue {core.age(item)} min ago'
    if runtime != 'codex' or item['state'] != 'doing':
        return None, f'issue {core.age(item)} min ago'
    try:
        codex = core.Codex()
        try:
            status, turns, last = core.codex_snapshot(codex, session, 1)
        finally:
            codex.socket.close()
    except (OSError, SystemExit, ValueError) as error:
        return None, f'status unknown: {core.codex_line(error)}'
    # notLoaded with a running last turn: the app holds the session and works in it.
    working = bool(turns) and turns[0].get('app', False)
    state = ('busy' if working else 'idle' if status['type'] in ('idle', 'notLoaded')
             else 'dead' if status['type'] == 'systemError' else 'busy')
    return state, f'{status["type"]}{" (turn running in the app)" if working else ""}, last event {core.codex_age(last)}'


def inbox_line(inbox):
    """Issues by non-collaborators, named so the manager sees them; taskq never acts on them. A collaborator makes
    one a task with `add` (a new task that links it)."""
    return f'Inbox: {len(inbox)} issues by non-collaborators ({", ".join(map(core.ref, sorted(inbox, key=lambda issue: issue["iid"])))})\n\n' if inbox else ''


NUDGE = 'Continue the assigned task; hand in result or ask the owner through taskq.'
# #42: the launchd timer's turn of the coordinator session, before the tick's output.
WAKE_PROMPT = ('taskq tick --act (the launchd timer) found what needs judgement; it already did the mechanical steps '
               '(spawn, retire, nudges). Do the coordinator pass by taskq-manager.md § 3 on the output below; do not run '
               '`taskq tick` again in this turn. Reply in the owner\'s language.\n\n')


def starts(args, loaded, selected):
    """The tasks to start on this machine now, each with its runtime, within this machine's free places."""
    free, start = core.room(loaded[0], args.profile['limits']), []
    preferred = args.profile['preferred_runtime']
    for item in core.startable(loaded=loaded):
        if item['iid'] not in selected:
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
                full_access=item['full_access'])), item=item)
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
    # Keep the inode: unlinking a flock file could let a third pass lock a different file.
    lock = core.TICK_BEAT.with_name('taskq-tick.lock')
    lock.parent.mkdir(parents=True, exist_ok=True)
    with lock.open('a') as held:
        try:
            fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            core.record(args, 'tick', status='refused', reason='another tick pass is running')
            if hasattr(args, 'output'):
                args.output['refusals'].append('another tick pass is running')
            print('Skipped: another tick pass is running.', file=sys.stderr)
            return
        if args.act:
            return act(args)
        judgement = tick_pass(args)
        if hasattr(args, 'output') and judgement:
            args.output['outcome'] = 'judgement_needed'
            args.output['refusals'] = judgement



def act(args):
    """#42 `tick --act`: the mechanical steps done here, stdout only for what needs judgement; exit 1 then.
    `--wake` (the launchd timer) also gives that output to the coordinator session as one turn."""
    with contextlib.redirect_stdout(io.StringIO()) as said:
        judgement = tick_pass(args, act=True)
    if hasattr(args, 'output') and judgement:
        args.output['outcome'] = 'failure' if any(event.get('status') == 'failed' for event in args.output['actions']) else 'judgement_needed'
        args.output['refusals'] = judgement
    if not judgement:
        return
    print(said.getvalue(), end='')
    if args.wake:
        wake(said.getvalue(), judgement)
    sys.stdout.flush()
    sys.exit(1)


def woken():
    return core.TICK_BEAT.with_name('taskq-tick-woken')


def wake(output, judgement):
    """One turn of the coordinator ([coordinator] session of taskq.local.toml) per new set of items: a review
    still open five minutes later wakes nobody again; a busy coordinator gets it on the next tick."""
    session = core.personal().get('coordinator', {}).get('session')
    if not session:
        return print(f'\nNo coordinator to wake: no [coordinator] session in {core.LOCAL} (manager contract § 2).')
    key = hashlib.sha256('\n'.join(judgement).encode()).hexdigest()[:12]
    if woken().exists() and woken().read_text().strip() == key:
        return print('\nThe coordinator was already woken for these items.')
    if (core.claude_agents().get(session) or {}).get('status') == 'busy':
        return print('\nThe coordinator is busy: the next tick wakes it.')
    core.claude_wake(session, WAKE_PROMPT + output)
    woken().write_text(key + '\n')
    print(f'\nWoke the coordinator {session}.')


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
    stop, clean = idle.get('stop', 5), idle.get('cleanup', True)
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
            core.cleanup(argparse.Namespace(apply=True))  # its Remove and Ask the owner sections go to the coordinator
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


def tick_pass(args, act=False):
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
    stalled = dead + stalled
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
    held = {item['iid'] for item in loaded[0] if item['state'] not in ('ready', 'waiting')}
    for issue in core.issues(f'state=opened&my_reaction_emoji={core.LOCK}'):
        if issue['iid'] in selected and issue['iid'] not in held and all(time.time() - core.stamp(item['created_at']) > core.LOCK_SECONDS for item in core.locks(issue['iid'])):
            core.unlock(issue['iid'])
            core.record(args, 'unlock', task=issue['iid'])
            print(f'Unlocked {core.ref(issue)}: nobody holds it.')
    misplaced = []
    if not core.BOARDS and core.api('GET', 'board'):
        print(f'Board: {core.api("GET", "board")["url"]}')
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
    fresh = [(item, text) for item, text, shown in asked if shown is None]
    summary = [(item, text) for item, text, shown in asked if shown and time.time() - shown >= core.SUMMARY_SECONDS]
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
    idle, claude_idle, rows = [], [], []
    for item in workers:
        session, runtime = item['claim']['session'], item['claim'].get('runtime')
        state, activity = alive.get(item['iid']) or liveness(item, agents)
        if state == 'idle' and item['state'] == 'doing' and not item.get('result'):
            (claude_idle if runtime == 'claude' else idle).append(item)
        rows.append(f'| {core.ref(item)} {item["title"][:40].replace("|", "/")} | {item["state"]} | {runtime}{core.where(item["claim"])} '
                    f'| {session_link(item["claim"], agents.get(session))} | {activity} |')
    if rows:
        print('## Workers\n\nShow the owner this table as printed; every link opens in a browser:\n\n'
              '| Task | State | Runtime | Session | Last activity |\n|---|---|---|---|---|\n' + '\n'.join(rows) + '\n')
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
             and not item.get('result') and core.QUIET_MINUTES <= core.age(item) < core.QUIET_MINUTES + 5]
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
    if odd:
        print('## Board mismatch\n\nThese issues are not in a state taskq can run. Fix each:\n')
        print(core.data(''.join(f'- {line}\n' for line in odd).rstrip()))
    if problems:
        print('## Problems without a task\n\nRead each. Fix it now if small, else `add` a task for it; then close the '
              'issue with a note of what was done.\n')
        print(core.data(''.join(f'- {core.ref(issue)} {issue["title"]}\n' for issue in problems).rstrip()))
    for item in review:
        sha = item['result'].get('sha')
        print(f'## Review {core.ref(item)}: {item["title"]}\n\n{item["text"]}\n\nHanded in:\n\n{core.data(core.handed_in(item))}\n'
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
    return ([f'review {item["iid"]} {item["result"].get("sha")}' for item in review] + [f'ask {item["iid"]}' for item, _ in fresh + summary]
            + odd + [f'problem {issue["iid"]}' for issue in problems] + [f'inbox {issue["iid"]}' for issue in inbox] + failed)
