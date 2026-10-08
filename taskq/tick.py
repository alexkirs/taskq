"""`tick`: the coordinator's pass over the queue, its mechanical steps and the R6 report."""
import argparse
import contextlib
import errno
import hashlib
import io
import os
from pathlib import Path
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


def contract_news():
    """Name a package or coordinator-contract change once per checkout before the pass."""
    seen = core.TICK_BEAT.with_name('taskq-contract-seen')
    files = (core.CONTRACTS / 'principles.md', core.CONTRACTS / 'taskq-manager.md')
    new = hashlib.sha256(core.version().encode() + b''.join(path.read_bytes() for path in files)).hexdigest()[:7]
    old = seen.read_text().strip() if seen.exists() else ''
    if old != new:
        seen.parent.mkdir(parents=True, exist_ok=True)
        seen.write_text(new + '\n')
        print(f'TaskQ package or contracts changed since your last tick ({old or "none"}→{new}): re-read '
              f'{files[0]} and {files[1]} before this pass.')


def question(iid):
    """The latest question of an `ask` task and when `tick` last showed it (None: not yet). The newest page is
    enough: while a task waits in ask, only `shown` notes follow its question."""
    shown, text = None, 'no question note'
    notes = core.collaborators(core.api('GET', f'issues/{iid}/notes?sort=desc&per_page=100&activity_filter=only_comments'))
    for item in notes:
        if item['body'].startswith('**shown**') and shown is None:
            shown = core.stamp(item['created_at'])
        elif item['body'].startswith('**ask**'):
            text = item['body'].split('\n\n', 1)[-1]
            break
    return text, shown


def handed_in(item, kinds):
    """The newest trusted note of the claim session among `kinds`: what the worker handed in, or None."""
    claim = item['claim'] or {}
    who = f'{claim.get("runtime")}:{(claim.get("session") or "")[:8]}'
    return next((note['body'] for note in reversed(core.comments(item['iid']))
                 if note['body'].startswith(tuple(f'**{kind}** · {who}' for kind in kinds))), None)


def listed(item):
    """A row of the R6 table: a task with a worker's state, or with a supervisor (#243: its slot is taken)."""
    return item['state'] in ('doing', 'ask', 'review') or bool(item.get('supervisor'))


def report_row(item, agents, activity):
    """One row of the R6 table: the same columns on every runtime, the session links built per runtime
    (`session_link`): the worker's, then its supervisor's (#243)."""
    claim, found = item['claim'] or {}, item.get('supervisor') or {}
    links = ([session_link(claim, agents.get(claim['session']))] if claim.get('session') else []) + (
        ['supervisor ' + session_link(found, agents.get(found['session']))] if found else [])
    runtime = claim.get('runtime') or found.get('runtime') or 'unknown'
    title = ' '.join(item['title'].replace('|', '/').split())
    return f'| {core.ref(item)} {title} | {item["state"]} ({activity}) | {runtime}{core.where(claim)} | {" · ".join(links) or "unavailable"} |'


def report(board, rows):
    """R6: the project heading, the Board link and one table Task | Status | Runtime | Session."""
    print(f'## {core.PROJECT_PATH}\n\nBoard: {board}\n\n| Task | Status | Runtime | Session |\n|---|---|---|---|')
    print('\n'.join(rows) or '| none | | | |', end='\n\n')


def profile_arguments(args):
    """Only this invocation's explicit flags, false, empty and zero included: the rest each worker reads itself."""
    flags = ' --filter ' + shlex.quote(args.filter) if args.filter is not None else ''
    flags += {True: ' --mine', False: ' --no-mine', None: ''}[args.mine]
    if args.limit:
        flags += ' --limit ' + ','.join(f'{name}={count}' for name, count in args.limit.items())
    return flags


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


# #240: a resumed worker continues its own claim; `taskq worker` gives such a session its own task's brief.
NUDGE = ('Continue the task you claimed (taskq worker prints its brief); do not take another task; '
         'hand in result or ask the owner through taskq.')
# #243: the line that wakes a task's supervisor; no backticks: it goes inside a double-quoted shell argument.
SUPERVISE = 'Continue as the supervisor of task #{iid}: run taskq supervise {iid} and do what it prints for its state.'


def wake_supervisor(item):
    found, text = item['supervisor'], SUPERVISE.format(iid=item['iid'])
    if found['runtime'] == 'claude':
        return core.claude_wake(found['session'], text)
    if found['runtime'] == 'codex':
        # a supervisor reaches the app server to drive its worker: workspace-write denies that socket
        return core.codex_send(argparse.Namespace(thread=found['session'], text=text, full_access=True))
    return core.executor_run(found['runtime'], 'send', session=found['session'], text=text)



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


