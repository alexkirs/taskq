# Session message contract

Status: proposed by #232. This is a readable envelope for existing TaskQ
routes, not a queue, scheduler, receipt database, or new protocol. It reuses
the #191 receive/applied distinction, the #205 concise RU/EN rule, and links
completion supervision (#223) and runtime qualification (#224).

## Write, deliver, render

1. **Source authoring.** Record the exact UTF-8 text supplied by the sender,
   its route, and sender/session identity. Shell quoting is part of authoring.
2. **Delivery.** Record the exact destination payload and destination identity.
   Compare it to source unchanged. A difference is a delivery defect only
   after this comparison.
3. **Rendering.** Record the human-visible output separately. A renderer may
   shorten or format text, but must not be used as payload evidence.
4. **Terminal.** ANSI redraw/control output is terminal presentation. Strip it
   only for a separately labelled display comparison, never from the captured
   payload.

Preserve whitespace, paragraphs, Unicode, links, code and identities. Do not
insert spaces heuristically or decode literal escapes in arbitrary text.

## Human envelope

Use concise labelled prose. Omit a field when absent; do not compress words.
The labels work in RU or EN:

| Service field | RU label | EN label |
|---|---|---|
| assignment | Задача | Assignment |
| context | Контекст | Context |
| constraints | Ограничения | Constraints |
| question | Вопрос | Question |
| blocker | Блокер | Blocker |
| result | Результат | Result |
| check evidence | Проверка | Check evidence |
| next step | Следующий шаг | Next step |

Example:

```text
Задача: проверить #232.
Ограничения: не менять main.
Проверка: `taskq view 232 --notes 30`.
Следующий шаг: независимый review.
```

## Existing routes and evidence

| Route | Source | Destination/readback | Current status |
|---|---|---|---|
| DOT ↔ Codex | `taskq codex-send THREAD --text TEXT` | Codex thread; `codex-read` is diagnostic only | unqualified |
| DOT ↔ Claude | configured runtime executor | runtime-supported readback; generic `taskq send --runtime claude` refuses | unqualified |
| worker ↔ PM board | `claim`, `result`, `ask`, `answer`, issue notes | `taskq view IID --notes N` | authoring reproduction below; live route qualification incomplete |
| PM report | #191 readback JSON | `taskq report-verify FILE` | schema/fixture only unless supported live evidence is supplied |

For every genuine qualification, keep the correlated harmless probe ID,
source text, destination text, rendered text, route/session identities,
timestamps, and independent-review verdict. Missing evidence is
`unknown`/`unqualified`, not an ACK. #223 owns completion reconciliation;
#224 owns native Codex-PM qualification.

## Literal-escape reproduction

The prior #232 hand-in was authored through the `--text` fragment
`"Audit complete...\\n\\nFinding: ..."`, where `\\n\\n` was literal source text.
`taskq view 232 --notes 30` reads the board note with those same literal
sequences. This proves an authoring-boundary case; it does **not** prove a
transport defect. The exact board text is retained in #232's `result` note.

The other supplied source sample was already collapsed before delivery:

```text
Root GitHub подтверждает221CLOSED07:49:18 exact620d0207;223покаq-waitingclaimnullна07:56.
```

## Exact minimal refactor proposal — pending acceptance

Implemented by #232 after technical acceptance: `ce4e4ce` is the docs-first
proposal; the parser change keeps delivery text verbatim.

| Area | Proposed change |
|---|---|
| `taskq/__init__.py` | Extend the shared parser used by `ask`, `result`, `answer`, `reject`, `release`, `close`, `later`, `problem`, `codex-send`, `send` and optional-text `spawn`: exactly one input where text is required; UTF-8 file text is read verbatim. |
| `taskq/worker.py`, `taskq/codex.py` | No route/serialization change: both continue consuming `args.text`. |
| tests | Parser/read-file regressions: real paragraphs, Unicode, Markdown links, code, literal backslash-n unchanged; reject absent/both inputs; assert the existing `--text` callers remain compatible. |

API: `--text-file PATH` is optional alternative input, not an escape decoder,
formatter, new message type, or receipt mechanism. It avoids shell-authored
newline mistakes while leaving transport text verbatim.

## Evidence gaps

- The #191 report-contract fixture accepts labelled Codex, Claude and DOT
  readback schemas. It is not live transport qualification.
- `taskq codex-read 01a11aab-0be7-7632-8433-da13f9400c62 --limit 1` was
  refused locally with `PermissionError: [Errno 1] Operation not permitted`
  while connecting to the Codex socket. No Codex payload was sent or read. An
  earlier malformed probe ID is not evidence that it caused this refusal.
- No owned Claude session was supplied for a harmless probe; do not message
  another worker's session.
- Board evidence above is an exact source/readback reproduction, not a
  complete independent qualification.
