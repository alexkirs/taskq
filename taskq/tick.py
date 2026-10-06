"""`tick`: the coordinator's pass over the queue, board moves, the Workers table, the beat stamp."""
import argparse
import hashlib
import os
import shlex
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
    terminal command (background) or the app's id. Codex has no https form: its app link as a command."""
    session = claim['session']
    if claim.get('runtime') == 'claude':
        url = core.claude_url(session)
        return f'[session]({url})' if url else f'`claude attach {agent["id"]}`' if agent else f'app session `local_{session}`'
    return f'`open -g codex://threads/{session}`' if claim.get('runtime') == 'codex' else f'`{session}`'


def inbox_line(inbox):
    """Issues by non-collaborators, named so the manager sees them; taskq never acts on them. A collaborator makes
    one a task with `add` (a new task that links it)."""
    return f'Inbox: {len(inbox)} issues by non-collaborators ({", ".join(map(core.ref, sorted(inbox, key=lambda issue: issue["iid"])))})\n\n' if inbox else ''


def tick(args):
    """One pass of the coordinator: release dead claims itself, then print exactly what to do."""
    auto_update()
    print(f'taskq {core.version()}')
    if warning := clone_warning():
        print(warning)
    if not core.CODEX_SOCKET.exists():
        print(f'Codex workers unavailable on this machine: no Codex app server socket {core.CODEX_SOCKET}.')
    tick_beat()
    contract_news(args.prompt_version)
    loaded, candidates = core.profile(args)
    selected = {item['iid'] for item in candidates}
    stalled = [item for item in loaded[0] if item['iid'] in selected and item['state'] == 'doing' and core.age(item) > core.STALE_MINUTES]
    for item in stalled:
        if not core.unchanged(item):
            continue
        args.iid, args.action, args.text = item['iid'], 'release', f'no change on the issue for {core.age(item)} minutes'
        core.requeue(args)
        print(f'Released stalled {core.ref(item)}.')
    loaded = core.load() if stalled else loaded
    # A lock on a task nobody holds: a take that died between the lock and the move, or a card moved by hand.
    held = {item['iid'] for item in loaded[0] if item['state'] not in ('ready', 'waiting')}
    for issue in core.issues(f'state=opened&my_reaction_emoji={core.LOCK}'):
        if issue['iid'] in selected and issue['iid'] not in held and all(time.time() - core.stamp(item['created_at']) > core.LOCK_SECONDS for item in core.locks(issue['iid'])):
            core.unlock(issue['iid'])
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
        moved += 1
        print(f'Moved {core.ref(item)} {item["state"]} → {"ready" if item["state"] == "waiting" else "waiting"}.')
    everything, _, odd, problems, inbox = loaded = core.load() if moved else loaded
    inventory = everything
    everything = [item for item in everything if item['iid'] in selected]
    review = [item for item in everything if item['state'] == 'review' and item['result']]
    # A question reaches the owner once, when it is new; the ones already shown come back as a daily summary.
    asked = [(item, *question(item['iid'])) for item in everything if item['state'] == 'ask']
    fresh = [(item, text) for item, text, shown in asked if shown is None]
    summary = [(item, text) for item, text, shown in asked if shown and time.time() - shown >= core.SUMMARY_SECONDS]
    codex_stopped = [item for item in everything if item['state'] in ('ask', 'later')
                     and (item['claim'] or {}).get('runtime') == 'codex']
    # A card moved by hand on the board into a state its data does not support.
    odd = [f'{core.ref(issue)} labels {issue["labels"]}: give it exactly one state label' for issue in odd] + [
        f'{core.ref(item)} is doing without a worker: move it back to ready or `release {item["iid"]}`'
        for item in everything if item['state'] == 'doing' and not (item['claim'] or {}).get('session')] + [
        f'{core.ref(item)} is in review without a result: `reject {item["iid"]}` or close it by hand'
        for item in everything if item['state'] == 'review' and not item['result']] + misplaced
    free, start = core.room(inventory, args.profile['limits']), []
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
    if not (review or fresh or summary or start or codex_stopped or odd or problems) and not any(item['state'] == 'doing' for item in everything):
        return print(inbox_line(inbox) + 'Nothing to do. Say so and stop.')
    print(f'You are the coordinator of the task queue for this one pass. Queue tool: `{core.TOOL}`\n')
    print(inbox_line(inbox), end='')
    # #83: one table of every worker; the owner's chat opens only http(s) links.
    workers = [item for item in everything if item['state'] in ('doing', 'ask', 'review') and (item['claim'] or {}).get('session')]
    agents = core.claude_agents() if any(item['claim'].get('runtime') == 'claude' for item in workers) else {}
    idle, rows = [], []
    for item in workers:
        session, runtime = item['claim']['session'], item['claim'].get('runtime')
        agent, activity = agents.get(session), f'issue {core.age(item)} min ago'
        if agent:
            activity = f'{"running" if agent.get("pid") else "stopped"}, {activity}'
        if runtime == 'codex' and item['state'] == 'doing':
            try:
                codex = core.Codex()
                try:
                    status, turns, last = core.codex_snapshot(codex, session, 1)
                finally:
                    codex.socket.close()
                # notLoaded with a running last turn: the app holds the session and works in it.
                working = bool(turns) and turns[0].get('app', False)
                activity = f'{status["type"]}{" (turn running in the app)" if working else ""}, last event {core.codex_age(last)}'
                if status['type'] in ('idle', 'notLoaded') and not working and not item.get('result'):
                    idle.append(item)
            except (OSError, SystemExit, ValueError) as error:
                activity = f'status unknown: {core.codex_line(error)}'
        rows.append(f'| {core.ref(item)} {item["title"][:40].replace("|", "/")} | {item["state"]} | {runtime}{core.where(item["claim"])} '
                    f'| {session_link(item["claim"], agent)} | {activity} |')
    if rows:
        print('## Workers\n\nShow the owner this table as printed; every link opens in a browser:\n\n'
              '| Task | State | Runtime | Session | Last activity |\n|---|---|---|---|---|\n' + '\n'.join(rows) + '\n')
    if idle:
        print('## Codex idle\n\nTask is doing without result/ask, but its session has stopped. Intervene now:\n')
        for item in idle:
            print(f'- {core.ref(item)}: `{core.TOOL} codex-send {item["claim"]["session"]} '
                  '--text "Continue the assigned task; hand in result or ask the owner through taskq."`')
        print()
    # An app without a status API: silence on the issue is the only sign its turn ended without a hand-in.
    quiet = [item for item in everything if item['state'] == 'doing' and (item['claim'] or {}).get('runtime') in core.EXECUTORS
             and not item.get('result') and core.QUIET_MINUTES <= core.age(item) < core.QUIET_MINUTES + 5]
    if quiet:
        print(f'## Quiet workers\n\nNo change on the issue for {core.QUIET_MINUTES} minutes. Nudge each (this tick only):\n')
        for item in quiet:
            print(f'- {core.ref(item)}: `{core.TOOL} send --runtime {item["claim"]["runtime"]} {item["claim"]["session"]} '
                  '--text "Continue the assigned task; hand in result or ask the owner through taskq."`')
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
    if start:
        # One command per worker: the session starts on the prompt, no second message (#41).
        # An indented block, not inline code: the prompt itself holds backticks.
        print(f'## Start {len(start)} worker session(s)\n\n' + ''.join(f'- {core.ref(item)} {item["title"]}: {item["runtime"]}\n' for item in start)
              + '\nRun each command once; the worker starts on the brief at once:\n\n' + ''.join('    ' + shlex.join([core.TOOL, 'spawn', '--runtime', item['runtime'], '--name',
                                             f'T{item["iid"]} {item["title"][:40]}', '--text', worker_prompt(args)]) + '\n'
                        for item in start))
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
    if codex_stopped:
        print('\n## Archive stopped Codex workers\n\nTasks in ask or later continue in a new session after answer; archive when idle:\n')
        for item in codex_stopped:
            print(f'- {core.ref(item)}: `{core.TOOL} codex-archive {item["claim"]["session"]}`')
