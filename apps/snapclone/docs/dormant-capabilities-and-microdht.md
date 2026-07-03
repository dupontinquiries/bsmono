# Dormant capabilities, default-flip candidates, and micro-DHT planning

This is an audit of capabilities that are **built but turned off**, a recommendation
on which defaults to flip (encryption, DHT), and design thinking on the micro-DHT.

Written after auditing the build flags, the runtime QSettings defaults, and the
`snapclone-crypto` / `snapclone-microdht` crates.

---

## 1. What's built but dormant

### Build-time (CMake) — all OFF by default
| Flag | What it gives | Current fallback when OFF |
|---|---|---|
| `SNAPCLONE_RUST_CRYPTO` | Real AES-256-GCM + X25519 ECDH + Ed25519 + ML-KEM (PQ) | **HMAC-SHA256 XOR-stream "placeholder crypto", dev-only** |
| `SNAPCLONE_OPENDHT_BACKEND` | Full Kademlia DHT peer discovery (OpenDHT) | **No DHT at all — LAN gossip / WebSocket only** |
| `SNAPCLONE_JAMI_BACKEND` | Jami/dhtnet P2P stack | same as above |

### Runtime (QSettings) — notable gates
| Key | Default | Effect |
|---|---|---|
| `beta/micro_dht_enabled` | `false` | The lightweight single-node rendezvous "DHT" (see §3) |
| `dht/parkScreenOff` | `false` | Suspends DHT traffic when screen off (battery) |
| `ui/fullBleedPreview` | `true` | Fullscreen camera redesign (now on) |
| `audio/speechBoost` | `"off"` | VAD-driven speech gain |
| `network/relayUrl` | empty | TURN-style relay fallback for when P2P fails |

---

## 2. Recommendation on flipping defaults

### Encryption: `SNAPCLONE_RUST_CRYPTO` — YES, should become the default, with caveats

The default build today ships **placeholder crypto**: an HMAC-SHA256 XOR keystream
with a documented "development/testing only" banner, and a key-agreement step that
the source itself calls "NOT secret-key agreement … purely a dev placeholder."
For an app whose whole pitch is *encrypted* snaps, shipping that as the default is
the single most important thing to fix.

The real crate (`snapclone-crypto`) is properly constructed:
- AES-256-GCM (authenticated) via the `aes_gcm` crate
- X25519 ECDH for session-key agreement via `x25519_dalek`
- Ed25519 signatures via `ed25519_dalek`
- ML-KEM-768 + a hybrid (classical⊕PQ) session key — forward-looking PQ

**Caveats before flipping the CMake default to ON:**
1. **Test coverage was lopsided.** The 9 existing tests all covered the ML-KEM /
   hybrid path; the *classical core* that an AES build actually uses had no direct
   tests. I added 6 (AES-GCM roundtrip, tamper-detection, wrong-key, buffer-too-
   small, X25519 both-sides-agree, Ed25519 sign/verify+tamper). They're written
   against the real FFI signatures but I could **not execute them in this
   environment** — the apt Rust (1.75) is older than the `edition2024` floor some
   modern crypto deps require. They must be run on a toolchain ≥1.81 before trusting
   the flip.
2. **Key-file migration.** The placeholder derives public keys via
   `HMAC-SHA256(priv, "pubkey")`; the real backend uses X25519. Keys generated under
   the placeholder are **not** valid X25519 keypairs. A straight default-flip will
   invalidate any existing identity. Need either a migration (regenerate identity on
   first real-crypto launch) or accept that beta identities reset.
3. **Build dependency.** ON requires `cargo` present at configure time (the
   `find_program(CARGO_EXE … REQUIRED)`), so the CI/build image must have Rust.

**Suggested path:** flip the default to ON, gate identity regeneration behind a
one-time migration check, and keep `-DSNAPCLONE_RUST_CRYPTO=OFF` available for fast
dev builds that don't want the cargo dependency.

### DHT: more nuanced — do NOT just flip OpenDHT on

The honest finding from the device logs: the phones that produced our logs were
built with `OPENDHT_BACKEND=ON` (we saw `OpenDHT node stopped`, `DHT lookup`). The
*default* build has no DHT at all. So "the DHT" people have been testing is the
OpenDHT one, not a default.

OpenDHT as a hard default is heavy: it pulls gnutls/nettle/argon2/msgpack, builds
from a submodule, and a full Kademlia node on a phone is real battery + bandwidth.
For a postmarketOS target that's a lot. **This is exactly why the micro-DHT exists**
(§3) — and why I'd push energy there rather than making OpenDHT the default.

