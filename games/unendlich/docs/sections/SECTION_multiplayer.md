# SECTION — Multiplayer & Netcode

## 1. What this section is
Everything that lets more than one player share a world: **local splitscreen**, **deterministic
lockstep co-op** (input-sharing + rollback over the fixed-point sim), and the **world-join /
client-streaming** path (discover a host, punch/relay to it, stream the world in, replicate
entities). Player-facing goal: from the main menu, pick **Singleplayer / Host / Join** (or add a
second local player) and end up in the same `run_world` loop, with a host-authoritative world that
joins fast and stays smooth on a phone-tier uplink.

Two networked models coexist on purpose: **lockstep/rollback** (`sim::net`) for small, fully
deterministic peer sets, and **host-authoritative streaming+replication** (the world-join port) for
open join/large worlds. Local splitscreen is orthogonal to both.

## 2. Already built (real APIs in tree)
- `sim/session_mode.rs` — host/join **decision + flow layer**. `enum SessionRole { Local, Host,
  Client }`; `WorldAdvert{ host_username, candidates, … }` (`is_full`, `key`); `ServerBrowser`
  (`observe`/`prune(now_ms,ttl_ms)`/`by_recent`/`select`/`selected`); `plan_connect(advert,
  self_public, is_cgnat, has_relay) -> ConnectPlan`; `JoinFlow`/`HostFlow` state machines
  (`JoinPhase`, `HostPhase`, `connect`/`on_connected`/`on_failed`, `open`/`go_live`/`close`,
  `make_advert`). Pure.
- `sim/net_candidates.rs` — LAN-first candidate ordering. `order_targets(cands, self_public)` (drops
  the public-hairpin, de-dups), `is_dead_end(..)`, `transport_order(has_target, is_cgnat, has_relay)
  -> Vec<Transport>` (`{UdpPunch,DirectTcp,Relay,Mailbox}`). Pure. (dispatch-r58)
- `sim/net_rendezvous.rs` — the rendezvous codec + hole-punch FSM. `MailboxBlob{…}` (`encode`/
  `decode`, `BLOB_VERSION=1`), `candidate_priority(kind, local_pref)`, `PunchConfig`, `HolePunch`
  (`new`/`new_seeded(…, txid_seed)`, `on_remote_blob`, `on_packet(pair,kind,txid,now_ms)`,
  `poll(now_ms) -> Vec<Action>`, `PunchState`). Pure (time/txid passed in; seeded).
- `sim/net/` — **deterministic lockstep + rollback** netcode. `session.rs` `Session::{record_local,
  outgoing_input_packet(ack), outgoing_sync_packet(rb), note_confirmed, ingest<W:WorldQuery>}`,
  `Ingested`; `wire.rs` `Packet`/`encode_input_packet`/`encode_sync_packet`/`decode`; `transport.rs`
  `UdpTransport`/`LoopTransport`/`loopback_pair`/`drain`. Note `transport.rs` does real UDP IO
  (`io::Result`) — it is the **bin/IO side** living in the crate behind a gate, not a pure module.
- `sim/player_views.rs` — **local splitscreen** (orthogonal to networking). `ViewRegistry::{add_player
  -> ViewId, remove_player, views, viewport_of(id), set_aspect, step(dt)}` (animated split,
  `DEFAULT_ANIM_SECS`), `compute_overlap(view_planes, candidates) -> VisibilityOverlap` for
  per-view frustum culling. Pure.
- Decision docs: `handoff/MULTIPLAYER_WORLD_JOIN.md` (snapclone→unendlich port, DHT lifecycle,
  pure/IO split §7, channel QoS §6), `handoff/UNENDLICH_FULLGAME.md` (`run_world(SessionRole)` seam),
  `docs/COMPRESSION_DECISION.md` (chunk wire = seed-or-lzma, **never octree**).

## 3. The big sub-sections to build

