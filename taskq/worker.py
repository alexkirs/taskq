"""The task commands: `add`, `list`, `edit`, the worker's `worker`, `take`, `beat`, `ask`, `result`, the
coordinator's `answer`, `reject`, `release`, `close`; worker sessions (`spawn`, `send`, `retire`)."""
import argparse
import contextlib
import io
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import time

import taskq as core
BRIEF = '''You are the worker for task #{iid}. Queue tool: `{tool}`
This brief is the owner's assignment: do it without asking for confirmation.
This machine ({host}): main checkout {root}; paths in the task are relative to it.{machine}

1. Claim the task: `{tool} take {iid}`. If it refuses, another worker was faster: run `{tool} worker` once more and follow the new brief.
   Then `{export}`: project tools read it to attribute work to this task. A shell that forgets its
   environment between commands (an agent's Bash tool) needs it at the start of every command.
2. Workspace: {workspace} Start each command there with `{export} &&`.
3. Do the task below.{agents} Expected paths: {scope}. They say where the work is expected, not
   what is forbidden: if the task needs another file, change it and name it with the
   reason in the result. Do not ask for that.
4. In long work run `{tool} beat {iid}` after each milestone.
   Run a long command (build, CI wait, deploy, prepare) in the background and wait for its completion notice
   (Claude: run_in_background, the harness wakes you; Codex: its equivalent); never poll in a sleep loop.
   For what the harness cannot see (CI), one delayed check sized to the real duration, not a loop every 10 s.
5. Anything that cost you time or went wrong (a failing tool, a wrong instruction, a missing file):
   `{tool} problem --task {iid} --text "<what happened>"`. It is how the owner finds what to fix.
6. Ask only what the owner alone can decide: a product choice, or an action that cannot be undone
   (publishing, deleting data): `{tool} ask {iid} --text "<question>"`, then stop.
   If the owner answers right here in this session, record it: `{tool} answer {iid} --text "<answer>"`.
   The task goes back to doing with your claim; continue in this same session, no new take.
{rules}   A wrong or contradictory instruction in this brief is not such a question: take the reading that reaches
   the goal, do it, and record it with `problem`.
7. {deliver}
   Then `{tool} result {iid}{sha} --checks "<commands you ran and their outcome>" --text "<summary>"` and stop.
Everything you write through `{tool}` is public: no environment values, paths outside the repository, tokens.
{preferences}

# {title}

{text}

# History (oldest first)

{notes}
'''
DELIVER = {
    True: 'Deliver: commit, `git fetch origin && git rebase origin/main`, run the checks, `git push origin HEAD:main` (never force).',
    False: 'Deliver: put the whole answer into `--text`.',
}

# #243 (principles R2, R3, R11): the brief of a task's supervisor session, which the tick spawns per ready task.
SUPERVISOR_BRIEF = '''You are the supervisor of task #{iid}, now {state}. Queue tool: `{tool}`
This brief is the owner's assignment: do it without asking for confirmation. You drive one worker session for this
task, review its result, publish it and retire the worker. Never take the task or do its work yourself.
This machine ({host}): main checkout {root}. Its worker runs on {runtime}: you and it hold one slot together.
Start each shell command with `export TASKQ_TASK={iid} TASKQ_RUNTIME={runtime} &&`.

By the task's state (`{tool} view {iid}` reads it again):
- ready, no worker launched yet: start it, once, with this command; then end your turn:
      {spawn}
- ready again after an answer, a reject or a release (worker {worker}): send that same worker the newest answer or reject text
  and tell it to run `{tool} take {iid}` and continue: {resume}
  Never spawn a second worker. A worker still busy (`claude agents`, `{tool} codex-read <id>`) needs nothing: end your turn.
- doing or ask: the worker works or waits for the owner (the PM relays the answer); end your turn.
- review: check the exact result SHA against the Acceptance below: read the commit (`git show <sha>`), run the
  task's focused tests in a fresh tree, check that CI on that SHA is green (taskq-manager.md § 3 Acceptance).
  Accepted: `{tool} close {iid} --text "<what you checked>"`: {publish}; close also retires the worker.
  Not accepted: `{tool} reject {iid} --text "<exact fixes>"`, then send the fixes to the same worker as above.
The tick wakes you when the task needs you; after close end your turn, the tick retires this session.
Keep this session's name as spawn set it (never rename it) and start no other session but this task's one worker.
What went wrong or needs the owner: `{tool} problem --task {iid} --text "<what>"`, then end your turn.
Everything you write through `{tool}` is public: no environment values, paths outside the repository, tokens.
{preferences}

# {title}

{text}

# History (oldest first)

{notes}
'''
PUBLISH = {'direct': 'the worker pushed to main; close checks the SHA is in origin/main',
           'review': 'close fast-forwards main to that reviewed branch head (no force)'}

REVIEW_DELIVER = ('Deliver: commit on branch `taskq-{iid}`, `git fetch origin && git rebase origin/main`, '
                 'run the checks, `git push --force-with-lease origin HEAD:refs/heads/taskq-{iid}`. '
                 'Push only this branch; never push main. Hand in its exact full SHA; the manager publishes after review.')


def doing_since(iid):
    """When the issue last entered doing, by GitLab's own clock: the newest `add q-doing` label event."""
    return max([core.stamp(event['created_at']) for event in core.pages(f'issues/{iid}/resource_label_events')
                if event['action'] == 'add' and (event.get('label') or {}).get('name') == core.PREFIX + 'doing'], default=0)


def need_owner(current):
    mine, claim = core.me(), current['claim'] or {}
    if (claim.get('runtime'), claim.get('session')) != (mine['runtime'], mine['session']):
        core.fail(f'#{current["iid"]} is not claimed by this session')


# --- commands: anyone -----------------------------------------------------------------------

