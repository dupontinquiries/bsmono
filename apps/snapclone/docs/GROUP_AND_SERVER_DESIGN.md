# Group Chat, Distributed Media, and the Client/Server Split — Design

Status: **design** (no production code yet beyond the foundational group-crypto
crate). This document settles the hard decisions before implementation because
they interlock — the group key scheme constrains the mailbox layout, which
constrains what the thin LAN server is even allowed to see.

Three intertwined asks:

1. **Group chat** over the existing P2P + DHT substrate.
2. **DHT mailbox** semantics for *multiple* recipients, plus synchronization
   and distribution of group photos/videos across a truly distributed system.
3. A **two-binary split**: the full mobile client, and a thin LAN *server* that
   browsers connect to for limited text/landmark/public-ephemeral-group use.

The connecting constraint is crypto. Everything below is designed so that the
DHT (and the thin server) only ever sees ciphertext + routing metadata, never
group plaintext or the full social graph.

---

## 0. What we already have (the foundation)

- **Mailbox** (`rust/snapclone-mailbox`): per-recipient bucket
  `MailboxId = SHA-256("mailbox:" + sc_id)` (full 256-bit, S-5 fixed); entries
  are `SignedMailboxEntry` (Ed25519 over canonical bytes, S-12); a documented
  gap (S-19) that the entry body still leaks sender/recipient/size/timing and
  must eventually be sealed to the recipient. `seal`/`open` were stubbed for
  exactly this.
- **Crypto** (`rust/snapclone-crypto`): X25519 ECDH, HKDF-SHA256,
  AES-256-GCM, and ML-KEM-768 hybrid (PQ). All the primitives a sender-key
  group scheme needs — but no group layer exists yet.
- **DHT value layer**: sc-ID keyed, opaque encrypted values, ~1h TTL,
  re-announce ~55min, multi-value-per-key. (The micro-DHT reproduces exactly
  these semantics for testing.)
- **Clean layering**: `snapclone_network`, `_core`, `_storage`, `_crypto` are
  separate static libs; *neither network nor core includes anything from
  `ui/`*. The UI is only linked into the final executable. **This is what makes
  the server binary feasible without a refactor.**

---

## 1. Group chat — the crypto model

### 1.1 Why not just "encrypt to each member"

The naive approach (encrypt every message N times, once per member's pubkey,
N DHT entries) is simple and is a fine *fallback for small groups*, but:

- Media (photos/videos) are large; N copies of a 5 MB video is N×5 MB of
  egress and storage. Unacceptable past a handful of members.
- It leaks group size and per-recipient timing to the DHT.

So media uses a **sender-key** scheme (one symmetric content key per message,
distributed once per member), and only the small *key-distribution* messages
are encrypted per-recipient.

### 1.2 Sender-key scheme (Signal-style, adapted)

Each group has:

- A stable **`group_id`** = random 256-bit, never derived from membership (so
  it survives membership changes). Formatted `grp-<64hex>`.
- A **membership epoch** `e` (monotonic counter). Every add/remove bumps `e`.
- Per member per epoch, a **sender key** `SK_{member,e}`: a 32-byte chain key
  the member ratchets forward per message (`CK_{i+1} = HKDF(CK_i, "ratchet")`),
  deriving a per-message key `MK_i = HKDF(CK_i, "msg")`. Forward secrecy within
  an epoch; the chain key never goes backward.

**Sending a message/media:**

1. Sender derives `MK_i` from its current sender chain.
2. Encrypts the content once: `AES-256-GCM(MK_i, content)`. (Media: encrypt the
   blob; store/fetch as today. Text: encrypt inline.)
3. Publishes ONE group mailbox entry (see §2) carrying the ciphertext (text) or
   the content hash + fetch hints (media), the epoch `e`, the sender's
   ratchet index `i`, and an Ed25519 signature.

**Receiving:** a member who holds `SK_{sender,e}` ratchets its local copy of the
sender's chain forward to index `i`, derives `MK_i`, decrypts. No per-recipient
re-encryption of the content.

