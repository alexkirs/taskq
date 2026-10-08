class Adapter:
    def __init__(self, remote_control=True, **_):
        self.remote_control = remote_control

    def spawn(self, name, prompt):
        from taskq.worker import claude_spawn
        return claude_spawn(name, prompt=prompt, remote_control=self.remote_control)

    def send(self, session, text):
        from taskq.worker import claude_wake
        return claude_wake(session, text)

    def liveness(self, session):
        from taskq import claude_agents
        agent = claude_agents().get(session)
        if not agent:
            return 'unknown'
        return 'dead' if agent.get('state') in ('done', 'failed', 'stopped') else 'alive'

    def list(self):
        from taskq import claude_agents
        return claude_agents()

    def close(self, session):
        from taskq import claude_stop
        return claude_stop(session, remove=True)

    def link(self, session):
        from taskq import claude_url
        return claude_url(session) or f'claude attach {session}'
