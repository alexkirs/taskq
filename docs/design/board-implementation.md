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
