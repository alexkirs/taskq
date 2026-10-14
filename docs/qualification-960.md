# macOS tray answer qualification (#960)

Historical native interaction on 2026-10-14, collected for
[#960](https://github.com/taskqorg/taskq/issues/960). This qualifies real numbered
answers reaching the board and the same workers continuing. It does not qualify
the tray's completion indicator: the owner observed it staying on **Sending**.
No synthetic task or new board answer was created by this qualification worker.

## Native artifact and observation

The host build transcript records checkout of TaskQ commit
`7b4884c39bfbdd754fbc0dbb1f0934396285c37b`, a release build with version
`4.2.0-dev.1791954717`, successful selftest and installation at 05:12 UTC.
At 05:14 UTC, process readback recorded that version running `taskq tray`.
This is a locally built development artifact, with its Cargo version changed
for installation; no historical binary hash or release attestation was captured.
The source/build/process association comes from that transcript, not from a
commit identifier embedded in the binary.

The [owner/PM report](https://github.com/taskqorg/taskq/issues/960#issuecomment-6162209422)
identifies both subsequent interactions as macOS tray answers and records the
Sending defect. This worker did not witness or repeat those clicks. Transcript
excerpts were inspected locally; private paths and unrelated conversation are
not published here.

## Board readback and worker continuation

The worker re-read the board comments through the GitHub API. The links below
are durable board acknowledgements, not a captured native IPC JSON receipt.
Operation IDs and expected revisions match the original ask cards. Subsequent
entries name the original worker attempts; a board answer alone is not used as
proof of resumption.

| Chain | Ask revision / worker attempt | Answer read back | Same-attempt continuation |
| --- | --- | --- | --- |
| dashq#63 | `6162059014` / `6162046186` | [answer 1](https://github.com/taskqorg/dashq/issues/63#issuecomment-6162078103), 06:07:36 UTC; `qac-mv7pjru06pudi47e592` | [requeue](https://github.com/taskqorg/dashq/issues/63#issuecomment-6162104010), 06:09:38 UTC; worker applied the dependency decision and returned the task to wait for #96 |
| dashq#93 | `6162080172` / `6162075881` | [answer 2](https://github.com/taskqorg/dashq/issues/93#issuecomment-6162111267), 06:10:12 UTC; `qac-mv7pn5dmjxkt0yz57f` | [result](https://github.com/taskqorg/dashq/issues/93#issuecomment-6162144334), 06:12:46 UTC; worker delivered the published-release policy in PR #98 |

#63 was an existing dependency decision. #93 was a real asset-source decision
with a resulting contract change. Neither answer was issued by this worker.
The native receipt display remains unqualified even though the board retained
the answers and both workers continued.

## Project checks and exact scope

The evidence branch starts at
`19618334f2a73505c340db4ff1f9b9e44415ea71`; its only change is this document.
No application source, test, dependency, fixture, configuration or contract rule
is changed. The historical tray artifact above and this evidence base are
separate candidates.

On that base, the worker's `cargo test --locked` reported 147 passed, 2 failed,
1 ignored. `platform::tests::host_memory_reads_on_this_host` could not read host
memory; `policy::tests::seatbelt_keeps_workers_out_of_the_policy_store` failed
with `sandbox-exec: sandbox_apply: Operation not permitted`. These are recorded
failures, not skipped or converted to passing tests.

The PM then ran `cargo test -q --locked` on the same base outside the worker
sandbox. The inspected output reports 149 passed, 0 failed, 1 ignored, plus
integration groups of 5 and 1 passing tests. The
[host evidence](https://github.com/taskqorg/taskq/issues/960#issuecomment-6162369435)
records both previously failing host checks passing. This is a host run on the
base commit, not an exact-head host run after this document was committed.
The worker's `cargo run -- selftest` on the base passed 69 golden traces, card
formats and a live board read. Final candidate commands and outcomes belong in
the result/PR, with sandbox failures kept distinct from the PM's host evidence.

## Open

- Sending remained visible after the board accepted the answer. No fix or fresh
  native completion-display check is included. The
  [follow-up menu task](https://github.com/taskqorg/taskq/issues/1021) is separate;
  its existence does not prove this defect fixed.
- Free text, stale/unknown delivery, retries, other macOS builds and Windows
  were not qualified by these two numbered-answer interactions.
