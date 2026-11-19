#!/usr/bin/env bash
# Build a self-contained macOS app and disk image:
#   dist/Oxishell.app
#   dist/oxishell_<version>_macos_<arch>.dmg      (arch: arm64 or x86_64)
#
# The GTK 4 / libadwaita libraries come from Homebrew, and VTE from a source build
# (VTE_SOURCE=1, default) or Homebrew (VTE_SOURCE=0); all are copied into the
# bundle, so users need neither Homebrew nor any other install.
# Requires (Homebrew): gtk4 libadwaita vte3 librsvg adwaita-icon-theme
#                      pkgconf dylibbundler meson ninja
# plus Rust and the Xcode command line tools (Xcode 16.3 or newer for the VTE
# build). Run from anywhere.
#
# VTE_SOURCE=1 (the default) builds VTE itself without ICU, GTK 4 only, into
# target/vte-noicu/<version>-<arch> (reused on later runs) and bundles that
# instead of Homebrew's vte3, which drags in ~30 MiB of ICU for legacy charsets
# oxishell never uses (terminals are always UTF-8). vte3 stays installed: it
# provides VTE's dependencies. VTE_SOURCE=0 bundles Homebrew's vte3 as before.
#
# The Adwaita cursor theme is dropped (GDK on macOS uses NSCursor), `oxi` is
# checked to not link GTK/libadwaita/VTE, and bundle sizes are printed as
# "size: ..." lines for CI logs.
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

# --- VTE without ICU (VTE_SOURCE=1) -------------------------------------------
VTE_SOURCE=${VTE_SOURCE:-1}
case $VTE_SOURCE in
    0 | 1) ;;
    *) echo "VTE_SOURCE must be 0 or 1, not '$VTE_SOURCE'" >&2; exit 1 ;;