Recommendation: keep OpenDHT an opt-in build, and make the **micro-DHT the default
rendezvous** once it's hardened (it's currently behind `beta/micro_dht_enabled`).

---

## 3. Micro-DHT: current design + how I'd evolve it

### What exists today (`snapclone-microdht`, 19 passing tests)
A complete, dependency-free system:
- `node.rs` — a UDP server: `Mutex<HashMap>` store + a background TTL sweeper
- `store.rs` — key → list-of-values, per-value TTL, re-announce refreshes TTL
- `protocol.rs` — line protocol: `PING` / `PUT key hex ttl` / `GET key` / `STATS`
- `client.rs` — request/response client
- `main.rs` — standalone `sc-microdht` binary
- C++ `MicroDhtClient` — the in-app UDP client (same protocol), behind the beta flag

It is **hub-and-spoke**, not a distributed hash table: one central node holds all
the key→value mappings. Calling it a "DHT" is aspirational — today it's a tiny
rendezvous/mailbox server. For the actual problem (two phones behind one NAT needing
to exchange addresses + connection requests) that is *exactly enough*, and far
cheaper than Kademlia.

### What it's for
The same role the OpenDHT mailbox plays: a peer PUTs its current addresses + any
queued connection-requests under its `scId`; the other peer GETs them. It's the
fallback that **does not depend on the AP forwarding broadcast** (the thing AP
client-isolation breaks). Last session's hairpin fix made CONNREQs carry the LAN
address; the rendezvous is how those CONNREQs reach a peer that gossip can't.

### If I could change it — the plan

**Stage 0 — harden the single node (do this before flipping the default):**
- **Auth / abuse resistance.** Today any client can `PUT` under any `scId`. Since
  every identity already has an Ed25519 key (in the real-crypto crate), require PUTs
  to be **signed**: `PUT scId hex ttl sig`, where `sig` covers `scId||value||ttl`
  and the node verifies against the pubkey embedded in / derived from the scId. This
  stops a malicious client from poisoning a victim's rendezvous record.
- **Rate-limiting + value caps.** A per-source-IP token bucket and a max-values-per-
  key cap (the store already lists multiple values per key — cap it, evict oldest).
  Prevents a single client from filling the node's RAM.
- **Size + TTL ceilings.** Clamp TTL (e.g. ≤2h) and value size (≤4KB — a CONNREQ blob
  is ~100B, an address set a few hundred). The store is `Mutex<HashMap>`, so an
  unbounded PUT is a trivial DoS today.
- **Persistence is optional.** A rendezvous is ephemeral by nature; an in-RAM store
  that loses everything on restart is acceptable (peers re-announce every ~55 min).

**Stage 1 — remove the single-point-of-failure (make it actually distributed-ish):**
The cheapest step up from hub-and-spoke is **not** full Kademlia. Two lighter ideas:
- **N-of-M replication across a small fixed fleet.** Run 3–5 known nodes; the client
  PUTs to all, GETs from all and unions the results (the store already returns a
  *list* of values per key and de-dups on re-announce, so union-on-GET is natural).
  This survives any single node dying without any routing complexity. The client
  already times out per-request, so fanning out to M nodes and taking the first/
  union of responses is a small change to `MicroDhtClient`.
- **Rendezvous-hashing the keyspace** if the fleet grows: `node = argmin_i H(scId||node_i)`.
  Each scId deterministically maps to a primary + a couple of replicas. This is
  consistent-hashing-lite — no finger tables, no iterative routing, just a hash. It's
  the 80/20 of a DHT: you get key→node assignment and graceful fleet changes without
  Kademlia's machinery.

**Stage 2 — only if the fleet must be permissionless:** *then* it's worth real
Kademlia (or just turning OpenDHT back on). But for a friend-scale / community-scale
app, a small operator-run replicated fleet (Stage 1) is almost certainly the right
ceiling. Kademlia's cost (routing tables, iterative lookups, churn handling, eclipse/
Sybil concerns) only pays off at open-internet scale.

### Why I'd stop at Stage 1
The micro-DHT's whole reason for existing is that OpenDHT is too heavy for the
target. Re-implementing Kademlia badly would throw that advantage away. The actual
requirement — "a phone can reliably leave a small encrypted blob somewhere its
contact can pick it up, even behind a hostile NAT" — is met by a replicated
rendezvous fleet with signed PUTs. That's a few hundred lines on top of what already
exists, stays dependency-free, and is testable deterministically (the existing tests
already spin up a node in-process and round-trip against it).

