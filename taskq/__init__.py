#!/usr/bin/env python3
"""taskq: the project task queue. Codex and Claude sessions use it the same way, from any machine.

A task is a GitLab issue: labels are its state, runtime and type, one JSON block in the description
is the rest of its data, the notes are its history. Nothing local stores task state.
Everything specific to a project is its `taskq.toml`. Contracts: `taskq contract`.
"""
import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import re
import shlex
import socket
import subprocess
import sys
import time
from urllib.parse import quote

# Set from the project's taskq.toml by `configure`.
PROJECT = PROJECT_PATH = ROOT = TICK_BEAT = HELPERS = None  # GitLab API prefix, `relates_to` target, main checkout
HOST = None  # GitLab host for glab; None: glab's own choice (the git remote of the current directory)
BOARD = 'taskq'
AREAS = ()
CODEX_PROJECT = CODEX_SECTION = None  # the Codex app's project and sidebar section for worker threads
WORKSPACE = {
    'continue': 'this task was started before in worktree `taskq-{iid}` (`git worktree list` shows its path); continue there. If it is gone, create it: `git worktree add -b taskq-{iid} ../taskq-{iid} origin/main`.',
    'new': 'from the main checkout run `git fetch origin && git worktree add -b taskq-{iid} ../taskq-{iid} origin/main` and work only there.',
    'none': 'this task is expected to end in an answer, not a commit: work from the main checkout. If it turns out to need file changes, make a worktree `taskq-{iid}`, work there, push like a code task and name the commit in the result text.',
}
RULES = ''  # project rules for workers, from [brief] rules: lines of step 6 of the brief
RETIRE = None  # printed after `close` of a code task: how to remove its worktree
REPO = 'https://github.com/alexkirs/taskq'  # where every install takes `main` from
UPDATE = {'auto': True, 'every': '24h'}  # [update] of taskq.toml: tick checks REPO at most this often
# A cache, not queue state: when this machine last asked REPO for its `main`.
UPDATE_STAMP = Path(os.environ.get('XDG_STATE_HOME') or Path.home() / '.local/state') / 'taskq' / 'update-last'
# Keys a taskq.toml gets, as TOML text, when it lacks them: a new version's fields reach old configs.
DEFAULTS = {'update': {'auto': 'true', 'every': '"24h"'}}
# waiting: open dependencies, moved only by `tick`; ask: a question for the owner (worker's or manager's);
# later: deferred by the owner, nobody waits on anything. Board columns in this order.
STATES = ('ready', 'waiting', 'doing', 'review', 'ask', 'later')
SUMMARY_SECONDS = 24 * 3600  # questions already shown come back as one summary this often
TYPES = ('code', 'docs', 'research', 'asset')  # the type label is the bare name
STALE_MINUTES = 120  # a `doing` issue this long without any change goes back to the queue
# The lock is this award emoji on the task's issue: GitLab lets one user award one name once (404 on the
# second). Across users the earliest reaction wins. An old lock on an unheld task is a crash's.
LOCK, LOCK_SECONDS = 'lock', 120
PROBLEM = 'problem'  # label of an issue for a problem without a task
CLEANUP_DAYS = 30  # cleanup reads open issues and the ones closed this recently, not the whole history
PREFIX, RUN = 'q-', 'run-'
PRIORITIES = (1, 2)
BLOCK = re.compile(r'<!-- taskq:start -->\s*```json\n(.*?)\n```\s*<!-- taskq:end -->', re.S)
FIELDS = ('scope', 'deps', 'claim', 'waiting_for', 'result')  # what labels cannot say
RUNTIMES = {'claude': 'CLAUDE_CODE_SESSION_ID', 'codex': 'CODEX_THREAD_ID'}
# Default: Claude for code, Codex for visual work (models, rig, animation, 3D, visual comparison).
DEFAULT_RUNTIME = {'asset': 'codex', 'code': 'claude', 'docs': 'claude', 'research': 'claude'}
TOOL = 'taskq'  # finds the project's taskq.toml from the current directory up
WORKER = None  # the worker prompt, with the main checkout: set by `configure`
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

# {title}

{text}

# History (oldest first)