def add(args):
    if not args.goal.strip() or not args.acceptance.strip():
        core.fail('a task needs a goal and an acceptance')
    unknown = set(args.area) - set(core.AREAS)
    if unknown:
        core.fail(f'unknown areas {sorted(unknown)}; configure [areas] names and run init')
    runtime = core.DEFAULT_RUNTIME[args.type] if args.runtime is None else None if args.runtime == 'any' else args.runtime
    block = {'scope': args.scope, 'deps': args.deps, 'claim': None, 'waiting_for': None, 'result': None}
    text = f'## Goal\n\n{args.goal}\n\n## Acceptance\n\n{args.acceptance}'
    labels = args.label + [f'area-{area}' for area in args.area] + [f'{core.PREFIX}ready', f'priority-{args.priority}', args.type] + ([core.RUN + runtime] if runtime else []) + ([core.ON + args.host] if args.host else [])
    body = {'title': args.title, 'description': core.render(text, block), 'labels': ','.join(labels)}
    if args.mine:
        body['assignee_ids'] = [core.user()]
    if args.milestone:
        body['milestone_id'] = core.milestone_id(args.milestone)
    issue = core.api('POST', 'issues', body)
    core.link(issue['iid'], args.deps)
    found = absolute(' '.join([args.title, args.goal, args.acceptance, *args.scope]))
    if found:
        print(f'Warning: absolute path {found[0]}: a task is read on every machine; write paths relative to the repository', file=sys.stderr)
    print(f'#{issue["iid"]} {issue["web_url"]}')


def absolute(text):
    """Machine paths in task text (/Users/…, /home/…, C:\\…, //wsl.localhost/…): each machine has its own checkout root."""
    return re.findall(r'(?<![\w:/.])(?:/(?:Users|home|mnt|root)/\S+|[A-Za-z]:\\\S*|//wsl[.$]\S+)', text)


def edit(args):
    """Change dependencies, scope or the milestone (epic) of an open task; `tick` then moves it ready<->waiting
    and weighs the new scope against the others."""
    current = core.task(args.iid)
    if args.milestone is not None:
        core.api('PUT', f'issues/{args.iid}', {'milestone_id': core.milestone_id(args.milestone) if args.milestone else None})
    changes, notes = {}, []
    if args.deps is not None:
        core.link(args.iid, args.deps)
        changes['deps'], notes = args.deps, notes + [f'deps {current["deps"]} → {args.deps}']
    if args.scope is not None:
        changes['scope'], notes = args.scope, notes + [f'scope {current["scope"]} → {args.scope}']
    if args.supervisor is not None:
        # #240 P1: the handoff is judged on a read taken after the link/milestone calls above. An owner who assigned
        # or cleared the supervisor meanwhile is seen: a session's stale handoff is refused, never written over it.
        fresh = core.task(args.iid)
        if core.session() is not None and (fresh.get('supervisor'), fresh['claim']) != (current.get('supervisor'), current['claim']):
            core.fail(f'#{args.iid}: its supervisor or claim changed while editing; read it again before a handoff')
        current = fresh
        if (found := supervisor_change(current, args.supervisor)) != current.get('supervisor'):
            changes['supervisor'] = found
            notes.append(f'supervisor {identity(current.get("supervisor"))} → {identity(found)}')
    if changes:
        core.save(current, note_action='edit', note_text='; '.join(notes), **changes)
    print(f'#{args.iid} edited')


def identity(found):
    return f'{found["runtime"]}:{found["session"]}' if found else 'none'


def supervisor_change(current, value):
    """#240: the supervisor `edit --supervisor` may write. Cooperative tracker authority, the same boundary as claims:
    a session identity is self-declared, so this is no proof of a root PM session. The owner's shell (no session)
    assigns, hands off or clears ('' or none); the current supervisor only names its successor; no other session."""
    found = None
    if value not in ('', 'none'):
        runtime, _, session = value.partition(':')
        if runtime not in core.RUNTIMES or not session:
            core.fail(f'--supervisor: write RUNTIME:SESSION with the full session id, RUNTIME one of {", ".join(core.RUNTIMES)}')
        found = {'runtime': runtime, 'session': session}
    old = current.get('supervisor')
    if core.session() is not None and not (found and core.is_caller(old)):
        core.fail(f'#{current["iid"]}: only the owner\'s shell assigns or clears a supervisor; '
                  'the current supervisor may only hand off to a successor')
    claim = current['claim'] or {}
    if found and (claim.get('runtime'), claim.get('session')) == (found['runtime'], found['session']):
        core.fail(f'#{current["iid"]}: the supervisor cannot be the claim session')
    return found


def outsider(current):
    """#240: a decision on a supervised task from a session other than its claim: only its supervisor or the owner."""
    adopt(current)
    found = current.get('supervisor')
    if found and core.session() is not None and not core.is_caller(found):
        core.fail(f'#{current["iid"]} is supervised by {core.short(found)}: only that session or the owner decides')


def supervision(current, mine):
    """#240: why session `mine` cannot take supervised `current`, or None: only the worker of the supervisor's newest
    launch takes it (the same worker after a queue answer); before its launch note lands, a live `T<N>` worker here."""
    from taskq.tick import launched
    found = current.get('supervisor')
    if not found:
        return None
    if (mine['runtime'], mine['session']) == (found['runtime'], found['session']):
        return 'its supervisor does not take it'
    bound = supervised_worker(current['iid'], found)
    return None if bound == mine['session'] or not bound and mine['session'] in (launched(current['iid'], mine['runtime']) or []) else \
        f'supervised by {core.short(found)}: only the worker it launched takes it'


def supervised_worker(iid, found):
    """The session of supervisor `found`'s newest `launch` note, or None. #284: launches by the sessions it was
    resumed from (`supervisor A → B (resumed)` notes) are its own."""
    mine = {core.short(found)}
    for body in reversed(core.notes(core.comments(iid))):
        if (resume := re.search(r'^supervisor (\w+):(\S+) → (\w+):(\S+) \(resumed\)$', body, re.M)) and f'{resume[3]}:{resume[4][:8]}' in mine:
            mine.add(f'{resume[1]}:{resume[2][:8]}')
        if (launch := re.match(r'\*\*launch\*\* · (\S+)\n\nsession (.+)', body)) and launch[1] in mine:
            return launch[2]
    return None


def later(args):
    """The owner defers a task; nobody waits on anything. Back with `answer` or by hand to ready."""
    current = core.task(args.iid, ('ready', 'waiting', 'ask'))
    core.save(current, 'later', 'later', args.text, waiting_for=args.text)


