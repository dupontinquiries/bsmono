# Phone → compute cluster: the flow, and what each stage is waiting on

Goal: a phone away from home reaching an M2 Ultra and an M4 mini. Every decision layer is built and
tested; the flow does not run. This is where each stage stops.

## The stages

| # | stage | state | blocked on |
|---|---|---|---|
| 1 | this machine has an identity | **placeholder** | `crypto-kit` Ed25519 not wired; the id is identical on every install |
| 2 | operator registers a peer by id | built (`machineid`, UI) | stage 1 — a shared placeholder id registers a twin |
| 3 | registration binds a key | built (`trustkit::register_keyed`) | stage 1 supplies the key |
| 4 | find where the peer is now | built (`rendezvous`) | no announce; nothing writes an address anywhere |
| 5 | learn our own public address | built (`reflexive`) | no socket; needs one STUN-shaped request |
| 6 | decide punch vs relay | built (`natpath`) | stage 5 supplies the observations |
| 7 | prove the peer is that peer | built (`peerauth`) | stage 1 supplies the keypair |
| 8 | place duties by reach | built (`workerkit`) | everything above |

**Stage 1 gates five of the other seven.** It is also the smallest: generate an Ed25519 keypair, persist
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

## The immediate next step

Add `crypto-kit` to `logscope-fan`, generate and persist an identity, and derive `machineid` from it.
Until then the UI says so, in amber, rather than showing a plausible id for an identity that does not
exist — the same failure as "✓ verified" for a run with no verifier.
