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
| 5 | learn our own public address | built (`reflexive`) | no socket; needs one STUN-shaped request |
| 6 | decide punch vs relay | built (`natpath`) | stage 5 supplies the observations |
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

## The immediate next step

**Stages 4 and 5 share one dependency: a UDP socket.** Stage 5 needs a single request to a host that
echoes the source address it saw; stage 4 needs a way to write an address somewhere the peer can read
it, which `share-kit`'s `Mailbox` already models over a `put`/`get` backend.

Do stage 5 first. It is one request, it classifies the NAT (`reflexive` needs two hosts to say
anything), and its answer decides whether stage 6 ever needs a relay — which is currently the largest
unknown in the whole flow and is a measurement, not a design question.
