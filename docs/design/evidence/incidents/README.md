# Incident operator evidence

Native Windows WebView2 captures from isolated synthetic fixtures, generated with
`docs/design/incident-fixture.py` and `taskq tray --snapshot`. No service was started
and no report was posted. Project `alpha`, host `fixture`, repository and report
identifiers are fixture data. Each CLI artifact is the complete `taskq service status`
output for the corresponding fixture, including the raw status JSON.
`Update: unknown` means no checked update is recorded in these fixtures.

| State | Windows menu | Service status |
|---|---|---|
| Sent | [Menu](sent-project-light.png) | [Text](sent-status.txt) |
| Local | [Menu](local-project-light.png) | [Text](local-status.txt) |
| Unsupported authentication | [Menu](unsupported-auth-project-light.png) | [Text](unsupported-auth-status.txt) |
| Disabled | [Menu](disabled-project-light.png) | [Text](disabled-status.txt) |
| None | [Menu](none-project-light.png) | [Text](none-status.txt) |
| Existing-project notice | [Menu](notice-project-light.png) | [Text](notice-status.txt) |

Local reports use neutral text, separate from failed tasks; their actions open the
sanitized report or the new-issue page. The notice discloses public reporting and
requires an owner decision; it has no action that silently enables sending.

Both Node tray checks and focused Rust incident/setup checks passed. These captures
prove Windows rendering with fixture data, not delivery, native action execution,
accessibility, or macOS behavior. The menu is scrollable; a capture shows its visible
viewport.
