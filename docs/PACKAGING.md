# Packaging (Phase 12)

## .deb (implemented)

`scripts/build-deb.sh` builds `dist/oxishell_<version>_<arch>.deb`:
- `Depends` is computed by `dpkg-shlibdeps` from the binaries (GTK 4, libadwaita, VTE-GTK4, glib, libc).
- `Recommends: git`.
- Installs binaries, desktop entry, scalable icon, AppStream metainfo and docs; postinst/postrm only
  refresh desktop and icon caches. Removal deletes every installed file; user data in `~/.config` and
  `~/.local/state` is left alone (standard Debian behaviour; see INSTALL.md to remove it).
- Reproducibility: `cargo build --locked`, `SOURCE_DATE_EPOCH` (commit time when available) for file
  timestamps, and path prefixes remapped so local paths are not embedded. Verified: two clean builds on
  the same machine with the same `SOURCE_DATE_EPOCH` produced byte-identical `.deb` files. Not yet
  verified across different machines/toolchains.

## macOS app and disk image (implemented)

`scripts/build-macos.sh` builds `dist/Oxishell.app` and `dist/oxishell_<version>_macos_<arch>.dmg` on a
Mac with Homebrew:
- The version comes from Cargo.toml into `Info.plist` (template `data/macos/Info.plist.in`), like the .deb.
- `dylibbundler` copies every non-system library (GTK 4, libadwaita, VTE, GLib, Pango, …) into
  `Contents/Frameworks` and rewrites load paths; the script fails if anything still points into Homebrew.
- `Contents/Resources` holds the GTK runtime data: compiled GSettings schemas, the Adwaita and hicolor
  icon themes, gdk-pixbuf loaders (librsvg for SVG icons) with a cache template whose paths are filled
  in at startup (`platform::macos::setup_bundle_env`), and the fontconfig configuration.
- Signed ad hoc (required on Apple silicon). Developer ID signing and notarization are sketched in the
  script for when an Apple Developer account exists.
- Homebrew's libraries target the macOS release they were built on, so `LSMinimumSystemVersion` is the
  build machine's major version.

## Homebrew formula (implemented)

`packaging/homebrew/oxishell.rb.in` builds from the release's source tarball against Homebrew's GTK 4,
libadwaita and VTE. The release workflow fills in version and checksum and pushes it to the tap
`Ajinkyasp99/homebrew-oxishell` (needs the `HOMEBREW_TAP_TOKEN` secret).

## Releases

One tag builds everything at the Cargo.toml version (`.github/workflows/release.yml`): `scripts/bump-version.sh`,
commit, then push the tag `vX.Y.Z`. The workflow checks the tag against Cargo.toml, builds the Ubuntu .deb and
the macOS .dmg for Apple silicon (`macos-15`) and Intel (`macos-15-intel`), and publishes a single release only
when all of them succeeded, with `SHA256SUMS` over every file. Then it updates the Homebrew formula.
A `vX.Y.Z-rc.N` tag publishes a pre-release the same way, without touching Homebrew.

## Flatpak (evaluated, recommended next)

Pros: runs on any distro; GNOME runtime already ships GTK 4 and libadwaita; VTE is a small module to
build in the manifest; Flathub distribution and updates.

Required work and caveats:
- A terminal must run the user's host shell. Inside the sandbox that means spawning through
  `flatpak-spawn --host` (needs `--talk-name=org.freedesktop.Flatpak`), the same approach GNOME Console
  and Ptyxis use. VTE's spawn would be wrapped accordingly.
- `/proc`-based cwd/ports/agent detection only sees sandboxed processes; host processes need the host
  helper or `--filesystem=host` plus `--talk-name`. Git metadata would run `git` on the host.
- Agent hook files live in `~/.claude`, `~/.codex`, ... — needs `--filesystem` permissions for those paths.
- The `oxi` CLI must be callable from host shells: export it via a small host-side wrapper.

Estimate: medium effort; worthwhile once the feature set is stable.

## AppImage (evaluated, not recommended now)

Bundling GTK 4 + libadwaita + VTE produces a large image (~60–90 MB) and GTK 4 AppImages are known to
have theme/input-method issues because GTK modules, icon themes and GSettings schemas must be bundled or
taken from the host inconsistently. Since Flatpak solves the same "any distro" goal with shared
runtimes, AppImage is deferred.

## Other distributions

The code depends only on GTK ≥ 4.14, libadwaita ≥ 1.5 and VTE-GTK4 ≥ 0.76, which Fedora 40+, Arch,
openSUSE Tumbleweed and Debian 13 ship. Native RPM/AUR packaging is straightforward from the same
install layout as the .deb.
