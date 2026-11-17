#!/usr/bin/env bash
# Build a self-contained macOS app and disk image:
#   dist/Oxishell.app
#   dist/oxishell_<version>_macos_<arch>.dmg      (arch: arm64 or x86_64)
#
# The GTK 4 / libadwaita / VTE libraries come from Homebrew and are copied into
# the bundle, so users need neither Homebrew nor any other install.
# Requires (Homebrew): gtk4 libadwaita vte3 librsvg adwaita-icon-theme
#                      pkgconf dylibbundler
# plus Rust and the Xcode command line tools. Run from anywhere.
#
# The app is signed ad hoc (required on Apple silicon), not with a Developer ID,
# so on first launch users right-click > Open (see README).
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ $(uname -s) != Darwin ]]; then
    echo "build-macos.sh runs on macOS; on Linux use scripts/build-deb.sh" >&2
    exit 1
fi
for tool in brew cargo dylibbundler rsvg-convert iconutil hdiutil codesign glib-compile-schemas gdk-pixbuf-query-loaders; do
    command -v "$tool" >/dev/null || { echo "missing tool: $tool (see the header of $0)" >&2; exit 1; }
done

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
ARCH=$(uname -m)
BREW=$(brew --prefix)
# Homebrew's libraries are built for this macOS release, so the app needs it too.
MIN_MACOS=${MIN_MACOS:-$(sw_vers -productVersion | cut -d. -f1).0}

export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct 2>/dev/null || stat -f %m Cargo.lock)}
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$PWD=. --remap-path-prefix=$HOME/.cargo=cargo"
export MACOSX_DEPLOYMENT_TARGET=$MIN_MACOS
cargo build --release --locked

APP=dist/Oxishell.app
C=$APP/Contents
rm -rf "$APP" dist/*.dmg
mkdir -p "$C/MacOS" "$C/Resources" "$C/Frameworks"

# --- binaries and metadata ----------------------------------------------------
install -m755 target/release/oxishell "$C/MacOS/oxishell"
install -m755 target/release/oxi "$C/MacOS/oxi"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@MIN_MACOS@/$MIN_MACOS/g" data/macos/Info.plist.in > "$C/Info.plist"
plutil -lint "$C/Info.plist" >/dev/null
printf 'APPL????' > "$C/PkgInfo"
mkdir -p "$C/Resources/doc"
cp README.md LICENSE docs/RELEASE_NOTES.md "$C/Resources/doc/"
cp data/fonts/LICENSE.md "$C/Resources/doc/LICENSE-Hack-fonts.md"
cp data/themes/LICENSE-warp-themes "$C/Resources/doc/"

# --- icon -----------------------------------------------------------------------
ICONSET=$(mktemp -d)/oxishell.iconset
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
    rsvg-convert -w $s -h $s data/icons/dev.oxishell.Oxishell.svg -o "$ICONSET/icon_${s}x${s}.png"
    rsvg-convert -w $((s * 2)) -h $((s * 2)) data/icons/dev.oxishell.Oxishell.svg -o "$ICONSET/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$ICONSET" -o "$C/Resources/oxishell.icns"

# --- GTK runtime data (found at startup by platform::macos::setup_bundle_env) ---
R=$C/Resources
# GSettings schemas GTK reads (file chooser, colour chooser, emoji chooser...).
mkdir -p "$R/share/glib-2.0/schemas"
cp "$BREW"/share/glib-2.0/schemas/org.gtk.gtk4.*.gschema.xml "$R/share/glib-2.0/schemas/"
glib-compile-schemas "$R/share/glib-2.0/schemas"
# Icon themes: Adwaita for the symbolic icons, hicolor as the base theme.
mkdir -p "$R/share/icons"
cp -R "$BREW/share/icons/Adwaita" "$R/share/icons/"
mkdir -p "$R/share/icons/hicolor"
cp "$BREW/share/icons/hicolor/index.theme" "$R/share/icons/hicolor/"
mkdir -p "$R/share/icons/hicolor/scalable/apps"
cp data/icons/dev.oxishell.Oxishell.svg "$R/share/icons/hicolor/scalable/apps/"
# Fontconfig configuration (Pango may consult it next to CoreText).
if [[ -d $BREW/etc/fonts ]]; then
    mkdir -p "$R/etc"
    cp -RL "$BREW/etc/fonts" "$R/etc/"
fi
# gdk-pixbuf loaders, including librsvg's SVG loader for symbolic icons. The
# cache lists absolute paths, so it is shipped as a template: @RESOURCES@ is
# replaced with the real location when the app starts.
PIXBUF_REL=lib/gdk-pixbuf-2.0/2.10.0
BREW_LOADERS=$BREW/$PIXBUF_REL/loaders
mkdir -p "$R/$PIXBUF_REL/loaders"
LOADERS=()
for f in "$BREW_LOADERS"/*.so "$BREW_LOADERS"/*.dylib; do
    [[ -e $f ]] || continue
    cp -L "$f" "$R/$PIXBUF_REL/loaders/"
    LOADERS+=("$R/$PIXBUF_REL/loaders/$(basename "$f")")
done
GDK_PIXBUF_MODULEDIR=$BREW_LOADERS gdk-pixbuf-query-loaders \
    | sed "s|$BREW_LOADERS|@RESOURCES@/$PIXBUF_REL/loaders|g" > "$R/$PIXBUF_REL/loaders.cache.in"

# --- bundle the shared libraries -------------------------------------------------
# Copies every non-system dylib the binaries and loaders use into Frameworks and
# rewrites their load paths to @executable_path/../Frameworks.
FIX=(-x "$C/MacOS/oxishell" -x "$C/MacOS/oxi")
for l in "${LOADERS[@]}"; do FIX+=(-x "$l"); done
dylibbundler -of -cd -b "${FIX[@]}" -d "$C/Frameworks" -p @executable_path/../Frameworks/ -s "$BREW/lib"

# Nothing may still point into Homebrew, or the app breaks on Macs without it.
leaks=$(find "$C/MacOS" "$C/Frameworks" "$R/lib" -type f \( -perm -u+x -o -name '*.dylib' -o -name '*.so' \) \
    -exec otool -L {} \; | grep -E "^[[:space:]]+($BREW|/usr/local/(opt|Cellar))" || true)
if [[ -n $leaks ]]; then
    echo "bundle still links to Homebrew paths:" >&2
    echo "$leaks" >&2
    exit 1
fi

# --- sign (ad hoc) ---------------------------------------------------------------
# Every Mach-O file must be signed after its load paths were rewritten.
find "$C/Frameworks" "$R/lib" -type f \( -name '*.dylib' -o -name '*.so' \) -exec codesign --force -s - {} \;
codesign --force -s - "$C/MacOS/oxi"
codesign --force -s - "$APP"
codesign --verify --strict "$APP"
# Developer ID signing and notarization, once an Apple Developer account exists:
#   codesign --force --deep --options runtime --timestamp -s "$MACOS_SIGN_IDENTITY" "$APP"
#   xcrun notarytool submit "$DMG" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
#       --password "$APPLE_APP_PASSWORD" --wait
#   xcrun stapler staple "$DMG"

# --- disk image ------------------------------------------------------------------
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
DMG=dist/oxishell_${VERSION}_macos_${ARCH}.dmg
hdiutil create -volname "oxishell $VERSION" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DMG" >/dev/null
echo "Built $DMG"
