# Board label implementation and adoption boundary

The owner answer on #769 stages implementation here; live adoption follows in [#776](https://github.com/alexkirs/taskq/issues/776), parked until the compatible release and owner all-host upgrade/idle gate. No board repaint, global rename, closed issue edit or service/configuration change was performed for this implementation.

The catalog lives in `src/labels.rs`. After approved setup, `taskq labels` applies the fixed catalog; normal issue creation also ensures it and the used dynamic labels. Both adapters compare metadata before writing and read it back afterward; issue creation/projection verify assignments. A partial failure names the unfinished label; retry skips already-correct metadata. An issue-creation readback failure identifies the issue number: inspect before retry to avoid duplicates.

Existing boards with any legacy label object automatically retain legacy writes. `label_schema=canonical` is the explicit adoption switch after the gate; `legacy` pins the old write scheme. Both schemes accept priority and focus/status aliases. Fold states/logs/golden traces are unchanged. Conflicting single constraints are diagnosed and never claimed; add rejects invalid/repeated single options before creating an issue. Do not remove legacy objects merely to trigger auto-detection.

| Criterion | Implementation evidence | Qualification boundary |
|---|---|---|
| B-A1 | Catalog/default/alias validation tests; CLI add and service conflict tests | Native board metadata deferred to #776 |
| B-A2 | Both adapter mocks assert exact HEX and platform color encoding | GitHub/GitLab native light/dark rendering unknown |
| B-A3 | Setup catalog and create share ensure path; mocks repeat without writes, preserve unrelated metadata | Fresh native setup deferred |
| B-A4 | Canonical projection, alias priority/focus/status, add defaults and conflict admission tests; unchanged golden traces | Existing services not upgraded here |
| B-A5 | Legacy auto-detection and explicit adoption gate; #776 requires open-only reassignment, filter inventory and closed-issue preservation | Live adoption deferred by owner |
| B-A6 | Both adapters test denied second write, safe retry and false-success readback rejection; assignment readback test | Native permission failure injection unknown |
| B-A7 | WSL cargo project checks and focused adapter tests | Native Windows GNU build initially blocked by missing dlltool.exe; macOS unknown |

WSL `cargo run -- selftest` reads the live GitHub board without writes and passes 31 golden traces. Mock adapter tests are separate from this native read and do not qualify live color/assignment adoption. No legacy Python suite remains after the v4 cutover on this branch.

Rules amended: R8/R13 implementation of the approved target, with sections 1, 5, 6 and 9 updated together. The existing `waiting_and_filters_hold_admission` assertion now expects `q-blocked` instead of `q-waiting` on a canonical board; no golden trace was changed or removed.

## Live adoption on alexkirs/taskq (#776, 2026-10-13)

Release [v4.0.11](https://github.com/alexkirs/taskq/releases/tag/v4.0.11) (tag at the #769 merge) ran on mac and win. Inventory before any change: legacy objects `priority-1`, `priority-2`, `q-waiting`; no `p1`, `p2`, `q-blocked`; open assignments `priority-1` #770 #612 #597, `priority-2` #652 #651 #650, `q-waiting` #781; closed assignments `priority-1` 239, `priority-2` 130, `q-waiting` 2; focus `all` on both hosts; no `label_schema`, so legacy was auto-detected. The owner approved the gate with #765 still running.

Steps: `label_schema: canonical` in each host's `taskq.json`, win first, then `taskq service restart` on win and mac. The service caches the schema per process, so the restart is required; mac re-executed with its live children, win had none. Then `taskq labels` and the reassignment of the six open priority labels. #781 had already left `waiting`, so no `q-waiting` reassignment was needed. This file's `taskq.json` pins `canonical`: the retained legacy objects would otherwise make a fresh checkout auto-detect legacy.

| Criterion | Live evidence | Unknown or mock only |
|---|---|---|
| B-A1 | Read back on GitHub: the names, descriptions and HEX of the fixed catalog and of used `host-win` match `src/labels.rs` | — |
| B-A2 | API HEX matches B2 for every catalog label | Native light/dark rendering was not inspected; GitLab unknown (no GitLab board) |
| B-A3 | A second `taskq labels` reported "label catalog readback verified" with the same metadata; unrelated labels (`bug`, `documentation`, `problem`, `taskq-events`, `run-any`, unused `host-*`, `area-*`) are unchanged | Fresh native setup was not run |
| B-A4 | After the change: `status p1` equals `status priority-1`; mac focus `all` still lists open labels; both hosts heartbeat v4.0.11 after the restart | — |
| B-A5 | Open: `p1` #770 #612 #597, `p2` #652 #651 #650, no legacy names. Closed counts unchanged (239/130/2). Legacy objects kept. Both services restarted canonical before the reassignment | — |
| B-A6 | A bad token on `taskq labels` gives HTTP 401, exit 1 and no success line | Native partial-failure injection: only the mocks from #769 |
| B-A7 | `cargo test --locked` and `cargo run -- selftest` (see PR) | Mocks are not native proof |

`run-any` on #770 is outside the catalog and is not an enabled runtime, so #770 stays ready but is not admitted. This was the same before the adoption; the label was left alone (B4).