def listing(args):
    everything, open_iids, odd, problems, _ = core.load()
    for item in sorted(everything, key=lambda item: core.STATES.index(item['state'])):
        claim, detail = item['claim'] or {}, ''
        if item['state'] == 'ready':
            detail = core.refusal(item, everything, open_iids) or ('continue' if claim else '')
        elif item['state'] == 'doing':
            detail = f'{claim.get("runtime")}:{(claim.get("session") or "")[:8]}{core.where(claim)}, last change {core.age(item)} min ago'
        elif item['state'] == 'waiting':
            detail = f'open dependencies {sorted(set(item["deps"]) & open_iids)}'
        elif item['state'] == 'later':
            detail = item['waiting_for'] or ''
        # A claim without a session (released, answered) still holds the task's paths: name what waits on it.
        held = claim and not claim.get('session') and [other['iid'] for other in everything if other['state'] == 'ready'
                and core.refusal(other, everything, open_iids) == f'scope overlaps #{item["iid"]}']
        if held:
            detail = '; '.join(filter(None, (detail, f'holds scope for {", ".join(f"#{iid}" for iid in held)}')))
        print(f'#{item["iid"]:<4} {item["state"]:<8} p{item["priority"]} {item["runtime"] or "any":<6} '
              + (f'{item["web_url"]} ' if args.links else '') + item['title'] + (f'  [{detail}]' if detail else ''))
    for issue in odd:
        print(f'#{issue["iid"]:<4} ?        labels {issue["labels"]}: not a valid task, see `tick`')
    for issue in problems:
        print(f'#{issue["iid"]:<4} {core.PROBLEM:<8} {issue["title"]}')


def set_runtime(args):
    current = core.task(args.iid, ('ready', 'waiting', 'ask', 'later'))
    runtime = None if args.runtime == 'any' else args.runtime
    core.save(current, add=[core.RUN + runtime] if runtime else [], remove=[core.RUN + current['runtime']] if current['runtime'] else [],
         note_action='runtime', note_text=f'{current["runtime"] or "any"} → {args.runtime}')
    print(f'#{args.iid} runtime {args.runtime}')


# --- commands: worker -----------------------------------------------------------------------

def brief(current):
    claim, pushes = current['claim'], current['type'] in ('code', 'docs')
    kind = ('continue' if claim else 'new') if pushes else 'none'
    found = core.comments(current['iid'], everyone=True)
    kept = core.collaborators(found)
    omitted = f'\n\n{len(found) - len(kept)} comments by non-collaborators omitted' if len(found) > len(kept) else ''
    # the brief also renders without a session (CI, a plain shell): the claim or the task's runtime stands in
    runtime = (core.session() or claim or {}).get('runtime') or current['runtime'] or 'unknown'
    return BRIEF.format(**{**current, 'tool': core.TOOL, 'rules': core.RULES, 'deliver': REVIEW_DELIVER.format(iid=current['iid']) if pushes and core.PUBLISH == 'review' else DELIVER[pushes],
                           'agents': ' Follow AGENTS.md.' if (core.ROOT / 'AGENTS.md').is_file() else '',
                           'workspace': core.WORKSPACE[kind].format(iid=current['iid']),
                           'sha': ' --sha <pushed commit>' if pushes else '',
                           'export': f'export TASKQ_TASK={current["iid"]} TASKQ_RUNTIME={runtime}',
                           'scope': ', '.join(current['scope']) or 'none', 'host': core.machine(), 'root': core.ROOT,
                           'machine': ''.join(f'\n   {line}' for line in (core.personal().get('machine', {}).get('notes') or '').strip().splitlines()),
                           'preferences': core.preferences_text(),
                           'notes': ('\n\n---\n\n'.join(core.notes(kept)) or 'none') + omitted})


def supervisor_brief(current):
    """#243: what `supervise` prints: the supervisor's steps for `current`, with its worker's spawn and resume commands."""
    found, iid = current['supervisor'], current['iid']
    runtime, worker = found['runtime'], supervised_worker(iid, found)
    spawn = shlex.join([core.TOOL, 'spawn', '--runtime', runtime, '--name', f'T{iid} {current["title"][:40]}', '--text', worker_prompt(iid)]
                       + ['--codex-full-access'] * (current['full_access'] and runtime == 'codex'))
    text = f'<that text>. Then run {core.TOOL} take {iid} and continue.'
    resume = ('none launched yet' if not worker else f'`claude --bg --resume {worker} "{text}"`' if runtime == 'claude'
              else f'`{core.TOOL} codex-send {worker} --text "{text}"`' if runtime == 'codex'
              else f'`{core.TOOL} send --runtime {runtime} {worker} --text "{text}"`')
    kept = core.collaborators(core.comments(iid, everyone=True))
    return SUPERVISOR_BRIEF.format(**{**current, 'tool': core.TOOL, 'host': core.machine(), 'root': core.ROOT, 'runtime': runtime,
                                      'spawn': spawn, 'worker': worker or 'none', 'resume': resume, 'publish': PUBLISH[core.PUBLISH],
                                      'preferences': core.preferences_text(),
                                      'notes': '\n\n---\n\n'.join(core.notes(kept)) or 'none'})


def worker_prompt(iid):
    """The first turn of task `iid`'s worker: `worker --task N` prints exactly that task's brief."""
    return core.WORKER.replace(f'{core.TOOL} worker`', f'{core.TOOL} worker --task {iid}`')


def supervise(args):
    """#243: the first command of a supervisor session the tick spawned (`S<N> …`), and of each wake: its brief.
    spawn writes the session into the block right after it starts, so a block without a supervisor is waited for."""
    mine = core.me()
    for _ in range(SUPERVISE_WAIT):
        issue = core.api('GET', f'issues/{args.iid}')
        if issue['state'] != 'opened':
            return print(f'#{args.iid} is closed: nothing to supervise. End your turn; the tick retires this session.')
        current = core.task(args.iid)
        if current.get('supervisor'):
            adopt(current)
            break
        time.sleep(5)
    if not core.is_caller(current.get('supervisor')):
        core.fail(f'#{args.iid} is supervised by {core.short(current.get("supervisor"))}, not by {core.short(mine)}: end your turn')
    print(supervisor_brief(current))


SUPERVISE_WAIT = 6  # reads, 5 s apart: spawn's block write lands seconds after the session starts


