# TaskQ test strategy audit

## Factual state

TaskQ previously told every code or docs worker to run
`python3 -m unittest discover -s tests` before `result`. The changed brief now
requires focused tests for the changed behavior, and a local full suite for
shared TaskQ command, queue, runtime, publication, permission, cleanup,
update, or test-infrastructure behavior, or when focused evidence cannot
cover the risk. Docs-only or planning work does not run behavior tests when
they cannot verify the change. The manager review then runs focused tests, and
the exact candidate SHA must pass the GitHub Actions `tests` workflow before
publication. That workflow runs on a `taskq-*` branch push, pull request, and
`main` push. Pages is a separate workflow: its input job runs for the same
events, but a deployment is not a CLI test gate and is explicitly excluded from
update eligibility.

The relevant protections are different work, not interchangeable waits:

| Step | Evidence it provides | Not measured as test execution |
| --- | --- | --- |
| Worker focused test | changed behaviour while editing | CI queue and runner delay |
| Exact-SHA CI tests | repository-wide regression check for the candidate | Pages jobs and deployment |
| Manager review | acceptance, diff and ownership check | independent reviewer time |
| Tool/CI wait | availability or external completion | test coverage |

## Measurements

Existing independent logs, both on 2026-10-08, record:

| Log | Test cases reported | Elapsed test execution |
| --- | ---: | ---: |
| `taskq-221-620d020-independent-full.log` | 376 | 106.023 s |
| `taskq-226-84594f7-independent-full.log` | 372 | 107.990 s |
| `taskq-221-620d020-independent-matrix.log` | 79 | 38.439 s |

The two full runs establish a practical local cost of about 106–108 seconds
per run. They do not establish a coverage delta: they ran different revisions.
The 79-case matrix is a selected execution, not evidence that it covers the
other cases or that its cases were newly added.

Current discovery loads 376 cases with 376 unique test ids. That tells us only
that this committed discovery has no duplicate ids; it is not a coverage
measure.

The cited historical 79-case matrix must not be described as 79 unique
regressions. Static inspection of
`/tmp/taskq-221-current-independent-matrix.py` finds 66 executions of 19
underlying test-method implementations. The later
`/tmp/taskq-221-19b4587-last-read-probe.py` adds `LastRead`: 79 executions of
20 implementations. `unittest.main()` collects the imported `Review` class as
well as every subclass. Each subclass rediscovers its inherited `Review` and,
where applicable, `Boundaries` baseline methods. `tests/test_review.py` itself
reuses setup and helpers from `test_taskq.Cycle` by assignment rather than
subclassing it. Shared fixtures and repeated setup further mean case count is
not a measure of independent behavioural coverage.

No flakiness rate was measured. Resource warnings in both full logs are
observations, not evidence that a test is flaky or redundant.

## What the tests qualify

Most TaskQ cases are unit/fixture integration: an in-memory GitLab/GitHub
store, fake app/server responses, and disposable files. They provide fast
coverage of queue state, claim ownership, idempotency and fail-closed runtime
logic without a live tracker or worker. `test_review.py` is stronger local
integration: it creates a disposable Git repository and bare remote, then
checks exact-SHA publication, stale-review refusal and retry safety.
`test_pages_inputs.py` uses real disposable Git histories for input detection.
`test_open_bridge.py` invokes a Node harness. These are subprocess integration
checks, still not production qualification.

`selftest --scope quick` and `--scope full` are the genuine runtime/end-to-end
qualifications: they operate through a tracker and, for full, start real
worker sessions. Their unit tests simulate those paths; they do not qualify
live credentials, app transport or a production enrollment. The inspected
probes caught maintenance-critical regressions rather than mere count: close
retry/retirement after interruption, stale or missing review branches,
preserved review evidence, reopening during final reads, terminal/working
contradictions, unknown runtime status, permission-wait deduplication, and
failure to broaden launch permissions. No test is proposed for deletion.

## CI timing

GitHub job timestamps separate workflow dispatch, runner/setup, and the
unittest step. They do not expose queue time before `created_at`, human review,
or Pages elapsed time; those are therefore not included as test execution.

| Candidate | Event | Run created → job start | Job start → test step start | Test step | Test step → job complete |
| --- | --- | ---: | ---: | ---: | ---: |
| #221 branch `37744887911` | push | 3 s | 3 s | 90 s | 3 s |
| #221 PR `37745523762` | pull request | 4 s | 2 s | 98 s | 2 s |
| #221 main `37745809420` | push | 3 s | 2 s | 80 s | 3 s |
| #226 branch `37743359919` | push | 3 s | 2 s | 94 s | 2 s |
| #226 PR `37743731491` | pull request | 38 s | 4 s | 97 s | 3 s |
| #226 main `37744148819` | push | 3 s | 3 s | 92 s | 4 s |

Automatic duplicate branch/PR execution may be worth a later CI design review,
but this task makes no CI scope change. Pages and independent human review are
separate controls with timing not measured here.

## Fast gate and exact candidate policy

Use the narrowest existing test module that covers the changed module, then
the final policy below. Examples:

| Changed area | Focused command |
| --- | --- |
| queue/worker/review publication | `python3 -m unittest discover -s tests -p test_taskq.py`; `python3 -m unittest discover -s tests -p test_review.py` |
| runtime or permission observation | `python3 -m unittest discover -s tests -p test_runtime_observation.py`; `python3 -m unittest discover -s tests -p test_permission_qualification.py` |
| Pages input detector | `python3 -m unittest discover -s tests -p test_pages_inputs.py` |
| docs-only/planning | no behavior test if it cannot verify the edit |

For every candidate, run focused tests before `result`. Also run the local full
suite for the listed shared/safety areas or insufficient focused evidence.
Review the exact SHA and acceptance, run focused review checks, and require the
exact-SHA CI `tests` success before publication. Pages, CI queue/setup, tool
waits and independent review remain separate evidence.

Expected local saving is one avoided 106–108 second full run for a narrow task
that has a focused test. It is an estimate from two independent full runs, not
a case-count calculation. It is not a CI saving and it does not reduce review
or Pages elapsed time. A change in any listed shared/safety area still runs the
full suite locally; unknown flakiness remains unknown.
