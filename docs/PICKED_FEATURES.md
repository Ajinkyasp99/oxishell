# Picked features plan

As of 2026-11-14, oxishell 0.1.3. These are the eight features picked from the feature menu, with what each
one does, an example, where it goes in the code, and how to test it.

## Summary

There are 5 small features, 2 medium and 1 large: roughly 3 to 4 weeks of work for one person. Build them in
this order. The small features come first, then the two medium features that reuse their helpers, and the
multi-window feature last because it touches the most code.

| Order | ID | Feature | Effort | Main files |
|---|---|---|---|---|
| 1 | T1 | Clickable file:line links | S | `src/ui/links.rs`, new `src/editor.rs` |
| 2 | A4 | Usage limit alerts | S | `src/usage.rs`, `src/ui/usage_footer.rs` |
| 3 | W4 | Workspace colour and icon | S | `src/model.rs`, `src/ui/sidebar.rs`, `src/session.rs` |
| 4 | L2 | Background opacity | S | `src/config.rs`, `src/ui/pane.rs`, `src/ui/style.css` |
| 5 | D4 | Update notice | S | `src/config.rs`, new `src/update_check.rs` |
| 6 | A6 | Send a failed block to an agent | M | `src/ui/blocks.rs`, `src/ui/pane.rs` |
| 7 | W7 | Git detail popover | M | `src/meta.rs`, `src/ui/sidebar.rs` |
| 8 | P9 | Workspace in its own window | L | `src/ui/mod.rs`, `src/model.rs`, `src/session.rs` |

T1 goes first because W7 reuses its editor opener. A4 comes early because it adds the "notify once per
threshold" helper.

## Status

As of 2026-11-14: T1, A4, W4, L2 and D4 are built and tested. A6, W7 and P9 are not started.

| ID | Status | Differences from this plan |
|---|---|---|
| T1 | Done | The file-exists check also runs on hover (a single `stat`), so no tooltip or popover appears for files that don't exist. More editors are auto-detected (`codium`, `subl`, `kate`, `gedit`). |
| A4 | Done | Logic lives in `src/usage_alerts.rs`, not `usage.rs`. A jump straight past both thresholds (79→96) sends one 95% alert, not two. Uses the existing `Message` notification kind instead of a new `Kind`. The body shows time left ("Resets in 1h 30m"), not a clock time. A reset more than an hour old is not announced. |
| W4 | Done | 7 colours (red, yellow, green, cyan, blue, magenta, gray) + none, not 8. Icons: terminal, server, database, web, tools, docs, science, star, lock, cloud, warning, home. The pane focus ring is **not** tinted, so it doesn't compete with the attention ring. The stripe is its own small widget, so the drag-and-drop highlight still shows on coloured rows. |
| L2 | Done | A spin row (50–100, steps of 5), not a slider. Below 100% the terminal is transparent and the window paints the background with alpha, instead of `set_color_background` with alpha. Themes with a background image stay opaque. |
| D4 | Done | A toast that stays until dismissed (Release notes + ✕), not an `adw::Banner`, which has room for only one button. The setting is in Settings → Notifications (there is no General page). User agent `oxishell/<version>`. HTTP client: `ureq` 2 with rustls. |

Related fixes:
- In the notifications panel, notifications without a workspace (usage alerts, `oxi notify --global`) said
  "closed"; they now show no workspace name.
- Command block headers (`~/folder ⎇ branch ✓ 0.01s`) could be drawn on top of a running program's output,
  such as Claude Code's input box. Blocks are stored at absolute terminal rows, and the offset to the
  scroll position was only re-measured at a shell prompt. When a running program dropped rows (`ESC[3J`
  clearing the scrollback, or output passing the scrollback limit), every older header slid down the
  screen. `src/ui/blocks.rs` now re-anchors the offset after any output by matching the visible text to
  VTE's rows, and draws no headers when it cannot. Two interaction tests reproduce both cases.

## Shared groundwork

Four pieces are used by more than one feature. Build each one with the first feature that needs it.

- **Editor opener** (`src/editor.rs`, with T1; reused by W7). `open_at(path, line, col)` picks a command in
  this order: the `[editor] command` setting, `$VISUAL`, `$EDITOR`, then `code`, `zed`, `gnome-text-editor`
  if they are on `PATH`. Known editors get their line syntax: `code -g file:line:col`, `zed file:line:col`,
  `vim +line file`, `nvim +line file`, `nano +line,col file`. A terminal editor (vim, nvim, nano, hx, micro)
  opens in a new split next to the pane instead of a detached process.
