"""Full queue cycles against an in-memory GitLab; sessions are environment identities, no network."""
import contextlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import taskq as q  # noqa: E402

q.configure(Path(__file__).resolve().parent / 'taskq.toml')
# `cleanup` stands on a project's worktree tools; its tests run where a folder of them is named.
HELPERS = os.environ.get('TASKQ_CLEANUP_HELPERS')
if HELPERS:
    sys.path.insert(0, HELPERS)
CLAUDE = {'CLAUDE_CODE_SESSION_ID': 'claude-session', 'CODEX_THREAD_ID': ''}
CODEX = {'CLAUDE_CODE_SESSION_ID': '', 'CODEX_THREAD_ID': 'codex-session'}
COORDINATOR = {'CLAUDE_CODE_SESSION_ID': 'coordinator-session', 'CODEX_THREAD_ID': ''}


class CodexServer:
    """Finite app-server responses; fail immediately on an unexpected request."""
    def __init__(self):
        self.status, self.turns, self.entries, self.calls = 'idle', [], {}, []
        self.path = None
        self.socket = SimpleNamespace(close=lambda: None)

    def call(self, method, params):
        self.calls.append((method, params))
        if len(self.calls) > 100:
            raise AssertionError('unbounded Codex requests')
        if method == 'thread/read':
            return {'thread': {'status': {'type': self.status}, 'updatedAt': 9999999999, 'path': self.path}}
        if method == 'thread/turns/list':
            return {'data': [dict(turn) for turn in self.turns[:params['limit']]]}
        if method == 'thread/items/list':
            return {'data': self.entries.get(params['turnId'], [])[:params['limit']]}
        if method == 'thread/start':
            return {'thread': {'id': 'spawned'}}
        if method in ('thread/resume', 'turn/start', 'turn/steer', 'thread/name/set', 'thread/section/move',
                      'thread/unsubscribe'):
            return {}
        raise AssertionError(f'unexpected Codex method: {method}')

    def wait_turn(self, thread, seconds):
        self.calls.append(('wait_turn', {'thread': thread, 'seconds': seconds}))


class AppIpc:
    """The Codex app's IPC router: `owner` is the app window owning the thread, or None."""
    def __init__(self):
        self.owner, self.calls = None, []
        self.socket = SimpleNamespace(close=lambda: None)

    def request(self, method, params, version, target=None):
        self.calls.append((method, params, version, target))
        if method == 'thread-owner-discovery':
            return ({'resultType': 'success', 'handledByClientId': self.owner} if self.owner
                    else {'resultType': 'error', 'error': 'no-client-found'})
        if method in ('thread-follower-start-turn', 'thread-follower-steer-turn'):
            return {'resultType': 'success', 'result': {}}
        raise AssertionError(f'unexpected IPC method: {method}')


class Gitlab:
    """Issues, labels and notes the way taskq uses them; note ids are the server order, times are real."""
    def __init__(self):
        self.issues, self.notes, self.labels, self.boards, self.links = {}, {}, {}, [], set()
        self.milestones = [{'id': 5, 'title': 'Maps'}]
        self.awards, self.events = {}, {}  # award emoji by id; label events by issue
        self.uid = 1
        self.clock = 0

    def now(self):
        """Real time, but strictly increasing by at least 1 ms: the order of GitLab's writes."""
        self.clock = max(time.time(), self.clock + 0.001)
        return time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(self.clock)) + f'.{int(self.clock * 1000) % 1000:03d}Z'

    def locked(self):
        return sorted(award['iid'] for award in self.awards.values() if award['name'] == q.LOCK)

    def __call__(self, method, path, body=None):
        if path == '/user':
            return {'id': self.uid}
        if path.startswith('milestones'):
            return self.milestones
        if path.startswith('labels'):
            if method == 'DELETE':
                del self.labels[path.split('/', 1)[1]]
                return None
            if method == 'POST':
                self.labels[body['name']] = {'id': len(self.labels) + 1, **body}
            return self.labels.get(body and body['name']) or list(self.labels.values())
        if path.startswith('boards'):
            if method == 'POST' and path == 'boards':
                self.boards.append({'id': 7, 'name': body['name'], 'lists': []})
            elif method == 'POST':
                label = next(label for label in self.labels.values() if label['id'] == body['label_id'])
                self.boards[0]['lists'].append({'id': label['id'], 'label': label, 'position': len(self.boards[0]['lists'])})
                return self.boards[0]['lists'][-1]
            elif method == 'GET' and path.endswith('/lists'):
                return self.boards[0]['lists']
            elif method in ('PUT', 'DELETE'):
                lists = self.boards[0]['lists']
                item = next(item for item in lists if item['id'] == int(path.rsplit('/', 1)[1]))
                if method == 'PUT' and item['position'] == body['position']:
                    q.fail('GitLab PUT failed: List could not be moved!')  # real GitLab refuses a move in place
                lists.remove(item)
                if method == 'PUT':
                    lists.insert(body['position'], item)
                for position, each in enumerate(lists):
                    each['position'] = position
                return item
            return self.boards[-1] if method == 'POST' else self.boards
        if method == 'POST' and path == 'issues':
            iid = len(self.issues) + 1
            self.issues[iid] = {'iid': iid, 'state': 'opened', 'web_url': f'url/{iid}', 'title': body['title'],
                                'description': body['description'], 'labels': body['labels'].split(','),
                                'assignees': [{'id': uid} for uid in body.get('assignee_ids', [])],
                                'milestone_id': body.get('milestone_id'), 'updated_at': self.now(), 'created_at': self.now()}
            return self.issues[iid]
        if method == 'GET' and path.startswith('issues?'):
            label = re.search(r'labels=([^&]+)', path)
            state = re.search(r'state=(\w+)', path)[1]
            emoji = re.search(r'my_reaction_emoji=(\w+)', path)
            return [issue for issue in self.issues.values() if state in ('all', issue['state'])
                    and (not label or label[1] in issue['labels'])
                    and (not emoji or any(award['iid'] == issue['iid'] and award['name'] == emoji[1] for award in self.awards.values()))]
        iid = int(re.match(r'issues/(\d+)', path).group(1))
        if '/award_emoji' in path:
            if method == 'POST':
                if any(award['iid'] == iid and award['name'] == body['name'] and award['user']['id'] == self.uid for award in self.awards.values()):
                    q.fail(f'GitLab POST {path} failed: 404 Award Emoji Name has already been taken Not Found')
                number = max(self.awards, default=0) + 1
                self.awards[number] = {'id': number, 'iid': iid, 'name': body['name'], 'user': {'id': self.uid}, 'created_at': self.now()}
                return self.awards[number]
            if method == 'DELETE':
                del self.awards[int(path.rsplit('/', 1)[1])]
                return None
            return [award for award in self.awards.values() if award['iid'] == iid]
        if path.endswith('/resource_label_events') or '/resource_label_events?' in path:
            return self.events.get(iid, [])
        if path.endswith('/links'):
            if method == 'POST':
                assert body['link_type'] == 'relates_to' and body['target_project_id'] == q.PROJECT_PATH
                self.links |= {(iid, body['target_issue_iid']), (body['target_issue_iid'], iid)}
            return [{'iid': other} for this, other in sorted(self.links) if this == iid]
        if '/notes' in path:
            if method == 'GET':
                found = [note for note in self.notes.values() if note['iid'] == iid]
                return found[::-1] if 'sort=desc' in path else found
            if method == 'DELETE':
                del self.notes[int(path.rsplit('/', 1)[1])]
                return None
            number = max(self.notes, default=0) + 1
            self.notes[number] = {'id': number, 'iid': iid, 'body': body['body'], 'system': False, 'created_at': self.now()}
            if iid in self.issues:
                self.issues[iid]['updated_at'] = self.now()
            return self.notes[number]
        if method == 'DELETE':
            del self.issues[iid]
            return None
        issue = self.issues[iid]
        if method == 'PUT' and 'assignee_ids' in body:
            issue['assignees'] = [{'id': uid} for uid in body['assignee_ids']]
        if method == 'PUT' and 'milestone_id' in body:
            issue['milestone_id'] = body['milestone_id']
        elif method == 'PUT' and 'description' in body:
            issue['description'] = body['description']
            drop = body.get('remove_labels', '').split(',')
            issue['labels'] = [label for label in issue['labels'] if label not in drop]
            issue['labels'] += [label for label in body.get('add_labels', '').split(',') if label]
            self.events.setdefault(iid, []).extend({'action': 'add', 'label': {'name': label}, 'created_at': self.now()}
                                                   for label in body.get('add_labels', '').split(',') if label)
            issue['updated_at'] = self.now()
            if body.get('state_event') == 'close':
                issue['state'] = 'closed'
        return issue

    def said(self, iid):
        return [note['body'] for note in self.notes.values() if note['iid'] == iid]


