# Installing oxishell

## From the .deb (Ubuntu 24.04 and newer)

Download the latest package from [Releases](https://github.com/Ajinkyasp99/oxishell/releases/latest) and install it:

```bash
wget https://github.com/Ajinkyasp99/oxishell/releases/latest/download/oxishell_amd64.deb
sudo apt install ./oxishell_amd64.deb
```

Each release also lists `SHA256SUMS`; verify with `sha256sum -c SHA256SUMS --ignore-missing`.

`apt` pulls in the runtime libraries (GTK 4, libadwaita, VTE-GTK4). Then start **oxishell** from the
application menu, or run `oxishell` in a terminal. The `oxi` command-line tool is installed too.

Optional: `git` (recommended) — branch and status in the sidebar.

Then enable agent notifications (optional):

```bash
oxi hooks install --all
```

## From source

```bash
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev
cargo build --release
./target/release/oxishell
```

Rust 1.87+ is required. To build the package yourself: `scripts/build-deb.sh` (needs `dpkg-dev`).

## Update

Settings, themes, workspaces and agent hooks are kept. Close oxishell before updating (or restart it after).

From a release:

```bash
wget -O oxishell_amd64.deb https://github.com/Ajinkyasp99/oxishell/releases/latest/download/oxishell_amd64.deb
sudo apt install ./oxishell_amd64.deb
oxi --version
```

From source:

```bash
git pull
scripts/build-deb.sh && sudo apt install --reinstall ./dist/oxishell_*.deb
```

A local build keeps its version number, so `--reinstall` is needed or apt skips it as already installed.

## Uninstall

1. Remove agent hooks first (they live in your agents' own config files):
   ```bash
   oxi hooks uninstall claude     # repeat for codex, gemini, qoder, opencode as installed
   ```
   A one-time backup of each original file was saved next to it as `*.oxishell.bak`.
2. Remove the package:
   ```bash
   sudo apt remove oxishell
   ```
3. Optionally delete your settings and saved sessions:
   ```bash
   rm -rf ~/.config/oxishell ~/.local/state/oxishell
   ```

The package installs only: `/usr/bin/oxishell`, `/usr/bin/oxi`, the desktop entry, icon, AppStream
metadata and docs under `/usr/share/doc/oxishell`.