- **New config sections** in `src/config.rs`. `Config` uses `deny_unknown_fields`, so every new key needs a
  `#[serde(default)]` field and a parse test, or older config files fail to load.
  ```toml
  [editor]
  command = ""            # empty = auto-detect

  [notifications]
  usage_alerts = true     # A4

  [appearance]
  opacity = 100           # L2, 50–100

  [updates]
  check = false           # D4, off by default
  ```
- **Settings rows** in `src/ui/settings.rs` for each new key, using the existing switch/row helpers.
- **One-time notifications**. A4 and D4 both post a notification only once per event. They store a small
  "last notified" key (threshold + reset time, or version) in the state directory from `src/paths.rs`, so a
  restart does not repeat it.

## T1. Clickable file:line links

**Example.** `cargo build` prints `--> src/model.rs:230:9`. You click it and your editor opens `model.rs` at
line 230, column 9. Ctrl+click opens it straight away, like URLs.

**Behaviour**
- Matches `path:line` and `path:line:col`, where the path is relative (`src/a.rs`), `./`, `../`, `~/` or
  absolute. Also matches Python's `File "x.py", line 12`.
- A relative path is resolved against the pane's current folder (from `meta::cwd_of`). If the file does not
  exist, the text is not treated as a link, so `12:30:45` timestamps and `host:22` stay plain text.
- Clicking opens the same popover as URLs, with **Open in editor** and **Copy path**.

**Code**
- `links.rs`: add `FILE_PATTERN` next to `URL_PATTERN`. It must follow the same rule (flat character
  classes only, because of VTE's PCRE2 depth limit of 64). Register it with `match_add_regex`.
- `link_at` returns an enum `Link::Url(String) | Link::File { path, line, col }`; `open` dispatches to the
  browser or to `editor::open_at`.
- Do the file-exists check on click, not on hover, to keep hover cheap.

**Tests**
- Unit tests for parsing: `src/a.rs:1`, `src/a.rs:1:2`, `./a.rs:3`, `/tmp/a.rs:4`, `a.rs:12:30:45`
  (should give line 12, col 30), `File "x.py", line 12`.
- Unit tests for the editor argument builder (code, zed, vim, nano, unknown editor).
- An e2e test in `tests/interactions.rs`: print a path into a pane and check that a match is found at that cell.

## A4. Usage limit alerts

**Example.** You don't notice you are using up your Claude allowance. At 80% of the 5-hour window you get one
desktop notification: "Claude usage at 80% · 5-hour limit · resets 3:40 PM". At 95% you get a second one. When
the window resets, a quiet note says "Claude 5-hour limit reset".

**Behaviour**
- Thresholds are 80 and 95 for both windows (5-hour and weekly) and both agents (Codex, Claude).
- Each threshold fires once per window. The key is `(agent, window, threshold, resets_at)`, so a new window
  can alert again and a restart does not repeat an alert.
- The reset note fires only if an alert fired in that window.
- Uses normal urgency and a quiet sound, so it never looks like an agent waiting for input.
- Setting: Settings → Notifications → **Usage limit alerts** (on by default).

**Code**
- `usage.rs`: a pure function `crossed(prev: Option<&Window>, now: &Window) -> Vec<u8>` that returns the
  thresholds crossed. Easy to unit-test.
- `usage_footer.rs`: `attach` already receives every poll. Compare against the previous snapshot there and
  call the notification path in `notify.rs` with a new source such as `usage`.
- Add a `Kind` for usage in `notify.rs` so it gets its own sound/urgency/category.

**Tests**
- `crossed`: 79→81 gives `[80]`; 79→96 gives `[80, 95]`; 81→85 gives nothing; a new `resets_at` with 85
  gives `[80]`.
- The once-only store: the same key twice notifies once.

## W4. Workspace colour and icon

**Example.** You make `prod-ssh` red and `scratch` grey. The sidebar row shows a coloured stripe on its left
edge and a small icon before the name, so you see that you are in production before you type.

```
▌ prod-ssh     (red, server icon)
▌ api          (blue, code icon)
▌ scratch      (grey, no icon)
```

**Behaviour**
- Right-click a workspace → **Colour** (8 swatches + none) and **Icon** (about 12 symbolic icons: code,
  server, database, globe, terminal, book, flask, rocket, bug, lock, cloud, star).
- The colour also tints the pane focus ring in that workspace.
- Colours come from the current theme's ANSI palette (red, yellow, green, cyan, blue, magenta, plus two
  greys), so they always fit the theme.
- CLI: `oxi workspace set ID --color red --icon server`.

**Code**
- `model.rs`: add `color: Option<String>` and `icon: Option<String>` to `Workspace`, with
  `#[serde(default)]` so old session files still load.
