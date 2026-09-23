#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Build AppImage for ducknet-dev-tool.
# Canonical build host is Debian 13 (oldest glibc in our matrix): an AppImage
# built here runs on newer systems (Fedora 44+), but NOT vice versa. Do not
# "fix" paths for Fedora only — every host path below must probe Debian first.
# Runs on Debian 13 + Fedora 41+ (and 33 if built on older base).
# Host trust stores are directly visible (no sandbox), so src/common.rs family()
# + src/devcerts.rs detect_ca_status() check /usr/local/share/ca-certificates/myCA.crt (Debian, update-ca-certificates)
# or /etc/pki/ca-trust/source/anchors/myCA.crt (Fedora, update-ca-trust) based on
# /etc/os-release at startup. Unsupported distros show UnsupportedPage.qml.
# pkexec is used only for write (cp + update-ca-certificates/update-ca-trust) via src/common.rs privileged_output.
set -euo pipefail
# Prefer rustup cargo/rustc if available (Debian's 1.85 too old for cxx 1.0.199)
if [ -x "$HOME/.cargo/bin/cargo" ]; then export PATH="$HOME/.cargo/bin:$PATH"; fi
ROOT="$(cd "$(dirname "$0")" && pwd)"
BUILD_DIR="$ROOT/build-appimage"
APPDIR="$BUILD_DIR/AppDir"
VERSION="$(grep '^version' "$ROOT/Cargo.toml" | head -1 | cut -d'"' -f2)"
ARCH="$(uname -m)"
# The AppImage runs on systems with glibc >= the build host's. Show it so a
# Fedora-built AppImage (newer glibc) is never mistaken for portable.
echo "==> Build host: $(lsb_release -ds 2>/dev/null || grep '^PRETTY_NAME=' /etc/os-release | cut -d'"' -f2) | $(ldd --version | head -n1) | $ARCH"
echo "    (build on the OLDEST glibc system — Debian 13 — for max compatibility)"

# 1. deps check
for cmd in cmake ninja cargo file pkg-config; do
  command -v "$cmd" >/dev/null || { echo "Missing $cmd - apt install cmake ninja-build cargo file pkg-config"; exit 1; }
done
# Still need a C++ compiler: src/appicon.cpp (QIcon bridge, cxx_build) is the
# one remaining C++ file. Without it the release cargo build fails in cxx_build.
if ! command -v g++ >/dev/null 2>&1 && ! command -v clang++ >/dev/null 2>&1 && ! command -v c++ >/dev/null 2>&1; then
  echo "No C++ compiler found (need g++/clang++ for src/appicon.cpp via cxx_build) - Debian: apt install build-essential ; Fedora: dnf install gcc-c++"
  exit 1
fi
if ! pkg-config --exists Qt6Core 2>/dev/null && ! qmake6 --version >/dev/null 2>&1; then
  echo "Qt6 not found - Debian: apt install qt6-base-dev qt6-tools-dev libqt6svg6-dev qml6-module-qtquick qml6-module-org-kde-kirigami extra-cmake-modules libkf6qqc2desktopstyle-dev"
  echo "           Fedora: dnf install qt6-qtbase-devel qt6-qtdeclarative-devel qt6-qtsvg-devel kf6-kirigami extra-cmake-modules kf6-qqc2-desktop-style"
  exit 1
fi

