# bo-suite

Cargo workspace containing two crates that ship together:

| Crate          | Role                                                    |
|----------------|---------------------------------------------------------|
| `bo-controller` | Qt/QML desktop controller for Bang & Olufsen networked devices (volume, playback, library, anticipation, identification ping, BT auto-connect). |
| `bo-mux`        | Headless soundscape engine + multi-speaker channel mux. Drives ambient stems, room presets, and mesh playback across a heterogeneous LAN. |

Both build with one workspace `cargo build`. Use the installer scripts to set up dependencies and lay binaries + assets into `~/.local`.

## Quick start (Debian/Ubuntu/Fedora/Arch)

```sh
./scripts/install-deps.sh      # apt/dnf/pacman, asks for sudo
./scripts/build.sh             # cargo build --workspace --release
./scripts/install.sh           # copies to ~/.local/bin + ~/.config
bo-controller                  # launch the GUI
```

To also run `bo-muxd` as a user systemd service:
```sh
./scripts/install.sh --systemd
systemctl --user enable --now bo-muxd
```

To uninstall:
```sh
./scripts/install.sh --uninstall
```

## Workspace layout

```
bo-suite/
├── Cargo.toml             # workspace root (members + shared profiles)
├── README.md              # this file
├── scripts/
│   ├── install-deps.sh    # distro-aware build/runtime deps
│   ├── build.sh           # cargo build --workspace --release
│   └── install.sh         # copy binaries + QML + desktop entry
├── bo-controller/         # Qt/QML controller
│   ├── Cargo.toml
│   ├── src/               # Rust modules
│   ├── qml/               # QML UI
│   ├── assets/            # SVG silhouettes, JSON product data
│   └── README.md
└── bo-mux/                # Soundscape daemon
    ├── Cargo.toml
    ├── src/
    ├── examples/          # forest.soundscape.json + similar
    ├── systemd/           # bo-muxd.service unit
    └── DESIGN.md
```

## Latency-sensitive subsystems

The controller layers several latency-reduction techniques on the basic
"POST a volume value to the device" loop:

- **Shadow streams** — silently pre-stream high-confidence candidate tracks at volume = 0 so the user-tap turns into a vol-restore (instant, no buffering)
- **Connection warmer** — proactive cheap GETs to keep TCP/keep-alive hot when the anticipator's confidence on device interaction is high
- **Circuit breaker** — fast-fails commands to a dead URL after 3 consecutive failures, sparing 30+ seconds of queued-command drain
- **SetVolume coalescing** — drains queued same-URL volume commands and POSTs only the latest value, collapsing dial-drag floods into a single round trip
- **TCP_NODELAY** on the track server — disables Nagle so the device receives the first audio bytes immediately rather than waiting up to 200 ms
- **Pre-buffer cue** — synthesizes a tempo-aligned low-freq cue while the device is still bringing up the audio path, perceptually masking the gap
- **Time-of-day biased priors** — the FSM weights its predictions by hour so the system pre-stages what *you* actually do at this time
- **RTT tracker** — measures observed round-trip times and auto-tunes cue duration to ~1.2× the p95
- **State epochs** — managers tag work with a freshness counter so cross-thread races (shadow restore vs user adjustment) resolve correctly

See `bo-controller/README.md` for component-level detail and `bo-mux/DESIGN.md` for the soundscape side.

## Configuration

The controller reads:
- `BO_QML_DIR` — path to QML assets (set by the desktop entry)
- `BO_MUSIC_DIR` — root directory the media scanner walks
- `TZ_OFFSET_HOURS` — local timezone offset for anticipator time-window biasing

The daemon reads:
- A soundscape JSON file passed as `--config` (see `bo-mux/examples/`)