- `session.rs`: saved and restored with the workspace.
- `sidebar.rs`: in the row constructor, add the stripe (a CSS class per colour) and a `gtk4::Image` for the
  icon. Add the two submenus in `attach_menu`.
- `style.css`: `.ws-color-red` and so on, using theme variables.
- `ipc.rs` / `bin/oxi.rs`: the `workspace set` command.

**Tests**
- A model round trip: serialise a workspace with colour and icon, and load an old session without them.
- An interaction test: set a colour and check the row has the CSS class.

## L2. Background opacity

**Example.** You set opacity to 85%. Your editor or docs show faintly through the terminal background. Text
stays fully opaque.

**Behaviour**
- Settings → Appearance → **Background opacity** slider, 50–100%, default 100.
- Only the background becomes transparent: terminal background, sidebar and header. Text, cursor and
  selection stay opaque.
- If the compositor does not support transparency (`gdk::Display::is_composited` is false), the slider is
  disabled and says why.
- Gradient and image themes multiply their own alpha by this value.

**Code**
- `config.rs`: `appearance.opacity: u8` with validation (50–100).
- `pane.rs`: the terminal background is set together with `set_clear_background` (line ~172); apply the alpha
  there with `set_color_background` using an RGBA.
- `theme.rs`: when building the window CSS, use `Rgb::rgba(alpha)` for the background colours instead of
  `hex()`.
- `style.css`: the window itself needs `background: transparent` when opacity < 100.

**Tests**
- Config validation (49 rejected, 50 and 100 accepted).
- Theme CSS output contains `rgba(…, 0.85)` at 85.
- Manual: GNOME Wayland, GNOME X11 and KDE; check that the text is crisp and there is no flicker on resize.

## D4. Update notice

**Example.** You turn on Settings → General → **Check for updates**. A day later a small banner appears at the
top of the window once: "oxishell 0.1.4 is out · Release notes · Dismiss". With the setting off, which is the
default, oxishell makes no network requests at all.

**Behaviour**
- Off by default. This must stay true: the README promises "no network listeners" and "works fully offline".
- When on: at most one request per 24 hours, to
  `https://api.github.com/repos/Ajinkyasp99/oxishell/releases/latest`, with a generic `User-Agent: oxishell`.
- Compare `tag_name` with `CARGO_PKG_VERSION` (semver, ignore a leading `v`). Show the banner only for a
  newer version, and only once per version.
- **Release notes** opens the release page in the browser. Nothing is downloaded or installed.
- Any failure (offline, rate limit, bad JSON) is silent.
- Update the README's Privacy section to describe this setting.

**Code**
- New `src/update_check.rs`: `is_newer(current, latest) -> bool` (pure) and `check() -> Option<Release>`
  run on a background thread, result sent back over `async_channel` the same way `usage_footer` polls.
- There is no HTTP client in `Cargo.toml` today. See the open questions for the options.
- UI: an `adw::Banner` at the top of the main window.

**Tests**
- `is_newer`: `0.1.3` vs `0.1.4`, `v0.2.0`, `0.1.3` (equal), `0.1.10` vs `0.1.9`, a garbage tag.
- With `check = false`, the check function is never called (a test with a counter).

## A6. Send a failed block to an agent

**Example.** `cargo test` fails in the server pane. You right-click the red block → **Send to agent** →
`claude (api)`. Claude's prompt now holds this text. You add "fix this" and press Enter yourself.

```
$ cargo test        (exit 101)
---- model::swap stdout ----
panicked at src/model.rs:240
assertion failed: left == right
```

**Behaviour**
- The menu item appears on a finished block with a non-zero exit status (and on any block from a second
  menu entry, **Send output to agent**).
- The submenu lists panes in all workspaces where an agent is detected (Claude, Codex, Gemini, OpenCode,
  Qoder), with workspace name and state. A pane whose agent is busy (running) is shown but greyed out.
- The text sent is: the command, the exit status, and the last 80 lines of output, with ANSI codes removed
  and capped at 8 KB. Long output says `… (N lines cut)` at the top.
- The text is sent as a **bracketed paste**, so a multi-line message stays in the prompt and nothing runs.
  Enter is never sent.
- Focus moves to the agent pane after sending.
- This is a user action in the UI, so it does not depend on `[ipc] allow_send` (that switch only guards the
  CLI).

**Code**
- `blocks.rs`: blocks already have start rows and `result: Option<(status, duration)>`. Add an end row (the
  row of the next prompt) and a `text_of(block) -> String` using VTE's text-range API.
- Add hit-testing so a right-click on a block header returns the block.
- `pane.rs`: add the menu items to the existing pane context menu. The send path already exists
  (`feed_child` at line ~68); wrap the text in `ESC[200~ … ESC[201~` for the paste.
