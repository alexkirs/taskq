# taskq operator design package

Approved future UI target; shipped behavior still follows [taskq.md](../../taskq.md).
Phase 0 decisions are recorded in [menu.md](menu.md), [operator-ui.md](operator-ui.md) and [implementation-brief.md](implementation-brief.md).

| Material | Purpose |
|---|---|
| [Contract](../../taskq.md) | Product rules; takes priority on conflict |
| [Principles](operator-ui.md) | Density, alignment, readability, accessibility; D criteria |
| [Board specification](board.md) | Approved labels, naming, attention-only palette, default adoption; B criteria |
| [Menu specification](menu.md) | Behavior, host scope, AI, maintenance, Add project; M criteria |
| [Menu prototype](menu-prototype.html) | Product HTML structure and fixture interactions; state/action bridge target |
| [Icon studies](icon-studies.html) | Selected unified strip geometry, themes, active/pause states, 22 pt macOS study |
| [Verification](verification.md) | Evidence and remaining qualification |
| [Implementation brief](implementation-brief.md) | Phased implementation target |

## Reading the prototypes

- Renderer: a popover window containing a wry webview (WKWebView on macOS, WebView2 on Windows), with a small Rust state/action bridge. The HTML is the product UI target; fixture data is not a service adapter.
- UI language: English, matching the CLI and README. No OpenAI widget state dependency.
- Windows: compact q only; summary and the M1 view chooser are hidden. macOS summary fits a 22 pt menu bar with about 12 pt digits and a 7 pt graph; the approved deviation from the 47×29 study preserves the geometry rules.
- Summary pause keeps the same numbers and bars in gray, with the compact q/Ⅱ centered on both axes. Icon geometry comes from `icon-studies.html`.
- One tray per host reads the installed-project registry `<install>/projects`. Observation means an installed service with focus off; ARM is service focus plus the host filter, with no separate host PM filter.
- Service data targets: local `<root>/.taskq/events` history (time, task, close|failed, attempt ID as event ID), original focus request via `--request`, and newer-version status with update action through `<root>/.taskq/update`. Incomplete history shows its start date; missing saved focus prompts for labels.
- M7 AI and Add project ship in phase 4, with the AI contract changes to R3/R9. Add project uses the OS folder picker and §6 onboarding inside the menu AI channel; there is no terminal or clipboard shortcut.
- The sole open owner question is the exact old-project marker; the proposal is `taskq.json` present with failing selftest. The four folder classifications and M-A14 are in the menu specification.
- Model tests do not prove native rendering, service integration, or AI. See verification for limits.

## Changes

General rule → `operator-ui.md`; menu behavior → `menu.md`; contract decision → `taskq.md` first (R13).
Then update the prototype and its checks. Preserve accepted features and acceptance IDs.
This task updates the delegated design target only: no Rust or §4 runtime changes.
This directory is the canonical package; older chat prototypes are not implementation sources.
