# Menu review and final candidate

Review of the integrated browser candidate against shared D1–D14, TaskQ M/I/K/C/MP contracts and accepted functional defaults. Not a fresh native-product audit.

| Finding | Correction | Rule |
|---|---|---|
| Fixture selectors interrupt questions and normal settings | One preview-tools toggle outside the product; diagnostics hidden by default, preserved for tests | D4/D13 |
| Repeated context and draft explanations compete with actions | Short labels; provenance, capabilities and long explanations in Details | D4/D9 |
| Three view modes crowd the admission strip | Stable view controls on first row; admission/pause/stop aligned together below when necessary | D2/D8 |
| Ordinary selections use bordered cards | Border only for composer/change review; section lines for current context | D4 |
| Custom editing would diverge from Summary | One schema/renderer; immutable built-in Summary and editable Custom copy | CI1 |
| Settings and lifecycle actions need recognizable navigation | Keep category icons and short labels; icon-only for unambiguous add, attach, edit, back and window actions | D11/D14 |

## Wording and structure

- Keep labels that disambiguate effects: Pause, Stop, Access, Delivery, Save/Use, Start and Custom. Do not replace distinct actions with identical unlabeled symbols.
- Use `Chats` and `Sessions` in workspace navigation, retaining accessible contextual names. Use `Review`, `Apply`, `Start` only inside a section with an unambiguous target/effect summary.
- Questions retain exact numbered source text and reason. Do not shorten user-authored options for cosmetic density.
- Preview diagnostics are outside ordinary product flow; unknown, stale, failure and unapplied state remain visible product information.
- Preserve every accepted route: AI/files/history/windows, questions, projects/board/session, Work mode/resources, Access/Delivery, settings/services/setup and recovery.

Existing shared rules already cover wording, icons and hierarchy; no duplicate TaskQ rule is added upstream. The custom renderer and its protocol are TaskQ-specific.