### Concrete next code steps (not done here — design only, as asked)
1. Add `sig` to the `PUT` line in `protocol.rs`; verify in `node.rs` against the
   crypto crate. (Couples microdht to snapclone-crypto — acceptable; both are ours.)
2. Add a token-bucket + per-key value cap to `store.rs` (pure logic, easy to test).
3. Add multi-node fan-out to `MicroDhtClient` (PUT-to-all, GET-union).
4. Only after the above: flip `beta/micro_dht_enabled` default → true, and point it
   at the replicated fleet.

---

## 4. Summary of recommended default changes
| Change | Confidence | Blocker |
|---|---|---|
| `SNAPCLONE_RUST_CRYPTO` → ON (real encryption) | High — it's the right thing | Run the new classical-core tests on rustc≥1.81; handle identity migration |
| Keep `OPENDHT_BACKEND` opt-in | High | — |
| Micro-DHT → default rendezvous | Medium | Sign PUTs + rate-limit + replicate first (Stage 0/1) |
| `dht/parkScreenOff` → consider ON | Low/medium | Battery win vs. delivery latency tradeoff; needs device measurement |

The crypto default-flip is the highest-value, most clearly-correct change. The
micro-DHT is the most *interesting* one and is genuinely close — it's a working
system a few hardening steps away from being a sound default.

---

## 5. Carrier-pigeon (Pollinator) + Bluetooth as first-class routes, and cross-network hole-punching

Two follow-up questions: can the LAN beacon ("carrier pigeon" = Pollinator) and
Bluetooth become first-class routes in the in-house DHT, and is cross-network hole-
punching still better left to OpenDHT?

### The seam already exists: `IDiscovery`
`NetworkManager.hpp` defines `IDiscovery` — `start/stop`, `announcePresence`,
`lookupPeer`, `setPunchPort/setStunHost`, and signals `peerDiscovered/peerLost/
networkChanged`. Both `GossipDiscovery` and OpenDHT's `DhtDiscovery` implement it.
There's even a forward-looking `TransportDescriptor` comment anticipating "community-
led mesh transports (LoRa, packet radio, sneakernet, mesh wifi)". So making a new
route "first-class" means: **implement `IDiscovery` (and/or a transport) and register
it in the same resolver the others use**, rather than bolting it onto one screen.

### Carrier pigeon (Pollinator) — YES, promote it; it's the easiest win
Today `PollinatorDiscovery` is a UDP multicast beacon (224.0.0.251:9879, TTL=1,
LAN-only) that is **only active while the Nearby screen is open**. That's a UX gate,
not an architectural limit — it already emits `peerBeaconed(id, name, senderAddr)`
with the sender's authoritative LAN address, which is exactly what auto-connect needs.

What "first-class" would mean concretely:
- Run the beacon **whenever the app is foreground on a LAN** (not just on the Nearby
  screen), so same-WiFi peers discover each other passively. This is the cheapest
  fix for the "0 peers observed" problem when an AP *doesn't* isolate clients.
- Fold its `peerBeaconed` into the same `peerDiscovered` path the gossip/DHT routes
  use, so a beacon hit is a normal discovery event that triggers `connectToPeer`.
- Note it overlaps `GossipDiscovery` (also multicast+broadcast). They should be
  **unified or de-duplicated** — running two near-identical multicast beacons on
  different ports is wasteful and confusing. I'd keep one LAN-discovery component.

Caveat (the honest part): on an AP with **client isolation**, multicast/broadcast
between clients is blocked, so the pigeon can't fly there either — same limitation as
gossip. It helps exactly the case gossip already helps (isolation-free LANs), so the
value is "make the existing LAN path always-on and unified," not "a new route that
defeats isolation." It does NOT replace the rendezvous for the hostile-NAT case.

### Bluetooth — NOT a transport today; promotion is a real build, not a flip
Important honest finding: `snapclone-proximity` is **RSSI ranging + bearing
estimation** (a "point your phone to find your friend" feature). It estimates coarse
distance bands and a gradient-following bearing from BLE signal strength. **It does
not move bytes.** There is no BLE data transport (no GATT service, no L2CAP/RFCOMM
socket, no chunked-transfer layer) anywhere in the tree.

So "Bluetooth as a first-class route" is not a dormant capability to switch on — it's
a feature to build. What it would take:
- A **GATT service** (or L2CAP CoC for throughput) exposing a snap-transfer
  characteristic; chunking + reassembly (BLE MTU is ~185–512B, snaps are MBs, so this
  is many writes with flow control); pairing/bonding tied to the existing identity
  keys so you don't transfer to a stranger.
