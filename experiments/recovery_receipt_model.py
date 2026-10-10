"""Fixture-only recovery correlation model; no runtime adapter or execution authority."""
import hashlib,json
CONFIG={}  # injected isolated fixture configuration, never production setup

class RecoveryOperation:
    """Dormant correlation model: serializable qualification state, never authority to execute/unlock."""
    stages = ('drain', 'continue', 'board', 'application')

    def __init__(self, item, role):
        if role not in ('worker', 'supervisor'):
            raise ValueError('unsupported recovery role')
        identity = item.get('claim' if role == 'worker' else 'supervisor') or {}
        self.target = {'task': item['iid'], 'role': role, **{key: identity.get(key) for key in ('session', 'runtime', 'name')}}
        if type(self.target['task']) is not int or self.target['task'] <= 0 or any(
                not isinstance(self.target[key], str) or not self.target[key] for key in ('session', 'runtime', 'name')):
            raise ValueError('complete recorded role identity required')
        self.repo = CONFIG.get('repo') or str(CONFIG['root'])
        raw = item.get('raw') or {}
        payload = raw  # exact authoritative payload, including answers, events and recipient acknowledgements
        self.payload = hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()
        self.id = hashlib.sha256(json.dumps([self.repo, self.target, self.payload], sort_keys=True).encode()).hexdigest()[:20]
        self.journal, self.phase = [], 'held'

    def matches(self, item):
        try:
            return RecoveryOperation(item, self.target['role']).id == self.id
        except (ValueError, KeyError, TypeError):
            return False

    def request(self, stage):
        if stage not in self.stages:
            raise ValueError('unsupported receipt stage')
        count = sum(r['kind'] == 'receipt' for r in self.journal)
        expected = self.stages[count] if count < len(self.stages) else None
        if stage != expected or self.phase in ('blocked', 'applied'):
            raise ValueError('stage out of order or outcome unresolved')
        request = {'kind': 'request', 'operation': self.id, 'stage': stage,
                   'request': f'{self.id}:{stage}', 'target': dict(self.target), 'payload': self.payload}
        prior = next((r for r in self.journal if r['kind'] == 'request' and r['stage'] == stage), None)
        if prior:
            return json.loads(json.dumps(prior))  # retrying a read of intent does not authorize retrying its effect
        self.journal.append(request)
        self.phase = f'awaiting-{stage}'
        return json.loads(json.dumps(request))

    def receipt(self, value):
        if not isinstance(value, dict):
            raise ValueError('malformed receipt')
        receipt = json.loads(json.dumps(value))
        stage = receipt.get('stage')
        request = next((r for r in self.journal if r['kind'] == 'request' and r['stage'] == stage), None)
        if not request or any(receipt.get(key) != request[key] for key in ('operation', 'stage', 'request', 'target', 'payload')):
            raise ValueError('wrong task/role/session/operation/request receipt')
        prior = next((r for r in self.journal if r['kind'] == 'receipt' and r['stage'] == stage), None)
        if prior:
            if receipt != prior:
                raise ValueError('conflicting duplicate receipt')
            return  # identical readback, not permission for a repeated effect
        if self.phase != f'awaiting-{stage}' or receipt.get('kind') != 'receipt' or receipt.get('status') not in ('ok', 'unknown', 'declined'):
            raise ValueError('receipt out of order or malformed status')
        proof = receipt.get('evidence')
        if not isinstance(proof, dict):
            raise ValueError('receipt evidence missing')
        if receipt['status'] == 'ok':
            required = {'drain': ('session_unloaded', 'inflight_reconciled'),
                        'continue': ('same_session', 'native_birth_verified'),
                        'board': ('identity_rechecked', 'history_preserved'),
                        'application': ('exact_payload_applied',)}[stage]
            if proof.get('source') != 'qualified-runtime' or any(proof.get(key) is not True for key in required):
                raise ValueError('unqualified evidence; manual assertions/timeout are not a receipt')
        self.journal.append(receipt)
        self.phase = ('blocked' if receipt['status'] != 'ok' else
                      'applied' if stage == 'application' else f'{stage}-recorded')

    def snapshot(self):
        return json.loads(json.dumps({'schema': 1, 'repo': self.repo, 'target': self.target,
                                     'operation': self.id, 'payload': self.payload, 'phase': self.phase, 'journal': self.journal}))

    @classmethod
    def restore(cls, saved, item):
        if not isinstance(saved, dict) or saved.get('schema') != 1 or not isinstance(saved.get('target'), dict):
            raise ValueError('unsupported recovery checkpoint')
        operation = cls(item, saved['target'].get('role'))
        if saved.get('operation') != operation.id or saved.get('repo') != operation.repo or saved['target'] != operation.target:
            raise ValueError('stale recovery checkpoint identity')
        if not isinstance(saved.get('journal'), list):
            raise ValueError('invalid recovery journal')
        for value in saved['journal']:
            if not isinstance(value, dict):
                raise ValueError('invalid recovery journal entry')
            if value.get('kind') == 'request':
                if operation.request(value.get('stage')) != value:
                    raise ValueError('invalid request correlation')
            else:
                operation.receipt(value)
        if operation.phase != saved.get('phase') or operation.snapshot() != saved:
            raise ValueError('checkpoint does not match validated receipt replay')
        return operation