def worker(args):
    """What a fresh worker session runs first: the brief of the first task that can start now."""
    loaded, candidates = core.profile(args)
    mine = core.me()
    runtime = mine['runtime']
    # #243: a supervisor that runs `worker` gets its own supervisor brief, never work of its own.
    if own := [item for item in loaded[0] if core.is_caller(item.get('supervisor'))]:
        return print(supervisor_brief(own[0]))
    # #240: a session that holds a doing claim here continues that task; it never selects fresh work.
    own = [item for item in loaded[0] if item['state'] == 'doing' and core.local_claim(item['claim'] or {})
           and ((item['claim'] or {}).get('runtime'), (item['claim'] or {}).get('session')) == (runtime, mine['session'])]
    if own:
        return print(f'No task can start now for this session: it holds #{own[0]["iid"]} (doing, your claim). '
                     f'Continue that task; do not take another. Its brief:\n\n{brief(own[0])}')
    if args.task:  # the worker a supervisor launched for task N: its brief; `take` checks the rest
        return print(brief(core.task(args.task)))
    free = core.room(loaded[0], args.profile['limits'])
    # #240: a supervised task's worker is launched by its supervisor (`worker --task N`), never picked here.
    found = [item for item in candidates if item['state'] == 'ready' and free[runtime] > 0 and not item.get('supervisor')
             and not core.refusal(item, loaded[0], loaded[1], runtime) and not core.sandbox_refusal(item, runtime)]
    print(brief(found[0]).replace(f'{core.TOOL} worker`', f'{core.TOOL} worker{core.profile_arguments(args)}`')
          if found else 'No task can start now. Say so and stop.')


def take(args):
    """Check the admission rule, post a `take` note, and go on only when it is the winning one (`taken`): notes are
    append-only and ordered by the store, so two parallel takes read the same winner. The loser deletes its note.
    Scope is a rule between tasks: after the move the newer of two overlapping takes gives way."""
    mine, uid = core.me(), core.user()
    everything, open_iids, *_ = core.load()
    current = next((item for item in everything if item['iid'] == args.iid), None) or core.fail(f'#{args.iid} is not an open taskq task')
    claim = current['claim'] or {}
    if current['state'] == 'doing' and (claim.get('runtime'), claim.get('session')) == (mine['runtime'], mine['session']):
        core.note(args.iid, 'take')  # a repeated take of its own task, e.g. after an answer in the session
        return print(f'#{args.iid} is yours')
    reason = (f'state is {current["state"]}' if current['state'] != 'ready' else
              core.refusal(current, everything, open_iids, mine['runtime']) or core.sandbox_refusal(current, mine['runtime'])
              or delegation(current, uid) or supervision(current, mine))
    if reason:
        core.fail(f'#{args.iid} cannot start: {reason}')
    posted = core.note(args.iid, 'take')
    drop = lambda: core.api('DELETE', f'issues/{args.iid}/notes/{posted["id"]}')
    if taken(args.iid, posted) != posted['id']:
        drop()
        core.fail(f'#{args.iid} cannot start: another worker took it first')
    # The assignee stays untouched; only an unassigned task becomes the taker's, as before.
    assign = {} if current['assignees'] else {'assignee_ids': [uid]}
    try:
        core.save(current, 'doing', claim=mine, result=None, waiting_for=None, **assign)
    except BaseException:
        with contextlib.suppress(Exception, SystemExit):
            drop()
        raise
    # A task without paths overlaps nothing: no second read.
    rivals = current['scope'] and [other for other in core.load()[0] if other['iid'] != args.iid
             and (other['claim'] or {}).get('session') and core.overlap(current['scope'], other['scope'])]
    if rivals:
        # Each take moves its task before it reads, so the later of two sees the earlier; both order them alike.
        since = doing_since(args.iid)
        older = [other for other in rivals if (doing_since(other['iid']), other['iid']) < (since, args.iid)]
        if older:
            core.save({**current, 'state': 'doing'}, 'ready', **({'assignee_ids': []} if assign else {}))
            drop()
            core.fail(f'#{args.iid} cannot start: scope overlaps #{older[0]["iid"]}')
    print(f'#{args.iid} is yours')


RESETS = ('**ready**', '**answer**', '**reject**', '**release**')  # notes after which a task is ready to take again


def taken(iid, posted):
    """The id of the winning `take` note: the earliest trusted one since the task last became ready. One older than
    TAKE_SECONDS before `posted` is a take that died before its move: void."""
    found = []
    for item in core.comments(iid):
        if item['body'].startswith(RESETS):
            found = []
        elif item['body'].startswith('**take**') and core.stamp(item['created_at']) >= core.stamp(posted['created_at']) - core.TAKE_SECONDS:
            found.append(item['id'])
    return min(found, default=None)


def beat(args):
    """A new note moves `updated_at` (an edited one does not); the previous beat, if it is the newest, goes."""
    need_owner(core.task(args.iid, ('doing',)))
    last = core.collaborators(core.api('GET', f'issues/{args.iid}/notes?sort=desc&per_page=1&activity_filter=only_comments'))
    core.note(args.iid, 'beat')
    if last and last[0]['body'].startswith('**beat**'):
        core.api('DELETE', f'issues/{args.iid}/notes/{last[0]["id"]}')


def ask(args):
    """A worker asks about its doing task; the manager asks about a task not started yet."""
    current = core.task(args.iid, ('doing', 'ready', 'waiting', 'later'))
    if current['state'] == 'doing':
        need_owner(current)
    core.save(current, 'ask', 'ask', args.text)
    if current['state'] == 'doing':
        print(f'#{args.iid} asked. Stop here. If the owner answers in this session, record it with '
              f'`{core.TOOL} answer {args.iid} --text "<answer>"` and continue here; otherwise the coordinator '
              'brings the answer through the queue.')


def result(args):
    current = core.task(args.iid, ('doing',))
    need_owner(current)
    if current['type'] in ('code', 'docs') and not args.sha:
        core.fail(f'a {current["type"]} result needs --sha')
    core.save(current, 'review', 'result', f'`{args.sha}`\n\n{args.text}\n\nChecks: {args.checks}',
         result={'sha': args.sha, 'checks': args.checks})


