# `cmux` CLI

`cmux` talks to the running app over `$XDG_RUNTIME_DIR/cmux-linux/cmux.sock` (override with
`--socket` or `CMUX_SOCKET`). Inside a cmux-linux pane, `CMUX_PANE_ID`, `CMUX_WORKSPACE_ID` and
`CMUX_SOCKET` are set, and pane arguments default to the current pane. Add `--json` for raw output.

| Command | Effect |
|---|---|
| `cmux ping` | Check the app is running |
| `cmux identify` | Show this shell's pane/workspace |
| `cmux workspace list` | List workspaces (`*` = active) |
| `cmux workspace new [--name N] [--cwd DIR]` | Create and select a workspace |
| `cmux workspace select ID` / `next` / `prev` | Switch workspace |
| `cmux workspace rename ID NAME` | Rename |
| `cmux workspace move ID INDEX` | Reorder |
| `cmux workspace close ID [--force]` | Close; refuses if programs are running unless `--force` |
| `cmux pane list [--workspace ID]` | List panes with cwd, git, ports, agent state |
| `cmux pane split [--pane ID] [-d right\|down] [--cwd DIR]` | Split a pane |
| `cmux pane focus ID` | Focus a pane (switches workspace) |
| `cmux pane close ID [--force]` | Close; refuses if a program is running unless `--force` |
| `cmux pane send [--pane ID] [--enter] TEXT` | Type text into a pane (**disabled by default**) |
| `cmux pane read [--pane ID] [--lines N]` | Print the last N lines (plain text) |
| `cmux notify [-t TITLE] [-b BODY] [--pane ID] [--global]` | Post a notification |
| `cmux notifications list` / `clear` / `jump` | History, clear, focus latest unread |
| `cmux agent-event --agent A --state running\|waiting\|idle\|ended` | Report agent state (used by hooks) |
| `cmux hooks install AGENT\|--all`, `hooks uninstall AGENT`, `hooks list` | Manage agent hooks |

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
