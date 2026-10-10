Project guard: held G-fa79d4746c21; controller mac codex:controller; controller accessibility/operation unknown. PM: supported drain/readback; guard не снимать по наблюдению.
tmpmno13kj0 · [board](https://github.com/fixture/repo/issues)
In work 0 · Waiting for answer 1 · Ready 0

Questions (answer N.M):

| Question | Brief reason | Options |
|---|---|---|
| [#1 Captured obligation](https://fixture/1) | Current choice | 1.1 retain · 1.2 qualify |

Проблемы сессий (read-only; references не являются answer tokens):

| Reference / task | Роль / состояние | Свидетельство / проблема | Ответственный / Следующее действие |
|---|---|---|---|
| P1-c5388e6632a9 / #1 | supervisor / waiting | blocked-prerequisite: Prerequisite остаётся открытым; dependency:#2;list-snapshot | supervisor: проверить зависимость; не обходить gate |
| P1-aabbda984a3d / #1 | owner / waiting | owner-question: Текущий вопрос ожидает board receipt; board; ask:9 | owner/PM: сверить текущий вопрос и уже данный ответ; не запрашивать повторное разрешение |
| P1-920565440c93 / #1 | recipient / waiting | pending-event: Событие ожидает receipt; event:9;recipient:6b987654a411 | recipient: сверить exact recipient/event; не ack по наблюдению |
| P1-3d5255bb9963 / #1 | supervisor / unknown | surface-auth: Последняя команда отказала по auth; cli-turn;evidence:fc164d6f353e | owning-runtime: сравнить контекст запуска; credentials не переносить |
| P1-4d1899a501c5 / #1 | worker / unknown | surface-auth: Последняя команда отказала по auth; cli-turn;evidence:fc164d6f353e | owning-runtime: сравнить контекст запуска; credentials не переносить |

Диагностика: per-task re-read; зависимости — list snapshot; runtime evidence не доказывает application/result receipt.

Mode: events · arm: <arm_tick>
