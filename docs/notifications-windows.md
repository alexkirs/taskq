# Windows notifications: bounded evidence for #1048

Contract: [taskq.md §4](../taskq.md#4-service), [M14 / M-A22](design/menu.md#m14-notifications).
The owner answer on [#1048](https://github.com/taskqorg/taskq/issues/1048) moves native click and OS-denial evidence to [#1049 Windows G8](https://github.com/taskqorg/taskq/issues/1049).

Windows NT 10.0.26100, rustc 1.99.0, x86_64-pc-windows-gnu:

- `cargo test --locked --bin taskq notify:: -- --nocapture`: 7 passed. Shared modes, per-project overrides, persisted once-per-event cursor, title preservation and burst replacement pass on Windows.
- Native WinRT XML and ToastNotification construction preserve Unicode, XML-sensitive text, sound choice and replacement tag/group. Invoking the production COM activation delegate preserves the exact card payload; this is API evidence, not a user click.
- Native ShellLink / property-store / persist APIs create and read back the TaskQ identity in OS temp, reopen an owned shortcut, and refuse an unrelated identity without changing its bytes. Initial read-only loading caused 0x80030005 on save; read/write loading fixes the same check.
- Permission mapping covers Enabled, all four OS-denial values, unknown values and API errors. No real OS denial was applied.
- `node tests/tray-navigation.test.cjs`: notification settings, Blocked UI, project overrides and closed-task card routing pass.

No toast was shown, Start Menu shortcut installed, OS setting changed, working tray restarted or release deployed. Native delivery, sound, banner/Notification Center clicks (including tray restart), and real denial/revocation remain **unknown** until #1049 G8 runs the installed candidate. An API pass does not qualify these flows. The shortcut targets the stable installed tray; activation is handled by the running tray.
