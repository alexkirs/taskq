"""`cleanup`: finished worktrees, branches and worker sessions, reported first, removed only when proven finished."""
import argparse
import contextlib
import json
import os
import re
from pathlib import Path
import shlex
import subprocess
import sys
import time

import taskq as core


# --- cleanup: report first; only proven finished rows may be applied ---------------------------

def cleanup_issues():
    """Live issues own task state and link workers to their tasks (closed issues before 2026-10-06 keep `type` in the block).
    Open issues and those closed in the last CLEANUP_DAYS: a worker of an older task is no longer proven
    finished, so its session is asked about, never removed."""
    found = {}
    after = time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(time.time() - core.CLEANUP_DAYS * 86400))
    for issue in core.issues('state=opened') + core.issues(f'state=closed&updated_after={after}'):
        block = core.BLOCK.search(issue.get('description') or '')
        try:
            block = block and json.loads(block.group(1))
            if block:
                found[issue['iid']] = {**block, 'closed': issue['state'] == 'closed',
                                       'type': next((label for label in issue['labels'] if label in core.TYPES), block.get('type')),
                                       'state': (core.parse(issue) or {}).get('state', 'unknown')}
        except (ValueError, TypeError):  # a malformed block is no record: skip it
            continue
    return found


def cleanup_codex(roots):
    """Read only thread metadata, never conversations; include workers whose tree is already gone."""
    codex, found = core.Codex(), {}
    try:
        cursor = None
        while True:
            response = codex.call('thread/list', {'archived': False, 'limit': 100, 'cursor': cursor})
            for thread in response['data']:
                cwd = Path(thread.get('cwd') or '/').resolve()
                if thread.get('projectId') == core.codex_override('project') or cwd in roots:
                    found[thread['id']] = thread
            cursor = response.get('nextCursor')
            if not cursor:
                return found
    finally:
        codex.socket.close()


def process_cwds():
    """(pid, cwd) of this user's processes: `lsof` lists them on macOS and Linux. None when it cannot run."""
    try:
        done = subprocess.run(['lsof', '-a', '-d', 'cwd', '-u', str(os.getuid()), '-Fpn'], capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.SubprocessError):
        return None
    found, pid = [], None
    for line in done.stdout.splitlines():
        if line.startswith('p'):
            pid = int(line[1:])
        elif line.startswith('n') and pid:
            found.append((pid, line[1:]))
    return found if found or not done.returncode else None


class Builtin:
    """The worktree tools `cleanup` uses when the project names no helpers: plain git, `lsof` and a size walk."""
    GIB = 1024 ** 3

    @staticmethod
    def _git(root, *args):
        return subprocess.run(['git', '-C', str(root), *args], capture_output=True, text=True, check=True).stdout

    @classmethod
    def worktrees(cls, root):
        rows = []
        for block in cls._git(root, 'worktree', 'list', '--porcelain').strip().split('\n\n'):
            row = {}
            for line in block.splitlines():
                key, _, value = line.partition(' ')
                row[key] = value or True
            rows.append(row)
        return rows

    @staticmethod
    def _live(tree):
        tree = Path(tree).resolve()
        return {pid for pid, cwd in process_cwds() or () if Path(cwd) == tree or tree in Path(cwd).parents}

    @classmethod
    def inspect(cls, root, tree):
        refusals = []
        if not Path(tree).is_dir():
            return {'refusals': ['the tree folder is missing (git worktree prune drops it)']}
        if subprocess.run(['git', '-C', tree, 'status', '--porcelain', '--untracked-files=all'], capture_output=True, text=True).stdout.strip():
            refusals.append('uncommitted or untracked files')
        if any(row.get('locked') for row in cls.worktrees(root) if Path(row['worktree']).resolve() == Path(tree).resolve()):
            refusals.append('the worktree is locked')
        if process_cwds() is None:
            refusals.append('processes not checked: lsof failed')
        elif cls._live(tree):
            refusals.append('processes work in it')
        return {'refusals': refusals}

    @staticmethod
    def _measure(tree, seen):
        total = 0
        for folder, _, files in os.walk(tree):
            for name in files:
                with contextlib.suppress(OSError):
                    total += os.lstat(os.path.join(folder, name)).st_size
        return total, seen



