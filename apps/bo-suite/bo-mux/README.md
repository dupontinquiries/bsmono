# bo-mux

Distributed multi-speaker channel mux & coordinator: turn a roomful of
heterogeneous devices (Mozart B&O speakers, pre-Mozart/ASE B&O speakers, a
laptop, a phone — anything running the app) into a coordinated multichannel
playback system.

**Read `DESIGN.md` first.** It explains the architecture and is honest
about what's achievable — especially the sync limits that separate
"ambient magic" from "tight movie surround."

## TL;DR of the honest assessment

- **Coordination layer (election, channel claiming, assignment): solid.**
  Built and unit-tested here.
- **Ambient / multi-stem playback (your water + birds example): the sweet
  spot.** Coarse sync is inaudible for non-transient layers — this will
  feel great.
- **Multiroom music: good** (Sonos/Chromecast-group tier).
- **Tight movie 5.1 phantom imaging: best-effort, not THX.** We push
  channels to speakers over their REST/DLNA APIs but don't own their
  clocks/buffers, so we can't promise <10ms front/rear lock. Degrades
  gracefully; the UI should set expectations.
- **2.0 → virtual surround upmix: built and tested**, independent of sync.

## What's in this drop

| module | kind | status |
|--------|------|--------|
| `protocol.rs` | pure logic | **verified** — election + assignment, unit tests |
| `sync.rs`     | pure math  | **verified** — offset estimation + scheduling, unit tests |
| `upmix.rs`    | pure DSP   | **verified** — 2.0→5.1 matrix + Haas/decorrelation, unit tests |
| `mesh.rs`     | I/O        | structured sketch — mDNS + UDP-multicast transport |
| `mux.rs`      | I/O        | sketch — per-channel HTTP splitter, TODOs mark missing crates |

The three pure modules' logic was **independently verified** by porting the
exact algorithms to Python and running the same test cases (the build env
has no Rust toolchain). The election state machine, the lowest-cost/tie-break
assignment resolver, the NTP-style offset math, the scheduled-start
arithmetic, and the upmix matrix (including the key property that mono
content yields no rears and antiphase content cancels to center) all pass.

One real bug was caught this way: a hand-computed test expectation for the
"best offset picks lowest RTT" case was wrong (offset 95 vs the asserted
105); the *code* was right and the test value was corrected.

## v1 focus: soundscapes (per your direction)

The first version is built around **ambient multi-stem soundscapes** — the
achievable sweet spot where loose sync is inaudible — with room to add
tight synchronous playback later.

A **soundscape** (`soundscape.rs`) is a set of independent **layers**
(water, birds, wind, a pad...), each drawing from a **pool** of clips under
**rules**: loop with crossfade, random gain/pan/pitch drift, probabilistic
triggering. A small **FSM director** steers the scene through **states**
(dawn → midday → dusk), re-weighting layers as it goes, with weighted-random
transitions for endless variation. See `examples/forest.soundscape.json`.

Key properties, all **verified by tests**:
- **Deterministic from (seed, scene).** The director computes an identical
  schedule on every device from the same seed — which is exactly what makes
  the **rollback sync system** work: a device that drifts or rejoins just
  recomputes from the last checkpoint, no buffered state to ship.
- **Combine tracks** = two layers pinned to different channels (e.g. vocals
  on FL, instrumental on FR — verified test).
- **Silent layers** (weight 0 in a state) produce no events; randomization
  stays within authored bounds; events are time-sorted.

**Adaptive delivery** (`delivery.rs`, verified): per channel/device it picks
WholeFile (cacheable device + loop clip → zero stream jitter), OpusSubstream
(tight uplink / many channels), or HttpsStream (dumb speaker, fat uplink).
**Endpoint output** follows your priority: line-out → Bluetooth → wireless
LAN, with fallback.

### Verified modules (logic proven in this drop)

| module | what | tests |
|--------|------|-------|
| `soundscape.rs` | FSM director + layer scheduler | 8 — determinism, bounds, combine, silence |
| `player.rs`     | runtime engine: voices, crossfade, rolling horizon, rollback | 6 — chunk==oneshot, rollback convergence |
| `transport.rs`  | clock thread: voice on/off diffing, rollback silencing | 4 — on/off, no-dup-ON, every-OFF-was-ON |
| `delivery.rs`   | delivery-mode + output priority + rollback | 12 |
| `protocol.rs`   | election + assignment | 9 |
| `sync.rs`       | offset estimation + scheduling | 6 |
| `upmix.rs`      | 2.0→5.1 DSP | 7 (Python-verified: 6 shown) |

