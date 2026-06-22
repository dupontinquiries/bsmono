#!/usr/bin/env bash
# flatpak-build.sh — Build and optionally install the SnapClone Flatpak.
#
# Usage:
#   ./flatpak-build.sh            # build only
#   ./flatpak-build.sh --install  # build + install for current user
#   ./flatpak-build.sh --run      # build + install + launch
#
# Prerequisites (install once):
#   flatpak install flathub org.kde.Platform//6.7 org.kde.Sdk//6.7

set -euo pipefail

APP_ID="com.snapclone.SnapClone"
MANIFEST="${APP_ID}.json"
BUILD_DIR=".flatpak-build"
REPO_DIR=".flatpak-repo"

# ── Sanity checks ─────────────────────────────────────────────────────────────
if ! command -v flatpak-builder &>/dev/null; then
    echo "Error: flatpak-builder not found."
    echo "Install with: sudo dnf install flatpak-builder  OR  sudo apt install flatpak-builder"
    exit 1
fi

if ! flatpak info org.kde.Sdk//6.7 &>/dev/null; then
    echo "KDE SDK 6.7 not installed. Installing now…"
    flatpak install --user -y flathub org.kde.Platform//6.7 org.kde.Sdk//6.7
fi

# ── Build ─────────────────────────────────────────────────────────────────────
echo "Building ${APP_ID}…"
flatpak-builder \
    --force-clean \
    --ccache \
    --repo="${REPO_DIR}" \
    "${BUILD_DIR}" \
    "${MANIFEST}"

echo "Build complete."

# ── Install ───────────────────────────────────────────────────────────────────
if [[ "${1:-}" == "--install" || "${1:-}" == "--run" ]]; then
    echo "Installing ${APP_ID} for current user…"
    flatpak --user remote-add --no-gpg-verify --if-not-exists \
        snapclone-local "${REPO_DIR}"
    flatpak --user install --reinstall -y snapclone-local "${APP_ID}"
    echo "Installed."
fi

# ── Run ───────────────────────────────────────────────────────────────────────
if [[ "${1:-}" == "--run" ]]; then
    echo "Launching ${APP_ID}…"
    flatpak run "${APP_ID}"
fi

echo "Done. To run: flatpak run ${APP_ID}"
