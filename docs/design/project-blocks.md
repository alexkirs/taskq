# Shared project summaries and task rows

Implementation of [B1–B4](shared-blocks.md), [K-A1–K-A8](task-counters.md) and Menu M2/M3, under R6. TaskQ owns `src/project-blocks.js` and `.css`; both the tray and browser consumers render these exact assets. No network client, credentials, parser, framework or task operation is added.

## Interface and delivery

`TaskqProjectBlocks` (browser global or CommonJS) exposes `summary(project)`, `aggregate(projects)`, `task(project,row,global)` and `handle(event,adapter)`. Insert returned HTML; bind the click handler once. Adapter functions `project(id)`, `tasks(id,state)` and `open(url)` own navigation. Project and counter controls are siblings, never nested. Consumers arrange blocks and set the inherited font/available width; they do not override internals.

A project has `{id,name,board:{kind,url},complete,closed,tasks}`. Board URL identifies provider/server/repository; `id` only routes navigation. Rows are canonical snapshot rows (`n`, `state`, `revision`, `title`, `url`) plus adapter-supplied `host`, `runtime`, `local_session`, `session_url`. Only the owning host supplies local session links; remote rows disclose that limitation. Links accept HTTP(S), with `codex://threads/` additionally allowed for local sessions. Untrusted text is escaped.

Counters deduplicate board/issue identities, keep Doing and Review distinct, hide known zero, preserve unknown, and mark stale/incomplete/conflicting sources Partial. Closed is a separate authoritative managed-issue total, never inferred from open rows or events; missing/conflicting totals show `?`. Tray currently has no authoritative closed source and opens an explicit unknown Closed view with provider Board routes. This deferral preserves the target, rather than inventing data or dropping Closed. The native adapter's filter route shows the matching observed project/state rows; partial data remains labelled.

Release staging publishes `taskq-<release>-project-blocks.js`, `.css`, `.LICENSE`, `-fixtures.json` and `-sources.json` through the existing release workflow, checksums and attestations. DashQ pins an actual release and its checksums, serves assets locally under its CSP, and supplies its own navigation adapter. Source hashes are not evidence of release publication or DashQ adoption. No DashQ files are edited here.

## Parent-to-child map

[sources manifest](project-blocks.sources.json) pins design PR #943 head `75757d5d843159726a2ea44ca1615f9cf7fd18fd`, merged as `cf46e637d45f971dc83f1c1ea5f1e9a119a74256`, parent source hashes, component hashes and supplier icon hashes. Asset schema revision is 1; release version is assigned by the existing tag workflow.

| Parent | Shared child | Permitted placement / evidence |
|---|---|---|
| `menu-next-variants.html` counts()/projectRow(), K-A8 | summary()/aggregate(), scoped `.tqp-counts` | Consumer supplies width; order, capsule geometry, exact tokens, compact gaps and every wrapped right edge compared on identical counts |
| `menu-next-variants.html` task(), Menu M3 | task() | Flowing metadata/title over full width; actual host/runtime/session targets replace synthetic WIN/MAC/Codex. Independent issue and session links; remote session remains read-only |
| visual-style.md, design/assets | bundled SVGs and tokens | Local Lucide 1.17.0 (same release as parent); provider/runtime marks are byte-identical Simple Icons assets. Ask is labelled Ask per K semantics (parent demo calls it Human) |
| tray adapter | exact JS/CSS embedded by tray.rs | Existing answer-card instance/draft/refresh behavior retained beside task row; native qualification remains separate |

`node tests/project-blocks.test.cjs` verifies shared semantics and provenance. `node tests/project-blocks.browser.cjs --evidence` compares pinned editable parent functions/CSS, standalone browser consumer and actual tray HTML on shared fixtures at both themes, 16/20 px, 320/360 px, counts 0/1/12/100 and long content. It also checks unknown/partial, provider marks, independent counter/project/issue/session navigation and overflow. The existing answer-card browser comparisons remain applicable; their counter expectation now follows R6/K-A1/K-A2.

Owner-approved delivery for #938: extraction PR, release wiring and browser comparisons first; published release/checksum and native Windows/macOS conformance follow separately, as for #920. README screenshot freshness is an open native capture gate; no manifest or check is weakened here. Composer/create/recovery remain #949/#948.

Parent defect (M3/B3): the prototype's unbounded project button can overflow on a long unbroken project name. The shared block bounds its identity region and wraps text without shrinking it. Counter comparisons use identical bounded labels; long project identity is additionally checked in both consumer harnesses. Parent task geometry is compared with identical long titles, including full-width continuation.
