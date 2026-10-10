# Tight / Hermes onboarding candidate (#538)

Status: design and portable mocks only. This is a proposal, not an operational
policy or activation approval. [taskq.md](../taskq.md) remains the manager contract.
The recorded owner approval permits this document and isolated fixtures only.
#526 retains Claude/Codex live qualification; #274/#275 are historical references,
not evidence that Hermes is qualified.

## Proposed smallest scope

R1/R4: replace optional-only assignment protection with a default authenticated
native-board membership check on every fresh execution read, including unfiltered
managers. A task with assignees is eligible only for an authenticated member.
Multiple native assignees remain eligible. Missing/failed authentication refuses
assigned tasks. No Tight-specific login registry, label or host/session ownership
workaround. Preserve the optional #480 report/selection filter as an additional
restriction, never as authentication. Preserve `assignee-only` strict refusal for
unassigned tasks. Proposed unassigned default: preserve existing shared intake
eligibility, subject to PM, status, dependencies, host and capacity gates. This
unassigned choice still needs owner approval before implementation.

R3/R4: assignment is eligibility, not controller authority. Preserve #532:
`pm` records the genuine manager runtime/session/host, determines supervisor
runtime, manager gate and outcome recipient. Hermes PM selects native Hermes
supervisor; `run-codex` independently selects Codex worker. The recorded supervisor
remains sole controller, preserving the existing recorded-PM-on-owner-word and
plain-owner-shell command exceptions in § 4; no second manager review path is
introduced. An eligible
assignee with a foreign PM cannot dispatch or consume that PM's outcomes.
A matching session string on a different runtime is not the same identity.

Reassignment requires explicit owner approval and a fresh guarded board read.
Do not steal, retire, replace or rebind an active/unknown claim, supervisor, order,
model turn, recovery hold or pending receipt. Assignment changes alone never
rewrite PM or route outcomes. Initial transfer scope should cover only unstarted
parked tasks with no recorded sessions/resources/pending events. A transfer that
also changes PM needs an explicit atomic board transition naming old/new assignee
and genuine PM, fresh verification, and preserved history; there is no such
qualified transfer command today. Active transfer remains outside this slice.

R1/R8: reuse `add --later`, which already creates Later without dispatch. The CLI
currently lacks explicit native assignment at creation. Proposed `add --later
--assignee LOGIN` must create the parked issue with its native assignment in the
same adapter creation request and verify both on fresh read before success.
Unknown creation must retain the guard, never retry creation or briefly create
Ready. GitHub/GitLab/custom adapter support and failure diagnostics need review
before implementing this option. No second queue or copied manager policy.

## Minimal reproducible onboarding after separate approval

1. Register a separate Tight GitHub account, then independently verify repository
   membership/API access and the authenticated native login. Shared `alexkirs`
   cannot distinguish Tight. No registration or credentials are handled here.
2. Select a reviewed immutable TaskQ release and read its contract with `taskq pm`
   from the explicit project. Use genuine Hermes-supplied `HERMES_SESSION_ID`;
   never supply a fabricated identity or relabel a Codex session.
3. On separately qualified Linux, follow the existing
   [native bridge setup](../taskq.md#native-hermes-admission-local-candidate).
   Explicit isolated Hermes home and installed gateway argv are prerequisites.
   Availability is not authentication or lifecycle proof. Preserve configured
   models, effort, permissions and host/project capacity; no live launch here.
4. After the creation/assignment candidate is approved and qualified, create one
   explicitly assigned Later fixture. Read back native assignee, Later status,
   genuine PM and absence of sessions before any separately approved dispatch.
5. Approve one bounded live lifecycle packet before Ready: exact release/head,
   disposable fixture/identity, finite limits, cleanup, acceptance and stop rules.
   Reuse the existing manager contract and Linux pilot rather than new timers,
   workers, management instructions or adoption automation.

## Evidence and remaining gates

| Requirement | Portable evidence in this slice | Required later evidence |
|---|---|---|
| Foreign assignment, no personal filter | Pure model excludes foreign native login | Fresh adapter read and all execution/send/recovery paths refuse before effects |
| Own/multiple assignment | Model includes authenticated member | Genuine board identity, PM gate and finite admission |
| Unassigned policy | Model includes unlabelled, excludes strict label | Owner accepts proposed policy; native adapter parity |
| Explicit transfer | Design refuses active/unknown ownership | Approved guarded unstarted transfer, no claim theft, preserved events/history |
| Initial Later | Existing contract supports `add --later` | Atomic native assignment creation/readback; ambiguous response/no-dispatch fault tests |
| Authority | Preserve R3/R4/#532, independent worker runtime | Foreign PM cannot control/wake/consume; exact runtime/session checks |
| Hermes lifecycle | Existing portable protocol mocks only | Genuine native IDs/names; questions, receipt/application, rework, exact-head review/publication, manager wake |
| Restart/retirement | No durability claim | Linux birth/pidfd owner proof, lost-owner refusal, safe recovery and confirmed retirement |

Run `python3 -m unittest tests.test_single.TightAssignmentDesign` for the isolated
selection proposal. These assertions cannot prove production selection, native
identity, board permissions, atomic creation, transfer or Hermes operation.
No taskq.py change, production import, account, launch, timer, permission or
configuration change is part of this deliverable. Approve the proposed product
scope before implementation; approve bounded Linux live qualification separately.