class Cycle(unittest.TestCase):
    def setUp(self):
        self.enterContext(patch.object(q, 'AREAS', ('maps', 'engine')))
        self.gitlab = Gitlab()
        self.codex, self.ipc = CodexServer(), AppIpc()
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.directory = Path(directory.name)
        self.agents = {}  # `claude agents --json --all`, by session id
        for target, value in (('api', self.gitlab), ('claude_agents', lambda: self.agents),
                              ('Codex', lambda **kwargs: self.codex), ('CodexIpc', lambda **kwargs: self.ipc)):
            patcher = patch.object(q, target, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def do(self, who, *argv):
        with patch.dict(os.environ, who), contextlib.redirect_stdout(io.StringIO()) as out:
            q.main([str(item) for item in argv])
        return out.getvalue()

    def refused(self, who, *argv):
        with self.assertRaises(SystemExit) as caught, contextlib.redirect_stdout(io.StringIO()):
            with patch.dict(os.environ, who):
                q.main([str(item) for item in argv])
        return str(caught.exception)

    def add(self, *extra):
        self.do(CLAUDE, 'add', '--title', 't', '--goal', 'g', '--acceptance', 'a', *extra)
        return len(self.gitlab.issues)

    def state(self, iid):
        return q.parse(self.gitlab.issues[iid])['state']

    def test_question_then_another_runtime_continues_and_tick_guides(self):
        iid = self.add('--type', 'research', '--runtime', 'any')
        self.assertIn('Start 1 worker', self.do(CLAUDE, 'tick'))
        self.assertIn(f'take {iid}', self.do(CODEX, 'worker'))
        self.do(CODEX, 'take', iid)
        self.do(CODEX, 'ask', iid, '--text', 'which one?')
        waiting = self.do(CLAUDE, 'tick')
        self.assertIn('which one?', waiting)
        self.assertNotIn('Start', waiting)
        self.do(CLAUDE, 'answer', iid, '--text', 'the first')
        # A session of the other runtime continues: the question and the answer are in its brief.
        continued = self.do(CLAUDE, 'worker')
        self.assertIn('which one?', continued)
        self.assertIn('the first', continued)
        self.do(CLAUDE, 'take', iid)
        self.assertIn('not claimed by this session', self.refused(CODEX, 'result', iid, '--text', 'x', '--checks', 'x'))
        self.do(CLAUDE, 'result', iid, '--text', 'done', '--checks', 'none')
        self.assertIn(f'close {iid}', self.do(CODEX, 'tick'))
        self.do(CODEX, 'reject', iid, '--text', 'more')
        self.do(CODEX, 'take', iid)
        self.do(CODEX, 'result', iid, '--text', 'done again', '--checks', 'none')
        self.assertIn('codex-archive codex-session', self.do(CLAUDE, 'tick'))
        self.do(CODEX, 'problem', '--task', iid, '--text', 'glab was slow')
        self.do(CLAUDE, 'close', iid, '--text', 'ok')
        report = self.do(CLAUDE, 'report')
        self.assertIn(f'#{iid}: take → ask +0m → answer', report)
        self.assertIn('glab was slow', report)
        self.assertIn('codex:codex-se', report)
        self.assertEqual(self.gitlab.issues[iid]['state'], 'closed')
        self.assertFalse([label for label in self.gitlab.issues[iid]['labels'] if label.startswith('q-')])
        self.assertIn('Nothing to do', self.do(CLAUDE, 'tick'))

    def race(self, first, second, at='lock'):
        """Two workers on two machines take at once: `second` runs its whole take while `first` stops right
        after its lock (`at='lock'`) or right after moving its task to doing (`at='save'`). Nothing local is
        shared: only the fake GitLab is common. Returns who got a task."""
        won, paused = [], []

        def take(who, iid):
            try:
                self.do(who, 'take', iid)
                won.append(who['CLAUDE_CODE_SESSION_ID'] or who['CODEX_THREAD_ID'])
            except SystemExit as refusal:
                self.assertIn('cannot start', str(refusal))

        def pause(original):
            def step(*args, **kwargs):
                value = original(*args, **kwargs)
                if not paused and (at == 'lock' or kwargs.get('claim', {}) and args[1:2] == ('doing',)):
                    paused.append(1)
                    take(*second)
                return value
            return step
        with patch.object(q, at, pause(getattr(q, at))):
            take(*first)
        # A lock is held only by a task someone holds.
        self.assertEqual(self.gitlab.locked(), sorted(iid for iid in self.gitlab.issues if self.state(iid) == 'doing'))
        return won

    def test_two_machines_take_at_once_exactly_one_wins(self):
        other = {'CLAUDE_CODE_SESSION_ID': 'other-machine', 'CODEX_THREAD_ID': ''}
        same = self.add('--type', 'code')
        self.assertEqual(self.race((CLAUDE, same), (other, same)), ['claude-session'])
        self.assertEqual(q.parse(self.gitlab.issues[same])['claim']['session'], 'claude-session')
        # Overlapping scope, two tasks: the take that entered doing later gives way, whichever reads first.
        for at, winner in (('lock', 'codex-session'), ('save', 'other-machine')):
            left, right = self.add('--type', 'code', '--scope', f'v1/{at}'), self.add('--type', 'code', '--scope', f'v1/{at}/a.rs', '--runtime', 'codex')
            self.assertEqual(self.race((other, left), (CODEX, right), at), [winner])
            self.assertEqual(sorted(self.state(iid) for iid in (left, right)), ['doing', 'ready'])
            for iid in (left, right):
                if self.state(iid) == 'doing':
                    self.do(COORDINATOR, 'release', iid, '--text', 'next case')
        # Known property: the last place in LIMIT may go to both for the seconds of a race.
        self.do(COORDINATOR, 'release', same, '--text', 'free the place')
        third = {'CLAUDE_CODE_SESSION_ID': 'third-machine', 'CODEX_THREAD_ID': ''}
        self.do(third, 'take', same)
        one, two = self.add('--type', 'code', '--scope', 'a'), self.add('--type', 'code', '--scope', 'b')
        self.assertEqual(sorted(self.race((third, one), (CLAUDE, two))), ['claude-session', 'third-machine'])

    def test_lock_success_404_release_and_crash(self):
        iid = self.add('--type', 'research', '--runtime', 'any')
        self.assertTrue(q.lock(iid))
        self.assertFalse(q.lock(iid))  # GitLab's 404 «has already been taken»
        # A take that died between its lock and the move: refused until tick clears the old lock.
        self.assertIn('another worker holds its lock', self.refused(CLAUDE, 'take', iid))
        self.do(COORDINATOR, 'tick')
        self.assertEqual(self.gitlab.locked(), [iid])  # younger than LOCK_SECONDS: maybe a take in progress
        with patch.object(q, 'LOCK_SECONDS', -1):
            self.assertIn(f'Unlocked #{iid}', self.do(COORDINATOR, 'tick'))
        self.do(CLAUDE, 'take', iid)
        self.assertEqual(self.gitlab.locked(), [iid])
        self.do(COORDINATOR, 'release', iid, '--text', 'dead worker')
        self.assertEqual(self.gitlab.locked(), [])
        self.do(CODEX, 'take', iid)
        self.do(CODEX, 'result', iid, '--text', 'done', '--checks', 'none')
        self.do(COORDINATOR, 'reject', iid, '--text', 'again')
        self.assertEqual(self.gitlab.locked(), [])
        self.do(CODEX, 'take', iid)
        self.do(CODEX, 'result', iid, '--text', 'done', '--checks', 'none')
        self.do(COORDINATOR, 'close', iid, '--text', 'ok')
        self.assertEqual(self.gitlab.locked(), [])

    def test_beat_keeps_one_note_and_problem_without_task_is_an_issue(self):
        iid = self.add('--type', 'research')
        self.do(CLAUDE, 'take', iid)
        for _ in range(3):
            self.do(CLAUDE, 'beat', iid)
        self.assertEqual([body.split(' ')[0] for body in self.gitlab.said(iid)], ['**take**', '**beat**'])
        self.do(CLAUDE, 'problem', '--task', iid, '--text', 'slow')
        self.do(CLAUDE, 'beat', iid)
        self.assertEqual([body.split(' ')[0] for body in self.gitlab.said(iid)], ['**take**', '**beat**', '**problem**', '**beat**'])
        self.assertIn('url/', self.do(CLAUDE, 'problem', '--text', 'glab hung\nfor a minute'))
        problem = len(self.gitlab.issues)
        self.assertEqual(self.gitlab.issues[problem]['labels'], ['problem'])
        self.assertIn(f'#{problem} problem: glab hung', self.do(COORDINATOR, 'tick'))
        self.assertIn(f'#{problem}    problem  problem: glab hung', self.do(COORDINATOR, 'list'))
        self.assertIn('glab hung', self.do(COORDINATOR, 'report').split('# Problems')[1])
        self.gitlab.issues[problem]['state'] = 'closed'
        self.assertNotIn('glab hung', self.do(COORDINATOR, 'tick'))

    def test_migrate_orders_the_board_drops_old_states_and_links_deps(self):
        self.gitlab('POST', 'labels', {'name': 'q-held'})
        self.gitlab('POST', 'boards', {'name': q.BOARD})
        for name in ('q-ready', 'q-doing', 'q-review', 'q-ask'):
            self.gitlab('POST', 'labels', {'name': name})
        for name in ('q-ready', 'q-doing', 'q-review', 'q-ask', 'q-held'):
            self.gitlab('POST', 'boards/7/lists', {'label_id': self.gitlab.labels[name]['id']})
        old = self.gitlab('POST', 'issues', {'title': 't', 'labels': 'q-held,code',
                                             'description': q.render('g', {'scope': [], 'deps': []})})['iid']
        first = self.add('--type', 'code')
        second = self.add('--type', 'code')
        self.gitlab.links.clear()
        current = q.parse(self.gitlab.issues[second])
        q.save(current, deps=[first])  # deps written before links existed
        self.assertIn(f'label q-held kept: still on [{old}]', self.do(CLAUDE, 'migrate'))
        self.assertEqual([item['label']['name'] for item in self.gitlab.boards[0]['lists']], [f'q-{state}' for state in q.STATES])
        self.assertIn((second, first), self.gitlab.links)
        self.gitlab.issues[old]['labels'] = ['code']
        before = json.dumps(self.gitlab.boards)
        self.assertNotIn('kept', self.do(CLAUDE, 'migrate'))
        self.assertNotIn('q-held', self.gitlab.labels)
        self.assertEqual(before, json.dumps(self.gitlab.boards))

    def test_init_writes_a_config_then_labels_and_board_once(self):
        with tempfile.TemporaryDirectory() as tmp:
            before = Path.cwd()
            os.chdir(tmp)
            try:
                self.assertIn('wrote taskq.toml for group/new', self.do(CLAUDE, 'init', '--project', 'group/new'))
                self.assertIn('project = "group/new"', Path('taskq.toml').read_text())
                labels, boards = json.dumps(self.gitlab.labels), json.dumps(self.gitlab.boards)
                self.assertNotIn('wrote', self.do(CLAUDE, 'init', '--project', 'group/new'))
            finally:
                os.chdir(before)
        self.assertEqual((labels, boards), (json.dumps(self.gitlab.labels), json.dumps(self.gitlab.boards)))
        self.assertTrue({'q-ready', 'run-codex', 'asset', 'problem', 'priority-1', 'priority-2'} <= set(self.gitlab.labels))
        self.assertEqual([item['label']['name'] for item in self.gitlab.boards[0]['lists']], [f'q-{state}' for state in q.STATES])
        paths = self.do(CLAUDE, 'contract').split()
        self.assertEqual([Path(path).name for path in paths], ['taskq-manager.md', 'taskq.md'])
        self.assertTrue(all(Path(path).is_file() for path in paths))

    def test_tick_moves_ready_and_waiting_by_dependencies(self):
        dep = self.add('--type', 'research', '--scope', 'a')
        iid = self.add('--type', 'code', '--scope', 'b', '--deps', dep)
        self.assertIn((iid, dep), self.gitlab.links)  # add links each dependency
        self.assertIn(f'Moved #{iid} ready → waiting', self.do(CLAUDE, 'tick'))
        self.assertEqual(self.state(iid), 'waiting')
        self.assertIn(f'open dependencies [{dep}]', self.do(CLAUDE, 'list'))
        # A hand moving it back to ready while the dependency is open is undone by the next tick.
        self.gitlab('PUT', f'issues/{iid}', {'description': self.gitlab.issues[iid]['description'],
                                             'add_labels': 'q-ready', 'remove_labels': 'q-waiting'})
        self.assertIn(f'Moved #{iid} ready → waiting', self.do(CLAUDE, 'tick'))
        self.gitlab.issues[dep]['state'] = 'closed'
        self.assertIn(f'Moved #{iid} waiting → ready', self.do(CLAUDE, 'tick'))
        self.assertEqual(self.state(iid), 'ready')
        other = self.add('--type', 'code', '--scope', 'c')
        self.do(CLAUDE, 'edit', iid, '--deps', other, '--milestone', 'Maps')
        self.assertIn((iid, other), self.gitlab.links)
        self.assertEqual(self.gitlab.issues[iid]['milestone_id'], 5)
        self.assertIn(f'Moved #{iid} ready → waiting', self.do(CLAUDE, 'tick'))
        self.assertIn("no active milestone 'None'", self.refused(CLAUDE, 'edit', iid, '--milestone', 'None'))

    def test_question_is_shown_once_then_in_the_daily_summary(self):
        iid = self.add('--type', 'code', '--scope', 'a')
        self.do(CLAUDE, 'ask', iid, '--text', 'A or B?')  # the manager asks about a task not started
        self.assertIn('A or B?', self.do(CLAUDE, 'tick'))
        self.assertNotIn('A or B?', self.do(CLAUDE, 'tick'))  # shown within the day
        for item in self.gitlab.notes.values():
            if item['body'].startswith('**shown**'):
                item['created_at'] = '2000-01-01T00:00:00.000Z'
        again = self.do(CLAUDE, 'tick')
        self.assertIn('daily summary', again)
        self.assertIn('A or B?', again)
        self.assertNotIn('New questions', again)
        self.assertNotIn('A or B?', self.do(CLAUDE, 'tick'))  # the summary is a new `shown`
        self.do(CLAUDE, 'answer', iid, '--text', 'A')
        self.assertNotIn('shown', self.do(CLAUDE, 'worker'))  # the marker is not task history
        self.do(CLAUDE, 'take', iid)
        self.do(CLAUDE, 'ask', iid, '--text', 'C or D?')
        self.assertIn('New questions', self.do(CLAUDE, 'tick'))  # a new question after the answer is new again
        later = self.add('--type', 'docs', '--scope', 'b')
        self.do(CLAUDE, 'later', later, '--text', 'after the beta')
        self.assertIn('after the beta', self.do(CLAUDE, 'list'))
        self.assertNotIn(f'take {later}', self.do(CLAUDE, 'worker'))
        self.do(CLAUDE, 'answer', later, '--text', 'now')
        self.assertEqual(self.state(later), 'ready')

    def test_tick_names_cards_moved_by_hand(self):
        doing, review, off = self.add('--type', 'code'), self.add('--type', 'research'), self.add('--type', 'docs')
        for iid, add, remove in ((doing, 'q-doing', 'q-ready'), (review, 'q-review', 'q-ready'), (off, '', 'q-ready')):
            self.gitlab('PUT', f'issues/{iid}', {'description': self.gitlab.issues[iid]['description'],
                                                 'add_labels': add, 'remove_labels': remove})
        output = self.do(CLAUDE, 'tick')
        self.assertIn(f'#{doing} is doing without a worker', output)
        self.assertIn(f'#{review} is in review without a result', output)
        self.assertIn(f'#{off} labels', output)
        self.assertIn('not a valid task', self.do(CLAUDE, 'list'))

    def test_code_brief_names_one_tree_and_the_prepare_table(self):
        iid = self.add('--type', 'code')
        brief = self.do(CLAUDE, 'worker')
        self.assertIn(f'make worktree NAME=taskq-{iid}`', brief)
        self.assertNotIn('LIGHT', brief)
        self.assertIn('make prepare WHAT=', brief)

    def test_admission_scope_dependency_limit(self):
        first = self.add('--type', 'code', '--scope', 'v1/sim')
        second = self.add('--type', 'code', '--scope', 'v1/sim/src/rigid.rs')
        third = self.add('--type', 'docs', '--scope', 'docs/a.md', '--deps', first)
        self.assertIn('make worktree NAME=taskq-1', self.do(CLAUDE, 'worker'))
        self.do(CLAUDE, 'take', first)
        self.do(CLAUDE, 'ask', first, '--text', 'q')
        self.do(COORDINATOR, 'answer', first, '--text', 'a')
        self.assertIn('started before in worktree `taskq-1`', self.do(CLAUDE, 'worker'))
        self.do(CLAUDE, 'take', first)
        self.assertIn(f'scope overlaps #{first}', self.refused(CLAUDE, 'take', second))
        self.assertIn('open dependencies', self.refused(CLAUDE, 'take', third))
        self.assertIn('No task can start', self.do(CODEX, 'worker'))
        self.assertIn('needs --sha', self.refused(CLAUDE, 'result', first, '--text', 'x', '--checks', 'x'))
        self.assertIn('is yours', self.do(CLAUDE, 'take', self.add('--type', 'research')))

    def test_owner_answers_in_the_worker_session_at_a_full_limit(self):
        # The worker asked, the owner answered in the worker's own session while both claude
        # places were taken by others; the worker records the answer and hands in without the coordinator.
        iid = self.add('--type', 'research')
        self.do(CLAUDE, 'take', iid)
        asked = self.do(CLAUDE, 'ask', iid, '--text', 'publish?')
        self.assertIn(f'answer {iid}', asked)
        for name in ('other-1', 'other-2'):
            self.do({'CLAUDE_CODE_SESSION_ID': name, 'CODEX_THREAD_ID': ''}, 'take', self.add('--type', 'research'))
        self.assertIn('is doing again', self.do(CLAUDE, 'answer', iid, '--text', 'yes'))
        current = q.parse(self.gitlab.issues[iid])
        self.assertEqual((current['state'], current['claim']['session']), ('doing', 'claude-session'))
        self.assertIn('is yours', self.do(CLAUDE, 'take', iid))  # a repeated take is not refused by the limit
        self.do(CLAUDE, 'result', iid, '--text', 'published', '--checks', 'none')
        self.assertEqual(self.state(iid), 'review')
        self.assertIn(f'#{iid}: take → ask +0m → answer +0m → take +0m → result', self.do(CLAUDE, 'report'))

    def test_coordinator_answer_requeues_and_drops_the_session(self):
        iid = self.add('--type', 'research')
        self.do(CLAUDE, 'take', iid)
        self.do(CLAUDE, 'ask', iid, '--text', 'q')
        self.do(COORDINATOR, 'answer', iid, '--text', 'a')
        current = q.parse(self.gitlab.issues[iid])
        self.assertEqual((current['state'], current['claim']), ('ready', {'runtime': None, 'session': None}))
        with patch.dict(os.environ, {'CLAUDE_CODE_SESSION_ID': '', 'CODEX_THREAD_ID': ''}):  # the owner's shell
            other = self.add('--type', 'research')
            self.do(CLAUDE, 'take', other)
            self.do(CLAUDE, 'ask', other, '--text', 'q')
            q.main(['answer', str(other), '--text', 'a'])
        self.assertEqual(self.state(other), 'ready')

    def test_brief_says_how_to_record_an_answer_given_in_the_session(self):
        iid = self.add('--type', 'research')
        self.assertIn(f'answer {iid} --text', self.do(CLAUDE, 'worker'))

    def test_runtime_default_by_type_and_runtime_command(self):
        runtime = lambda iid: q.parse(self.gitlab.issues[iid])['runtime']
        asset, code, anyone = self.add('--type', 'asset'), self.add('--type', 'code'), self.add('--type', 'docs', '--runtime', 'any')
        self.assertEqual((runtime(asset), runtime(code), runtime(anyone)), ('codex', 'claude', None))
        self.do(CLAUDE, 'runtime', code, 'codex')
        self.assertEqual(runtime(code), 'codex')
        self.assertIn('**runtime** · claude:claude-s\n\nclaude → codex', self.gitlab.said(code)[-1])
        self.assertIn('run-codex', self.gitlab.issues[code]['labels'])
        self.assertNotIn('runtime', q.BLOCK.search(self.gitlab.issues[code]['description'])[1])
        self.do(CLAUDE, 'runtime', code, 'any')
        self.assertIsNone(runtime(code))
        self.do(CODEX, 'take', asset)
        self.assertIn('is doing', self.refused(CLAUDE, 'runtime', asset, 'claude'))

    def test_profile_limits_are_local_and_manual_take_ignores_them(self):
        first = self.add('--type', 'code')
        self.do(CLAUDE, 'take', first)
        second = self.add('--type', 'code')
        self.assertIn('No task can start', self.do(CLAUDE, 'worker', '--limit', 'claude=1,codex=0'))
        self.assertNotIn('Start 1 worker', self.do(CLAUDE, 'tick', '--limit', 'claude=1,codex=0'))
        with patch.object(q.socket, 'gethostname', return_value='another-machine'):
            self.assertIn(f'take {second}', self.do(CLAUDE, 'worker', '--limit', 'claude=1,codex=0'))
        self.assertIn('is yours', self.do(CLAUDE, 'take', second))
        self.assertEqual(self.refused(CLAUDE, 'worker', '--limit', 'claude=-1'), '2')

    def test_legacy_codex_locality_uses_files_without_app_server(self):
        root = self.directory / 'sessions' / '2026' / '10' / '06'
        root.mkdir(parents=True)
        (root / 'rollout-2026-10-06-local-thread.jsonl').write_text('')
        with patch.dict(os.environ, {'CODEX_HOME': str(self.directory)}):
            self.assertTrue(q.local_claim({'runtime': 'codex', 'session': 'local-thread'}))
            self.assertFalse(q.local_claim({'runtime': 'codex', 'session': 'remote-thread'}))

    def test_worker_retry_preserves_profile_arguments(self):
        self.add('--type', 'code', '--mine', '--area', 'maps')
        text = self.do(CLAUDE, 'worker', '--filter', 'labels=area-maps', '--mine', '--limit', 'claude=1,codex=0')
        self.assertIn('taskq worker --filter labels=area-maps --mine --limit claude=1,codex=0', text)

    def test_legacy_local_claims_still_fill_machine_slots(self):
        for sid in ('234', '240'):
            iid = self.add('--type', 'code')
            self.do(CLAUDE, 'take', iid)
            issue = self.gitlab.issues[iid]
            current = q.parse(issue)
            block = {key: current.get(key) for key in q.FIELDS}
            block['claim'] = {'runtime': 'claude', 'session': sid}
            issue['description'] = q.render(current['text'], block)
            folder = self.directory / 'account' / 'org'
            folder.mkdir(parents=True, exist_ok=True)
            (folder / f'local_{sid}.json').write_text('{}')
        self.add('--type', 'code')
        with patch.object(q, 'CLAUDE_APP_SESSIONS', self.directory):
            self.assertNotIn('Start 1 worker', self.do(CLAUDE, 'tick', '--limit', 'claude=2,codex=0'))
            self.assertIn('No task can start', self.do(CLAUDE, 'worker', '--limit', 'claude=2,codex=0'))
        with patch.object(q, 'CLAUDE_APP_SESSIONS', self.directory / 'different-machine'):
            self.assertIn('Start 1 worker', self.do(CLAUDE, 'tick', '--limit', 'claude=2,codex=0'))

    def test_two_users_assignees_pool_manual_take_and_ask(self):
        mine = self.add('--type', 'code', '--mine', '--area', 'maps')
        pool = self.add('--type', 'code', '--area', 'maps')
        self.gitlab.uid = 2
        self.assertIn(f'take {pool}', self.do(CLAUDE, 'worker', '--filter', 'labels=area-maps'))
        self.assertIn('No task can start', self.do(CLAUDE, 'worker', '--mine'))
        self.do(CLAUDE, 'take', pool)
        self.assertEqual(self.gitlab.issues[pool]['assignees'], [{'id': 2}])
        self.do(CLAUDE, 'take', mine)
        self.assertEqual(self.gitlab.issues[mine]['assignees'], [{'id': 2}])
        self.do(CLAUDE, 'ask', mine, '--text', 'Only second user sees this')
        self.gitlab.uid = 1
        self.assertNotIn('Only second user sees this', self.do(CLAUDE, 'tick'))
        self.gitlab.uid = 2
        self.assertIn('Only second user sees this', self.do(CLAUDE, 'tick', '--mine'))
        self.do(CLAUDE, 'answer', mine, '--text', 'yes')
        self.do(CLAUDE, 'result', mine, '--sha', 'abc123', '--text', 'done', '--checks', 'ok')
        self.gitlab.uid = 1
        self.assertNotIn(f'## Review #{mine}', self.do(CLAUDE, 'tick'))
        self.gitlab.uid = 2
        self.assertIn(f'## Review #{mine}', self.do(CLAUDE, 'tick', '--mine'))
        typo = self.do(CLAUDE, 'tick', '--filter', 'labels=area-typo')
        self.assertIn('candidates=0', typo)
        self.assertIn('Warning:', typo)

    def test_filter_keeps_dependency_and_scope_safety(self):
        engine = self.add('--type', 'code', '--area', 'engine', '--scope', 'shared')
        maps = self.add('--type', 'code', '--area', 'maps', '--deps', engine)
        self.assertIn('No task can start', self.do(CLAUDE, 'worker', '--filter', 'labels=area-maps'))
        self.do(CLAUDE, 'take', engine)
        self.add('--type', 'code', '--area', 'maps', '--scope', 'shared')
        self.assertIn('No task can start', self.do(CLAUDE, 'worker', '--filter', 'labels=area-maps'))

    def test_two_users_race_one_task_only_first_wins(self):
        iid = self.add('--type', 'code', '--runtime', 'any')
        real, raced = self.gitlab, False
        def interleaved(method, path, body=None):
            nonlocal raced
            if not raced and method == 'GET' and '/award_emoji?' in path:
                raced = True
                self.gitlab.uid = 2
                try:
                    self.assertIn('another worker holds its lock', self.refused(CODEX, 'take', iid))
                finally:
                    self.gitlab.uid = 1
            return real(method, path, body)
        with patch.object(q, 'api', interleaved):
            self.assertIn('is yours', self.do(CLAUDE, 'take', iid))
        self.assertEqual(self.gitlab.issues[iid]['assignees'], [{'id': 1}])
        self.assertEqual(q.parse(self.gitlab.issues[iid])['claim']['session'], 'claude-session')
        self.assertEqual(self.gitlab.locked(), [iid])

    def test_cross_user_lock_first_reaction_wins(self):
        iid = self.add('--type', 'code')
        self.assertTrue(q.lock(iid))
        self.gitlab.uid = 2
        self.assertFalse(q.lock(iid))
        q.unlock(iid)
        self.assertEqual(self.gitlab.locked(), [iid])
        self.gitlab.uid = 1
        q.unlock(iid)
        self.assertEqual(self.gitlab.locked(), [])

    def test_codex_doing_does_not_block_claude(self):
        for _ in range(2):
            self.do(CODEX, 'take', self.add('--type', 'asset'))
        claude = self.add('--type', 'code')
        self.assertIn(f'take {claude}', self.do(CLAUDE, 'worker'))
        self.assertIn(f'#{claude} claude', self.do(CLAUDE, 'tick'))
        self.do(CLAUDE, 'take', claude)

    def test_runtime_pins_who_may_take_a_task(self):
        pinned = self.add('--type', 'research', '--runtime', 'codex')
        self.assertIn('codex  t', self.do(CLAUDE, 'list'))
        self.assertIn(f'#{pinned} codex', self.do(CLAUDE, 'tick'))
        self.assertIn('No task can start', self.do(CLAUDE, 'worker'))
        self.assertIn('runtime is codex', self.refused(CLAUDE, 'take', pinned))
        anyone = self.add('--type', 'research')
        self.assertIn(f'take {anyone}', self.do(CLAUDE, 'worker'))
        self.assertIn(f'take {pinned}', self.do(CODEX, 'worker'))
        self.do(CODEX, 'take', pinned)
        self.do(CODEX, 'ask', pinned, '--text', 'q')
        self.do(CLAUDE, 'answer', pinned, '--text', 'a')
        self.assertEqual(q.parse(self.gitlab.issues[pinned])['runtime'], 'codex')
        self.assertIn('runtime is codex', self.refused(CLAUDE, 'take', pinned))

    def test_tick_releases_a_stalled_task(self):
        iid = self.add('--type', 'research', '--runtime', 'any')
        self.do(CODEX, 'take', iid)
        self.assertIn('last change 0 min ago', self.do(CLAUDE, 'list'))
        with patch.object(q, 'STALE_MINUTES', -1):
            self.assertIn(f'Released stalled #{iid}', self.do(CLAUDE, 'tick'))
        self.assertEqual(self.state(iid), 'ready')
        self.assertEqual(self.gitlab.locked(), [])  # the release took the lock off
        self.assertIn('continue', self.do(CLAUDE, 'list'))
        self.do(CLAUDE, 'take', iid)

    def test_codex_archive_refuses_a_working_thread(self):
        class Server:
            calls = []
            status, path, held, app_running = 'active', '/s/rollout.jsonl', False, False

            def call(self, method, params):
                self.calls.append(method)
                if method == 'thread/archive' and Server.held:
                    raise SystemExit('taskq: Codex thread/archive: already has an active writer')
                if method == 'thread/turns/list':
                    return {'data': [{'id': 'u1', 'status': 'interrupted'}]}
                return {'thread': {'status': {'type': Server.status}, 'path': Server.path}}
        announced = []
        with patch.object(q, 'Codex', Server), patch.object(q, 'codex_announce', lambda *a: announced.append(a)), \
                patch.object(q, 'codex_app_running', lambda metadata, turn: Server.app_running):
            self.assertIn('is working; not archived', self.refused(CLAUDE, 'codex-archive', 't1'))
            Server.status, Server.app_running = 'notLoaded', True
            self.assertIn('is working; not archived', self.refused(CLAUDE, 'codex-archive', 't1'))
            self.assertNotIn('thread/archive', Server.calls)
            Server.app_running, Server.held = False, True
            refusal = self.refused(CLAUDE, 'codex-archive', 't1')
            self.assertIn('Cmd+Shift+A', refusal)
            self.assertIn('codex://threads/t1', refusal)
            Server.held = False
            self.assertIn('archived t1', self.do(CLAUDE, 'codex-archive', 't1'))
            self.assertEqual(Server.calls[-1], 'thread/archive')
            Server.path = '/h/.codex/archived_sessions/rollout.jsonl'
            self.assertIn('already archived t1', self.do(CLAUDE, 'codex-archive', 't1'))
        self.assertEqual(announced, [('t1', 'thread-archived', 2)])

    def test_codex_read_turns_commands_and_current_operation(self):
        self.codex.status = 'active'
        self.codex.turns = [{'id': 'new', 'status': 'inProgress', 'startedAt': 20},
                            {'id': 'old', 'status': 'completed', 'completedAt': 10}]
        self.codex.entries = {
            'new': [{'turnId': 'new', 'startedAtMs': 21000,
                     'item': {'type': 'commandExecution', 'id': 'run', 'status': 'inProgress', 'command': 'sleep 30'}},
                    {'turnId': 'new', 'startedAtMs': 20000,
                     'item': {'type': 'userMessage', 'content': [{'type': 'text', 'text': 'continue'}]}}],
            'old': [{'turnId': 'old', 'startedAtMs': 9000, 'completedAtMs': 10000,
                     'item': {'type': 'commandExecution', 'status': 'completed', 'command': 'python3 test.py',
                              'exitCode': 7, 'aggregatedOutput': 'failed\nshort output'}},
                    {'turnId': 'old', 'startedAtMs': 8000,
                     'item': {'type': 'agentMessage', 'text': 'investigating'}}]}
        with patch.object(q.time, 'time', lambda: 31):
            output = self.do(CLAUDE, 'codex-read', 't1')
        for wanted in ('status: active', 'last event: 10s ago', 'user: continue', 'agentMessage: investigating',
                       'exit=7 python3 test.py | failed short output', 'now: command inProgress',
                       '1970-01-01 00:00:21 UTC'):
            self.assertIn(wanted, output)
        self.assertLess(output.index('turn old'), output.index('turn new'))
        self.codex.calls.clear()
        output = self.do(CLAUDE, 'codex-read', 't1', '--limit', 1)
        self.assertNotIn('turn old', output)
        self.assertEqual(len(self.codex.calls), 3)

    def test_codex_read_unknown_timestamp_and_validation(self):
        self.assertIn('last event: unknown', self.do(CLAUDE, 'codex-read', 't1'))
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            self.do(CLAUDE, 'codex-read', 't1', '--limit', 0)
        self.assertEqual(len(self.codex.calls), 2)

    def test_codex_read_running_command_missing_from_paginated_store(self):
        self.codex.status = 'active'
        self.codex.turns = [{'id': 'live', 'status': 'inProgress', 'startedAt': 1}]
        def record(kind, **payload):
            return json.dumps({'type': 'response_item', 'timestamp': '1970-01-01T00:00:02Z',
                               'payload': {'type': kind, 'internal_chat_message_metadata_passthrough': {'turn_id': 'live'},
                                           **payload}}) + '\n'
        import json
        path = self.directory / 'rollout.jsonl'
        self.codex.path = str(path)
        path.write_text(record('custom_tool_call', call_id='start', name='exec',
                               input='text(await tools.exec_command({cmd:"sleep 40"}));') +
                        record('custom_tool_call_output', call_id='start', output=[{'text': '{"session_id": 123}'}]) +
                        record('function_call', call_id='wait', name='wait', arguments='{"cell_id":"2"}') +
                        '{"partial')
        with patch.object(q.time, 'time', lambda: 5):
            output = self.do(CLAUDE, 'codex-read', 't1')
        self.assertIn('now: command inProgress', output)
        self.assertIn('sleep 40', output)
        self.assertIn('live tool wait inProgress', output)
        self.assertIn('last event: 3s ago', output)
        # Once the API records command completion and the tool replies, neither remains running.
        path.write_text(path.read_text().removesuffix('{"partial') +
                        record('function_call_output', call_id='wait', output='done'))
        self.codex.entries['live'] = [{'turnId': 'live', 'startedAtMs': 1000, 'completedAtMs': 3000,
                                      'item': {'type': 'commandExecution', 'processId': '123', 'status': 'completed',
                                               'command': 'sleep 40', 'exitCode': 0}}]
        output = self.do(CLAUDE, 'codex-read', 't1')
        self.assertNotIn('now: command inProgress', output)
        self.assertNotIn('live tool wait', output)

    def test_codex_live_tail_is_bounded_and_does_not_read_other_turns(self):
        import json
        path = self.directory / 'rollout.jsonl'
        path.write_text('x' * q.CODEX_TAIL_BYTES + '\n' + json.dumps({
            'type': 'response_item', 'timestamp': '1970-01-01T00:00:02Z',
            'payload': {'type': 'function_call', 'call_id': 'other', 'name': 'exec', 'arguments': 'other turn',
                        'internal_chat_message_metadata_passthrough': {'turn_id': 'other'}}}) + '\n')
        entries, stamp = q.codex_live_entries(path, {'id': 'live', 'entries': []})
        self.assertEqual((entries, stamp), ([], None))

    def test_codex_send_active_steers_and_new_turns_pin_policy(self):
        self.codex.status = 'active'
        self.codex.turns = [{'id': 'running', 'status': 'inProgress'}]
        with patch.object(q.subprocess, 'run', side_effect=AssertionError('must not invoke CLI codex queue')):
            self.assertIn('delivered to t1 (steered active turn)', self.do(CLAUDE, 'codex-send', 't1', '--text', 'answer'))
        self.assertEqual([method for method, _ in self.codex.calls], ['thread/read', 'thread/turns/list', 'turn/steer'])
        params = self.codex.calls[-1][1]
        self.assertEqual(params['input'], [{'type': 'text', 'text': 'answer'}])
        self.assertEqual(params['expectedTurnId'], 'running')
        self.codex.calls.clear()
        self.codex.status = 'notLoaded'
        self.assertIn('delivered to t1', self.do(CLAUDE, 'codex-send', 't1', '--text', 'next'))
        self.assertEqual([method for method, _ in self.codex.calls],
                         ['thread/read', 'thread/resume', 'turn/start', 'thread/unsubscribe'])
        self.assertEqual(self.codex.calls[-2][1]['approvalPolicy'], 'never')
        self.assertEqual(self.codex.calls[-2][1]['sandboxPolicy'], {'type': 'dangerFullAccess'})
        self.codex.status = 'idle'
        self.codex.calls.clear()
        self.do(CLAUDE, 'codex-send', 't1', '--text', 'after a restricted turn')
        self.assertEqual(self.codex.calls[-2][1]['sandboxPolicy'], {'type': 'dangerFullAccess'})
        self.assertEqual(self.codex.calls[-1][0], 'thread/unsubscribe')
        self.assertEqual([call[0] for call in self.ipc.calls], ['thread-owner-discovery'])
        self.codex.status = 'notLoaded'
        with patch.object(q, 'CodexIpc', side_effect=FileNotFoundError('no app')):
            self.assertIn('delivered to t1', self.do(CLAUDE, 'codex-send', 't1', '--text', 'app closed'))

    def app_rollout(self, *records):
        path = Path(self.enterContext(tempfile.TemporaryDirectory())) / 'rollout.jsonl'
        path.write_text(''.join(json.dumps(record, separators=(',', ':')) + '\n' for record in records))
        self.codex.path = str(path)

    def test_codex_send_to_a_thread_the_app_holds_goes_through_the_app(self):
        self.codex.status, self.ipc.owner = 'notLoaded', 'window'
        self.codex.turns = [{'id': 'done', 'status': 'completed'}]
        self.assertIn('new turn in the Codex app', self.do(CLAUDE, 'codex-send', 't1', '--text', 'next'))
        method, params, version, target = self.ipc.calls[-1]
        self.assertEqual((method, version, target), ('thread-follower-start-turn', 2, 'window'))
        self.assertEqual(params['turnStart']['request']['sandboxPolicy'], {'type': 'dangerFullAccess'})
        self.assertEqual(params['turnStart']['request']['approvalPolicy'], 'never')
        self.assertNotIn('thread/resume', [method for method, _ in self.codex.calls])
        # The shared server reads the app's running turn as interrupted; its rollout has no end for it yet.
        self.codex.turns = [{'id': 'running', 'status': 'interrupted'}]
        self.app_rollout({'type': 'event_msg', 'payload': {'type': 'task_started', 'turn_id': 'running'}})
        self.assertIn('steered the active turn', self.do(CLAUDE, 'codex-send', 't1', '--text', 'more'))
        self.assertEqual(self.ipc.calls[-1][0], 'thread-follower-steer-turn')
        self.assertIn('turn running in the Codex app', self.do(CLAUDE, 'codex-read', 't1'))
        self.app_rollout({'type': 'event_msg', 'payload': {'type': 'turn_aborted', 'turn_id': 'running'}})
        self.assertNotIn('in the Codex app', self.do(CLAUDE, 'codex-read', 't1'))
        self.assertIn('new turn in the Codex app', self.do(CLAUDE, 'codex-send', 't1', '--text', 'again'))

    def test_codex_spawn_pins_first_turn_policy(self):
        with patch.object(q, 'codex_announce'):
            self.assertIn('spawned', self.do(CLAUDE, 'spawn', '--runtime', 'codex', '--name', 'probe'))
        params = next(params for method, params in self.codex.calls if method == 'turn/start')
        self.assertEqual([method for method, _ in self.codex.calls][-2:], ['wait_turn', 'thread/unsubscribe'])
        self.assertEqual(params['approvalPolicy'], 'never')
        self.assertEqual(params['sandboxPolicy'], {'type': 'dangerFullAccess'})
        start = next(params for method, params in self.codex.calls if method == 'thread/start')
        self.assertEqual(start['sandbox'], 'danger-full-access')
        self.assertEqual(start['approvalPolicy'], 'never')

    def run_recorded(self, outputs=None):
        """subprocess.run that records argv and answers from `outputs` by the first two words."""
        runs = []

        def run(argv, **kwargs):
            runs.append(argv)
            return SimpleNamespace(returncode=0, stdout=(outputs or {}).get(' '.join(argv[:2]), ''), stderr='')
        return runs, patch.object(q.subprocess, 'run', run)

    def test_claude_spawn_is_a_background_session_without_the_app(self):
        """#270: `claude --bg` changes no app window; the id comes from `claude agents`."""
        self.agents = {'abcd1234-0000': {'id': 'abcd1234', 'sessionId': 'abcd1234-0000', 'pid': 1}}
        runs, patched = self.run_recorded({'claude --bg': 'backgrounded · abcd1234 · T1 x (idle — send a prompt to start)'})
        with patched:
            printed = self.do(CLAUDE, 'spawn', '--name', 'T1 x')
        self.assertEqual(printed.splitlines()[0], 'abcd1234-0000')
        self.assertIn('claude attach abcd1234', printed)
        self.assertEqual(runs, [['claude', '--bg', '--name', 'T1 x']])
        self.agents = {}
        with self.run_recorded({'claude --bg': 'backgrounded · ffff0000 · T1 x'})[1]:
            self.assertIn('does not list the new session ffff0000', self.refused(CLAUDE, 'spawn'))

    def test_show_stops_the_background_run_then_imports_and_restores_on_the_focus_line(self):
        home = self.directory / 'home'
        log = home / 'Library/Logs/Claude/main.log'
        log.parent.mkdir(parents=True)
        log.write_text('old line\n')
        self.agents = {'s1': {'id': 's1short', 'sessionId': 's1', 'pid': 7}}
        runs = []

        def run(argv, **kwargs):
            runs.append(argv)
            if 'resume?session=s1' in argv[-1]:
                with log.open('a') as out:
                    out.write('[info] [CCD] LocalSessions.setFocusedSession: sessionId=local_s1\n')
            return SimpleNamespace(returncode=0, stdout='', stderr='')
        with patch.object(q.subprocess, 'run', run), patch.object(q.Path, 'home', lambda: home), \
                patch.object(q, 'CLAUDE_APP_SESSIONS', self.directory / 'none'):
            started = time.time()
            printed = self.do(CLAUDE, 'show', 'local_s1', '--restore', 'owner')
        self.assertLess(time.time() - started, 5)  # the log line, not the 20 s timeout
        self.assertEqual(runs, [['claude', 'stop', 's1short'], ['open', '-g', 'claude://resume?session=s1'],
                                ['open', '-g', 'claude://claude.ai/epitaxy/local_owner']])
        self.assertIn('stopped the background run s1short', printed)

    def test_tick_lists_worker_sessions_and_retires_a_reviewed_background_worker(self):
        iid = self.add('--type', 'research', '--runtime', 'claude')
        self.do(CLAUDE, 'take', iid)
        self.agents = {'claude-session': {'id': 'claudese', 'sessionId': 'claude-session', 'pid': 3}}
        listed = self.do(COORDINATOR, 'tick').split('## Claude worker sessions')[1]
        self.assertIn(f'#{iid} t: background, running: `claude attach claudese`', listed)
        self.assertIn('show claude-session', listed)
        self.do(CLAUDE, 'result', iid, '--checks', 'c', '--text', 'done')
        self.assertIn('taskq retire claude-session', self.do(COORDINATOR, 'tick'))
        runs, patched = self.run_recorded()
        with patched:
            self.assertIn('retired claude-session', self.do(COORDINATOR, 'retire', 'claude-session'))
        self.assertEqual(runs, [['claude', 'stop', 'claudese'], ['claude', 'rm', 'claudese']])

    def test_codex_send_does_not_steer_a_completed_or_unknown_turn(self):
        self.codex.status = 'active'
        self.assertIn('active turn id unavailable', self.refused(CLAUDE, 'codex-send', 't1', '--text', 'answer'))
        self.assertNotIn('turn/steer', [method for method, _ in self.codex.calls])

    def test_codex_read_last_turn_actual_sandbox_not_thread_defaults(self):
        import json
        self.codex.turns = [{'id': 'last', 'status': 'completed'}]
        path = self.directory / 'rollout.jsonl'
        self.codex.path = str(path)
        def context(turn, sandbox, approval):
            return json.dumps({'type': 'turn_context', 'payload': {'turn_id': turn,
                               'sandbox_policy': sandbox, 'approval_policy': approval}}) + '\n'
        path.write_text(context('last', {'type': 'danger-full-access'}, 'never') +
                        context('last', {'type': 'workspace-write', 'network_access': False}, 'on-request') +
                        'x' * (q.CODEX_TAIL_BYTES + 1) + '\n' +
                        context('other', {'type': 'danger-full-access'}, 'never'))
        output = self.do(CLAUDE, 'codex-read', 't1')
        self.assertIn('last turn sandbox: {"type": "workspace-write", "network_access": false}; approvalPolicy: on-request', output)
        self.assertNotIn('last turn sandbox: {"type": "danger-full-access"}', output)

    def test_tick_codex_idle_requires_intervention_and_later_archive(self):
        iid = self.add('--type', 'asset')
        self.do(CODEX, 'take', iid)
        output = self.do(CLAUDE, 'tick')
        self.assertIn('## Codex idle', output)
        self.assertIn('codex-session: idle; last event unknown', output)
        self.assertIn('codex-send codex-session', output)
        self.codex.status = 'active'
        self.assertNotIn('## Codex idle', self.do(CLAUDE, 'tick'))
        # The app holds the session: the shared server says notLoaded while the app runs the turn.
        self.codex.status, self.codex.turns = 'notLoaded', [{'id': 'app', 'status': 'interrupted'}]
        self.app_rollout({'type': 'event_msg', 'payload': {'type': 'task_started', 'turn_id': 'app'}})
        output = self.do(CLAUDE, 'tick')
        self.assertIn('notLoaded (turn running in the app)', output)
        self.assertNotIn('## Codex idle', output)
        self.codex.turns, self.codex.path = [], None
        self.do(CODEX, 'ask', iid, '--text', 'owner screen?')
        output = self.do(CLAUDE, 'tick')
        self.assertNotIn('## Codex idle', output)
        self.assertIn('codex-archive codex-session', output)
        # Deferred Codex tasks also get cleanup reminders, without forwarding a question to the owner.
        current = q.parse(self.gitlab.issues[iid])
        q.save(current, 'later', waiting_for='triage')
        output = self.do(CLAUDE, 'tick')
        self.assertIn('codex-archive codex-session', output)
        self.assertNotIn('Waiting for the owner', output)

    def test_tick_codex_unavailable_does_not_stop_other_coordinator_work(self):
        self.do(CODEX, 'take', self.add('--type', 'asset'))
        self.add('--type', 'code')
        with patch.object(q, 'Codex', side_effect=OSError('socket unavailable')):
            output = self.do(CLAUDE, 'tick')
        self.assertIn('status unknown: socket unavailable', output)
        self.assertIn('Start 1 worker', output)
        self.assertNotIn('## Codex idle', output)


class GithubRest:
    """GitHub's REST shapes for what `Github` asks: issues by `number` with label objects, comments, labels,
    milestones, label events, blobs and refs (the lock), the user, and `deleteIssue` over GraphQL."""
    def __init__(self):
        self.issues, self.comments, self.labels, self.refs, self.blobs = {}, {}, {}, {}, {}
        self.calls, self.clock = [], 0

    def now(self):
        self.clock = max(time.time(), self.clock + 0.001)
        return time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(self.clock)) + f'.{int(self.clock * 1000) % 1000:03d}Z'

    def label(self, name):
        return self.labels.get(name) or {'id': 0, 'name': name}

    def page(self, items, query):
        found = {key: value[0] for key, value in q.parse_qs(query).items()}
        size, page = int(found.get('per_page', 30)), int(found.get('page', 1))
        return items[(page - 1) * size:page * size]

    def __call__(self, method, path, body=None):
        self.calls.append((method, path))
        route, _, query = path.partition('?')
        if route == 'user':
            return {'id': 1, 'login': 'alice'}
        if route.startswith('user/'):
            return {'id': int(route[5:]), 'login': {1: 'alice', 2: 'bob'}[int(route[5:])]}
        if route == 'graphql' and 'deleteIssue' in body['query']:
            number = next(number for number, issue in self.issues.items() if issue['node_id'] == body['variables']['id'])
            del self.issues[number]
            return {'data': {}}
        if route == 'graphql':
            found = body['variables']
            items = [issue for issue in self.issues.values() if not found['states'] or issue['state'].upper() in found['states']
                     and all(name in [label['name'] for label in issue['labels']] for name in found['labels'] or [])
                     and (not (found['filter'] or {}).get('since') or issue['updated_at'] >= found['filter']['since'])]
            nodes = [{'number': i['number'], 'id': i['node_id'], 'title': i['title'], 'body': i['body'], 'state': i['state'].upper(),
                      'url': i['html_url'], 'createdAt': i['created_at'], 'updatedAt': i['updated_at'], 'labels': {'nodes': i['labels']},
                      'assignees': {'nodes': [{'databaseId': a['id'], 'login': a['login']} for a in i['assignees']]},
                      'milestone': i['milestone'], 'comments': {'totalCount': sum(c['issue'] == i['number'] for c in self.comments.values())}}
                     for i in items]
            return {'data': {'repository': {'issues': {'pageInfo': {'hasNextPage': False, 'endCursor': None}, 'nodes': nodes}}}}
        if route == 'milestones':
            return [{'number': 5, 'title': 'Maps'}]
        if route == 'labels':
            if method == 'POST':
                assert not body['color'].startswith('#')
                self.labels[body['name']] = {'id': len(self.labels) + 1, **body}
                return self.labels[body['name']]
            return self.page(list(self.labels.values()), query)
        if route.startswith('labels/'):
            del self.labels[q.parse_qs('x=' + route[7:])['x'][0]]
            return None
        if route == 'git/blobs':
            sha = f'blob{len(self.blobs) + 1}'
            self.blobs[sha] = body['content']
            return {'sha': sha}
        if route.startswith('git/blobs/'):
            import base64
            return {'content': base64.b64encode(self.blobs[route[10:]].encode()).decode()}
        if route == 'git/refs':
            if body['ref'] in self.refs:
                q.fail('GitHub POST git/refs failed: {"message":"Reference already exists","status":"422"}')
            self.refs[body['ref']] = body['sha']
            return {'ref': body['ref'], 'object': {'sha': body['sha']}}
        if route.startswith('git/ref/'):
            ref = 'refs/' + route[8:]
            if ref not in self.refs:
                q.fail('GitHub GET failed: {"message":"Not Found","status":"404"}')
            return {'ref': ref, 'object': {'sha': self.refs[ref]}}
        if route.startswith('git/refs/'):
            del self.refs['refs/' + route[9:]]
            return None
        if route.startswith('git/matching-refs/'):
            prefix = 'refs/' + route[18:]
            return [{'ref': ref} for ref in self.refs if ref.startswith(prefix)]
        if route == 'issues' and method == 'POST':
            number = len(self.issues) + 1
            self.issues[number] = {'number': number, 'node_id': f'node{number}', 'state': 'open', 'title': body['title'],
                                   'body': body['body'], 'labels': [self.label(name) for name in body.get('labels', [])],
                                   'assignees': [{'id': {'alice': 1, 'bob': 2}[login], 'login': login} for login in body.get('assignees', [])],
                                   'milestone': {'number': body['milestone']} if body.get('milestone') else None,
                                   'html_url': f'url/{number}', 'comments': 0, 'created_at': self.now(), 'updated_at': self.now(),
                                   'events': []}
            return self.issues[number]
        if route == 'issues':
            found = {key: value[0] for key, value in q.parse_qs(query).items()}
            items = [issue for issue in self.issues.values() if found.get('state', 'open') in ('all', issue['state'])
                     and all(name in [label['name'] for label in issue['labels']] for name in found.get('labels', '').split(',') if name)
                     and (not found.get('since') or issue['updated_at'] >= found['since'])]
            return self.page(items, query)
        if route.startswith('issues/comments/'):
            del self.comments[int(route[16:])]
            return None
        number = int(re.match(r'issues/(\d+)', route)[1])
        rest = route[len(f'issues/{number}'):]
        issue = self.issues[number]
        if rest == '/comments' and method == 'POST':
            cid = max(self.comments, default=0) + 1
            self.comments[cid] = {'id': cid, 'issue': number, 'body': body['body'], 'created_at': self.now(), 'user': {'id': 1, 'login': 'alice'}}
            issue['updated_at'] = self.now()
            return self.comments[cid]
        if rest == '/comments':
            return self.page([item for item in self.comments.values() if item['issue'] == number], query)
        if rest == '/events':
            return self.page(issue['events'], query)
        if method == 'PATCH':
            if 'labels' in body:
                issue['events'] += [{'event': 'labeled', 'label': {'name': name}, 'created_at': self.now()}
                                    for name in body['labels'] if name not in [label['name'] for label in issue['labels']]]
                issue['labels'] = [self.label(name) for name in body['labels']]
            if 'assignees' in body:
                issue['assignees'] = [{'id': {'alice': 1, 'bob': 2}[login], 'login': login} for login in body['assignees']]
            if 'milestone' in body:
                issue['milestone'] = {'number': body['milestone']} if body['milestone'] else None
            if 'body' in body:
                issue['body'] = body['body']
            if body.get('state') == 'closed':
                issue['state'] = 'closed'
            issue['updated_at'] = self.now()
        issue['comments'] = sum(item['issue'] == number for item in self.comments.values())
        return issue


