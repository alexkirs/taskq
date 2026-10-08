"""Every runtime adapter keeps the small common surface."""
import unittest

from taskq.runtimes import claude, codex, dot, hermes


class Fake:
    def __init__(self): self.calls = []
    def __call__(self, *args, **kwargs): self.calls.append((args, kwargs)); return 'session'


class RuntimeContractTests(unittest.TestCase):
    def test_every_adapter_has_the_runtime_contract(self):
        for module in (claude, codex, dot, hermes):
            with self.subTest(runtime=module.__name__):
                adapter = module.Adapter()
                for name in ('spawn', 'send', 'liveness', 'list', 'close', 'link'):
                    self.assertTrue(callable(getattr(adapter, name)))