# 1.1 rustc >=1.88 required by cxx 1.0.199 / cxx-qt 0.10
# Debian 13 ships rustc 1.85 (curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh)
RUSTC_VER="$(rustc --version | sed -n 's/rustc \([0-9]*\)\.\([0-9]*\).*/\1.\2/p')"
RUSTC_MAJOR="$(echo "$RUSTC_VER" | cut -d. -f1)"
RUSTC_MINOR="$(echo "$RUSTC_VER" | cut -d. -f2)"
NEED_MAJOR=1; NEED_MINOR=88
if [ "$RUSTC_MAJOR" -lt "$NEED_MAJOR" ] || { [ "$RUSTC_MAJOR" -eq "$NEED_MAJOR" ] && [ "$RUSTC_MINOR" -lt "$NEED_MINOR" ]; }; then
  echo "rustc $RUSTC_VER too old (need >=1.88 for cxx 1.0.199). Trying rustup..."
  if [ -x "$HOME/.cargo/bin/rustc" ]; then
    RUSTC_VER2="$("$HOME/.cargo/bin/rustc" --version | sed -n 's/rustc \([0-9]*\)\.\([0-9]*\).*/\1.\2/p')"
    echo "Found rustup rustc $RUSTC_VER2 at \$HOME/.cargo/bin/rustc - using it"
    export PATH="$HOME/.cargo/bin:$PATH"
  elif command -v rustup >/dev/null 2>&1; then
    echo "rustup found but no toolchain - running: rustup default stable"
    rustup default stable
    export PATH="$HOME/.cargo/bin:$PATH"
  else
    echo "Installing rustup (https://rustup.rs)..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    export PATH="$HOME/.cargo/bin:$PATH"
    rustup default stable || true
  fi
  echo "Now: $(rustc --version) / $(cargo --version)"
  RUSTC_VER="$(rustc --version | sed -n 's/rustc \([0-9]*\)\.\([0-9]*\).*/\1.\2/p')"
  RUSTC_MAJOR="$(echo "$RUSTC_VER" | cut -d. -f1)"
  RUSTC_MINOR="$(echo "$RUSTC_VER" | cut -d. -f2)"
  if [ "$RUSTC_MAJOR" -lt "$NEED_MAJOR" ] || { [ "$RUSTC_MAJOR" -eq "$NEED_MAJOR" ] && [ "$RUSTC_MINOR" -lt "$NEED_MINOR" ]; }; then
    echo "Still old rustc $RUSTC_VER - falling back to downgrading cxx to 1.0.130 (supports 1.85)"
    echo "  cargo update -p cxx --precise 1.0.130 && cargo update -p cxxbridge-flags --precise 1.0.130 && cargo update -p cxxbridge-macro --precise 1.0.130"
    cargo update -p cxx --precise 1.0.130 || true
    cargo update -p cxxbridge-flags --precise 1.0.130 || true
    cargo update -p cxxbridge-macro --precise 1.0.130 || true
    cargo update -p cxx-gen --precise 0.7.130 || true
  fi
fi

# 1.2 fail-fast gates (seconds now vs a wasted 30-min release burn later).
# check-qml.sh exits non-zero only on real qmllint errors (warnings pass);
# cargo test covers the Rust backends incl. the service-user script rendering.
# Full logs go to $LOG_FILE (previous `| tail` hid the real error on failure).
LOG_FILE="$ROOT/build-appimage.log"
: > "$LOG_FILE"
if [[ -x "$ROOT/check-qml.sh" ]]; then
  echo "==> QML lint (fail-fast)..."
  "$ROOT/check-qml.sh" || { echo "QML lint errors — fix before release (warnings are OK)."; exit 1; }
fi
echo "==> Rust unit tests (fail-fast)..."
cargo test --lib >>"$LOG_FILE" 2>&1 || { echo "cargo test failed — see $LOG_FILE. Fix before release."; tail -15 "$LOG_FILE"; exit 1; }

# 2. fetch linuxdeploy + plugins (if not present)
TOOLS="$ROOT/.appimage-tools"
mkdir -p "$TOOLS"
LINUXDEPLOY="$TOOLS/linuxdeploy-x86_64.AppImage"
PLUGIN_QT="$TOOLS/linuxdeploy-plugin-qt-x86_64.AppImage"
APPIMAGETOOL="$TOOLS/appimagetool-x86_64.AppImage"
if [[ ! -x "$LINUXDEPLOY" ]]; then
  echo "==> Downloading linuxdeploy..."
  curl -L https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage -o "$LINUXDEPLOY"
  chmod +x "$LINUXDEPLOY"
fi
if [[ ! -x "$PLUGIN_QT" ]]; then
  echo "==> Downloading linuxdeploy-plugin-qt..."
  curl -L https://github.com/linuxdeploy/linuxdeploy-plugin-qt/releases/download/continuous/linuxdeploy-plugin-qt-x86_64.AppImage -o "$PLUGIN_QT"
  chmod +x "$PLUGIN_QT"
