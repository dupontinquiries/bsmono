# SnapClone

A distributed, serverless peer-to-peer snap messenger — no central server, no accounts, no cloud.

Built in Qt6/C++20. Inspired by the good parts of Snapchat; deliberately different where Snapchat was bad.

## What it is

- **Serverless P2P** — peers discover each other via UDP gossip multicast on LAN, or Jami/OpenDHT for internet routing (optional build flag)
- **First-class save** — recipients can save any received image to `~/Pictures/SnapClone/` with a timestamped filename. No surveillance model; saving doesn't notify the sender
- **Auto-close viewer** — snaps close after `durationSecs` but the user can save or close early at any time
- **Warm, muted UI** — designed for calm daily use, not engagement maximisation

## Quick start

```bash
cmake -B build -DCMAKE_BUILD_TYPE=RelWithDebInfo
cmake --build build --parallel
./build/src/SnapClone
```

The app auto-logs in as `$USER@<LAN-IP>`. No account creation needed.

## Build options

| Flag | Default | Description |
|---|---|---|
| `SNAPCLONE_JAMI_BACKEND=ON` | OFF | Enable Jami/OpenDHT internet-scale discovery. Requires `libjami-dev`. |
| `SNAPCLONE_RUST_CRYPTO=ON` | OFF | Enable Rust AES-256-GCM + X25519 crypto. Requires `cargo`. |
| `SNAPCLONE_ENABLE_SANITIZERS=ON` | OFF | ASAN + UBSAN (Debug builds only). |
| `SNAPCLONE_BUILD_TESTS=ON` | ON | Build unit tests. |

## Architecture

```
UI layer          MainWindow · SwipeNavigator · InboxWidget · SnapViewerDialog
                  CameraWidget · FriendListWidget · SettingsPage
Core layer        AppController · SessionManager · SnapRouter
Network layer     NetworkManager · GossipDiscovery · WebSocketTransport
                  FirewallPortManager · NetworkDiagnostics · JamiAdapter (stub)
Storage layer     StorageManager (SQLite WAL)
Crypto layer      CryptoService (Qt placeholder) | RustCryptoService (Rust FFI)
```

Signal flow is documented in [`docs/SignalRegistry.md`](docs/SignalRegistry.md).

## Documentation

- **[`docs/VISION.md`](docs/VISION.md)** — what we're building and why.
- **[`docs/ROADMAP.md`](docs/ROADMAP.md)** — strategic priorities, stage by stage.
- **[`docs/TODO.md`](docs/TODO.md)** — tactical work items.
- **[`docs/DEVELOPING.md`](docs/DEVELOPING.md)** — build, debug, and architecture invariants.
- **[`docs/WORK_LOG.md`](docs/WORK_LOG.md)** — session-by-session summary of significant fixes.
- **[`ARCHITECTURE.md`](ARCHITECTURE.md)** — Architecture Decision Records.

See **[`docs/README.md`](docs/README.md)** for the full index.

## Saved images

Saved snaps land in `~/Pictures/SnapClone/` with the naming convention:

```
2026-04-03_14-22-01_from_alice@192.168.2.10_ab12cd34.jpg
```

Format: `<timestamp>_from_<senderID>_<snapID-8chars>.jpg`

## Network setup

Open two ports in your firewall:

```bash
# firewalld
sudo firewall-cmd --add-port=9876/tcp --permanent   # WebSocket snaps
sudo firewall-cmd --add-port=9877/udp --permanent   # Gossip discovery
sudo firewall-cmd --add-protocol=igmp --permanent   # Multicast
sudo firewall-cmd --reload

# UFW
sudo ufw allow 9876/tcp
sudo ufw allow 9877/udp
```

Or use the in-app **Network Diagnostics** (🔍 button) which auto-detects firewalld/UFW and shows copy-paste fix commands.

## Flatpak

See [`FLATPAK.md`](FLATPAK.md) for sandboxed distribution instructions.

## Production roadmap

These are the known gaps between current state and a production-ready deployment:

### Crypto
- [ ] Replace Qt HMAC-SHA256 placeholder with Rust AES-256-GCM (`-DSNAPCLONE_RUST_CRYPTO=ON`)
- [ ] Replace placeholder ECDH with proper X25519 key exchange (in Rust crate)
- [ ] Implement Double Ratchet for forward secrecy
- [ ] Store identity keys in OS keychain (libsecret on Linux, DPAPI on Windows, Keychain on macOS)

### Network / discovery
- [ ] STUN/TURN NAT traversal for internet peers without Jami (implement `dhtnet::IceTransport` directly or via miniupnpc)
- [ ] Implement identity challenge-response over WebSocket handshake (replace the `inbound:ip:port` temporary peer ID)
- [ ] Replace gossip SWIM-lite with a proper SWIM failure detector for large peer sets

### Viewer / media
- [ ] Video snap support (currently only images)
- [ ] Swipe-down-to-save gesture in SnapViewerDialog
- [ ] Thumbnail generation for inbox rows (low priority)

### Reliability
- [ ] Offline queue: buffer outbound snaps when the peer is unreachable, deliver on reconnect
- [ ] Message ordering and deduplication for multi-hop gossip relay

### Platform
- [ ] iOS / Android port via Qt for Mobile (SwipeNavigator and touch handling already ready)
- [ ] Flatpak submission to Flathub