# --- commands: coordinator and owner ----------------------------------------------------------

def requeue(args):
    """answer: the owner's reply to a question or a hold. reject: review sends the work back.
    release: drop a dead worker's claim. The next worker session continues with the full history."""
    current = core.task(args.iid, {'answer': ('ask', 'later'), 'reject': ('review',)}.get(args.action, core.STATES))
    claim = current['claim'] or {}
    if args.action in ('answer', 'reject') and claim.get('session') and {key: claim.get(key) for key in ('runtime', 'session')} == core.session():
        # The owner answered (or asked for a change in review: reject, #127) in the worker's own session: it
        # continues with its claim, never through ready, where a tick would start a second worker. Its doing
        # place was free while the task waited, so the limit is not checked: the worker never left.
        core.save(current, 'doing', args.action, args.text, waiting_for=None, result=None)
        return print(f'#{args.iid} is doing again with your claim: continue in this session')
    if getattr(args, 'function', None) is requeue and args.action != 'answer':
        # The tick's dead/stalled release (its own command) keeps its evidence path. #243: an answer is the owner's
        # word, which the PM relays from its own session (R3: it moves tasks); the supervisor then resumes the worker.
        outsider(current)
    before = args.action == 'release' and releases(args.iid)
    # An empty claim marks a started task: it keeps its paths and its next worker continues.
    claim = current['claim'] and {'runtime': None, 'session': None}
    if before:
        # #157: a worker that fails the same way (Blender in a Codex sandbox) would take, crash and release on
        # every tick; the second release in a row without an owner's answer or reject goes to the owner instead.
        core.save(current, 'ask', 'release', args.text, waiting_for=None, result=None, claim=claim)
        return core.note(args.iid, 'ask', f'Released {len(before) + 1} times in a row, the last because: {args.text}\n'
                         f'Earlier: {before[-1]}\nDecide how it can run (runtime, access, a fix first) and answer.')
    core.save(current, 'ready', args.action, args.text, waiting_for=None, result=None, claim=claim)


def releases(iid):
    """The texts of the `release` notes since the task's last `answer` or `reject` (the owner's word resets them)."""
    found = []
    for body in core.notes(core.comments(iid)):
        head, _, text = body.partition('\n\n')
        if head.startswith(('**answer**', '**reject**')):
            found = []
        elif head.startswith('**release**'):
            found.append(text.splitlines()[0] if text else 'no reason')
    return found


def close(args):
    issue = core.api('GET', f'issues/{args.iid}')
    if issue['state'] != 'opened':
        return print(f'#{args.iid} already closed')
    current = core.task(args.iid, ('review',))
    outsider(current)  # #243 (R3): a supervised task's review, publication and close are its supervisor's
    if current['type'] in ('code', 'docs'):
        try:
            sha = core.commit(current['result']['sha'])
        except argparse.ArgumentTypeError as error:
            core.fail(f'{error}; reject the task so the worker hands in the pushed commit')
        current = core.unchanged(current)
        if not current:
            return
        if core.PUBLISH == 'review':
            publish_review(current, sha)
        else:
            subprocess.run(['git', 'fetch', '-q', 'origin', 'main'], check=True)
            if subprocess.run(['git', 'merge-base', '--is-ancestor', sha, 'origin/main']).returncode:
                core.fail(f'{sha} is not in origin/main; reject the task so the worker pushes it')
    # Publication can succeed before an interrupted close records it. Do not close or retire a later claim/result.
    current = core.unchanged(current)
    if not current:
        return
    core.save(current, close=True, note_action='close', note_text=args.text)
    print(f'#{args.iid} closed')
    if current['type'] in ('code', 'docs'):
        # Author, date and subject show whether the commit is this task's.
        subprocess.run(['git', 'log', '-1', '--format=%h %an %ad %s', sha], check=False)
    retire_local(current)


def publish_review(current, sha):
    """Publish exactly the reviewed branch head without touching the caller's working tree."""
    branch = f'taskq-{current["iid"]}'

    def git(*args):
        return subprocess.run(['git', '-C', str(core.ROOT), *args], check=True,
                              capture_output=True, text=True).stdout.strip()

    try:
        git('fetch', '-q', 'origin', f'+refs/heads/main:refs/remotes/origin/main',
            f'+refs/heads/{branch}:refs/remotes/origin/{branch}')
        if sha != git('rev-parse', f'refs/remotes/origin/{branch}'):
            raise ValueError('result SHA is not the task branch head')
        if not subprocess.run(['git', '-C', str(core.ROOT), 'merge-base', '--is-ancestor', sha,
                               'refs/remotes/origin/main']).returncode:
            return
        # A detached worktree lets a coordinator run from any checkout, including a dirty one.
        with tempfile.TemporaryDirectory(prefix='taskq-review-') as folder:
            tree = str(Path(folder) / 'tree')
            git('worktree', 'add', '-q', '--detach', tree, 'refs/remotes/origin/main')
            try:
                # main moved since review: replay the reviewed commits on it (no conflict = same change), no round trip
                if subprocess.run(['git', '-C', tree, 'merge', '--ff-only', sha], capture_output=True).returncode:
                    subprocess.run(['git', '-C', tree, 'checkout', '-q', '--detach', sha], check=True, capture_output=True, text=True)
                    subprocess.run(['git', '-C', tree, 'rebase', '-q', 'refs/remotes/origin/main'], check=True,
                                   capture_output=True, text=True)
                head = subprocess.run(['git', '-C', tree, 'rev-parse', 'HEAD'], check=True, capture_output=True, text=True).stdout.strip()
                git('push', 'origin', f'{head}:refs/heads/main')
                if head != sha:
                    git('push', '-f', 'origin', f'{head}:refs/heads/{branch}')  # the branch stays the published commits
                    print(f'Rebased {sha[:7]} onto main as {head[:7]} (main moved since review).')
            finally:
                git('worktree', 'remove', '--force', tree)  # a failed rebase leaves it dirty
    except (subprocess.CalledProcessError, ValueError) as error:
        detail = core.last_line(error.stderr or error.stdout) if isinstance(error, subprocess.CalledProcessError) else str(error)
        core.fail(f'Publication refused: {detail}. Keep this review result and resolve the remote state before retrying close.')


