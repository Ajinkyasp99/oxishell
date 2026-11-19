# Idle work, startup, memory and macOS download size — design

Date: 2026-11-18. Source: an external review of the v0.3.0 release (DMG contents, Linux idle sampling,
history-import timing). This document turns its findings into changes.

## Intent

Oxishell should cost less while it sits there and download faster on a Mac, with no change in what the
user sees: ports, git status, Compose stacks, agent detection, notifications and history behave as today.

- Idle cost matters most on macOS laptops (battery): today one visible idle pane launches `lsof` about 20
  times a minute, four panes about 80.
- Startup: importing a large shell history blocks the window (0.75 s for 20,000 entries, 3 s for 50,000).
- Memory: every pane builds Compose stack widgets it will almost never show.
- Download: the arm64 DMG is 39 MiB; ICU data (32 MiB raw, ~12.5 MiB compressed) and an unused cursor
  theme (18 MiB raw, ~2.5 MiB compressed) are the largest avoidable parts.

## Goals

1. Fewer helper processes and wakeups at idle on both platforms; on macOS at most one `lsof` per batch.
2. History import under 50 ms for 20,000 entries (measured on the Linux bench machine, release build).
3. Lower GUI private memory per pane, measured at 1, 10 and 20 panes.
4. Smaller macOS DMG: no cursor theme, no ICU, an `oxi` CLI that does not load GTK.

## Non-goals

- Changing the session-restore model (panes of all saved workspaces are still created at startup).
- Rendering/GPU work, VTE scrollback tuning, font loading.
- Linux packaging (the .deb uses system libraries; nothing to shrink).
- Changing the CPU/RAM strip's content or the 3 s foreground/cwd cadence for the visible workspace.

## Anti-goals (failure even if it "works")

- A server started from an idle prompt (`npm run dev &`, `python -m http.server &`) no longer appears in
  the sidebar, or appears much later than today in the common case (see the ports design for the bound).
- Git status visibly lags after a command that changes it (`git commit`, `git checkout`, editing then
  `git add`).
- A running Compose stack stops being monitored, or a crash notification is missed or delayed.
- History order, deduplication or timestamps differ from today's for the same files.
- The Mac app breaks on a Mac without Homebrew, shows wrong cursors, or mis-renders non-ASCII text.

## Constraints

- Both platforms (project CLAUDE.md). OS-specific code stays in `crate::platform` / `crate::meta` or
  behind `cfg(target_os)`. No new shortcuts in this work.
- Everything in `meta_worker` stays off the GTK thread; the existing per-pane job coalescing and the one
  process-table snapshot per batch stay.
- The Mac build machine is not available from Linux. On Linux only the macOS compile check runs
  (`PKG_CONFIG_ALLOW_CROSS=1 ... cargo clippy --target aarch64-apple-darwin`); `build-macos.sh`, DMG size
  and Mac UI behaviour are verified by the CI `macos` job and by hand on a Mac.
- No commits or pushes by the builder; changes are left uncommitted for the owner.

## Approaches considered

**Ports.** (a) Only batch `lsof` into one call per batch: cuts launches by the pane count but still runs
every 3 s. (b) Batch, plus a separate slower port schedule with back-off, plus an immediate check when a
pane's process tree changes. (c) Replace `lsof` with libproc socket fdinfo calls (`proc_pidfdinfo` with
`PROC_PIDFDSOCKETINFO`): no helper at all, but the `libc` crate has no bindings for the socket structs,
so it means hand-written FFI structs that must match both Intel and Apple-silicon layouts. **Picked (b)**:
most of the saving, no unsafe FFI, and the tree-change trigger keeps detection prompt. (c) stays
a possible follow-up.

**History.** (a) Fix the algorithm and keep loading synchronous. (b) Also move loading to a background
thread and merge commands typed meanwhile. **Picked (a)**: once linear, 20,000 entries should take a few
milliseconds, and synchronous loading keeps the ordering rules trivial. Do (b) only if the measurement
after (a) is still over 50 ms.

