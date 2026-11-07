# Installing cmux-linux

## From the .deb (Ubuntu 24.04 and newer)

```bash
sudo apt install ./cmux-linux_0.1.0_amd64.deb
```

`apt` pulls in the runtime libraries (GTK 4, libadwaita, VTE-GTK4). Then start **cmux** from the
application menu, or run `cmux-linux` in a terminal. The `cmux` command-line tool is installed too.

Optional:
- `git` (recommended) — branch and status in the sidebar.
- `tmux` — set `session.backend = "tmux"` to keep programs running across app restarts.

Then enable agent notifications (optional):

```bash
cmux hooks install --all
```

## From source

```bash
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev
cargo build --release
./target/release/cmux-linux
```

Rust 1.87+ is required. To build the package yourself: `scripts/build-deb.sh` (needs `dpkg-dev`).

## Uninstall

1. Remove agent hooks first (they live in your agents' own config files):
   ```bash
   cmux hooks uninstall claude     # repeat for codex, gemini, qoder, opencode as installed
   ```
   A one-time backup of each original file was saved next to it as `*.cmux-linux.bak`.
2. Remove the package:
   ```bash
   sudo apt remove cmux-linux
   ```
3. Optionally delete your settings and saved sessions:
   ```bash
   rm -rf ~/.config/cmux-linux ~/.local/state/cmux-linux
   ```
4. If you used the tmux backend, end leftover sessions with `tmux ls` / `tmux kill-session -t cmux-linux-<id>`.

The package installs only: `/usr/bin/cmux-linux`, `/usr/bin/cmux`, the desktop entry, icon, AppStream
metadata and docs under `/usr/share/doc/cmux-linux`.