def retire_command(root, tree):
    """Remove a finished tree: the helpers' retire, else `git worktree remove` (no --force: git refuses a dirty tree)."""
    if core.HELPERS:
        return [sys.executable, str(Path(root) / core.HELPERS / 'workspace_gc.py'), 'retire', str(tree), '--delete']
    return ['git', '-C', str(root), 'worktree', 'remove', '--', str(tree)]


def helpers(root):
    """The project's worktree tools (`workspace_gc`, `host_gentle`) from [workspace] cleanup_helpers, else the built-ins."""
    if not core.HELPERS:
        return Builtin
    folder = str(Path(root) / core.HELPERS)
    if folder not in sys.path:
        sys.path.insert(0, folder)
    import host_gentle
    import workspace_gc
    host_gentle.lower_priority()
    return workspace_gc


def cleanup_plan(root):
    gc = helpers(root)
    rows = gc.worktrees(root)
    roots = {Path(row['worktree']).resolve() for row in rows}
    issues, app = cleanup_issues(), core.claude_sessions()
    mine = {(runtime, os.environ[variable]) for runtime, variable in core.RUNTIMES.items() if os.environ.get(variable)}
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
                    'choices': [('keep', 'true'), ('check again', f'{core.TOOL} cleanup')]})
    current = gc._git(root, 'branch', '--show-current').strip()
    protected = {'main', current, 'origin/main', 'origin/HEAD'}
    protected.update(ref.removeprefix('refs/heads/').removeprefix('refs/remotes/') for ref in core.PROTECTED_REFS)
    task_name = lambda name: bool(re.fullmatch(r'(?:worktree-)?taskq-[0-9]+', name))
    spawned_trees = {Path(meta['cwd']).resolve() for sid, meta in app.items()
                     if meta.get('cwd') and meta.get('adoptedFromOtherSurface') and meta.get('sessionId') == f'local_{sid}'}
    spawned_trees.update(Path(thread['cwd']).resolve() for sid, thread in threads.items()
                         if thread.get('cwd') and ('codex', sid) in workers)
    owned = {}
    for iid, issue in issues.items():
        if not issue['closed']:
            # A ready task with an old claim also keeps its continuation tree: `.worktrees/taskq-N` (branch `taskq-N`),
            # `../taskq-N` from before 2026-10-07, or a Claude `.claude/worktrees/taskq-N` (branch `worktree-taskq-N`).
            for name in (f'taskq-{iid}', f'worktree-taskq-{iid}'):
                owned[name] = f'open task #{iid} ({issue["state"]})'
    for row in rows:
        branch = row.get('branch', '').removeprefix('refs/heads/')
        if any((thread.get('status') or {}).get('type') not in ('idle', 'notLoaded') or ('codex', sid) in mine
               or any(iid not in issues or not issues[iid]['closed'] for iid in workers.get(('codex', sid), ()))
               for sid, thread in threads.items() if Path(thread.get('cwd') or '/').resolve() == Path(row['worktree']).resolve()):
            owned[branch or row['worktree']] = 'current / active / unknown Codex session state'
    for identity in mine:
        for iid in workers.get(identity, ()):
            protected.update((f'taskq-{iid}', f'worktree-taskq-{iid}'))
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
            keep.append({'what': what, 'why': owned.get(branch) or owned.get(task_branch) or owned.get(row['worktree']) or 'protected ref / main tree'})
            continue
        task_tree = task_name(branch) if branch else task_name(tree.name) or tree.resolve() in spawned_trees
        if not task_tree:
            keep.append({'what': what, 'why': 'ownership not proven: not a taskq worker tree'})
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
                choices[-1] = ('delete', shlex.join(retire_command(root, tree)) + ' && ' + choices[-1][1])
            ask.append({'what': what, 'why': 'commits not proven in origin/main', 'choices': choices})
        else:
            remove.append({'kind': 'tree', 'what': what, 'path': str(tree), 'branch': branch,
                           'head': row['HEAD'], 'why': 'clean, no processes; every patch in origin/main'})
    for branch in git('for-each-ref', '--format=%(refname:short)', 'refs/heads').splitlines():
        if branch in checked:
            continue
        if branch in protected or branch in owned:
            keep.append({'what': f'branch {branch}', 'why': owned.get(branch) or 'protected ref'})
        elif not task_name(branch):
            keep.append({'what': f'branch {branch}', 'why': 'ownership not proven: not a taskq branch'})
        elif merged(branch):
            remove.append({'kind': 'branch', 'what': f'branch {branch}', 'branch': branch,
                           'head': git('rev-parse', branch).strip(), 'why': 'every patch in origin/main'})
        else:
            ask.append({'what': f'branch {branch}', 'why': 'has commits outside origin/main', 'choices': branch_choices(branch)})
    for ref in git('for-each-ref', '--format=%(refname)', 'refs/remotes/origin').splitlines():
        branch = ref.removeprefix('refs/remotes/origin/')
        if f'origin/{branch}' in protected or branch in protected or branch in owned:
            keep.append({'what': f'branch origin/{branch}', 'why': owned.get(branch) or 'protected ref'})
        elif not task_name(branch):
            keep.append({'what': f'branch origin/{branch}', 'why': 'ownership not proven: not a taskq branch'})
        elif merged(ref):
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
                        'choices': [('keep', 'true'), ('archive', shlex.join([core.TOOL, 'codex-archive', sid]))]})
    # Only this machine's app archives its sessions; an archived one is done.
    claude = {sid for runtime, sid in workers if runtime == 'claude' and sid in app and not app[sid].get('isArchived')}
    # A spawned worker that never took a task: imported from the CLI in the main checkout, idle, with no claim.
    unknown = {sid for sid, meta in app.items() if meta.get('adoptedFromOtherSurface') and not meta.get('isArchived')
               and meta.get('sessionId') == f'local_{sid}' and Path(meta.get('cwd') or '/').resolve() == root.resolve()
               and time.time() - meta.get('lastActivityAt', 0) / 1000 > core.STALE_MINUTES * 60} - {sid for _, sid in workers}
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
    # `claude --bg` workers of this checkout; `retire` stops them and drops them from `claude agents`.
    for sid, agent in core.claude_agents().items():
        identity, what = ('claude', sid), f'Claude background session {agent["id"]} ({agent.get("name")})'
        if Path(agent.get('cwd') or '/').resolve() != root.resolve():
            continue
        if identity in mine or active_task(identity) or agent.get('status') == 'busy':
            keep.append({'what': what, 'why': 'current session / open task / busy'})
        elif finished(identity):
            remove.append({'kind': 'claude-bg', 'what': what, 'thread': sid, 'why': 'worker of closed tasks, not busy'})
        elif not agent.get('pid'):
            # Retire keeps the transcript: a stopped or failed session of no open task can still be resumed by id.
            remove.append({'kind': 'claude-bg', 'what': what, 'thread': sid, 'why': f'{agent.get("state") or "stopped"}, no open task'})
        elif identity not in workers and time.time() - agent.get('startedAt', 0) / 1000 > core.STALE_MINUTES * 60:
            remove.append({'kind': 'claude-bg', 'what': what, 'thread': sid, 'why': f'idle, no claim, started over {core.STALE_MINUTES} min ago'})
        elif identity in workers:
            ask.append({'what': what, 'why': 'worker without a proven closed task (spawn without a claim, or the task is not finished)',
                        'choices': [('keep', 'true'), ('retire', shlex.join([core.TOOL, 'retire', sid]))]})
    return remove, ask, keep


