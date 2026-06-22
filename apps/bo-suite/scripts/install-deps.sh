#!/usr/bin/env bash
# install-deps.sh — install the build + runtime dependencies for the
# Bang & Olufsen controller + bo-mux soundscape suite.
#
# Detects your distro and runs the right package-manager invocation.
# Always asks for sudo; never runs sudo on your behalf.
#
# Now also handles Alpine Linux (apk) and openSUSE (zypper).
#
# The "guarded check and rerun" pattern: every package install is
# wrapped in a check → install → verify cycle. If a package isn't in
# the default repo, the script auto-enables fallback repos (Alpine
# community, Fedora RPMFusion, Ubuntu universe) and re-tries.
# Final missing-deps are reported with a clear summary so the user
# can decide whether to build from source or live without the feature.
#
# Usage:
#   ./scripts/install-deps.sh                # interactive
#   ./scripts/install-deps.sh --yes          # skip confirmation
#   ./scripts/install-deps.sh --dry-run      # print plan, do nothing
#   ./scripts/install-deps.sh --verify-only  # just check, never install

set -euo pipefail

YES=0
DRY_RUN=0
VERIFY_ONLY=0
for arg in "$@"; do
    case "$arg" in
        --yes|-y)         YES=1 ;;
        --dry-run|-n)     DRY_RUN=1 ;;
        --verify-only|-v) VERIFY_ONLY=1 ;;
        --help|-h)
            echo "Usage: $0 [--yes|--dry-run|--verify-only]"
            exit 0
            ;;
    esac
done

# ── Distro detection ────────────────────────────────────────────────
detect_distro() {
    if [[ -f /etc/os-release ]]; then
        . /etc/os-release
        local id="${ID,,}"
        local id_like="${ID_LIKE,,}"
        case "$id" in
            alpine|postmarketos)             echo alpine; return ;;
            arch|manjaro|endeavouros)        echo arch;   return ;;
            fedora|rhel|centos|almalinux|rocky) echo fedora; return ;;
            opensuse*|suse*)                 echo suse;   return ;;
            debian|ubuntu|linuxmint|pop|elementary|kde-neon) echo debian; return ;;
        esac
        case "$id_like" in
            *arch*)                          echo arch;   return ;;
            *fedora*|*rhel*)                 echo fedora; return ;;
            *debian*|*ubuntu*)               echo debian; return ;;
            *suse*)                          echo suse;   return ;;
        esac
    fi
    # Fall back to package-manager probing.
    if   command -v apk     &>/dev/null; then echo alpine
    elif command -v pacman  &>/dev/null; then echo arch
    elif command -v dnf     &>/dev/null; then echo fedora
    elif command -v zypper  &>/dev/null; then echo suse
    elif command -v apt     &>/dev/null; then echo debian
    else echo unknown
    fi
}

DISTRO=$(detect_distro)
echo ">>> Detected distro: $DISTRO"

# ── Guarded check + install + verify ────────────────────────────────
#
# Tracks state across invocations: missing[] lists tools we still
# need, installed[] lists ones we just put in. The user sees the
# audit trail at the end.

declare -a MISSING_TOOLS=()
declare -a INSTALLED_TOOLS=()
declare -a FAILED_TOOLS=()

# Check if a binary is on the PATH. Returns 0 if found.
have_bin() {
    command -v "$1" &>/dev/null
}

# Run an install command with the appropriate sudo + non-interactive
# flags for this distro. Echo what's about to happen; respect DRY_RUN.
run_install() {
    local -a cmd=("$@")
    echo "    → ${cmd[*]}"
    if [[ $DRY_RUN -eq 1 ]]; then
        return 0
    fi
    "${cmd[@]}"
}

# Try to install a list of packages with the distro's primary repo.
# Returns 0 if the command succeeded, 1 otherwise.
try_install_primary() {
    local -a pkgs=("$@")
    case "$DISTRO" in
        alpine) run_install sudo apk add --no-cache "${pkgs[@]}" ;;
        arch)   run_install sudo pacman -S --noconfirm --needed "${pkgs[@]}" ;;
        fedora) run_install sudo dnf install -y "${pkgs[@]}" ;;
        suse)   run_install sudo zypper install -y "${pkgs[@]}" ;;
        debian) run_install sudo apt install -y "${pkgs[@]}" ;;
        *)      return 1 ;;
    esac
}

