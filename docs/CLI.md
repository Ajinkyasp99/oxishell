# `oxi` CLI

`oxi` talks to the running app over `$XDG_RUNTIME_DIR/oxishell/oxishell.sock` (override with
`--socket` or `OXISHELL_SOCKET`). Inside an oxishell pane, `OXISHELL_PANE_ID`, `OXISHELL_WORKSPACE_ID` and
`OXISHELL_SOCKET` are set, and pane arguments default to the current pane. Add `--json` for raw output.

| Command | Effect |
|---|---|
| `oxi ping` | Check the app is running |
| `oxi identify` | Show this shell's pane/workspace |
| `oxi workspace list` | List workspaces (`*` = active) |
| `oxi workspace new [--name N] [--cwd DIR]` | Create and select a workspace |
| `oxi workspace select ID` / `next` / `prev` | Switch workspace |
| `oxi workspace rename ID NAME` | Rename |
| `oxi workspace move ID INDEX` | Reorder |
| `oxi workspace set ID [--color C] [--icon I]` | Sidebar colour (red, yellow, green, cyan, blue, magenta, gray) and icon (terminal, server, database, web, tools, docs, science, star, lock, cloud, warning, home); `none` clears |
| `oxi workspace close ID [--force]` | Close; refuses if programs are running unless `--force` |
| `oxi pane list [--workspace ID]` | List panes with cwd, git, ports, agent state |
| `oxi pane split [--pane ID] [-d right\|down] [--cwd DIR]` | Split a pane |
| `oxi pane focus ID` | Focus a pane (switches workspace) |
| `oxi pane close ID [--force]` | Close; refuses if a program is running unless `--force` |
| `oxi pane send [--pane ID] [--enter] TEXT` | Type text into a pane (**disabled by default**) |
| `oxi pane read [--pane ID] [--lines N]` | Print the last N lines (plain text) |
| `oxi notify [-t TITLE] [-b BODY] [--pane ID] [--global]` | Post a notification |
| `oxi notifications list` / `clear` / `jump` | History, clear, focus latest unread |
| `oxi agent-event --agent A --state running\|waiting\|idle\|ended` | Report agent state (used by hooks) |
| `oxi hooks install AGENT\|--all`, `hooks uninstall AGENT`, `hooks list` | Manage agent hooks |

## Safety

- The socket is mode 0600 inside a 0700 directory, and every connection's peer uid is checked
  (`SO_PEERCRED`). There is no network listener.
- `pane send` types into a shell, so it is off until you set `[ipc] allow_send = true` (or enable it in
  Settings). Text is limited to 64 KiB per request and is never sent with an implicit Enter.
- Requests are validated (method allow-list, typed parameters, size limit 1 MiB) before they reach the UI.

## Protocol

One JSON object per line: `{"id":1,"method":"pane.split","params":{"pane":2,"direction":"down"}}` →
`{"id":1,"ok":true,"result":{"pane":5}}`. Methods: `ping`, `workspace.{list,create,select,next,prev,rename,move,close}`,
`pane.{list,split,focus,close,send,read}`, `notify`, `agent.event`, `notification.{list,clear,jump}`.
