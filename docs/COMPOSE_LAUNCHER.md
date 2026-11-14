# Compose Launcher

As of 2026-11-14, oxishell 0.1.3. Status: proposed, not built yet.

Clickable mockup: https://claude.ai/artifact/RZUTJ6wBYP43r5kZgNkhna

## Summary

A button in each pane, left of the drag handle, lists every Docker Compose file in the repo. Clicking a file
types and runs `docker compose -f <file> up` in that pane, so nobody has to remember paths or flags. While a
stack is up, a bar under the terminal shows its services with **rebuild**, **restart** and **stop**, and
oxishell sends a desktop notification if a service crashes.

This is an oxishell feature and it is optional. It is on by default and can be turned off in Settings. When
it is off, or when the repo has no compose files, the button is not shown at all.

Effort: about 6.5 days for one person, the top of a medium (M). A first version that only lists files and
runs `up` takes about 2 days. The stack bar, crash notifications and Rebuild can follow as a second step of
about 2.5 days.

## Example

You open a pane in `~/code/shop`. The compose icon appears next to the drag handle because the repo has
`docker-compose.yml`. You press it (or `Ctrl+Shift+L`), pick `docker-compose.yml` and press Enter. The pane
runs `docker compose -f docker-compose.yml up`. The compose icon greys out because the pane is busy, and a bar
appears under the terminal:

```
● docker-compose.yml  4/4 up │ 02:13 │ attached            [rebuild] [restart] [stop]
● api :8000   ● web :5173   ● postgres :5432   ● redis :6379
```

## Settings

The feature is on for a new install. A config file without a `[compose]` section still loads, with the
feature on.

```toml
# ~/.config/oxishell/config.toml
[compose]
enabled = true          # false hides the button, the bar and Ctrl+Shift+L, and skips the scan
run_immediately = true  # false = type the command and wait for Enter
notify_on_crash = true
default_action = "up"   # what clicking a file name runs
command = ""            # empty = auto-detect docker compose, docker-compose or podman compose
```

Settings page rows, using the existing `switch` helper in `src/ui/settings.rs`:

- **Docker Compose launcher** (main switch): "Show a button in each pane that lists the repo's compose files
  and runs them."
- **Run the command right away**: "Off: type the command and wait for Enter." Greyed out when the main switch
  is off.
- **Notify when a service crashes**: "Desktop notification for a non-zero exit or an unhealthy service."
  Greyed out when the main switch is off.

When the button appears:

| Setting | Compose files in repo | Button | Bar under terminal | Ctrl+Shift+L | Scan |
|---|---|---|---|---|---|
| On | Found | Shown | While a stack or logs runs | Opens the list | When the pane's folder changes |
| On | None | Hidden | Hidden | Does nothing | When the pane's folder changes |
| Off | Any | Hidden | Hidden | Does nothing | Never runs |

Turning the feature off while a stack is running only hides the button and the bar. It never stops
containers or interrupts what is running in the pane.

## Pane button

- An icon button in the pane's `actions` box, before the drag handle and close button
  (`src/ui/pane.rs`, where `handle` and `close_btn` are added). Same flat, circular style as those two.
- No count badge. Whether the button is shown already tells you the repo has compose files.
- Tooltip: "Compose files in this repo (Ctrl+Shift+L)".
- **Disabled while the pane is busy.** If any program is in the foreground, the button greys out with the
  tooltip "Busy: wait for the command to finish". This includes commands the launcher started itself:
  `up` and `logs -f` keep running, so the button stays off until they end. `up -d` and `down` return to the
  prompt, so the button stays on after them.
- Busy means the foreground process group of the PTY (`meta::foreground_pgrp`) is not the shell. The meta
  worker refreshes this every 3 s for the visible workspace. Check it again on click, because
  `tcgetpgrp` is cheap enough to call on the GTK thread.

## File list