def supervisor_iid(name):
    found = re.match(r'S(\d+) ', name or '')  # #243: the tick names a task's supervisor `S<N> <title>`
    return found and int(found[1])


def session_iid(name):
    """#243: the task of a worker or supervisor session: one slot holds both."""
    return worker_iid(name) or supervisor_iid(name)


def codex_workers(match=worker_iid):
    """#185: (iid, thread id) of this checkout's live `T<N> ` Codex threads (active, idle or notLoaded; archived ones
    are not listed), from the read-only `thread/list` that cleanup and the archive pass read. None: unreachable.
    `match` session_iid: supervisors' `S<N> ` threads too."""
    if not core.CODEX_SOCKET.exists():
        return []  # no app server here: no Codex worker of this machine
    from taskq.cleanup import cleanup_codex
    root = core.ROOT.resolve()
    try:
        threads = cleanup_codex({root})
    except (OSError, SystemExit, ValueError):
        return None
    return [(iid, sid) for sid, thread in threads.items() if Path(thread.get('cwd') or '/').resolve() == root
            and (thread.get('status') or {}).get('type') != 'systemError' and (iid := match(thread.get('name')))]


def launched(iid, runtime):
    """Session ids of this checkout's live `T<iid>` workers of `runtime`, a supervised worker's binding before its
    launch note lands; None when that inventory is unreadable or the runtime has none (a [runtimes] app)."""
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
    # #240, #243: a supervised task's worker is launched by its supervisor only; this tick starts supervisors.
    for item in ready:
        if item.get('supervisor'):
            print(f'{core.ref(item)}: supervised by {core.short(item["supervisor"])}; that session launches its worker, not this tick.')
    ready = [item for item in ready if not item.get('supervisor')]
    # #185: a worker spawned by an earlier pass that has not taken its task yet (a manual TICK, a restart) is not
    # spawned again, and holds its runtime's place like a claim. An unreadable inventory holds what it could hide.
    # #243: the same for a supervisor (`S<N>`): a task's live supervisor and worker hold one slot together.
    agents, codex = (core.claude_agents(strict=True), codex_workers(session_iid)) if ready else ({}, [])
    live = ([('claude', iid, sid) for sid, agent in (agents or {}).items() if local(agent) and alive(agent) and (iid := session_iid(agent.get('name')))]
            + [('codex', iid, sid) for iid, sid in codex or []])
    # Exactly what room counted: a local claim of a doing task. A live session of a ready/review/ask task still holds a place.
    counted = {item['iid'] for item in loaded[0] if item['state'] == 'doing' and core.local_claim(item['claim'] or {})}
    for iid, runtime in {iid: runtime for runtime, iid, _ in live}.items():
        if iid not in counted and runtime in free:
            free[runtime] -= 1
    spawned = {iid for _, iid, _ in live}
    unknown = {'claude': agents is None, 'codex': codex is None}
    for item in ready:
        if item['iid'] in spawned:
            print(f'{core.ref(item)}: its supervisor or worker is already running here; not started again.')
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


def supervisor_prompt(iid):
    """#243: the first turn of task `iid`'s supervisor session; `supervise` prints its brief."""
    return core.WORKER.replace(f'{core.TOOL} worker`', f'{core.TOOL} supervise {iid}`')


def supervisor_spawn(item):
    """#243 (R2, R3): the `spawn` arguments of a ready task's supervisor. Same command on every runtime: spawn names
    it `S<N> …`, writes it into the task's block and the session starts on `supervise N`; it launches the worker."""
    return argparse.Namespace(runtime=item['runtime'], name=f'S{item["iid"]} {item["title"][:40]}', remote_control=True,
                              text=supervisor_prompt(item['iid']), full_access=item['full_access'] or item['runtime'] == 'codex')


def launch(start, step):
    """Start a slot per task: one supervisor session (#243), which launches the worker."""
    for item in start:
        step(f'spawn a {item["runtime"]} supervisor for {core.ref(item)}', lambda item=item: core.spawn(supervisor_spawn(item)))


def tick(args):
    """One pass (R4): the tick does the mechanical steps itself (spawn, nudge, wake, retire) and prints the R6
    report and what needs judgement; exit 1 only for judgement."""
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
            if hasattr(args, 'output'):
                args.output['outcome'] = 'refused'
            return print('Skipped: another tick pass is running.', file=sys.stderr)
        judgement = queue_pass(args)
    if not judgement:
        return
    if hasattr(args, 'output'):
        args.output['outcome'] = 'judgement_needed'
        args.output['refusals'] = judgement
    sys.stdout.flush()
    sys.exit(1)


