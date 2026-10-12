# Testing contracts

Policy: taskq.md §10. Runner: pytest; existing unittest/subTests unchanged.
Dependencies: requirements-test.txt, test-only. No paid models or live board writes.

| Contract | Existing behavioral checks |
|---|---|
| admission | Tick, Commands, HermesAdmission, board controls, assignment negatives |
| lifecycle | lifecycle/model integration, Commands, StateKernel, TestStateKernel, RealChild |
| answers | rejected result, delivery/application receipts, subscriptions, EventDelivery/Wait/MultiPM |
| recovery | owner/reset repair, legacy reconciliation, Failed, RecoveryQualification, SchemaRepair, kernel |
| capacity | native faulted HTTP anchors, model limits, Coordination, competing checkouts, kernel |
| restart | real service SQLite/IPC/children, model integration, PID/birth and turn boundaries |
| upgrade | Contract, DirectPublication/PullRequests, ProjectBootstrap, runner checks |

Selection table: tests/portfolio.py. Unclassified checks remain in every subset.
Unknown paths; shared taskq/service/schema/config/harness changes → full suite.
No selector claims coverage from filenames or replaces acceptance review.

Commands:

```
python -m pip install -r requirements-test.txt
python -m pytest --collect-only -q
python -m pytest --contract restart --contract recovery
python -m pytest --changed tests/test_host_service.py
python -m pytest --receipt test-receipt.json
```

Receipt: actual artifact bytes + installed dependencies + OS/Python/encoding;
tested commit/tree; final selected-node digest; full/subset/collection-only;
start/end/outcome; artifact drift; redacted failed contract/node/phase.
No prompts, question/result text, secrets, locals or exception payloads.
Hypothesis prints its native minimized fixture example; explicit seed recorded.
Fixture-specific task/run/request/role traces stay in their existing synthetic fixtures.
No invented trace fields when the fixture lacks them.
Dedicated CI output test-receipt.json is ignored; keep other receipts outside
the artifact checkout. Interpreter bytes and venv mode bind native-launch proof.
Windows venv redirectors can change the admitted PID: run native-boundary checks
with the base interpreter plus isolated test packages, as CI does. A venv refusal
is not service qualification and must not be bypassed by weakening PID/birth guards.

Reuse only identical artifact/environment AND selected scope, unchanged artifact,
successful non-collection run; timestamps/outcomes stay attached to the original run.
Fingerprint equality alone grants neither acceptance nor deployment authority.

CI: one full three-OS candidate gate; aggregate check stays named `tests` for
existing publication/updater consumers. No duplicate pull_request trigger.
Main post-merge alarm retained. Fork candidates require a maintainer-owned ref
or explicit workflow dispatch; acceptance still requires exact-head check evidence.

Retain existing native guards/encoding/race/recovery tests. Remove only a proved
duplicate after independent review; this first package deletes none.
