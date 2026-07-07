# MULTIPLAYER_WORLD_JOIN — porting snapclone's mailbox + presence to unendlich

Status: design + pruned-code handoff. Source: `v1-r3` (snapclone, Rust over Jami
`dhtnet`/OpenDHT). This writeup says **what to port, what to drop, where the
pure/IO line is, how the DHT is gated to server-hosted worlds, and the channel QoS
layout.** It pairs with the pruned source subset (`snapclone-net-pruned.zip`) and
the `net_rendezvous` worker brief (dispatch-r57).

---

## 1. What we take from snapclone (and what we don't)

snapclone is a P2P messenger: stable `sc-ID` identities, an OpenDHT **mailbox**
(tiny intent records under a key derived from the recipient), STUN-discovered
reflexive addresses, UDP hole-punch with TCP/relay fallback, and a learned
reachability memory. We reuse the **rendezvous + presence + traversal** layer and
drop everything social/media (snaps, vcard, camera, audio, groups, AV1).

**Keep (maps directly to world-join):**
- DHT **value-layer semantics**: `PUT key value ttl` / `GET key -> [values]`,
  multi-value-per-key, TTL expiry, periodic re-announce. (`snapclone-microdht`
  models exactly this, dependency-free — the cleanest spec for our DHT use.)
- **Presence announce**: publish `(id, name, public_ip, punch_port)` after STUN;
  others `GET` it. (`snapclone-dht::announce`, `gossip::GossipView`.)
- **Mailbox intent record** with **delivery hints** (candidate addresses + relay).
  (`snapclone-mailbox::{entry, envelope}`.)
- **Join-request approval** state machine. (`snapclone-mailbox::requests` — this is
  the "adding friends" flow.)
- **Which IP to punch/relay**: LAN-first, drop-hairpin, dead-end→relay.
  (`snapclone-net-intel::connreq_targets` — carries a real same-WiFi bug fix.)
- **Transport selection**: CGNAT→relay/mailbox, else punch/direct, history-biased.
  (`snapclone-session::selector`, `snapclone-net-intel` reachability memory.)
- Online/offline + session models. (`snapclone-core::{contact, peer}`.)

**Drop:** snaps/media transfer, vcard, camera-bridge, audio/AV1/face/landmark,
groups/DAG, theme/menu-FFI, the Qt/egui UI, and the C++ `dhtnet`/`relay-server`
internals (we keep their *protocol*, not their code).

---

## 2. Concept mapping — snapclone → unendlich

| snapclone | unendlich world-join |
|---|---|
| `sc-ID` (opaque stable peer identity) | **unendlich username** (human-readable handle, stable per account) + **world-ID** (per saved world) |
| DHT presence announce | **world advertisement**: host publishes a joinable world to the DHT |
| `GossipView` of online peers | **server browser**: list of currently-advertised worlds (TTL-pruned) |
| `Contact { online, last_seen, state }` | **KnownWorld** (a remembered server: name, host username, online?, last_seen) |
| `Peer { status, address, we_initiated }` | **JoinSession** (a player↔host connection in progress/up) |
| Mailbox intent record (snap to fetch) | **join request** record (a player wants in; carries their candidates) |
| `MailboxEntry::DeliveryHint{DirectWs,Relay}` | host/peer **candidate set** to punch/relay to |
| `RequestMailbox` (accept/reject/block sender) | **host approves joiners** → world allowlist / banlist |
| `connreq_targets::order_targets` | **candidate ranking** before punching |
| `TransportSelector::ordered_choices` | **punch vs direct vs relay vs wait** decision |
| `ReachabilityDb` (warm/negative cache) | per-host **reachability memory** (which path worked last time) |

The pivot: a *contact you message* becomes a *world you join*; *accepting a friend
request* becomes *the host admitting a player*; *resolving an sc-ID to an address*
becomes *resolving a world-ID to the host's punchable candidates*.

### 2a. Identity: unendlich username (not an opaque sc-ID)