def idle_ticks():
    return core.TICK_BEAT.with_name('taskq-tick-idle')


def idle_stop(failed):
    """Count empty passes and run configured cleanup on the stop-th one."""
    idle = core.personal().get('idle', {})
    stop, clean = idle.get('stop', 5), idle.get('cleanup', True) and core.CLEANUP.get('enabled', True)
    count = int(idle_ticks().read_text()) + 1 if idle_ticks().exists() else 1
    if not stop or count < stop:
        idle_ticks().write_text(f'{count}\n')
        return None
    idle_ticks().unlink()
    cleanup = f'`{core.TOOL} cleanup --apply`'
    if clean:
        try:
            core.cleanup(argparse.Namespace(apply=True))  # its Remove and Ask the owner sections go to the coordinator
        except (SystemExit, OSError, subprocess.SubprocessError) as error:
            failed.append(f'run {cleanup}: {core.codex_line(str(error))}')
    return f'\nIdle {count} ticks:' + f' ran {cleanup}' * clean + '; report to the owner.'


def retire(open_iids):
    """R11, the one retire step: this checkout's `T<N>`/`S<N>` sessions (spawn names them) whose task N is closed,
    once their turn has ended. Claude: stop and remove; Codex: archive (reversible). Trees and branches: `cleanup`.
    A failure is the sender's log line, never judgement: the next pass tries again (an app-held Codex thread)."""
    found = [(sid, iid, lambda sid=sid: core.claude_stop(sid, remove=True)) for sid, agent in core.claude_agents().items()
             if (iid := session_iid(agent.get('name'))) and local(agent) and not busy(agent)]
    if core.CODEX_SOCKET.exists():
        from taskq.cleanup import cleanup_codex
        root = core.ROOT.resolve()
        try:
            threads = cleanup_codex({root})  # also lists the app project's threads: only this checkout's are its sessions
        except (OSError, SystemExit, ValueError) as error:
            threads = {}
            print(f'Codex threads not checked: {core.codex_line(str(error))[:120]}', file=sys.stderr)
        found += [(sid, iid, lambda sid=sid: core.codex_archive(argparse.Namespace(thread=sid))) for sid, thread in threads.items()
                  if (iid := session_iid(thread.get('name'))) and Path(thread.get('cwd') or '/').resolve() == root
                  and (thread.get('status') or {}).get('type') in ('idle', 'notLoaded')]
    for sid, iid, stop in found:
        if iid in open_iids:
            continue
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                stop()
            print(f'Retired {sid}: #{iid} is closed.', file=sys.stderr)
        except (SystemExit, OSError, subprocess.SubprocessError) as error:
            print(f'Kept {sid} of closed #{iid} for a later pass: {core.codex_line(str(error))[:120]}', file=sys.stderr)


def board_link(candidates):
    """The Board link of the R6 report, or 'unavailable' when it cannot be read (R12)."""
    try:
        if not core.BOARDS:
            return core.issue_board()
        board = next((board for board in core.api('GET', 'boards') if board['name'] == core.BOARD), None)
        issue = next((item for item in candidates if item.get('web_url')), None)
        return f'{re.split(r"/(?:-/)?issues/", issue["web_url"])[0]}/-/boards/{board["id"]}' if board and issue else 'unavailable'
    except (SystemExit, OSError, ValueError, subprocess.SubprocessError):
        return 'unavailable'