### 1.3 Distributing sender keys (the only per-recipient step)

When a member first joins an epoch (group creation, or after a membership
change bumps `e`), each existing member sends that member its current
`SK_{self,e}` via a **per-recipient sealed key-distribution message**:

- Encrypted with the existing 1:1 channel crypto we already have
  (X25519 → HKDF → AES-256-GCM, optionally ML-KEM hybrid). This reuses
  `sc_derive_session_key` / `sc_encrypt_aes256gcm` — **no new primitive.**
- Delivered through each member's existing 1:1 mailbox bucket, NOT the group
  bucket. So the DHT sees "some encrypted blob in Alice's mailbox" — it cannot
  tell it's a group key.

Key-distribution messages are small (≈100 bytes), so N-way fan-out is cheap;
it's only the *content* we refuse to send N times.

### 1.4 Membership changes — the hard part

- **Add**: bump epoch `e→e+1`. Every member generates a *fresh* `SK_{self,e+1}`
  and sends it (per-recipient sealed) to all members including the newcomer.
  The newcomer cannot read anything from epoch `e` or earlier (no back-secrecy
  leak). This is "post-compromise security" on add.
- **Remove**: bump epoch `e→e+1`. Every *remaining* member generates a fresh
  `SK_{self,e+1}` and distributes it to remaining members only. The removed
  member never receives the new sender keys, so it cannot read epoch `e+1`+
  messages — **forward secrecy on removal without re-keying past messages.**
- This is the well-understood downside of sender-key vs. full MLS/TreeKEM:
  removal is O(N) messages (each remaining member re-distributes). Acceptable
  for the small-to-medium groups this app targets; MLS's O(log N) tree is
  future work, explicitly out of scope for V1 (noted in §6).

### 1.5 Authentication & trust

- Every group entry and key-distribution message is **Ed25519-signed** by the
  sender (reusing the `SignedMailboxEntry` pattern). Recipients verify against
  the sender's contact key (TOFU, as today).
- Group membership itself is **signed by an admin set**: a group has one or
  more admin pubkeys (the creator at minimum). An "add/remove member" control
  message is only honored if signed by a current admin. This prevents a random
  peer from injecting members. Admin rotation is itself an admin-signed control
  message.
- **What the DHT/server can forge**: nothing it can't already — it can drop or
  delay entries (availability, not confidentiality/integrity), exactly the
  threat model we already accept for 1:1.

---

## 2. DHT mailbox for groups

### 2.1 Group bucket vs. per-recipient bucket

Two bucket types:

- **Per-recipient mailbox** (exists today): `SHA-256("mailbox:" + sc_id)`.
  Used for key-distribution (§1.3) and 1:1.
- **Group feed bucket** (new): `SHA-256("group-feed:" + group_id + ":" + epoch)`.
  All group messages for an epoch land here as multi-values. Every member polls
  this one bucket — no per-recipient fan-out for content. Including the epoch in
  the key means a removed member, who can compute the *next* epoch's key string
  but lacks the sender keys, gets ciphertext it can't read; and we can let old
  epoch buckets expire naturally.

The group bucket is *public* in the sense that anyone who knows `group_id` +
`epoch` can poll it — but `group_id` is a 256-bit secret shared only with
members, so in practice it's unguessable. (Public ephemeral groups, §4.3,
deliberately use a *published* group_id — that's the point of "public".)

### 2.2 The sealed group entry (closing S-19 for groups)

A `GroupMailboxEntry` body is **sealed** so the DHT sees only:

- the routing bucket (public, required for polling),
- an opaque sealed blob,
- an Ed25519 signature + sender pubkey (so forgeries are rejected without
  decryption).

The sealed body (decryptable only by epoch members holding the sender key)
contains: `group_id`, `epoch`, sender ratchet `index`, message kind
(text | media-ref | control), and either inline ciphertext (text) or
`{content_hash, payload_bytes, delivery_hints}` (media). The DHT never sees
sender_id↔recipient mapping or group size. This is the structural fix S-19
asked for, applied to groups from day one rather than retrofitted.

