# Queue decisions

Living specification. Owner decisions, 2026-10-12.

## Capacity

- `limits.codex=N`: at most N admitted Codex tasks per host.
- One worker per task. Supervisor does not consume worker capacity.
- Task owns slot through execution, review and merge.
- Close or failed releases slot after verified retirement.
- Parked question releases slot after verified retirement; answer requires fresh admission.
- Supervisor waits without active model turn. Result wakes supervisor.
- Supervisor active turns use separate bounded model capacity.
- Review admitted tasks before preparing new tasks.
- Provider/model limits remain separate from worker count.
- Never exceed worker count during restart, retry or concurrent admission.
- Initial Windows limits: tasks 4, worker turns 4, supervisor turns 4.
- Shared project ceilings: tasks 8, worker turns 8, supervisor turns 8.
- Close release proves recorded native turns stopped and saved sessions archived.
- Close release never claims detached writer fencing.
- Ordinary ask/failed with unproved retirement retain slot; automatic release remains unsupported.
- Existing doing/review owners require explicit accounting before new admission.
- Count covers admitted tasks participating in new slot ledger.
- Historical held ask tasks remain outside ledger; never resume or import automatically.
- Historical external writers remain unproved; no total historical writer-count guarantee.

## Board

- Board stores commands, questions, answers, steering and results.
- Tick reads pending events and wakes recipient.
- Recipient records handling/application receipt. Sending alone proves no application.
- Direct session messages optional; never authoritative queue control.
- Preserve full per-task event history.

## Outcomes

- Later: deferred work, needs future decision.
- Failed: attempt ended without release; includes abandoned unfinished work.
- Failed preserves result and history for investigation.
- Failed label never proves old writer fenced.
- Unknown writer retains execution block until fencing proved.

## Measurement

- Benchmark: ten Windows Codex tasks, same workload and topology.
- Record admission, worker, review, merge, close and retirement times separately.
- Disclose retries, errors, pauses, fixes and manager interventions.
- Preserve prior evidence before deleting benchmark tickets.
- Interrupted series is not completed throughput baseline.