def show(args, remove, ask, keep):
    """Print the plan; with --json also put it in the output (report only proposes, never acts)."""
    if hasattr(args, 'output'):
        args.output.update(plan={'remove': remove, 'ask': ask, 'keep': keep},
                           sessions=[item['thread'] for item in remove if item.get('thread')],
                           refusals=[{'what': item['what'], 'reason': item['why']} for item in ask + keep])
        if ask:
            args.output['outcome'] = 'judgement_needed'
        if not args.apply:
            for item in remove:
                core.record(args, 'remove', status='proposed', **item)
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


def apply_removals(args, root, gc, remove):
    """Act on the "Remove" items only, each after a fresh native recheck. Returns the #197 counts (by `what`) and
    what was removed: a refusal keeps its target (state changed, git refused, application-only), an error is an
    act that raised."""
    counts = {'attempted': [], 'succeeded': [], 'refused': [], 'errors': []}
    removed, freed, blocked_trees = [], 0, set()

    def refuse(item, reason, line, action='remove'):
        core.record(args, action, status='refused', reason=reason, **item)
        counts['refused'].append(item['what'])
        print(line)
    # Archive by cwd while the finished tree still exists; otherwise its proof would disappear.
    for item in sorted(remove, key=lambda item: item['kind'] != 'codex'):
        counts['attempted'].append(item['what'])
        try:
            if item['kind'] == 'claude':
                # Only the coordinator's application tool can archive these: a visible refusal, never a guess.
                core.record(args, 'archive', status='refused', session=item['thread'], reason='requires coordinator application tool')
                counts['refused'].append(item['what'])
                continue
            if item['kind'] == 'tree' and Path(item['path']).resolve() in blocked_trees:
                refuse(item, 'session not archived', f'Kept: the session of this tree is not archived: {item["what"]}')
                continue
            # Re-read live task/session/process/ref state before each act.
            fresh, _, _ = cleanup_plan(root)
            if item not in fresh:
                refuse(item, 'state changed on recheck', f'Kept after the recheck: {item["what"]}')
                continue
            if item['kind'] == 'tree':
                size = gc._measure(Path(item['path']), set())[0]
                if subprocess.run(retire_command(root, item['path']), cwd=root, stdout=subprocess.DEVNULL if getattr(args, 'json', False) else None).returncode:
                    refuse(item, 'retire refused', f'Kept: retire refused {item["what"]}')
                    continue
                freed += size
                removed.append(item['path'])
                core.record(args, 'remove_tree', status='done', **item)
            if item['kind'] in ('tree', 'branch') and item['branch']:
                # -d can refuse a patch-equivalent rebased branch; do not force or rewrite its ref.
                done = subprocess.run(['git', '-C', str(root), '-c', f'branch.{item["branch"]}.remote=origin',
                                       '-c', f'branch.{item["branch"]}.merge=refs/heads/main', 'branch', '-d', '--', item['branch']], capture_output=True, text=True)
                if done.returncode:
                    core.record(args, 'delete_branch', status='refused', reason=done.stderr.strip(), **item)
                    print(f'Kept branch {item["branch"]}: {done.stderr.strip()}')
                    if item['kind'] == 'branch':
                        counts['refused'].append(item['what'])
                        continue
                else:
                    removed.append(item['branch'])
                    core.record(args, 'delete_branch', status='done', **item)
            elif item['kind'] == 'codex':
                try:
                    core.codex_archive(argparse.Namespace(thread=item['thread']))
                    removed.append(item['what'])
                    core.record(args, 'archive', status='done', session=item['thread'])
                except (SystemExit, OSError) as error:
                    if item.get('cwd'):
                        blocked_trees.add(Path(item['cwd']).resolve())
                    core.record(args, 'archive', status='refused', session=item['thread'], reason=str(error))
                    print(f'Kept: {item["what"]}: {error}')
                    counts['errors'].append({'item': item['what'], 'error': str(error)})
                    continue
            elif item['kind'] == 'claude-bg':
                core.claude_stop(item['thread'], remove=True)
                removed.append(item['what'])
                core.record(args, 'retire', status='done', session=item['thread'])
            counts['succeeded'].append(item['what'])
        except (SystemExit, OSError, ValueError, subprocess.SubprocessError) as error:
            core.record(args, 'remove', status='refused', reason=f'error: {error}', **item)
            print(f'Kept after an error: {item["what"]}: {error}')
            counts['errors'].append({'item': item['what'], 'error': str(error)})
    return counts, removed, freed


