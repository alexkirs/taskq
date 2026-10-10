# #641 bounded state-model pilot

Historical evidence for rejected result7, retained unchanged below. Supervisor
ask8 rejected the separate pilot implementation. Current same-worker answer9
application is documented in [state-model-641-answer9.md](state-model-641-answer9.md).

Owner641.1 authorized this research-only two-task/two-worker pilot. R13 and §10
were amended before implementation. No queue, ARM, dispatch, production schema,
receipt store, daemon or production activation changed. Other Later work is unchanged.

## One executable kernel

`taskq.py:pilot_transition` owns pure transitions. The isolated runtime runner
`experiments/state_replay.py` and Hypothesis state machine call that exact function;
the tests contain independent safety assertions, not a second transition model.
The kernel is deliberately not wired to production admission: this pilot grants
no execution authority. It models explicit fresh revisions, one immutable owner
per task/one task per worker, capacity-one model and heavy dimensions, application
receipts distinct from ACKs, and resumable repair checkpoints. Its plain in-memory
values are test inputs, not a new persisted board schema.

Replay command: `python3 experiments/state_replay.py < trace.json`, where a trace
is a JSON command list with `task` (1/2), `worker` (A/B), `action`, and explicit
`revision`. Refusals are recorded in output without changing state. The unittest
scenarios construct complete reproducible traces, including refused commands.
A stale second claim models two guarded serialization orders; this is not proof
of a provider's atomic guard or arbitrary thread interleavings.

The terminal/drain inputs are abstract qualified observations. Setting a boolean
in a replay is never native drain evidence. Unknown/false observations retain the
relevant grant. CLI death plus terminal evidence releases only model capacity;
a live writer retains heavy capacity and the task's resume/accept/repair barrier.
Receipt readback after a lost ACK does not execute the effect again.

## Dependency review and checks

The checkout had no requirements/pyproject dependency declaration, no Hypothesis
imports and no installed Hypothesis in the source-test Python. Existing runtime
is stdlib. `requirements-test.txt` pins Hypothesis 6.140.3 (test-only; its attrs and
sortedcontainers dependencies are installed only in the test environment).
An isolated temporary venv was used; no global install. CI installs the test
requirement. Without Hypothesis the normal suite explicitly skips the pilot;
such a skip is not PASS. Stateful settings: 100 deterministic examples, at most
50 steps each, no persistent example database, no deadlines. Hypothesis shrinks
failing sequences; deterministic named traces retain known regression shapes.

| Requirement | Check / detected failure | Evidence / blindspot |
| --- | --- | --- |
| Ownership, stale/concurrent claims, resume | StateKernel both claim serialization orders; KernelMachine immutable owner and revision assertions | Model only; no concurrent provider write proof |
| Accepted result refuses resume | StateKernel accepted-result trace, injected omitted acceptance guard | Synthetic red observed; existing lifecycle regression shape replayed, historical pre-fix revision not executed |
| Dead CLI with live writer, model/heavy separation | StateKernel separate-capacity trace, injected heavy release/model ceiling removal | Synthetic red observed; no native writer drain proof |
| Lost ACK after effect, duplicate/out-of-order application | StateKernel receipt trace, injected duplicate effect/delivery ACK | Synthetic red observed; no durable crash proof |
| Repair restart | StateKernel checkpoint trace, injected premature completion | Synthetic red observed; no storage introduced |
| Existing local repair/write boundaries | SchemaRepair.test_mixed_board_blocks_unrelated_task_and_partial_repair_resumes; NativeAction.test_lost_final_response_readback_is_idempotent | Passed with fake downstream; not native provider qualification |

Seven injected faults are required to fail their same deterministic scenario
assertions. These are synthetic sensitivity tests, not claims of historical
red-before-fix execution. Existing historical regression shapes (accepted-result
rework, lost response after commit, partial repair) are replayed at the behavioral
level. Exact historical failure-before-fix revisions were not available/run;
that evidence is unknown. No #495/#496/#311/#481 campaign was added.

The supplemental Lifecycle.test_accepted_result_refuses_direct_and_normal_rework_before_effects
failed before its assertions: inherited installed-release selection rejected a
source checkout; isolated source testing then hit the sandbox's intentional tick
refusal. No permission/sandbox change was made. That local/native category is NOT
VERIFIED here. Model PASS cannot replace it, and production promotion remains
subject to the contract's separate qualification gates.

Quint remains Later only if interleaving/liveness questions outgrow these bounded
Python traces; it must replay traces through this kernel. Lean is not mandatory.
No paid model calls or live board fixtures were launched. Costs and flake rate
are unknown; the first focused model run passed, the supplemental boundary run
had the environmental failures above, not an unchanged-suite flake claim.
