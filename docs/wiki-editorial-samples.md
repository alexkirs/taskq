# Wiki editorial samples (#205)

Draft for user review. Not published. Sources and versions: [inventory](wiki-editorial-inventory.md).
Rule under test: action -> command -> result; material blocker, risk or unknown; normal spaces.

## Home

Before (`d3cc01c`, 45 lines): navigation, then four full specifications (#195, #191, #186, #187). The #195 status is stale.

After:

```markdown
taskq: a task queue in GitHub or GitLab Issues, worked by Claude Code and Codex agents. Start with the [README](https://github.com/alexkirs/taskq#readme).

- [Required settings](https://github.com/alexkirs/taskq/wiki/Required-settings): what workers need on each machine.
- [Known issues](https://github.com/alexkirs/taskq/wiki/Known-issues): what `taskq doctor` cannot fix.
- [Writing rule](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/taskq.md#writing): docs, briefs and reports.

## Specifications

| Specification | Status (2026-10-08) | Text |
|---|---|---|
| [#195](https://github.com/alexkirs/taskq/issues/195) CLI gate and conditional Pages | Implemented, closed | [Wiki 18038ed](https://github.com/alexkirs/taskq/wiki/Home/18038edc2ae72235d7f87a139919f30df09a441a), [qualification](https://github.com/alexkirs/taskq/blob/main/docs/pages-gate-qualification.md) |
| [#191](https://github.com/alexkirs/taskq/issues/191) PM tick report contract | Accepted, in review | [Wiki 05cf6aa](https://github.com/alexkirs/taskq/wiki/Home/05cf6aab1c6ed5fc9589b9e4673365cec34c58e6) |
| [#186](https://github.com/alexkirs/taskq/issues/186) Multiproject TICK path | Accepted, later | [Wiki 6b9027b](https://github.com/alexkirs/taskq/wiki/Home/6b9027b29bf43d18a24d2cc96ef98d314f396794) |
| [#187](https://github.com/alexkirs/taskq/issues/187) Onboarding and two-level help | Proposed, later | [Wiki d3cc01c](https://github.com/alexkirs/taskq/wiki/Home/d3cc01c131d9d85b565d7c88f81757c03caa6bbb) |
```

Risk: the `#writing` anchor exists only after the contract change. Publish Home after it.

## Required settings: #187 addition

Before (`d3cc01c`, lines 3-32): example output, 10-row scenario table, proposed help outline, acceptance criteria.

After:

```markdown
## Short start and help

Help today: `taskq --help`, `taskq <command> --help`, `taskq contract`.
A short start and scenario help are proposed in [#187](https://github.com/alexkirs/taskq/issues/187) ([draft](https://github.com/alexkirs/taskq/wiki/Required-settings/d3cc01c131d9d85b565d7c88f81757c03caa6bbb)).
```

## Required settings: PM notes (#188, #185, #186)

Before (lines 46-48, under "Codex workers"): three long bullets; two are proposals.

After:

```markdown
## PM

- **Local-command ACK before worker launch.** The PM runs `taskq preflight --json` in the main checkout and returns stdout, stderr, exit code and time. It proves local execution only ([#177](https://github.com/alexkirs/taskq/issues/177)).
- **Separate TICK sender, one PM for several projects:** proposed, not qualified ([#185](https://github.com/alexkirs/taskq/issues/185), [#186](https://github.com/alexkirs/taskq/issues/186)). Guide: [external PM](https://github.com/alexkirs/taskq/blob/main/docs/external-pm-tick.md).
```

## Required settings: #195

No #195 text on Required-settings. The Home row above covers it.

## Message samples

Before (real #195 close comment, glued numbers and words):

```text
manual full37674768854SUCCESS. CLI-only e4e6fec PR202/main tests37675786566SUCCESS, Pages37675786553 inputs4s/build+deploySKIPPED
```

After:

```text
Manual full Pages run 37674768854: success.
CLI-only push e4e6fec (PR 202): tests run 37675786566 success; Pages run 37675786553 skipped build and deploy, inputs unchanged.
```

### PM tick, English

```text
Started [#205](https://github.com/alexkirs/taskq/issues/205): `taskq spawn` -> [session](https://claude.ai/code/session_…).
[#191](https://github.com/alexkirs/taskq/issues/191) in review: candidate [abc1234](https://github.com/alexkirs/taskq/commit/…), checks pass. Needs your review.
Unknown: [#176](https://github.com/alexkirs/taskq/issues/176) session state, runtime-status unavailable. No retry.

Board: https://github.com/users/alexkirs/projects/…
| Task | State | Worker | Last activity |
|---|---|---|---|
```

### PM tick, Russian

```text
Запущен [#205](https://github.com/alexkirs/taskq/issues/205): `taskq spawn` -> [сессия](https://claude.ai/code/session_…).
[#191](https://github.com/alexkirs/taskq/issues/191) на ревью: кандидат [abc1234](https://github.com/alexkirs/taskq/commit/…), проверки прошли. Нужно ваше ревью.
Неизвестно: состояние сессии [#176](https://github.com/alexkirs/taskq/issues/176), runtime-status недоступен. Повтора нет.

Board: https://github.com/users/alexkirs/projects/…
| Задача | Состояние | Воркер | Активность |
|---|---|---|---|
```

### Blocker, English

```text
Blocked [#205](https://github.com/alexkirs/taskq/issues/205): Wiki edit needs your approval.
`taskq ask 205` -> waiting. Worker [session](https://claude.ai/code/session_…) keeps its claim.
Risk: none until you answer. Nothing published.
```

### Blocker, Russian

```text
Блок [#205](https://github.com/alexkirs/taskq/issues/205): правка Wiki требует вашего решения.
`taskq ask 205` -> ждёт ответа. [Сессия](https://claude.ai/code/session_…) воркера держит задачу.
Риск: нет, пока нет ответа. Ничего не опубликовано.
```

### Result, English

```text
[#205](https://github.com/alexkirs/taskq/issues/205) result: inventory and samples, commit [abc1234](https://github.com/alexkirs/taskq/commit/…) on branch taskq-205.
Checks: `python3 -m unittest discover -s tests` -> pass.
Not done: Wiki and contract changes. They wait for your review.
```

### Result, Russian

```text
[#205](https://github.com/alexkirs/taskq/issues/205) результат: инвентаризация и примеры, коммит [abc1234](https://github.com/alexkirs/taskq/commit/…) в ветке taskq-205.
Проверки: `python3 -m unittest discover -s tests` -> успешно.
Не сделано: правки Wiki и контракта. Ждут вашего ревью.
```
