# Phone → compute cluster: the flow, and what each stage is waiting on

Goal: a phone away from home reaching an M2 Ultra and an M4 mini. Every decision layer is built and
tested; the flow does not run. This is where each stage stops.

## The stages

| # | stage | state | blocked on |
|---|---|---|---|
| 1 | this machine has an identity | **done** — real Ed25519, persisted at `~/.local/share/logscope/identity.key` | — |
| 2 | operator registers a peer by id | built (`machineid`, UI) | **nothing — exercisable now** |
| 3 | registration binds a key | built (`trustkit::register_keyed`) | **nothing — exercisable now** |
| 4 | find where the peer is now | built (`rendezvous`) | no announce; nothing writes an address anywhere |
| 5 | learn our own public address | **done** — `stunwire`, a real STUN Binding request; `nat-probe` runs it | — |
| 6 | decide punch vs relay | built (`natpath`) | **nothing — run `nat-probe` on each machine** |
| 7 | prove the peer is that peer | built (`peerauth`) | **nothing — exercisable now** |
| 8 | place duties by reach | built (`workerkit`) | everything above |

**Stage 1 is done.** It gated five of the other seven; three of those (2, 3, 7) are now exercisable with
no network at all, and the remaining blockers are all one thing: **no socket has ever been opened.** It is also the smallest: generate an Ed25519 keypair, persist
it beside the config, derive the id from the public half. `shared/crypto-kit` already provides the
primitive; no crate here depends on it yet.

## Why the order is not obvious

The tempting first move is the transport — sockets feel like the missing thing. They are not: with a
placeholder identity, a working transport connects two machines that both believe they are the same
peer, and `peerauth` refuses them correctly. **The transport would be built, correct, and untestable.**

Identity first makes stages 2, 3 and 7 immediately exercisable without a network at all: register a
second machine's id by hand, verify `trustkit` binds its key, verify a signed challenge round-trips.

## What is deliberately not built

**A DHT.** `snapcore` is 1,615 lines of Kademlia in Rust with `SocketAddr` as data, and OpenDHT is
29,346 lines of C++ behind eight native dependencies. Neither is needed: `trustkit` establishes that an
unregistered machine gets nothing, so the peer set is enumerated in advance and discovering unknown
peers is work whose output the trust model discards. What remains is rendezvous, not routing.

**A relay, until measured.** `natpath` sends symmetric/symmetric pairs straight to one, and that pair is
not rare on mobile carriers — but which pair *your* phone and Ultra form is a measurement nobody has
taken. Stage 5 answers it in one run.

## Stage 5 is done: the socket exists (2026-09-06)

`logscope-fan/src/stunwire.rs` is the request this document said everything was
blocked on. STUN's core and nothing else: a 20-byte Binding request, and
`XOR-MAPPED-ADDRESS` parsed out of the reply. No auth, no TURN, no ICE —
`peerauth` proves identity and the mailbox carries the signalling, so what
remained of STUN was the address echo.

Three decisions worth keeping:

* **The transaction id is checked.** It is the only thing separating our reply
  from a stray datagram on a UDP socket anyone can send to. Believing an
  unmatched reply would let an off-path sender who guesses the port choose what
  this host thinks its own address is — which propagates into `natpath` and out
  into a mailbox that peers read.
* **XOR beats MAPPED, whichever came first.** A NAT that rewrites addresses
  inside packet bodies is exactly what this measures, so a plain
  `MAPPED-ADDRESS` can be a lie told by the thing under test. It is parsed as a
  fallback and marked as the weaker source.
* **One socket, two servers.** The pair must go out from the SAME local port or
  any NAT hands out two mappings and every network looks symmetric.

`cargo run -p logscope-fan --bin nat-probe` takes the measurement: both replies,
the classification, and what `natpath::strategy` would decide against each
possible remote — so one run on the phone plus one on each box gives the verdict
for every link.

`nat-probe --selftest` runs two STUN servers inside the process on loopback and
probes them. That exists because the failure mode of this tool is
indistinguishable from the thing it measures: "no answer" looks the same whether
the servers are down, UDP is blocked, or the binary is broken. Verified here —
both loopback servers saw the same source port, classified endpoint-independent.
The real run is blocked in this sandbox (no outbound UDP), so the measurement
itself is still yours to take.

## The immediate next step

**Stage 4: the announce.** Stage 5 is done, so the remaining blocker is a place to
put the address it learned. `share-kit`'s `Mailbox` already models the `put`/`get`;
what is missing is a backend behind it that both machines can reach, and the
answer to "which backend" depends on the `nat-probe` numbers — a pair that
punches needs only a rendezvous, a pair that cannot needs the relay to be the
same box.

So: run `nat-probe` on the phone and on each compute box, then pick. That is one
command per machine and it settles the largest remaining design question.
