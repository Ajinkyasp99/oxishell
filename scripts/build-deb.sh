#!/usr/bin/env bash
# Build a Debian/Ubuntu package: dist/cmux-linux_<version>_<arch>.deb
# Requires: cargo, dpkg-dev (dpkg-deb, dpkg-shlibdeps). Run from the repo root.
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
ARCH=$(dpkg --print-architecture)
PKG=cmux-linux
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

# Reproducible-ish: fixed timestamps and no embedded local paths.
# Timestamp: $SOURCE_DATE_EPOCH, else the last commit time, else Cargo.lock mtime.
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct 2>/dev/null || stat -c %Y Cargo.lock)}
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$PWD=. --remap-path-prefix=$HOME/.cargo=cargo"
cargo build --release --locked

R=$STAGE/root
install -Dm755 target/release/cmux-linux "$R/usr/bin/cmux-linux"
install -Dm755 target/release/cmux "$R/usr/bin/cmux"
install -Dm644 data/dev.cmux.Linux.desktop "$R/usr/share/applications/dev.cmux.Linux.desktop"
install -Dm644 data/dev.cmux.Linux.metainfo.xml "$R/usr/share/metainfo/dev.cmux.Linux.metainfo.xml"
install -Dm644 data/icons/dev.cmux.Linux.svg "$R/usr/share/icons/hicolor/scalable/apps/dev.cmux.Linux.svg"
install -Dm644 README.md "$R/usr/share/doc/$PKG/README.md"
install -Dm644 docs/RELEASE_NOTES.md "$R/usr/share/doc/$PKG/RELEASE_NOTES.md"
install -Dm644 docs/TROUBLESHOOTING.md "$R/usr/share/doc/$PKG/TROUBLESHOOTING.md"
install -Dm644 data/themes/LICENSE-warp-themes "$R/usr/share/doc/$PKG/LICENSE-warp-themes"
install -Dm644 data/themes/NOTICE.md "$R/usr/share/doc/$PKG/THEMES-NOTICE.md"
install -Dm644 data/fonts/LICENSE.md "$R/usr/share/doc/$PKG/LICENSE-Hack.md"
install -Dm644 data/fonts/NOTICE.md "$R/usr/share/doc/$PKG/FONTS-NOTICE.md"
cat > "$R/usr/share/doc/$PKG/copyright" <<C
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: cmux-linux
Comment: Linux-native terminal inspired by cmux (https://github.com/manaflow-ai/cmux).
Files: *
Copyright: Ajinkya Patil and cmux-linux contributors
License: GPL-3.0-or-later
 On Debian systems, the full text is in /usr/share/common-licenses/GPL-3.

Files: data/themes/*
Copyright: Warp and the original theme authors
License: Apache-2.0
 Theme data compiled into the binary comes from https://github.com/warpdotdev/themes.
 See /usr/share/doc/cmux-linux/LICENSE-warp-themes.

Files: data/fonts/*
Copyright: 2018 Source Foundry Authors; 2003 Bitstream Inc.
License: MIT and Bitstream-Vera
 Unmodified Hack fonts, bundled in the binary.
 See /usr/share/doc/cmux-linux/LICENSE-Hack.md and FONTS-NOTICE.md.
C

chmod 644 "$R/usr/share/doc/$PKG/copyright"

# Shared-library dependencies computed from the binaries.
mkdir -p "$STAGE/debian"
printf 'Source: %s\n\nPackage: %s\nArchitecture: any\n' "$PKG" "$PKG" > "$STAGE/debian/control"
DEPS=$(cd "$STAGE" && dpkg-shlibdeps -O "root/usr/bin/cmux-linux" "root/usr/bin/cmux" 2>/dev/null | sed -n 's/^shlibs:Depends=//p')
[ -n "$DEPS" ] || { echo "dpkg-shlibdeps failed" >&2; exit 1; }

mkdir -p "$R/DEBIAN"
SIZE=$(du -sk --exclude=DEBIAN "$R" | cut -f1)
cat > "$R/DEBIAN/control" <<C
Package: $PKG
Version: $VERSION
Architecture: $ARCH
Maintainer: Ajinkya Patil <Ajinkyasp99@users.noreply.github.com>
Installed-Size: $SIZE
Depends: $DEPS
Recommends: git
Section: x11
Priority: optional
Homepage: https://github.com/Ajinkyasp99/cmux_linux
Description: terminal workspaces and split panes for AI coding agents
 Native GTK4/libadwaita terminal (VTE) with a vertical workspace sidebar,
 nested split panes, git/port metadata, notification rings for coding agents
 (Claude Code, Codex, Gemini CLI, Qoder, OpenCode), a command palette and a
 local CLI/socket API ("cmux"). Inspired by cmux for macOS.
C
# Refresh desktop caches when available (no-ops otherwise).
cat > "$R/DEBIAN/postinst" <<'C'
#!/bin/sh
set -e
command -v update-desktop-database >/dev/null && update-desktop-database -q /usr/share/applications || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t /usr/share/icons/hicolor || true
exit 0
C
cp "$R/DEBIAN/postinst" "$R/DEBIAN/postrm"
chmod 755 "$R/DEBIAN/postinst" "$R/DEBIAN/postrm"
find "$R" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +

mkdir -p dist
OUT=dist/${PKG}_${VERSION}_${ARCH}.deb
dpkg-deb --root-owner-group --build "$R" "$OUT" >/dev/null
echo "$OUT"
