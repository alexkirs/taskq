# README page plan

Rebuild the project page so a visitor copies one phrase and leaves; the agent does the rest.
Decisions change in [taskq.md § Product](../../taskq.md#product) first (R13). Style: [Core](https://github.com/taskqorg/specq/blob/main/caveman/rules/core.md), [Documentation](https://github.com/taskqorg/specq/blob/main/caveman/rules/documentation.md); evidence: [F1/F5](https://github.com/taskqorg/specq/blob/main/design/implementation-fidelity.md); use-case method: [specq](https://github.com/taskqorg/specq/blob/main/engineering/use-cases.md).

## Baseline (observed 2026-10-14)

- README today: header, tagline, three steps, Mix agents, link, release verification, license, donate. Defects: step 2 says `Run taskq pm`, a command § 5 does not have; the compression sentence is a v3 remnant (#633); no screenshots; no use cases; the phrases were never checked against a fresh agent session.
- Screenshot pipeline exists: `taskq tray --snapshot DIR` writes icon PNGs and popover snapshots in both themes (§ 4) from `<install>/projects`; [tray-fixture.sh](tray-fixture.sh) is an edge-case fixture (errors, stale data, long titles), not a showcase; [evidence/4.1.9/manifest.json](evidence/4.1.9/manifest.json) is the pinning format.
- Setup entrances ([S1](setup-and-project-locations.md#s1-two-installation-entrances-one-setup-flow)): the agent phrase is the default route; a shell install command is not published until the endpoint exists. Nothing in setup writes a PM bootstrap into the owner's project; the "daily" phrase must be verified, not assumed.
- UI sources that change the pictures: `src/tray.rs`, `src/tray.html`, `src/icon.rs`, `docs/design/icon-summary.config.json`.

## Target page

Order, top to bottom. Everything else leaves the page.

1. **Header**: `docs/header.webp` with caption `Agents working.`, verbatim.
2. **Setup phrase**, first thing under the picture, one `text` code block so it is copied as one piece:
   ```text
   Install taskq from https://github.com/taskqorg/taskq and set it up for this project.
   ```
   One line under it: paste into Claude or Codex; the agent installs taskq, connects the board and becomes the manager. A second block for a new project: `… and create project <name> from <idea or folder>`.
3. **One sentence**: `Your repository board is the task list; your usual AI apps do the work.`
4. **What you get**: at most six bullets, one line each, shipped behavior only (R8, R12): tasks are issues on GitHub or GitLab; the service runs as many Claude or Codex workers as you set, per computer; a worker's question waits for your answer; review accepts or sends back, close publishes the PR; the tray menu shows what needs you, pauses and resumes, shows a day's history; several computers share one board.
5. **Screenshots**: dark theme, two popover images side by side (Home, Project) and the summary icon row, from `docs/shots/`.
6. **Use cases**: one table `You want | Say to the manager | What happens`; rows below.
7. **Footer**: `Agents: install, setup, commands and development are in taskq.md.`, the two release verification lines (checksum, `gh attestation verify`) verbatim, license, donate line verbatim.

Mix agents becomes a use-case row; the Steps section disappears.

## Screenshots: source, refresh and staleness

- `docs/shots/fixture.sh`: showcase data, two projects with plausible names and titles, states `doing`, `review`, `ask`, `ready`, one day of history, no errors, no stale files. Separate from the test fixture: the test fixture must keep its edge cases.
- `docs/shots/update.sh` (macOS): `cargo build --release`, run the fixture in a temp dir, `TASKQ_INSTALL=… taskq tray --snapshot`, copy `popover-dark-home-16.png`, `popover-dark-project-16.png` and `icon-summary-run-dark-*.png` into `docs/shots/`, write `docs/shots/manifest.json`: `version`, `sourceCommit`, `binarySha256`, `environment` (os, arch, renderer), per file sha256. Same shape as the evidence manifest (F5). Implemented (#923): `update.sh` uses `--locked`, refuses uncommitted UI sources, passes the Cargo version to the fixture, and also writes `uiSources` (git blob id per UI source at `sourceCommit`).
- Stale rule: shots are stale when a UI source (list above) changed since `manifest.sourceCommit`. `docs/shots/check.sh` runs in `tests.yml` (checkout `fetch-depth: 0`) and also checks that every image README references is in the manifest. Implemented (#923) as the `shots` job that `tests` needs. Deviation: `check.sh` compares `uiSources` blob ids with `HEAD`, not `git diff sourceCommit HEAD`: a squash merge drops `sourceCommit` from main, so the diff would fail on main after each merge. The icon file name carries this Mac's menu-bar size (`icon-summary-run-dark-34pt@2x.png`); the history range shows the capturing Mac's time zone. A PR that changes the UI without rerunning `update.sh` fails CI. This is the "new build, new pictures, new page" rule; the PR carries the pictures.
- Automatic generation (gated, Q1 below): once `taskq tray --snapshot` is proven on `macos-latest`, a macOS job runs `update.sh` on main when UI sources changed and commits the result. Until then `update.sh` runs on the owner's Mac in the dev loop.
- Dark only on the page (owner). The snapshot writes light too; `update.sh` copies dark. A `<picture>` light/dark switch is a later option, not now.
- Windows pictures: Not tested; add a Windows row only when a qualified capture path exists.

## Use cases on the page

Rule: a row is `situation · exact phrase · observable result · UC id`. Source: [use cases](../use-cases/README.md). Only cases whose path is shipped and covered by a golden trace or selftest qualify; design targets stay off the page. Each phrase is checked once in a fresh PM session before it is published; evidence in the task result.

| You want | Say | What happens | UC |
|---|---|---|---|
| a task done | `File a task: fix the login redirect.` | an issue with Goal and Acceptance, label `taskq`; a worker starts when a slot is free | UC-02 |
| it done first | `File it as p0.` | admitted before ordinary tasks, within limits | UC-04 |
| a plan broken up | `Split this plan into tasks.` | tasks linked with `Depends on #N`, independent ones run in parallel | UC-07 |
| to answer a worker | menu → Waiting for answer → pick; or `Answer #12 with 2.` | the answer lands on the issue, the worker continues | UC-09 |
| to accept or reject | `Review #12.` | accept closes and publishes the PR; reject sends it back with the reason | UC-14 |
| a quiet day | tray Pause, or `Pause the service.` | running attempts finish, nothing new starts | UC-13 |
| another computer | `Install taskq on this host too.` | a second host works the same board under its own limits | UC-12 |
| a specific agent | `Use Codex for the visuals.` | the task gets `run-codex`; the rest stays as configured | UC-02 |

Rows the check rejects are dropped, not softened.

## taskq.md changes (before the README)

Replace the four Product lines `README is the whole page…`, `Tagline…`, `Steps…`, `Setup phrase…` with four:

- README is the whole page: header, setup phrase, one sentence, what you get, screenshots, use cases, one link to taskq.md, release checksum and provenance verification, license, donate.
- Setup phrase: one `text` block, verbatim `Install taskq from https://github.com/taskqorg/taskq and set it up for this project.`; new-project variant beside it.
- What you get and use cases list shipped behavior only; each use-case row names its UC id and a phrase checked in a fresh PM session.
- Screenshots: dark theme from `taskq tray --snapshot` over `docs/shots/fixture.sh`; `docs/shots/manifest.json` pins version, commit and hashes; stale when a UI source changed since that commit (CI check).

Net change 0 lines; the contract is at its 300-line limit, so each new line replaces one.

## Tasks

| # | Task | Depends on | Host | Acceptance |
|---|---|---|---|---|
| A #923 | Shots pipeline: `fixture.sh`, `update.sh`, `manifest.json`, `check.sh` in `tests.yml`; first dark set committed | none | mac | A1 `update.sh` writes the three dark files and the manifest. A2 `check.sh` fails on a UI change without refresh and passes after `update.sh`. A3 CI green on the PR. |
| B #925 | Product lines in taskq.md, then the README per Target page; phrases checked | A | any | B1 taskq.md Product changed first, ≤ 300 lines. B2 README order matches Target page. B3 each use-case phrase checked in a fresh session, result lists kept and dropped rows. B4 `cargo test --locked` green. |
| C #924 | Q1: `taskq tray --snapshot` on `macos-latest` by `workflow_dispatch`; if pictures render, move `update.sh` into CI with a bot commit on main | A | any | C1 workflow artifact holds the PNGs or the failure is recorded. C2 only on success: a UI change on main refreshes `docs/shots/` without a human. |

## Owner decisions

- O1 Setup phrase wording: one phrase plus the new-project variant (recommended), or one phrase only.
- O2 Phrase check scope: Claude and Codex (recommended), or Claude only.
- O3 Shell install command: stays off the page until S1's endpoint is qualified (recommended, no decision needed).
