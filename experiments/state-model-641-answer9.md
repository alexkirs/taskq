# #641 answer9 application evidence

Source: owner641.1 and Sentinel_68fb95025c788191b8c5d1916fc24ba7. Application event:
641:9. This is authorized rework of rejected result7 after supervisor ask8 in the
same worker/session and existing PR649, not acceptance of result7. Recorded worker:
`codex:01a127ef-d26c-73d1-8b8e-011575dce671`. No replacement was requested or made.

## Applied technical instruction

The separate `pilot_state`/`pilot_transition` implementation is removed. Existing
pure runtime bodies were extracted into `taskq.py:lifecycle_transition`:
accepted-result guard, answer creation, admission/generation continuity,
application receipt + worker ACK, and acceptance receipt validation. The actual
`lifecycle` calls this reducer. Filesystem/process/provider observations, native
identity checks, board guard, write/readback and effect boundaries stay in the
existing runtime adapters. No permissive execution fallback was added.

`StateKernel` calls the actual lifecycle/result adapters with hermetic FakeBoard
and mocked downstream providers. Its capture/replay test records exact reducer
inputs and outputs from actual runtime calls, then replays those inputs through
the same reducer. Hypothesis uses native task payloads for two tasks/two worker
identities and the existing `capacity_reserve` and `ReceiptOperation` reducers.
Tests supply explicit checkpoint/adapter facts and independent invariants, not a
second transition implementation. The two-worker runtime check runs both
serialization orders, sharing the actual capacity reducer and retaining identities
and history. It proves serialized admission, not arbitrary concurrent provider CAS.

Accepted-result proof reaches assertions: admit, verified fixture park/application,
actual acceptance and actual `queue_move` result submission, then admit/resume/answer
all refuse with the rework diagnostic; board remains unchanged and runtime.start
is not called. It no longer relies on sandbox tick to reach this requirement.
Four synthetic faults are detected in shared runtime reducers: omitted accepted
result guard, omitted unfinished-drain guard, foreign application session accepted,
and removed capacity ceiling. The first three replay through actual lifecycle.

The approved test-only Hypothesis 6.140.3 exception is retained. Runtime imports
remain stdlib. No new schema, daemon, storage, security/config/provider permission,
external board fixture, queue/ARM dispatch, production activation or unrelated
Later change. Quint remains conditional Later with Python replay; Lean is not
mandatory. Board/native receipts remain operational truth.

## Recorded checks and limits before final gate

- Affected reducers/runtime + existing receipt, repair and model-capacity checks:
  35 tests passed in 2.118 seconds. The subsequently added two-worker runtime case
  is included in the final focused pilot check and final full gate.
- Focused pilot exercises deterministic replays and 100 Hypothesis examples with
  at most 50 steps, derandomize enabled, no persistent example database/deadlines.
- Existing local controlled-child boundary checks passed: real child park/answer/
  resume/repeat, crashes at park board-write boundaries and unknown birth refusal
  (3 tests, 1.236 seconds). These use the pre-existing disposable fixture and native
  SQLite/process boundary, not a remote provider or generic Codex writer-drain proof.
- Existing native local checks cover model settlement while heavy grants remain,
  dead CLI not proving application drain, lost readback after committed effects,
  duplicate ACK/effect preservation, and partial repair resume. Retain them rather
  than duplicate their transitions in an experimental model.
- First focused attempts exposed fixture errors (Mock project domain was not JSON
  data; JSON serialization changed integer FakeBoard keys). These were corrected
  in fixtures, not by weakening production guards. They are not historical bugs.
- Historical regression shapes are exercised at the current shared runtime, and
  synthetic fault sensitivity is observed. Pre-fix historical revisions were not
  executed; historical red-before-fix remains unknown. No paid model calls or live
  provider qualification was launched. Model PASS is not native/provider proof.

Final exact-head command/CI outcomes are recorded in the TaskQ handoff/PR after
this artifact's commit. A committed artifact alone is not application ACK: native
`applied 641:9` must verify it, and an identical repeat must return repeated=true
before formal result. At preflight the native task had scope=[]; the installed
applied adapter also resolves project-root Git rather than the worker worktree.
These guards are not overridden. A native refusal must preserve this exact worker,
pending event/history and failure, and block claiming successful application.

Read-only tool limitation: `headroom_retrieve` returned
`MCP tool call requires approval, but approval policy is never`. No approval or
permission change was attempted; normal allowed source reads supplied the needed
code. A malformed read-only `status 641` invocation was also refused without effects.

Rejected result7 evidence is retained in `state-model-641.md` as historical evidence,
explicitly superseded by this answer application. This document is a repository
artifact for the existing native application receipt, not a second task authority.