def retire_local(current):
    """After close, what the coordinator did by hand (#41): a local claim's session, then the task's tree by
    [workspace] retire and its merged branch `taskq-<N>`. One printed line per step; a failure never stops close."""
    claim, iid = current['claim'] or {}, current['iid']

    def step(what, action):
        try:
            with contextlib.redirect_stdout(io.StringIO()) as said:  # codex_archive prints its own line
                done = action()
            print(f'{what}: {done or said.getvalue().strip()}')
        except SystemExit as error:  # fail(): its message is the line, e.g. codex-archive's computer-use recipe
            print(f'{what}: {str(error).removeprefix("taskq: ")}')
        except Exception as error:  # noqa: BLE001 - one line, never a traceback after the task is closed
            print(f'{what}: failed: {core.last_line(str(error)) or type(error).__name__}')

    def run(argv, **kwargs):
        done = subprocess.run(argv, cwd=core.ROOT, capture_output=True, text=True, timeout=120, **kwargs)
        return 'done' if not done.returncode else f'failed: {core.last_line(done.stderr + done.stdout)}'

    session = claim.get('session')
    if session and core.local_claim(claim):
        from taskq.runtimes import get
        step('session', lambda: (get(claim['runtime']).close(session), f'retired {session}')[1]
             if claim['runtime'] == 'claude' else get(claim['runtime']).close(session))
    elif session:
        print(f'session: {claim.get("runtime")}:{session} is on another machine; retire it there')
    if current['type'] not in ('code', 'docs'):
        return
    if core.RETIRE:
        step('worktree', lambda: run(core.RETIRE.format(iid=iid), shell=True))
    branch = f'taskq-{iid}'
    if not subprocess.run(['git', 'rev-parse', '--verify', '-q', f'refs/heads/{branch}'], cwd=core.ROOT, capture_output=True).returncode:
        step(f'branch {branch}', lambda: run(['git', 'branch', '-d', branch]))


def problem(args):
    """On the task's issue; without a task, an issue of its own that `tick` names until someone closes it."""
    if args.task:
        return core.note(args.task, 'problem', args.text)
    issue = core.api('POST', 'issues', {'title': f'{core.PROBLEM}: {args.text.splitlines()[0][:80]}', 'labels': core.PROBLEM,
                                   'description': f'**{core.PROBLEM}** · {core.who()}\n\n{args.text}'})
    print(f'#{issue["iid"]} {issue["web_url"]}')


def report(args):
    """Where the time went and what went wrong, from the taskq notes of issues changed in the last hours."""
    since = time.time() - args.hours * 3600
    after = time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(since))
    from concurrent.futures import ThreadPoolExecutor
    found, tasks, changed = [], set(), core.issues(f'state=all&updated_after={after}')
    # One request per changed issue, ~1.3 s each through glab: 8 at once (192 issues: 255 s alone).
    with ThreadPoolExecutor(8) as pool:
        histories = list(pool.map(lambda issue: core.comments(issue['iid']), changed))
    for issue, history in zip(changed, histories):
        if core.BLOCK.search(issue.get('description') or ''):
            tasks.add(issue['iid'])
        # A task-less problem is its issue's description.
        own = [{'body': issue['description'], 'created_at': issue['created_at']}] if core.PROBLEM in issue['labels'] else []
        for item in own + history:
            head = re.match(r'\*\*(.+?)\*\* · (\S+)', item['body'] or '')
            if head and core.stamp(item['created_at']) >= since:
                found.append({'task': issue['iid'], 'at': core.stamp(item['created_at']), 'action': head[1],
                              'who': head[2], 'text': item['body'].split('\n\n', 1)[-1]})
    found.sort(key=lambda event: event['at'])
    if hasattr(args, 'output'):
        args.output.update(actions=found, tasks=sorted(tasks), sessions=sorted({event['who'] for event in found}))
    print(f'# Tasks, last {args.hours} h (minutes spent before each step)')
    for iid in sorted({event['task'] for event in found} & tasks):
        steps = [event for event in found if event['task'] == iid and event['action'] not in ('beat', 'shown')]
        print(f'#{iid}: ' + ' → '.join(event['action'] + (f' +{int(event["at"] - before["at"]) // 60}m' if before else '')
                                       for before, event in zip([None] + steps, steps)))
    print('\n# Sessions (first to last note)')
    for session in sorted({event['who'] for event in found}):
        mine = [event for event in found if event['who'] == session]
        print(f'{session}  {int(mine[-1]["at"] - mine[0]["at"]) // 60} min, {len(mine)} notes, '
              f'tasks {sorted({event["task"] for event in mine if event["task"] in tasks})}, {sum(event["action"] == "problem" for event in mine)} problems')
    print('\n# Problems')
    for event in found:
        if event['action'] == 'problem':
            print(f'- {time.strftime("%Y-%m-%dT%H:%MZ", time.gmtime(event["at"]))} #{event["task"]} {event["who"]}: {event["text"]}')


def spawn(args):
    """Create a worker session in the main checkout that starts on `--text` (the worker prompt) and print its id.
    Claude: a CLI background session (`claude_spawn`). Codex: `codex_spawn`. Without `--text` the session is idle.
    #268: the name is `<T|S><N> <ORCH> <title> (<machine>)` (`session_name`): the owner sees which orchestrator
    launched each worker and where it runs. No `@`: SendMessage
    rejects a name containing it as a name@team address. A task's worker (`T<N> …`) gets a `launch` note naming
    its session: the binding `take` checks for a supervised task. #243: a supervisor (`S<N> …`) is written into
    its task's block (`assign`)."""
    from taskq.tick import worker_iid, supervisor_iid
    from taskq.runtimes import get, session_name
    name = session_name(args.name, core.machine())
    iid, supervised = worker_iid(args.name), supervisor_iid(args.name)
    if supervised:
        unsupervised(core.task(supervised))  # #243: a supervisor (`S<N> …`) only for a ready task without one
    if iid and (found := core.task(iid).get('supervisor')) and not core.is_caller(found):
        core.fail(f'#{iid} is supervised by {core.short(found)}: only that session launches its worker')
    session = get(args.runtime, full_access=getattr(args, 'full_access', False), remote_control=args.remote_control).spawn(name, args.text)
    if iid:
        core.note(iid, 'launch', f'session {session}')
    if supervised:
        assign(supervised, args.runtime, session)
    if args.runtime == 'codex':
        print(session)
    else:
        print(f'{session}\nWatch it: `claude attach {session[:8]}` or `claude agents`.')
    return session


