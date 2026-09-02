# Session 2026-09-01 — OpenDHT interop, serving nodes, NAT traversal, UI

Twenty-two commits on `7653dda0` (the head of `c14unified-009.bundle`),
in order. Every claim below was checked by a test named in the commit;
"verified" means the test failed before the change or was broken on
purpose afterwards. Nothing in this session ran on a phone or crossed a
real NAT: the sandbox has neither.

## Interop: the codec was not the reference's format

- `30fdfa98` dht-kit rewritten from `vendor/opendht`'s packer source. Seven
  independent incompatibilities, any one fatal: integer token (the
  reference throws), `v`/`sa` refused as unknown keys, `find` target under
  the wrong key, nodes as an array not a blob, a two-key value not
  `{id,dat:{body:{type,data}}}`, put's array under the wrong name, `s`/`w`
  never sent. Decoder skips unknown keys structurally over the whole
  msgpack format table.
- `8b468225` snapcore: `wire` default-on (a feature names a dependency, not
  a capability); `Responder` mints and checks real tokens bound to ip+key;
  `sa`, `want`, `vid`; refresh implemented. Fixed a fixture that believed
  it had two peers: `InfoHash::from_seed(2n) == from_seed(2n+1)`.
- `f1743bad` presence: `Value::new`, since a value carries its type now.
- `59f79a00` the interop test, run: `dhtnode` built in-sandbox
  (`maintenance/OPENDHT_INTEROP.md`), ping/sa/id-correction/n4/token+put
  stored and read back by the reference's `g`/its put decoded here/full
  lookup A→B. Removing the token makes it fail. Two captured datagrams
  re-encode byte-identically.
- `492c3e4e` a put to a peer the lookup never queried asks it first (the
  shortlist is the k closest, not the k queried; the reference 401s).

## Independent of bootstraps

- `24c6b914` `WireUdpTransport::serve`: one thread blocks in `recv_from`
  with no timeout, answers via `Responder` behind the rate limiter, routes
  replies by tid. Routing table and lookup shortlist both replace a
  contact at a known address under a new id (the guessed bootstrap id was
  a phantom the real node answered for). Two served transports find each
  other with nobody else.
- `389c7bfc` `NodeHost` serves; lookups on a routing-table snapshot; put
  stores locally, get reads locally. `tests/two_hosts.rs` (no third node)
  and `tests/opendht_host_interop.rs`: the reference started with our host
  as its ONLY bootstrap reports "1 good", reads our raw publish, its put
  resolves here.
- `4d4f7ec7` frontend: DHT bound on the listen port's number, bootstraps
  from every remembered peer under its real id, announces the DHT-reported
  ip when routable. `87a7c376` corrects the port to listen+1: UDP on the
  listen port is the voice socket — my regression, caught in-session.
- `aa85dfe8` keepalive: the host's blocking recv is `recv_timeout(25 s)`;
  the idle wake pings the three nearest peers and reports
  `Event::Keepalive{pinged,answered}`; the frontend says "network changed"
  on the transition to nobody answering.

## Camera, voice loop, NAT

- `fcd55ce6` `camera_enumerate` asks PipeWire (`pw-dump`, the bridge's own
  probe) before `/dev/video*`: C13_TODO §23's decoder-nodes-as-cameras
  had one cause. Bridge `virtual-cam-*` nodes rank first. FP5-shaped dump
  fixture. A window-offset bug in the bridge's own scanner noted under
  §17b, not fixed there.
- `74d111cc` the frame loop runs at the audio frame period during a call;
  at 500 ms wakeups `pump_voice` drained a fifth of what the mic produced.
- `52032584` `NatTraversal.cpp` ported as `nat_traversal.rs` (sans-io):
  STUN from the punch socket, 60×150 ms volley then 20 s maintenance,
  the peer's real source registered from the first punch; the voice
  socket outlives a call and is polled idle; the record carries the
  STUN-mapped punch port. Not run across a real NAT. §35 lists what is
  not driven (classify → relay).

## UI

- `9e1b62bc` + `614718e9` uikit-egui: the monorepo-wide swipe bug. A drag a
  widget owns (`dragged_id`, or press origin in the focused field) is not
  a page gesture; a container (rect ≥ half the surface: a ScrollArea)
  yields to the page along the page axis. Both shared bridges.
- `aec364ac` every page but the viewfinder scrolls inside its bounds (§24,
  verified by drawing bare: y=707 on a 640 screen).
- `dca609d3` `hint::hint(&Situation)`: one prioritised line; phone short
  above the tab bar, desktop long under the toolbar.
- `a2cc0b96` double-tap flips the camera; phone camera picker is a
  dropdown; feed tiles sense click only.
- `24d1904e` columns on a wide screen (kit's fit + no-wrap window); the
  trackpad detent turns pages (kit convention: positive along axis = Next;
  220 raw at 0.75/ms ≈ 300 ms). The Tab-typing test now states a
  single-column screen.
- `d4d1bd70` C13_TODO §24 item done; §36 records UI state and next steps.
- `8c2b9afc` the goodies from `wiring_check`: received snaps are held and
  open by kind (before, `SnapArrived` dropped the bytes and nothing could
  open); hold → snap menu (`menu_ui` finally drawn); swipe up sends; the
  help manual drawn with filter/scroll/idle; `close_video`,
  `ambient_snap_arrived`, `purge_stale`, `set_talk_prominent`, Escape
  dismisses. Eighteen remain with a reason each (§36 addendum).
- `2a28caca` command palette (Ctrl+K) with a schedule ("call at the hour"
  = next top of the hour; "in 10 min"; "at 14:30" under a given offset;
  unparsed time ≠ now); keyboard captions (Tab on, pointer move off,
  Ctrl+1..5 pages, chords read from key events); tile hover caption and
  hold ring.

## Counts at the end

dht-kit 32+8, snapcore 112 (+1 ignored interop), presence 24+3 (+1
ignored), uikit-egui 335, snapclone-egui 1605, binary type-checks with
`--features app`. `lock_check` 0 stale; `orphan_check` 7 pre-existing.

## Known gaps, in the order I would take them

1. Run on the phone: camera enumeration, the call loop, punching.
2. `nat_punch::classify` driven from two STUN answers → `Strategy::Relay`,
   and a relay through a reachable snapclone peer.
3. Local UTC offset into the palette; Opus for voice (a streaming codec
   path does not exist yet); a real QR encoder if wanted (~300 lines).
4. Peers page as a table on desktop / cards on phone; keyboard focus for
   tiles and the tab bar; hover affordances on the strip.