{notes}
'''
DELIVER = {
    True: 'Deliver: commit, `git fetch origin && git rebase origin/main`, run the checks, `git push origin HEAD:main` (never force).',
    False: 'Deliver: put the whole answer into `--text`.',
}


def fail(message):
    sys.exit(f'taskq: {message}')


def main_checkout(start):
    """The checkout every worktree of `start` shares (its Git common dir's parent); `start` outside Git."""
    done = subprocess.run(['git', '-C', str(start), 'rev-parse', '--path-format=absolute', '--git-common-dir'],
                          capture_output=True, text=True)
    return Path(done.stdout.strip()).parent if not done.returncode else Path(start).resolve()


def configure(path=None):
    """Load the project's taskq.toml: `path`, else the nearest one from the current directory up."""
    global RULES, HOST, PROJECT, PROJECT_PATH, BOARD, AREAS, CODEX_PROJECT, CODEX_SECTION, RETIRE, HELPERS, ROOT, TICK_BEAT, WORKER
    import tomllib
    here = Path.cwd()
    path = Path(path) if path else next((folder / 'taskq.toml' for folder in (here, *here.parents)
                                         if (folder / 'taskq.toml').is_file()), None)
    if not path:
        fail('no taskq.toml in this directory or above it (README: «A new project»)')
    config = tomllib.loads(complete(path))
    gitlab, codex, workspace = config['gitlab'], config.get('codex', {}), config.get('workspace', {})
    PROJECT_PATH, HOST = gitlab['project'], gitlab.get('host')
    # `projects/:id` makes glab look the project up first: +1 s per request (measured 2026-10-06).
    PROJECT = 'projects/' + quote(PROJECT_PATH, safe='')
    BOARD = gitlab.get('board', BOARD)
    AREAS = tuple(config.get('areas', {}).get('names', ()))
    CODEX_PROJECT, CODEX_SECTION = codex.get('project'), codex.get('section')
    WORKSPACE.update({key: workspace[key] for key in WORKSPACE if key in workspace})
    RETIRE, HELPERS = workspace.get('retire'), workspace.get('cleanup_helpers')
    UPDATE.update(config.get('update', {}))
    seconds(UPDATE['every'])
    RULES = ''.join(f'   {line}\n' for line in config.get('brief', {}).get('rules', '').strip().splitlines())
    ROOT = main_checkout(path.parent)
    TICK_BEAT = ROOT / '.local' / 'taskq-tick-last'
    WORKER = f'Run `cd {ROOT} && {TOOL} worker` and follow the instructions it prints.'


def complete(path):
    """The text of `path` with every DEFAULTS key it lacked written into it; says what it added."""
    import tomllib
    text = path.read_text()
    config = tomllib.loads(text)
    for section, keys in DEFAULTS.items():
        lines = ''.join(f'{key} = {value}\n' for key, value in keys.items() if key not in config.get(section, {}))
        if not lines:
            continue
        header = re.search(rf'^\[{section}\][^\n]*\n', text, re.M)
        text = (text[:header.end()] + lines + text[header.end():] if header
                else text.rstrip('\n') + f'\n\n[{section}]\n' + lines)
        print(f'{path}: added to [{section}]: {lines.strip().replace(chr(10), "; ")}')
    if text != path.read_text():
        path.write_text(text)
    return text


def seconds(every):
    """`30m`, `24h`, `7d` in seconds."""
    found = re.fullmatch(r'(\d+)([mhd])', str(every))
    if not found:
        fail(f'[update] every = "{every}": write a number and m, h or d, e.g. "24h"')
    return int(found[1]) * {'m': 60, 'h': 3600, 'd': 86400}[found[2]]


def session():
    """This session's identity, or None for the owner's own shell."""
    return next(({'runtime': runtime, 'session': os.environ[variable]} for runtime, variable in RUNTIMES.items()
                 if os.environ.get(variable)), None)


def me():
    return {**(session() or fail('no session identity: set ' + ' or '.join(RUNTIMES.values()))), 'host': socket.gethostname()}


def who():
    return next((f'{runtime}:{os.environ[variable][:8]}' for runtime, variable in RUNTIMES.items()
                 if os.environ.get(variable)), 'owner')


def api(method, path, body=None):
    command = ['glab', 'api', '-X', method, path[1:] if path.startswith('/') else f'{PROJECT}/{path}'] + (['--hostname', HOST] if HOST else [])
    if body is not None:
        command += ['--input', '-', '-H', 'Content-Type: application/json']
    done = subprocess.run(command, input=json.dumps(body) if body is not None else None,
                          capture_output=True, text=True, timeout=60)
    if done.returncode:
        fail(f'GitLab {method} {path} failed: {done.stderr.strip() or done.stdout.strip()}')
    return json.loads(done.stdout) if done.stdout.strip() else None


def stamp(text):
    return datetime.fromisoformat(text.replace('Z', '+00:00')).timestamp()


def note(iid, action, text=''):
    """Every taskq note starts with `**action** · who`: the history anyone reads in GitLab, and `report`'s input."""
    api('POST', f'issues/{iid}/notes', {'body': f'**{action}** · {who()}' + (f'\n\n{text}' if text else '')})


# --- issue <-> task -----------------------------------------------------------------------

def parse(issue):
    found = BLOCK.search(issue.get('description') or '')
    labels = issue['labels']
    states = [label[len(PREFIX):] for label in labels if label.startswith(PREFIX)]
    if not found or len(states) != 1 or states[0] not in STATES:
        return None
    return {**json.loads(found.group(1)), 'iid': issue['iid'], 'title': issue['title'], 'state': states[0],
            'type': next((label for label in labels if label in TYPES), None),
            'runtime': next((label[len(RUN):] for label in labels if label.startswith(RUN)), None),
            'assignees': [user['id'] for user in issue.get('assignees', [])],
            'priority': min([int(label[9:]) for label in labels if re.fullmatch(r'priority-\d', label)] or [9]),
            'age': int(time.time() - stamp(issue['updated_at'])) // 60,
            'text': BLOCK.sub('', issue['description']).strip()}


def render(text, block):
    return f'{text}\n\n<!-- taskq:start -->\n```json\n{json.dumps(block, indent=1, ensure_ascii=False)}\n```\n<!-- taskq:end -->'


def pages(path):
    """Every page: GitLab cuts a list at per_page without saying so."""
    found, page = [], 1
    while True:
        batch = api('GET', f'{path}{"&" if "?" in path else "?"}per_page=100&page={page}')
        found += batch
        if len(batch) < 100:
            return found
        page += 1


def issues(query='state=opened'):
    return pages(f'issues?{query}')


def load(query=''):
    """Every open task by priority, the numbers of all open issues (for dependencies), the open issues
    with a task block that are not valid tasks (a card moved off the board's state columns by hand),
    and the open problem issues."""
    opened = issues('state=opened' + ('&' + query if query else ''))
    found = sorted(filter(None, map(parse, opened)), key=lambda item: (item['priority'], item['iid']))
    odd = [issue for issue in opened if BLOCK.search(issue.get('description') or '') and not parse(issue)]
    problems = [issue for issue in opened if PROBLEM in issue['labels']]
    return found, {issue['iid'] for issue in opened}, odd, problems


def task(iid, states=STATES):
    issue = api('GET', f'issues/{iid}')
    found = parse(issue) if issue['state'] == 'opened' else None
    if not found:
        fail(f'#{iid} is not an open taskq task')
    if found['state'] not in states:
        fail(f'#{iid} is {found["state"]}, not {" or ".join(states)}')
    return found


def save(current, state=None, note_action=None, note_text='', close=False, add=(), remove=(), assignee_ids=None, **changes):
    """One PUT moves the labels and the block together; the note is the readable history."""
    block = {key: current.get(key) for key in FIELDS}
    block.update(changes)
    add, remove = list(add), list(remove)
    if state and state != current['state']:
        add, remove = add + [PREFIX + state], remove + [PREFIX + current['state']]
    body = {'description': render(current['text'], block), **labels(add, remove)}
    if assignee_ids is not None:
        body['assignee_ids'] = assignee_ids
    if close:
        body.update(state_event='close', remove_labels=PREFIX + current['state'])
    api('PUT', f'issues/{current["iid"]}', body)
    if note_action:
        note(current['iid'], note_action, note_text)


def labels(add, remove):
    return {key: ','.join(names) for key, names in (('add_labels', add), ('remove_labels', remove)) if names}


def comments(iid):
    return pages(f'issues/{iid}/notes?sort=asc&activity_filter=only_comments')


def notes(iid):
    return [item['body'] for item in comments(iid) if not item['body'].startswith(('**beat**', '**shown**'))]


def link(iid, deps):
    """A clickable `relates_to` link per dependency; `deps` in the block stays the source of truth."""
    have = {item['iid'] for item in api('GET', f'issues/{iid}/links')}
    for dep in sorted(set(deps) - have):
        api('POST', f'issues/{iid}/links', {'target_project_id': PROJECT_PATH, 'target_issue_iid': dep,
                                           'link_type': 'relates_to'})


def milestone_id(title):
    found = [item for item in api('GET', 'milestones?state=active&per_page=100') if item['title'] == title]
    if not found:
        fail(f'no active milestone {title!r}: create it in GitLab first')
    return found[0]['id']


def overlap(left, right):
    return any(a == b or a.startswith(b + '/') or b.startswith(a + '/') for a in left for b in right)


def user():
    return api('GET', '/user')['id']


def limits(text):
    found = {'claude': 2, 'codex': 3}
    if not text:
        return found
    for pair in text.split(','):
        match = re.fullmatch(r'(claude|codex)=(\d+)', pair)
        if not match:
            raise argparse.ArgumentTypeError('expected claude=N,codex=M with non-negative integers')
        found[match[1]] = int(match[2])
    return found


def local_claim(claim):
    if claim.get('host'):
        return claim['host'] == socket.gethostname()
    # Upgrade existing claims from local app evidence, without editing someone else's task.
    sid = claim.get('session')
    if not sid:
        return False
    if claim.get('runtime') == 'claude':
        return any(CLAUDE_APP_SESSIONS.glob(f'*/*/local_{sid}.json'))
    if claim.get('runtime') == 'codex':
        root = Path(os.environ.get('CODEX_HOME', Path.home() / '.codex'))
        return any(any((root / folder).glob(f'**/*-{sid}.jsonl')) for folder in ('sessions', 'archived_sessions'))
    return False


def room(everything, capacity):
    taken = [(item['claim'] or {}).get('runtime') for item in everything
             if item['state'] == 'doing' and local_claim(item['claim'] or {})]
    return {name: count - taken.count(name) for name, count in capacity.items()}


def eligible(item, uid, mine=False):
    assigned = item.get('assignees', [])
    return assigned == [uid] or (not assigned and not mine)


def profile(args):
    # Filtering must not hide dependency or scope owners. Keep the unfiltered safety inventory.
    loaded = load()
    matching = load(args.filter)[0] if args.filter else loaded[0]
    uid = user()
    candidates = [item for item in matching if eligible(item, uid, args.mine)]
    print(f'Profile: filter={args.filter!r}; mine={args.mine}; limit=' +
          ','.join(f'{name}={count}' for name, count in args.limit.items()) + f'; candidates={len(candidates)}')
    if args.filter and not candidates:
        print('Warning: nonempty filter returned 0 candidates; check the GitLab filter.')
    return loaded, candidates


def refusal(candidate, everything, open_iids, runtime=None):
    """Why this ready task cannot start now (in a session of `runtime`, if named), or None. The only admission rule."""
    if runtime and candidate['runtime'] not in (None, runtime):
        return f'runtime is {candidate["runtime"]}'
    waiting = sorted(set(candidate['deps']) & open_iids)
    if waiting:
        return f'open dependencies {waiting}'
    # A started task keeps its paths through questions and review, until closed or released.
    for other in everything:
        if other['claim'] and other['iid'] != candidate['iid'] and overlap(candidate['scope'], other['scope']):
            return f'scope overlaps #{other["iid"]}'
    return None


def startable(runtime=None, loaded=None):
    everything, open_iids, *_ = loaded or load()
    return [item for item in everything if item['state'] == 'ready' and not refusal(item, everything, open_iids, runtime)]


def lock(iid):
    """True: this call set the lock. False: it was already set (GitLab answers 404 «has already been taken»)."""
    try:
        award = api('POST', f'issues/{iid}/award_emoji', {'name': LOCK})
        first = min(locks(iid), key=lambda item: (stamp(item['created_at']), item['id']))
        if first['id'] != award['id']:
            api('DELETE', f'issues/{iid}/award_emoji/{award["id"]}')
            return False
        return True
    except SystemExit as error:
        if 'has already been taken' in str(error):
            return False
        raise


def locks(iid):
    return [item for item in api('GET', f'issues/{iid}/award_emoji?per_page=100') if item['name'] == LOCK]


def unlock(iid):
    uid = user()
    for item in locks(iid):
        if item['user']['id'] != uid:
            continue
        api('DELETE', f'issues/{iid}/award_emoji/{item["id"]}')


def doing_since(iid):
    """When the issue last entered doing, by GitLab's own clock: the newest `add q-doing` label event."""
    return max([stamp(event['created_at']) for event in pages(f'issues/{iid}/resource_label_events')
                if event['action'] == 'add' and (event.get('label') or {}).get('name') == PREFIX + 'doing'], default=0)


def need_owner(current):
    mine, claim = me(), current['claim'] or {}
    if (claim.get('runtime'), claim.get('session')) != (mine['runtime'], mine['session']):
        fail(f'#{current["iid"]} is not claimed by this session')


# --- commands: anyone -----------------------------------------------------------------------

def add(args):
    if not args.goal.strip() or not args.acceptance.strip():
        fail('a task needs a goal and an acceptance')
    unknown = set(args.area) - set(AREAS)
    if unknown:
        fail(f'unknown areas {sorted(unknown)}; configure [areas] names and run init')
    runtime = DEFAULT_RUNTIME[args.type] if args.runtime is None else None if args.runtime == 'any' else args.runtime
    block = {'scope': args.scope, 'deps': args.deps, 'claim': None, 'waiting_for': None, 'result': None}
    text = f'## Goal\n\n{args.goal}\n\n## Acceptance\n\n{args.acceptance}'
    labels = [f'area-{area}' for area in args.area] + [f'{PREFIX}ready', f'priority-{args.priority}', args.type] + ([RUN + runtime] if runtime else [])
    body = {'title': args.title, 'description': render(text, block), 'labels': ','.join(labels)}
    if args.mine:
        body['assignee_ids'] = [user()]
    if args.milestone:
        body['milestone_id'] = milestone_id(args.milestone)
    issue = api('POST', 'issues', body)
    link(issue['iid'], args.deps)
    print(f'#{issue["iid"]} {issue["web_url"]}')


def edit(args):
    """Change dependencies or the milestone (epic) of an open task; `tick` then moves it ready<->waiting."""
    current = task(args.iid)
    if args.milestone is not None:
        api('PUT', f'issues/{args.iid}', {'milestone_id': milestone_id(args.milestone) if args.milestone else None})
    if args.deps is not None:
        link(args.iid, args.deps)
        save(current, note_action='deps', note_text=f'{current["deps"]} → {args.deps}', deps=args.deps)
    print(f'#{args.iid} edited')


def later(args):
    """The owner defers a task; nobody waits on anything. Back with `answer` or by hand to ready."""
    current = task(args.iid, ('ready', 'waiting', 'ask'))
    save(current, 'later', 'later', args.text, waiting_for=args.text)


def listing(args):
    everything, open_iids, odd, problems = load()
    for item in sorted(everything, key=lambda item: STATES.index(item['state'])):
        claim, detail = item['claim'] or {}, ''
        if item['state'] == 'ready':
            detail = refusal(item, everything, open_iids) or ('continue' if claim else '')
        elif item['state'] == 'doing':
            detail = f'{claim.get("runtime")}:{(claim.get("session") or "")[:8]}, last change {item["age"]} min ago'
        elif item['state'] == 'waiting':
            detail = f'open dependencies {sorted(set(item["deps"]) & open_iids)}'
        elif item['state'] == 'later':
            detail = item['waiting_for'] or ''
        print(f'#{item["iid"]:<4} {item["state"]:<8} p{item["priority"]} {item["runtime"] or "any":<6} {item["title"]}' + (f'  [{detail}]' if detail else ''))
    for issue in odd:
        print(f'#{issue["iid"]:<4} ?        labels {issue["labels"]}: not a valid task, see `tick`')
    for issue in problems:
        print(f'#{issue["iid"]:<4} {PROBLEM:<8} {issue["title"]}')


def set_runtime(args):
    current = task(args.iid, ('ready', 'waiting', 'ask', 'later'))
    runtime = None if args.runtime == 'any' else args.runtime
    save(current, add=[RUN + runtime] if runtime else [], remove=[RUN + current['runtime']] if current['runtime'] else [],
         note_action='runtime', note_text=f'{current["runtime"] or "any"} → {args.runtime}')
    print(f'#{args.iid} runtime {args.runtime}')


# --- commands: worker -----------------------------------------------------------------------

def brief(current):
    claim, pushes = current['claim'], current['type'] in ('code', 'docs')
    kind = ('continue' if claim else 'new') if pushes else 'none'
    return BRIEF.format(**{**current, 'tool': TOOL, 'rules': RULES, 'deliver': DELIVER[pushes],
                           'workspace': WORKSPACE[kind].format(iid=current['iid']),
                           'sha': ' --sha <pushed commit>' if pushes else '',
                           'scope': ', '.join(current['scope']) or 'none',
                           'notes': '\n\n---\n\n'.join(notes(current['iid'])) or 'none'})


def worker(args):
    """What a fresh worker session runs first: the brief of the first task that can start now."""
    loaded, candidates = profile(args)
    runtime = me()['runtime']
    free = room(loaded[0], args.limit)
    found = [item for item in candidates if item['state'] == 'ready' and free[runtime] > 0
             and not refusal(item, loaded[0], loaded[1], runtime)]
    print(brief(found[0]).replace(f'{TOOL} worker`', f'{TOOL} worker{profile_arguments(args)}`')
          if found else 'No task can start now. Say so and stop.')


def take(args):
    """Check the admission rule, set the lock, move the task to doing. The lock decides one task;
    Scope is a rule between tasks, so after the move the newer of two overlapping takes gives way."""
    mine = me()
    everything, open_iids, *_ = load()
    current = next((item for item in everything if item['iid'] == args.iid), None) or fail(f'#{args.iid} is not an open taskq task')
    claim = current['claim'] or {}
    if current['state'] == 'doing' and (claim.get('runtime'), claim.get('session')) == (mine['runtime'], mine['session']):
        note(args.iid, 'take')  # a repeated take of its own task, e.g. after an answer in the session
        return print(f'#{args.iid} is yours')
    reason = f'state is {current["state"]}' if current['state'] != 'ready' else refusal(current, everything, open_iids, mine['runtime'])
    if reason:
        fail(f'#{args.iid} cannot start: {reason}')
    if not lock(args.iid):
        fail(f'#{args.iid} cannot start: another worker holds its lock')
    save(current, 'doing', claim=mine, result=None, waiting_for=None, assignee_ids=[user()])
    held = {**current, 'state': 'doing'}
    # A task without paths overlaps nothing: no second read.
    rivals = current['scope'] and [other for other in load()[0] if other['iid'] != args.iid and (other['claim'] or {}).get('session')
              and overlap(current['scope'], other['scope'])]
    if rivals:
        # Each take moves its task before it reads, so the later of two sees the earlier; both order them alike.
        since = doing_since(args.iid)
        older = [other for other in rivals if (doing_since(other['iid']), other['iid']) < (since, args.iid)]
        if older:
            save(held, 'ready', claim=current['claim'], result=current['result'], waiting_for=current['waiting_for'],
                 assignee_ids=current.get('assignees', []))
            unlock(args.iid)
            fail(f'#{args.iid} cannot start: scope overlaps #{older[0]["iid"]}')
    note(args.iid, 'take')
    print(f'#{args.iid} is yours')


def beat(args):
    """A new note moves `updated_at` (an edited one does not); the previous beat, if it is the newest, goes."""
    need_owner(task(args.iid, ('doing',)))
    last = api('GET', f'issues/{args.iid}/notes?sort=desc&per_page=1&activity_filter=only_comments')
    note(args.iid, 'beat')
    if last and last[0]['body'].startswith('**beat**'):
        api('DELETE', f'issues/{args.iid}/notes/{last[0]["id"]}')


def ask(args):
    """A worker asks about its doing task; the manager asks about a task not started yet."""
    current = task(args.iid, ('doing', 'ready', 'waiting', 'later'))
    if current['state'] == 'doing':
        need_owner(current)
    save(current, 'ask', 'ask', args.text)
    if current['state'] == 'doing':
        print(f'#{args.iid} asked. Stop here. If the owner answers in this session, record it with '
              f'`{TOOL} answer {args.iid} --text "<answer>"` and continue here; otherwise the coordinator '
              'brings the answer through the queue.')


def result(args):
    current = task(args.iid, ('doing',))
    need_owner(current)
    if current['type'] in ('code', 'docs') and not args.sha:
        fail(f'a {current["type"]} result needs --sha')
    save(current, 'review', 'result', f'`{args.sha}`\n\n{args.text}\n\nChecks: {args.checks}',
         result={'sha': args.sha, 'checks': args.checks})


# --- commands: coordinator and owner ----------------------------------------------------------

def requeue(args):
    """answer: the owner's reply to a question or a hold. reject: review sends the work back.
    release: drop a dead worker's claim. The next worker session continues with the full history."""
    current = task(args.iid, {'answer': ('ask', 'later'), 'reject': ('review',)}.get(args.action, STATES))
    claim = current['claim'] or {}
    if args.action == 'answer' and claim.get('session') and {key: claim.get(key) for key in ('runtime', 'session')} == session():
        # The owner answered in the worker's own session: it continues with its claim. Its doing place
        # was free while the task waited in ask, so the limit is not checked: the worker never left.
        save(current, 'doing', 'answer', args.text, waiting_for=None)
        return print(f'#{args.iid} is doing again with your claim: continue in this session')
    # An empty claim marks a started task: it keeps its paths and its next worker continues.
    claim = current['claim'] and {'runtime': None, 'session': None}
    save(current, 'ready', args.action, args.text, waiting_for=None, result=None, claim=claim)
    unlock(args.iid)


def close(args):
    current = task(args.iid, ('review',))
    if current['type'] in ('code', 'docs'):
        sha = current['result']['sha']
        subprocess.run(['git', 'fetch', '-q', 'origin', 'main'], check=True)
        if subprocess.run(['git', 'merge-base', '--is-ancestor', sha, 'origin/main']).returncode:
            fail(f'{sha} is not in origin/main; reject the task so the worker pushes it')
    save(current, close=True, note_action='close', note_text=args.text)
    unlock(args.iid)
    print(f'#{args.iid} closed')
    if current['type'] in ('code', 'docs'):
        if RETIRE:
            print(f'Now retire its worktree from the main checkout: {RETIRE.format(iid=args.iid)}')


def problem(args):
    """On the task's issue; without a task, an issue of its own that `tick` names until someone closes it."""
    if args.task:
        return note(args.task, 'problem', args.text)
    issue = api('POST', 'issues', {'title': f'{PROBLEM}: {args.text.splitlines()[0][:80]}', 'labels': PROBLEM,
                                   'description': f'**{PROBLEM}** · {who()}\n\n{args.text}'})
    print(f'#{issue["iid"]} {issue["web_url"]}')


def report(args):
    """Where the time went and what went wrong, from the taskq notes of issues changed in the last hours."""
    since = time.time() - args.hours * 3600
    after = time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(since))
    from concurrent.futures import ThreadPoolExecutor
    found, tasks, changed = [], set(), issues(f'state=all&updated_after={after}')
    # One request per changed issue, ~1.3 s each through glab: 8 at once (192 issues: 255 s alone).
    with ThreadPoolExecutor(8) as pool:
        histories = list(pool.map(lambda issue: comments(issue['iid']), changed))
    for issue, history in zip(changed, histories):
        if BLOCK.search(issue.get('description') or ''):
            tasks.add(issue['iid'])
        # A task-less problem is its issue's description.
        own = [{'body': issue['description'], 'created_at': issue['created_at']}] if PROBLEM in issue['labels'] else []
        for item in own + history:
            head = re.match(r'\*\*(.+?)\*\* · (\S+)', item['body'] or '')
            if head and stamp(item['created_at']) >= since:
                found.append({'task': issue['iid'], 'at': stamp(item['created_at']), 'action': head[1],
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


def contract(args):
    """Where the contracts live: the queue (taskq.md) and the manager/coordinator session (taskq-manager.md)."""
    folder = Path(__file__).resolve().parent / 'contracts'
    print('\n'.join(str(path) for path in sorted(folder.glob('*.md'))))


def git(*args, cwd=None):
    """Git's output, or None when it failed or took too long (no network)."""
    try:
        done = subprocess.run(['git', *(['-C', str(cwd)] if cwd else []), *args], capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired:
        return None
    return None if done.returncode else done.stdout.strip()


def install():
    """('clone', folder) for an editable clone; ('git', commit) for an install from REPO; (None, None) otherwise."""
    folder = Path(__file__).resolve().parents[1]
    if (folder / '.git').exists():
        return 'clone', folder
    from importlib import metadata
    try:
        origin = json.loads(metadata.distribution('taskq').read_text('direct_url.json') or '{}')
    except metadata.PackageNotFoundError:
        origin = {}
    commit = origin.get('vcs_info', {}).get('commit_id')
    return ('git', commit) if commit else (None, None)


def version():
    kind, where = install()
    return (git('rev-parse', '--short=7', 'HEAD', cwd=where) if kind == 'clone' else (where or '')[:7]) or 'unknown'


def update(args):
    """Bring this install to `main` of REPO: fast-forward of an editable clone, else a reinstall from Git.
    True when it updated. A clone with uncommitted changes or commits `main` lacks is left alone."""
    say = print if args.verbose else (lambda text: None)
    kind, where = install()
    old, remote = version(), git('ls-remote', REPO, 'refs/heads/main')
    if not remote:
        return say(f'update skipped: {REPO} did not answer')
    new = remote.split()[0]
    if kind is None:
        return print(f'not updated: this taskq is not installed from Git; reinstall: pipx install --force git+{REPO}')
    if new == (git('rev-parse', 'HEAD', cwd=where) if kind == 'clone' else where):
        return print(f'up to date {old}')
    if kind == 'clone':
        if git('status', '--porcelain', '--untracked-files=no', cwd=where):
            return print(f'not updated: {where} has uncommitted changes')
        if git('fetch', '-q', REPO, 'main', cwd=where) is None:
            return say(f'update skipped: fetch from {REPO} failed')
        if git('merge-base', '--is-ancestor', 'HEAD', 'FETCH_HEAD', cwd=where) is None:
            return print(f'not updated: {where} has commits main of {REPO} lacks')
        if git('merge', '-q', '--ff-only', 'FETCH_HEAD', cwd=where) is None:
            return print(f'not updated: fast-forward of {where} failed (`git -C {where} merge --ff-only FETCH_HEAD` says why)')
    else:
        # ponytail: pipx and uv tool by their venv path, pip otherwise; another installer reinstalls by hand.
        prefix = Path(sys.prefix).parts
        command = (['pipx', 'install', '--force'] if 'pipx' in prefix else ['uv', 'tool', 'install', '--force'] if 'uv' in prefix
                   else [sys.executable, '-m', 'pip', 'install', '-q', '--force-reinstall'])
        subprocess.run([*command, f'git+{REPO}'], check=True, capture_output=True, timeout=600)
    print(f'updated {old} → {new[:7]}')
    return True


def auto_update():
    """[update] auto: at most once per `every`; a pass that updated goes on as the new version (exec)."""
    if not UPDATE['auto'] or (UPDATE_STAMP.exists() and time.time() - UPDATE_STAMP.stat().st_mtime < seconds(UPDATE['every'])):
        return
    UPDATE_STAMP.parent.mkdir(parents=True, exist_ok=True)
    UPDATE_STAMP.touch()
    if update(argparse.Namespace(verbose=False)):
        sys.stdout.flush()
        os.execv(sys.executable, [sys.executable, '-m', 'taskq', *sys.argv[1:]])


def migrate(args):
    """`init`: a new project, and once per schema change; idempotent. Labels for every state, runtime, type and
    priority; the board with one list
    per state in STATES order; state labels taskq no longer has leave the board, and leave GitLab once no
    issue carries them; every open task gets its `relates_to` links. Claims, results and history stay."""
    have = {label['name']: label for label in pages('labels')}
    for name in ([PREFIX + state for state in STATES] + [RUN + runtime for runtime in RUNTIMES] + list(TYPES) + [PROBLEM]
                 + [f'priority-{level}' for level in PRIORITIES] + ['area-' + name for name in AREAS]):
        if name not in have:
            have[name] = api('POST', 'labels', {'name': name, 'color': '#6699cc'})
    board = next((board for board in api('GET', 'boards') if board['name'] == BOARD), None) or api('POST', 'boards', {'name': BOARD})
    lists = {item['label']['name']: item for item in board['lists']}
    for name, item in lists.items():
        if name.startswith(PREFIX) and name[len(PREFIX):] not in STATES:
            api('DELETE', f'boards/{board["id"]}/lists/{item["id"]}')
    for state in STATES:
        if PREFIX + state not in lists:
            api('POST', f'boards/{board["id"]}/lists', {'label_id': have[PREFIX + state]['id']})
    # GitLab shifts positions on every create and refuses a move to the current place: compare fresh positions.
    for position, state in enumerate(STATES):
        item = next(item for item in api('GET', f'boards/{board["id"]}/lists') if item['label']['name'] == PREFIX + state)
        if item['position'] != position:
            api('PUT', f'boards/{board["id"]}/lists/{item["id"]}', {'position': position})
    for name in have:
        if name.startswith(PREFIX) and name[len(PREFIX):] not in STATES:
            carriers = [issue['iid'] for issue in issues(f'state=all&labels={name}')]
            if carriers:
                print(f'label {name} kept: still on {sorted(carriers)}')
            else:
                api('DELETE', f'labels/{name}')
    everything = load()[0]
    for item in everything:
        link(item['iid'], item['deps'])
    print(f'board {board["id"]}: {", ".join(PREFIX + state for state in STATES)}; links checked on {len(everything)} tasks')


# --- Codex app server (the desktop app's shared one); JSON-RPC over a WebSocket on a unix socket ---

CODEX_SOCKET = Path.home() / '.codex/app-server-control/app-server-control.sock'
CODEX_IPC = Path.home() / '.codex/ipc/ipc.sock'
# The Claude app writes `<account>/<org>/local_<id>.json` here when it has imported a session.
CLAUDE_APP_SESSIONS = Path.home() / 'Library/Application Support/Claude/claude-code-sessions'
# Design decision 2026-10-06: a Codex worker runs outside the sandbox and never asks. Inside it, git
# could not write the main checkout's refs (`git worktree add`) and glab could not read its token.
CODEX_TURN_POLICY = {'approvalPolicy': 'never', 'sandboxPolicy': {'type': 'dangerFullAccess'}}
# thread/start and thread/resume use the CLI spelling; turn/start uses the policy enum.
CODEX_ACCESS = {'sandbox': {'dangerFullAccess': 'danger-full-access'}[CODEX_TURN_POLICY['sandboxPolicy']['type']],
                'approvalPolicy': CODEX_TURN_POLICY['approvalPolicy']}


class Codex:
    def __init__(self, timeout=60):
        import base64
        import socket
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(timeout)
        self.socket.connect(str(CODEX_SOCKET))
        key = base64.b64encode(os.urandom(16)).decode()
        self.socket.sendall(('GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
                             f'Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n').encode())
        head = b''
        while b'\r\n\r\n' not in head:
            head += self.exact(1)
        if b' 101 ' not in head.split(b'\r\n')[0]:
            fail(f'Codex app server refused the connection: {head[:80]}')
        self.counter = 0
        self.call('initialize', {'clientInfo': {'name': 'taskq', 'version': '1'}, 'capabilities': {'experimentalApi': True}})
        self.send({'method': 'initialized'})

    def exact(self, size):
        data = b''
        while len(data) < size:
            data += self.socket.recv(size - len(data)) or fail('Codex app server closed the connection')
        return data

    def send(self, value):
        import struct
        data, mask = json.dumps(value).encode(), os.urandom(4)
        size = len(data)
        head = bytes([0x81]) + (bytes([0x80 | size]) if size < 126 else bytes([0xfe]) + struct.pack('>H', size)
                                if size < 65536 else bytes([0xff]) + struct.pack('>Q', size))
        self.socket.sendall(head + mask + bytes(byte ^ mask[index % 4] for index, byte in enumerate(data)))

    def receive(self):
        import struct
        message = b''
        while True:
            first, second = self.exact(2)
            size = second & 0x7f
            size = struct.unpack('>H', self.exact(2))[0] if size == 126 else struct.unpack('>Q', self.exact(8))[0] if size == 127 else size
            payload, opcode = self.exact(size), first & 0x0f
            if opcode == 9:
                self.socket.sendall(bytes([0x8a, 0x80]) + b'\0\0\0\0')
                continue
            if opcode == 8:
                fail('Codex app server closed the connection')
            message += payload
            if first & 0x80:
                return json.loads(message)

    def call(self, method, params):
        self.counter += 1
        self.send({'id': self.counter, 'method': method, 'params': params})
        while True:
            value = self.receive()
            if value.get('id') == self.counter and 'method' not in value:
                if 'error' in value:
                    fail(f'Codex {method}: {value["error"]}')
                return value['result']

    def wait_turn(self, thread, seconds):
        end = time.time() + seconds
        while time.time() < end:
            value = self.receive()
            if value.get('method') == 'turn/completed' and value['params'].get('threadId') == thread:
                return
        fail(f'Codex thread {thread}: no turn/completed in {seconds} s')


class CodexIpc:
    """The app's IPC router (4-byte little-endian length, then JSON); versions are the app's own table."""
    def __init__(self, timeout=60):
        import socket
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(timeout)
        self.socket.connect(str(CODEX_IPC))
        self.client = 'initializing-client'
        self.client = self.request('initialize', {'clientType': 'taskq'}, 0)['result']['clientId']

    def put(self, value):
        import struct
        data = json.dumps(value).encode()
        self.socket.sendall(struct.pack('<I', len(data)) + data)

    def exact(self, size):
        data = b''
        while len(data) < size:
            data += self.socket.recv(size - len(data)) or fail('Codex app IPC closed the connection')
        return data

    def request(self, method, params, version, target=None):
        import struct
        import uuid
        request = str(uuid.uuid4())
        self.put({'type': 'request', 'requestId': request, 'sourceClientId': self.client, 'version': version,
                  'method': method, 'params': params, **({'targetClientId': target} if target else {})})
        while True:
            reply = json.loads(self.exact(struct.unpack('<I', self.exact(4))[0]))
            if reply.get('type') == 'client-discovery-request':
                # The router asks every client; answering at once makes "nobody owns it" a 0 s, not 10 s, error.
                self.put({'type': 'client-discovery-response', 'requestId': reply['requestId'],
                          'response': {'canHandle': False}})
            elif reply.get('type') == 'response' and reply.get('requestId') == request:
                return reply

    def broadcast(self, method, params, version):
        self.put({'type': 'broadcast', 'method': method, 'sourceClientId': self.client, 'params': params,
                  'version': version})
        time.sleep(0.5)


def codex_announce(thread, method='thread-unarchived', version=1):
    """The way 'taskq probe 1' (2026-10-06) got into the app's sidebar: one `thread-unarchived` broadcast.
    `thread-archived` (version 2 in the app's table) takes it out the same way."""
    ipc = CodexIpc(timeout=10)
    try:
        ipc.broadcast(method, {'hostId': 'local', 'conversationId': thread}, version)
    finally:
        ipc.socket.close()


def codex_release(codex, thread):
    """Only an explicit unsubscribe lets the shared server unload a thread: 60 s after its turn ends it drops
    the writer lock (~/.codex/thread-writer-locks), and the app may take the thread (2026-10-06).
    A closed connection alone keeps it loaded, and the app shows 'This is open in another app'."""
    codex.call('thread/unsubscribe', {'threadId': thread})


def codex_send_app(codex, thread, metadata, text):
    """Deliver through the app when its window owns the thread, as a second app window would
    (`thread-follower-*`). None: the app does not own it. The app applies the request's policy."""
    try:
        ipc = CodexIpc()
    except OSError:
        return None  # the app is not running
    try:
        found = ipc.request('thread-owner-discovery', {'hostId': 'local', 'conversationId': thread}, 1)
        if found['resultType'] != 'success':
            return None
        owner, item = found['handledByClientId'], [{'type': 'text', 'text': text, 'text_elements': []}]
        turns = codex.call('thread/turns/list', {'threadId': thread, 'limit': 1, 'itemsView': 'notLoaded'})['data']
        if turns and codex_app_running(metadata, turns[0]):
            cwd = metadata.get('cwd') or str(ROOT)
            reply = ipc.request('thread-follower-steer-turn', {'conversationId': thread, 'input': item, 'restoreMessage':
                                {'text': text, 'cwd': cwd, 'context': {'workspaceRoots': [cwd]}}}, 1, owner)
            if reply['resultType'] == 'success':
                return 'steered the active turn in the Codex app'
        reply = ipc.request('thread-follower-start-turn', {'conversationId': thread, 'turnStart': {
            'request': {'threadId': thread, 'input': item, **CODEX_TURN_POLICY}, 'context': {}}}, 2, owner)
        if reply['resultType'] != 'success':
            fail(f'Codex app refused the message for {thread}: {reply.get("error")}')
        return 'new turn in the Codex app'
    finally:
        ipc.socket.close()


def codex_spawn(name):
    """A persistent thread of the app's project `CODEX_PROJECT` in section `CODEX_SECTION`, with one finished turn, announced
    to the app and released by the shared server, so the owner can write in it. It runs with `CODEX_ACCESS`."""
    codex = Codex(timeout=300)
    if not CODEX_PROJECT:
        fail('a Codex worker needs [codex] project in taskq.toml (the app\'s project id, `project/list`)')
    thread = codex.call('thread/start', {'cwd': str(ROOT), 'projectId': CODEX_PROJECT,
                                         'ephemeral': False, **CODEX_ACCESS})['thread']['id']
    codex.call('thread/name/set', {'threadId': thread, 'name': name})
    if CODEX_SECTION:
        codex.call('thread/section/move', {'threadId': thread, 'sectionId': CODEX_SECTION})
    codex.call('turn/start', {'threadId': thread, **CODEX_TURN_POLICY,
                            'input': [{'type': 'text', 'text': 'Reply with the single word: ready'}]})
    codex.wait_turn(thread, 280)
    codex_release(codex, thread)
    codex_announce(thread)
    return thread


def codex_send(args):
    """Pin every new turn's policy; deliver active input through steer without a new turn. A thread the
    shared server has not loaded may be the app's: then the app delivers it."""
    codex = Codex()
    metadata = codex.call('thread/read', {'threadId': args.thread})['thread']
    status = metadata['status']['type']
    route = codex_send_app(codex, args.thread, metadata, args.text) if status == 'notLoaded' else None
    if route:
        return print(f'delivered to {args.thread} ({route})')
    if status == 'active':
        turns = codex.call('thread/turns/list', {'threadId': args.thread, 'limit': 1, 'itemsView': 'notLoaded'})['data']
        if not turns or turns[0]['status'] != 'inProgress':
            fail(f'Codex thread {args.thread}: active turn id unavailable; no message sent')
        codex.call('turn/steer', {'threadId': args.thread, 'expectedTurnId': turns[0]['id'],
                                 'input': [{'type': 'text', 'text': args.text}]})
    else:
        try:
            # Also for a loaded idle thread: resume subscribes this connection, so the release below unloads it.
            codex.call('thread/resume', {'threadId': args.thread, **CODEX_ACCESS})
        except SystemExit as error:
            if 'active writer' in str(error):
                fail(f'Codex thread {args.thread} is held by the Codex app, but no app window owns it; '
                     f'open it there (`open -g codex://threads/{args.thread}`) and send again')
            raise
        codex.call('turn/start', {'threadId': args.thread, **CODEX_TURN_POLICY,
                                'input': [{'type': 'text', 'text': args.text}]})
        codex_release(codex, args.thread)
    print(f'delivered to {args.thread}' + (' (steered active turn)' if status == 'active' else ''))


# Bounded diagnostic output, independent of the number of items in a long worker turn.
CODEX_ITEM_LIMIT = 100
CODEX_TAIL_BYTES = 256 * 1024  # bounded live diagnostic, not a second session log


def codex_live_entries(path, turn):
    """The paginated store omits running exec commands. Read only this thread's bounded rollout tail."""
    from datetime import datetime
    if not path:
        return [], None
    try:
        with open(path, 'rb') as handle:
            handle.seek(0, 2)
            offset = max(0, handle.tell() - CODEX_TAIL_BYTES)
            handle.seek(offset)
            lines = handle.read(CODEX_TAIL_BYTES).splitlines()
        if offset:
            lines = lines[1:]  # possibly partial first record
    except OSError:
        return [], None
    calls, processes, last = {}, {}, None
    recorded = {str(entry['item'].get('processId')) for entry in turn['entries']
                if entry['item'].get('type') == 'commandExecution'}
    for line in lines:
        try:
            record = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            continue  # partial last write is normal while the thread works
        payload = record.get('payload', {})
        if record.get('type') != 'response_item':
            continue
        meta = payload.get('internal_chat_message_metadata_passthrough') or {}
        if meta.get('turn_id') != turn['id']:
            continue
        kind, call = payload.get('type'), payload.get('call_id')
        if kind not in ('custom_tool_call', 'function_call', 'custom_tool_call_output', 'function_call_output'):
            continue
        stamp = datetime.fromisoformat(record['timestamp'].replace('Z', '+00:00')).timestamp()
        last = max(last or stamp, stamp)
        if kind in ('custom_tool_call', 'function_call'):
            calls[call] = {'turnId': turn['id'], 'startedAtMs': stamp * 1000,
                           'item': {'type': 'liveToolCall', 'id': call, 'status': 'inProgress',
                                    'tool': payload['name'], 'arguments': payload.get('input', payload.get('arguments', ''))}}
        else:
            entry = calls.pop(call, None)
            output = payload.get('output')
            blocks = output if isinstance(output, list) else [{'text': output}]
            for block in blocks:
                try:
                    result = json.loads(block.get('text', ''))
                except (ValueError, TypeError):
                    continue
                if isinstance(result, dict) and result.get('session_id') is not None and entry is not None:
                    sid = str(result['session_id'])
                    if sid not in processes and 'exec_command' in entry['item']['arguments']:
                        processes[sid] = {**entry, 'item': {'type': 'commandExecution', 'status': 'inProgress',
                                                          'command': entry['item']['arguments'], 'processId': sid}}
            # A commandExecution completion in the API is authoritative; tool output alone may be a polling result.
    return [entry for sid, entry in processes.items() if sid not in recorded] + list(calls.values()), last


def codex_turn_policy(path, turn_id):
    """Report this turn's recorded policy, never its thread defaults or our desired policy."""
    policy = None
    if not path:
        return policy
    try:
        with open(path, 'rb') as handle:
            # ponytail: bounded-memory history scan; add a reverse seek if large rollouts make this costly.
            while line := handle.readline(CODEX_TAIL_BYTES):
                if b'"turn_context"' not in line:
                    continue
                try:
                    record = json.loads(line)
                except (ValueError, UnicodeDecodeError):
                    continue
                payload = record.get('payload', {})
                if record.get('type') == 'turn_context' and payload.get('turn_id') == turn_id:
                    policy = {'sandbox': payload.get('sandbox_policy'), 'approval': payload.get('approval_policy')}
    except OSError:
        pass
    return policy


def codex_app_running(metadata, turn):
    """The shared server reads a turn the app is running from its rollout and calls it `interrupted`, since
    the turn has no end yet. The end (`task_complete` or `turn_aborted`) is always the turn's last record."""
    if metadata['status']['type'] != 'notLoaded' or turn['status'] not in ('interrupted', 'inProgress'):
        return False
    try:
        with open(metadata.get('path') or '', 'rb') as handle:
            handle.seek(max(0, handle.seek(0, 2) - CODEX_TAIL_BYTES))
            tail = handle.read(CODEX_TAIL_BYTES)
    except OSError:
        return False
    return not any(f'"type":"{end}","turn_id":"{turn["id"]}"'.encode() in tail for end in ('task_complete', 'turn_aborted'))


def codex_snapshot(codex, thread, limit=3, include_policy=False):
    """Read persisted item lifecycles, including in-progress commands, without resuming the worker."""
    metadata = codex.call('thread/read', {'threadId': thread})['thread']
    turns = codex.call('thread/turns/list', {'threadId': thread, 'limit': limit, 'itemsView': 'notLoaded'})['data']
    if turns and codex_app_running(metadata, turns[0]):
        turns[0].update(status='inProgress', app=True)
    for turn in turns:
        page = codex.call('thread/items/list', {'threadId': thread, 'turnId': turn['id'],
                                               'limit': CODEX_ITEM_LIMIT, 'sortDirection': 'desc'})
        turn['entries'] = list(reversed(page['data']))
        turn['olderItems'] = bool(page.get('nextCursor'))
        if turn['status'] == 'inProgress':
            live, last = codex_live_entries(metadata.get('path'), turn)
            turn['entries'] += live
            turn['liveStamp'] = last
        turn['entries'].sort(key=lambda entry: entry.get('startedAtMs') or 0)
    # updatedAt is thread metadata recency, not the last item event. Never substitute it silently.
    stamps = [entry[key] / 1000 for turn in turns for entry in turn['entries']
              for key in ('startedAtMs', 'completedAtMs') if entry.get(key) is not None]
    stamps += [turn[key] for turn in turns for key in ('startedAt', 'completedAt') if turn.get(key) is not None]
    stamps += [turn['liveStamp'] for turn in turns if turn.get('liveStamp') is not None]
    if include_policy and turns:
        turns[0]['policy'] = codex_turn_policy(metadata.get('path'), turns[0]['id'])
    return metadata['status'], turns, max(stamps, default=None)


def codex_age(stamp):
    return f'{max(0, time.time() - stamp):.0f}s ago' if stamp is not None else 'unknown (no event timestamp)'


def codex_line(value):
    """One short line per event; never dump an entire command output or tool result."""
    text = ' '.join(str(value or '').split())
    return text[:300] + ('…' if len(text) > 300 else '')


def codex_item(item):
    kind = item['type']
    if kind == 'userMessage':
        return 'user: ' + codex_line(' '.join(part.get('text', f'[{part["type"]}]') for part in item['content']))
    if kind in ('agentMessage', 'plan'):
        return f'{kind}: ' + codex_line(item['text'])
    if kind == 'commandExecution':
        return (f'command {item["status"]} exit={item.get("exitCode")} '
                f'{codex_line(item["command"])} | {codex_line(item.get("aggregatedOutput"))}')
    if kind in ('mcpToolCall', 'dynamicToolCall'):
        return f'{kind} {item["status"]} {item.get("server", item.get("namespace", ""))}/{item["tool"]}'
    if kind == 'liveToolCall':
        return f'live tool {item["tool"]} inProgress: {codex_line(item["arguments"])}'
    return f'{kind}: {codex_line(item.get("status", item.get("query", item.get("path", ""))))}'


def codex_read(args):
    """Print recent turns and timestamped events, with the active operation and event age."""
    codex = Codex()
    status, turns, stamp = codex_snapshot(codex, args.thread, args.limit, include_policy=True)
    held = turns and turns[0].get('app')
    print(f'status: {status["type"]}' + (f' {status.get("activeFlags")}' if status['type'] == 'active' else '') +
          (' (turn running in the Codex app, which holds the session)' if held else ''))
    print(f'last event: {codex_age(stamp)}')
    policy = (turns[0].get('policy') if turns else None) or {}
    sandbox = policy.get('sandbox') or {}
    shown = {key: sandbox[key] for key in ('type', 'network_access') if key in sandbox}
    print('last turn sandbox: ' + (json.dumps(shown, ensure_ascii=False) if shown else 'unknown (no turn_context)') +
          f'; approvalPolicy: {policy.get("approval") or "unknown"}')
    for turn in reversed(turns):
        print(f'turn {turn["id"]}: {turn["status"]}')
        if turn['olderItems']:
            print(f'  older events omitted; showing last {CODEX_ITEM_LIMIT}')
        for entry in turn['entries']:
            when = entry.get('startedAtMs')
            stamp_text = time.strftime('%Y-%m-%d %H:%M:%S UTC', time.gmtime(when / 1000)) if when is not None else 'time unknown'
            print(f'  {stamp_text} {codex_item(entry["item"])}')
    if status['type'] == 'active':
        running = [entry['item'] for turn in turns if turn['status'] == 'inProgress'
                   for entry in turn['entries'] if entry['item'].get('status') == 'inProgress']
        print('now: ' + ('; '.join(codex_item(item) for item in running) if running else 'active; no running tool recorded'))


def codex_archive(args):
    """Archive an idle Codex thread (reversible: `thread/unarchive`) and take it out of the app's sidebar."""
    codex = Codex()
    metadata = codex.call('thread/read', {'threadId': args.thread})['thread']
    if '/archived_sessions/' in (metadata.get('path') or ''):
        return print(f'already archived {args.thread}')
    turns = codex.call('thread/turns/list', {'threadId': args.thread, 'limit': 1, 'itemsView': 'notLoaded'})['data']
    if metadata['status']['type'] == 'active' or (turns and codex_app_running(metadata, turns[0])):
        fail(f'Codex thread {args.thread} is working; not archived')
    try:
        codex.call('thread/archive', {'threadId': args.thread})
    except SystemExit as error:
        # 2026-10-06: a thread the app has opened stays loaded in the app's private app server (stdio, not
        # reachable) for 3 h after it leaves view, or until more than 10 such threads; no IPC request archives
        # or releases it (`thread-archived` only hides the row), and osascript has no assistive access. The
        # app's own context menu archives it; an agent does that with computer-use, not the owner.
        if 'active writer' in str(error):
            fail(f'Codex thread {args.thread} is held open by the Codex app. Archive it there with computer-use '
                 f'(com.openai.codex, full-screen control): `open -g codex://threads/{args.thread}`, activate the app, '
                 f'click the chat body and press Cmd+Shift+A (Archive chat); then run codex-archive again to confirm and '
                 f'open the session the window showed before the same way (the link switches the window)')
        raise
    codex_announce(args.thread, 'thread-archived', 2)
    print(f'archived {args.thread}')


def spawn(args):
    """Create a visible worker session in the main checkout and print its id; the coordinator then sends
    it the worker prompt. Claude: a CLI session imported into the desktop app. Codex: `codex_spawn`."""
    if args.runtime == 'codex':
        return print(codex_spawn(args.name))
    import uuid
    session, root = str(uuid.uuid4()), ROOT
    env = {key: value for key, value in os.environ.items() if key not in RUNTIMES.values()}
    done = subprocess.run(['claude', '-p', 'Reply with the single word: ready', '--session-id', session],
                          cwd=root, env=env, capture_output=True, text=True, timeout=300)
    if done.returncode:
        fail(f'claude could not create the session: {done.stderr.strip() or done.stdout.strip()}')
    # 2026-10-06: the app's resume handler imports the session and then always navigates its main pane
    # to it (no option to skip). -g only keeps the app in the background. With --restore the pane goes back
    # to the session the owner had open as soon as the import has written the session's record.
    subprocess.run(['open', '-g', f'claude://resume?session={session}'], check=True)
    if args.restore:
        end = time.time() + 20  # measured 1.2 s
        while not any(CLAUDE_APP_SESSIONS.glob(f'*/*/local_{session}.json')) and time.time() < end:
            time.sleep(0.05)
        restore = args.restore if args.restore.startswith('local_') else f'local_{args.restore}'
        subprocess.run(['open', '-g', f'claude://claude.ai/epitaxy/{restore}'], check=True)
    print(f'local_{session}')


def question(iid):
    """The latest question of an `ask` task and when `tick` last showed it (None: not yet). The newest page
    is enough: while a task waits in ask, only `shown` notes follow its question."""
    shown = None
    for item in api('GET', f'issues/{iid}/notes?sort=desc&per_page=100&activity_filter=only_comments'):
        if item['body'].startswith('**shown**') and shown is None:
            shown = stamp(item['created_at'])
        elif item['body'].startswith('**ask**'):
            return item['body'].split('\n\n', 1)[-1], shown
    return 'no question note', shown


TICK_LIVE_MINUTES = 15  # a tick younger than this means another coordinator session is armed


def tick_beat():
    """Record this tick and report the previous one, so a second session does not arm a second tick."""
    before = TICK_BEAT.stat().st_mtime if TICK_BEAT.exists() else None
    TICK_BEAT.parent.mkdir(parents=True, exist_ok=True)
    TICK_BEAT.touch()
    if before is None:
        return print('Last tick: none.')
    minutes = int((time.time() - before) // 60)
    live = ' (another coordinator is armed: do not CronCreate a second tick)' if minutes < TICK_LIVE_MINUTES else ''
    print(f'Last tick: {minutes} min ago{live}.')


def profile_arguments(args):
    flags = (' --filter ' + shlex.quote(args.filter) if args.filter else '') + (' --mine' if args.mine else '')
    flags += ' --limit ' + ','.join(f'{name}={count}' for name, count in args.limit.items())
    return flags


def worker_prompt(args):
    return WORKER.replace(f'{TOOL} worker`', f'{TOOL} worker{profile_arguments(args)}`')


def tick(args):
    """One pass of the coordinator: release dead claims itself, then print exactly what to do."""
    auto_update()
    print(f'taskq {version()}')
    tick_beat()
    loaded, candidates = profile(args)
    selected = {item['iid'] for item in candidates}
    stalled = [item for item in loaded[0] if item['iid'] in selected and item['state'] == 'doing' and item['age'] > STALE_MINUTES]
    for item in stalled:
        args.iid, args.action, args.text = item['iid'], 'release', f'no change on the issue for {item["age"]} minutes'
        requeue(args)
        print(f'Released stalled #{item["iid"]}.')
    loaded = load() if stalled else loaded
    # A lock on a task nobody holds: a take that died between the lock and the move, or a card moved by hand.
    held = {item['iid'] for item in loaded[0] if item['state'] not in ('ready', 'waiting')}
    for issue in issues(f'state=opened&my_reaction_emoji={LOCK}'):
        if issue['iid'] in selected and issue['iid'] not in held and all(time.time() - stamp(item['created_at']) > LOCK_SECONDS for item in locks(issue['iid'])):
            unlock(issue['iid'])
            print(f'Unlocked #{issue["iid"]}: nobody holds it.')
    # Only tick moves ready<->waiting: a card a hand moved between them goes back here.
    moved = 0
    for item in loaded[0]:
        if item['iid'] not in selected:
            continue
        open_deps = sorted(set(item['deps']) & loaded[1])
        if item['state'] == 'ready' and open_deps:
            save(item, 'waiting', 'waiting', f'open dependencies {open_deps}')
        elif item['state'] == 'waiting' and not open_deps:
            save(item, 'ready', 'ready', 'dependencies closed')
        else:
            continue
        moved += 1
        print(f'Moved #{item["iid"]} {item["state"]} → {"ready" if item["state"] == "waiting" else "waiting"}.')
    everything, _, odd, problems = loaded = load() if moved else loaded
    inventory = everything
    everything = [item for item in everything if item['iid'] in selected]
    review = [item for item in everything if item['state'] == 'review' and item['result']]
    # A question reaches the owner once, when it is new; the ones already shown come back as a daily summary.
    asked = [(item, *question(item['iid'])) for item in everything if item['state'] == 'ask']
    fresh = [(item, text) for item, text, shown in asked if shown is None]
    summary = [(item, text) for item, text, shown in asked if shown and time.time() - shown >= SUMMARY_SECONDS]
    codex_stopped = [item for item in everything if item['state'] in ('ask', 'later')
                     and (item['claim'] or {}).get('runtime') == 'codex']
    # A card moved by hand on the board into a state its data does not support.
    odd = [f'#{issue["iid"]} labels {issue["labels"]}: give it exactly one state label' for issue in odd] + [
        f'#{item["iid"]} is doing without a worker: move it back to ready or `release {item["iid"]}`'
        for item in everything if item['state'] == 'doing' and not (item['claim'] or {}).get('session')] + [
        f'#{item["iid"]} is in review without a result: `reject {item["iid"]}` or close it by hand'
        for item in everything if item['state'] == 'review' and not item['result']]
    free, start = room(inventory, args.limit), []
    for item in startable(loaded=loaded):
        if item['iid'] not in selected:
            continue
        who = item['runtime'] or max(free, key=free.get)
        if free[who] > 0:
            free[who] -= 1
            start.append({**item, 'runtime': who})
    if not (review or fresh or summary or start or codex_stopped or odd or problems) and not any(item['state'] == 'doing' for item in everything):
        return print('Nothing to do. Say so and stop.')
    print(f'You are the coordinator of the task queue for this one pass. Queue tool: `{TOOL}`\n')
    codex_doing = [item for item in everything if item['state'] == 'doing'
                   and (item['claim'] or {}).get('runtime') == 'codex']
    idle = []
    if codex_doing:
        print('## Codex sessions\n')
        for item in codex_doing:
            session = item['claim']['session']
            try:
                codex = Codex()
                try:
                    status, turns, last = codex_snapshot(codex, session, 1)
                finally:
                    codex.socket.close()
                # notLoaded with a running last turn: the app holds the session and works in it.
                working = bool(turns) and turns[0].get('app', False)
                print(f'- #{item["iid"]} {session}: {status["type"]}{" (turn running in the app)" if working else ""}; '
                      f'last event {codex_age(last)}; `{TOOL} codex-read {session}`')
                if status['type'] in ('idle', 'notLoaded') and not working and not item.get('result'):
                    idle.append(item)
            except (OSError, SystemExit, ValueError) as error:
                print(f'- #{item["iid"]} {session}: status unknown: {codex_line(error)}; '
                      f'`{TOOL} codex-read {session}`')
        print()
    if idle:
        print('## Codex idle\n\nTask is doing without result/ask, but its session has stopped. Intervene now:\n')
        for item in idle:
            print(f'- #{item["iid"]}: `{TOOL} codex-send {item["claim"]["session"]} '
                  '--text "Continue the assigned task; hand in result or ask the owner through taskq."`')
        print()
    if odd:
        print('## Board mismatch\n\nThese issues are not in a state taskq can run. Fix each:\n')
        print(''.join(f'- {line}\n' for line in odd))
    if problems:
        print('## Problems without a task\n\nRead each. Fix it now if small, else `add` a task for it; then close the '
              'issue with what was done: `glab issue close <N>` after `glab issue note <N> -m "<what was done>"`.\n')
        print(''.join(f'- #{issue["iid"]} {issue["title"]}\n' for issue in problems))
    for item in review:
        print(f'## Review #{item["iid"]}: {item["title"]}\n\n{item["text"]}\n\nHanded in:\n\n{notes(item["iid"])[-1]}\n\n'
              f'Check the result against the Acceptance above (for code and docs read the commit).\n'
              f'Accepted: `{TOOL} close {item["iid"]} --text "<what you checked>"`. '
              f'Not accepted: `{TOOL} reject {item["iid"]} --text "<what to fix>"`.\n')
        if (item['claim'] or {}).get('runtime') == 'codex':
            print(f'After close, archive its Codex session: `{TOOL} codex-archive {item["claim"]["session"]}`.\n')
    if start:
        print(f'## Start {len(start)} worker session(s)\n\n'
              f'Each worker is a new visible session of your own app that receives exactly this prompt:\n\n'
              f'    {worker_prompt(args)}\n\n'
              f'Runtime of each: ' + ', '.join(f'#{item["iid"]} {item["runtime"]}' for item in start) + '.\n'
              f'Claude worker: `{TOOL} spawn --restore <session id of the focused pane from the window layout '
              f'tool>` (keeps the owner\'s window where it was), then send the prompt to the printed `local_<id>` with the session '
              f'message tool. Codex worker: `{TOOL} spawn --runtime codex --name "T<N> <title>"`, then '
              f'`{TOOL} codex-send <printed id> --text "<prompt>"`.\n')
    named = [item for item in everything if item['state'] == 'doing' and (item['claim'] or {}).get('runtime') == 'claude' and item['claim']['session']]
    if named:
        print('## Name the worker sessions\n\nSet each session title with the session title tool, if it differs:\n')
        for item in named:
            print(f'- `local_{item["claim"]["session"]}` → `T{item["iid"]} {item["title"][:48]}`')
        print()
    if fresh:
        print('## Waiting for the owner\n\nNew questions. Do not answer these yourself. End your reply with this list, verbatim:\n')
        for item, text in fresh:
            print(f'- #{item["iid"]} {item["title"]}: {text}')
            note(item['iid'], 'shown')
    if summary:
        print('## Still waiting for the owner (daily summary)\n\nEnd your reply with this list, verbatim:\n')
        for item, text in summary:
            print(f'- #{item["iid"]} {item["title"]}: {text.splitlines()[0] if text else "no note"}')
            note(item['iid'], 'shown')
    if fresh or summary:
        print(f'\nThe owner answers with: `{TOOL} answer <N> --text "<answer>"`.')
    if codex_stopped:
        print('\n## Archive stopped Codex workers\n\nTasks in ask or later continue in a new session after answer; archive when idle:\n')
        for item in codex_stopped:
            print(f'- #{item["iid"]}: `{TOOL} codex-archive {item["claim"]["session"]}`')


# --- cleanup: report first; only proven finished rows may be applied ---------------------------

def cleanup_issues():
    """Live issues own task state and link workers to their tasks (closed issues before 2026-10-06 keep `type` in the block).
    Open issues and those closed in the last CLEANUP_DAYS: a worker of an older task is no longer proven
    finished, so its session is asked about, never removed."""
    found = {}
    after = time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(time.time() - CLEANUP_DAYS * 86400))
    for issue in issues('state=opened') + issues(f'state=closed&updated_after={after}'):
        block = BLOCK.search(issue.get('description') or '')
        if block:
            block = json.loads(block.group(1))
            found[issue['iid']] = {**block, 'closed': issue['state'] == 'closed',
                                   'type': next((label for label in issue['labels'] if label in TYPES), block.get('type')),
                                   'state': (parse(issue) or {}).get('state', 'unknown')}
    return found


def claude_sessions():
    """This machine's Claude app sessions by CLI session id: the app's own metadata (cwd, times, archived), never
    conversations. A `spawn` worker is one the app imported from the CLI (`adoptedFromOtherSurface`)."""
    found = {}
    for path in CLAUDE_APP_SESSIONS.glob('*/*/local_*.json'):
        try:
            meta = json.loads(path.read_text())
        except (OSError, ValueError):
            continue
        if meta.get('cliSessionId'):
            found[meta['cliSessionId']] = meta
    return found


def cleanup_codex(roots):
    """Read only thread metadata, never conversations; include workers whose tree is already gone."""
    codex, found = Codex(), {}
    try:
        cursor = None
        while True:
            response = codex.call('thread/list', {'archived': False, 'limit': 100, 'cursor': cursor})
            for thread in response['data']:
                cwd = Path(thread.get('cwd') or '/').resolve()
                if thread.get('projectId') == CODEX_PROJECT or cwd in roots:
                    found[thread['id']] = thread
            cursor = response.get('nextCursor')
            if not cursor:
                return found
    finally:
        codex.socket.close()


def helpers(root):
    """The project's worktree tools `cleanup` stands on (`workspace_gc`, `host_gentle`): [workspace] cleanup_helpers."""
    if not HELPERS:
        fail('cleanup needs [workspace] cleanup_helpers in taskq.toml: the folder with workspace_gc.py and host_gentle.py')
    folder = str(Path(root) / HELPERS)
    if folder not in sys.path:
        sys.path.insert(0, folder)
    import workspace_gc
    return workspace_gc


def cleanup_plan(root):
    gc = helpers(root)
    import host_gentle
    host_gentle.lower_priority()
    rows = gc.worktrees(root)
    roots = {Path(row['worktree']).resolve() for row in rows}
    issues, app = cleanup_issues(), claude_sessions()
    mine = {(runtime, os.environ[variable]) for runtime, variable in RUNTIMES.items() if os.environ.get(variable)}
    workers = {}
    for iid, issue in issues.items():
        claim = issue.get('claim') or {}
        if claim.get('runtime') and claim.get('session'):
            workers.setdefault((claim['runtime'], claim['session'].removeprefix('local_')), set()).add(iid)
    remove, ask, keep = [], [], []
    try:
        threads = cleanup_codex(roots)
        session_error = None
    except (OSError, SystemExit, RuntimeError) as error:
        threads, session_error = {}, str(error)
        ask.append({'what': 'Codex inventory', 'why': session_error,
                    'choices': [('keep', 'true'), ('check again', f'{TOOL} cleanup')]})
    current = gc._git(root, 'branch', '--show-current').strip()
    protected = {'main', current}
    owned = {}
    for iid, issue in issues.items():
        if not issue['closed']:
            # A ready task with an old claim also keeps its continuation tree.
            owned[f'worktree-taskq-{iid}'] = f'open task #{iid} ({issue["state"]})'
    for row in rows:
        branch = row.get('branch', '').removeprefix('refs/heads/')
        if any((thread.get('status') or {}).get('type') not in ('idle', 'notLoaded') or ('codex', sid) in mine
               or any(iid not in issues or not issues[iid]['closed'] for iid in workers.get(('codex', sid), ()))
               for sid, thread in threads.items() if Path(thread.get('cwd') or '/').resolve() == Path(row['worktree']).resolve()):
            owned[branch or row['worktree']] = 'current / active / unknown Codex session state'
    for identity in mine:
        for iid in workers.get(identity, ()):
            protected.add(f'worktree-taskq-{iid}')
    git = lambda *args: gc._git(root, *args)

    def merged(ref):
        # cherry ignores merges; require their ancestry separately so merge-only work is never lost.
        if git('rev-list', '--merges', f'origin/main..{ref}', '--').strip():
            return False
        return not any(line.startswith('+') for line in git('cherry', 'origin/main', ref).splitlines())

    def branch_choices(branch, remote=False):
        ref = f'origin/{branch}' if remote else branch
        choices = [('keep', 'true'), ('show diff', shlex.join(['git', 'diff', f'origin/main...{ref}', '--']))]
        if remote:
            choices = [('delete on the server', shlex.join(['git', 'push', 'origin', '--delete', branch])), ('keep', 'true')]
        else:
            choices.append(('delete', shlex.join(['git', 'branch', '-d', '--', branch])))
        return choices

    checked = set()
    for row in rows:
        tree = Path(row['worktree'])
        branch = row.get('branch', '').removeprefix('refs/heads/')
        checked.add(branch)
        what = f'tree {tree} / {branch or "detached HEAD"}'
        task_branch = f'worktree-{tree.name}'
        if tree.resolve() == root.resolve() or branch in protected or branch in owned or task_branch in owned or row['worktree'] in owned:
            keep.append({'what': what, 'why': owned.get(branch) or owned.get(task_branch) or owned.get(row['worktree']) or 'main / current branch or tree of the calling session'})
            continue
        if session_error:
            keep.append({'what': what, 'why': 'Codex session state not checked'})
            continue
        inspection = gc.inspect(root, str(tree))
        if inspection['refusals']:
            choices = [('keep', 'true'), ('show changes', shlex.join(['git', '-C', str(tree), 'status', '--short', '--untracked-files=all']))]
            holders = gc._live(tree)
            if holders:
                choices.append(('show process', shlex.join(['ps', '-p', ','.join(map(str, sorted(holders))), '-o', 'pid,ppid,comm'])))
            ask.append({'what': what, 'why': '; '.join(inspection['refusals']), 'choices': choices})
        elif not merged(branch or row['HEAD']):
            choices = branch_choices(branch) if branch else [('keep', 'true'), ('show diff', shlex.join(['git', 'diff', f'origin/main...{row["HEAD"]}', '--']))]
            if branch:
                choices[-1] = ('delete', shlex.join([sys.executable, str(root / HELPERS / 'workspace_gc.py'), 'retire', str(tree), '--delete'])
                               + ' && ' + choices[-1][1])
            ask.append({'what': what, 'why': 'commits not proven in origin/main', 'choices': choices})
        else:
            remove.append({'kind': 'tree', 'what': what, 'path': str(tree), 'branch': branch,
                           'head': row['HEAD'], 'why': 'clean, no processes; every patch in origin/main'})
    for branch in git('for-each-ref', '--format=%(refname:short)', 'refs/heads').splitlines():
        if branch in checked:
            continue
        if branch in protected or branch in owned:
            keep.append({'what': f'branch {branch}', 'why': owned.get(branch) or 'main / current branch'})
        elif merged(branch):
            remove.append({'kind': 'branch', 'what': f'branch {branch}', 'branch': branch,
                           'head': git('rev-parse', branch).strip(), 'why': 'every patch in origin/main'})
        else:
            ask.append({'what': f'branch {branch}', 'why': 'has commits outside origin/main', 'choices': branch_choices(branch)})
    for ref in git('for-each-ref', '--format=%(refname)', 'refs/remotes/origin').splitlines():
        branch = ref.removeprefix('refs/remotes/origin/')
        if branch not in ('main', 'HEAD') and merged(ref):
            ask.append({'what': f'branch origin/{branch}', 'why': 'merged; deleting on the server needs an answer of the owner',
                        'choices': branch_choices(branch, remote=True)})
    finished_trees = {Path(item['path']).resolve() for item in remove if item['kind'] == 'tree'}

    def finished(identity):
        iids = workers.get(identity, set())
        if not iids or not all(iid in issues and issues[iid]['closed'] for iid in iids):
            return False
        for iid in iids:
            issue = issues[iid]
            if issue.get('type') in ('code', 'docs'):
                sha = (issue.get('result') or {}).get('sha')
                if not sha or subprocess.run(['git', '-C', str(root), 'merge-base', '--is-ancestor', sha, 'origin/main'],
                                             capture_output=True).returncode:
                    return False
        return True

    def active_task(identity):
        return any(iid not in issues or not issues[iid]['closed'] for iid in workers.get(identity, ()))

    for sid, thread in threads.items():
        identity = ('codex', sid)
        status = (thread.get('status') or {}).get('type')
        what = f'Codex session {sid}'
        if identity in mine or status == 'active' or active_task(identity):
            keep.append({'what': what, 'why': 'current session / active / open task'})
        elif status in ('idle', 'notLoaded') and (finished(identity) or Path(thread.get('cwd') or '/').resolve() in finished_trees):
            remove.append({'kind': 'codex', 'what': what, 'thread': sid, 'cwd': thread.get('cwd'), 'why': 'not active; task closed or tree finished'})
        else:
            ask.append({'what': what, 'why': f'no proven closed task, or state unknown ({status})',
                        'choices': [('keep', 'true'), ('archive', shlex.join([TOOL, 'codex-archive', sid]))]})
    # Only this machine's app archives its sessions; an archived one is done.
    claude = {sid for runtime, sid in workers if runtime == 'claude' and sid in app and not app[sid].get('isArchived')}
    # A spawned worker that never took a task: imported from the CLI in the main checkout, idle, with no claim.
    unknown = {sid for sid, meta in app.items() if meta.get('adoptedFromOtherSurface') and not meta.get('isArchived')
               and meta.get('sessionId') == f'local_{sid}' and Path(meta.get('cwd') or '/').resolve() == root.resolve()
               and time.time() - meta.get('lastActivityAt', 0) / 1000 > STALE_MINUTES * 60} - {sid for _, sid in workers}
    for sid in sorted(claude):
        identity, what = ('claude', sid), f'Claude session local_{sid}'
        if identity in mine or active_task(identity):
            keep.append({'what': what, 'why': 'current session / open task'})
        elif finished(identity):
            remove.append({'kind': 'claude', 'what': what, 'thread': sid, 'why': 'worker of closed tasks; the coordinator checks liveness'})
        else:
            unknown.add(sid)
    for sid in sorted(unknown):
        if any(session == sid for _, session in mine):
            keep.append({'what': f'session {sid}', 'why': 'current session'})
        else:
            ask.append({'what': f'Claude session local_{sid}', 'why': 'worker without a proven closed task (spawn without a claim, or the task is not finished)',
                        'choices': [('keep', 'true'), ('archive', f'coordinator: archive_session local_{sid}')]})
    return remove, ask, keep


def cleanup(args):
    root = main_checkout(Path.cwd())
    gc = helpers(root)
    if Path.cwd().resolve() != root.resolve() or gc._git(root, 'branch', '--show-current').strip() != 'main':
        fail('cleanup runs only from the main checkout on branch main')
    # Fetch updates tracking refs only; report never changes local branches, trees or sessions.
    subprocess.run(['git', '-C', str(root), 'fetch', '-q', 'origin'], check=True)
    remove, ask, keep = cleanup_plan(root)
    for title, items in (('Remove', remove), ('Ask the owner', ask), ('Kept', keep)):
        print(f'\n# {title}')
        if not items:
            print('(none)')
        for item in items:
            print(f'- {item["what"]}: {item["why"]}')
            for label, command in item.get('choices', []):
                print(f'  {label}: {command}')
            if item.get('kind') == 'claude':
                print(f'  coordinator: archive_session local_{item["thread"]}')
    if not args.apply:
        return
    removed, freed, blocked_trees = [], 0, set()
    # Archive by cwd while the finished tree still exists; otherwise its proof would disappear.
    for item in sorted(remove, key=lambda item: item['kind'] != 'codex'):
        if item['kind'] == 'claude':
            continue  # only the coordinator's application tool can archive these
        if item['kind'] == 'tree' and Path(item['path']).resolve() in blocked_trees:
            print(f'Kept: the session of this tree is not archived: {item["what"]}')
            continue
        # Re-read live task/session/process/ref state before each act.
        fresh, _, _ = cleanup_plan(root)
        if item not in fresh:
            print(f'Kept after the recheck: {item["what"]}')
            continue
        if item['kind'] == 'tree':
            size = gc._measure(Path(item['path']), set())[0]
            done = subprocess.run([sys.executable, str(root / HELPERS / 'workspace_gc.py'), 'retire', item['path'], '--delete'], cwd=root)
            if done.returncode:
                print(f'Kept: retire refused {item["what"]}')
                continue
            freed += size
            removed.append(item['path'])
        if item['kind'] in ('tree', 'branch') and item['branch']:
            # -d can refuse a patch-equivalent rebased branch; do not force or rewrite its ref.
            done = subprocess.run(['git', '-C', str(root), '-c', f'branch.{item["branch"]}.remote=origin',
                                   '-c', f'branch.{item["branch"]}.merge=refs/heads/main', 'branch', '-d', '--', item['branch']], capture_output=True, text=True)
            if done.returncode:
                print(f'Kept branch {item["branch"]}: {done.stderr.strip()}')
            else:
                removed.append(item['branch'])
        elif item['kind'] == 'codex':
            try:
                codex_archive(argparse.Namespace(thread=item['thread']))
                removed.append(item['what'])
            except (SystemExit, OSError) as error:
                if item.get('cwd'):
                    blocked_trees.add(Path(item['cwd']).resolve())
                print(f'Kept: {item["what"]}: {error}')
    print(f'\nRemoved: {len(removed)}; freed {freed} bytes of data ({freed / gc.GIB:.3f} GiB).')
    for name in removed:
        print(f'- {name}')
    print('Physical free space may differ (APFS clones / WSL disk image).')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    sub = parser.add_subparsers(dest='action', required=True)

    def command(name, function, *arguments):
        item = sub.add_parser(name)
        item.set_defaults(function=function)
        for names, options in arguments:
            item.add_argument(*names, **options)
    iid = (('iid',), {'type': int})
    text = (('--text',), {'required': True})
    command('add', add, (('--title',), {'required': True}), (('--goal',), {'required': True}),
            (('--acceptance',), {'required': True}), (('--type',), {'required': True, 'choices': TYPES}),
            (('--scope',), {'nargs': '*', 'default': []}), (('--deps',), {'nargs': '*', 'type': int, 'default': []}),
            (('--priority',), {'type': int, 'choices': PRIORITIES, 'default': 2}),
            (('--milestone',), {'help': 'milestone title: the epic this task belongs to'}),
            (('--runtime',), {'choices': (*RUNTIMES, 'any'), 'help': 'only a session of this app may take it; default by type'}),
            (('--mine',), {'action': 'store_true'}), (('--area',), {'nargs': '+', 'default': []}))
    command('runtime', set_runtime, iid, (('runtime',), {'choices': (*RUNTIMES, 'any')}))
    command('list', listing)
    profile_flags = ((('--filter',), {'default': '', 'help': 'GitLab issues query string, passed unchanged'}),
                     (('--mine',), {'action': 'store_true'}),
                     (('--limit',), {'type': limits, 'default': limits(''), 'metavar': 'claude=N,codex=M'}))
    command('worker', worker, *profile_flags)
    command('take', take, iid)
    command('beat', beat, iid)
    command('ask', ask, iid, text)
    command('result', result, iid, text, (('--sha',), {}), (('--checks',), {'required': True}))
    for name in ('answer', 'reject', 'release'):
        command(name, requeue, iid, text)
    command('close', close, iid, text)
    command('later', later, iid, text)
    command('edit', edit, iid, (('--deps',), {'nargs': '*', 'type': int}),
            (('--milestone',), {'help': 'milestone title (epic); empty string removes it'}))
    command('tick', tick, *profile_flags)
    command('spawn', spawn, (('--runtime',), {'choices': tuple(RUNTIMES), 'default': 'claude'}),
            (('--name',), {'default': 'taskq worker', 'help': 'Codex thread name'}),
            (('--restore',), {'help': 'Claude: session to show again after the import (the focused pane)'}))
    thread = (('thread',), {})
    command('codex-send', codex_send, thread, text)
    command('codex-read', codex_read, thread,
            (('--limit',), {'type': int, 'choices': range(1, 21), 'default': 3, 'metavar': '1..20',
                           'help': 'recent turns (default 3), up to 100 latest events per turn'}))
    command('codex-archive', codex_archive, thread)
    command('problem', problem, text, (('--task',), {'type': int}))
    command('cleanup', cleanup, (('--apply',), {'action': 'store_true'}))
    for name in ('init', 'migrate'):
        command(name, migrate, (('--project',), {'help': 'GitLab project path: writes a minimal taskq.toml here if none'}),
                (('--host',), {'help': 'GitLab host for that taskq.toml, e.g. gitlab.example.com'}))
    command('contract', contract)
    command('update', update, (('--verbose',), {'action': 'store_true', 'help': 'say why a check was skipped'}))
    command('report', report, (('--hours',), {'type': int, 'default': 24}))
    args = parser.parse_args(argv)
    if getattr(args, 'project', None) and not Path('taskq.toml').exists():
        Path('taskq.toml').write_text(f'# taskq: this project\'s task queue; keys: `taskq contract`, README of taskq.\n'
                                      f'[gitlab]\nproject = "{args.project}"\n' + (f'host = "{args.host}"\n' if args.host else ''))
        print(f'wrote taskq.toml for {args.project}')
    if PROJECT is None and args.function not in (contract, update):
        configure()
    args.function(args)


if __name__ == '__main__':
    main()