- An `IDiscovery`/transport impl wrapping it, plus a `TransportDescriptor` advertising
  low bandwidth so the router chunks small and defers non-essential traffic (the
  descriptor comment already anticipates exactly this).
- Realistically BLE throughput is ~1–2 Mbps best case, often far less — fine for the
  *rendezvous blob* (addresses + CONNREQ, ~hundreds of bytes) but painful for a
  multi-MB video snap. **The high-value use is BLE-as-signaling**, not BLE-as-bulk-
  transport: two co-located phones exchange addresses/keys over BLE, then do the bulk
  transfer over WiFi. That sidesteps AP client-isolation entirely (BLE is peer-to-
  peer, no AP involved) and is the genuinely compelling reason to build it.

Recommendation: treat BLE as a **signaling/rendezvous transport first** (small, high-
value, defeats AP isolation), and only consider BLE bulk transfer later if a real
offline-proximity use case demands it. Reuse the proximity crate's presence (it
already scans/ranges) to know a peer is *there*, then open a small GATT channel to
swap the rendezvous blob.

### Cross-network hole-punching — STUN is independent; the SIGNALING is the question
The hole-punch itself (`NatTraversal.cpp`) is **not** OpenDHT-specific:
- STUN reflexive-address discovery uses public STUN servers (Google/Jami) — works
  regardless of which discovery backend is active.
- Simultaneous UDP punch + TCP-after-punch is pure socket work.

The ONE thing punching needs from "the DHT" is a **signaling/rendezvous channel**:
both peers must learn each other's `{public IP, punch port}` and start punching within
the same ~2s window. Today that coordination rides on OpenDHT (publish my punch
endpoint, read the peer's). But that role is a generic **PUT/GET of a small blob keyed
by scID with a TTL** — which is *exactly* what the micro-DHT already does.

So: **cross-network punching is NOT inherently better on OpenDHT.** What OpenDHT gives
that the micro-DHT (today) doesn't is:
1. **No central operator / no single point of failure.** OpenDHT is a real
   distributed network; the micro-DHT is one node. (Stage-1 replication in §3 closes
   most of this gap.)
2. **Reachability without you running infrastructure.** OpenDHT bootstraps off the
   public Jami/OpenDHT network; the micro-DHT needs a node *you* host.

Neither of those is about punching *quality* — the punch succeeds or fails based on
NAT types (full-cone/restricted/symmetric/CGNAT), not on which channel delivered the
endpoints. A symmetric-NAT/CGNAT pair fails to punch regardless of OpenDHT vs micro-
DHT; that's what the `network/relayUrl` TURN-style fallback (currently empty) is for.

**Conclusion on punching:** the micro-DHT can serve as the punch-signaling channel
just as well as OpenDHT, because the requirement is only timely PUT/GET of the
endpoint blob. Keep OpenDHT as the option for users who want zero-infra,
no-central-operator reachability; use the (replicated, signed) micro-DHT as the
default signaling channel. The actual cross-NAT success rate is governed by NAT type
and the presence of a relay fallback — so the higher-leverage work for "snaps reach
across networks" is **wiring up `network/relayUrl` (a TURN relay)**, not choosing
between the two DHTs.

### Net recommendation for routes
1. **Promote Pollinator to always-on LAN discovery, unified with gossip.** Cheap,
   helps the isolation-free LAN case immediately. (Pure wiring; no new transport.)
2. **Build BLE as a signaling transport** (rendezvous-blob over GATT), reusing the
   proximity crate for presence. This is the one route that genuinely defeats AP
   client-isolation for co-located peers. Medium effort; high value; not a flip.
3. **Make the micro-DHT a first-class `IDiscovery`** (signed PUT/GET, replicated) so
   it serves punch-signaling co-equally with OpenDHT.
4. **Wire `network/relayUrl`** — the missing piece for the cross-NAT cases that no
   amount of signaling can punch through. This, not OpenDHT-vs-micro-DHT, is what
   determines whether hard-NAT pairs ever connect.

All four plug into the existing `IDiscovery` / transport seam, so none requires
re-architecting — the abstraction is already there and already has multiple
implementations.

---

## 6. BLE as a control/metadata plane: mailbox, thumbnails, contact-sync (NOT full photos)

The right question. Reframing BLE from "carry snaps" to "carry the small stuff" turns
a marginal transport into a strong one, because every control/metadata payload is
*tiny* and BLE's weakness is only bulk throughput.

### Size budget (the whole argument)
Conservative BLE goodput assumption: **~20 KB/s** sustained over GATT (real phones do
10–50 KB/s; L2CAP CoC does better; 20 is a safe floor for budgeting).

| Payload | Measured / estimated size | Time over BLE @20KB/s | Verdict |
|---|---|---|---|
| **Mailbox blob** (CONNREQ / address rendezvous) | ~107 B (from logs: 24B nonce + ~83B ct) | <10 ms | **Slam dunk** |
| **Contact record** (vCard: scId + names + 32B pubkey) | ~80–150 B each; 50 contacts ≈ 5–7 KB | <1 s for a whole address book | **Easily fits** |
| **Thumbnail** (~160px JPEG q70) | ~5–10 KB each | ~0.3–0.5 s each; strip of 10 ≈ 3–5 s | **Fits as background prefetch**, marginal on-demand |
| Full photo / video | 0.1–34 MB | minutes | **Out of scope (correct)** |

So mailbox and contact-sync are *ideal* BLE payloads — sub-second, no chunking
stress. Thumbnails fit but are the heavy end: great as a background "pull the preview
strip while we're near each other," less great as "tap a conversation and wait."

### Why the building blocks are already there
- **Mailbox is already a keyed-blob primitive.** `publishConnectionRequest(target,
  blob)` + the mailbox check are a PUT/GET of a small encrypted blob keyed by scId.
  That's transport-agnostic in shape — BLE just becomes another carrier for the same
  blob. No new payload format needed.
- **Contact-sync already has a serialization: `VCardIO`.** Contacts export/import as
  vCard 3.0 with the routing ID and key embedded (UID for dedup on re-import). A BLE
  contact-sync is literally "send the vCard text over a GATT characteristic, run it
  through `importVCard` on the other side." The hard part (canonical serialization +
  dedup) is done.
