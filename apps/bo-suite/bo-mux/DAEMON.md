# bo-mux daemon (`bo-muxd`) + GUI integration

The audio/mesh engine runs as a **separate headless daemon** so it can be
systemd-managed, survive GUI restarts, and keep playing when no window is
open. The GUI is a **thin client** that talks to it over a local socket —
and can **install and auto-update** the daemon.

```
┌────────────┐   Unix socket (NDJSON)   ┌─────────────────────────┐
│  Qt GUI    │ ───────────────────────► │  bo-muxd (daemon)       │
│ (client.rs)│ ◄─────────────────────── │  owns: player, mesh,    │
│            │   responses + events     │  scene library, audio   │
│  deploy.rs │ ── installs/updates ───► │  out                    │
└────────────┘                          └─────────────────────────┘
        │                                         ▲
        └─── systemctl --user / unit files ───────┘
```

## Why a daemon + socket (not in-process)

- **Survives the GUI.** Soundscapes keep playing with no window open; the
  phone/laptop can close the app and the room stays alive.
- **systemd-managed.** Socket-activated, auto-restart on crash, starts on
  login. Standard, observable, `journalctl`-friendly.
- **Clean separation.** The GUI can be rewritten/restarted/updated without
  touching playback. Multiple frontends (GUI, CLI, web) can drive one
  daemon.
- **No network surface.** Unix socket in `$XDG_RUNTIME_DIR`, user-private
  (0600). The daemon only reaches the LAN to drive speakers.

## The API (api.rs)

Line-delimited JSON over the socket. One object per line.

Request:  `{"id":7,"cmd":"play","seed":42}`
Response: `{"id":7,"ok":true,"data":{"playing":true,"seed":42}}`
Event:    `{"event":"scene_state_changed","state":"dawn","scene_ms":1000}`

Commands: `ping, status, list_scenes, load_scene, play, stop,
start_session, leave_session, list_peers, save_scene, shutdown`.

`API_VERSION` is sent in `ping`/`status` so the GUI can detect a stale
daemon and trigger an update.

## Socket activation (bo-muxd)

The daemon acquires its listener in priority order:
1. **systemd-passed fd** — when started by `bo-mux.socket`, it inherits the
   already-bound listener (LISTEN_FDS protocol, fd 3). Fast startup,
   restart without losing queued connections.
2. **standalone bind** — when run from a terminal, it binds
   `$XDG_RUNTIME_DIR/bo-mux/control.sock` itself.

Same binary, both modes, no flags.

## Install & auto-update (deploy.rs)

The GUI bundles a `bo-muxd` binary + the two unit files and calls
`deploy::install_or_upgrade(...)`. It is **user-level, permissionless**:

  - binary  → `~/.local/bin/bo-muxd`
  - units   → `~/.config/systemd/user/{bo-mux.socket,bo-mux.service}`
  - data    → `~/.local/share/bo-mux/` (scenes/, version stamp)

`plan_action(installed, shipping)` decides FreshInstall / Upgrade /
AlreadyCurrent from a version stamp (verified by tests). On upgrade it stops
the running units, atomically replaces files (temp + rename), writes the new
stamp, `daemon-reload`s, and re-enables the socket. So the GUI shipping a
newer version transparently upgrades the daemon on next launch.

Manual equivalent: `scripts/install-daemon.sh /path/to/bo-muxd`.

## GUI client (client.rs)

`Client::default_socket()` connects to the standard path; `is_up()` pings;
`send(Command)` does request/response. A second long-lived connection can
read the event stream for live UI (scene state, peer changes). This is what
the controller app's "Spaces" page will use.

## What's verified vs. sketched

- **Verified (pure logic, tested):** API wire format, deploy `plan_action`
  + path layout, client id bookkeeping, and the whole engine core
  (soundscape director, player, delivery, rollback, election, sync, upmix).
- **Real but not compile-verified here (no toolchain):** the daemon's
  socket loop and dispatch, socket-activation fd handling, the systemctl
  calls. Structure is standard and straightforward.
- **Still sketched:** the audio render path (mux.rs) and mesh socket I/O
  (mesh.rs) the daemon wires together — these need real audio/network crates
  and devices to validate.

## Next steps to go live

1. `cargo build --release --bin bo-muxd` on the target, drop it where the
   GUI's deployer expects it (or point the GUI at the built path).
2. Implement the audio render thread (mux.rs) so `play` produces sound.
3. Wire mesh.rs to the app's mdns-sd daemon for real peer discovery.
4. Build the GUI "Spaces" page on top of client.rs.
