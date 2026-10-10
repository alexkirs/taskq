# Active-turn budget implementation checkpoint

Owner decision, 2026-10-10: count concurrent model turns separately from heavy
resources. An idle task keeps its exact worker and duplicate-prevention ownership.
This decision is not permission to resume production or ARM.

Implemented staged layer:

- Host binding accepts one `model:codex` unit only, with a real running PID/birth.
- Settlement requires that exact CLI to be dead, unchanged JSONL prefix, the bound
  native thread, one ordered turn and its final completed/failed record.
- Unknown launch cannot be revoked as if no process had started, or relaunched.
- Generic controlled-artifact settlement cannot substitute for model proof.
- Model settlement never records application acknowledgement or artifact acceptance,
  and never releases a heavy lease. Changed settled log evidence refuses release.

The focused tests include one finite Python child with actual native PID/birth;
its JSONL is synthetic. This is process-identity coverage, not real Codex proof.

Ordinary candidate wiring now uses durable board turn intents, explicit project/host
capacity, the existing named Codex spawn/resume adapter and native application receipts.
Schema3 is the sole ordinary schema; explicit repair0/1 ->3 preserves pending events.
Historical fake adapter tests mock model admission explicitly and do not qualify it.

Remaining qualification: the authorized real two-turn TaskQ worker probe and the
independent exact-head Windows gate. Earlier schema1/component gates are not this proof.

Windows coordinator reports that its disposable Python Job Object tree test passed,
but the real Codex test failed containment: owned Codex exited with an empty job while
fixture descendants remained outside it. Reported cleanup was identity verified.
That result is not a model-budget blocker under the selected semantics; it blocks
claiming generic heavy/resource drain. Broker/escape cause remains unresolved.

No production install, board repair, queue, ARM, publication or shared-app restart
was performed in this checkpoint. The authorized TaskQ real model probe remains
unconsumed here.

Mac verification: 12 focused tests passed; full suite 371 tests in 62.346s,
17 skips, exit0. Independent static review findings were fixed. Complete suite logs
and runtime digest are in `exports/queue-research/model-turn-budget-checkpoint/`.