fi
# NOTE (2026-09, Fedora 44): the upstream plugin-qt AppImage bundles patchelf
# 0.15.0 + an old strip, which silently corrupt binaries with .relr.dyn
# (SHT_RELR from GCC 16/binutils 2.46): .init section dropped while DT_INIT
# still points at it -> SIGSEGV in QQmlThread dlopening e.g.
# usr/qml/QtQuick/Controls/libqtquickcontrols2plugin.so. The file in
# .appimage-tools/ was repacked locally (extract, replace usr/bin/patchelf +
# usr/bin/strip with host 0.18.0/2.46.1, repack via appimagetool). Do NOT
# delete/re-download it without re-applying that fix; keep NO_STRIP=1 below.
if [[ ! -x "$APPIMAGETOOL" ]]; then
  echo "==> Downloading appimagetool..."
  curl -L https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage -o "$APPIMAGETOOL"
  chmod +x "$APPIMAGETOOL"
fi

# 3. clean + configure (Release, prefix /usr - linuxdeploy expects /usr)
echo "==> Configuring ($VERSION)... (log: $LOG_FILE)"
rm -rf "$BUILD_DIR"
cmake -B "$BUILD_DIR" -S "$ROOT" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=/usr \
  -DCMAKE_INSTALL_LIBDIR=lib \
  -GNinja >>"$LOG_FILE" 2>&1
echo "    configure done (last lines):"; tail -5 "$LOG_FILE"

echo "==> Building... (this takes a while on first run - watch: tail -f $LOG_FILE)"
cmake --build "$BUILD_DIR" -j"$(nproc)" >>"$LOG_FILE" 2>&1
echo "    build done (last lines):"; tail -10 "$LOG_FILE"

# 4. install to AppDir (DESTDIR)
echo "==> Installing to AppDir..."
DESTDIR="$APPDIR" cmake --install "$BUILD_DIR" >>"$LOG_FILE" 2>&1
tail -10 "$LOG_FILE"
# Ensure cargo binary is present (fallback if corrosion_install missed)
if [[ ! -x "$APPDIR/usr/bin/ducknet-dev-tool" && -x "$ROOT/target/release/ducknet-dev-tool" ]]; then
  mkdir -p "$APPDIR/usr/bin"
  cp -a "$ROOT/target/release/ducknet-dev-tool" "$APPDIR/usr/bin/"
fi
# Desktop + icon must be at AppDir root for linuxdeploy
cp -a "$ROOT/org.kde.ducknetdevtool.desktop" "$APPDIR/" 2>/dev/null || true
cp -a "$ROOT/org.kde.ducknetdevtool.svg" "$APPDIR/" 2>/dev/null || true
# Also ensure icon at hicolor for appimagetool
mkdir -p "$APPDIR/usr/share/icons/hicolor/scalable/apps"
cp -a "$ROOT/org.kde.ducknetdevtool.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/" 2>/dev/null || true

# 5. linuxdeploy --plugin qt bundles Qt6 + Kirigami + QQC2DesktopStyle
# QML_IMPORT_PATH is set by plugin via qmlimportscanner
# NOTE: wayland platform plugins are needed for Fedora 44 Wayland sessions.
# Host has libqwayland-*.so but plugin-qt only auto-bundles xcb, so request
# wayland explicitly AND manually copy as fallback (see step 5b).
echo "==> Bundling Qt/KDE via linuxdeploy-plugin-qt (this takes 30-60s)..."
QMAKE_BIN="$(command -v qmake6 || echo /usr/bin/qmake6)"
# NO_STRIP=1: linuxdeploy AppImages bundle an old binutils strip that cannot
# parse .relr.dyn (SHT_RELR, type 0x13) emitted by Fedora 44 / GCC 16 / binutils
# 2.46 toolchains (e.g. libQt6Core.so.6). It fails with:
#   strip: unknown type [0x13] section `.relr.dyn' -> Failed to execute deferred operations
# Host strip (2.46.1) handles RELR fine. Upstream workaround (linuxdeploy#311):
# skip bundled strip; optionally strip later with host /usr/bin/strip.
QML_SOURCES_PATHS="$ROOT/src/qml" \
QMAKE="$QMAKE_BIN" \
NO_STRIP=1 \
EXTRA_QT_PLUGINS="svg;imageformats;wayland-shell-integration;wayland-decoration-client;wayland-graphics-integration-client;platforms;" \
"$LINUXDEPLOY" --appdir "$APPDIR" \
  --desktop-file "$APPDIR/org.kde.ducknetdevtool.desktop" \
  --icon-file "$APPDIR/org.kde.ducknetdevtool.svg" \
  --plugin qt >>"$LOG_FILE" 2>&1
