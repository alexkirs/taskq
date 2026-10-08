import argparse


class Adapter:
    def __init__(self, full_access=False):
        self.full_access = full_access

    def spawn(self, name, prompt):
        from taskq import codex_spawn
        return codex_spawn(name, prompt, self.full_access)

    def send(self, session, text):
        from taskq import codex_send
        return codex_send(argparse.Namespace(thread=session, text=text, full_access=self.full_access))

    def liveness(self, session):
        from taskq import Codex, codex_snapshot
        try:
            codex = Codex()
            try:
                status, _, _ = codex_snapshot(codex, session, 1)
            finally:
                codex.socket.close()
        except (OSError, SystemExit, ValueError):
            return 'unknown'
        return 'dead' if status['type'] == 'systemError' else 'alive'

    def list(self):
        from taskq import Codex
        codex = Codex()
        try:
            return codex.call('thread/list', {}).get('data', [])
        finally:
            codex.socket.close()

    def close(self, session):
        from taskq import codex_archive
        return codex_archive(argparse.Namespace(thread=session))

    def link(self, session):
        from taskq import PAGES
        return f'{PAGES.rstrip("/")}/open.html#codex://threads/{session}'