# Fallback repo install. Knows about:
#   alpine community + edge testing
#   fedora rpmfusion
#   debian universe (via add-apt-repository)
try_install_fallback() {
    local -a pkgs=("$@")
    case "$DISTRO" in
        alpine)
            echo "    Falling back to Alpine community + edge testing repos..."
            run_install sudo apk add --no-cache \
                --repository https://dl-cdn.alpinelinux.org/alpine/edge/community \
                --repository https://dl-cdn.alpinelinux.org/alpine/edge/main \
                "${pkgs[@]}" 2>/dev/null \
            || run_install sudo apk add --no-cache \
                --repository https://dl-cdn.alpinelinux.org/alpine/edge/testing \
                "${pkgs[@]}"
            ;;
        fedora)
            echo "    Falling back to RPMFusion..."
            local fedora_ver
            fedora_ver=$(rpm -E %fedora 2>/dev/null || echo "")
            if [[ -n "$fedora_ver" ]]; then
                run_install sudo dnf install -y \
                    "https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-${fedora_ver}.noarch.rpm" \
                    || true
                run_install sudo dnf install -y --allowerasing "${pkgs[@]}"
            else
                return 1
            fi
            ;;
        debian)
            echo "    Enabling universe + retrying..."
            run_install sudo apt install -y software-properties-common
            run_install sudo add-apt-repository -y universe || true
            run_install sudo apt update
            run_install sudo apt install -y "${pkgs[@]}"
            ;;
        *)
            return 1
            ;;
    esac
}

# The guarded install: install pkg_list, then verify the binary appeared.
# If verification fails, try the fallback chain. Records outcomes for
# the summary at the end.
guarded_install() {
    local label="$1"; shift
    local verify_bin="$1"; shift
    local -a pkg_list=("$@")

    if have_bin "$verify_bin"; then
        echo "  ✓ $label ($verify_bin) already installed"
        return 0
    fi

    if [[ $VERIFY_ONLY -eq 1 ]]; then
        echo "  ✗ $label ($verify_bin) MISSING"
        MISSING_TOOLS+=("$label ($verify_bin)")
        return 1
    fi

    echo "  Installing $label..."
    if try_install_primary "${pkg_list[@]}"; then
        if have_bin "$verify_bin"; then
            echo "  ✓ $label installed"
            INSTALLED_TOOLS+=("$label")
            return 0
        else
            echo "  ⚠ $label install ran but $verify_bin still missing — trying fallback"
        fi
    else
        echo "  ⚠ Primary install failed — trying fallback"
    fi

    if try_install_fallback "${pkg_list[@]}"; then
        if have_bin "$verify_bin"; then
            echo "  ✓ $label installed (fallback)"
            INSTALLED_TOOLS+=("$label (fallback)")
            return 0
        fi
    fi

    echo "  ✗ $label could not be installed — check the log above for details"
    FAILED_TOOLS+=("$label ($verify_bin)")
    return 1
}

# ── Distro-specific package name tables ─────────────────────────────
#
# Each entry: guarded_install <label> <verify_binary> <package-name(s)>

install_alpine() {
    # Refresh first so we have current package lists.
    if [[ $DRY_RUN -eq 0 && $VERIFY_ONLY -eq 0 ]]; then
        sudo apk update 2>/dev/null || true
    fi

    guarded_install "build tools"          gcc          build-base pkgconf  || true
    guarded_install "OpenSSL dev"          openssl      openssl-dev          || true
    guarded_install "SQLite dev"           sqlite3      sqlite-dev sqlite    || true
    guarded_install "DBus dev"             dbus-launch  dbus dbus-dev        || true
    guarded_install "Qt5 declarative"      qmlscene     qt5-qtdeclarative-dev qt5-qtquickcontrols2-dev || true
    guarded_install "PulseAudio utilities" paplay       pulseaudio-utils     || true
    guarded_install "ALSA utilities"       aplay        alsa-utils           || true
    guarded_install "Bluetooth (bluez)"    bluetoothctl bluez                || true
    guarded_install "FFmpeg"               ffmpeg       ffmpeg ffmpeg-dev    || true
    guarded_install "Avahi (mDNS)"         avahi-daemon avahi avahi-dev      || true
    guarded_install "Opus codec"           libopus.so.0 opus opus-dev        || true
}

