# snapclone-microdht

A tiny, **dependency-free** DHT node for SnapClone beta testing.

The production app talks to the real Jami/OpenDHT network. That's right for
shipping but hard to test: you can't deterministically reproduce "peer
published its endpoint → I looked it up → the value expired an hour later," and
you can't run it offline. This crate is a **local stand-in** that reproduces the
*app-visible value-layer semantics* of the DHT — keyed by sc-ID, opaque
encrypted endpoint values, TTL expiry, multiple values per key, re-announce
de-duplication — over a trivial UDP **text** protocol.

It is **not** wire-compatible with OpenDHT (no Kademlia routing, no node IDs, no
OpenDHT packet format). A real device on the real DHT cannot talk to it. It
models the value layer the app cares about, not the transport.

## Run a node

```
cargo run -p snapclone-microdht --bin sc-microdht           # 127.0.0.1:4544
cargo run -p snapclone-microdht --bin sc-microdht -- -b 0.0.0.0:4544
cargo run -p snapclone-microdht --bin sc-microdht -- --quiet
```

Poke it by hand (UDP, one line per datagram):

```
echo -n 'PUT sc-deadbeef 6869 60' | nc -u -w1 127.0.0.1 4544   # -> OK
echo -n 'GET sc-deadbeef'         | nc -u -w1 127.0.0.1 4544   # -> VALUES sc-deadbeef 6869
echo -n 'STATS'                   | nc -u -w1 127.0.0.1 4544
echo -n 'PING'                    | nc -u -w1 127.0.0.1 4544   # -> PONG
```

## Protocol

| Request                          | Response                                   |
|----------------------------------|--------------------------------------------|
| `PING`                           | `PONG`                                     |
| `PUT <sc-id> <hexvalue> <ttl>`   | `OK`                                       |
| `GET <sc-id>`                    | `VALUES <sc-id> <hex> <hex> ...`           |
| `STATS`                          | `STATS keys=.. values=.. puts=.. gets=.. expired=..` |
| (unparseable)                    | `ERR <reason>`                             |

`sc-id` is the ASCII sc-ID string (`sc-XXXXXXXX`). Values are opaque blobs
(the app's encrypted endpoint payloads), hex-encoded on the wire. TTL is in
seconds, capped at `MAX_TTL_SECS` (3600) to match OpenDHT's ~1h value lifetime.

## App integration

The C++ side speaks this protocol through `Network::MicroDhtClient`
(`include/network/MicroDhtClient.hpp`). The "Beta testers" settings submenu
(Settings → About → Beta testers) exposes:

* `beta/micro_dht_enabled` — route DHT publish/lookup here instead of OpenDHT,
* `beta/micro_dht_host` — the node `IP:port` (default `127.0.0.1:4544`),
* a "Test connection" button that PINGs the node.

**Wiring boundary (not yet connected to the live DHT path):** the four
operations to route when the toggle is on are `DhtDiscovery::announceAddress`,
`announcePresence`, `lookupPeer`, and `publishConnectionRequest`
(`src/network/JamiAdapter_dht_impl.cpp`). The toggle currently records intent;
hooking those entry points to `MicroDhtClient` is a small, reviewable change
that was deliberately left out until it can be compiled and tested on-device,
to avoid silently altering the load-bearing real-DHT path.

## Tests

```
cargo test -p snapclone-microdht
```

Covers the store (TTL expiry, re-announce dedup, multi-value, sweep), the
protocol (hex round-trip, request parsing, error cases), the node
(ping/put/get/stats, TTL cap, end-to-end over a real UDP socket), and the
client (round-trip against a live node).