def unsupervised(current):
    """#243: refuse a supervisor spawn for a task that is not ready or already has one."""
    if current['state'] != 'ready' or current.get('supervisor'):
        core.fail(f'#{current["iid"]} gets no new supervisor: it is {current["state"]}'
                  + (f', supervised by {core.short(current["supervisor"])}' if current.get('supervisor') else ''))


def assign(iid, runtime, session):
    """#243: the supervisor session spawn just started goes into the block, on a fresh read: the dispatcher's own
    assignment, the one `edit --supervisor` exception (cooperative authority, like claims). A task that got a
    supervisor or left ready meanwhile keeps it, and the new session is retired."""
    current = core.task(iid)
    if current['state'] != 'ready' or current.get('supervisor'):
        with contextlib.suppress(Exception, SystemExit):
            from taskq.runtimes import get
            get(runtime).close(session)
        core.fail(f'#{iid}: it changed while its supervisor started; that session was retired')
    found = {'runtime': runtime, 'session': session}
    core.save(current, supervisor=found, note_action='edit', note_text=f'supervisor none → {identity(found)} (spawned)')


def resumed(current, new):
    """#284: supervisor `new` is the block's supervisor resumed under a new session id: the block takes it."""
    core.save(current, supervisor=new, note_action='edit', note_text=f'supervisor {identity(current["supervisor"])} → {identity(new)} (resumed)')
    current['supervisor'] = new


def adopt(current):
    """#284: a woken Claude supervisor runs under a new session id; its transcript proves it the same supervisor."""
    found, mine = current.get('supervisor'), core.session()
    if found and mine and found['runtime'] == mine['runtime'] == 'claude' and forked(found['session'], mine['session']):
        resumed(current, mine)


def delegation(current, uid):
    """A task assigned to someone else runs only under that owner's explicit delegation; taskq has no such policy yet."""
    assigned = current.get('assignees') or []
    return None if not assigned or uid in assigned else 'assigned to another user; delegation needs an owner-verified policy and none is configured'


def claude_env(extra=None):
    env = {**{key: value for key, value in os.environ.items() if key not in core.RUNTIMES.values()}, **(extra or {})}
    # #148: a Windows claude under WSL runs its Bash tool through wsl.exe, which passes on only the variables WSLENV
    # names: without CLAUDE_CODE_SESSION_ID there `taskq worker` has no session identity (csgo #316).
    if core.windows_claude_binary() and 'CLAUDE_CODE_SESSION_ID' not in [item.split('/')[0] for item in env.get('WSLENV', '').split(':')]:
        env['WSLENV'] = ':'.join(filter(None, [env.get('WSLENV'), 'CLAUDE_CODE_SESSION_ID']))
    return env


# #38 (2026-10-06, verified live): a worker needs only these tools and no MCP. settings.local.json stays as is
# (the coordinator shares it), so every worker run is narrowed at its start. --strict-mcp-config leaves the
# built-in claude-in-chrome server on: --no-chrome drops it (#51, seen live; EndConversation always stays).
# `--tools` takes several values: keep a flag after it, never the prompt. A `--resume` cannot take them (see
# Selftest.full), so only a spawned run is narrowed.
# #71: the mode is pinned too, else the user's defaultMode (`auto`) applies and its classifier stops `taskq` commands.
CLAUDE_WORKER_TOOLS = ['--permission-mode', core.PERMISSION_MODE, '--tools', 'Bash,Read,Edit,Write,Glob,Grep,WebFetch,WebSearch', '--strict-mcp-config', '--no-chrome']


def claude_spawn(name, extra=None, prompt=None, remote_control=True):
    """The CLI session id of a new Claude worker: a `claude --bg` session named `name` (idle without
    `prompt`), which SendMessage reaches by that name. #270 (2026-10-06): no app window change at all.
    #83 (owner's decision 2026-10-07): Remote Control on, so the worker has an https link (`claude_url`) and its
    questions show in the app and web. It also shows in the owner's apps on other machines (csgo #303): the
    name says the machine. Off: `remoteControlAtStartup: false` (`/rc connecting…` gone, checked live)."""
    # A `--resume` keeps only --name and --settings (#51): the mode goes into --settings as well.
    settings = {'permissions': {'defaultMode': core.PERMISSION_MODE}, **({} if remote_control else {'remoteControlAtStartup': False})}
    no_live_in_tests('claude --bg spawn')
    done = subprocess.run(['claude', '--bg', *CLAUDE_WORKER_TOOLS, '--name', name, '--settings', json.dumps(settings), *([prompt] if prompt else [])], cwd=core.ROOT,
                          env=claude_env(extra), capture_output=True, text=True, timeout=120)
    # FORCE_COLOR in the caller's environment colours the id (seen live 2026-10-06): strip ANSI before matching.
    short = re.search(r'backgrounded · (\w+)', re.sub(r'\x1b\[[0-9;]*m', '', done.stdout))
    if done.returncode or not short:
        core.fail(f'claude could not start the session: {done.stderr.strip() or done.stdout.strip()}')
    session = next((sid for sid in claude_agents() if sid.startswith(short[1])), None)
    if not session:
        core.fail(f'claude agents does not list the new session {short[1]}')
    return session


CLAUDE_ENDED = ('done', 'failed', 'stopped')  # `claude agents` states with no way back but a resume


AGENT_TEXT = ('id', 'cwd', 'kind', 'sessionId', 'name', 'state', 'status')  # `claude agents --json` fields taskq reads


