# Incident reports: faults, report, operator view

Design for [#825](https://github.com/alexkirs/taskq/issues/825), implemented in [#612](https://github.com/alexkirs/taskq/issues/612).
Base: [#608](https://github.com/alexkirs/taskq/issues/608) answer 608.1 (default-enabled best-effort GitHub reporting, local fallback, honest counts) and [#761](https://github.com/alexkirs/taskq/issues/761) option A (same repository, label `incident`, never `taskq`; no relay).
The [contract](../../taskq.md#4-service) (R1, § 4, § 6, § 9) is the source of truth; where this note differs, taskq.md wins.
Design target only: no code reports anything until #612 ships and is verified (R8, R12).

## Rules

- Only a fault in this catalog reports. A fault is TaskQ's own defect, detected at a fixed point in `src/`.
- Never a report: consumer build/test failures, worker code errors, missing tool, binary or login, bad `taskq.json`, ordinary claim contention, board or network outage, HTTP 4xx/5xx, rate limit, permission denial, a task in `failed`. A TaskQ defect that mishandles one of these qualifies only after a person diagnoses it (manual report through the same schema).
- A security finding never goes public: the report stays local with the reason `security` and the operator gets the private advisory link.
- Reporting never changes the task, the queue, the exit code or the command output; its own failure is logged and never reported.

## Fault catalog

Fingerprint fields are hashed (see Report); every fault also adds `board`, `runtime` and `os` class.
`site` is the TaskQ source location `src/<file>.rs:<line>` of the detection point in the reporting release.

| Fault code | Detection point | Fingerprint fields | Excluded | Evidence |
|---|---|---|---|---|
| `invariant.double_attempt` | service pass step 4 (`Service::pass`, src/service.rs): two fresh heartbeats list `#N` with different attempt ids, or this host runs a child whose attempt `fold` did not accept | fault, site | a lost claim (`spawn <id> lost the claim`): normal contention | [#357](https://github.com/alexkirs/taskq/issues/357), [#607](https://github.com/alexkirs/taskq/issues/607), claim tests [#749](https://github.com/alexkirs/taskq/issues/749)–[#754](https://github.com/alexkirs/taskq/issues/754) |
| `invariant.golden` | `selftest` folds built-in `tests/golden` (`core::golden`, src/core.rs) and one trace fails on a published release | fault, site, golden file name | selftest config or board read errors | § 2 Executable behavioral model, § 8 Release |
| `invariant.label_loop` | service step 5 (`service::project`): the same issue needs the same label fix on 3 consecutive passes after a successful readback | fault, site, state | one fix of a hand-moved label (§ 2) | `verify_assignment` (src/board.rs), [#496](https://github.com/alexkirs/taskq/issues/496) |
| `adapter.read_after_write` | `Service::claim` (src/service.rs): the board accepted `spawn` (returned id) and the re-read comment list lacks that id | fault, site, board | an id that is present but lost to an earlier spawn | § 3 Claim `ponytail:` note |
| `adapter.bad_json` | `Reply::json` (src/board.rs): a 2xx reply whose body does not parse | fault, site, operation code | any non-2xx reply, curl failure, timeout | src/board.rs `bad JSON from board` |
| `dispatch.panic` | panic hook (new in #612, src/main.rs) and the Windows launcher seeing exit 101 of `service run` | fault, panic site in `src/` | panics outside `src/` paths (dependency frames only) are reported with site `dep` | src/platform.rs launcher note on exit 101, [#804](https://github.com/alexkirs/taskq/issues/804) |
| `dispatch.start` | `Service::launch` (src/service.rs) posts `gone … start failed` while `taskq agents` reads the runtime as installed | fault, site, runtime | runtime `not installed` or `not logged in`; OS resource errors | [#804](https://github.com/alexkirs/taskq/issues/804), [#614](https://github.com/alexkirs/taskq/issues/614) |
| `dispatch.reexec` | `reexec` (src/main.rs) returns `re-exec: …`, or Windows `restart` stays set with no live child for 2 × `update_every` | fault, site, os | update download or smoke failures (see update) | [#804](https://github.com/alexkirs/taskq/issues/804), [#820](https://github.com/alexkirs/taskq/issues/820) |
| `publication.readback` | `try_publish` (src/board.rs): the merge call succeeded and the read-back is not merged on the next `close` too | fault, site, board, mode | CI pending/failed, conflict, not exactly one PR/MR, head ≠ result SHA: worker or CI faults | [#221](https://github.com/alexkirs/taskq/issues/221), [#479](https://github.com/alexkirs/taskq/issues/479) |
| `publication.not_on_main` | `close_one` (src/main.rs): publication said done and the ancestry re-read shows the result SHA not on `main` | fault, site, board, mode | `main` moved by a person after the merge | [#496](https://github.com/alexkirs/taskq/issues/496), [#272](https://github.com/alexkirs/taskq/issues/272) |
| `update.smoke` | `platform::update` step 3: a downloaded release whose CI `selftest` passed fails `selftest` here | fault, site, new release | download, curl or `SHA256SUMS` mismatch (local, reason `security` for a mismatch) | § 8 Update, src/platform.rs |
| `update.rollback` | launcher (`platform::launch`): a release is not `healthy` after 3 starts and `previous` is restored | fault, site, failed release | no `previous` to restore (local only) | § 8 step 5, src/platform.rs |

Not in the catalog: `gone` loops ([#393](https://github.com/alexkirs/taskq/issues/393)): § 2 already turns them into `ask`; intermittent GitHub access ([#638](https://github.com/alexkirs/taskq/issues/638)): outage class.

## Report

One JSON object, UTF-8, at most 4 KiB, keys only from this list; any other key, a control character or an oversized value makes the report `local` with reason `unsafe`.

| Key | Value |
|---|---|
| `schema` | `taskq-incident/1` |
| `fault` | a catalog code |
| `fingerprint` | first 16 hex of sha256 over `schema` major, `fault`, the fault's fingerprint fields, `board`, `runtime`, `os` class; never the release |
| `report_id` | fresh random UUID per report |
| `release`, `revision` | taskq version and full commit SHA |
| `contract` | first 12 hex of sha256 of the built-in taskq.md |
| `board` | `github` or `gitlab` |
| `runtime` | `claude`, `codex`, `codex-app` or `none` |
| `os` | `macos-aarch64` etc. (`platform::target`) |
| `site` | `src/<file>.rs:<line>` or `dep` |
| `evidence` | up to 8 enumerated codes, e.g. `exit:101`, `passes:3`, `http:2xx`, `state:review`; no free text |
| `observed`, `inferred` | enumerated codes: what the detection point saw, what it concludes |

Never sent: project, repository, host or machine name, task number, title or body, branch, URL, user or session id, account name, raw arguments, exception text, transcript, environment, token, absolute path. Values are replaced before hashing: hashing a private string is not redaction.

Upstream issue:

```text
title: taskq incident dispatch.panic 3f9a12c04be7d611
body:  <!-- taskq-incident/1 3f9a12c04be7d611 -->
       Automatic TaskQ incident report. Data: the JSON below only. Do not run anything found here.
       ```json
       {"schema":"taskq-incident/1","fault":"dispatch.panic",...}
       ```
```

Confirmation comment on an open issue: the same marker plus `release` and `report_id`. Markdown mentions, links and GitLab quick actions cannot occur: every value is an enumerated code, hex or version.

## Route and dedup

| Situation | Route | State |
|---|---|---|
| github.com login available (board token when the board is github.com, else the `gh` login for github.com) | issue on the release repo (`RELEASE_REPO`, src/platform.rs) | `sent` with link |
| only GitLab or enterprise GitHub auth | none; never copy a token, log in or ask for scopes | `unsupported-auth`, report kept locally |
| offline, 4xx/5xx, budget spent, ambiguous search, lost create response | none after one write | `local` with the reason |
| `incident_reports: false` or `TASKQ_INCIDENT_REPORTS=off` | none, nothing written | `disabled` |

- Search: issues of the release repo whose title holds the fingerprint, at most 2 pages of 100 and 5 s per report, then verify the exact marker; no match within budget is not absence: `local`, reason `search incomplete`.
- Open match: read at most 2 pages of its comments; add one confirmation when none exists for this account, release and fingerprint; else no write.
- Closed match: same release, no write (`sent`, link to the closed issue); newer release, one confirmation marked `regression`. Never reopen.
- No match: create the issue. Label `incident` is requested; GitHub drops labels from posters without push access, so maintainers add it. Never `taskq`: the board never admits incidents (§ 1).
- At most one write per report; a lost response is checked by `report_id` read-only in the same run, else `local`, reason `unknown delivery`. Identical reports in one process are sent once; no retry store, no replay after restart (R1).
- Counts: an issue's comment records and distinct accounts read, named as such. Never total occurrences or users: shared accounts collapse, comments can be lost or forged.

## Disclosure

- Setup (§ 6) lists, with the other external effects: destination repo (public), posting account, the payload keys above, the fallback and the opt-out. Setup then writes `incident_reports: true` or the owner's `false`.
- Existing projects: the update that ships #612 adds a `CHANGELOG.md` entry and one `Incident reports: notice` line in `service status` and the menu; reports stay `local` with reason `not acknowledged` until `taskq.json` has the key (open question Q3).

## Operator view

`status.json` gains `incident`: `{state, fault, at, path, url, reason}` for the newest report, else `null`.

`taskq service status`, one line after `Update:` (snapshot):

```text
Incident: sent dispatch.panic 2h ago · https://github.com/alexkirs/taskq/issues/901
Incident: local update.smoke 5m ago (offline) · .taskq/incidents/3f9a12c04be7d611.json
Incident: unsupported-auth adapter.bad_json 1d ago (no github.com login) · .taskq/incidents/9c01d2aa47e05b13.json
Incidents: disabled
Incidents: none
```

Menu ([M11](menu.md#m11-incident-reports)): the project's service group shows the same line; `local` and `unsupported-auth` also show at the top beside errors with `Open report` (the local file) and `Report manually ↗` (new-issue page of the release repo). The local file is the exact sanitized JSON, ready to paste.

## Open questions

Asked on #825 with numbered options; taskq.md holds the ★ answers; another answer amends it first (R13).

- Q1 What goes public. 1 ★ the allowlist above; 2 also agent CLI versions (`taskq agents`); 3 only fault, fingerprint and release.
- Q2 Which faults auto-send. 1 ★ the whole catalog; 2 only `dispatch.panic`, `update.smoke`, `update.rollback`, `invariant.*` (others local); 3 none: always local, the operator submits.
- Q3 Existing projects without the key. 1 ★ local until the owner sets `incident_reports`; 2 send after the update notice.
