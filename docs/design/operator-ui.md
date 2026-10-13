# TaskQ operator UI acceptance

Apply [shared D1–D14](https://github.com/taskqorg/specq/blob/main/design/operator-ui.md).
Apply [shared implementation fidelity F1–F6](https://github.com/taskqorg/specq/blob/main/design/implementation-fidelity.md) and [native acceptance](ui-acceptance.md).
This file contains TaskQ-specific checks, not a second definition of the shared principles.
[Contract](../../taskq.md#product) · [Menu](menu.md) · [Square summary](icon-summary.md).

## Product boundaries

- Menu behavior, renderer, registry, AI, navigation and maintenance belong to M1–M12 in the menu specification.
- Square geometry, font calibration and platform qualification belong to I1–I5 in the icon specification.
- Menu counters use nonzero symbol capsules; zero leaves no slot. The summary icon retains zero in its fixed positions (M1/M3).
- Task rows keep issue numbers and host names intact; long titles wrap under metadata from the block's left edge (M3).
- Specifications are design targets. Runtime follows taskq.md until implementation and verification establish otherwise.

## Acceptance

| ID | TaskQ evidence |
|---|---|
| D-A1 | Status, issue number and title start in one flow; continuation uses the full block width. |
| D-A2 | The task body opens its issue. Separate session/PR links do not also open the issue. Project identity stays unambiguous. |
| D-A3 | Menu font changes apply throughout and persist; icon sizing remains independent. |
| D-A4 | At 320/360 px, long titles produce no collisions or horizontal scrolling. |
| D-A5 | Both themes at 100/200% scaling preserve the grid and visible-edge alignment. |
| D-A6 | Meaning survives without color; keyboard, screen reader and touch targets work. |
| D-A7 | The menu contains no empty groups or redundant content. |
| D-A8 | Compact counters hide 0; symbols with 1/12/100 fit; unknown stays visible. Wrap as a group under K rules, never stretch across empty width. |

General improvements belong in specq. TaskQ behavior belongs in taskq.md and its linked specifications.
Preserve these acceptance IDs when refining product checks.
