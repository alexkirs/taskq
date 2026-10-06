"""`selftest`: the queue's own mechanisms, each row a fact read back from the store."""
import argparse
import contextlib
import io
import json
import os
import re
import subprocess
import sys
import time

import taskq as core


SELFTEST_GOAL = """This is a taskq selftest task: it checks the queue, not the project. Skip the project's startup
reading and any workspace. Run only these commands from the main checkout, N being this task's number, then stop:

1. `taskq beat N`
2. If the History has no **answer** note: `taskq ask N --text "selftest question"` and stop.
3. Otherwise: `taskq result N --checks "selftest" --text "selftest result"` and stop."""


class SelftestError(Exception):
    pass


def selftest_run(calls, timeout=180):
    """taskq commands as separate processes, all at once: [(env, argv)] -> [(exit code, output)]."""
    started = [subprocess.Popen([sys.executable, '-m', 'taskq', *map(str, argv)], cwd=core.ROOT, env=env, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE) for env, argv in calls]
    outputs = [process.communicate(timeout=timeout) for process in started]
    for _, errors in outputs:
        sys.stderr.write(''.join(line for line in errors.splitlines(True) if line.startswith('taskq trace:')))
    return [(process.returncode, errors + output) for process, (output, errors) in zip(started, outputs)]