- **Thumbnails already exist as small JPEGs** via `ThumbnailService` — no new
  rendering, just transfer the already-cached thumb file.

### How it would slot in
This is the BLE-as-signaling transport from §5, generalized slightly to a **"small
encrypted blob" channel** with a tiny type tag:
```
[1B type][2B len][payload]   type ∈ { MAILBOX, VCARD, THUMB }
```
- A GATT service with one "blob in / blob out" characteristic (or two: write + notify).
- Chunking only matters for thumbnails (5–10 KB > MTU), and it's simple sequential
  chunks with a length prefix — far easier than streaming MB media with flow control.
- Each blob is already encrypted at the app layer (mailbox blobs are; vCard/thumbs
  would be sealed to the peer's key the same way), so BLE pairing security is a
  second layer, not the only one.

### The genuinely compelling part: it works where nothing else does
This control plane is most valuable **exactly when the IP paths fail**:
- **AP client isolation** blocks gossip/Pollinator and same-WiFi direct — but BLE is
  peer-to-peer, no AP involved. Two co-located phones can still swap the mailbox blob
  (→ then punch/connect over the internet) and sync contacts/thumbnails.
- **No internet at all** (festival, transit, dead zone): BLE contact-sync and
  thumbnail preview still work face-to-face; the actual photos queue in the outbox
  and deliver later over IP (the persist-first + SYNC model already handles that).

So BLE-as-control-plane is not just "a smaller transport" — it's the **offline /
isolation-proof rendezvous and metadata channel**, with full media deferred to
whenever an IP path comes back. That's a coherent, honest product story: *be near
your friend → contacts and previews sync instantly over BLE → the full snaps follow
over WiFi/internet when they can.*

### Effort / honesty
- Mailbox-over-BLE: **small** — the blob and the keyed PUT/GET already exist; wrap the
  existing mailbox blob in the GATT channel.
- Contact-sync-over-BLE: **small** — `VCardIO` is the payload; add the channel + an
  import-confirm UX.
- Thumbnail-over-BLE: **medium** — needs chunking + reassembly and a "which thumbs
  does the peer lack" diff (mirror the snap SYNC hash-reconciliation, but for thumb
  hashes). Worth doing after the first two prove the channel.
- Still requires building the GATT service + bonding tied to identity keys (the one
  real new subsystem). But the *payloads* are all ready, which is most of the risk.

### Recommendation
Yes — and this is a better first use of BLE than either signaling-only or bulk media.
Build the GATT "small blob" channel once, then carry: (1) the mailbox blob (defeats AP
isolation for rendezvous), (2) vCard contact-sync (instant face-to-face add), (3)
thumbnails as background prefetch. Keep full photos/videos on IP transports (deferred
via the existing outbox/SYNC). The split matches BLE's physics exactly: tiny control
data goes BLE-first, bulk media goes IP-first.