esac
if [[ $VTE_SOURCE == 1 ]]; then
    VTE_VERSION=0.84.1
    VTE_SHA256=aca1caa8478aebcdbb1d67897fb3511eb7601debae6810e16a15b6fa25f31ac8
    VTE_PREFIX=$PWD/target/vte-noicu/$VTE_VERSION-$ARCH
    VTE_LIB=$VTE_PREFIX/lib/libvte-2.91-gtk4.0.dylib

    # A previous build is reused while it is complete, its install name is still
    # this path, and every Homebrew library it links to still exists (a Homebrew
    # upgrade can replace one, e.g. a new simdutf major version).
    vte_cached() {
        local dep
        [[ -f $VTE_PREFIX/lib/pkgconfig/vte-2.91-gtk4.pc && -f $VTE_LIB ]] || return 1
        [[ $(otool -D "$VTE_LIB" | tail -n 1) == "$VTE_LIB" ]] || return 1
        for dep in $(otool -L "$VTE_LIB" | awk 'NR > 1 {print $1}'); do
            case $dep in
                "$BREW"/*) [[ -e $dep ]] || { echo "VTE build links to missing $dep; rebuilding" >&2; return 1; } ;;
            esac
        done
    }

    if vte_cached; then
        echo "VTE $VTE_VERSION without ICU: reusing $VTE_PREFIX"
    else
        for tool in meson ninja curl shasum; do
            command -v "$tool" >/dev/null || { echo "missing tool: $tool (needed for VTE_SOURCE=1, see the header of $0)" >&2; exit 1; }
        done
        # VTE 0.84 uses C++23 std::out_ptr: Apple clang 17 (Xcode 16.3) or newer,
        # the same limit Homebrew's vte3 formula has.
        clang_build=$(c++ --version | sed -n 's/.*(clang-\([0-9]*\).*/\1/p' | head -n 1)
        if [[ -n $clang_build && $clang_build -lt 1700 ]]; then
            echo "VTE $VTE_VERSION needs Xcode 16.3 or newer (Apple clang build 1700+, found $clang_build), or build with VTE_SOURCE=0" >&2
            exit 1
        fi
        VTE_BUILD=$PWD/target/vte-noicu-build
        rm -rf "$VTE_BUILD" "$VTE_PREFIX"
        mkdir -p "$VTE_BUILD"
        curl -fsSL --retry 3 -o "$VTE_BUILD/vte.tar.xz" \
            "https://download.gnome.org/sources/vte/${VTE_VERSION%.*}/vte-$VTE_VERSION.tar.xz"
        got=$(shasum -a 256 "$VTE_BUILD/vte.tar.xz" | cut -d' ' -f1)
        if [[ $got != "$VTE_SHA256" ]]; then
            echo "vte-$VTE_VERSION.tar.xz: SHA-256 $got, expected $VTE_SHA256" >&2
            exit 1
        fi
        tar -xJf "$VTE_BUILD/vte.tar.xz" -C "$VTE_BUILD"
        # Option names from the release's meson_options.txt. The tarball carries the
        # fmt, simdutf and fast_float subprojects; nodownload keeps meson offline.
        # The other dependencies (gtk4, pango, gnutls, pcre2, lz4, fribidi,
        # simdutf...) come from Homebrew, installed with vte3.
        PKG_CONFIG_PATH="$BREW/lib/pkgconfig:$BREW/share/pkgconfig:${PKG_CONFIG_PATH:-}" \
            meson setup "$VTE_BUILD/build" "$VTE_BUILD/vte-$VTE_VERSION" \
            --prefix "$VTE_PREFIX" --libdir lib --buildtype release --wrap-mode nodownload \
            -Dgtk3=false -Dgtk4=true -Dicu=false -Dgir=false -Dvapi=false -Ddocs=false \
            -Dapp=false -Dglade=false -D_systemd=false -D_b_symbolic_functions=false
        ninja -C "$VTE_BUILD/build" install
        # oxishell's load command (and so dylibbundler) uses the install name; it
        # must be this absolute path, not @rpath.
        if [[ $(otool -D "$VTE_LIB" | tail -n 1) != "$VTE_LIB" ]]; then
            install_name_tool -id "$VTE_LIB" "$VTE_LIB"
            codesign --force -s - "$VTE_LIB"
        fi
        rm -rf "$VTE_BUILD"
        vte_cached || { echo "VTE build in $VTE_PREFIX is incomplete" >&2; exit 1; }
        echo "VTE $VTE_VERSION without ICU: built into $VTE_PREFIX"
    fi
    export PKG_CONFIG_PATH="$VTE_PREFIX/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
    # pkg-config alone is not enough to pick this VTE: every -sys crate adds its
    # Homebrew -L dirs to the final link, gtk4-sys & co. come before vte4-sys,
    # and one of those dirs also holds Homebrew's libvte-2.91-gtk4.dylib, so
    # `-lvte-2.91-gtk4` resolves there first. Link through a name only this
    # prefix has instead; ld records the dylib's install name ($VTE_LIB), not
    # the symlink. The system-deps overrides keep the pkg-config version check.
    ln -sf libvte-2.91-gtk4.0.dylib "$VTE_PREFIX/lib/liboxishell-vte-noicu.dylib"
    export SYSTEM_DEPS_VTE_2_91_GTK4_SEARCH_NATIVE="$VTE_PREFIX/lib"
    export SYSTEM_DEPS_VTE_2_91_GTK4_LIB=oxishell-vte-noicu
fi

cargo build --release --locked

if [[ $VTE_SOURCE == 1 ]]; then
    # The vte4-sys build script reruns when PKG_CONFIG_PATH changes; check anyway,
    # since linking Homebrew's libvte would bring ICU back.
    vte_link=$(otool -L target/release/oxishell | awk '/libvte/ {print $1}')
    if [[ $vte_link != "$VTE_LIB" ]]; then
        echo "oxishell links '$vte_link', not the ICU-free $VTE_LIB" >&2
        exit 1
    fi
fi

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
# -L: in Homebrew share/icons/Adwaita is a symlink into the Cellar; copy the files.
cp -RL "$BREW/share/icons/Adwaita" "$R/share/icons/"
# GDK's macOS backend maps cursor names to NSCursor and never reads Xcursor
# themes, so the cursor theme is dead weight in the bundle.
if [[ -d $R/share/icons/Adwaita/cursors ]]; then
    echo "size: cursors removed $(du -sk "$R/share/icons/Adwaita/cursors" | cut -f1) KiB"
    rm -rf "$R/share/icons/Adwaita/cursors"
fi
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
# ${a[@]+...}: an empty array is an "unbound variable" for set -u in macOS's bash 3.2.
for l in ${LOADERS[@]+"${LOADERS[@]}"}; do FIX+=(-x "$l"); done
SEARCH=()
if [[ $VTE_SOURCE == 1 ]]; then SEARCH+=(-s "$VTE_PREFIX/lib"); fi
dylibbundler -of -cd -b "${FIX[@]}" -d "$C/Frameworks" -p @executable_path/../Frameworks/ \
    ${SEARCH[@]+"${SEARCH[@]}"} -s "$BREW/lib"
# dylibbundler rewrites what the loaders link to, but not their own install
# name (LC_ID_DYLIB), which still points into Homebrew.
for l in ${LOADERS[@]+"${LOADERS[@]}"}; do
    install_name_tool -id "@executable_path/../Resources/$PIXBUF_REL/loaders/$(basename "$l")" "$l"
done

# The oxi helper must stay GUI-free: build.rs links it with -dead_strip_dylibs,
# so GTK, libadwaita and VTE must not appear among its load commands.
oxi_libs=$(otool -L "$C/MacOS/oxi" | grep -E 'libgtk|libadwaita|libvte' || true)
if [[ -n $oxi_libs ]]; then
    echo "oxi links GUI libraries (dead_strip_dylibs not effective):" >&2
    echo "$oxi_libs" >&2
    exit 1
fi

# With VTE_SOURCE=1 no ICU may be bundled. If this fails, find the library that
# links it below rather than deleting ICU (VTE_SOURCE=0 restores the old build).
if [[ $VTE_SOURCE == 1 ]]; then
    icu_files=$(find "$C/Frameworks" -name 'libicu*')
    if [[ -n $icu_files ]]; then
        echo "ICU is bundled although VTE_SOURCE=1:" >&2
        echo "$icu_files" >&2
        echo "linked by:" >&2
        find "$C/MacOS" "$C/Frameworks" "$R/lib" -type f \( -perm -u+x -o -name '*.dylib' -o -name '*.so' \) \
            | while IFS= read -r f; do otool -L "$f" | awk -v f="$f" '/^[[:space:]]/ && /libicu/ {print f ":" $0}'; done >&2
        exit 1
    fi
    # Informational: the ICU libraries Homebrew's vte3 would have added.
    brew_vte=$BREW/lib/libvte-2.91-gtk4.0.dylib
    if [[ -f $brew_vte ]]; then
        icu_kib=0
        icu_direct=$(otool -L "$brew_vte" | awk '/^[[:space:]]/ && /libicu/ {print $1}' || true)
        icu_all=$(for f in $brew_vte $icu_direct; do
            if [[ -f $f ]]; then otool -L "$f" | awk '/^[[:space:]]/ && /libicu/ {print $1}' || true; fi
        done | sort -u)
        for f in $icu_all; do
            if [[ -f $f ]]; then icu_kib=$((icu_kib + $(du -kL "$f" | cut -f1))); fi
        done
        echo "size: libicu avoided $icu_kib KiB (what Homebrew's vte3 would have bundled)"
    fi
fi

# Nothing may still point into Homebrew or the build tree (target/vte-noicu), or
# the app breaks on other Macs.
leaks=$(find "$C/MacOS" "$C/Frameworks" "$R/lib" -type f \( -perm -u+x -o -name '*.dylib' -o -name '*.so' \) \
    -exec otool -L {} \; | grep -E "^[[:space:]]+($BREW|/usr/local/(opt|Cellar)|$PWD/target)" || true)
if [[ -n $leaks ]]; then
    echo "bundle still links to Homebrew or build-tree paths:" >&2
    echo "$leaks" >&2
    exit 1
fi

# Links inside the bundle break signing ("No such file or directory") or point
# outside it; everything must be a real file.
links=$(find "$APP" -type l)
if [[ -n $links ]]; then
    echo "bundle contains symlinks:" >&2
    echo "$links" >&2
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
echo "size: app $(du -sk "$APP" | cut -f1) KiB"
echo "size: Frameworks $(du -sk "$C/Frameworks" | cut -f1) KiB"
echo "size: dmg $(( $(stat -f %z "$DMG") / 1024 )) KiB"
