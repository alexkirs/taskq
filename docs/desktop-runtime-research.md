# Desktop runtime research (#770)

Question: can a desktop app session (Claude Desktop Code tab, Codex Desktop) be a taskq runtime for tasks that need
the screen? A `releases/<ver>/taskq` child never holds macOS Screen Recording; an app session runs under the app's own
grant.

Sources: static reading of the shipped app bundles, official docs, and five disposable experiments on one macOS host
(owner answer 1 on #770). Unknown means not proven here (R12). Nothing was changed in runtimes, permissions, accounts or
settings.

## Versions

| Item | Version |
|---|---|
| macOS | 27.0.1 |
| Claude Desktop | 2.31226.1, Code tab CLI pinned 2.1.295 |
| Claude Code CLI | 2.1.296 |
| Codex Desktop (`com.openai.codex`) | 26.1007.30912 |
| Codex CLI | 0.159.3 |
| Codex Computer Use service | 26.1005.1001425 |
| Windows | not tested: the Windows host was not reachable during this attempt |

## Experiments (macOS)

| # | Step | Observed |
|---|---|---|
| E1 | `codex exec --json` in a scratch repo, brief: name your screen tools | thread id from `thread.started`; answer `NONE`; the rollout has no computer-use or screen tool |
| E2 | `open "claude://code/new?q=<brief>&folder=<scratch>"` | no new Code session record within a minute: the prompt is not sent |
| E3 | `claude -p --session-id <uuid>`, then `open "claude://resume?session=<uuid>"` | Desktop log: `Imported CLI session <uuid> as Desktop session local_<uuid>`; record has `cliSessionId` = `<uuid>`, `permissionMode: auto`, CLI pin 2.1.295 |
| E4 | `codex queue --thread <E1 id> --message ...`, then `open codex://threads/<E1 id>` | queue emptied; the app ran the turn in the same thread within 7 s (`task_complete PONG2`) |
| E5 | `codex queue` again while the thread is open in the app | the app ran it without another open; the turn lists `mcp__codex_app__capture_screen_context` |

In E4 and E5 the app kept the thread's model, sandbox and approval policy from the `exec` turn (same `turn_context`).

## Matrix

P = proven here (experiment or app bundle code, version above). D = official docs only. U = unknown.

| Capability | Claude Desktop macOS | Claude Desktop Windows | Codex Desktop macOS | Codex Desktop Windows |
|---|---|---|---|---|
| Start with a brief | No unattended path. `claude://code/new?q=` only fills the composer (P E2; D: "never sends it for you"). `claude --desktop --resume <id>` or `claude://resume?session=<id>` moves a CLI session in without a new turn (P E3; D v2.1.285+). Scheduled tasks run a fixed prompt, but taskq would write the app's config. | U; D: `claude --desktop` exists on x64 | Yes, two steps: `codex exec` handshake turn, then `codex queue` with the brief while the thread is open in the app (P E1, E4) | U |
| Session id | `local_<cli uuid>`; the CLI uuid is ours from `--session-id` (P E3) | U | thread id from `thread.started`, the same in the app (P E1, E4) | U |
| Answer or steer | No external path: the in-app `ccd_session_mgmt send_message` tool is reachable only from another Desktop session (P bundle); D: messages cannot be delivered into an unwatched session | U | `codex queue --thread <id> --message <text>`; an open thread runs it without another open (P E5) | U |
| Exit detection | Session record `completedTurns` and the log line `[Stop hook] Query completed for session local_<id>` (P read only) | U | rollout `task_complete` event and the empty queue row (P E4, E5) | U |
| Screen and computer use | Code sessions get `mcp__computer-use` (P bundle); D: research preview, Pro or Max, off by default, needs Accessibility and Screen Recording for Claude.app; grant state U | D: supported, toggle takes effect at once; U here | App turns have `capture_screen_context` (P E5); computer use runs in `Codex Computer Use.app` with its own grant (P bundle, D); grant state U | D: supported, cannot run in the background in the same Windows session; U here |
| Permissions and R9 | Imported session runs `auto`; D: `dontAsk` is CLI only. taskq's default cannot be kept without the owner's choice | U | model, sandbox and approval of the `exec` turn are kept (P E4, E5), so `codex` options in `taskq.json` apply | U |
| App closed or user logged out | D: computer use needs the app running; sessions are app children. Not tested | U | U: queued rows persist in the app's local store, but who runs them when the app is closed is not tested | U |

## Recommendation

Feasible today on one path only: **Codex Desktop on macOS**. Claude Desktop has no way to start or steer a turn from
outside, and it cannot run taskq's `dontAsk` mode, so it is not a runtime; its nearest alternative is the Codex path, or
an owner who presses Send on a prefilled `claude://code/new` link.

Codex path:
1. The service spawns `codex exec --json <codex opts> -C <root> -` with a one-line handshake brief; the thread id comes
   from `thread.started` as today. The child exits after the handshake.
2. `open codex://threads/<id>` (Windows: unknown).
3. `codex queue --thread <id> --message <brief>`; the app runs the turn under its own screen grant.
4. `answer` and `steer` go through the same `codex queue` call.

Cost and limits:
- Needs a logged-in GUI session with the Codex app running; one host, and the app shows the thread to the user.
- `codex queue` takes the brief as an argument, not stdin; § 8 wants stdin. Stdin support is unknown: a spawn with no
  shell keeps the argument safe, but the rule needs an exception or a stdin form.
- `codex://threads/<id>` is not in the official docs; it is the link taskq already prints (R6).
- The attempt has no child process, so the lease needs its own rule (below).
- Untested before adoption: app closed with a queued row, logout, Windows, the screen grant of `Codex Computer Use`.

## Draft contract text

§ 8 row:

```
| Codex app | `codex exec --json <codex opts> -C <root> -` with a handshake brief; session from `thread.started`; then `open codex://threads/<id>` and `codex queue --thread <id> --message <brief>` | `codex queue --thread <id> --message <text>` |
```

§ 3 lease rule:

```
- **App lease.** An attempt of runtime `codex-app` has no child: the owning service lists `#<N> <id>` while the Codex
  app runs in this host's logged-in session and the thread's last turn has no `task_complete`. A completed turn without
  `result`, `ask` or `requeue`, a closed app or a logout counts as a child exit: `gone attempt <id>`. Only hosts whose
  `has` lists `screen` admit `needs-screen` tasks for it.
```

## Leftovers

Disposable artifacts, no settings: one Claude Code session imported into Claude Desktop (scratch folder), one Codex thread
visible in the Codex app, a possibly prefilled composer from E2, and scratch folders under the temp directory.