class GithubCycle(unittest.TestCase):
    """The queue on GitHub: the same commands through `Github`, read back in GitHub's own shapes."""
    def setUp(self):
        self.enterContext(patch.object(q, 'AREAS', ('maps', 'engine')))
        self.github = GithubRest()
        store = q.Github('owner/repo')
        store.run = self.github
        self.enterContext(patch.object(q, 'api', store))
        self.enterContext(patch.object(q, 'BOARDS', False))

    def do(self, who, *argv):
        with patch.dict(os.environ, who), contextlib.redirect_stdout(io.StringIO()) as out:
            q.main([str(item) for item in argv])
        return out.getvalue()

    def add(self, *extra):
        self.do(CLAUDE, 'add', '--title', 't', '--goal', 'g', '--acceptance', 'a', *extra)
        return len(self.github.issues)

    def names(self, number):
        return [label['name'] for label in self.github.issues[number]['labels']]

    def state(self, number):
        return q.parse(q.api('GET', f'issues/{number}'))['state']

    def test_full_cycle_on_github(self):
        self.do(CLAUDE, 'init')
        self.assertTrue({'q-ready', 'q-doing', 'run-claude', 'research', 'priority-1', 'area-maps'} <= set(self.github.labels))
        number = self.add('--type', 'research', '--mine', '--milestone', 'Maps', '--area', 'maps')
        issue = self.github.issues[number]
        self.assertEqual(([each['login'] for each in issue['assignees']], issue['milestone']['number']), (['alice'], 5))
        self.assertIn('Start 1 worker', self.do(CLAUDE, 'tick'))
        self.assertIn(f'take {number}', self.do(CLAUDE, 'worker'))
        self.do(CLAUDE, 'take', number)
        self.assertEqual(self.state(number), 'doing')
        self.assertIn('refs/taskq/lock/%d' % number, self.github.refs)
        self.do(CLAUDE, 'beat', number)
        self.do(CLAUDE, 'beat', number)
        self.assertEqual(sum(item['body'].startswith('**beat**') for item in self.github.comments.values()), 1)
        self.do(CLAUDE, 'ask', number, '--text', 'which one?')
        self.assertIn('which one?', self.do(CLAUDE, 'tick'))
        self.do(CLAUDE, 'answer', number, '--text', 'the first')
        self.do(CLAUDE, 'take', number)
        self.do(CLAUDE, 'result', number, '--text', 'done', '--checks', 'none')
        self.assertIn(f'close {number}', self.do(CLAUDE, 'tick'))
        self.do(CLAUDE, 'close', number, '--text', 'ok')
        self.assertEqual(self.github.issues[number]['state'], 'closed')
        self.assertFalse([name for name in self.names(number) if name.startswith('q-')])
        self.assertEqual(self.github.refs, {})
        self.assertIn(f'#{number}: take → ask', self.do(CLAUDE, 'report'))

    def test_lock_is_a_ref_second_taker_loses_and_tick_heals_a_dead_lock(self):
        number = self.add('--type', 'code')
        self.assertTrue(q.lock(number))
        self.assertFalse(q.lock(number))  # 422 Reference already exists
        with patch.object(q, 'LOCK_SECONDS', -1):
            self.assertIn(f'Unlocked #{number}', self.do(CLAUDE, 'tick'))
        self.assertEqual(self.github.refs, {})
        other = {'CLAUDE_CODE_SESSION_ID': 'other-machine', 'CODEX_THREAD_ID': ''}
        self.do(other, 'take', number)
        with self.assertRaises(SystemExit) as refused, contextlib.redirect_stdout(io.StringIO()):
            with patch.dict(os.environ, CLAUDE):
                q.main(['take', str(number)])
        self.assertIn('cannot start', str(refused.exception))

    def test_newest_comment_is_read_from_the_last_page(self):
        number = self.add('--type', 'code')
        self.do(CLAUDE, 'take', number)
        for _ in range(105):
            q.note(number, 'beat')
        before = len(self.github.calls)
        self.do(CLAUDE, 'beat', number)
        paged = [path for method, path in self.github.calls[before:] if '/comments?' in path]
        self.assertEqual(paged, [f'issues/{number}/comments?per_page=100&page=2'])  # the newest: one page, not 106 comments
        self.assertEqual(sum(item['body'].startswith('**beat**') for item in self.github.comments.values()), 105)
        self.assertEqual([item['body'] for item in q.api('GET', f'issues/{number}/notes?sort=desc&per_page=1')], ['**beat** · claude:claude-s'])

    def test_init_writes_a_github_config(self):
        with tempfile.TemporaryDirectory() as tmp, contextlib.chdir(tmp), contextlib.redirect_stdout(io.StringIO()) as out:
            q.main(['init', '--github', 'owner/repo'])
            self.assertEqual(Path('taskq.toml').read_text().splitlines()[1:], ['[github]', 'repo = "owner/repo"'])
        self.assertIn('no board on GitHub', out.getvalue())