install_debian() {
    if [[ $DRY_RUN -eq 0 && $VERIFY_ONLY -eq 0 ]]; then
        sudo apt update 2>/dev/null || true
    fi

    guarded_install "build tools"          gcc          build-essential pkg-config || true
    guarded_install "OpenSSL dev"          openssl      libssl-dev                  || true
    guarded_install "SQLite dev"           sqlite3      libsqlite3-dev              || true
    guarded_install "DBus dev"             dbus-launch  libdbus-1-dev               || true
    guarded_install "Qt5 declarative"      qmlscene     qtdeclarative5-dev qml-module-qtquick2 qml-module-qtquick-controls2 qml-module-qtquick-layouts qml-module-qtgraphicaleffects || true
    guarded_install "PulseAudio utilities" paplay       pulseaudio-utils            || true
    guarded_install "ALSA utilities"       aplay        alsa-utils                  || true
    guarded_install "Bluetooth (bluez)"    bluetoothctl bluez                       || true
    guarded_install "FFmpeg"               ffmpeg       ffmpeg libavcodec-dev libavformat-dev libavutil-dev || true
    guarded_install "Avahi (mDNS)"         avahi-daemon avahi-daemon libavahi-client-dev || true
    guarded_install "Opus codec"           libopus.so.0 libopus-dev libopus0          || true
}

install_fedora() {
    guarded_install "build tools"          gcc          gcc gcc-c++ pkgconf-pkg-config || true
    guarded_install "OpenSSL dev"          openssl      openssl-devel                  || true
    guarded_install "SQLite dev"           sqlite3      sqlite-devel                   || true
    guarded_install "DBus dev"             dbus-launch  dbus-devel                     || true
    guarded_install "Qt5 declarative"      qmlscene     qt5-qtdeclarative-devel qt5-qtquickcontrols2 qt5-qtgraphicaleffects || true
    guarded_install "PulseAudio utilities" paplay       pulseaudio-utils               || true
    guarded_install "ALSA utilities"       aplay        alsa-utils                     || true
    guarded_install "Bluetooth (bluez)"    bluetoothctl bluez                          || true
    guarded_install "FFmpeg"               ffmpeg       ffmpeg ffmpeg-devel            || true
    guarded_install "Avahi (mDNS)"         avahi-daemon avahi avahi-devel              || true
    guarded_install "Opus codec"           libopus.so.0 opus opus-devel                || true
}

install_arch() {
    guarded_install "build tools"          gcc          base-devel pkgconf             || true
    guarded_install "OpenSSL dev"          openssl      openssl                        || true
    guarded_install "SQLite dev"           sqlite3      sqlite                         || true
    guarded_install "DBus dev"             dbus-launch  dbus                           || true
    guarded_install "Qt5 declarative"      qmlscene     qt5-declarative qt5-quickcontrols2 qt5-graphicaleffects || true
    guarded_install "PulseAudio utilities" paplay       libpulse                       || true
    guarded_install "ALSA utilities"       aplay        alsa-utils                     || true
    guarded_install "Bluetooth (bluez)"    bluetoothctl bluez bluez-utils              || true
    guarded_install "FFmpeg"               ffmpeg       ffmpeg                         || true
    guarded_install "Avahi (mDNS)"         avahi-daemon avahi                          || true
    guarded_install "Opus codec"           libopus.so.0 opus                           || true
}

