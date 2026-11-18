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

## macOS

**App:** download `oxishell_macos_arm64.dmg` (Apple silicon) or `oxishell_macos_x86_64.dmg` (Intel) from
[Releases](https://github.com/Ajinkyasp99/oxishell/releases/latest), open it and drag Oxishell to
Applications. All libraries are bundled. It needs the macOS version the release was built on or newer
(see `LSMinimumSystemVersion` in the app's `Info.plist`). The app is signed ad hoc, not notarized: the
first time, right-click it and choose **Open**, or run
`xattr -dr com.apple.quarantine /Applications/Oxishell.app`. For the CLI:
`sudo ln -sf /Applications/Oxishell.app/Contents/MacOS/oxi /usr/local/bin/oxi`.

**Homebrew app:** `brew install Ajinkyasp99/oxishell/oxishell` (or with `--cask`) installs the same app
into Applications, so it appears in Launchpad and Spotlight, and links `oxi` into Homebrew's `bin`. The
not-notarized note above applies.

**Homebrew formula** (command line only, builds from source):
`brew install Ajinkyasp99/oxishell/oxishell-cli`. It installs the `oxishell` and `oxi` binaries, not an
app, so nothing appears in Applications or Launchpad; start it with `oxishell` from a terminal. To switch
to the app: `brew uninstall --formula oxishell-cli`, then install the cask. (oxishell 0.3.0 named this
formula `oxishell`, so `brew install Ajinkyasp99/oxishell/oxishell` gave no app; switch with
`brew uninstall --formula oxishell && brew install --cask Ajinkyasp99/oxishell/oxishell`.)

**From source:**

```bash
brew install gtk4 libadwaita vte3 librsvg adwaita-icon-theme pkgconf
cargo build --release
./target/release/oxishell
```

To build the app and disk image yourself: `brew install dylibbundler`, then `scripts/build-macos.sh`
(writes `dist/Oxishell.app` and `dist/oxishell_<version>_macos_<arch>.dmg`).

Settings live in `~/.config/oxishell` and `~/.local/state/oxishell` as on Linux; the control socket is in
your private `$TMPDIR`. Started from Finder or the Dock, oxishell takes the `PATH` your login shell sets
up (so `git`, `docker` and Homebrew tools are found); set `OXISHELL_KEEP_PATH=1` to turn that off.

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

**macOS, installed with Homebrew (app):**

```bash
oxi --version                          # the version you have now
brew update
brew upgrade --cask oxishell           # replaces /Applications/Oxishell.app
oxi --version                          # the new version
```

Quit Oxishell (<kbd>⌘</kbd>+<kbd>Q</kbd>) first, or restart it afterwards. The app is not notarized, so if macOS
refuses to open the new version, allow it in System Settings → Privacy & Security ("Open Anyway") or run
`xattr -dr com.apple.quarantine /Applications/Oxishell.app`. Homebrew formula (CLI only):
`brew upgrade oxishell-cli`.

**macOS, installed from the .dmg:** quit Oxishell, download the new image for your Mac and replace the app
(by hand: open the .dmg and drag **Oxishell** onto Applications → Replace), or from a terminal:

```bash
curl -fLo ~/Downloads/oxishell.dmg \
  "https://github.com/Ajinkyasp99/oxishell/releases/latest/download/oxishell_macos_$(uname -m).dmg"   # arm64 or x86_64
hdiutil attach ~/Downloads/oxishell.dmg -nobrowse -mountpoint /Volumes/oxishell-update
rm -rf /Applications/Oxishell.app
cp -R /Volumes/oxishell-update/Oxishell.app /Applications/
hdiutil detach /Volumes/oxishell-update
xattr -dr com.apple.quarantine /Applications/Oxishell.app
oxi --version
```

The `oxi` link in `/usr/local/bin` points into the app, so it uses the new version automatically.

**macOS, built from source:** `git pull`, then `scripts/build-macos.sh` and install `dist/oxishell_*_macos_*.dmg`
as above (or run `./target/release/oxishell` after `cargo build --release`).

With Settings → Notifications → Check for updates on, oxishell shows "oxishell X is out" on both Linux and
macOS when a new release appears; use the commands above for your install method.

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

On macOS: remove the hooks as in step 1, then delete `/Applications/Oxishell.app` (or
`brew uninstall --cask oxishell` / `brew uninstall --formula oxishell-cli`) and, optionally, the two folders from step 3.

The package installs only: `/usr/bin/oxishell`, `/usr/bin/oxi`, the desktop entry, icon, AppStream
metadata and docs under `/usr/share/doc/oxishell`.
