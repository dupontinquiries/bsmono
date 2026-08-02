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
ENTRY=""
[ -d "$PKG" ] && ENTRY="$(find "$PKG" -maxdepth 1 -name '*.desktop' | head -1)"

# A DAEMON has no .desktop entry and should not have one — it is not something
# to launch from a menu. It still ships units and man pages, so an app without
# packaging/ is installed rather than refused; refusing here is what would send
# someone to write a pointless desktop entry for a socket-activated service.
if [ -z "$ENTRY" ]; then
    if [ ! -d "$APP_DIR/systemd" ] && [ ! -d "$APP_DIR/man" ]; then
        echo "ERROR: $APP_DIR has no packaging/, systemd/ or man/ — nothing to install" >&2
        exit 1
    fi
    APP_ID="$(basename "$APP_DIR")"
    ICON_NAME=""
    EXEC_NAME="$APP_ID"
    echo "==> $APP_ID (daemon: units and man pages only)"
else
    APP_ID="$(basename "$ENTRY" .desktop)"
fi

# The Icon= line is the contract between entry and artwork. Reading it rather
# than assuming it equals the app id means a deliberate stock icon (camera-photo,
# say) still installs correctly.
if [ -n "$ENTRY" ]; then
    ICON_NAME="$(sed -n 's/^Icon=//p' "$ENTRY" | head -1)"
    EXEC_NAME="$(sed -n 's/^Exec=//p' "$ENTRY" | head -1 | awk '{print $1}')"
fi

APPS_DIR="$PREFIX/share/applications"
ICONS_DIR="$PREFIX/share/icons/hicolor/scalable/apps"
BIN_DIR="$PREFIX/bin"

if [ "$UNINSTALL" = "1" ]; then
  rm -f "$APPS_DIR/$APP_ID.desktop" "$ICONS_DIR/$ICON_NAME.svg" \
        "$BIN_DIR/$EXEC_NAME" "$PREFIX/share/metainfo/$APP_ID.metainfo.xml"
  [ -d "$APP_DIR/man" ] && for m in "$APP_DIR"/man/*.1; do
    [ -f "$m" ] && rm -f "$PREFIX/share/man/man1/$(basename "$m")"
  done
  echo "removed $APP_ID"
else
  mkdir -p "$BIN_DIR"
  if [ -n "$ENTRY" ]; then
  mkdir -p "$APPS_DIR" "$ICONS_DIR"
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
  fi
  # AppStream metadata: without it the app installs and launches but never
  # appears in GNOME Software or KDE Discover, which read metainfo rather than
  # .desktop entries.
  META="$PKG/$APP_ID.metainfo.xml"
  if [ -n "$ENTRY" ] && [ -f "$META" ]; then
    mkdir -p "$PREFIX/share/metainfo"
    install -m644 "$META" "$PREFIX/share/metainfo/$APP_ID.metainfo.xml"
  fi
  # Man pages. The service units carry Documentation=man:<name>(1), and a
  # Documentation link that resolves to nothing is worse than none: systemctl
  # status offers it and `man` then reports no entry.
  if [ -d "$APP_DIR/man" ]; then
    MAN_DIR="$PREFIX/share/man/man1"
    mkdir -p "$MAN_DIR"
    for m in "$APP_DIR"/man/*.1; do
      [ -f "$m" ] && install -m644 "$m" "$MAN_DIR/$(basename "$m")"
    done
  fi

  # systemd user units, if the app ships any. @BIN_DIR@ is substituted here
  # rather than in the source, so the same unit file works at any prefix — a
  # hardcoded /usr/bin is why units so often need editing after a --user install.
  if [ -d "$APP_DIR/systemd" ]; then
    UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    mkdir -p "$UNIT_DIR"
    for u in "$APP_DIR"/systemd/*.service "$APP_DIR"/systemd/*.socket; do
      [ -f "$u" ] || continue
      sed "s|@BIN_DIR@|$BIN_DIR|g" "$u" > "$UNIT_DIR/$(basename "$u")"
    done
    systemctl --user daemon-reload >/dev/null 2>&1 || true
    echo "    units in $UNIT_DIR — enable the .socket, not the .service"
  fi
  if [ -n "$ENTRY" ]; then echo "installed $APP_ID -> $APPS_DIR"; else echo "installed $APP_ID (daemon)"; fi
fi

# Both launchers and GTK read caches rather than the directories, so a new entry
# or icon is invisible until these run. Failures are ignored: neither tool exists
# on every system and neither is required for the files to be correct.
update-desktop-database "$APPS_DIR" >/dev/null 2>&1 || true
gtk-update-icon-cache -t "$PREFIX/share/icons/hicolor" >/dev/null 2>&1 || true