A popover under the button. It is drawn like terminal output: the terminal's font, inverse colours for the
selected row, `│` separators and one key per action. Only the pane button itself is a normal GTK icon button.

Each row shows the file name, its folder relative to the repo root, and its services (for example
`4 services · api, web, postgres, redis`). The root file comes first, then the rest sorted by path.

Keys:

| Key | Action |
|---|---|
| `↑` `↓` or `j` `k` | Move the selection |
| `Enter` or `u` | `up` (or `default_action`) |
| `d` | `up -d` |
| `x` | `down` |
| `l` | `logs -f` |
| `/` | Focus the filter |
| `r` | Rescan |
| `Esc` | Close |

Letters act as keys only outside the filter, so single keys do not clash with typing a filter (the same rule
as lazygit). Each row also has clickable `d -d`, `x down` and `l logs` buttons.

`Ctrl+Shift+L` opens the list for the focused pane. It is not used by oxishell today. Register it as an app
action with this default accelerator so it can be changed in the `[shortcuts]` table like other actions.

## Running a command

- Command: `<compose command> -f '<path>' <action>`, run from the repo root. Quote the path for the shell when
  it has anything other than `A-Za-z0-9_./-`.
- Compose command: the `command` setting, or the first that works of `docker compose`, `docker-compose`,
  `podman compose`. Detect once and cache it.
- Send it with `Pane::send_input` (`src/ui/pane.rs`), followed by `\n` when `run_immediately` is on. This is
  the same path paste uses, and it queues input if the shell is still starting.
- The button is disabled while the pane is busy, so the command never goes to another program.

## Stack bar

Shown under the terminal while a stack started from the launcher has running containers. It uses the
terminal's font and reads like a tmux status line.

- **Status line:** state symbol, file name, `4/4 up`, uptime, and `attached` (started with `up` in this
  pane) or `detached` (started with `-d`).
- **Service chips:** one per service with its published port, for example `● api :8000`. `●` running,
  `↻` restarting, `✗` exited. Clicking a chip restarts only that service.
- **[restart]:** `docker compose -f <file> restart`.
- **[stop]:** `docker compose -f <file> stop`. Stop keeps the containers; `down` stays a row action in the
  list. An attached `up` exits by itself when its containers stop, so the pane returns to the prompt.
- **[rebuild]:** see below.

Restart and stop run as separate background processes, not typed into the pane, because an attached `up`
holds the pane. Report a failure in the bar (exit code and the last line of stderr).

### Logs line

When `logs -f` runs in the pane, a second line appears above the stack bar:

```
≡ logs -f  docker-compose.yml │ following                              [stop logs]
```

**[stop logs]** sends Ctrl+C to the pane. That ends only the log stream; the containers keep running. The
name differs from **[stop]** on purpose, so nobody stops the stack when they only meant to close the logs.

### Rebuild

Rebuild runs in the foreground, without `-d`, so the build output and logs stay in the pane:

1. If an attached `up` holds the pane, send Ctrl+C and wait for the shell prompt to come back.
2. Type `docker compose -f <file> up --build` and press Enter.
3. The stack is now attached to the pane and the uptime starts again.

A stack started with `-d` has a free pane, so step 1 is skipped. If some other program holds the pane, open
the rebuild in a new split instead.

### Status

Poll `docker compose -f <file> ps --format json` every 3 s, only while the bar is shown, on the meta worker
thread. Parse with `serde_json` (already a dependency). Hide the bar when nothing is running, including when
the stack was stopped outside oxishell.

## Crash notifications

Compare each `ps` result with the previous one. Notify when a service goes from running to exited with a
non-zero exit code, or when its health turns `unhealthy`. A clean exit with code 0 does not notify.

- Always mark the chip red (`✗`) in the bar and show `1 exited` in the status line.
- Send a desktop notification, through `src/ui/desktop_notify.rs`, only when the oxishell window is not
  focused or the pane is in another workspace, and only once per crash.