**ICU.** (a) Keep Homebrew's `vte3` (links ICU). (b) Build VTE from source with ICU off in
`build-macos.sh`, behind a switch, cached in CI. (c) Delete the ICU dylibs from the bundle: breaks the
load chain, rejected. **Picked (b)**: it is the only large download saving; oxishell never calls
`vte_terminal_set_encoding`, so terminals are always UTF-8 and nothing user-visible is lost. The switch
lets the build fall back to Homebrew's `vte3` in one line if the source build breaks.

**`oxi` CLI linking GTK on macOS.** (a) Split the crate into a GTK-free library and a GUI crate.
(b) A `build.rs` linker flag. On Linux `ldd target/release/oxi` already shows no GTK because the
toolchain links with `--as-needed`; macOS `ld` keeps every listed dylib unless told otherwise.
**Picked (b)**: one line, no restructuring.

## Design

### 1. Port discovery (`src/ui/meta_worker.rs`, `src/meta.rs`, `src/platform/`)

Today: every 3 s tick, each visible pane's job has `ports: true`; the worker builds one ppid table per
batch, then calls `meta::listening_ports(tree)` per pane. On macOS that is one `lsof` per pane; on Linux
it re-reads `/proc/net/tcp{,6}` per pane.

New behaviour:

- **Batched lookup.** New `meta::listening_ports_by_pid(pids: &[i32]) -> HashMap<i32, Vec<u16>>`.
  - macOS: one `lsof -nP -w -a -iTCP -sTCP:LISTEN -p <all pids> -Fpn` and a new parser in
    `platform/mod.rs` that attributes each `n` line to the preceding `p<pid>` line (unit-tested with
    captured `lsof -F pn` output, including IPv6 `[::1]:port` and `*:port`).
  - Linux: read `/proc/net/tcp{,6}` once per batch, then each pid's `fd` directory.
  - The worker maps pid → pane with the per-pane trees it already computes, and unions ports per pane.
  - The existing `listening_ports(&[i32])` stays as a thin wrapper (tests use it).
- **Own schedule, in the worker.** The worker keeps per-pane port state: last check time, current interval,
  last result, last process-tree pid set. Jobs keep `ports: bool`, now meaning "ports may be checked";
  the worker decides whether they are due:
  - Due when the pane's process-tree pid set differs from the last check (a process started or exited,
    which includes `cmd &` from an idle prompt and a foreground command starting or ending). This is
    checked every tick from the ppid table, which is cheap.
  - Due when the interval since the last check has passed. The interval starts at 9 s and doubles while
    the result stays the same (9 → 18 → 30 s cap). It resets to 9 s when the result changes or the tree
    changes.
  - Due on the next two ticks after a tree change (about +3 s and +6 s), because a freshly started dev
    server (Vite, Next.js) usually binds its port 0.5–several seconds after it forks. After those two
    follow-ups the normal interval applies.
  - Due when the pane is new to the worker, or is in a job flagged as forced (see below).
  - Panes that are not due keep their last result: the update carries `ports: None` (already means
    "unchanged" in `attach`).
- **Forced check.** Switching to a workspace sends its panes with a force flag (new `Job` field
  `force_ports: bool`), so the sidebar is fresh when the user looks at it.
- **Bound.** A port opened within ~6 s of a process starting appears within one tick of opening (3 s, as
  today). A process that starts listening later than that without any tree change appears within 30 s at
  worst, usually 9–18 s.
