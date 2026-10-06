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
import time

import taskq as core
BRIEF = '''You are the worker for task #{iid}. Queue tool: `{tool}`
This brief is the owner's assignment: do it without asking for confirmation.

1. Claim the task: `{tool} take {iid}`. If it refuses, another worker was faster: run `{tool} worker` once more and follow the new brief.
2. Workspace: {workspace}
3. Do the task below. Follow AGENTS.md. Expected paths: {scope}. They say where the work is expected, not
   what is forbidden: if the task needs another file, change it and name it with the
   reason in the result. Do not ask for that.
4. In long work run `{tool} beat {iid}` after each milestone.
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
    print(f'#{issue["iid"]} {issue["web_url"]}')


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
    if changes:
        core.save(current, note_action='edit', note_text='; '.join(notes), **changes)
    print(f'#{args.iid} edited')


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
    return BRIEF.format(**{**current, 'tool': core.TOOL, 'rules': core.RULES, 'deliver': DELIVER[pushes],
                           'workspace': core.WORKSPACE[kind].format(iid=current['iid']),
                           'sha': ' --sha <pushed commit>' if pushes else '',
                           'scope': ', '.join(current['scope']) or 'none',
                           'notes': ('\n\n---\n\n'.join(core.notes(kept)) or 'none') + omitted})


def worker(args):
    """What a fresh worker session runs first: the brief of the first task that can start now."""
    loaded, candidates = core.profile(args)
    runtime = core.me()['runtime']
    free = core.room(loaded[0], args.profile['limits'])
    found = [item for item in candidates if item['state'] == 'ready' and free[runtime] > 0
             and not core.refusal(item, loaded[0], loaded[1], runtime)]
    print(brief(found[0]).replace(f'{core.TOOL} worker`', f'{core.TOOL} worker{core.profile_arguments(args)}`')
          if found else 'No task can start now. Say so and stop.')


def take(args):
    """Check the admission rule, set the lock, move the task to doing. The lock decides one task;
    Scope is a rule between tasks, so after the move the newer of two overlapping takes gives way."""
    mine = core.me()
    everything, open_iids, *_ = core.load()
    current = next((item for item in everything if item['iid'] == args.iid), None) or core.fail(f'#{args.iid} is not an open taskq task')
    claim = current['claim'] or {}
    if current['state'] == 'doing' and (claim.get('runtime'), claim.get('session')) == (mine['runtime'], mine['session']):
        core.note(args.iid, 'take')  # a repeated take of its own task, e.g. after an answer in the session
        return print(f'#{args.iid} is yours')
    reason = f'state is {current["state"]}' if current['state'] != 'ready' else core.refusal(current, everything, open_iids, mine['runtime'])
    if reason:
        core.fail(f'#{args.iid} cannot start: {reason}')
    if not core.lock(args.iid):
        core.fail(f'#{args.iid} cannot start: another worker holds its lock')
    try:
        core.save(current, 'doing', claim=mine, result=None, waiting_for=None, assignee_ids=[core.user()])
    except BaseException:
        # A lock left behind refuses every later take of this task until tick clears it (#105).
        with contextlib.suppress(Exception, SystemExit):
            core.unlock(args.iid)
        raise
    held = {**current, 'state': 'doing'}
    # A task without paths overlaps nothing: no second read.
    rivals = current['scope'] and [other for other in core.load()[0] if other['iid'] != args.iid and (other['claim'] or {}).get('session')
              and core.overlap(current['scope'], other['scope'])]
    if rivals:
        # Each take moves its task before it reads, so the later of two sees the earlier; both order them alike.
        since = doing_since(args.iid)
        older = [other for other in rivals if (doing_since(other['iid']), other['iid']) < (since, args.iid)]
        if older:
            core.save(held, 'ready', claim=current['claim'], result=current['result'], waiting_for=current['waiting_for'],
                 assignee_ids=current.get('assignees', []))
            core.unlock(args.iid)
            core.fail(f'#{args.iid} cannot start: scope overlaps #{older[0]["iid"]}')
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
    if args.action == 'answer' and claim.get('session') and {key: claim.get(key) for key in ('runtime', 'session')} == core.session():
        # The owner answered in the worker's own session: it continues with its claim. Its doing place
        # was free while the task waited in ask, so the limit is not checked: the worker never left.
        core.save(current, 'doing', 'answer', args.text, waiting_for=None)
        return print(f'#{args.iid} is doing again with your claim: continue in this session')
    # An empty claim marks a started task: it keeps its paths and its next worker continues.
    claim = current['claim'] and {'runtime': None, 'session': None}
    core.save(current, 'ready', args.action, args.text, waiting_for=None, result=None, claim=claim)
    core.unlock(args.iid)


def close(args):
    current = core.task(args.iid, ('review',))
    if current['type'] in ('code', 'docs'):
        try:
            sha = core.commit(current['result']['sha'])
        except argparse.ArgumentTypeError as error:
            core.fail(f'{error}; reject the task so the worker hands in the pushed commit')
        subprocess.run(['git', 'fetch', '-q', 'origin', 'main'], check=True)
        if subprocess.run(['git', 'merge-base', '--is-ancestor', sha, 'origin/main']).returncode:
            core.fail(f'{sha} is not in origin/main; reject the task so the worker pushes it')
    core.save(current, close=True, note_action='close', note_text=args.text)
    core.unlock(args.iid)
    print(f'#{args.iid} closed')
    if current['type'] in ('code', 'docs'):
        # Author, date and subject show whether the commit is this task's.
        subprocess.run(['git', 'log', '-1', '--format=%h %an %ad %s', sha], check=False)
    retire_local(current)


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
    rejects a name containing it as a name@team address."""
    name = args.name if args.name.endswith(f' ({core.machine()})') else f'{args.name} ({core.machine()})'
    if args.runtime == 'codex':
        return print(core.codex_spawn(name, args.text))
    if args.runtime in core.EXECUTORS:
        session = executor_run(args.runtime, 'spawn', name=name)
        if args.text:
            executor_run(args.runtime, 'send', session=session, text=args.text)
        return print(session)
    session = claude_spawn(name, prompt=args.text, remote_control=args.remote_control)
    print(f'{session}\nWatch it: `claude attach {session[:8]}` or `claude agents`; in the app: `{core.TOOL} show {session}`.')


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
    return {**{key: value for key, value in os.environ.items() if key not in core.RUNTIMES.values()}, **(extra or {})}


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


def claude_agents():
    """This machine's `claude --bg` sessions by session id, stopped ones too (no `pid`)."""
    try:  # a machine without the claude CLI (CI, a Codex-only machine) has none
        done = subprocess.run(['claude', 'agents', '--json', '--all'], cwd=core.ROOT, capture_output=True, text=True, timeout=60)
        listed = json.loads(done.stdout) if not done.returncode else []
    except (OSError, subprocess.SubprocessError, ValueError):
        listed = []
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
    issue = core.api('GET', f'issues/{args.iid}')
    closed = issue['state'] != 'opened'  # close drops the state label
    labels = [label for label in issue['labels'] if not label.startswith(core.PREFIX)] + [core.PREFIX + core.STATES[0]] * closed
    item = core.parse({**issue, 'labels': labels if closed else issue['labels']}) or core.fail(f'#{args.iid} is not a taskq task')
    claim = item['claim'] or {}
    print(f'#{item["iid"]} {item["title"]}\nstate: ' + ('closed' if closed else item['state'])
          + f', p{item["priority"]}, runtime {item["runtime"] or "any"}, last change {core.age(item)} min ago')
    print('claim: ' + (f'{claim.get("runtime")}:{(claim.get("session") or "")[:8]}{core.where(claim)}' if claim else 'none'))
    found = core.notes(core.comments(item['iid']))
    for body in found[-args.notes:]:
        print('\n---\n' + body)
    if claim:
        print('\n=== result ===\n' + core.handed_in(item))


def show(args):
    """Open a Claude session in the desktop app on the owner's request. A running background session is
    stopped first: the app does not refuse it and would be a second writer of the same transcript."""
    session = args.session.removeprefix('local_')
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
