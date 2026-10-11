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

## Native application blocker (observed, not bypassed)

The committed technical application at
`d3fdfa9abbb6ed1391b6b9d829cac1a46e89c639` passed the final local full gate:
`python3 -m unittest tests.test_single` (311 tests, 17 platform skips, 52.537 seconds;
Hypothesis executed). Final focused pilot: 10 tests, 1.692 seconds, PASS.

After that commit the selected installed release
`b58b2e283dc15570b539c3e6b9d2d5da13909cbe` ran exactly:

```text
applied 641:9 --artifact experiments/state-model-641-answer9.md --sha d3fdfa9abbb6ed1391b6b9d829cac1a46e89c639
```

The identical command was repeated once. Both exited 1 with:

```text
taskq: applied artifact must be a real scoped file inside this task workspace
```

Fresh native issue readback after both refusals confirmed scope=[], the same exact
worker identity recorded above, answer9 still bound to its worker turn, and no
application_receipts["9"]. Answer9 contained only the already existing supervisor
ACK; it had no worker application ACK. Thus successful applied/idempotence is NOT
VERIFIED, and formal result submission is blocked. No claim/event/receipt was edited
to bypass this guard. The artifact exists in the required worker worktree, but the
installed adapter resolves CONFIG.root to the main project checkout (load_config's
worktree rule) and requires both a matching declared scope and exact root Git HEAD.
A supported scoped-worktree application route is an upstream prerequisite, not a
reason to write to main, rebind the worker, grant permissions or forge an ACK.

This final evidence-only amendment preserves the earlier exact code/test gate;
the amended artifact's own commit and affected documentation sentinel outcomes
are recorded in the PR/handoff. No formal successful result may be reported until
native application and its successful identical replay have been verified.

## Answer12: qualified native receipt handoff

Owner-approved continuation of answer9 in the same worker/session and PR649,
source Sentinel_68fb95025c788191b8c5d1916fc24ba7 and owner641.1. The owner approved
exact release `4fe8eae0a8dd3b3de3b693a8b29b0f63c2645cfd` after PR658 merged with exact CI.
Its contract was read before this handoff. The installed native route now verifies
registered `.worktrees/taskq-641` on branch `taskq-641`; scope=[] is not deny-all.
The historical refusals above remain evidence of the old release, not a current
application outcome. No old b58 command or old d3fdfa artifact SHA is reused.

This evidence-only addition applies answer12 by selecting the authorized qualified
route and retaining the existing answer9 implementation and all history. No new
implementation, model replay or full-suite rerun is required for this receipt-only
continuation. Prior code/test gates on d3fdfa9 and exact CI/documentation gates on
440a813 remain recorded; they are not relabeled as new-head executions.

Native applied commands for events 641:9 and 641:12 use the actual current full
HEAD containing this committed file. Verify working bytes against Git readback
before each first application; repeat each identical command once. First native
outcome must be applied=true/repeated=false, then applied=true/repeated=true.
Only native receipts/readback establish these outcomes; this pre-command artifact
does not fabricate an ACK. Submit formal result with that same head and truthful
check/receipt outcomes afterward. Any concrete refusal preserves the same worker,
identity and history, with no bypass or replacement. This continuation is technical
completion, not acceptance of rejected result7 or the research result.