- Agent detection: reuse `meta::agent_for_process` / the hook state from `agents.rs`.

**Tests**
- Unit tests for text building: trimming to 80 lines, the 8 KB cap, ANSI removal, the header line.
- An interaction test: two panes, one running a fake agent; send from a failed block and read the target
  pane's input.

## W7. Git detail popover

**Example.** You hover the branch name `feature-a` in the sidebar. A popover appears:

```
feature-a   ↑2 ↓0  origin/feature-a
last: "add login form" · 12 min ago
changed:
  M src/login.rs
  A src/form.rs
```

Clicking `src/login.rs` opens it in your editor through the T1 opener.

**Behaviour**
- Opens on hover after 400 ms, or on click (for touchpads and keyboard users).
- Shows: branch, upstream, ahead/behind, the last commit's subject and relative time, and up to 15 changed
  files with their status letter (`… and 8 more` after that).
- If the folder is not a git repo, there is no popover.
- Data is loaded when the popover opens, not on every sidebar refresh, so the sidebar stays as fast as today.

**Code**
- `meta.rs`: `GitInfo` already has `branch`, `ahead`, `behind`, `changes`. Add `git_details(dir) ->
  GitDetails` that reuses the `--porcelain=v2 --branch` output (the upstream name and file list are already
  in it) plus `git log -1 --format=%s%x00%ct`.
- Run it in the existing metadata worker thread (`ui/meta_worker.rs`) so the UI never waits on git.
- `sidebar.rs`: a `gtk4::Popover` on the branch label with an `EventControllerMotion` for hover.

**Tests**
- Parser tests for the file list (modified, added, deleted, renamed, untracked) and the log line.
- The real-repo test style already used in `meta.rs` (`git_info_on_real_repo_and_non_repo`).

## P9. Workspace in its own window

**Example.** You have two monitors. Right-click the `docs` workspace → **Open in new window**, and drag that
window to the second screen. Both windows share notifications, the command palette and the `oxi` CLI.
Closing the second window moves its workspaces back into the main window instead of closing them.

**Behaviour**
- Any window can hold any number of workspaces. Each window has its own sidebar showing only its workspaces.
- **Move to window** in the workspace menu lists the open windows plus **New window**.
- Shells keep running when a workspace moves (the VTE widgets are re-parented, not re-created).
- `oxi workspace list` shows which window each workspace is in. `oxi workspace move ID --window N`.
- Sessions restore windows with their size and the workspaces in each. If a monitor is gone, windows open on
  the remaining one.
- Notifications focus the right window when clicked. On Wayland, raising another window may need the
  activation token from the notification, which GTK provides.

**Code** (this is the big one)
- `ui/mod.rs`: `App` holds a single `window: adw::ApplicationWindow` and `build_window` builds one window's
  widgets. Split it into a `WindowUi` struct (window, sidebar, workspace stack, header) and keep
  `windows: Vec<WindowUi>` on `App`.
- `model.rs`: add `WindowId` and `window: WindowId` on `Workspace`, plus `active` per window instead of one
  global `active`. Keep a "focused window" for shortcuts and the CLI.
- Every place that uses `app.window` (dialogs, popovers, the palette, notifications, links) must use the
  window that owns the pane. Grep for `.window` to list them; expect several dozen call sites.
- `session.rs`: a new session format version with windows. Old files load into one window.
- Keyboard shortcuts act on the focused window.

**Tests**
- Model: create two windows, move a workspace, close a window and check its workspaces move to the main one.
- Session: round trip with two windows; load an old single-window session.
- Interaction: move a workspace with a running process to a new window and check the process is still alive.

## Open questions

- [ ] **Update check client (D4).** There is no HTTP library in `Cargo.toml`. Options: add `ureq` with
  rustls (about 1 MB more in the binary, no system dependency), or run `curl` if it is installed (no new
  crate, but silently does nothing without curl). Recommendation: `ureq`.
- [ ] **Editor for terminal editors (T1, W7).** Open vim/nvim/nano in a new split next to the pane, or in a
  new workspace? Recommendation: a split to the right.
- [ ] **Alert thresholds (A4).** Fixed at 80 and 95, or configurable in `config.toml`? Recommendation: fixed
  for the first version.
- [ ] **Colour list (W4).** Colours from the theme's palette (always fit the theme, but change with it) or
  fixed colours (always the same red)? Recommendation: theme palette.
- [ ] **P9 scope.** Ship P9 in its own release after the other seven, since it changes `App` and the session
  format? Recommendation: yes, as 0.2.0.