install_suse() {
    guarded_install "build tools"          gcc          gcc gcc-c++ pkg-config         || true
    guarded_install "OpenSSL dev"          openssl      libopenssl-devel               || true
    guarded_install "SQLite dev"           sqlite3      sqlite3-devel                  || true
    guarded_install "DBus dev"             dbus-launch  dbus-1-devel                   || true
    guarded_install "Qt5 declarative"      qmlscene     libqt5-qtdeclarative-devel libQt5QuickControls2-devel || true
    guarded_install "PulseAudio utilities" paplay       pulseaudio-utils               || true
    guarded_install "ALSA utilities"       aplay        alsa-utils                     || true
    guarded_install "Bluetooth (bluez)"    bluetoothctl bluez                          || true
    guarded_install "FFmpeg"               ffmpeg       ffmpeg                         || true
    guarded_install "Avahi (mDNS)"         avahi-daemon avahi                          || true
    guarded_install "Opus codec"           libopus.so.0 libopus-devel libopus0         || true
}

# ── Confirmation ─────────────────────────────────────────────────────
if [[ $YES -eq 0 && $DRY_RUN -eq 0 && $VERIFY_ONLY -eq 0 ]]; then
    echo
    echo "About to check + install dependencies for: $DISTRO"
    echo "Each step is guarded — already-installed deps are skipped."
    echo "Failed primary installs auto-retry against fallback repos:"
    case "$DISTRO" in
        alpine) echo "  → Alpine community + edge testing" ;;
        debian) echo "  → Ubuntu universe / Debian backports" ;;
        fedora) echo "  → RPMFusion" ;;
        arch)   echo "  → (no auto-fallback; AUR via yay is a manual step)" ;;
        suse)   echo "  → (no auto-fallback; consult software.opensuse.org)" ;;
    esac
    read -r -p "Proceed? [y/N] " yn
    case "$yn" in [yY]*) ;; *) echo "aborted."; exit 1 ;; esac
fi

# ── Dispatch ────────────────────────────────────────────────────────
case "$DISTRO" in
    alpine) install_alpine ;;
    debian) install_debian ;;
    fedora) install_fedora ;;
    arch)   install_arch ;;
    suse)   install_suse ;;
    *)
        echo "ERROR: Unsupported distro ('$DISTRO'). Install equivalents of:"
        echo "  - Rust 1.78+, pkg-config, OpenSSL/SQLite/DBus dev headers"
        echo "  - Qt5 declarative + quickcontrols2 + graphicaleffects"
        echo "  - pulseaudio-utils, alsa-utils, bluez, ffmpeg, avahi"
        exit 1
        ;;
esac

# ── Rust toolchain check ────────────────────────────────────────────
if ! command -v cargo &>/dev/null; then
    echo
    echo ">>> Rust toolchain not found. Install via rustup:"
    echo "    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    echo "    source \"\$HOME/.cargo/env\""
    FAILED_TOOLS+=("Rust toolchain")
else
    VER=$(rustc --version | awk '{print $2}')
    echo
    echo ">>> Found Rust $VER"
fi

# ── Summary ─────────────────────────────────────────────────────────
echo
echo "──────────────────────────────────────────────────────────"
echo "Dependency wrangle summary"
echo "──────────────────────────────────────────────────────────"
if [[ ${#INSTALLED_TOOLS[@]} -gt 0 ]]; then
    echo "Installed:"
    for t in "${INSTALLED_TOOLS[@]}"; do echo "  ✓ $t"; done
fi
if [[ ${#MISSING_TOOLS[@]} -gt 0 ]]; then
    echo "Missing (verify-only mode):"
    for t in "${MISSING_TOOLS[@]}"; do echo "  ✗ $t"; done
fi
if [[ ${#FAILED_TOOLS[@]} -gt 0 ]]; then
    echo "Failed:"
    for t in "${FAILED_TOOLS[@]}"; do echo "  ✗ $t"; done
    echo
    echo "Some dependencies couldn't be installed automatically. The"
    echo "controller and bo-mux can still build without them, but features"
    echo "that depend on them will be unavailable at runtime."
    exit 2
fi
if [[ ${#FAILED_TOOLS[@]} -eq 0 && ${#MISSING_TOOLS[@]} -eq 0 ]]; then
    echo
    echo ">>> All dependencies satisfied. Next: ./scripts/build.sh"
fi
