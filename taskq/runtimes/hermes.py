class Adapter:
    def __init__(self, **_): pass
    def spawn(self, name, prompt): return None
    def send(self, session, text): return None
    def liveness(self, session): return 'unknown'
    def list(self): return []
    def close(self, session): return None
    def link(self, session): return None