snapclone's identity is an opaque random `sc-ID` (`sc-32a51661`). For unendlich we
replace it with a **human-readable unendlich username** as the stable account handle
— so the server browser, the join requests, and the friends/allowlist all show names
a player recognizes (`"thornwhisper"`), not hex blobs.

- The username is the **stable identity key**: a player picks it once; it persists
  per account/install.
- DHT keys derive from it the same way snapclone derived them from the sc-ID — hash
  the human string into the routing key so the bytes stay fixed-width and collision-
  resistant: world key = `SHA256("world:" + username + ":" + world_name)`, join
  mailbox = `SHA256("join:" + host_username)`. The **plaintext username travels in
  the signed advert/record body** (for display); the **key is the hash** (for
  routing/poll), preserving snapclone's S-5 (full-256-bit key) and S-12 (Ed25519
  over the body) properties.
- Uniqueness/squatting is a registration concern, not a transport one: the Ed25519
  keypair still proves ownership of *this* identity; the username is the label bound
  to that key in the signed body. Two installs can't both *prove* the same username
  unless they share the key.
- Everywhere `identity::ScId` appeared, read **`Username`** (a validated handle:
  charset + length rules in place of the `sc-` prefix check). `world_presence`,
  `world_join_requests`, and the advert codec all key on the username.

---

## 3. DHT lifecycle — OFF in singleplayer, ON only when hosting a server

This is the core behavioural requirement. The DHT is **opt-in per session**, driven
by the main-menu world selector:

```
World selector choice            DHT          UDP listener     Announce
────────────────────────────────────────────────────────────────────────
"Play"            (singleplayer)  OFF          none             none
"Open as server"  (host)          ON           bind punch_port  advertise world (re-announce loop)
"Join a world"    (client)        ON (lookup)  bind punch_port  none (publishes a join request only)
```

- **Singleplayer = zero network.** No DHT bootstrap, no STUN, no socket, no
  announce. The networking subsystem is never constructed. (Privacy + battery +
  no surprise traffic — matches snapclone's "DHT only when an account is active.")
- **"Open as server"** flips it on: generate/load the player's username identity, STUN to learn the
  reflexive `public_ip:punch_port`, then **re-announce** the world record to the
  DHT on a TTL refresh loop (snapclone re-announces because DHT values expire). The
  world appears in others' server browsers. Closing the world stops the loop and
  lets the record expire.
- **"Join a world"** turns the DHT on only to **look up** the chosen world-ID and to
  publish the joiner's own candidate record (a mailbox entry), then hole-punches.
  A client never *advertises* a world.
- A **dedicated server** is just "Open as server" headless — same path.

State machine (host side): `Offline → Announcing → Listed → (joiners arrive) →
Serving → Closing → Offline`. The DHT handle's lifetime is the world's hosting
lifetime; dropping it tears down bootstrap + announce.

---

## 4. Presence detection (the server browser)

Port of `announce` + `GossipView` + `Contact.online`:

1. Host publishes a **WorldAdvert** value under a DHT key derived from a public
   "lobby" namespace (so browsers can enumerate) **and/or** under the world-ID (so a
   friend who knows the world-ID can resolve it directly). Snapclone keys the mailbox
   as `SHA256("mailbox:" + recipient_id)`; we key adverts as
   `SHA256("world:" + world_id)` and an optional shared lobby bucket.
2. WorldAdvert (small, signed, < ~200 B): `world_id, host_id, name, player_count,
   max_players, version, public_ip, punch_port, lan_addrs[], flags(passworded…),
   nonce`. **Signed (Ed25519)** over canonical bytes so a scraper can't forge a
   world claiming someone else's username (snapclone's S-12 fix).
3. Browser does `GET` on the lobby bucket, collects live values, and keeps a
   TTL-pruned view (`GossipView::purge_stale`). A world is "online" if its advert is
   fresh; "last seen" decays like `Contact.last_seen`.
4. **Re-announce cadence** trades discovery latency vs DHT load — see §6.

---

## 5. Which IP to punch / relay to (the heart of the request)