def claude_agents(strict=False):
    """This machine's `claude --bg` sessions by session id, stopped ones too (no `pid`). `strict` (#185): None when
    the CLI is there but its list could not be read, which is unknown, not an empty machine."""
    try:
        done = subprocess.run(['claude', 'agents', '--json', '--all'], cwd=core.ROOT, capture_output=True, text=True, timeout=60)
        listed = json.loads(done.stdout) if not done.returncode else None
    except FileNotFoundError:  # a machine without the claude CLI (CI, a Codex-only machine) has none
        listed = []
    except (OSError, subprocess.SubprocessError, ValueError):
        listed = None
    # #185: a row of another shape (null, a list, a non-string field) makes the whole list unknown, never a crash.
    if not isinstance(listed, list) or not all(isinstance(item, dict) and all(isinstance(item.get(key), (str, type(None))) for key in AGENT_TEXT)
                                               and isinstance(item.get('pid'), (int, type(None))) for item in listed):
        return None if strict else {}
    return {item['sessionId']: item for item in listed if item.get('kind') == 'background' and item.get('sessionId')}


def claude_url(session):
    """The Remote Control URL of a background session, or None (Remote Control off, not this machine). Not in
    `claude agents --json`: the job's `~/.claude/jobs/<short>/state.json` holds `bridgeSessionId` `cse_<id>`,
    the same session the URL spells `session_<id>` (the CLI's own cse_→session_ shim; CLI 2.1.291, #83)."""
    try:
        job = json.loads((core.CLAUDE_JOBS / session[:8] / 'state.json').read_text())
    except (OSError, ValueError):
        return None
    bridge = job.get('bridgeSessionId') if job.get('sessionId') == session else None
    return bridge and 'https://claude.ai/code/session_' + bridge.removeprefix('cse_').removeprefix('session_')


def claude_stop(session, remove=False):
    """Stop a background session (its conversation stays: `claude --resume` and the app can open it);
    `remove` also takes it out of `claude agents`. The CLI takes the short id, not the session id."""
    agent = claude_agents().get(session)
    for verb in ('stop', 'rm') if remove else ('stop',):
        if agent and (verb == 'rm' or agent.get('pid')):
            no_live_in_tests(f'claude {verb}')
            subprocess.run(['claude', verb, agent['id']], cwd=core.ROOT, check=True, capture_output=True, timeout=60)
    return agent


def claude_wake(session, prompt, extra=None):
    """One more turn of a background session: `claude --bg --resume <session> <prompt>`. The CLI exits 0 even when
    the new job fails at once (#72: an idle session without a transcript is 'source session … not found'), so the
    job's state in `claude agents` decides. A job alive after 15 s is left to the caller's own wait."""
    agents = claude_agents()
    before, names = set(agents), {prompt, (agents.get(session) or {}).get('name')}
    claude_stop(session)
    no_live_in_tests('claude --bg --resume')
    subprocess.run(['claude', '--bg', '--resume', session, prompt], cwd=core.ROOT, env=claude_env(extra),
                   check=True, capture_output=True, timeout=120)
    end = time.time() + 15
    while True:
        # ponytail: a new job is matched by name (the CLI names a failed one by the prompt), not by the printed id
        jobs = [agent for sid, agent in claude_agents().items() if sid not in before and agent.get('name') in names]
        if failed := next((agent for agent in jobs if agent.get('state') == 'failed'), None):
            core.fail(f'claude --resume {session[:8]}: the job {failed["id"]} failed at once (state failed in `claude agents`)')
        # #284: `--bg --resume` of a stopped session continues under a new session id; the caller records it
        if fork := next((agent['sessionId'] for agent in jobs if forked(session, agent['sessionId'])), None):
            return fork
        if time.time() > end:
            return session
        time.sleep(2)


def transcript(session):
    """The message uuids of a Claude session's transcript on this machine (`~/.claude/projects/*/<id>.jsonl`)."""
    path = next((core.CLAUDE_JOBS.parent / 'projects').glob(f'*/{session}.jsonl'), None)
    try:
        return [json.loads(line).get('uuid') for line in path.read_text().splitlines()] if path else []
    except (OSError, ValueError):
        return []


def forked(old, new):
    """#284: whether session `new` is `old` resumed under a new id: its transcript starts with a copy of the old
    one's messages, same uuids (CLI 2.1.x, seen live 2026-10-09: fa18ac97 resumed as c4b88ee4)."""
    first = next(filter(None, transcript(new)), None) if new != old else None
    return bool(first) and first in transcript(old)


def view(args):
    """A task as the queue sees it, read only: state, claim, the last notes, the result."""
    try:
        issue = core.api('GET', f'issues/{args.iid}')
    except SystemExit as error:
        if core.gone(error):
            core.fail(f'#{args.iid} not found')
        raise
    closed = issue['state'] != 'opened'  # close drops the state label
    labels = [label for label in issue['labels'] if not label.startswith(core.PREFIX)] + [core.PREFIX + core.STATES[0]] * closed
    item = core.parse({**issue, 'labels': labels if closed else issue['labels']}) or core.fail(f'#{args.iid} is not a taskq task')
    claim = item['claim'] or {}
    print(f'#{item["iid"]} {item["title"]}\nstate: ' + ('closed' if closed else item['state'])
          + f', p{item["priority"]}, runtime {item["runtime"] or "any"}, last change {core.age(item)} min ago')
    print('claim: ' + (f'{claim.get("runtime")}:{(claim.get("session") or "")[:8]}{core.where(claim)}' if claim else 'none'))
    if found := item.get('supervisor'):
        print(f'supervisor: {core.short(found)}')
    if item['state'] == 'doing' and claim.get('session'):
        from taskq.tick import liveness
        state, activity = liveness(item, claude_agents())
        print(f'worker: {state or "unknown here"}, {activity}')
    found = core.notes(core.comments(item['iid']))
    for body in found[-args.notes:]:
        print('\n---\n' + body)
    if claim:
        print('\n=== result ===\n' + core.handed_in(item))


def retire(args):
    """Archive a finished Claude background worker: stopped and out of `claude agents`; the transcript stays."""
    session = args.session.removeprefix('local_')
    print(f'retired {session}' if claude_stop(session, remove=True) else f'{session} is not a background session here')


def no_live_in_tests(what):
    """A unit test must never reach the real Claude/Codex runtime: leaked test sessions crowd the owner's apps and
    archive or stop real sessions. Patched runners (mocks) pass; the real subprocess.run under unittest fails."""
    if 'unittest' in sys.modules and subprocess.run.__module__ == 'subprocess':
        raise ConnectionRefusedError(f'taskq test touched the live runtime: {what}')
