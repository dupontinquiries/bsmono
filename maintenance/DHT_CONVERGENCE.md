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