When a browser has a WorldAdvert (or a host has a join request), it has a **candidate
set**: the peer's LAN addresses + public reflexive address (+ optional relay). The
order to *try* them is not obvious, and getting it wrong strands connections. Port
`connreq_targets::order_targets` verbatim (it's pure and already battle-tested):

- **Drop the hairpin.** A public candidate whose host == *our own* public IP is the
  shared router's external face — punching it fails. (The real snapclone bug: two
  phones on one WiFi each had only the other's public IP; both correctly skipped the
  hairpin and were then left with nothing, so messages hung forever.)
- **LAN first.** RFC1918 candidates (same subnet) are fastest and dodge NAT entirely
  — try them before any public candidate. (The fix was to *include* the sender's LAN
  address in the request; we must put the host's LAN addrs in the WorldAdvert and the
  joiner's LAN addrs in the join request.)
- **De-dup**, then the remaining public candidates.
- **Dead-end detection**: if ordering yields nothing (only-hairpin case), don't
  report failure — fall straight to relay/mailbox.

Then `TransportSelector::ordered_choices` picks the *method* per candidate, biased by
NAT state and reachability history:

```
is_cgnat            → [DhtMailbox, Relay]                 (skip direct entirely)
no known IP         → [DhtMailbox]                        (wait for the peer)
otherwise           → hints_for(history): [UdpPunch?, DirectTcp?, Relay?, Mailbox]
fallback            → [DirectTcp, DhtMailbox]
```

`ReachabilityDb` records success/failure per `(peer, network-bin, protocol)` with
time-decay + negative caching, so "the path that worked last time on this network"
is tried first. For unendlich this becomes per-host memory: punch worked on my home
WiFi last night → try punch first there; relay was needed on cellular → start there.

**Establishing the connection** (host side, mirror on client): on a successful punch
record success (`SessionCoordinator::on_connected` → `record_success`), reset
backoff; on failure record it and back off (`RetryBackoff`); when the direct path is
hopeless, the `Peer.status` becomes `Relayed`.

---

## 6. Channel layout / config (bandwidth + latency)

A messenger needs one warm path; a game world multiplexes traffic with very
different QoS. Define explicit **channels** over the established transport, each with
its own reliability/ordering and a bandwidth/latency budget. (snapclone normalizes
everything into fixed 4 KB encrypted envelopes to defeat traffic analysis; for a game
that padding is wrong for the hot channels — keep it only for the rendezvous/control
metadata, not the game stream.)

| Channel | Reliability / order | Latency target | Bandwidth | Notes |
|---|---|---|---|---|
| **Control / join** | reliable, ordered | ~loose | tiny | handshake, join req/approve, allowlist, world meta. Fixed-size envelope OK here. |
| **Entity snapshots** | **unreliable**, sequenced (drop stale) | **tight (<50 ms)** | medium, steady | dragon/player/mob transforms; newest wins, never block on a lost packet. |
| **Player input / commands** | reliable, ordered (per-sender) | tight | small | inputs, ability casts, world edits — must not drop or reorder. |
| **Voice / proximity audio** | unreliable | **tightest** | small-medium | latency over completeness; jitter buffer client-side. |
| **Bulk / world sync** | reliable, ordered | irrelevant | **high, throttled** | initial world chunk download, asset/region streaming. This is snapclone's *deferred bulk transfer* — only on a warm path, throttled so it never starves snapshots. |
| **Chat / events** | reliable, ordered | loose | tiny | text, notifications. |

Design rules carried over:
- **Separate intent from bulk** (snapclone §6 / mailbox): a join is a tiny DHT record
  that always succeeds even under CGNAT; the heavy world-state download only starts
  once a live session exists on a warm path. A player can "request to join" while the
  host is mid-game and get streamed in when the path is good — never lose the intent.
- **Snapshots are unreliable-sequenced**, not reliable: a dropped transform is
  replaced by the next one; blocking on retransmit would spike latency.
- **Bulk is throttled below snapshots** so region streaming can't starve the hot loop
  (the reachability/“warm path” bias decides when bulk is even allowed to run).
- **Re-announce / keepalive cadence**: world advert re-announce ~every 5 min (DHT TTL
  ~10 min) — discovery freshness vs DHT load; punch keepalive ~every 15 s on idle
  sessions to hold the NAT mapping open.

A concrete starting `ChannelConfig` belongs in the engine (pure data): `{ id, mode:
{ReliableOrdered, UnreliableSequenced, Unreliable}, priority, max_bytes_per_tick,
keepalive_ms }`, with the bin's transport (QUIC streams, or dhtnet multiplexed ICE
channels) honoring `priority`/`max_bytes_per_tick`.

