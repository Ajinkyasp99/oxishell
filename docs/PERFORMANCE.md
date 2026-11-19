# Performance (Phase 11)

All numbers below were **measured** with `scripts/bench.sh` on a release build (2026-11-07).
Reproduce with `xvfb-run -a scripts/bench.sh` or `DISPLAY=:N scripts/bench.sh`.

**Environment:** AMD Ryzen 7 4800H (16 threads), 14 GiB RAM, Ubuntu 24.04, kernel 7.0.0-42, GTK 4.14.5,
VTE 0.76.0. Display: **Xvfb** (no GPU; GTK's GL renderer runs on Mesa llvmpipe). A real desktop
session with a GPU will differ, particularly in memory and rendering cost. Wayland startup (headless GNOME
Shell 46, see below) was 216 ms.

| Metric | GL renderer (default) | Cairo renderer |
|---|---|---|
| Startup to control socket ready, median of 5 | 181 ms (174–188) | 178 ms (171–189) |
| Idle CPU, 1 pane, 10 s window | 1.0 % | 0.9 % |
| Idle CPU, 10 panes | 1.0 % | 2.8 % |
| Idle CPU, 20 panes | 1.2 % | 3.9 % |
| App RSS, 1 pane (private anon) | 242 MB (107 MB) | 158 MB (40 MB) |
| App RSS, 10 panes (private anon) | 249 MB (114 MB) | 169 MB (49 MB) |
| App RSS, 20 panes (private anon) | 258 MB (123 MB) | 181 MB (59 MB) |
| App + shells RSS, 20 panes | 372 MB | 295 MB |
| `seq 1 1000000` in a visible pane, to last line | 678 ms | 690 ms |
| CLI round trip (`oxi ping`, incl. process start) | 3.1 ms | 3.0 ms |

Reading the table:
- Most RSS is file-backed shared libraries (GTK, Mesa/LLVM, ICU, fonts) that other GTK apps share.
  Private anonymous memory grows by ~0.85 MB per extra pane; each bash adds ~5.5 MB of its own.
- Idle CPU is dominated by the focused pane's blinking cursor redraw (software rendering) and the 3 s
  metadata poll. An earlier version scanned all of `/proc` once per pane for port detection (4.4 % idle at
  20 panes); scanning once per batch brought it to 1.2 %.

## Stress tests (tests/e2e.rs, run in CI under Xvfb, also passed on headless Wayland)

- 16 panes across 4 workspaces, each printing 200,000 lines simultaneously: all finished, and the app
  answered a `workspace.list` request within 2 s afterwards. Whole stress test: ~13 s.
- 200 consecutive workspace switches through the API.
- 30 cycles of creating and force-closing a pane: every shell process exited (checked via `/proc`), pane
  count returned to 16.
- Shutdown by SIGTERM: all 16 shells exited; session and scrollback were saved.

## Metadata refresh policy

Nothing here runs on the GTK thread; jobs for the same pane are coalesced, and each batch reads the
process table once and looks up listening ports for all its panes in one call (`/proc` on Linux, one
`lsof` on macOS).

- **Tick:** every 3 s for panes of the visible workspace (cwd, foreground process); every 15 s for all panes.
- **Listening ports:** checked per pane every 9 s, then 18 s, then 30 s (the cap) while the result stays
  the same. A change of the pane's process tree is checked immediately, with two follow-up checks about
  3 s and 6 s later; after a process starts or exits, ports are checked at once and on the next two ticks, and the back-off then restarts at 9 s. Switching workspace forces a check.
- **Git status:** cached per directory with a TTL of 5 s, then 10, 20 and 30 s (the cap) while the result
  is unchanged. The TTL resets when the shell reports its prompt (the shell-integration env file's mtime
  changes, in any pane, which also caps every cached directory's TTL at 5 s), and a finished command sends an immediate refresh. Panes without shell integration keep a
  fixed 5 s TTL. `git` runs with `GIT_OPTIONAL_LOCKS=0` so it never contends with your own git commands.
- **Docker Compose:** command detection is cached 10 min, the repo root 5 min, and the compose-file scan
  30 s, doubling to 5 min while unchanged. An automatic scan whose root is exactly your home directory or
  `/` looks one level deep only, until you rescan from the launcher (it then stays deep). The `ps` of a
  running stack is unchanged.
- **Hidden window** (minimized or suspended): the CPU/RAM strip pauses, the 3 s visible-workspace tick only
  serves panes with a running Compose stack, and the 15 s full pass continues. On restore it samples
  once and refreshes everything. Verified on Linux only; macOS behaviour is unverified.

## Changes in this round (2026-11-19) and their numbers

Measured on Linux (same host as the baseline in `docs/superpowers/plans/2026-11-18-baseline-bench.txt`,
Xvfb, one run each, so small differences are noise). Nothing in this section was measured on macOS.

| Metric | Baseline (2026-11-18) | Now (2026-11-19) |
|---|---|---|
| Startup to control socket, median of 5 | 251 ms | 231 ms |
| Idle CPU, 1 pane | 1.60 % | 1.30 % |
| Idle CPU, 10 panes | 1.60 % | 1.20 % |
| Idle CPU, 20 panes | 1.70 % | 1.70 % |
| App RSS (private anon), 1 pane | 253 MB (112 MB) | 251 MB (111 MB) |
| App RSS (private anon), 10 panes | 263 MB (121 MB) | 259 MB (118 MB) |
| App RSS (private anon), 20 panes | 272 MB (130 MB) | 265 MB (124 MB) |
| `seq 1 1000000` in a visible pane | 627 ms | 646 ms |
| `oxi ping` round trip | 3.4 ms | 3.3 ms |
| Child processes started per minute, 1 idle pane in a git repo | 14 (git 11, docker 3) | 3 (git 3) |
| Child processes started per minute, 4 idle panes in a git repo | 31 (git 25, docker 6) | 3 (git 3) |

Idle CPU, startup and RSS differences are within run-to-run noise on this host; the clear gain is the
number of helper processes, which dropped by 79 % and 90 %.

- **Child processes** are the helper programs the app starts while idle in a git repo (git, docker
  compose probes), counted over 60 s with `strace -f` on `execve`.
- **History import** (`src/history.rs`) is now linear in the number of entries: 20,000 entries went from
  about 400 ms to about 16 ms (measured with the unit test `import_is_fast`, release build,
  20,000 unique entries, on this machine).
- **Compose stack and logs bars** are built the first time a stack or logs view shows, not for every pane:
  about 0.1 MB less private memory per pane, which is near the measurement noise.
- **macOS package size:** `oxi` is linked with `-dead_strip_dylibs`, the unused cursor theme is dropped
  from the bundle, and VTE is built from source without ICU. `scripts/build-macos.sh` prints `size:` lines;
  the resulting DMG size is not yet measured (needs a Mac or the CI `macos` job).

## Not measured

- GPU-accelerated sessions on real hardware, multi-monitor and fractional scaling (no physical display
  was available to this test run).
- Long-running (hours) leak testing.
