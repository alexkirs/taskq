"""The task commands: `add`, `list`, `edit`, the worker's `worker`, `take`, `beat`, `ask`, `result`, the
coordinator's `answer`, `reject`, `release`, `close`; worker sessions (`spawn`, `send`, `show`, `retire`)."""
import argparse
import contextlib
import io
import json
import os
from pathlib import Path
import re
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

# {title}

{text}

# History (oldest first)

{notes}
'''
DELIVER = {
    True: 'Deliver: commit, `git fetch origin && git rebase origin/main`, run the checks, `git push origin HEAD:main` (never force).',
    False: 'Deliver: put the whole answer into `--text`.',
}

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
    if args.supervisor is not None and (found := supervisor_change(current, args.supervisor)) != current.get('supervisor'):
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
        if runtime not in (*core.RUNTIMES, *core.EXECUTORS) or not session:
            core.fail(f'--supervisor: write RUNTIME:SESSION with the full session id, RUNTIME one of {", ".join((*core.RUNTIMES, *core.EXECUTORS))}')
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
    found = current.get('supervisor')
    if found and core.session() is not None and not core.is_caller(found):
        core.fail(f'#{current["iid"]} is supervised by {core.short(found)}: only that session or the owner decides')


def supervision(current, mine):
    """#240: why session `mine` cannot take supervised `current`, or None. Allowed: the worker adopting a reservation
    of its supervisor, or the worker of the supervisor's newest launch (the same worker after a queue answer)."""
    found = current.get('supervisor')
    if not found:
        return None
    if (mine['runtime'], mine['session']) == (found['runtime'], found['session']):
        return 'its supervisor does not take it'
    reservation = current['state'] == 'ready' and current.get('reservation')
    if reservation:
        return None if reservation.get('coordinator') == core.short(found) else \
            f'reserved by {reservation.get("coordinator")}, not by its supervisor {core.short(found)}'
    return None if supervised_worker(current['iid'], found) == mine['session'] else \
        f'supervised by {core.short(found)}: only the worker it launched takes it'


def supervised_worker(iid, found):
    """The session of the newest `launch` note of an attempt that supervisor `found` reserved, or None."""
    attempts, session, who = set(), None, core.short(found)
    for body in core.notes(core.comments(iid)):
        head, _, text = body.partition('\n\n')
        if head == f'**reserve** · {who}' and (match := re.match(r'Attempt (\w+):', text)):
            attempts.add(match[1])
        elif head.startswith('**launch**') and (match := re.match(r'Attempt (\w+) launched session (\S+)$', text)) and match[1] in attempts:
            session = match[2]
    return session


def acknowledged(iid, found):
    """Whether supervisor `found` acted since its assignment: a reserve, answer, reject or release by that session."""
    done = False
    for body in core.notes(core.comments(iid)):
        head, _, text = body.partition('\n\n')
        if head.startswith('**edit**') and f'→ {identity(found)}' in text:
            done = False
        elif head in (f'**{action}** · {core.short(found)}' for action in ('reserve', 'answer', 'reject', 'release')):
            done = True
    return done


def later(args):
    """The owner defers a task; nobody waits on anything. Back with `answer` or by hand to ready."""
    current = core.task(args.iid, ('ready', 'waiting', 'ask'))
    outsider(current)
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
                           'notes': ('\n\n---\n\n'.join(core.notes(kept)) or 'none') + omitted})


def worker(args):
    """What a fresh worker session runs first: the brief of the first task that can start now."""
    loaded, candidates = core.profile(args)
    mine = core.me()
    runtime = mine['runtime']
    # #240: a session that holds a doing claim here continues that task; it never selects fresh work.
    own = [item for item in loaded[0] if item['state'] == 'doing' and core.local_claim(item['claim'] or {})
           and ((item['claim'] or {}).get('runtime'), (item['claim'] or {}).get('session')) == (runtime, mine['session'])]
    if own:
        return print(f'No task can start now for this session: it holds #{own[0]["iid"]} (doing, your claim). '
                     f'Continue that task; do not take another. Its brief:\n\n{brief(own[0])}')
    free = core.room(loaded[0], args.profile['limits'])
    # #208: the worker a coordinator launched for a reserved task gets that task; its place is already counted.
    found = [item for item in loaded[0] if item['state'] == 'ready' and item.get('reservation') and adopts(item, mine)] or [
        item for item in candidates if item['state'] == 'ready' and free[runtime] > 0
        and not core.refusal(item, loaded[0], loaded[1], runtime) and not core.sandbox_refusal(item, runtime)]
    print(brief(found[0]).replace(f'{core.TOOL} worker`', f'{core.TOOL} worker{core.profile_arguments(args)}`')
          if found else 'No task can start now. Say so and stop.')


def take(args):
    """Check the admission rule, set the lock, check it again on a read under the lock, move the task to doing.
    The lock decides one task; scope is a rule between tasks, so after the move the newer of two overlapping
    takes gives way. #208: a reserved task goes only to the worker its coordinator launched, which adopts the
    reservation's lock; a refused adoption leaves the reservation and its lock to their owner."""
    mine, uid = core.me(), core.user()

    def read():
        everything, open_iids, *_ = core.load()
        current = next((item for item in everything if item['iid'] == args.iid), None) or core.fail(f'#{args.iid} is not an open taskq task')
        reserved = current['state'] == 'ready' and current.get('reservation')
        return current, (f'state is {current["state"]}' if current['state'] != 'ready' else
                         (core.refusal(current, everything, open_iids) if reserved and not adopts(current, mine) else None)
                         or core.refusal({**current, 'reservation': None}, everything, open_iids, mine['runtime'])
                         or core.sandbox_refusal(current, mine['runtime']) or delegation(current, uid)
                         or supervision(current, mine))
    current, reason = read()
    claim = current['claim'] or {}
    if current['state'] == 'doing' and (claim.get('runtime'), claim.get('session')) == (mine['runtime'], mine['session']):
        core.note(args.iid, 'take')  # a repeated take of its own task, e.g. after an answer in the session
        return print(f'#{args.iid} is yours')
    if reason:
        core.fail(f'#{args.iid} cannot start: {reason}')
    attempt = (current.get('reservation') or {}).get('attempt')
    locked = not (attempt and core.locks(args.iid))  # this take sets the lock, unless it adopts the reservation's
    if locked and not core.lock(args.iid):
        core.fail(f'#{args.iid} cannot start: another worker holds its lock')
    try:
        # An assignment, dependency, runtime, host or reservation changed between the first read and the lock refuses here.
        fresh, reason = read()
        if not reason and (fresh.get('reservation') or {}).get('attempt') != attempt:
            reason = 'its reservation changed since this take read it'
        if reason:
            core.fail(f'#{args.iid} cannot start: {reason}')
        # The assignee stays untouched; only an unassigned task becomes the taker's, as before.
        assign = {} if fresh['assignees'] else {'assignee_ids': [uid]}
        core.save(fresh, 'doing', claim=mine, result=None, waiting_for=None, reservation=None, **assign)
    except BaseException:
        # A lock left behind refuses every later take of this task until tick clears it (#105). The lock of a
        # reservation stays with it: it is its coordinator's to settle; so does one this take set, when the read
        # back shows any owner (#208).
        if locked:
            with contextlib.suppress(Exception, SystemExit):
                unlock_unowned(core.task(args.iid))
        raise
    held = {**fresh, 'state': 'doing', 'reservation': None}
    # A task without paths overlaps nothing: no second read. A reserved rival is older than any take (#208).
    rivals = fresh['scope'] and [other for other in core.load()[0] if other['iid'] != args.iid
             and ((other['claim'] or {}).get('session') or other.get('reservation')) and core.overlap(fresh['scope'], other['scope'])]
    if rivals:
        # Each take moves its task before it reads, so the later of two sees the earlier; both order them alike.
        since = doing_since(args.iid)
        older = [other for other in rivals if (0 if other.get('reservation') else doing_since(other['iid']), other['iid']) < (since, args.iid)]
        if older:
            core.save(held, 'ready', claim=fresh['claim'], result=fresh['result'], waiting_for=fresh['waiting_for'],
                      **({'assignee_ids': []} if assign else {}))
            core.unlock(args.iid)
            core.fail(f'#{args.iid} cannot start: scope overlaps #{older[0]["iid"]}')
    if attempt:
        principal = fresh['reservation'].get('principal')
        core.note(args.iid, 'take', f'Adopted reservation {attempt} of {fresh["reservation"].get("coordinator")}'
                  + (f'; execution principal {uid}, reserved by {principal}' if principal != uid else ''))
    else:
        core.note(args.iid, 'take')
    print(f'#{args.iid} is yours')


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
    if getattr(args, 'function', None) is requeue:
        outsider(current)  # the tick's dead/stalled release (its own command) keeps its evidence path
    before = args.action == 'release' and releases(args.iid)
    if current.get('reservation'):
        # #208: only the reserving user releases, and only the attempt it read: the last read before the write.
        uid, found = core.user(), current['reservation']
        current = core.task(args.iid)
        if current.get('reservation') != found:
            core.fail(f'#{args.iid}: its reservation changed since this release read it; read it again')
        if found.get('principal') != uid:
            core.fail(f'#{args.iid} is reserved by user {found.get("principal")}: only that user releases it')
    # An empty claim marks a started task: it keeps its paths and its next worker continues.
    claim = current['claim'] and {'runtime': None, 'session': None}
    if before:
        # #157: a worker that fails the same way (Blender in a Codex sandbox) would take, crash and release on
        # every tick; the second release in a row without an owner's answer or reject goes to the owner instead.
        core.save(current, 'ask', 'release', args.text, waiting_for=None, result=None, claim=claim, reservation=None)
        return core.note(args.iid, 'ask', f'Released {len(before) + 1} times in a row, the last because: {args.text}\n'
                         f'Earlier: {before[-1]}\nDecide how it can run (runtime, access, a fix first) and answer.')
    core.save(current, 'ready', args.action, args.text, waiting_for=None, result=None, claim=claim, reservation=None)
    core.unlock(args.iid)


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
        recover_closed(issue)
        print(f'#{args.iid} already closed')
        return
    current = core.task(args.iid, ('review',))
    if core.is_caller(current.get('supervisor')):
        core.fail(f'#{args.iid}: its supervisor does not close it; the publication lane closes after review')
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
    receipt = json.dumps({'claim': current['claim'], 'result': current['result']}, sort_keys=True)
    core.save(current, close=True, note_action='close', note_text=f'{args.text}\n\nTaskQ receipt: `{receipt}`')
    core.unlock(args.iid)
    print(f'#{args.iid} closed')
    if current['type'] in ('code', 'docs'):
        # Author, date and subject show whether the commit is this task's.
        subprocess.run(['git', 'log', '-1', '--format=%h %an %ad %s', sha], check=False)
    retire_local(current)


def recover_closed(issue):
    """Retry only a close receipt that still names this exact local, published result."""
    labels = [label for label in issue['labels'] if not label.startswith(core.PREFIX)] + [core.PREFIX + core.STATES[0]]
    current = core.parse({**issue, 'labels': labels})
    if not current or current['type'] not in ('code', 'docs') or not current.get('result'):
        return
    receipt = next((body.rsplit('`', 2)[1] for body in reversed(core.notes(core.comments(current['iid'])))
                    if body.startswith('**close**') and '\n\nTaskQ receipt: `' in body), None)
    try:
        receipt = json.loads(receipt)
    except (TypeError, ValueError):
        return
    if receipt != {'claim': current['claim'], 'result': current['result']} or not core.local_claim(current['claim'] or {}):
        return
    try:
        sha = core.commit(current['result']['sha'])
        subprocess.run(['git', 'fetch', '-q', 'origin', 'main'], check=True)
    except (argparse.ArgumentTypeError, subprocess.CalledProcessError):
        return
    if subprocess.run(['git', 'merge-base', '--is-ancestor', sha, 'origin/main']).returncode:
        return
    session, runtime = current['claim'].get('session'), current['claim'].get('runtime')
    if runtime == 'claude':
        agents = claude_agents(strict=True)
        agent = agents.get(session) if agents is not None else None
        if not (isinstance(agent, dict) and agent.get('sessionId') == session and agent.get('kind') == 'background'
                and agent.get('cwd') == str(core.ROOT) and agent.get('state') in CLAUDE_ENDED
                and agent.get('status') in (None, 'idle') and agent.get('pid') is None):
            return
    elif runtime == 'codex':
        if not core.codex_is_archived(session):
            return
    else:
        return
    latest = next((body.rsplit('`', 2)[1] for body in reversed(core.notes(core.comments(current['iid'])))
                   if body.startswith('**close**') and '\n\nTaskQ receipt: `' in body), None)
    fresh = core.api('GET', f'issues/{current["iid"]}')
    if fresh['state'] == 'opened':
        return
    labels = [label for label in fresh['labels'] if not label.startswith(core.PREFIX)] + [core.PREFIX + core.STATES[0]]
    fresh = core.parse({**fresh, 'labels': labels})
    if not fresh or fresh['claim'] != current['claim'] or fresh['result'] != current['result'] or not core.local_claim(fresh['claim'] or {}):
        return
    try:
        if json.loads(latest) != {'claim': fresh['claim'], 'result': fresh['result']}:
            return
    except (TypeError, ValueError):
        return
    retire_local(fresh)


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
                subprocess.run(['git', '-C', tree, 'merge', '--ff-only', sha], check=True,
                               capture_output=True, text=True)
                git('push', 'origin', f'{sha}:refs/heads/main')
            finally:
                git('worktree', 'remove', tree)
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
        if claim.get('runtime') == 'claude':
            step('session', lambda: f'retired {session}' if claude_stop(session, remove=True) else f'{session} is not a background session here')
        elif claim.get('runtime') == 'codex':
            step('session', lambda: core.codex_archive(argparse.Namespace(thread=session)))
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
    The name ends with ` (<machine>)`: the owner sees where each worker runs. No `@`: SendMessage
    rejects a name containing it as a name@team address. #208: a task's worker (`T<N> …`) starts only after
    `reserve` won task N; a failed launch with no worker releases the reservation, an unknown one keeps it."""
    from taskq.tick import worker_iid
    name = args.name if args.name.endswith(f' ({core.machine()})') else f'{args.name} ({core.machine()})'
    iid = worker_iid(args.name)
    attempt = iid and reserve(iid, args.runtime, getattr(args, 'limits', None))
    try:
        if args.runtime == 'codex':
            session = core.codex_spawn(name, args.text, getattr(args, 'full_access', False))
        elif args.runtime in core.EXECUTORS:
            session = executor_run(args.runtime, 'spawn', name=name)
            if attempt:
                launched_note(iid, attempt, session)  # before its first turn: no worker can take sooner
            if args.text:
                executor_run(args.runtime, 'send', session=session, text=args.text)
        else:
            session = claude_spawn(name, prompt=args.text, remote_control=args.remote_control)
    except BaseException as error:
        if attempt:
            failed_launch(iid, attempt, error)
        raise
    if attempt and args.runtime not in core.EXECUTORS:
        launched_note(iid, attempt, session)
    if args.runtime in ('codex', *core.EXECUTORS):
        print(session)
    else:
        print(f'{session}\nWatch it: `claude attach {session[:8]}` or `claude agents`; in the app: `{core.TOOL} show {session}`.')
    return session


# --- #208: a launch reserves its task first ------------------------------------------------------

def delegation(current, uid):
    """A task assigned to someone else runs only under that owner's explicit delegation; taskq has no such policy yet."""
    assigned = current.get('assignees') or []
    return None if not assigned or uid in assigned else 'assigned to another user; delegation needs an owner-verified policy and none is configured'


def admission(iid, runtime, uid, limits):
    """Task `iid` on a fresh read of the whole queue, and why a worker of `runtime` cannot start for it (or None)."""
    everything, open_iids, *_ = core.load()
    current = next((item for item in everything if item['iid'] == iid), None) or core.fail(f'#{iid} is not an open taskq task')
    return current, (f'state is {current["state"]}' if current['state'] != 'ready' else
                     core.refusal(current, everything, open_iids, runtime) or delegation(current, uid)
                     or (current.get('supervisor') and not core.is_caller(current['supervisor'])
                         and f'supervised by {core.short(current["supervisor"])}: only that session launches its worker')
                     or (f'no Codex app server on this machine ({core.CODEX_SOCKET})' if runtime == 'codex' and not core.CODEX_SOCKET.exists() else None)
                     or (f'no free {runtime} place on this machine' if core.room(everything, limits).get(runtime, 0) <= 0 else None))


def reserve(iid, runtime, limits=None):
    """Before a worker of task `iid` starts: check admission, take the task's tracker lock, check every guard again
    on a read made under the lock, then record the attempt in its block. The lock decides between coordinators of
    any machine, user or filter; the loser starts nothing. The reservation names who reserved it, not a worker:
    none exists before the launch."""
    uid = core.user()
    limits = limits or core.resolve(argparse.Namespace(filter=None, mine=None, limit=None))[0]['limits']
    current, why = admission(iid, runtime, uid, limits)
    if current['state'] == 'ready' and current.get('reservation') and reconcile(current):
        current, why = admission(iid, runtime, uid, limits)
    if why:
        core.fail(f'#{iid} not launched: {why}')
    if not core.lock(iid):
        core.fail(f'#{iid} not launched: another coordinator or worker holds its lock')
    attempt = {'attempt': os.urandom(4).hex(), 'runtime': runtime, 'principal': uid, 'coordinator': core.who(), **core.here(),
               'pid': os.getpid(), 'at': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}
    try:
        # What changed between the first read and the lock (an assignment, a dependency, a rival) refuses here.
        fresh, why = admission(iid, runtime, uid, limits)
        if why or fresh['claim'] != current['claim']:
            core.fail(f'#{iid} not launched: {why or "its claim changed since this launch read it"}')
        core.save(fresh, reservation=attempt, note_action='reserve',
                  note_text=f'Attempt {attempt["attempt"]}: a {runtime} worker{core.where(attempt)}, principal {uid}')
        # Overlapping tasks reserved at once: each reads after its own write, so the later sees the earlier and gives way.
        rivals = fresh['scope'] and [other['iid'] for other in core.load()[0] if other['iid'] != iid and core.overlap(fresh['scope'], other['scope'])
                                     and ((other['claim'] or {}).get('session') or other.get('reservation'))]
        if rivals:
            core.fail(f'#{iid} not launched: scope overlaps #{rivals[0]}')
    except BaseException as error:
        with contextlib.suppress(Exception, SystemExit):
            abandon(iid, attempt, core.codex_line(str(error)))
        raise
    return attempt


def abandon(iid, attempt, why):
    """Undo a reservation that failed before its launch: drop the attempt if the block holds it, then this launch's
    lock. A failing read or write keeps the lock, so a written attempt is never left without it (reconcile settles
    it); another attempt found in the block keeps the lock too: it is not this launch's to undo."""
    current = core.task(iid)
    found = current.get('reservation')
    if found and found != attempt:
        return
    if found:
        core.save(current, reservation=None, note_action='launch', note_text=f'Attempt {attempt["attempt"]} released: {why}')
    unlock_unowned({**current, 'reservation': None})


def unlock_unowned(current):
    """Undo a lock this command set, judged on a read made after its failure: only when no reservation or worker
    session owns the task. An owner found there keeps the lock; a failed read (raised) keeps it too."""
    if not current.get('reservation') and not (current['claim'] or {}).get('session'):
        core.unlock(current['iid'])


def launched_note(iid, attempt, session):
    """The session a launch returned, as an append-only note: a block write here could undo the worker's take."""
    core.note(iid, 'launch', f'Attempt {attempt["attempt"]} launched session {session}')


def launch_session(iid, attempt):
    """The session the coordinator's launch note binds to `attempt`, or None (no note yet, or it failed)."""
    head = re.compile(rf'Attempt {re.escape(attempt)} launched session (\S+)$')
    for body in reversed(core.notes(core.comments(iid))):
        found = body.startswith('**launch**') and head.search(body)
        if found:
            return found[1]
    return None


def adopts(current, mine):
    """Whether session `mine` is the worker launched for `current`'s reservation: same machine and runtime, and the
    session the launch note names; before that note lands, a live `T<N>` worker of this checkout with that session."""
    from taskq.tick import launched
    found = current['reservation']
    if found.get('runtime') != mine['runtime'] or not core.local_node(found.get('node') or ''):
        return False
    bound = launch_session(current['iid'], found.get('attempt') or '')
    return bound == mine['session'] if bound else mine['session'] in (launched(current['iid'], found['runtime']) or [])


def failed_launch(iid, attempt, error):
    """A launch that raised: no worker of the task on this machine's readable inventory releases the reservation;
    otherwise its outcome is unknown and the reservation stays, as a blocker for the coordinator."""
    from taskq.tick import launched
    with contextlib.suppress(Exception, SystemExit):
        why = core.codex_line(str(error))
        # `claude --bg` answered but `claude agents` did not list it yet: a session may exist, so the outcome is unknown.
        if 'does not list the new session' not in str(error) and launched(iid, attempt['runtime']) == []:
            release_reservation({**core.task(iid), 'reservation': attempt}, f'launch failed, no worker started: {why}')
        else:
            core.note(iid, 'launch', f'Attempt {attempt["attempt"]} outcome unknown: {why}. The reservation stays; '
                      f'release it with `{core.TOOL} release {iid}` once no worker of it runs.')
            print(f'#{iid}: launch outcome unknown, reservation kept: {why}', file=sys.stderr)


def release_reservation(current, why):
    """Drop exactly `current`'s reservation and its lock, on a fresh read: the same attempt of this principal on a
    still ready task with the same claim. Anything else changed meanwhile is someone else's to settle.
    ponytail: the tracker has no compare-and-set; the fresh read narrows the window to one round trip."""
    uid = core.user()  # before the last read: nothing but the write itself follows the ownership check
    found, fresh = current['reservation'], core.task(current['iid'])
    if (fresh['state'] != 'ready' or fresh.get('reservation') != found or fresh['claim'] != current['claim']
            or found.get('principal') != uid):
        return False
    core.save(fresh, reservation=None, note_action='launch', note_text=f'Attempt {found["attempt"]} released: {why}')
    core.unlock(current['iid'])
    return True


def running(pid):
    """Whether process `pid` of this machine still runs. ponytail: Windows has no harmless signal-0 probe, so
    there a reservation waits for session evidence or an explicit release."""
    if os.name == 'nt':
        return True
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def reconcile(current, args=None):
    """A reservation found again (restart, next pass, a new launch): True when evidence released it. Only this
    machine's reservation is settled here, and only by evidence: its launching process ended and its session
    stopped, or no worker of the task is alive. Age alone never releases it; an unknown outcome keeps it."""
    from taskq.tick import launched, liveness
    found, iid, release = current['reservation'], current['iid'], None
    if not core.local_node(found.get('node') or ''):
        why = f'reserved on another machine{core.where(found)}: settled there'
    elif found.get('principal') != core.user():
        why = f'reserved by user {found.get("principal")}: settled by that user'
    elif not found.get('pid'):
        why = 'its launching process is unknown'
    elif running(found['pid']):
        why = 'its launch is still running'
    elif session := launch_session(iid, found.get('attempt') or ''):
        state, _ = liveness({**current, 'state': 'doing', 'claim': {'runtime': found['runtime'], 'session': session}}, core.claude_agents())
        release = state == 'dead' and f'its worker {session} stopped before taking the task'
        why = release or f'its worker {session} is {state or "unknown here"}'
    else:
        sessions = launched(iid, found['runtime'])
        release = sessions == [] and 'its launch was interrupted: no worker of it runs'
        why = release or ('its launch outcome is unknown' if sessions is None else f'its worker {sessions[0]} has not taken it yet')
    released = bool(release) and release_reservation(current, release)
    core.record(args, 'reservation', status='released' if released else 'kept', task=iid, reason=why)
    print(f'{core.ref(current)}: reservation {found.get("attempt")} {"released" if released else "kept"}: {why}.')
    return released


def preflight(args):
    """A real, read-only local command ACK for an external PM; no worker, claim or policy changes."""
    from taskq.tick import report_contract
    contract = report_contract()
    if hasattr(args, 'output'):
        args.output['report_contract'] = contract
    root = core.ROOT.resolve()
    blocker = None
    try:
        done = subprocess.run([sys.executable, '-c',
                               'import os; print(os.getcwd())'], cwd=root,
                              capture_output=True, text=True, timeout=30)
        code, stdout, stderr = done.returncode, done.stdout, done.stderr
    except subprocess.TimeoutExpired as error:
        code, stdout, stderr = 124, error.stdout or '', error.stderr or ''
        blocker = 'Local command timed out after 30 seconds'
    except OSError as error:
        code, stdout, stderr = 126, '', str(error)
        blocker = f'Local command could not start: {error}'
    # TimeoutExpired may retain bytes even with text=True.
    stdout, stderr = (value.decode(errors='replace') if isinstance(value, bytes) else value for value in (stdout, stderr))
    ready = code == 0 and stdout.strip() == str(root)
    acknowledgement = {'status': 'ready' if ready else 'unknown',
                       'observed_at': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
                       'cwd': str(root), 'host': core.machine(), 'exit_code': code,
                       'stdout': stdout, 'stderr': stderr, 'source': 'local subprocess',
                       'exact_blocker': None if ready else blocker or 'Local command did not acknowledge the expected cwd',
                       'runtime_capability': 'unknown', 'effective_launch_policy': 'unknown'}
    acknowledgement['report_contract'] = contract
    core.record(args, 'local_command_ack', **acknowledgement)
    print(json.dumps(acknowledgement))
    if acknowledgement['status'] != 'ready':
        core.fail('local command did not acknowledge the expected cwd; no worker started')


def runtime_status(args):
    """Read supported status only. No private rollout, resume, IPC, approval reply or worker creation."""
    observation = {'runtime': args.runtime, 'session': args.session, 'status': 'unknown',
                   'observed_at': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()), 'event_at': None,
                   'session_link': None, 'exact_blocker': 'Runtime has no qualified status source',
                   'source': 'unavailable', 'permission_requests': [], 'approval_visibility': 'unknown',
                   'notify_dedup': None, 'metadata_updated_at': None, 'effective_launch_policy': 'unknown'}
    if args.runtime == 'codex':
        observation['session_link'] = f'{core.PAGES.rstrip("/")}/open.html#codex://threads/{args.session}'
    try:
        if args.runtime == 'codex':
            codex = core.Codex()
            try:
                metadata = codex.call('thread/read', {'threadId': args.session})['thread']
                turns = codex.call('thread/turns/list', {'threadId': args.session, 'limit': 1, 'itemsView': 'notLoaded'})['data']
                if turns:
                    turns[0]['entries'] = codex.call('thread/items/list', {'threadId': args.session,
                        'turnId': turns[0]['id'], 'limit': 100, 'sortDirection': 'desc'})['data']
                stamps = [entry[key] / 1000 for turn in turns for entry in turn.get('entries', [])
                          for key in ('startedAtMs', 'completedAtMs') if entry.get(key) is not None]
                observation = {**core.codex_observation(codex, args.session, metadata['status'], turns, max(stamps, default=None)),
                               'metadata_updated_at': metadata.get('updatedAt'),  # thread recency, not an event time
                               'effective_launch_policy': 'unknown'}  # thread/read has no policy; codex-read shows the rollout's
            finally:
                codex.socket.close()
        elif args.runtime == 'claude':
            agent = core.claude_agents().get(args.session) or {}
            try:  # the job record claude_url reads too; respawnFlags are the flags the job was launched with
                job = json.loads((core.CLAUDE_JOBS / args.session[:8] / 'state.json').read_text())
            except (OSError, ValueError):
                job = {}
            job = job if job.get('sessionId') == args.session else {}
            flags = job.get('respawnFlags') or []
            mode = flags[flags.index('--permission-mode') + 1] if '--permission-mode' in flags[:-1] else None
            # Positive terminal evidence only: no pid and a terminal CLI state. Busy is not proof of execution.
            ended = not agent.get('pid') and agent.get('state') in CLAUDE_ENDED
            observation.update(source='claude agents', session_link=core.claude_url(args.session),
                               status='terminal' if ended else 'unknown',
                               runtime_state={key: agent.get(key) for key in ('status', 'state')},
                               metadata_updated_at=job.get('updatedAt'),
                               effective_launch_policy={'permission_mode': mode, 'source': 'claude job respawnFlags'}
                               if mode else 'unknown',
                               exact_blocker=None if ended else
                               'dontAsk denies unlisted tools without a prompt; CLI exposes no approval events'
                               if mode == 'dontAsk' else 'CLI status does not expose qualified pending approval events')
    except (OSError, SystemExit, ValueError) as error:
        observation['exact_blocker'] = f'Status unavailable: {core.codex_line(error)}'
    core.record(args, 'runtime_observation', **observation)
    print(json.dumps(observation))


def executor_run(runtime, verb, **values):
    """One `[runtimes.<name>]` command (`spawn`, `send`) from the main checkout; its last output line."""
    done = subprocess.run(core.selftest_command(core.EXECUTORS[runtime][verb], **values), cwd=core.ROOT, capture_output=True, text=True, timeout=300)
    if done.returncode:
        core.fail(f'{runtime} {verb}: {core.last_line(done.stderr + done.stdout)}')
    return core.last_line(done.stdout)


def send(args):
    """One turn to a worker of a `[runtimes.<name>]` app: the worker prompt, an answer, a nudge."""
    if args.runtime not in core.EXECUTORS:
        core.fail(f'{args.runtime} has its own command: Claude: SendMessage; Codex: `{core.TOOL} codex-send`')
    print(executor_run(args.runtime, 'send', session=args.session, text=args.text))


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
            subprocess.run(['claude', verb, agent['id']], cwd=core.ROOT, check=True, capture_output=True, timeout=60)
    return agent


def claude_wake(session, prompt, extra=None):
    """One more turn of a background session: `claude --bg --resume <session> <prompt>`. The CLI exits 0 even when
    the new job fails at once (#72: an idle session without a transcript is 'source session … not found'), so the
    job's state in `claude agents` decides. A job alive after 15 s is left to the caller's own wait."""
    agents = claude_agents()
    before, names = set(agents), {prompt, (agents.get(session) or {}).get('name')}
    claude_stop(session)
    subprocess.run(['claude', '--bg', '--resume', session, prompt], cwd=core.ROOT, env=claude_env(extra),
                   check=True, capture_output=True, timeout=120)
    end = time.time() + 15
    while True:
        # ponytail: a new job is matched by name (the CLI names a failed one by the prompt), not by the printed id
        jobs = [agent for sid, agent in claude_agents().items() if sid not in before and agent.get('name') in names]
        if failed := next((agent for agent in jobs if agent.get('state') == 'failed'), None):
            core.fail(f'claude --resume {session[:8]}: the job {failed["id"]} failed at once (state failed in `claude agents`)')
        if time.time() > end:
            return
        time.sleep(2)


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
        print(f'supervisor: {core.short(found)} ({"acknowledged" if acknowledged(item["iid"], found) else "unacknowledged"})')
    found = core.notes(core.comments(item['iid']))
    for body in found[-args.notes:]:
        print('\n---\n' + body)
    if claim:
        print('\n=== result ===\n' + core.handed_in(item))


def show(args):
    """Open a Claude session in the desktop app on the owner's request. A running background session is
    stopped first: the app does not refuse it and would be a second writer of the same transcript."""
    session = args.session.removeprefix('local_')
    if sys.platform != 'darwin':  # the import is `open -g claude://…`; the session keeps running
        return print(f'skipped: opening in the desktop app is macOS only; watch it: `claude attach {session[:8]}`')
    if core.permissions_missing(core.ROOT):  # #71: the app opens it in the checkout's defaultMode (else the app's, e.g. auto)
        agent = claude_agents().get(session)
        return print(f'not opened in the app: it would run there without {core.PERMISSION_MODE} (`{core.TOOL} doctor` names the fix); '
                     f'watch it read-only: `claude attach {agent["id"] if agent else session[:8]}`')
    agent = claude_stop(session)
    if agent and agent.get('pid'):
        print(f'stopped the background run {agent["id"]}: its turn ends; continue it in the app')
    claude_import(session, args.restore or driver_app_session())
    print(f'local_{session}')


def retire(args):
    """Archive a finished Claude background worker: stopped and out of `claude agents`; the transcript stays."""
    session = args.session.removeprefix('local_')
    print(f'retired {session}' if claude_stop(session, remove=True) else f'{session} is not a background session here')


def claude_import(session, restore=None):
    """Import a CLI session into the desktop app. The resume link always shows it in the main pane (no
    option to skip). With `restore` the pane goes back as soon as the app logs the focus change:
    measured 2026-10-06 ~0.2 s of the new session instead of ~1.1 s waiting for its record file."""
    log = Path.home() / 'Library/Logs/Claude/main.log'
    start = log.stat().st_size if log.exists() else 0
    subprocess.run(['open', '-g', f'claude://resume?session={session}'], check=True)
    if not restore:
        return
    end, seen = time.time() + 20, ''
    while time.time() < end and f'setFocusedSession: sessionId=local_{session}' not in seen:
        if any(core.CLAUDE_APP_SESSIONS.glob(f'*/*/local_{session}.json')):
            break  # the record is written ~1 s after the focus: fallback if the log line changes
        if log.exists():
            with log.open(errors='replace') as stream:
                stream.seek(start)
                seen = stream.read()
        time.sleep(0.02)
    restore = restore if restore.startswith('local_') else f'local_{restore}'
    subprocess.run(['open', '-g', f'claude://claude.ai/epitaxy/{restore}'], check=True)


def claude_sessions():
    """This machine's Claude app sessions by CLI session id: the app's own metadata (cwd, times, archived), never
    conversations. A `spawn` worker is one the app imported from the CLI (`adoptedFromOtherSurface`)."""
    found = {}
    for path in core.CLAUDE_APP_SESSIONS.glob('*/*/local_*.json'):
        try:
            meta = json.loads(path.read_text())
        except (OSError, ValueError):
            continue
        if meta.get('cliSessionId'):
            found[meta['cliSessionId']] = meta
    return found


def driver_app_session():
    """The app session of the calling Claude session, to show again after an import; None from Codex or a shell."""
    sid = os.environ.get(core.RUNTIMES['claude'])
    return next((meta['sessionId'] for meta in claude_sessions().values() if meta.get('cliSessionId') == sid), None) if sid else None
