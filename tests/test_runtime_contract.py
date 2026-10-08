"""Every adapter delegates the six runtime operations unchanged."""
import argparse
import contextlib
import importlib
import io
import os
import unittest
from unittest.mock import patch

import taskq as core
from taskq import runtimes
tick = importlib.import_module('taskq.tick')  # `taskq.tick` the attribute is the command function
from taskq.worker import spawn
from taskq.runtimes import claude, codex, dot, hermes


class Fake:
    def __init__(self):
        self.calls = []

    def spawn(self, *args): return self.record('spawn', *args)
    def send(self, *args): return self.record('send', *args)
    def liveness(self, *args): return self.record('liveness', *args)
    def list(self): return self.record('list')
    def close(self, *args): return self.record('close', *args)
    def link(self, *args): return self.record('link', *args)

    def record(self, name, *args):
        self.calls.append((name, args))
        return f'{name}-result'


class RuntimeContractTests(unittest.TestCase):
    def test_every_adapter_delegates_all_operations(self):
        for module in (claude, codex, dot, hermes):
            with self.subTest(runtime=module.__name__):
                fake = Fake()
                adapter = module.Adapter(ops=fake)
                self.assertEqual(adapter.spawn('name', 'prompt'), 'spawn-result')
                self.assertEqual(adapter.send('session', 'text'), 'send-result')
                self.assertEqual(adapter.liveness('session'), 'liveness-result')
                self.assertEqual(adapter.list(), 'list-result')
                self.assertEqual(adapter.close('session'), 'close-result')
                self.assertEqual(adapter.link('session'), 'link-result')
                self.assertEqual(fake.calls, [('spawn', ('name', 'prompt')), ('send', ('session', 'text')),
                                              ('liveness', ('session',)), ('list', ()), ('close', ('session',)),
                                              ('link', ('session',))])

    def test_spawn_names_every_session_by_one_scheme(self):
        """#268: `<T|S><N> <ORCH> <title> (<machine>)` on every adapter; ORCH is the launching runtime's code."""
        for module in (claude, codex, dot, hermes):
            for launcher, orch in (('claude', 'CLD'), ('codex', 'CDX'), ('dot', 'DOT'), ('hermes', 'HRM'), ('grok', 'GRK'), ('other', 'UNK')):
                with self.subTest(runtime=module.__name__, launcher=launcher):
                    fake = Fake()
                    args = argparse.Namespace(runtime=module.__name__.rsplit('.', 1)[1], name='T7 Research it', text='go', remote_control=True)
                    with patch.object(runtimes, 'get', lambda *_, **__: module.Adapter(ops=fake)), \
                            patch.object(core, 'machine', lambda: 'mac'), patch.object(core, 'task', lambda iid: {}), \
                            patch.object(core, 'note', lambda *_: None), patch.dict(os.environ, {'TASKQ_RUNTIME': launcher}), \
                            contextlib.redirect_stdout(io.StringIO()):
                        spawn(args)
                    self.assertEqual(fake.calls, [('spawn', (f'T7 {orch} Research it (mac)', 'go'))])

    def test_session_name(self):
        with patch.dict(os.environ, {'TASKQ_RUNTIME': 'codex'}):
            self.assertEqual(runtimes.session_name('T256 Research it', 'mac'), 'T256 CDX Research it (mac)')
        self.assertEqual(runtimes.session_name('S257 x', 'mac', 'dot'), 'S257 DOT x (mac)')
        self.assertEqual(runtimes.session_name('T256 CLD x (mac)', 'mac', 'codex'), 'T256 CLD x (mac)')
        self.assertEqual(runtimes.session_name('probe', 'mac', 'claude'), 'probe (mac)')

    def test_parsers_accept_old_and_new_names(self):
        for name in ('T256 Research it (mac)', 'T256 CLD Research it (mac)', 'T256 UNK API fix (mac)'):
            with self.subTest(name=name):
                self.assertEqual(tick.worker_iid(name), 256)
                self.assertEqual(tick.session_iid(name), 256)
        self.assertEqual(tick.supervisor_iid('S9 CDX x (mac)'), 9)
        self.assertEqual(runtimes.orchestrator('T256 CLD x (mac)'), 'CLD')
        self.assertIsNone(runtimes.orchestrator('T256 API fix (mac)'))

    def test_report_row_shows_the_orchestrator(self):
        item = {'iid': 5, 'title': 't', 'state': 'doing', 'claim': {'runtime': 'claude', 'session': 'abc'}}
        with patch.object(core, 'ref', lambda item: f'#{item["iid"]}'), patch.object(tick, 'session_link', lambda *_: 'link'):
            self.assertIn('| claude, by CDX |', tick.report_row(item, {'abc': {'name': 'T5 CDX t (mac)'}}, 'busy'))
            self.assertIn('| claude |', tick.report_row(item, {'abc': {'name': 'T5 t (mac)'}}, 'busy'))
