"""Bounded sender contract model; no transport, timers or board mutation."""
class Handshake:
    def __init__(self):
        self.pending = None
        self.stopped = False

    def observe(self, event, acknowledged):
        if self.stopped or acknowledged is None:
            self.stopped = True
            return 'stop'
        if acknowledged:
            if self.pending == event:
                self.pending = None
            return 'superseded'
        if self.pending is not None:
            return 'pending'  # duplicate before ACK never creates another delivery
        self.pending = event
        return 'deliver'

    def delivered(self, success):
        if not success:
            self.stopped = True
        # success alone does not permit next wait

    def handled(self, event, receipt):
        if self.stopped or receipt is None or event != self.pending:
            self.stopped = True
            return 'stop'
        if receipt:
            self.pending = None
            return 'next-wait'
        return 'pending'