- Title: `api exited with code 1`. Body: `<workspace> · <file>`. Actions: **Restart api** and **Dismiss**.
- Off when `notify_on_crash = false`.

## Scanning

- Start folder: the pane's current folder (`meta::cwd_of`), then walk up to the git root
  (`git rev-parse --show-toplevel`). Without a git repo, use the current folder.
- Walk up to 4 levels deep. Skip `.git`, `node_modules`, `target`, `vendor`, `.venv` and hidden folders.
- Match `compose.yaml`, `compose.yml`, `docker-compose.yaml`, `docker-compose.yml` and
  `docker-compose.*.yml` / `docker-compose.*.yaml` (overrides such as `docker-compose.override.yml`).
- Read the top-level `services:` keys with `yaml-rust2` (already used in `src/theme.rs`). A file that does not
  parse still appears, without a service list.
- Cap the list at 50 files.
- Run on the meta worker thread. The worker already notices when a pane's folder changes; scan again only
  when the git root changes, and cache the result per root. The **r rescan** key scans on demand.

## Code

| File | Change |
|---|---|
| `src/compose.rs` (new) | Scanner, YAML service list, command builder and quoting, compose binary detection, `ps` JSON parsing, crash detection |
| `src/config.rs` | `Compose` struct with a manual `Default` (`enabled`, `run_immediately` and `notify_on_crash` true), field on `Config` |
| `src/ui/pane.rs` | Icon button, popover list and keys, stack bar, logs line, busy state |
| `src/ui/meta_worker.rs` | Git root lookup, scan on root change, `ps` polling while a bar is shown |
| `src/ui/desktop_notify.rs` | Crash notification with a Restart action |
| `src/ui/settings.rs` | Three switch rows |
| `src/ui/style.css` | Popover and bar styles in the terminal's font |
| `src/ui/icons.rs` | Compose icon name with a fallback, like `pane_move()` |
| `docs/CONFIGURATION.md`, `README.md` | Document the feature and the `[compose]` section |

`Config` uses `deny_unknown_fields`, and `#[derive(Default)]` would make the booleans `false`. Write
`impl Default for Compose` by hand, as `Notifications` does, so a config without `[compose]` gets the
feature on.

## Tests

- **Scanner** (unit, temp folder tree): finds root and nested files, respects the depth limit, skips
  `node_modules` and `.git`, caps at 50, sorts the root file first.
- **Service list** (unit): valid file, file without `services:`, invalid YAML.
- **Command builder** (unit): plain path, path with spaces and quotes, each action, each compose command.
- **Config** (unit): a file without `[compose]` loads with `enabled = true`; `enabled = false` round-trips; an
  unknown key in `[compose]` is rejected.
- **`ps` parsing and crash detection** (unit): running → exited with code 1 notifies once; exit code 0 does
  not notify; `unhealthy` notifies.
- **e2e** (`tests/interactions.rs`): with the feature on and a compose file in the pane's folder, the button
  is visible and choosing a file sends the expected input to the pane. With the feature off, or with no
  compose files, the button is hidden.

## Effort

| Piece | Days |
|---|---|
| Button and popover, keyboard list | 1.5 |
| Scanner and YAML service list | 0.75 |
| Stack bar and logs line | 0.75 |
| Stack status polling | 0.5 |
| Crash notifications | 0.5 |
| Run logic (quoting, compose detection, busy state) | 0.5 |
| Config and Settings rows | 0.5 |
| Rebuild | 0.25 |
| Tests | 1.0 |
| Docs | 0.25 |
| **Total** | **6.5** |

## Later ideas

- Docker badge on the workspace in the sidebar while a stack is up.
- Click a port chip to open `localhost:<port>` in the browser.
- Open a shell in a service (`exec <svc> sh`) in a new split.
- Running dot on each file in the list.
- Pick `--profile` and `.env` files.
- Run one service only.
- Show the same list in the command palette.
- Use the same popover for Makefile targets and `package.json` scripts.