class Selftest(unittest.TestCase):
    """`selftest --scope quick` against the fake GitLab; its worker processes run in this process."""
    do, add = Cycle.do, Cycle.add

    def setUp(self):
        Cycle.setUp(self)
        for target, value in (('ROOT', self.directory), ('TICK_BEAT', self.directory / 'beat'), ('selftest_run', self.run_calls)):
            patcher = patch.object(q, target, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def run_calls(self, calls, timeout=None):
        found = []
        for env, argv in calls:
            with patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(io.StringIO()) as out:
                try:
                    if env.get('GITLAB_TOKEN') == 'broken':
                        q.fail('GitLab GET issues failed: glab: 401 Unauthorized (HTTP 401)')
                    q.main([str(item) for item in argv])
                    code = 0
                except SystemExit as error:
                    code, out = (1 if error.code else 0), io.StringIO(out.getvalue() + str(error.code))
            found.append((code, out.getvalue()))
        return found

    def test_quick_reads_every_step_back_and_leaves_no_task(self):
        self.gitlab.issues[99] = {'iid': 99, 'state': 'opened', 'labels': [], 'description': '', 'title': 'owner',
                                  'assignees': [], 'updated_at': self.gitlab.now()}  # the issue the owner names for the report
        report = self.do(COORDINATOR, 'selftest', '--scope', 'quick', '--note', 99)
        self.assertIn('13 of 13 ok', report)
        for mechanism in ('| add |', '| take, claim, assignee |', '| ask |', '| tick: question |', '| answer |',
                          '| result |', '| tick: review |', '| close |', '| remove the selftest tasks |'):
            self.assertIn(mechanism, report)
        self.assertEqual(sorted(self.gitlab.issues), [99])  # the selftest tasks are deleted
        self.assertTrue(self.gitlab.said(99)[-1].startswith('**selftest** · claude:coordina'))
        self.assertFalse(self.gitlab.locked())
        self.assertFalse((self.directory / 'beat').exists())  # the real tick's last-run time is untouched

    def test_broken_worker_token_is_named_not_ok(self):
        with self.assertRaises(SystemExit), contextlib.redirect_stdout(io.StringIO()) as out, patch.dict(os.environ, COORDINATOR):
            q.main(['selftest', '--worker-env', 'GITLAB_TOKEN=broken'])
        self.assertIn('| take, claim, assignee | claude | FAIL |', out.getvalue())
        self.assertIn('401 Unauthorized', out.getvalue())
        self.assertIn('| beat | claude | skipped |', out.getvalue())
        self.assertIn('| remove the selftest tasks | - | ok |', out.getvalue())

    def test_a_selftest_task_is_only_for_a_profile_naming_it(self):
        iid = self.add('--type', 'research', '--label', 'selftest')
        self.assertIn('No task can start now', self.do(CLAUDE, 'worker'))
        self.assertIn('Nothing to do', self.do(COORDINATOR, 'tick'))
        self.assertIn(f'take {iid}', self.do(CLAUDE, 'worker', '--filter', 'labels=selftest'))

    def test_a_configured_runtime_is_one_table(self):
        with tempfile.TemporaryDirectory() as tmp, patch.dict(q.RUNTIMES), patch.dict(q.EXECUTORS):
            config = Path(tmp) / 'taskq.toml'
            config.write_text(Path(q.__file__).resolve().parents[1].joinpath('tests/taskq.toml').read_text() +
                              '\n[runtimes.grok]\nenv = "GROK_SESSION_ID"\nspawn = "run-grok spawn --name {name}"\n'
                              'send = "run-grok send {session} {text}"\n')
            q.configure(config)
            self.addCleanup(q.configure, Path(__file__).resolve().parent / 'taskq.toml')
            self.assertEqual(q.RUNTIMES['grok'], 'GROK_SESSION_ID')
            self.assertEqual(q.limits('grok=4')['grok'], 4)
            self.assertEqual(q.selftest_command(q.EXECUTORS['grok']['send'], session='s 1', text='a; rm -rf /'),
                             ['run-grok', 'send', 's 1', 'a; rm -rf /'])
            iid = self.add('--type', 'research', '--runtime', 'grok')
            self.assertIn(f'take {iid}', self.do({**CLAUDE, 'CLAUDE_CODE_SESSION_ID': '', 'GROK_SESSION_ID': 'g1'}, 'worker'))


class Host(unittest.TestCase):
    def test_glab_gets_the_configured_host_or_chooses_itself(self):
        calls = []
        done = SimpleNamespace(returncode=0, stdout='[]', stderr='')
        with patch.object(q.subprocess, 'run', lambda command, **kwargs: calls.append(command) or done):
            q.api('GET', 'issues')
            with patch.object(q, 'HOST', None):
                q.api('GET', 'issues')
        self.assertEqual(calls[0][-2:], ['--hostname', 'gitlab.example.com'])
        self.assertNotIn('--hostname', calls[1])
        self.assertEqual(calls[1][4], 'projects/group%2Fproject/issues')


class TickBeat(unittest.TestCase):
    def test_second_tick_within_live_window_is_told_not_to_arm(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(q, 'TICK_BEAT', Path(tmp) / 'beat'):
            with contextlib.redirect_stdout(io.StringIO()) as first:
                q.tick_beat()
            with contextlib.redirect_stdout(io.StringIO()) as second:
                q.tick_beat()
            os.utime(q.TICK_BEAT, (time.time() - 20 * 60,) * 2)
            with contextlib.redirect_stdout(io.StringIO()) as third:
                q.tick_beat()
        self.assertEqual(first.getvalue(), 'Last tick: none.\n')
        self.assertIn('another coordinator is armed', second.getvalue())
        self.assertEqual(third.getvalue(), 'Last tick: 20 min ago.\n')



class Update(unittest.TestCase):
    """A real `main` in a bare repository stands in for GitHub; the install is a clone of it."""
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.origin, self.work, self.clone = self.root / 'origin.git', self.root / 'work', self.root / 'clone'
        self.git('init', '-q', '--bare', '-b', 'main', str(self.origin))
        self.git('clone', '-q', str(self.origin), str(self.work))
        self.commit(self.work, 'one')
        self.git('clone', '-q', str(self.origin), str(self.clone))
        self.enterContext(patch.object(q, 'REPO', str(self.origin)))
        self.enterContext(patch.object(q, 'install', lambda: ('clone', self.clone)))

    def git(self, *args, cwd=None):
        return subprocess.run(['git', *(['-C', str(cwd)] if cwd else []), '-c', 'user.name=t', '-c', 'user.email=t@t', *args],
                              check=True, capture_output=True, text=True).stdout.strip()

    def commit(self, where, name):
        (where / name).write_text(name)
        self.git('add', name, cwd=where)
        self.git('commit', '-q', '-m', name, cwd=where)
        self.git('push', '-q', 'origin', 'HEAD:main', cwd=where)
        return self.git('rev-parse', '--short=7', 'HEAD', cwd=where)

    def update(self):
        with contextlib.redirect_stdout(io.StringIO()) as out:
            done = q.update(SimpleNamespace(verbose=True))
        return done, out.getvalue()

    def test_clone_fast_forwards_then_is_up_to_date(self):
        old = q.version()
        new = self.commit(self.work, 'two')
        self.assertEqual(self.update(), (True, f'updated {old} → {new}\n'))
        self.assertEqual(q.version(), new)
        self.assertEqual(self.update(), (None, f'up to date {new}\n'))

    def test_dirty_or_ahead_clone_is_left_alone(self):
        self.commit(self.work, 'two')
        (self.clone / 'one').write_text('edited')
        before = q.version()
        self.assertIn('uncommitted changes', self.update()[1])
        self.git('checkout', '-q', 'one', cwd=self.clone)
        (self.clone / 'mine').write_text('mine')
        self.git('add', 'mine', cwd=self.clone)
        self.git('commit', '-q', '-m', 'mine', cwd=self.clone)
        ahead = q.version()
        self.assertNotEqual(ahead, before)
        self.assertIn('commits main of', self.update()[1])
        self.assertEqual(q.version(), ahead)

    def test_no_network_is_silent_without_verbose(self):
        with patch.object(q, 'REPO', str(self.root / 'missing.git')), contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertIsNone(q.update(SimpleNamespace(verbose=False)))
            self.assertEqual(out.getvalue(), '')
            self.assertIn('did not answer', self.update()[1])

    def test_install_from_git_reinstalls_with_its_installer(self):
        old = self.git('rev-parse', 'HEAD', cwd=self.clone)
        self.commit(self.work, 'two')
        calls, real = [], subprocess.run
        def run(command, **options):
            calls.append(command)
            return real(command, **options) if command[0] == 'git' else None
        (self.root / 'pipx_metadata.json').write_text('{}')
        with patch.object(q, 'install', lambda: ('git', old)), patch.object(q.sys, 'prefix', str(self.root)), \
                patch.object(q.subprocess, 'run', run), contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertTrue(q.update(SimpleNamespace(verbose=False)))
        self.assertEqual(calls[-1], ['pipx', 'install', '--force', f'git+{self.origin}'])
        self.assertIn(f'updated {old[:7]} → ', out.getvalue())
        (self.root / 'pipx_metadata.json').unlink()
        def broken(command, **options):
            if command[0] == 'git':
                return real(command, **options)
            raise subprocess.CalledProcessError(1, command, stderr='ERROR: no network\n')
        with patch.object(q, 'install', lambda: ('git', old)), patch.object(q.sys, 'prefix', str(self.root)), \
                patch.object(q.subprocess, 'run', broken), contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertIsNone(q.update(SimpleNamespace(verbose=False)))
        self.assertIn('-m pip install -q --force-reinstall git+', out.getvalue())
        self.assertIn('failed: ERROR: no network', out.getvalue())

    def test_tick_checks_at_most_every_and_never_when_off(self):
        stamp, done = self.root / 'state' / 'update-last', []
        def update(args):
            done.append(1)
            return len(done) == 3
        with patch.object(q, 'UPDATE_STAMP', stamp), patch.object(q, 'update', update), \
                patch.object(q.os, 'execv', lambda *command: done.append(command)), patch.dict(q.UPDATE, auto=True, every='24h'):
            q.auto_update()
            q.auto_update()
            self.assertEqual(done, [1])
            os.utime(stamp, (time.time() - 25 * 3600,) * 2)
            q.auto_update()
            self.assertEqual(done, [1, 1])
            os.utime(stamp, (time.time() - 25 * 3600,) * 2)
            q.auto_update()
            self.assertEqual(done[3][1][1:3], ['-m', 'taskq'])
            os.utime(stamp, (time.time() - 25 * 3600,) * 2)
            q.UPDATE['auto'] = False
            q.auto_update()
            self.assertEqual(len(done), 4)
        self.assertEqual([q.seconds(every) for every in ('30m', '24h', '7d')], [1800, 86400, 604800])
        self.assertRaises(SystemExit, q.seconds, '1 day')

    def test_missing_config_fields_are_written_and_named(self):
        config = self.root / 'taskq.toml'
        config.write_text('[gitlab]\nproject = "g/p"\n')
        with contextlib.redirect_stdout(io.StringIO()) as out:
            q.complete(config)
        self.assertIn('added to [update]: auto = true; every = "24h"', out.getvalue())
        config.write_text('[gitlab]\nproject = "g/p"\n\n[update]\nauto = false\n')
        with contextlib.redirect_stdout(io.StringIO()) as out:
            q.complete(config)
            q.complete(config)
        self.assertEqual(out.getvalue().count('added'), 1)
        self.assertEqual(config.read_text(), '[gitlab]\nproject = "g/p"\n\n[update]\nevery = "24h"\nauto = false\n')


@unittest.skipUnless(HELPERS, 'TASKQ_CLEANUP_HELPERS names no folder with workspace_gc.py, host_tools.py, host_gentle.py')
class Cleanup(unittest.TestCase):
    """Real Git refs, patches and retire in disposable repositories; no live app mutations."""
    def setUp(self):
        import host_tools
        self.host = host_tools
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = (Path(directory.name) / 'repo').resolve()
        self.root.mkdir()
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'user.email', 'test@example.invalid')
        self.git('config', 'user.name', 'Test')
        self.git('config', 'commit.gpgsign', 'false')
        (self.root / '.gitignore').write_text('.local/\nscripts/\n')
        (self.root / 'base').write_text('base')
        self.git('add', '.')
        self.git('commit', '-qm', 'base')
        self.remote = Path(directory.name) / 'remote.git'
        self.git('init', '-q', '--bare', str(self.remote))
        self.git('remote', 'add', 'origin', str(self.remote))
        self.git('push', '-q', '-u', 'origin', 'main')
        self.issues, self.app, self.threads, self.agents = {}, {}, {}, {}
        # Retire runs the existing script, not a replacement that merely deletes a directory.
        scripts = self.root / 'scripts'
        scripts.mkdir()
        source = Path(HELPERS)
        for name in ('workspace_gc.py', 'host_tools.py', 'host_gentle.py'):
            shutil.copy2(source / name, scripts / name)
        self.before_cwd = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, self.before_cwd)
        # Explicit patches keep app and issue reads outside these disposable Git fixtures.
        for target, name, value in ((q, 'cleanup_issues', lambda: self.issues), (q, 'claude_sessions', lambda: self.app),
                                    (q, 'claude_agents', lambda: self.agents),
                                    (q, 'cleanup_codex', lambda roots: self.threads),
                                    (host_tools, 'live_paths', lambda: [])):
            patcher = patch.object(target, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)
        self.env = patch.dict(os.environ, CODEX)
        self.env.start()
        self.addCleanup(self.env.stop)

    def git(self, *args, cwd=None):
        return subprocess.run(['git', '-C', str(cwd or self.root), *args], capture_output=True,
                              text=True, check=True).stdout.strip()

    def tree(self, branch, merged=True):
        tree = self.root / '.claude/worktrees' / branch.removeprefix('worktree-')
        tree.parent.mkdir(parents=True, exist_ok=True)
        self.git('worktree', 'add', '-q', '-b', branch, str(tree))
        (tree / branch).write_text(branch)
        self.git('add', branch, cwd=tree)
        self.git('commit', '-qm', branch, cwd=tree)
        if merged:
            self.git('merge', '--ff-only', branch)
            self.git('push', '-q', 'origin', 'main')
        return tree

    def run_cleanup(self, apply=False):
        with contextlib.redirect_stdout(io.StringIO()) as out:
            q.main(['cleanup'] + (['--apply'] if apply else []))
        return out.getvalue()

    def test_merged_clean_tree_is_reported_then_retired_and_branch_deleted(self):
        tree = self.tree('worktree-done')
        # origin/main is ahead of local main: -d must use the verified upstream without saving config.
        self.git('reset', '--hard', 'HEAD~1')
        before = (self.git('worktree', 'list', '--porcelain'), self.git('branch'),
                  (self.root / '.git/config').read_bytes())
        report = self.run_cleanup()
        self.assertIn('# Remove', report)
        self.assertIn('# Ask the owner', report)
        self.assertIn('# Kept', report)
        self.assertIn(str(tree), report.split('# Ask the owner')[0])
        self.assertEqual(before, (self.git('worktree', 'list', '--porcelain'), self.git('branch'),
                                  (self.root / '.git/config').read_bytes()))
        self.assertIn('freed', self.run_cleanup(True))
        self.assertFalse(tree.exists())
        self.assertNotIn('worktree-done', self.git('branch'))
        self.assertTrue((self.root / 'base').exists())
        self.assertEqual(before[-1], (self.root / '.git/config').read_bytes())

    def test_unmerged_dirty_open_task_and_process_are_not_touched(self):
        unmerged = self.tree('worktree-unmerged', False)
        dirty = self.tree('worktree-dirty')
        (dirty / 'unknown').write_text('do not delete')
        doing = self.tree('worktree-taskq-7')
        held = self.tree('worktree-held')
        self.issues[7] = {'closed': False, 'state': 'doing', 'claim': {'runtime': 'claude', 'session': 'live'}}
        with patch.object(self.host, 'live_paths', lambda: [(9876, 'worker', 'cwd', str(held))]):
            report = self.run_cleanup(True)
        ask = report.split('# Ask the owner')[1].split('# Kept')[0]
        self.assertIn(str(unmerged), ask)
        self.assertIn(str(dirty), ask)
        self.assertIn(str(held), ask)
        self.assertIn('show diff: git diff', ask)
        self.assertIn('show changes: git -C', ask)
        self.assertIn('show process: ps', ask)
        self.assertIn(str(doing), report.split('# Kept')[1])
        for tree in (unmerged, dirty, doing, held):
            self.assertTrue(tree.exists())
        self.assertEqual((dirty / 'unknown').read_text(), 'do not delete')

    def test_rebased_patch_is_finished_but_d_never_becomes_force(self):
        tree = self.tree('worktree-rebased', False)
        old = self.git('rev-parse', 'worktree-rebased')
        (self.root / 'other').write_text('other')
        self.git('add', 'other')
        self.git('commit', '-qm', 'other')
        self.git('cherry-pick', old)
        self.git('push', '-q', 'origin', 'main')
        self.assertNotEqual(old, self.git('rev-parse', 'main'))
        remove, _, _ = q.cleanup_plan(self.root)
        self.assertTrue(any(row.get('path') == str(tree) for row in remove))
        report = self.run_cleanup(True)
        self.assertFalse(tree.exists())
        self.assertIn('Kept branch worktree-rebased', report)
        self.assertIn('worktree-rebased', self.git('branch'))

    def test_remote_merged_branch_only_asks_and_open_codex_keeps_its_tree(self):
        tree = self.tree('worktree-active')
        self.git('push', '-q', 'origin', 'worktree-active')
        self.threads['live'] = {'id': 'live', 'cwd': str(tree), 'status': {'type': 'active'}}
        report = self.run_cleanup(True)
        self.assertIn('delete on the server: git push origin --delete worktree-active', report)
        self.assertIn('Codex session live', report.split('# Kept')[1])
        self.assertTrue(tree.exists())
        self.assertIn('refs/heads/worktree-active', self.git('ls-remote', '--heads', 'origin'))

    def test_sessions_use_live_closed_issues_and_current_is_kept(self):
        self.issues[1] = {'closed': True, 'state': 'unknown', 'type': 'code',
                          'result': {'sha': self.git('rev-parse', 'main')},
                          'claim': {'runtime': 'codex', 'session': 'done'}}
        self.issues[2] = {'closed': True, 'state': 'unknown', 'type': 'research',
                          'claim': {'runtime': 'claude', 'session': 'claude-done'}}
        self.issues[3] = {'closed': True, 'state': 'unknown', 'type': 'code', 'result': {'sha': 'bad-sha'},
                          'claim': {'runtime': 'codex', 'session': 'unverified'}}
        for sid, status in (('done', 'idle'), ('unknown', 'idle'), ('unverified', 'idle'), ('codex-session', 'idle'), ('error', 'systemError')):
            self.threads[sid] = {'id': sid, 'cwd': str(self.root), 'status': {'type': status}}
        old = (time.time() - q.STALE_MINUTES * 60 - 60) * 1000
        spawned = {'adoptedFromOtherSurface': True, 'cwd': str(self.root), 'lastActivityAt': old}
        self.app = {'claude-done': {'sessionId': 'local_claude-done', **spawned},
                    'unknown-child': {'sessionId': 'local_unknown-child', **spawned},
                    'fresh-child': {'sessionId': 'local_fresh-child', **spawned, 'lastActivityAt': time.time() * 1000},
                    'archived-child': {'sessionId': 'local_archived-child', **spawned, 'isArchived': True},
                    'owner-chat': {'sessionId': 'local_other-id', 'cwd': str(self.root), 'lastActivityAt': old}}
        archived = []
        with patch.object(q, 'codex_archive', lambda args: archived.append(args.thread)):
            report = self.run_cleanup(True)
        self.assertEqual(archived, ['done'])
        self.assertIn('coordinator: archive_session local_claude-done', report)
        self.assertIn('Claude session local_unknown-child: worker without a proven', report)
        for sid in ('fresh-child', 'archived-child', 'owner-chat', 'other-id'):
            self.assertNotIn(sid, report)
        self.assertIn('Codex session unknown', report.split('# Ask the owner')[1])
        self.assertIn('Codex session unverified', report.split('# Ask the owner')[1])
        self.assertIn('codex-archive unknown', report)
        self.assertIn('Codex session codex-session', report.split('# Kept')[1])

    def test_background_workers_of_closed_tasks_are_retired_others_asked_or_kept(self):
        self.issues[1] = {'closed': True, 'state': 'unknown', 'type': 'research', 'claim': {'runtime': 'claude', 'session': 'bg-done'}}
        self.issues[2] = {'closed': False, 'state': 'doing', 'type': 'research', 'claim': {'runtime': 'claude', 'session': 'bg-open'}}
        old = (time.time() - q.STALE_MINUTES * 60 - 60) * 1000
        agent = lambda sid, **more: {'id': sid[:5], 'sessionId': sid, 'cwd': str(self.root), 'startedAt': old, 'status': 'idle', **more}
        self.agents = {sid: agent(sid) for sid in ('bg-done', 'bg-open', 'bg-orphan')}
        self.agents['bg-fresh'] = agent('bg-fresh', startedAt=time.time() * 1000)
        self.agents['bg-elsewhere'] = agent('bg-elsewhere', cwd='/elsewhere')
        retired = []
        with patch.object(q, 'claude_stop', lambda sid, remove=False: retired.append((sid, remove))):
            report = self.run_cleanup(True)
        self.assertEqual(retired, [('bg-done', True)])
        self.assertIn('Claude background session bg-op', report.split('# Kept')[1])
        self.assertIn('taskq retire bg-orphan', report.split('# Ask the owner')[1])
        for sid in ('bg-fresh', 'bg-elsewhere'):
            self.assertNotIn(sid, report)

    def test_new_activity_between_plan_and_apply_prevents_deletion(self):
        tree = self.tree('worktree-race')
        original, calls = q.cleanup_plan, []
        def plan(root):
            calls.append(1)
            if len(calls) == 2:
                (tree / 'changed').write_text('new work')
            return original(root)
        with patch.object(q, 'cleanup_plan', plan):
            self.assertIn('Kept after the recheck', self.run_cleanup(True))
        self.assertTrue(tree.exists())
        self.assertIn('worktree-race', self.git('branch'))

    def test_detached_merged_tree_and_idle_session_by_cwd_are_removed(self):
        tree = self.tree('worktree-detached')
        self.git('checkout', '--detach', cwd=tree)
        self.git('branch', '-d', 'worktree-detached')
        self.threads['by-cwd'] = {'id': 'by-cwd', 'cwd': str(tree), 'status': {'type': 'idle'}}
        archived = []
        def archive(args):
            archived.append(args.thread)
            del self.threads[args.thread]
        with patch.object(q, 'codex_archive', archive):
            self.run_cleanup(True)
        self.assertEqual(archived, ['by-cwd'])
        self.assertFalse(tree.exists())

    def test_archive_refusal_keeps_the_sessions_tree_and_branch(self):
        tree = self.tree('worktree-writer')
        self.threads['writer'] = {'id': 'writer', 'cwd': str(tree), 'status': {'type': 'notLoaded'}}
        with patch.object(q, 'codex_archive', side_effect=SystemExit('active writer')):
            report = self.run_cleanup(True)
        self.assertIn('active writer', report)
        self.assertIn('the session of this tree is not archived', report)
        self.assertTrue(tree.exists())
        self.assertIn('worktree-writer', self.git('branch'))

    def test_unavailable_inventory_keeps_trees_and_unknown_status_keeps_tree(self):
        tree = self.tree('worktree-unknown')
        with patch.object(q, 'cleanup_codex', side_effect=OSError('not connected')):
            report = self.run_cleanup(True)
        self.assertTrue(tree.exists())
        self.assertIn('Codex session state not checked', report)
        self.threads['unknown'] = {'id': 'unknown', 'cwd': str(tree), 'status': {'type': 'systemError'}}
        self.run_cleanup(True)
        self.assertTrue(tree.exists())

    def test_cleanup_refuses_linked_checkout(self):
        tree = self.tree('worktree-linked')
        os.chdir(tree)
        with self.assertRaisesRegex(SystemExit, 'only from the main checkout'):
            self.run_cleanup(True)


if __name__ == '__main__':
    unittest.main()
