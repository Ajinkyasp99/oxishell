# Coding agents

Any terminal agent runs in a cmux-linux pane like any other program. What the integration adds:

1. **Identification (no setup)** — the foreground process name is matched against known agents
   (`claude`, `codex`, `opencode`, `gemini`, `qodercli`, `cursor-agent`, `amp`, `aider`). The sidebar then
   shows e.g. "Claude Code (running)". This only says which program is in the foreground; it does not
   claim to know whether the agent is thinking or waiting.
2. **State and notifications (hooks)** — agents that offer hooks report real events. cmux-linux shows
   the state, rings the pane, badges the workspace and sends a desktop notification.
3. **Bell** — agents configured to ring the terminal bell (e.g. Claude Code's terminal-bell notification
   channel) mark the pane for attention without any hooks.

## Hooks

```bash
cmux hooks list                 # supported agents and whether they are on PATH
cmux hooks install --all        # install for each supported agent found on PATH
cmux hooks install claude
cmux hooks uninstall claude
```

| Agent | File changed | Events → state |
|---|---|---|
| Claude Code | `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR`) | UserPromptSubmit → running, Notification → waiting, Stop → done, SessionEnd → ended |
| Gemini CLI | `~/.gemini/settings.json` | BeforeAgent → running, AfterAgent → done, SessionEnd → ended |
| Qoder CLI | `~/.qoder/settings.json` | Stop → done, SessionEnd → ended |
| Codex CLI | `~/.codex/config.toml` (`notify` program) | agent-turn-complete → done |
| OpenCode | `~/.config/opencode/plugins/cmux-linux.js` | session.status busy → running, session.idle → done, permission → waiting |

Event names follow upstream cmux's hook catalog. Notes:

- Install is idempotent and keeps your existing hooks; only entries tagged `cmux-linux-hook` are added or
  removed. The first install backs up the original file as `*.cmux-linux.bak`. Invalid JSON/TOML is never
  overwritten.
- Codex supports a single `notify` program. If you already have one, install refuses; call
  `cmux agent-event --agent codex --codex-json "$1"` from your own script instead.
- The hook command exits 0 silently when not running inside cmux-linux, or if the app is not reachable, so
  hooks never break the agent.
- Only states the agent's hook system emits are shown. Codex and Qoder do not report "running", so their
  state appears when a turn finishes.

## Session resume

When a hook reports a session id (Claude `session_id`, Codex `thread-id`, ...) it is saved with the
session. After a restart, "Resume <agent> session" appears in the pane's context menu and in the command
palette. Choosing it **types** the resume command (e.g. `claude --resume <id>`) into the pane; you press
Enter. Session ids are validated (letters, digits, `-`, `_`) before being used in a command. Nothing is
resumed automatically.

## Other agents

Any tool can report state:

```bash
cmux agent-event --agent mytool --state waiting --message "Needs approval"
cmux notify -t "mytool" -b "Finished"
```
