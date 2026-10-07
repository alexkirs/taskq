# Multiproject read-only observation (#186, stage 2)

Specification: [accepted Wiki 9ffd02dd](https://github.com/alexkirs/taskq/wiki/Multiproject-read-only-preparation/9ffd02dd07dbff81da333089613642c883bbcf6f). This stage observes; it never acts.

```
python -m taskq.multiproject --manifest FILE --json
```

Prints one aggregate: per project its status, blockers and the project's existing #191 v1 report (`taskq/contracts/pm-report-v1.md`). Exit status 1 when any project is not `ok`. Without `--json` it prints each report the way `tick` renders it.

## Manifest

A TOML file the user writes on each machine. It lists only the projects chosen for observation. Folders are never auto-discovered.

```toml
version = 1

[[project]]
provider = "github"            # "github" or "gitlab"
host = "github.com"            # optional; github.com / gitlab.com by default
repository_id = 123456789      # the tracker's numeric id: GitHub `gh api repos/OWNER/REPO --jq .id`, GitLab project id
repository = "owner/repo"      # canonical path, as in [github] repo / [gitlab] project of the project's taskq.toml
board = "repo"                 # the board name the project's taskq.toml resolves to
checkout = "/abs/path/to/main/checkout"  # this machine's binding: the main checkout, not a worktree
timeout = 60                   # optional; seconds for this project's reader, 1..600
view = { filter = "labels=area-x", mine = false, limits = { codex = 2 } }  # optional; the same meaning as tick's flags
```

`view` only narrows what the report shows. It confers no ACL and no execution authority.

## Checks before any queue read

The whole manifest is refused if `version` is not 1 or it has no entries (50 at most). An entry is refused, and its reader never starts, if:

- a field has the wrong type;
- another entry has the same stable identity (provider, host, `repository_id`), the same repository, or the same checkout.

Inside the entry's reader, before the queue is read, the project is refused if any of these fails:

- the checkout has a `taskq.toml` and is a main checkout;
- `origin` points to `host`/`repository`;
- the project's `taskq.toml` names the same provider, host, repository and board;
- the tracker's repository id equals `repository_id`.

A refused project is reported with its blockers. It never counts as empty work.

## Reading

Each entry gets exactly one attempt per invocation, in manifest order. The attempt runs `python -P -m taskq.multiproject --observe` as its own subprocess. Its cwd is the verified checkout and it configures that project's `taskq.toml`. The subprocess has its own process group. When the timeout passes, the whole group is killed. That is safe only because the reader admits no mutation and launches no worker. The reader output is capped at 1 MiB.

The reader uses only existing read paths: `profile` (queue and effective profile), the board lookup, `claude agents`, `liveness`, `report_row` and `validate_report`. It never calls `tick` (even plain `tick` releases and unlocks), `update`, `take`, `spawn`, `release`, `cleanup`, board moves or timers. It writes no tracker state, configuration, profile or permission.

## Results

| status | meaning |
|---|---|
| `ok` | valid fresh v1 report, outcome `ok` |
| `blocked` | report present, but v1 validation, repository match or outcome failed: e.g. stale `observed_at`, source unavailable, missing session link |
| `refused` | manifest or identity check failed |
| `timeout` | reader killed at its timeout |
| `failed` | reader did not start or exited non-zero |
| `malformed` | reader output is not a v1 report |

The parent revalidates every report with `validate_report`, so stale or invalid data stays visible. Each report keeps its repository, board, profile, outcome, refusals, task/session/commit links, `event_at`, `observed_at` and unknown/unavailable values as the reader produced them. One project's failure never stops the next project.

`received_applied` is always `unknown`. Delivery, receipt and application stay with `taskq report-verify` and a supported channel's readback.

## Not here

The manifest is the only state; there is no database, queue, scheduler or receipt store. Removing an entry stops future reads and leaves the tracker's claims and ownership as they are. Single-project `tick` is unchanged.

There is no acting pass, timer, extra PM or live project activation. Acting multiproject passes are a later stage that needs #208, and parent #186 still depends on #185, #176 and #177. Fixture tests prove behavior against fakes only. They do not qualify live transport, approval or multiproject readiness.
