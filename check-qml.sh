#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Warn-only QML lint (qmllint) for ducknet-dev-tool.
# Intentionally NOT wired into build.rs / CMake: missing Qt dev tools must
# never break `cargo build`, and qmllint diagnostics are advisory.
# Usage: ./check-qml.sh [--format]  (--format also runs qmlformat --dry-run)
set -uo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
FORMAT=0
if [[ "${1:-}" == "--format" ]]; then FORMAT=1; fi

# 1. Locate qmllint (not on PATH on Debian: /usr/lib/qt6/bin/qmllint).
QMLLINT=""
for cand in qmllint /usr/lib/qt6/bin/qmllint; do
  if command -v "$cand" >/dev/null 2>&1; then QMLLINT="$cand"; break; fi
  if [[ -x "$cand" ]]; then QMLLINT="$cand"; break; fi
done
if [[ -z "$QMLLINT" ]] && command -v qmake6 >/dev/null 2>&1; then
  QBINS="$(qmake6 -query QT_INSTALL_BINS 2>/dev/null || true)"
  [[ -x "$QBINS/qmllint" ]] && QMLLINT="$QBINS/qmllint"
fi
if [[ -z "$QMLLINT" ]]; then
  echo "check-qml: qmllint not found (Debian: apt install qt6-declarative-dev-tools) — skipping."
  exit 0
fi
echo "check-qml: using $QMLLINT ($("$QMLLINT" --version 2>&1 | head -n1))"

# 2. Import paths: generated module dir first (resolves org.kde.ducknetdevtool
#    Rust types, cuts false positives), then system layouts (Debian first).
ARGS=()
for mod in "$ROOT"/target/debug/build/*/out/qt-build-utils/qml_modules \
           "$ROOT"/target/cxxqt/qml_modules; do
  if [[ -d "$mod" ]]; then ARGS+=(-I "$mod"); break; fi
done
for p in /usr/lib/x86_64-linux-gnu/qt6/qml /usr/lib64/qt6/qml \
         /usr/lib/qt6/qml /usr/lib/qml; do
  [[ -d "$p" ]] && ARGS+=(-I "$p")
done

# 3. Lint every page. qmllint exits 0 on warnings, non-zero on errors —
#    so this stays advisory for style issues but loud for real breakage.
mapfile -t FILES < <(ls "$ROOT"/src/qml/*.qml)
echo "check-qml: linting ${#FILES[@]} file(s)..."
RC=0
"$QMLLINT" "${ARGS[@]}" "${FILES[@]}" || RC=$?
if [[ $RC -eq 0 ]]; then
  echo "check-qml: OK (warnings above are advisory, errors would fail)."
else
  echo "check-qml: FAILED (qmllint exit $RC) — fix errors above."
  exit $RC
fi

# 4. Optional formatting check.
if [[ $FORMAT -eq 1 ]]; then
  QMLFORMAT=""
  for cand in qmlformat /usr/lib/qt6/bin/qmlformat; do
    if command -v "$cand" >/dev/null 2>&1; then QMLFORMAT="$cand"; break; fi
    if [[ -x "$cand" ]]; then QMLFORMAT="$cand"; break; fi
  done
  if [[ -z "$QMLFORMAT" ]]; then
    echo "check-qml --format: qmlformat not found — skipping."
    exit 0
  fi
  FRC=0
  for f in "${FILES[@]}"; do
    # qmlformat 6.8 has no --dry-run: format to stdout and diff.
    if ! "$QMLFORMAT" "$f" 2>/dev/null | diff -q - "$f" >/dev/null; then
      echo "check-qml --format: needs formatting: ${f#$ROOT/}"
      FRC=1
    fi
  done
  if [[ $FRC -ne 0 ]]; then
    echo "check-qml --format: files need formatting (run qmlformat -i src/qml/*.qml)."
    exit 1
  fi
  echo "check-qml --format: all files formatted."
fi