All verified by porting the exact algorithms to Python and running the same
cases (no Rust toolchain in the build env). 42 checks pass. One real bug was
caught this way (a wrong test expectation in the sync offset case).

## How it plugs into the app



- The app's existing **mdns-sd** daemon (added for speaker discovery) is
  reused to advertise/browse peer app instances (`_bo-mux._udp`).
- The app's existing **per-device REST calls** (Mozart `PlayQueue`, ASE
  `setData` stream) become the "point speaker S at channel URL" primitive —
  this is why it works across B&O generations without their proprietary
  multiroom protocol.
- A new **"Spaces" page** (future UI work): pick a layout, watch devices
  claim channels live, hit play.

## Daemon architecture (headless + systemd + thin GUI client)

`bo-mux` is built to run as a **headless daemon** (`bo-muxd`) that owns the
engine, with the GUI as a thin client — so the audio/mesh logic keeps
running independently of whether the GUI is open, and can be systemd-managed.

**Control API** (`api.rs`): a Unix domain socket carrying line-delimited
JSON (NDJSON — one JSON object per line). Local-only (no network surface),
trivially systemd-socket-activatable, and simple enough to speak from QML or
a tiny C++ shim without a JSON-RPC library. Commands: `ping`, `status`,
`list_scenes`, `load_scene` (named or inline), `play`, `stop`,
`start_session`, `leave_session`, `list_peers`, `save_scene`, `shutdown`.
The daemon also pushes unsolicited `Event` lines (scene-state changes, peer
joins, playback start/stop) on the same socket; clients match responses by
`id` and ignore event lines.

**Multi-device mesh** (`mesh_runner.rs` + `mesh.rs`): on startup the daemon
spawns the mesh on its own thread (UDP multicast + the verified term-based
election). `start_session` / `leave_session` drive it for real — announcing a
session, gathering channel claims, resolving the assignment map — and
`status` / `list_peers` report the live role + discovered peers. If
multicast can't start (sandboxed env), the daemon logs it and runs
single-device. A stable node id is derived from hostname + uid; configured
B&O speaker URLs come from `$BO_SPEAKERS`.

**The daemon** (`bin/bo-muxd.rs`): accepts the socket from systemd via
`sd_listen_fds` (fd 3) when socket-activated, or binds it itself when run
standalone. On `play` it spawns a **transport thread** (`transport.rs`) that
owns the verified `Player`, advances a real-time scene clock (100ms tick),
keeps the rolling horizon planned, and emits voice ON/OFF/gain events to a
pluggable `AudioSink`. The default `LogSink` records/prints what plays — so
the daemon genuinely *runs* a soundscape, observably, even before a real
PipeWire/HTTP audio backend is wired (that backend just implements
`AudioSink`). `stop` signals the thread to silence everything and exit.

**systemd units** (`systemd/`): a user `.socket` (ListenStream=
`%t/bo-mux/control.sock`, 0600) + `.service` (runs `%h/.local/bin/bo-muxd`,
hardened with ProtectSystem=strict / ProtectHome=read-only / scoped
ReadWritePaths). Socket-activated, so the daemon starts on first connection
and survives restarts without dropping the listener.

**GUI auto-update / deploy** (`deploy.rs`): the GUI can install or upgrade
the daemon itself. `plan_action` compares the installed version stamp
against the shipping version → FreshInstall / Upgrade / AlreadyCurrent.
`install_or_upgrade` stops running units, atomically copies the new binary +
unit files (temp + rename), writes the version stamp, runs
`systemctl --user daemon-reload`, and re-enables the socket. So the GUI can
ship a newer daemon and roll the deployed service forward on its own.

**Thin client** (`client.rs`): `Client::default_socket()`, `is_up()`,
`send(Command) -> Response`. Connection-per-request, skips interleaved event
lines. This is what the Qt GUI links against (or shells out to) to drive the
daemon.

