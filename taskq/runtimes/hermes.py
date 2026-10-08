class Adapter:
    def __init__(self, ops=None, **_): self.ops = ops
    def call(self, name, *args): return getattr(self.ops, name)(*args) if self.ops else None
    def spawn(self, name, prompt): return self.call('spawn', name, prompt)
    def send(self, session, text): return self.call('send', session, text)
    def liveness(self, session): return self.call('liveness', session) if self.ops else 'unknown'
    def list(self): return self.call('list') if self.ops else []
    def close(self, session): return self.call('close', session)
    def link(self, session): return self.call('link', session)
