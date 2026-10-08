class Adapter:
    def __init__(self, remote_control=True, ops=None, **_):
        self.remote_control, self.ops = remote_control, ops

    def call(self, name, *args):
        return getattr(self.ops, name)(*args) if self.ops else None

    def spawn(self, name, prompt):
        if self.ops: return self.call('spawn', name, prompt)
        from taskq.worker import claude_spawn
        return claude_spawn(name, prompt=prompt, remote_control=self.remote_control)

    def send(self, session, text):
        if self.ops: return self.call('send', session, text)
        from taskq import claude_wake
        return claude_wake(session, text)

    def liveness(self, session):
        if self.ops: return self.call('liveness', session)
        from taskq import claude_agents
        agent = claude_agents().get(session)
        if not agent:
            return 'unknown'
        return 'dead' if agent.get('state') in ('done', 'failed', 'stopped') else 'alive'

    def list(self):
        if self.ops: return self.call('list')
        from taskq import claude_agents
        return claude_agents()

    def close(self, session):
        if self.ops: return self.call('close', session)
        from taskq import claude_stop
        return claude_stop(session, remove=True)

    def link(self, session):
        if self.ops: return self.call('link', session)
        from taskq import claude_url
        return claude_url(session)
