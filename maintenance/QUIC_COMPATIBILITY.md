# Is the stack QUIC-compatible?

Short answer: **the codec is transport-agnostic and rides QUIC fine — but QUIC
is the wrong transport for DHT RPC and the right one for what follows it.**
Those are two different questions and the useful answer separates them.

## The codec layer: yes, and it already is

`dht-kit` has **zero transport references** — no `SocketAddr`, no socket, no
async, no runtime. It is bytes in, message out. So it rides UDP, QUIC unreliable
datagrams (RFC 9221), a QUIC stream, or a unix pipe without changing.

Sizes measured, since a datagram transport cares:

```
query (ping)              74 bytes
reply, 20 v4 nodes       628
reply, 20 v6 nodes       868
reply, one 1 KB value   1100
```

All inside **1200 bytes** — the figure QUIC uses for its own initial packets
(RFC 9000 §14) and safe on any IPv6 path, whose 1280-byte minimum MTU leaves
1232 after headers. `SAFE_DATAGRAM` and three tests now pin this, including the
realistic worst case of k = 20 v6 nodes, because a reply that quietly exceeds
the MTU still *usually* arrives — and a lookup dropped only on some paths is far
harder to diagnose than one that never worked.

## Why QUIC is the wrong transport for the DHT itself

QUIC's speed comes from amortising a TLS 1.3 handshake across a **sustained
connection**. Kademlia traffic is the opposite shape: an iterative lookup
contacts α = 3 new peers per round over several rounds, and **most peers are
contacted once and never again**. Paying a handshake per peer makes a lookup
slower, not faster, and holding connections open to hundreds of transient peers
is the memory profile a DHT specifically avoids.

This is why OpenDHT, BitTorrent's DHT and every other Kademlia are
UDP-datagram-based. It is not legacy.

## Where QUIC genuinely wins here

**The bulk transfer that follows a lookup.** snapclone's own design already
splits these: `snapclone-mailbox` documents a three-phase model — intent
published to the DHT (tiny, always succeeds, works on CGNAT), rendezvous when
both peers are reachable, then bulk transfer on a warm path. **Phase 3 is
exactly QUIC's case**: one peer, sustained, multiplexed, congestion-controlled,
and it survives the address changes a mobile device makes constantly.

So the split is already in the architecture:

| phase | traffic | transport |
|---|---|---|
| intent / lookup | many peers, one packet each | UDP datagrams — `dht-kit` |
| rendezvous | NAT discovery, hole punch | STUN — `nat-kit`, `snapcore::punch` |
| bulk transfer | one peer, sustained | **QUIC** |

## Two things QUIC would change, worth knowing before adopting it

* **Peer identity moves from the packet to the connection.** `snapcore::wire`'s
  `handle(node, from_addr, msg, now)` takes the source address from the socket
  precisely so a peer cannot claim one. Over QUIC the connection supplies it
  instead, which is stronger — but the guarantee has to come from somewhere, and
  a naive port would let the message's own contents fill the gap.
* **STUN becomes partly redundant.** QUIC has its own path validation and
  migration. `nat-kit` stays useful for discovering a reflexive address before
  any connection exists, which is still how a peer learns what to publish.

## Status

Nothing here needs changing to keep the QUIC door open — the seam is already in
the right place. What would need building is a QUIC transport for phase 3, and
that is a transport crate rather than a change to any kit.
