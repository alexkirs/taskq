# TaskQ visual system

Final candidate for the approved dense menu. Shared principles: [specq D1–D14](https://github.com/taskqorg/specq/blob/main/design/operator-ui.md).

## Geometry and type

- System UI font: SF Pro on macOS; Segoe UI Variable/Segoe UI on Windows. Normal shapes, weights 400/500.
- Menu base 16 px; user options 14/16/18/20. Secondary text at least max(12 px, 0.85×base).
- Native width and content-fit height follow [R-UI2](rendering-contract.md#r-ui2-native-menu-sizing); preview width is 360 px. Fixed demo heights are not production rules. Never scale the entire window down.
- Outer inset 8 px; row horizontal inset 4 px; related gap 4–6 px; section separation 6 px with one 1 px divider.
- Fine-pointer row target at least 32 px; coarse-pointer target at least 44 px. Icons normally 16 px; compact top-strip icons keep usable targets.
- Menu radius 12 px; controls 5–7 px; counter capsules; counter symbols may use 14 px within 32 px targets. No nested decorative cards; bordered surfaces only for composers and consequential change review.
- Header/control strip stays accessible. Native height is bounded by the monitor work area; scroll content, never clip it. AI window uses a fixed composer within the window and a scrolling transcript.
- At short/narrow sizes, reflow header controls or use the conversation list as a separate page. Maintain Back, Send, context and attachments.
- Metadata starts the task's first line; title continuation starts at the full block left edge. Do not shrink the title to accommodate columns.

## Palette

| Token | Light | Dark |
|---|---|---|
| Surface | #FAFBFC | #252B32 |
| Text | #243039 | #EEF2F5 |
| Secondary | #59636E | #B9C3CD |
| Divider | #DCE0E5 | #46505B |
| Hover/selected surface | #E9EDF1 | #37424C |
| Work / success | #246F59 | #8DD2B8 |
| Question / confirmation | #955525 | #EDBC96 |
| Ready / blocked | #5D6874 | #C8D0D9 |
| Closed text / fill | #68727D / #EFF1F3 | #9BA5B0 / #2B3138 |
| Failed | #A6324B | #EE96A5 |

Use subtle counter fills, not colored text blocks. Always include a symbol or label. Board-label HEX values remain in board.md; do not replace them with UI tokens.
Closed has the lowest visual emphasis without opacity; [counter semantics](task-counters.md).
Paused data is gray; partial pause/unknown does not gray all counts. Unknown ([never healthy](https://github.com/taskqorg/specq/blob/main/engineering/verification.md#unknown-is-never-healthy)) is labelled.

## Icon inventory

Use one outline family for actions, consistent 16 px optical size. Prototype uses Lucide; bundle production assets locally with license/source metadata.

| Meaning | Icon name / asset |
|---|---|
| Admission | play / pause |
| Controls / Work mode | sliders-horizontal / focus |
| Attention/question/failure | circle-alert / message-circle-question / circle-x |
| Project / pin / add | folder / pin / plus |
| Board / issue | github.svg or gitlab.svg plus external-link where needed |
| Conversations / Work sessions | messages-square / terminal |
| Attach / send / history | paperclip / arrow-up / history |
| Detach / return / close | panels-top-left / panel-left-close / x |
| Search / archive / restore | search / archive / archive-restore |
| Runtime | claude.svg; openai.svg with explicit Codex label, including codex-app |
| Remote host | network; no local-session link |
| Log / folder / restart | file-text / folder-open / rotate-cw |
| Stop / uninstall | square / trash-2 |
| Appearance / cadence / agents / services / about | type / chart-no-axes-column / bot / activity / info |

Icon-only buttons have accessible names, tooltips and focus rings. Brand marks identify the provider; visible/accessible labels identify the exact runtime. Never invent a permalink from the mark.

## Tray assets

- Reuse the accepted compact q geometry. `assets/q-*.png` are installed 4.1.8 fixture exports for reference, not universal production sizes.
- Summary follows [I1–I5](icon-summary.md). Do not copy the old wide strip or segmented digits.
- Active summary: fixed square with three counters and graph. Paused: identical gray data with compact q/pause centered.
- Overflow, unknown rendering, 6/12 bins and Windows 16 px still need explicit acceptance. The candidate does not silently choose a lossy policy.

## Asset sources

`claude.svg`, `openai.svg`, `github.svg`, `gitlab.svg`: Simple Icons 15.0.0, https://cdn.jsdelivr.net/npm/simple-icons@15.0.0/icons/ (CC0 library; marks retain their owners' trademark rights). OpenAI mark is provider identity, not a claimed separate Codex logo.
Other icons: Lucide supplied by the preview renderer; implementation must pin/bundle its chosen release. No production CDN dependency is required.
