# Installing cmux-linux

## From the .deb (Ubuntu 24.04 and newer)

Download the latest package from [Releases](https://github.com/Ajinkyasp99/cmux_linux/releases/latest) and install it:

```bash
wget https://github.com/Ajinkyasp99/cmux_linux/releases/latest/download/cmux-linux_amd64.deb
sudo apt install ./cmux-linux_amd64.deb
```

Each release also lists `SHA256SUMS`; verify with `sha256sum -c SHA256SUMS --ignore-missing`.

`apt` pulls in the runtime libraries (GTK 4, libadwaita, VTE-GTK4). Then start **cmux** from the
application menu, or run `cmux-linux` in a terminal. The `cmux` command-line tool is installed too.

Optional: `git` (recommended) — branch and status in the sidebar.

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

## Update

Settings, themes, workspaces and agent hooks are kept. Close cmux before updating (or restart it after).

From a release:

```bash
wget -O cmux-linux_amd64.deb https://github.com/Ajinkyasp99/cmux_linux/releases/latest/download/cmux-linux_amd64.deb
sudo apt install ./cmux-linux_amd64.deb
cmux --version
```

From source:

```bash
git pull
scripts/build-deb.sh && sudo apt install --reinstall ./dist/cmux-linux_*.deb
```

A local build keeps its version number, so `--reinstall` is needed or apt skips it as already installed.

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

The package installs only: `/usr/bin/cmux-linux`, `/usr/bin/cmux`, the desktop entry, icon, AppStream
metadata and docs under `/usr/share/doc/cmux-linux`.