tail -20 "$LOG_FILE"

# 5b. Ensure Wayland platform plugins are bundled (Fedora defaults to Wayland).
# Without these, Fedora logs: qt.qpa.plugin: Could not find the Qt platform plugin "wayland" in ""
# linuxdeploy-plugin-qt only auto-bundles xcb. Plugin path differs per distro,
# so query qmake first (Fedora: /usr/lib64/qt6/plugins, Debian: /usr/lib/x86_64-linux-gnu/qt6/plugins).
QT_PLUGIN_SRC="$(qmake6 -query QT_INSTALL_PLUGINS 2>/dev/null || echo /usr/lib/x86_64-linux-gnu/qt6/plugins)"
if [ ! -d "$QT_PLUGIN_SRC/platforms" ]; then
  # Debian (build host) first, then Fedora/other layouts.
  for fallback in /usr/lib/x86_64-linux-gnu/qt6/plugins /usr/lib64/qt6/plugins /usr/lib/qt6/plugins; do
    [ -d "$fallback/platforms" ] && QT_PLUGIN_SRC="$fallback" && break
  done
fi
if [ -d "$QT_PLUGIN_SRC/platforms" ]; then
  mkdir -p "$APPDIR/usr/plugins/platforms"
  # Wayland (Fedora default) + offscreen/minimal (headless testing/CI)
  # NOTE: Qt6 file is libqwayland.so (no dash), so match libqwayland*.so
  for f in "$QT_PLUGIN_SRC"/platforms/libqwayland*.so "$QT_PLUGIN_SRC"/platforms/libqoffscreen.so "$QT_PLUGIN_SRC"/platforms/libqminimal.so; do
    [ -f "$f" ] || continue
    if [ ! -f "$APPDIR/usr/plugins/platforms/$(basename "$f")" ]; then
      echo "==> Bundling Wayland platform plugin $(basename "$f")"
      cp -a "$f" "$APPDIR/usr/plugins/platforms/"
    fi
  done
  # Wayland helper plugin dirs (shell integration, decoration, graphics integration)
  for d in wayland-shell-integration wayland-decoration-client wayland-graphics-integration-client; do
    if [ -d "$QT_PLUGIN_SRC/$d" ] && [ ! -d "$APPDIR/usr/plugins/$d" ]; then
      echo "==> Bundling Qt plugin dir $d"
      cp -a "$QT_PLUGIN_SRC/$d" "$APPDIR/usr/plugins/"
    fi
  done
fi
# 5b2. Bundle Wayland runtime libs (else Fedora logs: Could not load wayland plugin "even though it was found").
# The wayland platform plugin needs libQt6WaylandClient + libwayland-* which
# linuxdeploy does not pull in (app binary does not link them directly).
for libdir in /usr/lib/x86_64-linux-gnu /usr/lib64; do
  [ -d "$libdir" ] || continue
  for pat in 'libQt6WaylandClient.so*' 'libQt6WaylandEglClientHwIntegration.so*' 'libwayland-client.so*' 'libwayland-cursor.so*' 'libwayland-egl.so*'; do
    for f in "$libdir"/$pat; do
      [ -e "$f" ] || continue
      base="$(basename "$f")"
      if [ ! -e "$APPDIR/usr/lib/$base" ]; then
        echo "==> Bundling Wayland lib $base"
        cp -a "$f" "$APPDIR/usr/lib/"
      fi
    done
  done
done

# 5c. Deploy our QML module plugin .so into the AppDir.
# The AppImage binary has NO static QML registration (nm shows no qml_register symbols);
# it relies on the dynamic plugin liborg_kde_ducknetdevtool.so next to qmldir.
# cmake install puts only qmldir + src/qml/*.qml there, so copy the Rust cdylib.
# On the Debian dev host, cargo run works via target/debug/.../qml_modules (CARGO_MANIFEST_DIR
# baked path), but that path does not exist on Fedora — hence:
#   file:///.../Main.qml:5:1: module "org.kde.ducknetdevtool" plugin "org_kde_ducknetdevtool" not found
PLUGIN_SRC=""
for cand in "$BUILD_DIR/cargo/build/x86_64-unknown-linux-gnu/release/libducknet_dev_tool.so" \
           "$BUILD_DIR/cargo/build/x86_64-unknown-linux-gnu/release/deps/libducknet_dev_tool.so" \
           "$BUILD_DIR/bin/libducknet_dev_tool.so" \
           "$ROOT/target/release/libducknet_dev_tool.so"; do
  if [ -f "$cand" ]; then PLUGIN_SRC="$cand"; break; fi
