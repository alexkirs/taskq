# Dormant recovery slice after f12b41e

Historical qualification record. The receipt reducer described below now lives only in
`experiments/recovery_receipt_model.py`; runtime keeps read-only identity/payload correlation.
There is still no authenticated app drain/recovery adapter. Current limits are in taskq.md.

Spec first; isolated branch only. Frozen Windows ZIP f12b41e remains unchanged.

Implemented recovery-plan GET-only unsupported/manual handoff; no apply command. RecoveryOperation preserves exact task/role/session/runtime/host and hashes the entire authoritative payload. Ordered drain, continuation, board and application receipts support strict checkpoint replay; stale identities/payloads, conflicting duplicates, malformed or out-of-order receipts fail closed. This model correlates fixture evidence only: it does not authenticate runtime claims or provide native drain/continuation execution.

Opt-in tick --unknown-after accepts finite positive minutes. Board inactivity can produce one persisted recovery question for a local unknown identity; never death/takeover. Existing question, result, role, order, event history and pending payloads are preserved. Fresh result/activity/identity checks precede updates. Defaults are unchanged. Unknown slot remains reserved; no production invocation performed.

15 hermetic recovery tests passed. Full Mac suite: 276 tests in 43.772s, OK (17 skipped); the subsequent Windows-only temporary USERPROFILE fixture adjustment has not been executed on Windows. Independent review found stale activity/result and incomplete payload correlation; both fixed with regression tests. Final reviewer found no blocking issue in the dormant slice. No live app lifecycle canary or production recovery claimed.

Windows verified frozen ZIP SHA256 3b12a514870479be3b0d80622e24853200e9ead78493204266c6ec95e9cb8b1e, 131051 bytes and bundle f12b41e. Frozen native diagnostic tests: 26 tests, 6 errors, exit1; Path.home cannot resolve because fixture clears Windows home environment. This next commit supplies USERPROFILE pointing only to its temporary isolated root; Windows rerun is pending, not PASS. No CODEX_HOME workaround or production policy change.

Architectural blocker: no qualified supported exact-session app writer/controller drain and in-flight reconciliation adapter is available. A new process birth cannot repair a legacy PID or prove old ownership release. Native same-session continuation therefore remains unsupported/manual, with guard retained. No queue restart, shared-server restart, lock deletion, identity rebind, install, push or publication.