### 2.3 Media synchronization & distribution

Media is **content-addressed**: `content_hash = SHA-256(ciphertext)`. The group
entry references the hash; the bytes live wherever a holder is reachable. A
receiver fetches by trying `delivery_hints` in order (direct WS → relay →
push-when-online), exactly as 1:1 does today.

**Distribution across a truly distributed system** — the part that makes this
robust without a server:

- **Any member who has fetched the blob becomes a source.** After a successful
  fetch + integrity check (`SHA-256(ct) == content_hash`), a member re-announces
  itself as a holder via a small "I have `content_hash`" hint into the group
  feed (or a content-keyed bucket `SHA-256("blob:" + content_hash)`). This turns
  the group into a swarm: late joiners and offline-then-online members fetch
  from whoever's closest/cheapest, not only the original sender.
- **Erasure-friendly chunking (future):** large videos split into fixed chunks,
  each content-addressed, so partial holders can still serve parts. V1 ships
  whole-blob; chunking is noted in §6.
- **TTL + re-announce** mirror the 1:1 mailbox: holder hints expire (~1h) and
  are re-announced while the holder still has the blob and the group is live.
  When the last holder drops, the media ages out — *ephemeral by construction*,
  which is on-brand and is exactly what public ephemeral groups (§4.3) want.

### 2.4 Why this stays "truly distributed"

No node is special. The DHT is the rendezvous (find the bucket), but content
moves peer-to-peer and any member can re-seed. The optional LAN server (§4) is
*just another holder/relay* with a browser front-end — never a required hub and
never trusted with group plaintext.

---

## 3. Crypto: careful-consideration checklist

Because this is the dangerous part, the implementation MUST honor:

1. **Reuse, don't reinvent.** Sender-key ratchet = HKDF chains over existing
   HKDF. Per-recipient seal = existing X25519+AES-GCM (+ optional ML-KEM). No
   new AEAD, no homegrown MAC. (The existing 1:1 DHT-endpoint crypto uses a
   homegrown SHA-256-CTR + HMAC tag — we are deliberately NOT extending that;
   group content uses the vetted AES-256-GCM path in `snapclone-crypto`.)
2. **Domain separation.** Every HKDF `info` label is distinct and versioned:
   `snapclone-v1-group-ratchet`, `-group-msg`, `-group-senderkey-seal`. A label
   collision across contexts is a real vuln; they're centralized as constants.
3. **Nonce discipline.** AES-256-GCM nonces are random-per-message (the crate
   already does `generate_nonce`); a sender key is *never* reused across epochs,
   so the (key, nonce) space can't collide across membership changes.
4. **No key reuse across roles.** A member's Ed25519 identity key signs; X25519
   does ECDH; sender keys are symmetric chains. Distinct key material per role.
5. **Verify-before-trust ordering.** Signature check → epoch/membership check →
   decrypt. Never decrypt attacker-controlled bytes before authenticating.
6. **Removed-member safety is a test, not a hope.** The crate has an explicit
   test that a removed member's retained epoch-`e` keys cannot read epoch-`e+1`
   content.
7. **Forward secrecy within epoch is a test.** Compromise of `CK_i` must not
   reveal `MK_{i-1}` (ratchet is one-way).

These map directly to unit tests in the new crate (§5).

---

## 4. Two-binary split: client + thin LAN server

### 4.1 Feasibility (confirmed)

The codebase already separates `core/network/storage/crypto` libraries from
`ui`, and the lower layers don't include `ui/`. So:

- **`snapclone`** (today): `QApplication` + `snapclone_ui` + everything. The
  full mobile/desktop client — camera, gestures, the works.
- **`snapclone-server`** (new): `QCoreApplication` (no widgets) linking
  `core + network + storage + crypto` *only*. A headless node that also serves
  an HTTP/WebSocket front-end for browsers.

No source needs to move; it's a new `qt_add_executable` + a small
`server_main.cpp`. The expensive/leaf-y deps (camera, GL, face onnx) live in
`ui` and simply aren't linked.

