# Agent settings workspace

Detailed candidate under MP5/P6; no active grant or runtime change. One selected Workspace review layout. [Integrated reference](workspace-prototype.html); [isolated component](agent-settings.html) is a development fixture.

## AS1 Entry and identity

- The review opens with menu and workspace navigation visible. Never present an isolated settings component as the complete product.
- Settings → Agents raises the existing workspace at Agents; permission requests deep-link to their exact host/runtime/project/task.
- Host once in the header. Runtime selector shows availability; retain disabled/not-installed agents. Probe age and unknown state remain visible.
- Per-runtime drafts survive navigation. Model/effort are separate from access; permission edits never overwrite them. This reference leaves their existing values unchanged.
- One current-policy summary: effective profile, source/revision, freshness and existing attempts using an older revision. Never mistake a selected draft for applied policy.

Delivery behavior has a separate [project-scoped proposal](agent-behavior.md); Access does not imply push or merge authority.

## AS2 Edit access

- Compact Read only / Workspace / Full control. Selecting prepares a draft, never applies it.
- Files, Network and Tools disclose exact effective restrictions; unknown or unsupported combinations block Apply with a reason. Additional paths require canonicalization and host review; arbitrary text is not a grant.
- Full without external enforcement displays unrestricted filesystem/tool egress and no runtime sandbox. Disable fictitious network/tool restrictions; explain why. Provider transport remains distinct from worker egress.
- Details preserve unsaved choices when switching presets; review the effective change, including derived values. Do not show a green containment badge from a user checkbox.
- Project exceptions are host-owned and keyed by canonical project identity. Show only exceptions plus Add; omitted projects inherit. Removing an override means return to inheritance, which may broaden access; include it in review.
- Local host override can raise or lower within the admin ceiling; repository/task restrictions still intersect it. Show inherited/effective values and any conflicting request.

## AS3 Review and application

- One review shows host/runtime, exact old→new capability changes and affected inheriting projects/overrides. Existing attempts continue with their recorded policy; narrowing is not an emergency stop.
- Confirm binds policy revision and immutable draft. Duplicate confirm is inert. Changed service revision invalidates review; reload and reconcile, never overwrite silently.
- Distinct states: Draft → Review → Saved / waiting for service → Applied, Failed, or Unknown. Applied requires matching fresh readback. Retry first checks receipt/revision; no blind second grant.
- Show next action on failure, preserve draft, retain previous effective policy until readback proves change. A running old-policy session cannot silently acquire or shed permissions on resume.
- Unchanged policy needs no repeat confirmation. Cancel review returns to draft; discard restores saved values.

## AS4 Reference boundary and acceptance

Preview state is a labelled demo control: selecting Editing, Saved, Applied, Failed, Unknown or Revision changed immediately displays that fixture. It is not a product setting. Ordinary confirmation still distinguishes save from service application.

Reference uses fictional versioned probes and two fixture projects. Editable presets/network, project override, removal, revision conflict, failure and explicit simulated readback exercise the route. Files/tools explain boundaries; adding real roots/tool patterns is deferred until adapters expose a truthful capability catalog. No raw runtime flag editor.

- AS-A1: selecting Full changes draft only; derived network/tools become unrestricted without false sandbox claims.
- AS-A2: exception survives host-default change; removal reviews inherited behavior; task/project constraints are not overwritten.
- AS-A3: switching runtime preserves separate draft; model/effort unchanged.
- AS-A4: Saved stays distinct from Applied; failed/unknown readback preserves prior effective state.
- AS-A5: stale revision refuses apply; repeat confirm cannot duplicate a grant.
- AS-A6: both themes and 20px narrow layout remain readable; all review actions reachable.
- AS-A7: runtime unavailable, unsupported enforcement and request above authority have explicit blocked routes in the production ledger.

Reusable rules already live in shared F1/F3/F5 and permission authority. This page defines TaskQ navigation and policy presentation only.
