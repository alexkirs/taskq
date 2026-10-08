# Native Codex PM qualification (#224)

Status: bounded board-route qualification completed; native-worker and native
continuation qualification is incomplete.

Scope: one ordinary native Codex session on macOS, TaskQ `90786be`, 2026-10-08
UTC. This receipt uses only TaskQ's existing board transport, `selftest`,
`tick`, and Codex read/send routes. It adds no scheduler, queue, database,
permission change, private route, or rollout claim.

## Result

The board-first path works when an ordinary Codex session acts as the PM and
runs the existing TaskQ commands: independent temporary Claude and Codex
fixture identities completed `ready -> doing -> ask -> ready -> doing ->
review -> closed`, with tracker readback after every transition. Cleanup
removed the fixture issues and left no fixture worktree.

This does **not** qualify an end-to-end native Codex PM. The full fixture could
not start either real worker, and TaskQ has no configured native-Codex PM wake
target. A board result remains discoverable without a session UI, but an idle
native Codex PM has no supported automatic continuation/acknowledgement route.

## Actual evidence

| Check | Evidence | Verdict |
| --- | --- | --- |
| Local command route | `taskq preflight --json` at `2026-10-08T10:46:35Z`: `ready`, exit `0`, local subprocess, runtime capability and effective launch policy both `unknown` | qualified only for local command execution |
| Current native Codex session observation | `taskq runtime-status --runtime codex <PM session> --json` at `2026-10-08T10:44:33Z`: `status=unknown`, source unavailable, `Operation not permitted` | not qualified for status or permission visibility |
| Claude board fixture | `taskq selftest --scope quick --runtime claude`: 13/13 rows OK; temporary #238 closed and removed | board lifecycle qualified with a process fixture, not a native Claude worker |
| Codex board fixture | `taskq selftest --scope quick --runtime codex`: 13/13 rows OK; temporary #239 closed and removed | board lifecycle qualified with a process fixture, not a native Codex worker |
| Full worker fixture | `taskq selftest --scope full --runtime claude codex`: temporary #235--#237 removed; Claude spawn failed creating its job directory (`EPERM`), Codex spawn failed (`[Errno 1] Operation not permitted`) | native worker lifecycle unqualified |
| Capacity and cleanup | Full run's parallel claim gave exactly one winner; both failed worker chains were skipped; cleanup removed #235--#237 and reported no fixture worktree or board mismatch | cleanup verified; admission after successful native spawn unqualified |

The full-run failure was recorded through `taskq problem 224`. It did not
create a duplicate worker, fabricate a session link, change permissions, or
leave a live selftest task.

## Capability comparison

| Capability | DOT | Claude PM | Native Codex PM now |
| --- | --- | --- | --- |
| Authoritative task state | Board metadata, trusted notes and labels | Same | Same |
| Worker result/question discovery | Existing board/tick read | Existing board/tick read | Existing board/tick read when the session runs it |
| Result while worker UI is unavailable | Board route; session is diagnostic | Board route; session is diagnostic | Board route; session is diagnostic |
| PM wake/continuation | No qualified route | `tick --act --wake` resumes configured Claude coordinator | Unsupported: `--wake` targets only configured Claude coordinator |
| Native message to worker | Route-specific/unknown | Claude send path | `taskq codex-send`; receipt is delivery, not application |
| PM apply receipt | No inferred receipt | Decision note/state transition; delivery alone is `unknown` | Same board evidence when a Codex PM runs the decision |
| Real worker fixture | Unqualified here | Spawn blocked in this run | Spawn blocked in this run |

The semantic guarantees agree where the board is used: trusted actors and
state guards decide transitions; result, ask, answer, reject, and close are
read from the board; a session observation never turns an old terminal summary
into `busy`; and delivery is not application. The command surfaces do not
agree: only Claude has a configured coordinator wake route.

## Missing transition before coding

The missing transition is not result consumption. #223 already correlates the
board pending revision with a wake and re-reads it before delivery. The missing
piece is a supported native Codex-PM continuation contract:

1. identify the PM's Codex thread in project configuration without treating a
   worker thread as PM;
2. send a bounded continuation to that exact thread after a changed pending
   board revision, with the existing ownership checks;
3. leave delivered separate from applied; accept only the correlated board
   decision/state transition as application; and
4. deduplicate duplicate/out-of-order board revisions and preserve terminal
   state, restart recovery, capacity, and cleanup rules already owned by #217,
   #219, and #221.

Until that transition exists and a real owned worker can be spawned, the
following remain unqualified: active/waiting/terminal observation, user answer
delivery into an idle Codex PM, negative/incomplete result correction received
and applied by that PM, lost callback/restart recovery, stale-result handling,
and next-task admission after a native worker completes.

## Fixture boundary

`quick` uses real tracker writes and readbacks but process identities, so it is
separate evidence from unit tests and from a native-session worker route. The
failed `full` run is the relevant scoped live attempt. Do not replace it with
another scheduler, direct app-private protocol, fabricated session, or broad
live rollout. Re-run only after the two documented spawn blockers are fixed and
the native Codex-PM continuation route has an accepted contract.
