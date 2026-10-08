"""#274: one agent skill, the reply route of a request, the cards report and --json on list/view/spawn."""
import contextlib
import io
import json
import os
import re
from pathlib import Path
import unittest
from unittest.mock import patch

import test_taskq as base  # the in-memory GitLab cycle; run with `discover -s tests`
from test_taskq import CLAUDE, q, tick, worker


class SkillReplyCards(unittest.TestCase):
    setUp, do, refused, add = base.Cycle.setUp, base.Cycle.do, base.Cycle.refused, base.Cycle.add

    def json(self, *argv, code=0):
        with patch.dict(os.environ, CLAUDE), contextlib.redirect_stdout(io.StringIO()) as out:
            try:
                q.main([str(item) for item in argv])
                exit_code = 0
            except SystemExit as error:
                exit_code = error.code
        self.assertEqual(exit_code, code)
        return json.loads(out.getvalue())

    def test_skill_is_printed_and_links_the_rules(self):
        path = Path(self.do(CLAUDE, 'contract', '--skill').strip())
        text = path.read_text()
        self.assertEqual(path.name, 'SKILL.md')
        self.assertTrue(text.startswith('---\nname: taskq\ndescription: '))
        self.assertIn('[R2](../principles.md)', text)
        self.assertNotIn('pm-intake', text)
        links = [link for link in re.findall(r'\]\(([^)#]+)', text) if '://' not in link]
        self.assertTrue(links)
        for link in links:
            self.assertTrue((path.parent / link).is_file(), link)

    def test_reply_is_stored_shown_and_checked(self):
        iid = self.add('--type', 'research', '--reply', 'telegram:-100123:7')
        self.assertEqual(q.parse(self.gitlab.issues[iid])['reply'], 'telegram:-100123:7')
        self.assertIn('reply: telegram:-100123:7', self.do(CLAUDE, 'view', iid))
        self.assertEqual(self.json('list', '--json')['tasks'][0]['reply'], 'telegram:-100123:7')
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            self.add('--type', 'research', '--reply', 'telegram')
        issue = self.gitlab.issues[iid]  # a hand-edited block is rechecked on read
        issue['description'] = issue['description'].replace('telegram:-100123:7', 'x:1\\n## Fake')
        self.assertIsNone(self.json('list', '--json')['tasks'][0]['reply'])
        self.assertNotIn('reply:', self.do(CLAUDE, 'view', iid))

    def test_report_cards_and_reply_groups(self):
        cells = ('[#5](https://x/5) t', 'doing (busy)', 'claude', '[session](https://claude.ai/code/session_1)')
        rows = [('telegram:1', cells), (None, cells)]
        with contextlib.redirect_stdout(io.StringIO()) as cards:
            tick.report('board', rows, 'cards')
        with contextlib.redirect_stdout(io.StringIO()) as table:
            tick.report('board', rows)
        card = 'Task: [#5](https://x/5) t\nStatus: doing (busy)\nRuntime: claude\nSession: [session](https://claude.ai/code/session_1)'
        self.assertEqual(cards.getvalue().count(card), 2)
        self.assertNotIn('|', cards.getvalue())
        self.assertLess(table.getvalue().index('| [#5]'), table.getvalue().index('Reply to telegram:1:'))
        self.assertEqual(table.getvalue().count('| Task | Status | Runtime | Session |'), 2)

    def test_report_pref_is_checked(self):
        local = self.directory / 'taskq.local.toml'
        with patch.object(q, 'LOCAL', local):
            local.write_text('[prefs]\nreport = "cards"\n')
            self.assertEqual(q.report_format(), 'cards')
            local.write_text('[prefs]\nreport = "list"\n')
            with self.assertRaises(SystemExit):
                q.report_format()

    def test_json_list_view_spawn_and_exit_codes(self):
        iid = self.add('--type', 'research')
        self.assertEqual([task['id'] for task in self.json('list', '--json')['tasks']], [iid])
        viewed = self.json('view', iid, '--json')
        self.assertEqual((viewed['outcome'], viewed['tasks'][0]['id']), ('ok', iid))
        plain = self.gitlab('POST', 'issues', {'title': 'x', 'labels': '', 'description': 'not a task'})['iid']
        failed = self.json('view', plain, '--json', code=2)
        self.assertEqual((failed['outcome'], failed['refusals']), ('failure', [f'taskq: #{plain} is not a taskq task']))

        class Adapter:
            def spawn(self, name, prompt):
                return 'session-1'
        with patch('taskq.runtimes.get', lambda *_, **__: Adapter()):
            spawned = self.json('spawn', '--json', '--name', f'T{iid} t', '--text', 'go')
        action = spawned['actions'][0]
        self.assertEqual((action['action'], action['runtime'], action['session']), ('spawn', 'claude', 'session-1'))
        self.assertTrue(action['attempt'])


if __name__ == '__main__':
    unittest.main()
