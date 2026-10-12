# CLI message audit (#597)

Scope: the owner's v4-only CLI validation/report/help/error slice, selected in
[the task](https://github.com/alexkirs/taskq/issues/597). The v3 prototype remains
archived; it is not part of this candidate. Tray presentation and documentation
implementation are separate slices. Decisions remain in [taskq.md](../../taskq.md)
and the existing [format handoff](message-formats-handoff.md).

Baseline locations below refer to `4f91c83` on main; current function names make
the findings locatable after edits. No lifecycle golden trace changed.

| Surface | Baseline deviation (file:line) | Candidate behavior / proof |
|---|---|---|
| Input character limits | `src/main.rs:238`, `width`: repeated spaces disappeared; an unmatched backtick exempted the rest of a line. | Count outside whitespace and Unicode characters; only paired code spans are exempt. Boundary regression includes repeated spaces, Unicode and unmatched backtick. |
| One-line input | `src/main.rs:257`, `one_line`, and `:294`, `ask_card`: carriage returns and multiline evidence links bypassed one-line checks. | Reject CR/LF in one-line fields and evidence input before writing. |
| Answer/instruction writes | `src/main.rs:1052`, `:1069`: no four-line/240-character validation. | Validate original nonblank lines before `act`; rejected input adds no comments. |
| Park/retry/hold writes | `src/main.rs:1064`, `:1069` and `add --later`: Reason/Next template not enforced. | Validate original line widths and labelled Reason/Next before any write, including task creation. Optional absent text stays absent where the command permits it. |
| Review verdict | `src/main.rs:265`, `fields`: rejected repeated labelled Checks lines; `:1074`: no exact result SHA check. | Accept 1–8 Checks lines in order; supplied review verdict must name the current SHA in Checks. Other repeated fields remain invalid. |
| Status task titles | `src/main.rs:472`, `report`: silently cut titles at 60 characters. | A truncated title ends in ellipsis within 60 characters and links to the complete issue. Stored title unchanged. |
| Status questions | `src/main.rs:499`: flattened card boilerplate, empty reason partition, unbounded options. | Extract Problem and its continuation; reasons max 160, options max 120, linked full text. Keep every N.M token and recommendation; old cards retain their first-line fallback. |
| Failed reasons | `src/main.rs:519`: unbounded comment body. | Compact linked reason; original log remains available through issue/inbox. |
| Dependency/session cells | `src/main.rs:473`, `:480`, `:437`: filtered task list omitted blockers outside the filter and ordinary issues; session host label bypassed cell escaping. | Retain open dependency IDs from the same board snapshot in `src/service.rs::View`; escape host/runtime display text. Fold condition unchanged. |
| Single/batch close outcomes | `src/main.rs:361`, `:406`, `:1074`: different success/error shapes; a failed current-state read aborted the batch. | `#N closed`, then publication acknowledgement; shared Error/Next with task identity. Read failure does not stop subsequent items. |
| Errors/refusals | `src/platform.rs:165`, `die`: unrestricted `taskq:` output. | Error/Next renderer; compact errors retain complete diagnostics in the local CLI error log. If saving fails, raw evidence remains visible. Tested both paths. |
| Help | `src/main.rs:23`, `:946`: verb-only usage, help treated as failure and `--help` required a value. | Full command syntax; parsed help returns before config, board access or dispatch, including `service pause --help` and `close N --help`. |
| Service list/focus | `src/main.rs:843`, `:864`: joined fields; missing focus became off; no counts/freshness/unknown reason. | One project/field/label per line; fresh successful cache supplies R6 counts and age. Missing/stale/failed data and unchecked process state stay unknown. Raw status JSON remains a diagnostic exception. |
| Service acknowledgements | `src/main.rs:871`, `:900`, `:917`: unnamed actions; ignored pause/stop/removal failures. | Name project or exact unit path; propagate CLI pause-write, stop-command and removal/registry-write failures. Stop acknowledges the request, not verified process exit. Tray pause caller only adapts to the fallible helper. |
| Selftest | `src/main.rs:951`: board/config skip was silent; board summary joined unrelated fields. | One outcome per field/check; offline or missing config explicitly says board unknown and no read. |

R7 now explicitly shares the batch renderer and retains complete compact-error
diagnostics. Existing width, close-output and fallible-pause assertions are updated
for those formats; their safety assertions remain. The new regressions also cover
historical text preservation, linked Unicode truncation and unknown service data.

Before/after examples are illustrative, not client screenshots:

- Question reason: `Done: inspected … To answer: …` → just `Problem` text, with
  an issue link for the complete card. No option or answer token is removed.
- Long title: silently cut 60-character prefix → 59 characters plus `…`, linked
  to the unchanged title.
- Missing service data: `focus: off` → `Focus: unknown (focus file unavailable)`
  and explicit unknown service/freshness/counts.
- Close: `closed #7: merged <sha>` → `#7 closed`, then `merged <sha>`; the exact
  publication evidence is retained.

Qualification: native Windows Rust tests and live read-only selftest are the
project gates. CLI help/error rendering is exercised directly; report snapshots
are covered through the fake board. No actual Codex/app, DOT, narrow screenshot,
media-accessibility, live stop/uninstall, macOS or Linux verification is claimed.
Those channel/platform proofs remain separate from model/format tests. The v3
Python suite is absent after cutover and is not presented as a passing gate.

Separate follow-up scope: native tray/menu text and rendering, documentation text
audit, and actual channel/width/media receipts. Raw service logs, heartbeat/spawn/
gone protocol entries, original inbox text and stored historical cards retain
their diagnostic/protocol form. Board label decisions shipped through #776/#777;
this slice does not change label names, palette, routing, model or permissions.