class Selftest:
    def __init__(self, args):
        self.args, self.rows, self.failed, self.created = args, [], None, []
        self.uid, self.stamp = core.user(), time.strftime('%Y%m%d%H%M%S')
        self.extra = dict(item.split('=', 1) for item in args.worker_env)
        self.record = core.ROOT / '.local' / core.SELFTEST / f'last-{self.stamp}.json'

    def records(self):
        """Every run's record but this one's, as (path, data, alive): a live run's tasks are not leftovers."""
        found = []
        for path in sorted(self.record.parent.glob('last*.json')):  # last.json: a record from before pids
            if path != self.record:
                data = json.loads(path.read_text())
                found.append((path, data, alive(data.get('pid'))))
        return found

    def step(self, mechanism, runtime, function):
        """One report row. After a failed step the rest of its chain is skipped: they would only repeat it."""
        if self.failed:
            return self.rows.append((mechanism, runtime, 'skipped', 0, f'after `{self.failed}` failed'))
        start = time.time()
        if os.environ.get('TASKQ_TRACE'):
            print(f'taskq trace: step {mechanism} ({runtime})', file=sys.stderr)
        try:
            detail, verdict = function() or '', 'ok'
        except (SelftestError, SystemExit, OSError, subprocess.SubprocessError, KeyError, TypeError, ValueError) as error:
            detail, verdict, self.failed = core.codex_line(error), 'FAIL', mechanism
        self.rows.append((mechanism, runtime, verdict, time.time() - start, detail))

    def chain(self):
        self.failed = None

    def owner(self, *argv):
        return self.call(core.selftest_env(), argv)

    def worker(self, runtime, session, *argv):
        return self.call(core.selftest_env(runtime, session, self.extra), argv)

    def call(self, env, argv):
        (code, output), = selftest_run([(env, argv)])
        if code:
            raise SelftestError(f'`taskq {argv[0]}` exit {code}: {core.last_line(output)}')
        return output

    def add(self, name, runtime):
        output = self.owner('add', '--title', f'selftest {self.stamp} {name}', '--goal', SELFTEST_GOAL, '--acceptance',
                            'The selftest reads every step back from GitLab.', '--type', 'research', '--runtime', runtime,
                            '--scope', f'.local/{core.SELFTEST}/{self.stamp}-{name}', '--label', core.SELFTEST)
        iid = int(re.search(r'^#(\d+) ', output, re.M)[1])
        self.created.append(iid)
        self.save()
        return iid

    def fact(self, iid, state=None, session=None, action=None, closed=False):
        """What GitLab says about the task: the state label, the claim, the assignee, the newest note."""
        from concurrent.futures import ThreadPoolExecutor
        with ThreadPoolExecutor(2) as pool:  # two reads at once: each glab call is ~1 s
            issue = pool.submit(core.api, 'GET', f'issues/{iid}')
            history = pool.submit(core.api, 'GET', f'issues/{iid}/notes?sort=desc&per_page=20&activity_filter=only_comments') if action else None
            issue, history, wrong = issue.result(), history and core.collaborators(history.result()), []
        item = (core.parse(issue) if issue['state'] == 'opened' else None) or {}
        if closed and issue['state'] != 'closed':
            wrong.append(f'issue is {issue["state"]}')
        if state and item.get('state') != state:
            wrong.append(f'state {item.get("state") or issue["state"]}, expected {state}')
        if session is not None and (item.get('claim') or {}).get('session') != session:
            wrong.append(f'claim {(item.get("claim") or {}).get("session")}, expected {session}')
        if state == 'doing' and [each['id'] for each in issue['assignees']] != [self.uid]:
            wrong.append(f'assignees {[each.get("username") for each in issue["assignees"]]}, expected the CLI user')
        newest = ''
        if action:
            newest = next((each['body'] for each in history if not each['body'].startswith('**shown**')), '')
            if not newest.startswith(f'**{action}**'):
                wrong.append(f'newest note {core.codex_line(newest)[:60]!r}, expected **{action}**')
        if wrong:
            raise SelftestError(f'#{iid}: ' + '; '.join(wrong))
        return f'#{iid} ' + ('closed' if closed else state or '') + (f', note {newest.splitlines()[0]}' if newest else '')

    def tick(self):
        """A coordinator pass over selftest tasks only; it keeps the real tick's last-run time."""
        before = core.TICK_BEAT.stat().st_mtime if core.TICK_BEAT.exists() else None
        try:
            return self.owner('tick', '--filter', f'labels={core.SELFTEST}', '--no-mine')  # selftest tasks are the pool's
        finally:
            if before is None:
                core.TICK_BEAT.unlink(missing_ok=True)
            else:
                os.utime(core.TICK_BEAT, (before, before))

    @staticmethod
    def mismatch(text):
        section = text.split('## Board mismatch', 1)[1].split('\n## ', 1)[0] if '## Board mismatch' in text else ''
        return {line for line in section.splitlines() if line.startswith('- #')}

    def save(self, **changes):
        """`.local/selftest/last-<stamp>.json`: what this run created and its pid, so `--scope check` and a crashed run can clean up."""
        self.record.parent.mkdir(parents=True, exist_ok=True)
        data = json.loads(self.record.read_text()) if self.record.exists() else {}
        data.update(changes, created=sorted(set(data.get('created', [])) | set(self.created)), stamp=self.stamp, pid=os.getpid())
        self.record.write_text(json.dumps(data, indent=1))

    # --- quick: the queue commands as worker processes, no sessions --------------------------------

    def quick(self, runtime):
        worker, iid = f'{core.SELFTEST}-{self.stamp}-a', None

        def add():
            nonlocal iid
            iid = self.add('quick', runtime)
            return self.fact(iid, 'ready')

        def listed():
            if not re.search(rf'^#{iid}\s', self.owner('list'), re.M):
                raise SelftestError(f'#{iid} is not in `taskq list`')
            return f'#{iid} listed'

        def by_worker(*argv, state, action, claim=True):
            self.worker(runtime, worker, *argv)
            return self.fact(iid, state, worker if claim else None, action)

        def by_owner(action, state):
            self.owner(action, iid, '--text', f'selftest {action}')
            return self.fact(iid, state, None, action)

        self.chain()
        self.step('add', runtime, add)
        self.step('list', runtime, listed)
        self.step('take, claim, assignee', runtime, lambda: by_worker('take', iid, state='doing', action='take'))
        self.step('beat', runtime, lambda: by_worker('beat', iid, state='doing', action='beat'))
        self.step('ask', runtime, lambda: by_worker('ask', iid, '--text', 'selftest question', state='ask', action='ask'))
        self.step('tick: question', runtime, lambda: self.shows(iid, '## Waiting for the owner', 'selftest question'))
        self.step('answer', runtime, lambda: by_owner('answer', 'ready'))
        self.step('take again', runtime, lambda: by_worker('take', iid, state='doing', action='take'))
        self.step('result', runtime, lambda: by_worker('result', iid, '--checks', 'selftest', '--text', 'selftest result',
                                                       state='review', action='result', claim=False))
        self.step('tick: review', runtime, lambda: self.shows(iid, f'## Review [#{iid}]', 'selftest result'))
        self.step('close', runtime, lambda: (self.owner('close', iid, '--text', 'selftest close'), self.fact(iid, closed=True, action='close'))[1])

    def shows(self, iid, *needles):
        text = self.tick()
        missing = [needle for needle in needles if needle not in text]
        if missing:
            raise SelftestError(f'tick does not print {missing}')
        if any(re.search(rf'#{iid}\b', line) for line in self.mismatch(text)):
            raise SelftestError(f'tick names #{iid} under Board mismatch')
        return f'tick prints {needles[0]}'

    def unlocked(self, iid):
        if core.locks(iid):
            raise SelftestError(f'#{iid} keeps its lock after release')
        return f'#{iid} ready, lock removed'

    def race(self, runtime):
        """Two worker processes take one task at the same moment: GitLab's lock lets exactly one through."""
        iid = self.add('race', runtime)
        sessions = [f'{core.SELFTEST}-{self.stamp}-{name}' for name in 'bc']
        done = selftest_run([(core.selftest_env(runtime, session, self.extra), ('take', iid)) for session in sessions])
        winners = [session for session, (code, _) in zip(sessions, done) if not code]
        claim = ((core.parse(core.api('GET', f'issues/{iid}')) or {}).get('claim') or {}).get('session')
        if len(winners) != 1 or claim != winners[0]:
            raise SelftestError(f'winners {winners}, claim {claim}; outputs: ' + ' / '.join(core.last_line(output) for _, output in done))
        loser = done[1 - sessions.index(winners[0])][1]
        return f'#{iid}: worker {winners[0][-1]} holds it; the other: {core.last_line(loser)}'

    # --- full: a real worker session of each app takes the task through its brief ------------------

    def full(self, runtime):
        iid, session, process, fresh = None, None, None, False
        name = f'selftest {self.stamp} {runtime}'
        log = core.ROOT / '.local' / core.SELFTEST / f'{self.stamp}-{runtime}.log'
        prompt = (f'Run `cd {core.ROOT} && {core.TOOL} worker --filter labels={core.SELFTEST} --no-mine --limit {runtime}=9` '
                  'and follow the instructions it prints.')

        def send():
            nonlocal process, fresh
            if fresh:  # spawn already started the first turn
                fresh = False
                return
            if process and process.poll() is None:
                process.wait(timeout=self.args.wait)  # one turn at a time in a CLI session
            if runtime == 'codex':
                with contextlib.redirect_stdout(io.StringIO()):
                    core.codex_send(argparse.Namespace(thread=session, text=prompt))
                return
            if runtime == 'claude':  # a background session between turns: wake it with the prompt, same id
                # No CLAUDE_WORKER_TOOLS here (#51, CLI 2.1.291): with any flag --resume starts a copy under a new id
                # (it failed live); without, the session keeps only its saved --name and --settings, so a wake
                # after a stop has the full tool set. Live workers are steered by SendMessage, not woken.
                return core.claude_wake(session, prompt, self.extra)
            command = core.selftest_command(core.EXECUTORS[runtime]['send'], session=session, text=prompt)
            with log.open('a') as out:
                process = subprocess.Popen(command, cwd=core.ROOT, env=core.selftest_env(extra=self.extra), stdout=out, stderr=subprocess.STDOUT)

        def until(state, action):
            """Poll GitLab until the worker moved the task; a turn that ended without moving it is a failure."""
            end, ended, wrong = time.time() + self.args.wait, None, None
            while time.time() < end:
                item = core.parse(core.api('GET', f'issues/{iid}')) or {}
                if item.get('state') == state:
                    try:  # #73: save() moves the label before it posts the note: poll on until the note is there too
                        return self.fact(iid, state, session if state in ('doing', 'ask') else None, action)
                    except SelftestError as error:
                        wrong = error
                # A `send` that exits 0 may only have queued the turn (a webhook): then wait the full time.
                if process and process.poll() is not None and (process.returncode or runtime not in core.EXECUTORS):
                    ended = ended or time.time()
                    if time.time() - ended > 30:
                        raise SelftestError(f'#{iid} is {item.get("state")}, the worker turn ended: '
                                            f'{core.last_line(log.read_text() if log.exists() else "")}')
                time.sleep(5)
            raise wrong or SelftestError(f'#{iid} not {state} after {self.args.wait} s')

        def add():
            nonlocal iid
            iid = self.add(runtime, runtime)
            return self.fact(iid, 'ready')

        def spawned():
            nonlocal session, fresh
            if runtime == 'claude':  # the first turn runs under CLAUDE_WORKER_TOOLS
                session, fresh = core.claude_spawn(name, self.extra, prompt), True
            elif runtime == 'codex':
                session = core.codex_spawn(name)
            else:
                session = core.last_line(subprocess.run(core.selftest_command(core.EXECUTORS[runtime]['spawn'], name=name), cwd=core.ROOT, check=True,
                                                   env=core.selftest_env(extra=self.extra), capture_output=True, text=True, timeout=300).stdout)
            self.save(sessions={**json.loads(self.record.read_text()).get('sessions', {}), runtime: session})
            return f'{runtime} session {session}'

        def first():
            send()
            until('doing', None)
            return until('ask', 'ask') + '; ' + self.notes(iid, session, 'take', 'beat')

        def again(action):
            self.owner(action, iid, '--text', f'selftest {action}')
            unlocked = self.unlocked(iid) if action == 'release' else ''
            send()
            return '; '.join(filter(None, (unlocked, until('review', 'result'))))

        self.chain()
        self.step('add', runtime, add)
        self.step('spawn', runtime, spawned)
        self.step('take, claim, assignee, beat, ask by the worker', runtime, first)
        self.step('answer, take again, result', runtime, lambda: again('answer'))
        self.step('reject, take again, result', runtime, lambda: again('reject'))
        self.step('release, take again, result', runtime, lambda: again('release'))
        self.step('close', runtime, lambda: (self.owner('close', iid, '--text', 'selftest close'), self.fact(iid, closed=True, action='close'))[1])
        if process:
            try:
                process.wait(timeout=self.args.wait)
            except subprocess.TimeoutExpired:
                process.kill()
        self.chain()
        if session:
            self.step('retire the session', runtime, lambda: selftest_retire(runtime, session, wait=self.args.wait))

    def notes(self, iid, session, *actions):
        tag = f'{session[:8]}'
        heads = [each['body'].split('\n')[0] for each in core.comments(iid)]
        missing = [action for action in actions if not any(head.startswith(f'**{action}** · ') and tag in head for head in heads)]
        if missing:
            raise SelftestError(f'#{iid}: no note {missing} by session {tag}')
        return ', '.join(f'{action} by {tag}' for action in actions)

    # --- the traces: nothing of the selftest stays on the board, in Git or in the apps ------------

    def clean(self):
        """This run's traces and those of every finished or crashed run; a live run's are its own."""
        others = self.records()
        dead = [(path, data) for path, data, live in others if not live]
        runs = [json.loads(self.record.read_text()) if self.record.exists() else {}] + [data for _, data in dead]
        created = sorted({iid for data in runs for iid in data.get('created', [])})
        busy = {iid for _, data, live in others if live for iid in data.get('created', [])}
        sessions = [(runtime, session) for data in runs for runtime, session in data.get('sessions', {}).items()]

        def held(iid):
            try:
                return core.locks(iid)
            except SystemExit as error:  # GitLab: the issue is deleted, its awards with it
                if core.gone(error):
                    return []
                raise

        def issues_gone():
            closed = []
            for iid in created:
                if held(iid):
                    core.unlock(iid)  # first: the GitHub lock ref outlives its issue
                try:
                    core.api('DELETE', f'issues/{iid}')
                except SystemExit as error:
                    if core.gone(error):
                        continue  # deleted by an earlier run
                    if core.api('GET', f'issues/{iid}')['state'] == 'opened':
                        core.api('PUT', f'issues/{iid}', {'state_event': 'close'})
                    closed.append(iid)  # the token may not delete issues: closed instead
            left = [issue['iid'] for issue in core.issues(f'state=opened&labels={core.SELFTEST}') if issue['iid'] not in busy]
            if left:
                raise SelftestError(f'open selftest issues remain: {left}')
            locked = [iid for iid in created if held(iid)]
            if locked:
                raise SelftestError(f'lock refs of selftest issues remain: {locked}')
            return f'deleted {sorted(set(created) - set(closed))}' + (f', closed (no right to delete) {closed}' if closed else '')

        def worktrees():
            listed = subprocess.run(['git', '-C', str(core.ROOT), 'worktree', 'list', '--porcelain'], capture_output=True, text=True).stdout
            left = [iid for iid in created if re.search(rf'^worktree .*taskq-{iid}$', listed, re.M)]
            if left:
                raise SelftestError(f'worktrees of selftest tasks remain: {left}')
            return 'no taskq-<N> worktree of a selftest task'

        def board():
            ours = [line for line in self.mismatch(self.tick()) if any(re.search(rf'#{iid}\b', line) for iid in created)]
            if ours:
                raise SelftestError('tick Board mismatch: ' + '; '.join(ours))
            return 'tick names no selftest task under Board mismatch'

        for mechanism, runtime, function in (
                ('remove the selftest tasks', '-', issues_gone), ('no worktree left', '-', worktrees),
                *(('no session left', runtime, lambda runtime=runtime, session=session: selftest_retire(runtime, session, check=True))
                  for runtime, session in sessions),
                # quick: each of its ticks already checked that no selftest task is under Board mismatch.
                *((('board as before', '-', board),) if self.args.scope != 'quick' else ())):
            self.chain()
            self.step(mechanism, runtime, function)
        if not any(row[2] == 'FAIL' for row in self.rows):
            for path, _ in dead:  # their traces are gone: this run's record now stands for them
                path.unlink(missing_ok=True)

    def report(self):
        lines = ['| mechanism | runtime | result | seconds | detail |', '|---|---|---|---|---|']
        lines += [f'| {mechanism} | {runtime} | {verdict} | {seconds:.0f} | {detail.replace("|", "/")} |'
                  for mechanism, runtime, verdict, seconds, detail in self.rows]
        failed, skipped = ([row for row in self.rows if row[2] == verdict] for verdict in ('FAIL', 'skipped'))
        return '\n'.join(lines) + f'\n\n{len(self.rows) - len(failed) - len(skipped)} of {len(self.rows)} ok' + (
            '; not working: ' + ', '.join(f'{row[0]} ({row[1]})' for row in failed) if failed else '') + (
            f'; {len(skipped)} skipped after a failure' if skipped else '')


