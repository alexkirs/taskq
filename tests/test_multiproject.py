"""Multi-project PM by explicit list (R10): the owner's list in taskq.local.toml, one ordinary tick per listed
checkout (a fake tick here: no tracker, no runtime), one R6 table per project, a failing or slow one named."""
import contextlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import taskq as core  # noqa: E402
import taskq.multiproject as multiproject  # noqa: E402

# The fake `taskq tick`: its behavior is named by the checkout's folder.
FAKE = """import json, os, sys, time
name = os.path.basename(os.getcwd())
open('limit.txt', 'w').write(sys.argv[sys.argv.index('--limit') + 1])
if name == 'slow':
    time.sleep(30)
if name == 'broken':
    sys.exit('taskq.toml: not valid TOML')
row = {'task': f'[#1](https://github.com/acme/{name}/issues/1)', 'title': f'{name} task', 'state': 'doing',
       'runtime': 'claude', 'machine': 'mac', 'session': '[s](https://claude.ai/code/s)'}
print(json.dumps({'outcome': 'ok', 'refusals': [], 'act': '--act' in sys.argv,
                  'report': {'board': f'https://github.com/users/acme/projects/{name}', 'workers': [row]}}))
"""


class MultiprojectTest(unittest.TestCase):
    def setUp(self):
        saved = dict(vars(core))  # configure() sets module globals the other test modules rely on
        self.addCleanup(vars(core).update, saved)
        self.dir = Path(self.enterContext(tempfile.TemporaryDirectory())).resolve()
        for name in ('pm', 'alpha', 'beta', 'broken', 'slow'):
            (self.dir / name).mkdir()
            (self.dir / name / 'taskq.toml').write_text(f'[github]\nrepo = "acme/{name}"\n')
        (self.dir / 'fake.py').write_text(FAKE)
        self.enterContext(patch.object(multiproject, 'TICK', [sys.executable, str(self.dir / 'fake.py')]))
        self.enterContext(patch.object(core, 'main_checkout', lambda path: path))
        self.agents = {}  # this machine's `claude agents`; no Codex app server
        self.enterContext(patch.object(core, 'claude_agents', lambda strict=False: self.agents))
        self.enterContext(patch.object(core, 'CODEX_SOCKET', self.dir / 'no-socket'))
        cwd = os.getcwd()
        os.chdir(self.dir / 'pm')
        self.addCleanup(os.chdir, cwd)

    def main(self, *argv):
        with contextlib.redirect_stdout(io.StringIO()) as said:
            code = multiproject.main(list(argv))
        return code, said.getvalue()

    def test_set_writes_the_list_and_keeps_the_rest(self):
        (self.dir / 'pm' / 'taskq.local.toml').write_text('[projects]\nold = "/x"\n\n[profile]\nmine = false\n')
        self.main('--set', f'alpha={self.dir / "alpha"}', f'beta={self.dir / "beta"}')
        core.configure()
        self.assertEqual(multiproject.projects(), {'alpha': str(self.dir / 'alpha'), 'beta': str(self.dir / 'beta')})
        self.assertFalse(core.personal()['profile']['mine'])  # the checked loader accepts [projects]
        with self.assertRaises(SystemExit):
            self.main('--set', f'gamma={self.dir}')  # no taskq.toml there

    def test_two_projects_two_tables(self):
        self.main('--set', f'alpha={self.dir / "alpha"}', f'beta={self.dir / "beta"}')
        code, said = self.main()
        self.assertEqual(code, 0)
        for name in ('alpha', 'beta'):
            self.assertIn(f'## {name}\nBoard: https://github.com/users/acme/projects/{name}\n\n'
                          '| Task | Status | Runtime | Session |', said)
            self.assertIn(f'| [#1](https://github.com/acme/{name}/issues/1) {name} task | doing | claude @mac |', said)
        self.assertTrue(all(found['report'] for found in json.loads(self.main('--json', '--act')[1])))
        with contextlib.redirect_stdout(io.StringIO()) as said, self.assertRaises(SystemExit) as done:
            core.main(['projects', '--json'])  # the documented command
        self.assertEqual((done.exception.code, len(json.loads(said.getvalue()))), (0, 2))

    def test_the_machine_cap_splits_by_the_other_projects_live_workers(self):
        (self.dir / 'beta' / 'taskq.toml').write_text('[github]\nrepo = "acme/beta"\n[profile.limits]\ncodex = 0\n')
        self.main('--set', f'alpha={self.dir / "alpha"}', f'beta={self.dir / "beta"}')
        live = lambda iid, state='working': {'sessionId': f's{iid}', 'name': f'T{iid} x', 'cwd': str(self.dir / 'alpha'), 'state': state}
        # alpha: one task's worker and supervisor (one slot), a stopped one, a session of another kind
        self.agents = {'w': live(7), 's': {**live(7), 'name': 'S7 x'}, 'd': live(8, 'stopped'), 'o': {**live(9), 'name': 'chat'}}
        self.main()
        limit = lambda name: (self.dir / name / 'limit.txt').read_text()
        self.assertEqual((limit('alpha'), limit('beta')), ('claude=2,codex=3', 'claude=1,codex=0'))
        self.agents = None  # unreadable: no free claude slot for the others
        self.main()
        self.assertEqual((limit('alpha'), limit('beta')), ('claude=0,codex=3', 'claude=0,codex=0'))

    def test_a_failing_and_a_slow_project_do_not_hide_the_other(self):
        self.main('--set', f'broken={self.dir / "broken"}', f'slow={self.dir / "slow"}', f'alpha={self.dir / "alpha"}')
        code, said = self.main('--timeout', '2')
        self.assertEqual(code, 1)
        self.assertIn('## broken\nError: tick exit 1: taskq.toml: not valid TOML', said)
        self.assertIn('## slow\nError: timeout after 2 s', said)
        self.assertIn('| [#1](https://github.com/acme/alpha/issues/1) alpha task | doing |', said)


if __name__ == '__main__':
    unittest.main()
