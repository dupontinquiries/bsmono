# Networking & crypto — drawing the snapcore / crypto-kit / share-kit lines

Where to split the networking/DHT/crypto so snapclone, unendlich, voiceterm and
logscope can share it without fighting. Grounded in a survey of the actual code
(the author has a high-level concept; this records what's really there). Feeds
the kits plan ([[KITS_AND_BOUNDARIES]]) step 3.

## What actually exists

**snapclone — ~42 crates, dense and well-layered already:**
- *Transport / DHT:* `dht` (real OpenDHT/Jami, Kademlia/UDP, bootstrap.jami.net),
  `microdht` (a **local stand-in** reproducing the app-visible DHT semantics —
  sc-ID keys, opaque encrypted values, TTL, multi-value, re-announce — for
  offline/deterministic tests), `gossip`, `nat`, `upnp`, `relay`, `peer`,
  `proximity`, `piggyback`, `net-intel`, `network-diag`, `firewall`.
- *Wire / proto:* `wire`, `wirecodec`, `proto`, `databus` (the FFI bus),
  `session`.
- *Identity / crypto:* `identity`; **`crypto`** = X25519 + Ed25519 + **hybrid
  ML-KEM-768 (FIPS 203 post-quantum)**, HKDF/SHA-2 — best-of-breed; **`group`** =
  one-way sender-key ratchet + per-recipient seal + epoch re-key + zeroizing drop.
- *Sharing:* **`mailbox`** (`MailboxId`, `MailboxEntry`/`SignedMailboxEntry`,
  `EncryptedEnvelope` fixed-size envelopes, a request state machine, a store),
  `group`, `mediashare`, `vcard`, `linkpreview`.

**unendlich:** no networking/crypto stack (single-player engine) → a future
*consumer*, not a source.

**opendht (vendored C++):** GnuTLS + Nettle + argon2; `crypto.h` / `securedht.h`
(certificate identity + encrypted DHT values). Lives at the DHT-transport
boundary; a later oxidation candidate.

## Best-of-best crypto verdict

- **App crypto → base `crypto-kit` on `snapclone-crypto`** (Rust, hybrid PQ).
  Not unendlich (none), not opendht's GnuTLS (C, classical-only).
- **Messaging crypto → `snapclone-group`'s ratchet/seal** into crypto-kit.
- **DHT value/identity crypto → opendht's SecureDht** for now (C++ boundary),
  oxidize later.

## The three lines

1. **`snapcore` — the shared NODE (DHT + transport + lifecycle).**
   - The **DHT interface** the `dht`/`microdht` split already implies (keyed
     put/get, TTL, multi-value, re-announce, opaque encrypted values), with two
     backends: `opendht-real` and `micro-local` (the latter = the net-sim
     harness, scratchpad E1).
   - Transport: `nat`, `upnp`, `relay`, `gossip`, `peer`, `proximity`,
     `piggyback`.
   - **NEW (doesn't exist yet) — the node ownership/lifecycle layer.** This is
     the piece the flags below need: a refcounted shared node handle with
     sleep/wake/idle, an optional parallel-node pool, and port arbitration.
2. **`crypto-kit` — best-of-best primitives.** `snapclone-crypto`
   (X25519/Ed25519/ML-KEM-768 hybrid, HKDF) + `snapclone-group` ratchet/seal.
   Pure Rust, cross-clean. Consumed by snapcore, share-kit, and apps.
3. **`share-kit` — the per-app mailbox over snapcore + crypto-kit.** Generalize
   `snapclone-mailbox`: an app-scoped `MailboxId` namespace, app-defined entry
   types, fixed-size encrypted envelopes, the request state machine, a store —
   over the **shared-DHT-slot** pattern ("share the slot, not the key", with
   per-sender ratchet) from `docs/group-mailbox.md`. Each app instantiates its
   own mailbox on one shared node.

## The flags to juggle (captured)

- **Node ownership:** one shared node (fewer nodes, no port-fighting, cleaner)
  vs a parallel-node pool (stability/staleness redundancy) → a policy in
  snapcore's node manager.
- **Lifecycle:** node asleep when idle / in-use by snapclone / woken on demand →
  refcounted handle; apps must tolerate a not-always-on node.
- **Per-app isolation:** each app its own `MailboxId` namespace + entry type over
  the shared node (snapclone DM/group, voiceterm zettelkasten share, logscope
  multi-AI conversation).
- **Offline / busy:** the `micro-local` backend covers offline; share-kit
  queues/retries when the node is asleep or busy.

## Where to draw THE line (headline)

`snapcore` owns the **node** (DHT + transport + lifecycle/ownership). `crypto-kit`
owns the **primitives**. `share-kit` owns the **mailbox** (per-app, over snapcore,
sealed with crypto-kit). snapclone keeps its media/AV/UI and becomes the first
consumer of all three; unendlich/voiceterm/logscope follow as they add networking.

## Sequencing (refines step 3)

1. Extract the **DHT interface trait** (the semantics `dht`/`microdht` already
   share) + the two backends.
2. Add the **node ownership/lifecycle layer** (the missing piece) with the flags.
3. Lift **crypto-kit** (`snapclone-crypto` + `group` ratchet) — pure Rust, easy,
   the lowest-risk of the three.
4. Lift **share-kit** (generalize `snapclone-mailbox`) over snapcore + crypto-kit.
5. Wire snapclone onto the kits (reference consumer); define unendlich's `dht`
   feature for the integrated/lean variant.

Each step needs real work and most needs the C++/FFI build on hardware to
validate — this doc draws the lines; it doesn't move code yet. **crypto-kit (3)
is the natural first lift** — pure Rust, cross-clean, no FFI.
