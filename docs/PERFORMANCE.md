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
| CLI round trip (`cmux ping`, incl. process start) | 3.1 ms | 3.0 ms |

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

- Every 3 s: cwd/foreground process for panes of the visible workspace (`/proc` reads, worker thread).
- Every 15 s: all panes, including listening ports for background workspaces.
- `git status` only when a pane's directory changed or the cached result is older than 5 s, with
  `GIT_OPTIONAL_LOCKS=0` so it never contends with your own git commands.
- Nothing runs on the GTK thread; jobs for the same pane are coalesced.

## Not measured

- GPU-accelerated sessions on real hardware, multi-monitor and fractional scaling (no physical display
  was available to this test run).
- Long-running (hours) leak testing.