### A. Pure world-join ports (§7 of MULTIPLAYER_WORLD_JOIN.md)
Five decision/codec modules that finish the join layer, all pure (`sim::`):
- **`net_transport_select`** — CGNAT/relay/punch decision with a passed-in reachability hint
  (trim snapclone's `ReachabilityDb` to a small table). Seams: feeds `session_mode::plan_connect`.
- **`world_presence`** — TTL-pruned online-world view (the gossip/contact model), time as a tick
  value. Seams: backs `ServerBrowser`.
- **`world_join_requests`** — host admit/deny mailbox: `Pending→Accept/Reject/Block`, unblock,
  `pending()`/badge. Seams: `HostFlow`.
- **`world_advert` codec** — sign/encode/decode the advert + candidate set; grounds `MailboxBlob`
  in the real `WorldAdvert` fields. Seams: `net_rendezvous`, `ServerBrowser`.
- **`net_channels`** — the §6 QoS table as pure data + a budgeting helper: per channel
  `{ kind: ReliableOrdered|UnreliableSequenced|Unreliable, priority, max_bytes_per_tick,
  keepalive_ms }`. Seams: consumed by the bin transport.

### B. Chunk-stream / snapshot wire codec (pure)
Per `docs/COMPRESSION_DECISION.md`: per chunk, **pristine → the seed** (coord+world seed, ~16 B,
client regenerates bit-exactly via the deterministic `WorldGenerator`); **modified → lzma'd dense
voxels or an lzma'd delta** vs the regenerated base (~0.5–0.9 KB). Octree never crosses the wire;
the client rebuilds `OctreeArray` locally. Pure module = the **record/selection logic** (decide seed vs
delta, build/parse the chunk-stream record); the lzma codec call + socket are bin glue. Derives:
COMPRESSION_DECISION + JOIN §"Bulk/world sync" (reliable, throttled below snapshots).

### C. Entity replication + authority (pure core + bin transport)
Host-authoritative entity snapshots, **unreliable-sequenced, newest-wins** (JOIN §6): a pure
snapshot encode/decode + per-entity sequence/staleness gate (dragon/player/mob transforms), plus
the authority rule (host owns truth; client predicts + reconciles — reuse the prediction/reconcile
pattern from the dispatch-r49 net-reconcile core). Seams: `sim::entity*`, the snapshot channel.

### D. `run_world(SessionRole)` seam + menu wiring (render/bin, compile-on-end)
Enrich `SessionRole` toward the FULLGAME shape (`Host{advert}`, `Client{target: ServerEndpoint}`),
add `game_flow::AppCmd::{HostWorld(WorldSpec), JoinWorld(WorldAdvert)}` + `AppEvent::
{EnterHosting,EnterJoining}`, the "Multiplayer" menu screen (Host/Join/Back) + server-browser layout,
and the host/client branches into `app::world_session::run_world` (r63-07 seam). Bin/render.

### E. Transport / DHT IO glue (bin, compile-on-end)
OpenDHT bootstrap/put/get (or `microdht` for LAN/offline), STUN, UDP punch send/recv, relay client,
Ed25519 sign/verify, wall clock + re-announce loop, QUIC/ICE channels honoring `net_channels`
priority/budget. DHT **off in singleplayer, on only when hosting** (JOIN §3). Consumes every pure
module above; performs what they decide.

## 4. Prioritized roadmap (each ≈ one worker thread)
1. **`world_advert` codec** (pure) — unblocks rendezvous + browser interop; grounds `MailboxBlob`.
2. **`net_transport_select`** (pure) — completes `plan_connect`; pairs with the built `net_candidates`.
3. **`world_presence`** (pure) — real TTL gossip view behind `ServerBrowser`.
4. **`world_join_requests`** (pure) — host admit/deny; unblocks `HostFlow` go-live.
5. **`net_channels`** (pure) — QoS table + budgeter; needed before any real transport.
6. **chunk-stream codec** (pure, §B) — seed-vs-delta record build/parse; unblocks client streaming.
7. **entity snapshot codec + authority gate** (pure, §C) — unreliable-sequenced newest-wins; reuse
   r49 reconcile. Depends on (5).
8. **`run_world(SessionRole)` seam + MP menu** (render/bin, compile-on-end) — wires 1–7 into the
   loop. Depends on the pure layer landing.
9. **DHT/UDP/relay IO glue** (bin, compile-on-end) — the big subsystem; consumes 1–7. Depends on (8).

Items 1–7 are pure `sim::` modules (new file + one `pub mod` line, no IO/clock/unseeded-RNG/unwrap,
≥6 tests) and can land in parallel; 8–9 are compile-on-end render/bin work and must come after the
pure layer they consume.

## 5. Open questions / decisions
- **`SessionRole` shape:** keep merged `{Local,Host,Client}` or enrich to FULLGAME's
  `Host{advert}` / `Client{target: ServerEndpoint}`? (Affects item 8's signature.)
- **Two MP models:** is host-authoritative streaming (world-join) the only networked path, or does
  the deterministic-lockstep `sim::net` (rollback) remain a supported co-op mode? If both, document
  when each is used (small deterministic peer set vs open join).
- **DHT backend:** OpenDHT (snapclone parity) vs `microdht` for LAN/offline — pick the shippable
  default and the test backend.
- **Identity:** username keypair storage + first-run generation is bin glue; confirm the validated
  `Username` handle rules (JOIN §2a) before `world_advert` signing lands.
- **Snapshot rate / interest management:** fixed tick rate vs distance-based entity interest (cull
  far entities from a client's snapshot) — affects item 7's scope.