---

## 7. The pure / IO boundary (what becomes engine sim vs bin glue)

unendlich engine sim modules are **pure** — no sockets, DHT, clock, threads, or RNG.
Everything reusable here is *decision/codec* logic and ports cleanly; the actual IO
stays in the bin (where snapclone's C++ `dhtnet` / async Rust lives).

**Pure → engine `sim::` modules (port targets):**
- `net_candidates` ← `connreq_targets`: `order_targets`, `is_dead_end`. (drop-in)
- `net_transport_select` ← `selector`: the CGNAT/relay/punch decision (trim the
  `ReachabilityDb` dep to a passed-in hint, or port a small reachability table).
- `world_join_requests` ← `mailbox::requests::RequestMailbox`: Pending→Accept/Reject/
  Block, unblock, `pending()`/badge — the host's admit/deny model.
- `world_presence` ← `gossip::GossipView` + `contact`: TTL-pruned online-world view
  (time passed in as a tick value, not a clock).
- `world_advert` codec ← `mailbox::entry`/`envelope`: encode/decode the signed advert
  + candidate set (the `net_rendezvous` brief's `MailboxBlob`, now grounded in the
  real fields).
- `net_channels`: the §6 channel-config table (pure data + budgeting helper).

**Bin glue (NOT a worker module — documented for the engine side):**
OpenDHT bootstrap/`put`/`get` (or `microdht` for LAN/offline testing), STUN, UDP
socket send/recv + hole-punch packets, the relay client, Ed25519 signing/verify, the
wall clock and re-announce timer, QUIC/ICE channel transport. The pure modules decide
*what* to send and *how to rank/admit*; the bin performs it and feeds events back.

This is exactly the split the `net_rendezvous` worker brief (dispatch-r57) already
describes — I'll fold the real WorldAdvert field layout and the `order_targets`
ranking into that brief's SCHEMA section so the worker builds against the true shape.

---

## 8. Pruned source subset (`snapclone-net-pruned.zip`)

The minimum to start a world-join connection — presence, candidate ranking, transport
decision, join approval, and the value-layer DHT model — plus the reference docs. See
`PRUNED_MANIFEST.md` in the zip for the per-file role and the pure/IO tag. The Qt/UI,
media, groups, camera, audio, and C++ transport are intentionally excluded.

## 9. Suggested next steps
1. Confirm this mapping + the channel QoS table.
2. I fold the real WorldAdvert schema + `order_targets` ranking into the
   `net_rendezvous` brief and reissue that one worker zip.
3. Port the six pure modules in §7 into `sim::` (gated), then wire the bin IO
   (OpenDHT/microdht + UDP punch + relay) behind the world-selector lifecycle in §3.

---
## Chunk wire format (r71) — see docs/COMPRESSION_DECISION.md
Streaming octrees is wrong: they are 145–265 KB/chunk (a query structure, not a compressor).
Validated on real generated chunks — send instead, per chunk:
- **generated & unmodified → the seed** (coord + world seed, ~16 B); the client regenerates it
  bit-exactly via the deterministic WorldGenerator.
- **modified → lzma'd dense voxels** (or an lzma'd delta vs the regenerated base), ~0.5–0.9 KB.
The client builds its `OctreeArray` locally from the voxels; the octree never crosses the wire.
This is 250–480× less bandwidth than octree streaming (near-zero for pristine terrain).
