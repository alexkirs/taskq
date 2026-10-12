# Agents

Read [taskq.md](taskq.md) first: it holds every decision of this repo, product ones included (§ Product).
Spec first (R13): a change that alters a decision edits taskq.md first; a task that conflicts with one is an `ask`, not an edit.
Lifecycle changes follow the [executable behavioral model principle](taskq.md#executable-behavioral-model).
Project setup follows [shared bootstrap](taskq.md#shared-project-bootstrap-600): preserve existing files and ownership; confirm concrete external effects before applying them.
Checks: `cargo test --locked` (includes the § 2 table coverage and spec size tests) and `cargo run -- selftest`.
Done is defined in taskq.md § 7.