done
if [ -n "$PLUGIN_SRC" ]; then
  echo "==> Deploying QML plugin from $PLUGIN_SRC"
  for qdir in "$APPDIR/usr/lib/x86_64-linux-gnu/qt6/qml/org/kde/ducknetdevtool" \
              "$APPDIR/usr/lib64/qt6/qml/org/kde/ducknetdevtool" \
              "$APPDIR/usr/lib/qt6/qml/org/kde/ducknetdevtool" \
              "$APPDIR/usr/qml/org/kde/ducknetdevtool"; do
    if [ -d "$qdir" ]; then
      cp -a "$PLUGIN_SRC" "$qdir/liborg_kde_ducknetdevtool.so"
      echo "    -> $qdir/liborg_kde_ducknetdevtool.so"
    fi
  done
  # Also mirror the module under usr/qml (matches qt.conf Imports = qml) if missing.
  # Module dir varies by distro (Debian multiarch vs Fedora lib64) — probe all.
  for modsrc in "$APPDIR/usr/lib/x86_64-linux-gnu/qt6/qml/org/kde/ducknetdevtool" \
                "$APPDIR/usr/lib64/qt6/qml/org/kde/ducknetdevtool" \
                "$APPDIR/usr/lib/qt6/qml/org/kde/ducknetdevtool"; do
    if [ -d "$modsrc" ] && [ ! -d "$APPDIR/usr/qml/org/kde/ducknetdevtool" ]; then
      echo "==> Mirroring QML module to usr/qml (qt.conf Imports path)"
      mkdir -p "$APPDIR/usr/qml/org/kde/ducknetdevtool"
      cp -a "$modsrc/." "$APPDIR/usr/qml/org/kde/ducknetdevtool/"
      break
    fi
  done
  # Strip 'prefer :/qt/qml/...' from AppDir qmldir(s) — binary has no qrc resources,
  # prefer would force qrc lookup and break file-based loading.
  find "$APPDIR/usr" -path "*org/kde/ducknetdevtool/qmldir" | while read -r qm; do
    if grep -q "^prefer" "$qm"; then
      echo "==> Stripping prefer line from $qm"
      sed -i '/^prefer /d' "$qm"
    fi
    echo "--- $qm ---"; cat "$qm"
  done
else
  echo "WARNING: libducknet_dev_tool.so not found, QML plugin will be missing!"
fi
# Clean stray double-DESTDIR artifact (CMake file(INSTALL) prepends DESTDIR itself)
if [ -d "$APPDIR/home" ]; then
  echo "==> Removing stray $APPDIR/home (double-DESTDIR artifact)"
  rm -rf "$APPDIR/home"
fi

# linuxdeploy-plugin-qt should have deployed qml modules; verify (KDE_INSTALL_QMLDIR varies: /usr/lib/*/qt6/qml vs /usr/qml)
for q in "$APPDIR/usr/lib/x86_64-linux-gnu/qt6/qml/org/kde/ducknetdevtool" "$APPDIR/usr/lib64/qt6/qml/org/kde/ducknetdevtool" "$APPDIR/usr/lib/qt6/qml/org/kde/ducknetdevtool" "$APPDIR/usr/qml/org/kde/ducknetdevtool"; do
  if [ -d "$q" ]; then echo "QML module at $q:"; ls -R "$q" 2>&1 | head -30; echo "qmldir:"; cat "$q/qmldir" 2>&1 || echo "qmldir missing at $q"; break; fi
done
ls -R "$APPDIR/usr/bin" 2>&1 | head -20 || true
ls "$APPDIR/usr/bin/ducknet-dev-tool" 2>&1 | head
# Verify all QML files bundled (Main, DevCertsPage, CertHelpPage, UnsupportedPage via src/qml -> qml_modules)
for f in Main.qml DevCertsPage.qml CertHelpPage.qml UnsupportedPage.qml SystemInfoPage.qml NginxManagerPage.qml EditSitePage.qml EditNginxBasePage.qml EditPhpfpmBasePage.qml SetupToolingPage.qml EditorFindBar.qml DatabaseManagerPage.qml CreateDatabasePage.qml; do
  find "$APPDIR" -name "$f" 2>/dev/null | head -3 || echo "WARN: $f not found in AppDir"
