# Coding agents

Any terminal agent runs in an oxishell pane like any other program. What the integration adds:

1. **Identification (no setup)** — the foreground process name is matched against known agents
   (`claude`, `codex`, `opencode`, `gemini`, `qodercli`, `cursor-agent`, `amp`, `aider`). The sidebar then
   shows e.g. "Claude Code (running)". This only says which program is in the foreground; it does not
   claim to know whether the agent is thinking or waiting.
2. **State and notifications (hooks)** — agents that offer hooks report real events. oxishell shows
   the state, rings the pane, badges the workspace and sends a desktop notification.
3. **Bell** — agents configured to ring the terminal bell (e.g. Claude Code's terminal-bell notification
   channel) mark the pane for attention without any hooks.

## Hooks

```bash
oxi hooks list                 # supported agents and whether they are on PATH
oxi hooks install --all        # install for each supported agent found on PATH
oxi hooks install claude
oxi hooks uninstall claude
```

| Agent | File changed | Events → state |
|---|---|---|
| Claude Code | `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR`) | UserPromptSubmit → running, Notification → waiting, Stop → done, SessionEnd → ended |
| Gemini CLI | `~/.gemini/settings.json` | BeforeAgent → running, AfterAgent → done, SessionEnd → ended |
| Qoder CLI | `~/.qoder/settings.json` | Stop → done, SessionEnd → ended |
| Codex CLI | `~/.codex/config.toml` (or `$CODEX_HOME/config.toml`, inline `hooks`) | UserPromptSubmit / PreModelRequest / PostToolUse → running, PermissionRequest / request_user_input PreToolUse → waiting, Stop → done, Interrupt / SessionEnd → ended |
| OpenCode | `~/.config/opencode/plugins/oxishell.js` | session.status busy → running, session.idle → done, permission → waiting |

Event names use each agent's native hook system. Notes:

- Install is idempotent and keeps your existing hooks; only entries tagged `oxishell-hook` are added or
  removed. The first install backs up the original file as `*.oxishell.bak`. Invalid JSON/TOML is never
  overwritten.
- Codex installation preserves user-defined `notify` programs and inline hooks. It replaces the old
  oxishell-managed `notify` entry with lifecycle hooks to avoid duplicate completion alerts.
- The hook command exits 0 silently when not running inside oxishell, or if the app is not reachable, so
  hooks never break the agent.
- Only states the agent's hook system emits are shown. Qoder's state appears when a turn finishes.

### Codex setup

Use a Codex CLI with [lifecycle hook support](https://learn.chatgpt.com/docs/hooks) (configuration checked
with Codex CLI 0.167.0). Run:

```bash
oxi hooks install codex
codex
# In Codex, open /hooks and review and trust the oxi hooks.
```

Restart an already-running Codex after installation. Codex skips untrusted hooks; oxishell does not alter
hook trust or permission policies. If you explicitly disabled hooks with `[features] hooks = false`,
that setting is preserved. Hooks in a separate `hooks.json` are preserved too; Codex merges them with
inline hooks and may warn about using both representations.

The sidebar shows Running while working, Needs input for approvals and `request_user_input` questions,
and Done with the final response preview when the turn stops. Tool completion or a new model request
restores Running after input. Interrupting or ending the session clears the hook status without a
finished notification. Session ids from hooks enable the existing explicit `codex resume` action.

For an older Codex CLI without lifecycle hooks, keep the completion-only integration by adding this at
the **top level** of `config.toml` instead of installing lifecycle hooks (use your absolute `oxi` path):

```toml
notify = ["/absolute/path/to/oxi", "agent-event", "--agent", "codex", "--marker", "oxishell-hook", "--codex-json"]
```

If you already have a notify program, keep it and call
`oxi agent-event --agent codex --codex-json "$1"` from that script. The legacy JSON argument format
remains supported. CLI/GTK lifecycle tests use hook fixtures; a live authenticated Codex conversation
is still a manual check.

## Session resume

When a hook reports a session id (Claude/Codex `session_id`, legacy Codex `thread-id`, ...) it is saved with the
session. After a restart, "Resume <agent> session" appears in the pane's context menu and in the command
palette. Choosing it **types** the resume command (e.g. `claude --resume <id>`) into the pane; you press
Enter. Session ids are validated (letters, digits, `-`, `_`) before being used in a command. Nothing is
resumed automatically.

## Other agents

Any tool can report state:

```bash
oxi agent-event --agent mytool --state waiting --message "Needs approval"
oxi notify -t "mytool" -b "Finished"
```
