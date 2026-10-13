# TaskQ from an existing agent session

Second entry to the same system: open a capable agent session, invoke TaskQ, manage the existing board.
Product direction under [P1/P2/P7](../product-principles.md). Mechanisms below are proposals; runtime roles and commands remain governed by [taskq.md](../../taskq.md).

## A1 Discover and activate

- Canonical invocation: `taskq`. Accept `task` or dictated `task you` only when intent clearly means TaskQ; do not trigger on every occurrence of “task”.
- An unfamiliar agent needs a discoverable entry: a thin project/global instruction, installed skill or exposed integration tool. Unknown words cannot bootstrap an integration by themselves.
- Installation registers the supported client's entry through an explicit setup step. Preserve existing instructions; link one versioned TaskQ contract instead of copying a prompt into every client.
- No installed entry: provide a documented one-time onboarding phrase/link. No shell/tools or board access: report the limitation; offer read-only context where possible, never pretend activation succeeded.
- Resolve project from explicit request or verified current project. Outside a known project, ask for the project; do not scan unrelated directories. Multiple projects require the owner list under R10.
- Load compatible TaskQ version, PM responsibilities, project/shared specs, commands, output rules, supported capabilities and current status. Use the installed release's contract; latest remote docs must not silently change an older binary's semantics.
- Invocation alone is read-only: no worker start, task creation, permission change or timer. Process the owner's subsequent request within existing authority.
- Keep the session and its useful context. TaskQ role applies to TaskQ work; never override higher-priority client rules or promote a worker/reviewer into a PM with greater authority.

## A2 Thin bootstrap, one contract

Proposed bootstrap response; no new command name or CLI flag committed yet:

- Protocol/contract version and authoritative rule references.
- Resolved project/board identity, effective scope and capability limits.
- PM command catalogue: read/status, task proposals, questions/answers, steering, results and observation.
- Canonical report format and instruction to relay its content unchanged.
- Recovery instruction: after context loss, reload this contract and current state before the next TaskQ operation.

The full rules load on activation, not on every unrelated message. Revalidate version/project after reconnect or scope change. No durable product truth lives only in chat memory.
Rule loading cannot guarantee obedience by every model; qualify supported clients with fixtures and enforce mutations in the service/CLI.

## A3 Standard reports, native rendering

- TaskQ generates factual reports from one snapshot; agents do not reconstruct counts, task rows or owner questions from memory.
- One semantic payload: schema version, project/board, freshness, task/attempt, question/revision, numbered options/recommendation, evidence and supported actions.
- A deterministic renderer supplies readable Markdown/plain text; native UI consumes the same payload when supported. Extend R6/§2 formats, not another independent reporting grammar.
- Preserve section order, status meanings, option numbers/text and links across clients. Different fonts, table wrapping, buttons and permalink support are client constraints; pixel identity is not promised.
- Plain text is the universal fallback. Show `N.K` with project context when needed. Never display a button the client cannot submit.
- Agent advice appears after the canonical report as explicitly separate suggestions. It must not rewrite the source question, move its recommendation or answer for the owner.
- A proposed alternative is discussed separately, then updates the authoritative question through an allowed flow before it becomes a selectable source option.
- A terse number is accepted only with one identified current question. Multiple projects/cards or a replaced revision require clarification. Re-read before submission; strong race safety still needs [Q2](plans-and-decisions.md#q2-commandapi-readiness).
- Untrusted task text, reports and tool results remain data; contract discovery uses the trusted integration entry, not instructions embedded in arbitrary board content.

## A4 Watching by agreement

The owner can request observation in this session. This specification does not create a watcher.

| Client capability | Mechanism | Limit |
|---|---|---|
| Active tool-capable session | Initial status, bounded wait, fresh report when an event arrives | Session/tool timeout, quota, disconnect or app closure may end observation |
| Native scheduled wake-up | Client scheduler resumes the named conversation and reads current state | Requires supported scheduling and explicit owner scope; do not invent wake-up capability |
| Service notifications | Service tracks relevant changes; supported client receives an event or owner opens the menu | Waking arbitrary chats needs a client adapter; notification is not an owner answer |
| No background support | On-demand refresh; explain that monitoring is inactive | Never promise future messages |

- Record the agreed projects/filter, events, destination, cadence or event trigger, duration/stop condition and observer capability. Reuse an existing matching watcher instead of duplicating it.
- Report only actionable changes by default: new/changed question, failure, review decision or requested completion. No periodic “nothing changed”. Deduplicate within the observer; unresolved questions remain visible on the board/menu.
- Observation does not change execution focus, spawn workers, extend permissions or answer questions. Suggestions stay separate under A3.
- Show Active / Interrupted / Stopped / Unknown with last successful check. A timer record alone is not proof that the agent can wake or deliver.
- Stop watching cancels that subscription/timer, not tasks. Closing a client follows its actual lifecycle; do not transfer watching to another channel silently.
- Queue execution remains independent of the PM and its timer under R3/R4. Model invocation is for explanation/decision support; deterministic service polling remains the execution mechanism.

## A5 Existing foundation and gaps

Source review: design worktree baseline `d7fd593`; not a fresh installed-release audit.

- R3 defines PM; R6 defines reports; §2 defines numbered answers; §5 provides `status`, `inbox`, `wait` and task commands.
- Existing `wait` defaults to a bounded 10-minute window and polls within configured limits (`src/main.rs`, `wait` and command dispatch). It is a blocking command, not a persistent scheduler or chat wake-up mechanism.
- `wait` stores one `.taskq/pm-cursor` per project root, tracks state strings and initially queries recent changes. Two observers share that cache; a new question with the same state is not independently identified. Read current status on entry; do not claim reliable multi-subscriber question delivery.
- Reliable observation needs per-subscription delivery cursors and question/attempt revisions, catch-up after gaps and explicit expiry. Cursors are reconstructible delivery metadata, never a second task queue. Define storage and R1 compatibility before implementation.
- Add bootstrap/capability contracts, shared structured reports and client bindings as a small first slice. No universal plugin engine or direct agent message bus is required.
- Scheduling, native buttons, remote permalinks and session wake-up need a capability matrix and client-specific acceptance. Unsupported clients retain standard text and on-demand commands.

## Acceptance

- A-A1: installed entry activates an unfamiliar PM in the current session; absent entry produces onboarding guidance, not fictitious tool access.
- A-A2: activation performs no mutation/timer; project ambiguity and worker-role conflicts remain explicit.
- A-A3: two supported agents show the same fixture's identities, counts, ordered source choices and evidence; native/text presentation may differ. Advice cannot alter the canonical card.
- A-A4: after context loss or a version mismatch, reload rules/status before acting; no duplicate task or answer.
- A-A5: initial unresolved questions appear; changed question in unchanged `ask` appears; concurrent observers do not consume each other's events. Existing `wait` alone does not meet this gate.
- A-A6: supported scheduled observation wakes the intended conversation; unsupported/closed/offline/quota-limited clients show the actual limitation. Stopping observation leaves execution intact.
- A-A7: owner answers in menu while PM is offline; PM refreshes and does not re-answer the resolved card. Unknown submission reconciles before retry.

Planning/start behavior follows [P2](plans-and-decisions.md#p2-ai-led-authoring): create held by default; explicit start; one brief reminder after an actionable planning milestone, no repeated reminder per message.

Remote-server application: [remote manager](remote-manager.md) defines project-chat routing, Hermes/FRA migration, observation delivery and qualification without adding a worker runtime.