done
# Verify the Rust QML backends are registered in the deployed plugin.
# (A stale qmltyperegistrar once shipped a plugin without a new type —
#  every backend below must appear, else its page fails with "not a type".)
for t in DevCertsManager NginxManager SystemInfoManager ToolingManager AppInfo DatabaseManager; do
  if ! strings "$APPDIR"/usr/lib/x86_64-linux-gnu/qt6/qml/org/kde/ducknetdevtool/liborg_kde_ducknetdevtool.so \
      "$APPDIR"/usr/lib64/qt6/qml/org/kde/ducknetdevtool/liborg_kde_ducknetdevtool.so \
      "$APPDIR"/usr/lib/qt6/qml/org/kde/ducknetdevtool/liborg_kde_ducknetdevtool.so \
      "$APPDIR"/usr/qml/org/kde/ducknetdevtool/liborg_kde_ducknetdevtool.so 2>/dev/null | grep -q "$t"; then
    echo "WARN: $t not found in deployed liborg_kde_ducknetdevtool.so"
  fi
done
# Verify the BUNDLED DatabaseManagerPage actually wires the current backend
# properties — the backend can log healthy values (enable_visible=true) while
# a stale page bundled here silently renders no button, with zero errors.
# Fail the build: a UI without its backend's controls must never ship.
_BUNDLED_DBPAGE="$(find "$APPDIR" -path "*org/kde/ducknetdevtool/src/qml/DatabaseManagerPage.qml" 2>/dev/null | head -1 || true)"
if [[ -z "$_BUNDLED_DBPAGE" ]]; then
  echo "ERROR: DatabaseManagerPage.qml not found in AppDir — refusing to ship."
  exit 1
fi
echo "==> Verifying bundled UI: $_BUNDLED_DBPAGE"
for marker in "mariadbEnableVisible" "enableMariadb"; do
  if ! grep -q "$marker" "$_BUNDLED_DBPAGE"; then
    echo "ERROR: bundled DatabaseManagerPage.qml lacks '$marker' (stale UI — backend exposes it, page never binds it)."
    exit 1
  fi
done
# Welcome-page banner (CMake png stanza installs it next to Main.qml; the
# inline Image in Main.qml resolves it relatively, so it must ship in-tree).
for f in workingduck_wide_bw_400.png; do
  if ! find "$APPDIR" -path "*org/kde/ducknetdevtool/src/qml/images/$f" 2>/dev/null | grep -q .; then
    echo "WARN: $f not found under org/kde/ducknetdevtool/src/qml/images/ in AppDir"
  fi
done

# 6. create AppImage
OUT="DuckNet-Dev-Tool-${VERSION}-${ARCH}.AppImage"
# Remove old
rm -f "$ROOT/$OUT"
# appimagetool needs ARCH env
ARCH="$ARCH" "$APPIMAGETOOL" "$APPDIR" "$ROOT/$OUT" >>"$LOG_FILE" 2>&1
tail -20 "$LOG_FILE"
chmod +x "$ROOT/$OUT"
ls -lh "$ROOT/$OUT"

echo ""
echo "==> AppImage: $ROOT/$OUT"
echo "    Run: ./$OUT  (no sandbox - host trust store visible)"
echo "    Debian: /usr/local/share/ca-certificates/myCA.crt + update-ca-certificates"
echo "    Fedora: /etc/pki/ca-trust/source/anchors/myCA.crt + update-ca-trust"
echo "    Unsupported (Arch etc): shows UnsupportedPage.qml"
echo "    Install cert: click Install in app -> pkexec prompt -> cp + update-ca-* on host (distro-aware via /etc/os-release)"
echo "    Test on Fedora: copy $OUT to Fedora 41+ and ./$OUT (built on Debian 13 = oldest glibc, runs on newer — never build the release AppImage on Fedora)"
echo "    Deps not bundled: openssl (host), pkexec (polkit), ca-certificates/ca-trust"
