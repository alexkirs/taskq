"""Every adapter delegates the six runtime operations unchanged."""
import unittest

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
