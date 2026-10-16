# Agents

Shared rules: [specq](https://github.com/taskqorg/specq) is binding for every task here ([principles](https://github.com/taskqorg/specq/blob/main/engineering/principles.md), [texts](https://github.com/taskqorg/specq/tree/main/caveman), [engineering](https://github.com/taskqorg/specq/tree/main/engineering)); this repo keeps only taskq's own decisions.
Read [taskq.md](taskq.md) first: it holds every decision of this repo, product ones included (§ Product).
Spec first (R13): a change that alters a decision edits taskq.md first; a task that conflicts with one is an `ask`, not an edit.
Lifecycle changes follow the [executable behavioral model principle](taskq.md#executable-behavioral-model).
Project setup follows [shared bootstrap](taskq.md#shared-project-bootstrap-600): preserve existing files and ownership; confirm concrete external effects before applying them.
Checks: `python3 scripts/check.py`; selection and budgets: [docs/testing.md](docs/testing.md).
Done is defined in taskq.md § 7.