def queue_pass(args):
    """One pass of the coordinator: release dead claims, spawn, nudge, wake and retire here (a step each), print the
    R6 report and what needs judgement. Returns the judgement lines."""
    failed = []

    def step(what, action):
        try:
            with contextlib.redirect_stdout(sys.stderr):
                action()
            print(f'Done: {what}.', file=sys.stderr)  # the sender's log, never the coordinator's turn
        except (SystemExit, OSError, ValueError, subprocess.SubprocessError) as error:
            failed.append(f'{what}: {core.codex_line(str(error))}')
    auto_update()
    contract_news()
    print(f'taskq {core.version()}')
    if warning := clone_warning():
        print(warning)
    if not core.CODEX_SOCKET.exists():
        print(f'Codex workers unavailable on this machine: no Codex app server socket {core.CODEX_SOCKET}; '
              f'start it: `{core.CODEX_HEADLESS}`.')
    loaded, candidates = core.profile(args)
    selected = {item['iid'] for item in candidates}
    if hasattr(args, 'output'):
        args.output['tasks'] = [{'id': item['iid'], 'state': item['state'], 'claim': item['claim']} for item in candidates]
    # #145: only the coordinator machine ([coordinator] machine of taskq.toml; none set: every machine) starts shared
    # workers, accepts reviews and shows questions; another machine releases its own work and starts its host-* tasks.
    holder = core.COORDINATOR in (None, core.machine())
    # #43: a session seen here decides at once: dead is released now, alive (busy or idle) never by age.
    doing = [item for item in loaded[0] if item['iid'] in selected and item['state'] == 'doing' and (item['claim'] or {}).get('session')]
    agents = core.claude_agents() if any(found.get('runtime') == 'claude' for item in loaded[0] if item['iid'] in selected
                                         for found in ((item['claim'] or {}), item.get('supervisor') or {}) if found.get('session')) else {}
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
        print(f'Released {"dead" if item in dead else "stalled"} {core.ref(item)}.')
    stalled = dead + stalled
    retire(loaded[1])
    if not holder:
        # A task pinned to this machine (`host-<name>`) starts only here: the coordinator elsewhere cannot start it.
        start = starts(args, core.load() if stalled else loaded, {item['iid'] for item in candidates if item.get('host') == core.machine()})
        if not start:
            return print(f'coordinator is {core.COORDINATOR}: this tick released only its own stalled work. Say so and stop.')
        print(f'coordinator is {core.COORDINATOR}: started only the workers pinned to this machine.\n')
        launch(start, step)
        return failed
    loaded = core.load() if stalled else loaded
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
        moved += 1
        print(f'Moved {core.ref(item)} {item["state"]} → {"ready" if item["state"] == "waiting" else "waiting"}.')
    everything, _, odd, problems, inbox = loaded = core.load() if moved else loaded
    board = board_link(candidates)
    everything = [item for item in everything if item['iid'] in selected]
    # #83: one table of every worker and supervisor (R6); the owner's chat opens only http(s) links.
    report(board, [report_row(item, agents, (alive.get(item['iid']) or (None, f'issue {core.age(item)} min ago'))[1])
                                    for item in everything if listed(item)])
    # #243 (R3): a supervised task's review is its supervisor's, never the coordinator's: the tick wakes that session.
    review = [item for item in everything if item['state'] == 'review' and item['result'] and not item.get('supervisor')]
    # A question reaches the owner once, when it is new; the ones already shown come back as a daily summary.
    asked = [(item, *question(item['iid'])) for item in everything if item['state'] == 'ask']
    fresh = [(item, text) for item, text, shown in asked if shown is None]
    summary = [(item, text) for item, text, shown in asked if shown and time.time() - shown >= core.SUMMARY_SECONDS]
    # An issue whose labels or data do not fit a state taskq can run.
    odd = [f'{core.ref(issue)} labels {issue["labels"]}: give it exactly one state label' for issue in odd] + [
        f'{core.ref(item)} is doing without a worker: move it back to ready or `release {item["iid"]}`'
        for item in everything if item['state'] == 'doing' and not (item['claim'] or {}).get('session')] + [
        f'{core.ref(item)} is in review without a result: `reject {item["iid"]}` or close it by hand'
        for item in everything if item['state'] == 'review' and not item['result']]
    start = starts(args, loaded, selected)
    # #197: the owner's tick (this machine coordinates) applies native cleanup when due; never a timer of its own.
    sys.modules['taskq.cleanup'].scheduled(args)  # the module: `core.cleanup` is the command function
    # #243: a supervisor whose task needs it (a result to review; ready again after an answer or reject) and whose
    # turn has ended is woken with one fixed line; a busy or unknown one (another machine) is left alone.
    supervise = [item for item in everything if item.get('supervisor') and (item['state'] == 'review' and item['result']
                 or item['state'] == 'ready')
                 and liveness({**item, 'state': 'doing', 'claim': item['supervisor']}, agents)[0] == 'idle']
    if not (review or fresh or summary or start or odd or problems or supervise) and not any(item['state'] == 'doing' for item in everything):
        # #153: an ask or review task waits for someone, so it is no idle pass.
        if any(item['state'] in ('ask', 'review') for item in everything):
            idle_ticks().unlink(missing_ok=True)
        elif stop := idle_stop(failed):
            print(inbox_line(inbox) + stop)
            return ['idle stop'] + [f'inbox {issue["iid"]}' for issue in inbox] + failed
        print(inbox_line(inbox) + 'Nothing to do. Say so and stop.')
        return [f'inbox {issue["iid"]}' for issue in inbox]
    idle_ticks().unlink(missing_ok=True)
    print(f'You are the coordinator of the task queue for this one pass. Queue tool: `{core.TOOL}`\n')
    print(inbox_line(inbox), end='')
    workers = [item for item in everything if item['state'] in ('doing', 'ask', 'review') and (item['claim'] or {}).get('session')]
    stuck = []
    for item in workers:
        session, runtime = item['claim']['session'], item['claim'].get('runtime')
        state = (alive.get(item['iid']) or liveness(item, agents))[0]
        if item['state'] != 'doing' or item.get('result') or state == 'busy':
            continue
        note = handed_in(item, ('result', 'ask', 'problem'))
        if note and note.startswith('**problem**'):
            # #223: the worker ended its turn on a `problem` note, no result or ask: judgement, never a nudge or a
            # release; its claim stays. Unknown (another machine, no CLI) is listed too: a listing has no effect.
            stuck.append((item, note))
        elif state == 'idle' and runtime == 'claude':
            from taskq.runtimes import get
            step(f'nudge idle Claude {core.ref(item)}', lambda session=session: get('claude').send(session, NUDGE))
        elif state == 'idle':
            from taskq.runtimes import get
            step(f'nudge idle Codex {core.ref(item)}', lambda item=item: get('codex', full_access=item['full_access']).send(session, NUDGE))
    for item in supervise:
        step(f'wake the supervisor of {core.ref(item)}', lambda item=item: wake_supervisor(item))
    permissions = [item['_runtime_observation'] for item in workers if item.get('_runtime_observation', {}).get('status') == 'waiting_permission']
    if permissions:
        print('## Runtime permissions\n\nThe owner approves in the linked runtime UI. Keep the same worker; do not answer, nudge or spawn another.\n')
        for observation in permissions:
            print(f'- [{observation["session"]}]({observation["session_link"]}): {observation["exact_blocker"]}')
    if stuck:
        print('## Worker problems\n\nTask is doing; its worker ended its turn on a problem note, no result or ask. Its claim stays: '
              'send the next step to that same session (Claude: `claude --bg --resume <session> "<next step>"`; Codex: '
              f'`{core.TOOL} codex-send <session> --text "<next step>"`), or `{core.TOOL} release <N>` only for a stopped session. '
              'Do not answer: the task is not in ask.\n')
        for item, note in stuck:
            print(f'- {core.ref(item)} {item["claim"]["runtime"]} {item["claim"]["session"]}:\n' + core.data(note.split('\n\n', 1)[-1]))
    if odd:
        print('## Board mismatch\n\nThese issues are not in a state taskq can run. Fix each:\n')
        print(core.data(''.join(f'- {line}\n' for line in odd).rstrip()))
    if problems:
        print('## Problems without a task\n\nRead each. Fix it now if small, else `add` a task for it; then close the '
              'issue with a note of what was done.\n')
        print(core.data(''.join(f'- {core.ref(issue)} {issue["title"]}\n' for issue in problems).rstrip()))
    for item in review:
        sha = item['result'].get('sha')
        print(f'## Review {core.ref(item)}: {item["title"]}\n\n{item["text"]}\n\nHanded in:\n\n{core.data(handed_in(item, ("result",)) or "none")}\n'
              + (f'Commit: [{sha}]({core.commit_url(item, sha)})\n' if sha and item.get('web_url') else '') +
              f'Check the result against the Acceptance above (for code and docs read the commit).\n'
              f'Accepted: `{core.TOOL} close {item["iid"]} --text "<what you checked>"`. '
              f'Not accepted: `{core.TOOL} reject {item["iid"]} --text "<what to fix>"`.\n')
        if (item['claim'] or {}).get('runtime') == 'codex' and not core.local_claim(item['claim']):
            print(f'After close, archive its Codex session on its machine: `{core.TOOL} codex-archive {item["claim"]["session"]}`.\n')
    launch(start, step)
    if failed:
        print('\n## Steps that failed\n\nThe tick could not do these itself; do each by hand (§ 3) or tell the owner:\n')
        print(core.data(''.join(f'- {line}\n' for line in failed).rstrip()))
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
    return ([f'review {item["iid"]} {item["result"].get("sha")}' for item in review] + [f'ask {item["iid"]}' for item, _ in fresh + summary]
            + [f'stuck {item["iid"]}' for item, _ in stuck] + [observation['notify_dedup'] for observation in permissions]
            + odd + [f'problem {issue["iid"]}' for issue in problems] + [f'inbox {issue["iid"]}' for issue in inbox] + failed)