### 4.2 What the server is — and is NOT

The server is a **constrained participant + browser gateway**, deliberately
limited so that running one "down the street" on a basic FCC-compliant modem,
or pairing with an ad-hoc network for security tightening, is low-risk:

- **IS**: a DHT/relay node (another holder/re-seeder per §2.3); a read view of
  **public ephemeral group chats**; a text-only message relay for browser
  clients; a landmark viewer.
- **IS NOT**: trusted with any private group's `group_id` or sender keys; a
  camera; a place where private 1:1 or private-group plaintext lives. It cannot
  decrypt private group content because it never holds the sender keys — by the
  same mechanism that protects the DHT.

### 4.3 Public ephemeral groups (the browser-facing surface)

A *public ephemeral group* is the one group type the thin server can fully
participate in, precisely because it's designed to be public:

- Its `group_id` is **published** (e.g. via a landmark or a known feed), so
  anyone — including a browser via the server — can poll the feed bucket.
- Content key distribution is **open**: the epoch sender keys for a public
  group are themselves posted into the public feed (or the group key is simply
  derived from the published `group_id`), so "encryption" here is integrity +
  uniformity of code path, NOT confidentiality. Messages are still
  Ed25519-signed so the server/browser can show *who* said something and reject
  forgeries, but anyone can read — that's the definition of public.
- **Ephemeral**: TTL-bound, no long-term storage on the server (it holds only
  the live window, re-announced while alive, gone when it ages out). A browser
  viewing it is reading a transient window, not an archive.
- Browser interactions are limited to **text send + view** and **landmark
  view** — no camera, matching the ask. The server signs browser-originated
  text with a *server-scoped* key clearly distinct from any real user identity,
  or proxies a lightweight browser-session identity, so a browser post is never
  mistaken for a full-client member.

### 4.4 Browser transport

Browsers speak WebSocket to the server (the app already has a
`WebSocketTransport`); the server bridges WS↔the public group feed. Static
assets (a minimal HTML/JS page) are served over plain HTTP. No accounts, no
camera, no private data path. This keeps the attack surface of the
internet-facing binary tiny.

### 4.5 Security posture for "down the street" hosting

- The server binary links the *minimum* (no camera/GL/onnx → fewer CVEs).
- It only ever handles public/ephemeral data + ciphertext it can't read.
- Pairs naturally with an ad-hoc/isolated network: even a fully-compromised
  server leaks only what's already public, and can at worst censor/delay
  (availability), never decrypt private groups.

---

## 5. Implementation plan (incremental, testable-first)

Ordered so each step is verifiable in the Rust-buildable environment before the
untestable C++/Qt integration:

1. **`rust/snapclone-group` crate (FIRST, this PR):** pure group crypto +
   data model, fully unit-tested. Sender-key ratchet, epoch handling,
   membership add/remove key flows, sealed group entry, the §3 safety tests.
   No FFI, no Qt — just the math, proven correct.
2. **Group mailbox buckets** in `snapclone-mailbox`: the group-feed `MailboxId`
   variant + sealed `GroupMailboxEntry`. Tested against the micro-DHT.
3. **FFI surface** (`sc_group_*`) once the Rust layer is proven, mirroring the
   existing `sc_*` conventions.
4. **C++ group session + UI** (untestable here → last, behind a flag).
5. **`snapclone-server` binary**: headless node + WS/HTTP browser gateway,
   public-ephemeral-group only.

---

## 6. Explicitly out of scope for V1 (filed, not forgotten)

- Full **MLS/TreeKEM** (O(log N) membership changes). Sender-key's O(N) removal
  is acceptable for target group sizes; revisit if large groups become a goal.
- **Chunked/erasure-coded** media for partial-holder swarming (§2.3) — V1 ships
  whole-blob content addressing.
- **Metadata-private membership** (hiding group size even from members) — the
  sealed entry hides it from the DHT/server; members necessarily know.
- Browser **write beyond text** (no media upload from browsers) — intentional.
