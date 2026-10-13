# Dynamic host resources

Accepted product target (D03), not current runtime behavior. Inherit [product principles](../product-principles.md), [Work mode](menu-product.md) and [decision register](decision-register.md#d03-machine-resources-and-urgency). taskq.md §3/§9 remains the runtime authority until compatible implementation.

## R1 Modes and ownership

- New host default: Auto. No session-count question is required. Start eligible work gradually while measured resources permit it; ten sessions are valid if the machine sustains them.
- Auto + optional maximum: host, project and runtime ceilings intersect. No maximum means no user-imposed count ceiling, not a promise of unlimited resources. Zero explicitly disables that target; absence is not zero in the new schema.
- Fixed: explicit count ceilings with automatic load gating disabled. Show the trade-off and load; preserve task/dependency/access checks. Never silently switch Auto to Fixed when metrics fail.
- Host operator owns resource policy. Project/task text cannot raise it. Project/runtime ceilings may narrow it. Disabled runtimes, paused projects, provider restrictions and host grants remain separate constraints.
- One host-wide admission/reservation authority coordinates every project and managed worker, reviewer and operator. Reserve before spawn, reconcile failed/unknown spawns, and prevent simultaneous projects from consuming the same headroom. Do not add a second canonical task queue.
- Urgent eligible work gets the next launch; ordinary projects rotate. Define and test reviewer/resume/operator liveness without unlimited privileged bypass. Drafting and deterministic status/answers remain usable while admission waits.

## R2 Measurement and launch proposal

Direction approved; numerical tuning below must be measured on macOS/Windows before becoming release defaults.

- Recommend a 60-second rolling CPU baseline plus current available-memory/pressure checks. Latest unhealthy reading vetoes launch even when the average is healthy. Longer 5/10/15-minute windows remain advanced options; they smooth noise but react/recover slowly.
- Reuse the current gate's starting thresholds for calibration: CPU below 85%, available RAM above 15%; reopen at CPU at most 75% and RAM at least 20%. Track sustained pressure/swap where reliable. Percentages alone need low-memory-host testing.
- Candidate sampling: every 30 seconds; require three valid post-baseline readings after startup or policy changes. Missing, invalid or over-60-second-old metrics hold new launches with a visible reason.
- Candidate pacing: at most one host-wide launch per 30 seconds; require a fresh post-launch reading before another. No burst of ten processes merely because an idle sample looks healthy.
- Pressure closes admission; healthy hysteresis reopens it. Do not terminate running work when load rises or a new ceiling falls below active count. Display that new starts are held.
- CPU/RAM cannot predict an idle agent's later build spike. Provider rate limits, credits, connectivity and session quotas are separate eligibility reasons; Auto is not a spending limit.
- Deterministic admission uses telemetry and configured rules. AI helps explain/change settings, never decides every launch.

## R3 Interface

Keep the current Work mode editor; no new top-level panel.

| Place | Compact display / action |
|---|---|
| Menu | `Auto · 7 running` or `Auto · waiting: memory pressure` |
| Optional cap | `Auto · 7 / 10 running`; caps count sessions, not CPU cores |
| Work mode | Auto / Fixed; optional maximum; project/runtime overrides behind Details |
| Details | Named host, sample freshness, CPU/RAM, observation window, effective ceilings and their sources |
| Waiting task | Actual cause: load, ceiling, pause, dependency, scope, access or provider; show all blockers in Details |
| AI request | “Use spare resources, at most six sessions; frontend at most two” → exact settings diff → confirmation → readback |

Distinguish saved policy from active policy. Unknown readings are not zero load. Remote edits show pending/offline until matching host readback. No ETA inferred from load.

## R4 Migration and acceptance

Existing limits remain until reviewed migration. Show which limits become explicit caps and which old defaults would be removed; preserve deliberate zero/disabled settings. Do not leave Auto secretly capped at legacy one, or reinterpret missing runtime limits as enabled. Older services must not bypass shared admission; define compatibility and host authority before enabling Auto.

- R-A1: fresh qualified host needs no count entry; Auto grows one launch at a time and can exceed one session.
- R-A2: two projects competing for headroom cannot double-admit; accounting includes reviewer/resume/operator and failed spawns.
- R-A3: external load, idle-agent build spike, stale telemetry and startup hold new launches; active work survives.
- R-A4: oscillating load does not flap admission; reopening and longer windows match documented sampling semantics.
- R-A5: strict caps, zero, disabled runtime and pause remain effective; lowering a cap preserves active attempts.
- R-A6: p0 waits for the next eligible admission; normal projects and review/interactive work have tested progress under sustained demand.
- R-A7: migration preserves explicit settings, explains removed inherited caps, and rejects incompatible shared admission.
- R-A8: native menu and CLI report the same effective mode, counts, freshness and blocking reasons on both OSes.

These extend UC-01/04/12. G3 requires lifecycle/accounting tests and hardware qualification, not only a settings mockup.
