# Configurable tray icons

Accepted product direction. Icon / Summary / Custom on macOS and Windows. Runtime implementation remains gated by R8/R13; this document changes no installed tray or agent grants.

## CI1 One renderer and format

- Icon keeps the approved compact Q asset. Summary is an immutable, versioned built-in configuration targeting every I1–I6 rule. [icon-summary.config.json](icon-summary.config.json) is the prototype preset source; the builder embeds it without a separate layout implementation. Native conformance remains a release gate. Custom uses exactly the same configuration schema, primitives, layout engine and renderer. No separate hardcoded Summary drawing branch.
- Edit Summary copies its configuration into a Custom draft. The built-in preset remains available. Custom is optional; existing choices survive migration.
- Persist the configuration, not a screenshot or prompt. Keep a short owner request as edit context, never as executable state. Menu totals and task state stay board-derived; this is local appearance preference.
- Read validated config + canonical snapshot + native size/theme/font metrics → render. No model call per refresh. Schema version, primitive identity and data bindings determine meaning.

## CI2 Agent tools and bounded composition

The agent receives a versioned capability catalogue before proposing a design: supported primitives, canonical data keys/units, fonts and metrics, platform sizes, semantic colors, geometry constraints and validation diagnostics.

| Capability | Contract |
|---|---|
| Inspect | Current saved config/revision, built-in preset, available primitives/fonts and platform slots |
| Compose | Draft only: counter, bounded text/approved glyph, stacked histogram, sparkline; rows, columns, grid, anchors, align/distribute and gaps |
| Bind | Named counts (doing/review/work/ask/ready/failed/blocked/later/closed), named event series and explicit period; no invented percentages or completion ETA |
| Type | Installed/licensed font catalogue; explicit fallback. Size/weight, ink-edge/baseline/center alignment. Optional fonts never silently download |
| Preview | Same renderer as tray; actual size plus labelled enlargement; both themes, active/paused/held/unknown, zero/multi-digit values, supported DPI |
| Validate | Unsupported fields, missing data, out-of-bounds/overlap, contrast, font fallback, unreadable overflow and invalid chart semantics reported by element |
| Save | Owner confirms exact validated revision/hash; atomic local preference write and readback. Stale drafts require re-review; failed save keeps prior active config |

Protocol names above describe capabilities, not shipped CLI commands. Use existing AI conversation/request infrastructure and allowlisted preference operations. The full operator uses its existing tools and permissions (R3/R9); icon settings still use a schema-returning proposal/preview route before applying the selected design.

Config contains version, preset/source revision, semantic data bindings, element IDs, layout/anchor constraints, typography, semantic palette, overflow and platform adaptations. Bounded numeric domains and allowlisted primitives; no JavaScript, external URLs, arbitrary filesystem/font paths or executable expressions. Reject unsupported schema versions. Prototype supports a deliberately smaller tested subset; it must not advertise unimplemented primitives as working controls.

## CI3 Describe → compare → edit → approve

- First Custom selection opens the editor in the existing workspace, leaves the active tray unchanged and explains the available choices with 2–3 visibly different previews. Suggested starting points: numbers first, graph first, attention first.
- User may describe any supported composition. AI returns a draft and concise explanation of any trade-off or unsupported request; never silently substitutes an unrelated template.
- Show current and draft at actual size. Keep enlarged inspection optional and labelled; do not approve based only on a large image.
- “Center the number”, “align the left edges”, “make the graph lower” changes only named constraints; unrelated elements remain unchanged. Ambiguous targets require clarification.
- Direct controls and language edits share one draft. Font/layout/data/spacing controls remain available without an agent. Generation failure preserves draft and active icon; no runtime or quota means manual editing still works.
- Use icon → validate → explicit approval → save/readback → select Custom. Selection of a saved Custom is immediate. Editing again loads the saved revision; cancel/discard never changes the tray. Keep the previous accepted revision for Revert.
- Scope is this host's appearance, not project policy. Restart restores it. Other hosts remain unchanged; missing fonts on a new host require fallback preview/review before activation.

## CI4 Truth and native constraints

- Custom may omit chosen counters; exact menu counts remain available. Every displayed value retains its definition. Unknown is a dash, not zero. Overflow uses explicit calibrated thresholds, never clipped digits or live per-value shrinking.
- Preserve fixed square dimensions, meaningful status indications and user-accessible state. Paused uses gray rendering plus the accepted centered Q/pause overlay; emergency hold/unknown stay distinguishable. Custom cannot hide the state policy.
- Layout uses measured ink edges, stable baselines and constrained cells. Rasterize once at native size/DPR; bars use integer pixel widths/gaps and fixed time bins. Shared scale across series; preserve totals during rebinning.
- Changing preset or editing cannot change admission, permissions, tasks or cadence collection. Charts label their period/series in accessible descriptions and details.
- Invalid/corrupt/incompatible config keeps a valid prior config or built-in Summary; report fallback visibly. No blank tray or silent reinterpretation.

## CI5 Acceptance

- CI-A1: the built-in Summary config reproduces I1–I6 through the same renderer as an identical Custom copy; compare pixels at the same font/size/snapshot.
- CI-A2: first Custom opens a draft without switching tray; different proposals are comparable at actual size. Unsupported language keeps the draft and explains the limitation.
- CI-A3: alignment/type/data edits affect only specified fields; cancel preserves saved config; approval saves the exact previewed revision.
- CI-A4: restart/edit/revert restores the expected config; duplicate/stale approval and write failure cannot overwrite a newer saved config.
- CI-A5: unknown schema, forbidden code/URL/font path, invalid bounds, unavailable fonts and large values fail validation or use an explicit qualified fallback.
- CI-A6: native size/theme/DPI matrix, all lifecycle states and large/zero/unknown values; no cropped glyph, invalid series or floating bar geometry.
- CI-A7: no agent available → manual controls work; icon refresh never invokes AI. No change to task or permission state.

Product protocol is final; OS font calibration, persisted-storage implementation and agent adapters remain engineering qualification. User preference is not proof of renderer correctness.