def apply_plan(args, root, gc):
    """One native application: fetch, a fresh plan, its Remove items acted on with rechecks, Ask left pending."""
    subprocess.run(['git', '-C', str(root), 'fetch', '-q', 'origin'], check=True)
    remove, ask, keep = cleanup_plan(root)
    show(args, remove, ask, keep)
    counts, removed, freed = apply_removals(args, root, gc, remove)
    if hasattr(args, 'output'):
        args.output.update(removed=removed, freed_bytes=freed)
        refusals = [event for event in args.output['actions'] if event.get('status') == 'refused']
        args.output['refusals'].extend(refusals)
        if refusals:
            args.output['outcome'] = 'judgement_needed'
    print(f'\nRemoved:{len(removed)}; freed {freed} bytes of data ({freed / gc.GIB:.3f} GiB).')
    for name in removed:
        print(f'- {name}')
    print('Physical free space may differ (APFS clones / WSL disk image).')
    return {**counts, 'pending_asks': [item['what'] for item in ask]}


def apply_scheduled(args, root, gc, trigger):
    """#197: `cleanup --apply` (manual), the owner's tick (tick) and the idle stop (idle) apply only here, under one
    lock and one state. Returns the schedule report, also recorded as a `cleanup` action."""
    from taskq import cleanup_schedule as schedule
    now = schedule.datetime.now(schedule.UTC)
    report = schedule.run(schedule.settings(), schedule.state_path(), now, lambda: apply_plan(args, root, gc), trigger)
    # partial is an execution error too: status `failed` is what json_command and tick_pass (#191) turn into
    # `failure`; the action keeps outcome `partial`.
    status = {'success': 'done', 'partial': 'failed', 'failed': 'failed'}.get(report['outcome'], report['outcome'])
    core.record(args, 'cleanup', status=status, **report)
    print(f'\nCleanup ({trigger}): {report["outcome"]}, {report["reason"]}; last success {report.get("last_success") or "none"}; '
          f'next due {report.get("next_due_local") or report.get("next_due") or "none"} ({report["timezone"]}'
          + (', the missing-setting fallback' if report['timezone_fallback'] else '') + ').')
    return report