- **The ppid table** is built only when some job in the batch wants ports (unchanged rule).
- State for panes that no longer exist is dropped (prune when the map exceeds the live pane count, or on
  a "pane closed" signal if one exists; builder's choice).

### 2. Git status back-off (`src/ui/meta_worker.rs`)

Today `GIT_TTL` is a fixed 5 s per directory.

New: per-directory TTL that starts at 5 s and doubles while `git_info` returns the same result
(5 → 10 → 20 → 30 s cap). It resets to 5 s when:

- the result changes,
- the pane's cwd changes (a new key, so already fresh),
- a command finished in the pane. The signal is the shell integration's prompt report: at every prompt
  the shell rewrites the pane's env file (`src/shellint.rs`), and `App::command_finished` is driven by
  that file's modification time. Sampling `busy` is not enough: `git add`/`git commit`/`git checkout`
  finish in milliseconds, between two 3 s ticks. The worker already reads the env file in every job; it
  remembers the file's modification time per pane, and a changed time makes it drop the cached git entry
  for that cwd before looking it up. This catches every prompt, including commands the line tracker could
  not follow. `App::command_finished` also sends a job for that pane at once, so the refresh does not
  wait for the next tick.

Panes without working shell integration (the env file does not exist, e.g. fish without its config line,
or a user who replaced `PROMPT_COMMAND`) keep today's fixed 5 s TTL, so they never lag more than now.

### 3. Compose discovery (`src/ui/meta_worker.rs`, `src/compose.rs`)

Today `ComposeCache` uses one 30 s TTL for repo roots (`git rev-parse` per cwd), compose file scans (up to
5,000 folders) and command detection (`docker compose version` etc.).

New TTLs:

| Cache | Today | New |
|---|---|---|
| `commands` (setting → detected program) | 30 s | 10 min (both found and not found) |
| `roots` (cwd → repo root) | 30 s | 5 min |
| `files` (root → compose files) | 30 s | starts at 30 s, doubles while unchanged, cap 5 min |

- An explicit rescan (`compose_ui::rescan`, and opening the picker if it already triggers one) bypasses all
  three, as today. A rescan or a changed result resets the `files` back-off.
- **Shallow automatic scan in `$HOME` and `/`.** When the scan root is exactly the user's home folder or
  `/` (a pane outside any repo sitting at its default folder), the automatic scan uses `max_depth: 1`.
  A rescan uses the default `Limits`. Every other folder keeps the default limits, so compose files
  outside a repo elsewhere still light up the pane button by themselves.
  The `files` cache entry remembers whether it came from a deep scan. Once a rescan has run for `$HOME`
  or `/`, automatic refreshes of that root also scan deep, so the button the rescan lit up does not
  disappear 30 s later. That lasts until the worker restarts (the app quits).
  (Earlier idea "shallow scan for every non-repo folder" was dropped: the pane button is hidden until a
  scan finds files, so deep files would never be discoverable.)
- Running-stack monitoring (`ps_of`, the 1 s compose UI timer, crash detection) is untouched.
- The existing tests set `ttl: Duration::ZERO` / assert the 30 s default; they are updated to the new
  structure and new tests cover: back-off growth and reset on change, rescan bypass, shallow scan only for
  `$HOME` and `/`.

### 4. Minimized / hidden window (`src/ui/system_status.rs`, `src/ui/meta_worker.rs`, `src/ui/mod.rs`)

"Hidden" means the main window is minimized or suspended: `gtk::Window::is_suspended()` (GTK ≥ 4.12,
`notify::suspended`), or the toplevel surface state containing `GdkToplevelState::MINIMIZED`
(`notify::state`). The app exposes one `Cell<bool>` and a way for the parts below to react.

- **System-status strip:** the 2 s timer skips sampling and redrawing while hidden. On becoming visible it
  takes a fresh sample at once. The CPU history keeps its gap (`None` entries are already supported).
- **Metadata:** while hidden, the 3 s tick requests nothing for the visible workspace on its own. The 15 s
  full pass still runs for all panes, so agent detection and busy state keep working at the background
  rate. Exception: panes with a running Compose stack (`compose.stack` is `Some`) keep the 3 s tick while
  hidden, because crash notifications matter most when the user is not looking. Those jobs carry
  `ports: false` and skip git (a `meta_only`/`compose_only` style flag; the builder names it). On becoming
  visible: one immediate request for the visible workspace with forced ports.
- Unchanged while hidden: IPC, notifications, agent hooks, desktop notifications, the usage footer, update
  checks, the Compose 1 s UI timer.
- macOS: whether GDK's macOS backend reports `suspended` or `MINIMIZED` when the window is minimized to the
  Dock is not verified. Implement both signals; check by hand on a Mac. If neither fires on macOS the
  feature is inert there (no regression).

### 5. History import (`src/history.rs`)

Today `from_oldest_timed` calls `push_at` per line: `retain` (O(n)) plus `insert(0)` (O(n)) for each of
up to tens of thousands of lines.

New `from_oldest_timed`: walk the lists newest first (last list first, each list from its end), keep a
`HashSet` of seen commands, append unseen ones to `entries` (newest first, the stored order), record the
time of that newest occurrence (`Some(t)` inserts, `None` leaves no time, matching today where the newest
push decides), and stop once `MAX_ENTRIES` are kept. `clean()` is applied exactly as in `push_at`.

- Result must be identical to today's for any input. The old algorithm is kept in the test module as an
  oracle; tests compare both on: duplicates across files, duplicates within a file, timed and untimed
  duplicates in both orders, more than `MAX_ENTRIES` unique entries, entries that `clean()` rejects.
- Benchmark (unit test marked `#[ignore]` or a small bench in `scripts/bench.sh`): 20,000 and 50,000
  unique entries; target < 50 ms each on the bench machine (release).
- Loading stays synchronous in `App` construction. If the target is missed, stop and report before moving
  it to a thread.
- Files are still read whole; a 50,000-line file is a few MB, fine.

### 6. Lazy Compose stack widgets (`src/ui/compose_ui.rs`)

Today `PaneCompose::new` builds the attach button (in the pane's action bar) plus the hidden stack bar
(status line, three buttons, chip `FlowBox`) and the logs bar for every pane.

New: the attach button stays eager (its visibility depends on scan results). The stack bar and logs bar
are created the first time they are needed (a stack or `logs` appears for that pane) and inserted at the
same place under the terminal as today. Code that updates them does nothing when they do not exist yet.
The exact struct shape (e.g. `RefCell<Option<StackBars>>` plus a placeholder container to insert into)
is the builder's call.

Measure GUI memory (private anonymous and PSS, the same method as `docs/PERFORMANCE.md`) with 1, 10 and
20 empty panes, before and after. If the saving is under ~0.1 MB per pane, report it and keep the change
only if the code stays as clear as today.

### 7. macOS packaging (`scripts/build-macos.sh`, `build.rs`, `.github/workflows/`)

- **Cursor theme.** GDK's macOS backend maps cursor names to `NSCursor` (`gdk/macos/gdkmacoscursor.c`:
  a name table, falling back to the arrow cursor) and does not read Xcursor themes. After copying
  Adwaita, delete `$R/share/icons/Adwaita/cursors`. Keep everything else in the icon themes.
  Hand check on a Mac: text I-beam over a terminal, pointer on links, resize cursors on split handles and
  window edges, the corner-resize grip, drag cursor when dragging a pane.
- **`oxi` without GTK.** New `build.rs`: when `CARGO_CFG_TARGET_OS == "macos"`, emit
  `cargo:rustc-link-arg-bin=oxi=-Wl,-dead_strip_dylibs`. `build-macos.sh` then fails the build if
  `otool -L Contents/MacOS/oxi` lists libgtk, libadwaita or libvte. Measure `oxi ping` round trip before
  and after on the Mac (the CI job can print it; it is informational).
- **VTE without ICU.** `build-macos.sh` gains a switch, `VTE_SOURCE=1` (default on):
  - Download a pinned VTE release tarball (same minor version as Homebrew's current `vte3`, at least
    0.76 for the `v0_76` feature) and verify its SHA-256 in the script.
  - Build it with meson into a prefix under `target/vte-noicu/<version>-<arch>`, GTK 4 only, with ICU,
    GTK 3, introspection, vala and docs off. Exact option names are taken from that release's
    `meson_options.txt`.
  - Put that prefix's `lib/pkgconfig` first on `PKG_CONFIG_PATH` for `cargo build`, and give
    `dylibbundler` its `lib` as a search path.
  - Fail the build if any `libicu*` is in `Contents/Frameworks`. Checked against the Homebrew formulas
    (2026-11-18): VTE (`vte3` 0.84.1) is the only bundled library that depends on `icu4c`. `harfbuzz` is
    built with `-Dicu=enabled`, which builds ICU support as the separate `libharfbuzz-icu` module
    (`icu_builtin` is off), and pango/GTK link only `libharfbuzz`. Homebrew's `libxml2` has no ICU, and
    librsvg uses macOS's own libxml2. If the check fails anyway, find the library with `otool -L` and
    report it rather than widening scope; `VTE_SOURCE=0` restores today's build.
  - Pin VTE 0.84.1 (Homebrew's current `vte3`) unless the builder finds a reason not to. It needs
    `simdutf`, `fribidi`, `pcre2`, `lz4`, `gnutls` from Homebrew, which the CI jobs install.
  - `VTE_SOURCE=0` keeps today's Homebrew `vte3` path.
  - CI `macos` job (`ci.yml`) and both jobs in `release.yml`: install meson/ninja and the VTE build
    dependencies, and cache the prefix with `actions/cache`, keyed on the VTE version, the arch and the
    Homebrew gtk4 version. `release.yml` gets this in the same change, since `VTE_SOURCE=1` is the
    release default.
  - Hand check on a Mac: UTF-8 text in several scripts (accents, CJK, emoji, right-to-left Arabic or
    Hebrew), pasting it, typing it via input methods, and the shell's `locale` output unchanged.
- Record the arm64 DMG size before and after, from the CI `macos` job (Apple silicon only). The Intel
  size is recorded from the next `release.yml` build; no Intel job is added to `ci.yml`.
- **How section 7 is checked.** The builder cannot push or run macOS builds. Section 7 is written as one
  batch after sections 1–6 pass on Linux, with the cross compile check run locally. Then the builder stops
  and hands over: the owner pushes it to a branch, and the CI `macos` log (build output, sizes, the
  `otool` checks) comes back to the builder for fixes. Expect more than one round for the VTE build.
  Each of the three changes prints its own size line in the build log, so one CI run yields the
  before/after numbers: the DMG size with cursors, the size without them, and `libicu*` bytes saved.

### 8. Docs

- `docs/PERFORMANCE.md`: new refresh policy (ports schedule, git back-off, Compose TTLs, hidden window)
  and new measured numbers (Linux; Mac numbers only if measured on a Mac).
- `docs/PACKAGING.md`: the VTE source build, the switch, the cursor removal, the `oxi` linker flag.
- `docs/RELEASE_NOTES.md`: user-facing summary.
- README: nothing unless an existing statement becomes wrong (no new shortcuts).

## Verification

Linux (this machine): `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test --lib --bins`, the GUI suites under `dbus-run-session -- xvfb-run -a`, and before/after
numbers from `scripts/bench.sh`: idle CPU at 1/10/20 panes, RSS/private memory at 1/10/20 panes, and
child processes started per minute with one and four idle panes in a git repo (e.g. counted with
`strace -f -e trace=execve` or `/proc` sampling; builder's choice). Also: start `python3 -m http.server &`
from an idle prompt and confirm the port appears in the sidebar within one tick.

macOS on Linux: the cross `cargo clippy --target aarch64-apple-darwin` compile check. On a Mac / CI:
`cargo clippy`, `cargo test --lib --bins`, `scripts/build-macos.sh` with `VTE_SOURCE=1` and `=0`, DMG
sizes, and the hand checks listed above. The GUI suites are Linux-only; macOS UI behaviour is checked
by hand only.

## Order of work

Each step is checked before the next starts.

1. Idle work (sections 1–4).
2. Startup (section 5).
3. Memory (section 6).
4. macOS packaging (section 7), the ICU switch last.
5. Docs.

## Calls made without the owner (review these)

- Port interval 9 → 18 → 30 s with immediate checks on process-tree changes and workspace switches, plus
  two follow-up checks (+3 s, +6 s) after a tree change.
- Git TTL 5 → 30 s back-off, reset by the shell's prompt report; panes without shell integration keep
  the fixed 5 s.
- Panes with a running Compose stack keep the 3 s tick while the window is minimized.
- After a rescan in `$HOME` or `/`, automatic refreshes of that root stay deep.
- Section 7 is handed over as one batch; the owner pushes it and relays the CI log. arm64 sizes in the
  PR, Intel from the next release.
- Compose TTLs: 10 min command detection, 5 min repo root, 30 s → 5 min files back-off, shallow automatic
  scan only in `$HOME` and `/`.
- While minimized, the visible workspace drops to the 15 s background rate.
- History stays synchronous unless it misses 50 ms.
- `VTE_SOURCE=1` is the default for release builds.

## Left to the builder

Struct shapes, names, where per-pane worker state lives, how stale state is pruned, the exact meson
options, the CI cache key format, and how child-process counts are measured.