**Verified logic** (Python port): API request/response serde shapes + NDJSON
framing, and the deploy `plan_action` version logic — all pass.

## To finish it (next steps, in order)

1. Wire `mesh.rs` to the app's mdns-sd daemon + a real UDP multicast socket;
   exercise the election with 2–3 instances on the LAN.
2. Implement `mux.rs` serving: `symphonia` decode → deinterleave → `tiny_http`
   WAV/L16 streaming per channel. Point one B&O speaker at `/ch/FL` to prove
   the pull path end-to-end.
3. Calibrate playout latency per device type (a chirp + mic, or a manual
   slider) and feed `sync.rs::EndpointTiming`.
4. Build the "Spaces" UI page.

Each step is independently testable; the verified core means the
distributed-systems reasoning underneath is already sound.

## Per-channel serving (assigned channel → device)

`chan_server.rs` is the concrete piece that turns "device X drives channel C"
into C's audio actually reaching X:

- A minimal HTTP server (std::net, no framework) serves each channel of a WAV
  source as a standalone **mono WAV** at `GET /ch/<name>` (plus `/health`).
- `fetch_channel_to_temp(url)` is the client side: an assigned device pulls
  its channel URL to a temp WAV and hands it to its `AudioSink`.
- Channel names map to source indices (standard 5.1 order FL FR C LFE SL SR,
  with case-insensitive + alias lookup).

**Verified end-to-end here**: WAV parsing + per-channel deinterleave proven
against a generated 6-channel file (each channel extracts exactly its own
samples; SL → index 4), and a **live TCP round-trip** — an HTTP server served
`/ch/SL`, a client fetched it over a real socket, and the bytes were a valid
mono WAV with the correct samples.

Honest scope: v1 serves WAV (no decode dependency) — perfect for the
soundscape clips and what B&O/ffplay pull happily. Arbitrary formats
(flac/mp3/aac) need a decode front-end (symphonia); the splitting + serving
plumbing is format-independent, so only that front-end changes. Tight
cross-device *timing* of these streams is still the sync.rs scheduled-start
work that needs real hardware to tune.

## Audio output (the last inch)

`sink_system.rs` is a concrete `AudioSink` that makes real sound by shelling
out to a system player — one process per sounding voice — preferring
PipeWire-friendly `paplay`, then `ffplay`, then `aplay`. The daemon picks it
automatically on `play` when a player is present (the Play response includes
`"audible": true`), and falls back to the `LogSink` headless so it still runs
observably in CI.

Verified here: the exact `ffplay`/`paplay` commands the sink builds were
exercised against a generated WAV — they parse, decode, and apply the
gain→volume mapping correctly (`paplay --volume` 0..65536, `ffplay -af
volume=`). The only thing this container lacks is a sound card.

Honest limits vs. a native mixing sink: no live crossfade ramps (subprocess
players can't be volume-ramped mid-play, so overlapping clips just both
sound during the overlap — fine for ambient, not for tight music), and
pan/rate aren't honored by simple players. A native per-channel sink with
real upmix + per-speaker routing can replace it behind the same trait
without touching the transport or planning logic.

## GUI-launchable setup scripts

`scripts/` holds operations the GUI can launch and stream:

- **`bo-mux-install.sh --payload <dir> [--version X.Y.Z]`** — installs the
  binary + systemd units, enables the socket. Emits line-tagged progress
  (`STEP`/`OK`/`WARN`/`ERR`/`DONE`) a GUI can parse and display live.
- **`bo-mux-doctor.sh [--json]`** — health checks (binary, units, systemd
  reachable, socket present, daemon ping, music dir). Human-tagged
  (`CHECK <name> PASS/WARN/FAIL`) or `--json` for a structured checklist.
  Exits non-zero if any check FAILs.
- **`bo-mux-uninstall.sh [--keep-data]`** — stops/disables units, removes
  binary + units (and data unless `--keep-data`).

The controller GUI launches these via `MuxBridge` (`run_install`,
`run_doctor`, `run_uninstall`), streaming output into a Setup &
Diagnostics panel that renders the doctor checklist with pass/warn/fail
pips and a raw log view. This keeps install/check/uninstall observable
from the GUI rather than requiring a terminal.