def scheduled(args):
    """The owner's tick: cleanup when due, from the main checkout on branch main only (as `cleanup` itself); the
    plan goes to the tick's log, its events to the tick's report. Never raises: a refusal is a recorded action."""
    # Events join the tick's own actions; the tick's JSON plan, refusals and outcome stay the tick's.
    actions = (args.output if hasattr(args, 'output') else args.pm_report)['actions']
    sub = argparse.Namespace(apply=True, trigger='tick', json=getattr(args, 'json', False), pm_report={'actions': actions})
    root = core.main_checkout(Path.cwd())
    try:
        gc = helpers(root)
        if Path.cwd().resolve() != root.resolve() or gc._git(root, 'branch', '--show-current').strip() != 'main':
            return core.record(sub, 'cleanup', status='refused', trigger='tick',
                               reason='scheduled cleanup runs only from the main checkout on branch main')
        with contextlib.redirect_stdout(sys.stderr):
            return apply_scheduled(sub, root, gc, 'tick')
    except (SystemExit, OSError, ValueError, subprocess.SubprocessError) as error:
        core.record(sub, 'cleanup', status='failed', trigger='tick', reason=str(error))


def cleanup(args):
    root = core.main_checkout(Path.cwd())
    gc = helpers(root)
    if Path.cwd().resolve() != root.resolve() or gc._git(root, 'branch', '--show-current').strip() != 'main':
        core.fail('cleanup runs only from the main checkout on branch main')
    if args.apply:
        report = apply_scheduled(args, root, gc, getattr(args, 'trigger', 'manual'))
        if hasattr(args, 'output'):
            args.output['cleanup'] = report
        return
    # Fetch updates tracking refs only; report never changes local branches, trees or sessions.
    subprocess.run(['git', '-C', str(root), 'fetch', '-q', 'origin'], check=True)
    show(args, *cleanup_plan(root))
