# Netcode DHT — merge + web-kit interaction analysis

## The duplication (fixed)
I built `netcode::microdht` (a Kademlia-lite: NodeId/XOR/k-buckets/store) **before checking
snapcore** — which already *is* the fleet's Kademlia DHT. That left two DHTs. **Merged:** netcode's
profile store now rides snapcore (`store::SnapcoreStore` over `snapcore::store::Store`), `microdht`
is deleted, and profiles + chat share one key space (chat already used `snapcore::store::Store` via
share-kit's `Mailbox`). Verified: netcode 6 tests, bsgames 5 tests, GUI builds.

## snapcore = the one DHT
`shared/snapcore`: `routing` (20-byte `NodeId`, XOR distance, k-buckets, `RoutingTable::closest`),
`node` (the Kademlia RPC `Transport` trait), `store` (TTL k-v, `Key = NodeId`), `lookup` +
`cluster` (iterative `lookup_nodes` / `lookup_value_resilient<T: Transport>`). Consumers already on
it: **share-kit `Mailbox`** (sealed, content-addressed messaging), **snapclone** (relay + session +
selector implement transports over it), and now **netcode** (profiles/presence).

## Two transport layers — they complement, not compete
There are two `Transport` traits at different planes, and that's *correct*:

| | trait | ops | plane | used for |
|---|---|---|---|---|
| **snapcore** | `node::Transport` | `find_node` / `find_value` / `store` / `ping` | **control** | discovery, profiles, presence, session announcement |
| **games** | e.g. unendlich `sim::net::Transport` | `send` / `try_recv` (UDP/enet) | **data** | realtime gameplay packets |

So the fleet DHT is the **who-is-where** layer; the game's own UDP/enet transport stays the
**fast realtime** layer. The unification is a *handoff*, not a merge of the two:

```
announce:  game session ── snapcore DHT.store(session_key, {host, transport, addr}) ──▶ (control)
discover:  friend's profile.presence = InGame{session} ── DHT.find_value ──▶ session record
connect:   bsgames "▶ join" ── resolve session ──▶ game's UDP/enet Transport.send (data plane)
```

## The web kits
- **net-kit** (`wire`/`http`/`auth`/`json`) — the fleet's line format + auth framing. Consumers
  today: gitmind cockpit, ipc-kit. It's the natural encoding for the **control-plane** DHT records
  (session announcements, lobby state) so every device parses them identically; the data plane stays
  binary/enet.
- **snapclone** — the web-facing social app; its relay/session/selector are concrete `Transport`s
  over snapcore (the `RelayTransport` is the MagicWormhole relay port). bsgames chat rides the same
  snapcore store snapclone does, so a bsgames DM and a snapclone share are the same primitive.

## Concrete next steps (from this analysis)
1. **Session records on the DHT.** Define a `SessionRecord{game, host, transport, addr}` encoded
   with net-kit `wire`; `bsgames` "join" does `DHT.find_value(session_key)` → hands the addr to the
   game's transport. Makes "▶ join" real without a central server.
2. **One control transport.** Give the games a small adapter from *their* `Transport` to snapcore's
   `node::Transport` (or let them talk to a local snapcore node), so a game announces + discovers via
   the fleet DHT instead of a bespoke tracker. unendlich's `UdpTransport` stays the data plane.
3. **Retire the last bespoke bits.** Anything doing its own peer list (game lobbies) points at the
   snapcore routing table + profiles instead.