def alive(pid):
    """True while the process `pid` runs (not this one)."""
    if not pid or pid == os.getpid():
        return False
    if os.name == 'nt':  # os.kill(pid, 0) terminates the process on Windows
        listed = subprocess.run(['tasklist', '/FI', f'PID eq {pid}', '/NH'], capture_output=True, text=True).stdout
        return str(pid) in listed.split()
    try:
        os.kill(pid, 0)  # ponytail: a reused pid reads as alive; the record's stamp names the run to check by hand
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def codex_interrupt(thread):
    """Interrupt the turn the shared server runs in `thread`; a turn the app runs is left to end."""
    codex = core.Codex()
    if codex.call('thread/read', {'threadId': thread})['thread']['status']['type'] != 'active':
        return
    turns = codex.call('thread/turns/list', {'threadId': thread, 'limit': 1, 'itemsView': 'notLoaded'})['data']
    if turns and turns[0]['status'] == 'inProgress':
        codex.call('turn/interrupt', {'threadId': thread, 'turnId': turns[0]['id']})


def selftest_retire(runtime, session, check=False, wait=600):
    """Archive a finished selftest worker session or, with `check`, prove it archived. A Codex thread still
    in a turn (a failed step leaves one running) is interrupted, and archived once the turn ends, up to `wait` s."""
    if runtime == 'codex':
        end = time.time() + wait
        while True:
            try:
                with contextlib.redirect_stdout(io.StringIO()) as out:
                    core.codex_archive(argparse.Namespace(thread=session))
                return out.getvalue().strip()
            except SystemExit as error:
                if check or 'is working' not in str(error) or time.time() > end:
                    raise
            try:
                codex_interrupt(session)
            except SystemExit:
                pass  # the turn ended between the two reads; the next archive takes it
            time.sleep(5)
    if runtime == 'claude':
        if not check:
            core.claude_stop(session, remove=True)
        if session in core.claude_agents():
            raise SelftestError(f'background session {session} is still in `claude agents`: `{core.TOOL} retire {session}`')
        return f'{session} retired'
    archive = core.EXECUTORS[runtime].get('archive')
    if check or not archive:
        return 'no archive command configured' if not archive else f'{session}: archived by the run'
    subprocess.run(core.selftest_command(archive, session=session), cwd=core.ROOT, check=True, capture_output=True, timeout=120)
    return f'{session} archived'


def selftest(args):
    """The queue's mechanisms through the real queue. quick: the commands as worker processes; full: a
    real worker session of each app; check: only that the traces of the last run are gone."""
    test = Selftest(args)
    if args.scope == 'check' and (live := [(data.get('stamp'), data['pid']) for _, data, alive in test.records() if alive]):
        core.fail(f'selftest check refused: the run {live[0][0]} is still alive (pid {live[0][1]}); its tasks are not leftovers. '
             f'Wait for it or stop it (`kill {live[0][1]}`), then check again.')
    if args.scope == 'quick':
        test.quick((args.runtime or [(core.session() or {}).get('runtime') or 'claude'])[0])
    elif args.scope == 'full':
        test.chain()
        test.step('parallel take, one winner', 'claude', lambda: test.race('claude'))
        for runtime in args.runtime or core.RUNTIMES:
            test.full(runtime)
    test.clean()
    text = f'taskq {core.version()}\n\n' + test.report()
    print(text)
    if args.note:
        core.note(args.note, 'selftest', f'`{core.TOOL} selftest --scope {args.scope}` from {core.who()}\n\n{text}')
    if any(row[2] == 'FAIL' for row in test.rows):
        sys.exit(1)
