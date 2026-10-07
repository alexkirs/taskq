# CLI gate and conditional Pages qualification (#195)

Implementation candidates require normal PM review before publication. This document separates
CLI eligibility, build evidence, and deployed site freshness; a successful CLI gate does not
mean Pages is healthy. No production settings change or deployment is part of the worker handoff.

## Accepted source

- [Exact Wiki design](https://github.com/alexkirs/taskq/wiki/Home/18038edc2ae72235d7f87a139919f30df09a441a#cli-update-gate-and-conditional-pages-publication-195): revision `18038edc2ae72235d7f87a139919f30df09a441a`.
- [PM acceptance](https://github.com/alexkirs/taskq/issues/195#issuecomment-6044106442), 2026-10-07T18:20:30Z: implement as two independently reviewable steps, preserving the issue acceptance and publication review.
- [Candidate review](https://github.com/alexkirs/taskq/pull/196). Commit 1 changes CLI gating; commit 2 changes conditional Pages. No unreviewed merge.

## Stage 1: trusted CLI eligibility

The active `tests.yml` workflow ID and path are resolved from GitHub. A repository `push`
run must match the exact target SHA and that identity, complete successfully, and have a
completed successful `tests` check from GitHub Actions (app ID 15368) in its suite. Both API
inventories are paginated. Latest checks within a suite and latest trusted push runs handle
reruns; older superseded tests cannot replace a pending or failing current attempt. Unrelated
same-name checks cannot satisfy mandatory tests, and still keep their existing gate.
Trusted PR tests for that SHA also require completed success, including their workflow run
and tests check; skipped/neutral PR tests do not become optional through the general check policy.

Missing, unreadable, pending, failed, skipped and neutral mandatory tests refuse. Unknown
non-Pages checks retain their previous completed-success/skipped/neutral policy. Only GitHub
Actions checks in an exact-SHA repository run with the known legacy Pages path/event or the
new Pages workflow path/event are excluded. Signing and clone startup rollback are unchanged.
Tests still run on PRs and main, with exact-head push tests added on review branches.

| Receipt | Immutable SHA / identity | UTC times and result |
| --- | --- | --- |
| Historical CLI-only main baseline | `98556ce3e542ac055fab9da34678f8b509d46ce9`; tests [37611862981](https://github.com/alexkirs/taskq/actions/runs/37611862981), legacy Pages [37611862867](https://github.com/alexkirs/taskq/actions/runs/37611862867) | 2026-10-07: tests 11:06:37-11:06:55, Pages 11:06:37-11:07:26, both success. Git changes only TaskQ modules/contracts/tests, outside site inputs. |
| Historical site-input main baseline | `e129d8ffe66255f7a86352589520ff585bc97507`; tests run [37663355416](https://github.com/alexkirs/taskq/actions/runs/37663355416), workflow 376614289, suite 102032751137 | Tests 17:59:33-17:59:52 on 2026-10-07, success. Legacy Pages run [37663354929](https://github.com/alexkirs/taskq/actions/runs/37663354929), suite 102032749361, 17:59:32-18:00:25, success. Old gate waited for all checks. |
| Historical site-input main baseline | `7ca51bacc7898cab93b3ee13de9ae3eb96de7025`; tests [37665660185](https://github.com/alexkirs/taskq/actions/runs/37665660185), legacy Pages [37665658055](https://github.com/alexkirs/taskq/actions/runs/37665658055) | 2026-10-07: tests 18:17:16-18:17:34, success; Pages 18:17:15-18:20:08, success. Pages API reports built for this SHA. |
| Stage 1 CLI-only review candidate | `b2e73ed3273547926f93cbcb02867bca48c5ba16`; push tests [37666768922](https://github.com/alexkirs/taskq/actions/runs/37666768922), PR tests [37666809727](https://github.com/alexkirs/taskq/actions/runs/37666809727) | 2026-10-07: push 18:25:53-18:26:13, PR 18:26:12-18:26:31; both success. Live gate accepted exact SHA. Local full suite: 196 tests, OK, 20.439 seconds. |
| Stage 1 isolated installed readback | Editable installation from a separate exact-SHA clone of `b2e73ed3273547926f93cbcb02867bca48c5ba16` in a disposable virtual environment | Fresh `taskq --version` prints `taskq b2e73ed`; imported package and install discovery resolve to that clone; Git full SHA agrees. Main installation untouched. |

Negative controls live in `tests/test_update_gate.py`. Existing real-Git signing, updater
refusal and startup rollback tests remain in `tests/test_taskq.py`. The #190 sandbox result
**FAILED: 188 tests / 49 errors** remains a historical failure, not repaired by later CI.
The original 26-99 second savings estimate remains an estimate. Candidate branch runs are
not a controlled before/after production savings measurement.

## Stage 2: publication inputs and migration

The repository is currently configured for **legacy main /docs**. Editing a managed workflow
or adding path filters cannot change that mechanism. After candidate review, the publisher
must switch the Pages publishing source to GitHub Actions, preserving the `github-pages`
environment and HTTPS, then perform a manual full main build. Do not change the source before
the reviewed workflow is on main. `configure-pages` does not request automatic enablement.

The candidate uses the official [Jekyll Pages action](https://github.com/actions/jekyll-build-pages)
with `source: ./docs`, the same site root. Source, Jekyll configuration and site dependencies
are inside `docs/**`; real generator/action configuration is in `.github/workflows/pages.yml`
and the input detector is `.github/scripts/pages_inputs.py`. Those are the complete current
input set, not root CLI README/package/test files. Future generator changes must update it
if they introduce dependencies outside those paths. Wiki is a separate Git repository and
has no event in this workflow.

The workflow compares the whole push range, or PR base to tested merge SHA, using Git rather
than GitHub's capped path-filter list. Deletions and both sides of renames count. All commits
in a multi-commit push are considered. Unknown/pre-migration/initial baselines and manual
runs build fully. Unchanged inputs produce an explicit summary and skip build/deploy.
Review branches and PRs may build artifacts but cannot deploy. Only main may deploy.
Read-only input/build jobs and deploy-only `pages: write`/`id-token: write` permissions preserve
the minimum Pages permission boundary. No timer, profile, multiproject or user permission change.

A successful Jekyll artifact includes `taskq-revision.txt` containing the built exact SHA;
this is a freshness receipt, not proof of deployment. After reviewed publication, verify
that receipt over HTTPS, the deployed Pages API SHA/status and workflow conclusion, rendered
existing URLs/content, and the session bridge. A failed deploy or stale receipt is a site
qualification failure even when CLI eligibility succeeds.

`tests/test_pages_inputs.py` uses real Git commits to cover CLI-only, manual, initial,
unknown/pre-migration baseline, source/config/dependencies/workflow/detector changes, deletion,
rename outside docs, multi-commit pushes and more than 300 changed files. `tests/test_open_bridge.py`
runs the unchanged JS in Node without navigating to an application: schemes, UUID shape,
encoded hash, invalid links and fallback are checked. Node must be available for this receipt;
a skipped bridge test is not qualification.

Baseline artifact `11501762939` from legacy run `37663354929`, live HTTPS `open.html`, and source
all have SHA-256 `7c4ee5d6b6a98a0c6f19af05daff1d7b69c249fcd4d63af3eaf53e06c31b71b3`.
No change to scheme/UUID/hash/fallback or `no-referrer` was made.

Stage 2 first candidate `618f5373053161607f7f90e5c94aea11bfdc7264` failed its push
Pages run [37667347832](https://github.com/alexkirs/taskq/actions/runs/37667347832),
2026-10-07T18:30:22Z-18:30:58Z. Jekyll succeeded, but its Docker-created `_site` was not
writable by the runner, so adding the revision receipt afterwards failed with permission
denied. The revision is now written into the writable source before Jekyll copies it;
no permission expansion is needed. This failed build remains a failure receipt, not healthy
publication or measured savings.

Stage 2 repaired candidate `0fc99ab19f5eaaa99e46c69f8587bb85d4172ae0` passed
push Pages run [37667556755](https://github.com/alexkirs/taskq/actions/runs/37667556755),
workflow 377716641, suite 102044783796, 2026-10-07T18:32:00Z-18:32:34Z;
PR Pages run [37667564277](https://github.com/alexkirs/taskq/actions/runs/37667564277)
passed at 18:32:03Z-18:32:42Z. Push tests [37667556726](https://github.com/alexkirs/taskq/actions/runs/37667556726)
and PR tests [37667564226](https://github.com/alexkirs/taskq/actions/runs/37667564226)
also succeeded. Deploy skipped on both review runs. Artifact `11503173070` contains the exact
built SHA in `taskq-revision.txt`. Comparison against legacy artifact `11501762939` found all
previous URLs present; existing rendered HTML differs only in the CSS revision query,
while CSS, source Markdown, header and `open.html` bytes are identical. Added files are the
qualification page, revision receipt and the already reviewed Wiki-process source from main.
This is build compatibility evidence, not deployment freshness. The matching failed PR
receipt is run [37667356335](https://github.com/alexkirs/taskq/actions/runs/37667356335),
job 112949919577, permission denied at 2026-10-07T18:31:03Z on the first candidate.

## Publication qualification still required

Before closing qualification, collect an approved main CLI-only push and a site-input push
with immutable SHAs, exact tests/Pages workflow identities, timestamps, build/skipped reason,
installed fresh-process readback, deployed revision/content and bridge controls. Compare like
for like to the historical baseline; do not call branch CI durations measured production savings.
The initial manual full build must succeed. Candidate artifacts alone do not prove site freshness.

## Rollback

Revert the two staged implementation commits through the normal review flow to restore the
old CLI gate. For Pages, restore legacy publishing from main `/docs`, request a full build,
and verify its successful deployment and existing URLs/session bridge. The source remains
Jekyll-compatible. A settings rollback alone does not prove a working publication; record the
built SHA, completion time and HTTPS content. Remove the stale revision receipt expectation
when returning to legacy publication.
