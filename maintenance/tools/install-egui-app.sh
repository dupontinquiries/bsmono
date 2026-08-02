#!/usr/bin/env bash
# install-egui-app.sh — install one egui frontend the way a Linux desktop expects.
#
# Modelled on rusty-board/scripts/install.sh, which was the only app with a real
# installer, rather than inventing a second convention.
#
# What "installed" means beyond copying a binary:
#
#   * the .desktop entry in share/applications, or the app never appears in a menu;
#   * the icon in share/icons/hicolor/scalable/apps under the SAME name the entry's
#     Icon= line gives — a mismatch here leaves a working app with a blank tile;
#   * the icon cache refreshed, because GTK and several launchers read the cache
#     rather than the directory and will not see a new icon until it is rebuilt.
#
# Usage:  install-egui-app.sh <app-dir> [--prefix DIR] [--uninstall]
set -euo pipefail

APP_DIR="${1:?usage: install-egui-app.sh <app-dir> [--prefix DIR] [--uninstall]}"
shift || true
PREFIX="${HOME}/.local"
UNINSTALL=0
while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    --uninstall) UNINSTALL=1; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

PKG="$APP_DIR/packaging"
[ -d "$PKG" ] || { echo "ERROR: no packaging/ in $APP_DIR" >&2; exit 1; }

ENTRY="$(find "$PKG" -maxdepth 1 -name '*.desktop' | head -1)"
[ -n "$ENTRY" ] || { echo "ERROR: no .desktop entry in $PKG" >&2; exit 1; }
APP_ID="$(basename "$ENTRY" .desktop)"

# The Icon= line is the contract between entry and artwork. Reading it rather
# than assuming it equals the app id means a deliberate stock icon (camera-photo,
# say) still installs correctly.
ICON_NAME="$(sed -n 's/^Icon=//p' "$ENTRY" | head -1)"
EXEC_NAME="$(sed -n 's/^Exec=//p' "$ENTRY" | head -1 | awk '{print $1}')"

APPS_DIR="$PREFIX/share/applications"
ICONS_DIR="$PREFIX/share/icons/hicolor/scalable/apps"
BIN_DIR="$PREFIX/bin"

if [ "$UNINSTALL" = "1" ]; then
  rm -f "$APPS_DIR/$APP_ID.desktop" "$ICONS_DIR/$ICON_NAME.svg" "$BIN_DIR/$EXEC_NAME"
  echo "removed $APP_ID"
else
  mkdir -p "$APPS_DIR" "$ICONS_DIR" "$BIN_DIR"
  install -m644 "$ENTRY" "$APPS_DIR/$APP_ID.desktop"
  SRC_ICON="$PKG/icons/hicolor/scalable/apps/$ICON_NAME.svg"
  if [ -f "$SRC_ICON" ]; then
    install -m644 "$SRC_ICON" "$ICONS_DIR/$ICON_NAME.svg"
  else
    # A stock name with no dots is meant to come from the theme; a reverse-DNS
    # name that is missing is a packaging bug worth saying out loud.
    case "$ICON_NAME" in
      *.*) echo "WARNING: $ENTRY names icon '$ICON_NAME' and no such file ships" >&2 ;;
      *)   echo "    icon '$ICON_NAME' expected from the theme" ;;
    esac
  fi
  echo "installed $APP_ID -> $APPS_DIR"
fi

# Both launchers and GTK read caches rather than the directories, so a new entry
# or icon is invisible until these run. Failures are ignored: neither tool exists
# on every system and neither is required for the files to be correct.
update-desktop-database "$APPS_DIR" >/dev/null 2>&1 || true
gtk-update-icon-cache -t "$PREFIX/share/icons/hicolor" >/dev/null 2>&1 || true
