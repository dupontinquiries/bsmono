# Three DHTs, one goal: Rust, many devices, OpenDHT-compliant

The stated end goal is a DHT written in Rust, running on many devices, ideally
**network-compatible with OpenDHT**. That is a stricter bar than API
compatibility, and it changes which of the fleet's three DHT layers matters.

## What exists, measured

| crate | lines | wire format | engine | used by |
|---|---:|---|---|---|
| `shared/snapcore` | 2,255 | **none** | routing + lookup + store + punch + schedule + cluster + sim | weather-share, share-kit, net-kit, games/netcode, bsgames |
| `apps/snapclone/rust/snapclone-dht` | 1,158 | **OpenDHT-shaped msgpack** | sans-io, `handle_packet(bytes) -> Vec<Out>` | snapclone |
| `apps/snapclone/rust/snapclone-microdht` | — | line-based, local | client/server over UDP | one integration test |

`snapclone-microdht` was the **first attempt** — establishing the API boundary
with a quick local-net implementation, not a network-compatible DHT. It has done
its job, and the one test still using it now has a snapcore-backed twin.

## The two halves are in different crates

This is the finding that matters. **snapcore has the better engine and no wire;
snapclone-dht has the wire and a thinner engine.**

snapcore's handlers take typed Rust values — `handle_find_node(&Contact,
&NodeId)` — so there is nothing to put on a socket. It also has no `listen` or
`refresh`, which are how OpenDHT does subscription and value keep-alive.

snapclone-dht's `wire.rs` speaks the reference's actual format: msgpack maps
keyed `t` (transaction), `y` (kind: `q`/`r`/`e`), `q` (query name), `a`
(arguments), `r` (reply), with query names ping/find/get/put/listen and
arguments `id`/`target`/`token`/`values`/`nodes`. Its engine is packet-in,
packets-out — the right shape for a real network.

## What was done here

**The compatibility claim had no test behind it.** `wire.rs` was 225 lines
asserting OpenDHT shape with zero tests. Now 10, chosen for the failure modes a
hand-rolled msgpack subset actually has:

* every query kind round-trips, and a `put` keeps its value — a codec that drops
  it stores nothing and reports success
* a reply carries nodes *and* values together, which the reference permits and
  an either/or codec would silently truncate
* **truncation at every byte length is refused** — a decoder returning a partial
  message puts attacker-controlled short reads into the routing table
* trailing garbage is ignored, because datagrams get padded
* values at 30/32/254/256/4000 bytes, straddling fixstr's 31 and bin8's 255,
  where a subset codec writes the wrong header byte
* transaction ids across the full u32 range, since a narrowed tid mismatches
  replies under load rather than failing

22 tests green in the crate.

## The convergence, in order

1. **Give snapcore the wire.** Lift `wire.rs` into `shared/snapcore` so the
   engine every other app already uses can speak the protocol. The codec is
   self-contained apart from `InfoHash`/`Addr`/`Value`, which snapcore has
   equivalents of.
2. **Add `listen` and `refresh` to snapcore.** Subscription and keep-alive are
   protocol features, not conveniences — without them a compliant peer cannot
   participate in a real swarm.
3. **Test against the reference.** Round-tripping our own encoder proves
   self-consistency, not compliance. The only real check is bytes from an actual
   OpenDHT node, and until that exists every claim here is "shaped like" rather
   than "compatible with".
4. **Then retire the other two.** `snapclone-microdht` first, since its last
   consumer already has a replacement.

Step 3 is the one that cannot be faked, and nothing in this repository currently
does it.

---

## Parity harness — step 1's safety net, landed

`snapclone-dht/tests/lan_parity.rs` runs the same LAN exchange through both
engines and compares what an observer sees. Four tests, all green.

**What parity means here, and what it cannot.** Not byte-identical packets —
snapcore has no wire at all, so asserting that would assert something that
cannot be true. Instead: given N nodes, a bootstrap, a put, and a get **from a
node that never stored the value**, do both deliver the same bytes from a lookup
that actually traversed other nodes?

| test | why it is the one to write |
|---|---|
| delivery to a non-storing reader | the property every caller depends on, and the one that must survive swapping the engine underneath |
| neither invents a value for an unstored key | the easier half to get wrong: returning something for an unknown key is worse than returning nothing, because the caller cannot tell |
| convergence at n = 4, 8, 16 | parity of SHAPE — if one engine needed twice the network, the swap would change behaviour under real conditions while both still passed the delivery test |
| payloads of 64, 300, 4096 bytes | a LAN transfer is not 30 bytes; this is where a subset wire starts writing the wrong length prefix and where a value-size cap diverges silently |

Both run with no sockets and a fixed clock, so a disagreement is behavioural
rather than a timing race.

**This is what makes step 1 safe.** Giving snapcore the wire means callers move
from one engine to the other, and until now nothing checked that the move
preserved anything. The harness is also the regression net for steps 2 and 4:
adding `listen`/`refresh` and retiring the older crates both have to keep these
four green.


---

## `shared/dht-kit` — the messaging layer, extracted

The fleet's kits are per-layer, and the DHT lacked one. `shared/dht-kit` is
**OpenDHT-compatible messaging and nothing else**:

* **no transport** — bytes in, message out; no sockets, no async, no runtime
* **no routing table, no engine, no storage** — those differ between
  implementations and are not what interoperability is made of
* **no dependencies** — a protocol crate that drags in serde forces a version
  choice on every consumer, and two engines with different toolchains is exactly
  the situation this serves

It carries the six query names the reference sends — including **`refresh`**,
which neither existing engine implements. A kit that shipped only the five we
already used would make "compatible" mean "compatible with what we had".

13 tests, chosen for what a hand-rolled msgpack subset actually gets wrong:

* truncation at **every** byte length is refused — a decoder returning a partial
  message puts attacker-controlled short reads into a routing table
* values at 0/1/254/255/256/65535/65536 bytes, straddling both bin8 and bin16
  boundaries, where a subset codec writes the wrong header and the payload
  shifts by one
* a node entry that is not 20-byte hash + 6-byte address is refused, because a
  different width means a different address family and guessing is worse than
  failing
* an **unknown map key is refused rather than ignored** — skipping unknown keys
  sounds tolerant and is how two implementations silently disagree about what a
  message meant
* transaction ids across the full u32 range, since a narrowed tid mismatches
  replies under load rather than failing

**Still not compliance.** Round-tripping our own encoder proves self-consistency.
Until bytes from a real OpenDHT node decode and re-encode identically, this is
"shaped like the reference". That test needs one packet capture and nothing else.

---

## `shared/nat-kit` — the other half of the stack

A DHT that cannot learn its own public address cannot be reached by peers behind
NAT, so STUN is not an optional extra here: it is the step before a routing
table means anything. The stack now has one kit per protocol layer:

| layer | kit | deps |
|---|---|---|
| DHT messaging | `dht-kit` | none |
| NAT discovery | `nat-kit` | none |
| leaf transport primitives | `net-kit` | — |
| engine | `snapcore` | — |

STUN already existed in `snapclone-nat`, ported from `NatTraversal.cpp` — but it
pulls **tokio, anyhow, bytes, rand and tracing**, which is right for an app and
wrong for a layer two engines with different toolchains must share.

**No randomness in the kit.** The caller supplies the transaction id. A crate
that generated one would either drag in `rand` or ship a weak PRNG, and the id
is security-relevant: it is the only thing matching a response to its request.

12 tests. Three worth naming:

* **the raw address must not appear on the wire** — that is the entire purpose
  of XOR-MAPPED-ADDRESS, since NATs rewrite addresses they recognise inside
  packet bodies, and a plain address gets mangled in transit by the very device
  describing it
* **v6 xors with cookie ++ tx where v4 uses the cookie alone** — getting that
  wrong decodes to a plausible but wrong address rather than failing, so the
  test asserts the same address under a different tx encodes differently
* **a packet with its top bits set is not STUN** — this is what lets STUN share
  a socket, and it is how a DHT packet arriving on the STUN port is rejected
  rather than misparsed into a routing decision

**Unknown attributes are skipped here and refused in `dht-kit`** — opposite
choices, deliberately. We control both ends of the DHT and neither end of a
public STUN server; refusing unknown comprehension-optional attributes would
reject valid responses from most of the internet.

---

## Step 1 done: snapcore can speak the wire

`shared/snapcore/src/wire.rs`, behind a `wire` feature that is **off by
default**. snapcore has had zero dependencies and five crates rely on that —
`cargo tree` still shows it dependency-free in the default build, and
share-kit, net-kit, netcode and bsgames were each built to confirm.

The bridge translates and nothing more: a decoded `Msg` becomes the right
`Node::handle_*` call and the answer becomes a `Msg` back. It does not own a
socket, retransmit, or decide when to query — those belong to a transport and to
the lookup driver, and putting them here would make this a third engine rather
than the seam between the two that exist.

Decisions worth keeping:

* **A reply is never answered.** Answering an answer is an amplification loop,
  not a protocol.
* **The sender's socket address is used, never one it claims.** A node that
  could name its own address would insert peers wherever it liked — that is how
  a routing table fills with one attacker's endpoints.
* **A v6 contact is omitted from a reply rather than mapped to 0.0.0.0**, which
  would advertise a peer nobody can reach.
* **A put with no value is an error, not a silent success**, or the sender
  believes its data landed.
* **`refresh` answers 501.** Nothing beneath implements value keep-alive, and a
  reply saying "fine" while the value expires is worse than a refusal.

A failing test taught something on the way: two queries from one node id at
different addresses correctly **replace** rather than duplicate. That is right —
a phone leaving wifi keeps its id and arrives from a new socket, and two entries
for one id send half the traffic to an address that stopped answering — so the
behaviour is now pinned by its own test.

**Remaining: steps 2–4.** `listen`/`refresh` in the engine, a real capture to
turn "shaped like" into "compatible with", then retire the older crates.
