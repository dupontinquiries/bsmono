# Flatpak Build Guide

## Quick start

```bash
# 1. Install prerequisites (one-time)
sudo dnf install flatpak flatpak-builder        # Fedora/RHEL
sudo apt  install flatpak flatpak-builder       # Ubuntu/Debian

# 2. Add Flathub and install the KDE 6 runtime (one-time, ~2 GB)
flatpak remote-add --user --if-not-exists flathub \
    https://flathub.org/repo/flathub.flatpakrepo
flatpak install --user -y flathub \
    org.kde.Platform//6.7 \
    org.kde.Sdk//6.7

# 3. Build + install + run
./flatpak-build.sh --run
```

## Build script reference

```
./flatpak-build.sh            # build only
./flatpak-build.sh --install  # build + install for current user
./flatpak-build.sh --run      # build + install + launch
```

## Build with optional backends

### Jami backend (internet-scale P2P)

Jami must be available in the Flatpak SDK.  Add it as a module before the
`snapclone` module in `com.snapclone.SnapClone.json`:

```json
{
    "name": "jami-daemon",
    "buildsystem": "cmake-ninja",
    "config-opts": ["-DBUILD_CONTRIB=ON", "-DBUILD_TESTING=OFF"],
    "sources": [{"type": "git",
                 "url": "https://review.jami.net/jami-daemon",
                 "tag": "stable"}]
},
{
    "name": "snapclone",
    "buildsystem": "cmake-ninja",
    "config-opts": [
        "-DCMAKE_BUILD_TYPE=Release",
        "-DSNAPCLONE_BUILD_TESTS=OFF",
        "-DSNAPCLONE_JAMI_BACKEND=ON"
    ],
    ...
}
```

### Rust crypto backend (AES-256-GCM)

The Flatpak SDK includes `rustup`.  Add a pre-build step to the `snapclone`
module:

```json
{
    "name": "snapclone",
    "buildsystem": "cmake-ninja",
    "build-commands": [
        "cargo build --release --manifest-path rust/snapclone-crypto/Cargo.toml"
    ],
    "config-opts": [
        "-DCMAKE_BUILD_TYPE=Release",
        "-DSNAPCLONE_RUST_CRYPTO=ON"
    ],
    ...
}
```

## Sandbox permissions explained

| Permission | Reason |
|---|---|
| `--share=network` | WebSocket peer connections and gossip UDP |
| `--device=all` | Camera access for snap capture |
| `--socket=wayland` + `--socket=fallback-x11` | Display output |
| `--filesystem=xdg-data/snapclone:create` | SQLite database and key files |
| `--filesystem=xdg-cache/snapclone:create` | Cached media |
| `--system-talk-name=org.fedoraproject.FirewallD1` | Auto-open ports via firewalld |
| `--system-talk-name=com.ubuntu.USF1` | Auto-open ports via UFW |
| `--talk-name=org.freedesktop.portal.Desktop` | XDG portal firewall fallback |
| `--talk-name=org.freedesktop.Notifications` | Snap arrival notifications (future) |

## Manual firewall setup (if D-Bus auto-config fails)

```bash
# firewalld (Fedora/RHEL/openSUSE)
sudo firewall-cmd --add-port=9876/tcp --permanent
sudo firewall-cmd --add-port=9877/udp --permanent
sudo firewall-cmd --add-protocol=igmp --permanent
sudo firewall-cmd --reload

# UFW (Ubuntu/Debian)
sudo ufw allow 9876/tcp
sudo ufw allow 9877/udp
```

Run the in-app **Network Diagnostics** (🔍 button in the inbox toolbar) to
verify port reachability after making firewall changes.

## Distribute without Flathub

```bash
# Create a single-file bundle
flatpak build-bundle .flatpak-repo SnapClone.flatpak com.snapclone.SnapClone

# Recipients install with
flatpak install --user SnapClone.flatpak
flatpak run com.snapclone.SnapClone
```

## Submitting to Flathub

1. Ensure `com.snapclone.SnapClone.metainfo.xml` passes AppStream validation:
   ```bash
   appstream-util validate resources/com.snapclone.SnapClone.metainfo.xml
   ```
2. Fork [github.com/flathub/flathub](https://github.com/flathub/flathub) and
   create a branch `new-submission/com.snapclone.SnapClone`.
3. Update the `sources` entry in the manifest to point at a tagged release
   tarball (not a local path).
4. Submit a pull request — Flathub CI runs `flatpak-builder` against your
   manifest automatically.

## Logging inside Flatpak

```bash
# All SnapClone categories
QT_LOGGING_RULES="snapclone.*=true" flatpak run com.snapclone.SnapClone

# Network only
QT_LOGGING_RULES="snapclone.network=true;snapclone.gossip=true" \
    flatpak run com.snapclone.SnapClone
```
