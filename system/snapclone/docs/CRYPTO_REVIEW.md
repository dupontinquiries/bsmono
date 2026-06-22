# SnapClone — Crypto code review

**Date:** 2026-05-15 (V32.33 session)
**Reviewer:** LLM-assisted code read (Claude Opus 4.7), offline, no execution.
**Scope:** `rust/snapclone-crypto/`, `rust/snapclone-identity/`,
`rust/snapclone-mailbox/`, `src/crypto/`, the SHAK handshake in
`src/core/AppController_connect.cpp`, the wire-format and routing in
`src/core/SnapRouter.cpp`. Light look at `rust/snapclone-peer/session.rs`
and the protocol-layer types in `rust/snapclone-proto/`.

## What this document IS

A code-review-style audit looking for common cryptographic mistakes
and threat-model gaps. Findings are listed by severity with file:line
references and a short "why it matters." Each finding is logged into
`docs/TODO.md` under a new `### V32.33 — Security review findings`
subsection for tracking.

## What this document IS NOT

- A real cryptographic security audit. That requires a firm like
  Trail of Bits, Cure53, or NCC Group spending 2-4 weeks with
  running code, test vectors, fuzzers, and proofs.
- A clean bill of health. The absence of a finding here does not
  mean the area is correct.
- A side-channel analysis. Timing leaks would require running
  binaries with measurement tools.
- A protocol-correctness proof. ProVerif / Tamarin or hand-proofs
  would be needed for that.

## What I could not check

- **Dependency CVEs.** `cargo audit` not available in this environment.
  Crates pulled in by `snapclone-crypto`: `aes-gcm 0.10`, `x25519-dalek 2`,
  `ed25519-dalek 2`, `hkdf 0.12`, `sha2 0.10`, `rand_core 0.6`,
  `getrandom 0.2`, `zeroize 1`. Versions are recent at time of writing;
  rerun `cargo audit` before any production cut.
- **Constant-time behavior.** The Rust crates (`aes-gcm`, dalek)
  document constant-time implementations. I trust their authors; I
  haven't measured.
- **Embedded test vectors.** No KAT (Known Answer Tests) for the
  primitives in `rust/snapclone-crypto/src/lib.rs`. The standard
  vectors from NIST AES-GCM, X25519, Ed25519 RFCs would be one-time
  effort and worth having.
- **OS-level key file protections** beyond the 0600 chmod that exists.
  Cross-process memory inspection, swap exposure, backup-tool inclusion
  are platform issues outside the code.

---

# Executive summary — three things to fix first

The detailed findings below run to twenty entries across four severity
bands. That's useful for tracking but bad for prioritisation. If you
have one week of attention to spend, spend it on these three. They
share a common theme — identity authenticity over time — and the
fixes compose into a coherent defence layer rather than landing as
isolated patches.

## 1. The persistent-MITM path (S-2 + S-20 combined)

This is the highest-impact problem in the audit. Neither finding
alone is the full attack; together they make persistent invisible
man-in-the-middle the **default behaviour** of the protocol given an
active on-path attacker.

**The attack walkthrough.** Mallory sits on the network path between
Bob and Alice. When Bob's app receives a SHAK frame claiming to be
from Alice, the SHAK contains a pubkey but **no signature**. Bob's
app has no way to test whether the entity sending the SHAK actually
holds the private key matching the advertised pubkey. So Mallory
sends a SHAK claiming to be Alice, advertising Mallory's own pubkey.
Bob derives a session key from Mallory's pubkey, encrypts to Mallory
believing it's Alice, and Mallory decrypts everything.

But it gets worse. The `peer_public_keys` SQLite table uses
`ON CONFLICT(peer_id) DO UPDATE SET public_key = excluded.public_key`.
Every SHAK silently overwrites whatever was previously stored.
**There is no Trust-On-First-Use comparison.** So even after Bob has
already established a real session with the real Alice — pubkey
stored, session keys working — Mallory can intercept a *later*
connection, send a SHAK with Mallory's pubkey claiming to be Alice,
and Bob's storage layer silently replaces Alice's real pubkey with
Mallory's. From that moment on, Bob encrypts to Mallory. Bob has
no signal that anything changed. The next snap from real Alice
will fail MAC verification and be silently dropped — looking like
a delivery glitch rather than active interception.

**Why this is the top priority.** Every other defensive layer in
this codebase (AEAD, ECDH, dedup, contact verification) assumes the
pubkey associated with a peer ID is correct. If an attacker can
silently swap pubkeys at will, those layers protect nothing — they
protect whatever-Mallory-is-claiming-to-be, not the real Alice.

**The fix is two coordinated changes:**

- **Sign the SHAK payload.** Add an Ed25519 signature over
  `(magic || idLen || id || pubKey || nonce_or_timestamp)`. The
  receiver verifies the signature against the advertised pubkey
  before accepting it. This proves the sender holds the matching
  private key — no one can advertise a pubkey they don't own.
  Files: `src/core/AppController_connect.cpp:14-156`,
  `rust/snapclone-crypto/src/lib.rs` (signing already exposed).

- **Trust-On-First-Use storage.** Before saving a pubkey, compare
  against the previously-stored one. If different: log a warning,
  do NOT auto-save, surface to UI as a key-change event that
  requires explicit user confirmation. Legitimate re-installs are
  rare and user-driven; silent swaps are attacks. Make the user
  see the difference.
  File: `src/storage/StorageManager.cpp:1337-1352`.

Either change alone is incomplete: signed SHAK without TOFU still
lets an attacker swap pubkeys whenever they want (the new sig
verifies against the new pubkey just fine — sig only proves
self-consistency, not "this is the same key as before"). TOFU
without signed SHAK still lets an attacker forge identities on first
contact. Ship them together.

Effort: maybe a week of work. ~150 lines of Rust + ~80 lines of C++
+ schema migration for the key-change history. By far the highest
security ROI in the codebase.

## 2. BLE highways as proximity-witness layer (defence-in-depth)

The S-2 + S-20 fix above is necessary but not sufficient. It catches
active MITM at the protocol level — if Mallory tries to swap pubkeys,
the user sees a warning. But users dismiss warnings. And users have
no way to verify pubkey fingerprints out of band unless they're
sitting in the same room as the other person.

This is where BLE highways earn their keep. Not as a primary
identity layer — as a slow, asynchronous, low-battery accumulator of
evidence that grounds identity claims in the physical world over
time.

**The model.** Devices in physical proximity exchange signed
witness records: "On day D, my device with key K saw sc-id S
advertising pubkey P." Records persist locally with TTL. Devices
forward records they've received. Over weeks and months, real
contacts accumulate consistent witness records — many independent
devices have seen the same pubkey for the same sc-id over a long
time horizon. Forged identities don't accumulate consistency
because the attacker can't be physically near enough people to
generate diverse witness records.

**Two reframings from earlier conversation, both load-bearing.**

First: this is **not a binary verification system**. It produces a
confidence scale. The UI should communicate evidence, not pass/fail.
Honest framing: "Bob's identity has been consistent across N devices
for D months." NOT "75% confident this is Bob" — that overstates
what the system actually knows and invites users to plan around a
precision that isn't real. Confidence categories (Minimal / Some /
Strong / Verified-by-you) communicate partial information without
implying false precision.

Second: this is **not continuous radio operation**. The duty cycle
should be aggressive low: never scan in background, only when
foregrounded AND battery healthy AND not in low-power mode, ~10% duty
cycle even then, piggyback on already-awake radio events whenever
possible. The trade-off is slower confidence accumulation
(weeks-to-months rather than days), which is fine — the synchronous
defences (S-2 + S-20 fix, multi-path verification at first contact)
do the urgent work. The witness layer is the long-tail reinforcement.

**Why this is on the priority list despite being asynchronous.** The
S-2 + S-20 fix above produces a key-change warning users will see
maybe once per legitimate re-install per contact. They'll learn to
click through. The witness layer is what makes the warning
*informative*: a key change with zero witness history is a probable
attack; a key change with witness records dating from before the
change is probably a legitimate re-install of someone known. The
witness data turns the warning from "make a decision with no
information" into "make a decision with months of evidence."

**Where it composes with existing work.** The V32.33 TODO already
lists "BLE highways design doc" as a deferred item with rotating
TOTP-style identifiers for presence detection. That radio
infrastructure is the same — witness records are a richer payload
on top of the same BLE advertising/scanning machinery. The
`DHT_DISCOVERABILITY.md` design lists "in-person via BLE
sidechannel + verification code" as bootstrap path 1; the witness
layer is the runtime-continuous version of that bootstrap.

**Not in scope for this audit, deferred to its own design doc.** The
real design work — witness record format, exchange protocol, Sybil
resistance, privacy of carrying records about contacts, recovery
when witness keys are lost — needs its own ~250-line treatment
similar to `DHT_DISCOVERABILITY.md`. Filing as
`docs/PROXIMITY_WITNESSES.md` (future). What belongs in THIS document
is the recognition that the witness layer is the natural complement
to S-2 + S-20, not an unrelated feature.

## 3. Network timing is too precise on the DHT

The `MailboxEntry` published to the DHT contains two `DateTime<Utc>`
fields: `created_at` and `expires_at`. These are millisecond-precision
timestamps, plaintext, sitting on a public store. Anyone scraping
the DHT learns exactly when every snap was sent, to millisecond.

This is a subset of S-19 (mailbox plaintext metadata) but deserves
separate treatment because the fix is structural rather than
"encrypt the whole entry":

**The asymmetric leak problem.** The recipient has a session key with
the sender. The recipient can derive things from data the sender
encrypts to them. But the DHT entry is read by *everyone*, not just
the recipient. Anything the DHT entry conveys, the entire internet
knows. Anything the entry doesn't convey, no one knows — including
the recipient, who currently has no privileged channel for receiving
finer information than the DHT-stored timestamp.

**The fix shape.** Store coarse on the DHT, fine in the encrypted
payload (or in a recipient-derivable field). Concretely:

- Round `created_at` in the DHT entry to 5-minute buckets. Anyone
  reading the DHT learns the 5-minute window. That's enough for
  TTL bookkeeping and dedup; it's not precise enough to
  fingerprint per-message patterns.
- Carry the fine-grained timestamp (~15s resolution) inside the
  AEAD-encrypted snap payload. Recipient decrypts and recovers
  exact time. Observers can't.

Three viable encodings discussed earlier, in increasing strength:

1. **Encrypt the fine offset** as a small AEAD-sealed field next
   to the coarse bucket. ~28 bytes overhead. Simple.

2. **Truncated MAC over time.** Coarse bucket plaintext + small
   HMAC tag derived from session key over the exact timestamp.
   Recipient brute-forces the ~20 sub-buckets within the 5-minute
   window to find the matching MAC. Looks like padding to outsiders.
   No "ciphertext-shaped" tell.

3. **Coarse-only, no recovery channel.** Recipient gets the same
   5-minute precision as everyone else. Simplest, but loses the
   recipient-only precision the design wants.

If the precision matters to the recipient (replay-window rejection,
exact ordering, latency measurement), use (1) or (2). If it doesn't,
use (3) and find another mechanism for whatever needs the precision —
likely the encrypted snap payload itself, which already carries an
exact timestamp the recipient can read on decrypt.

**Why this is priority 3 not priority 1.** The persistent-MITM path
(priority 1) lets attackers read snap content. The timing leak
(priority 3) lets observers fingerprint conversation patterns. Both
are real, but content compromise is strictly more severe than
metadata compromise. Fix the high-severity issues first; come back
to timing precision when the protocol layer is solid.

**Why this is on the priority list at all.** Because (a) it composes
with the larger "encrypt the mailbox entry" work from S-19, (b) the
fix is small and well-defined, and (c) the asymmetric-leak framing
generalises beyond timing — the same pattern (coarse plaintext,
fine encrypted) applies to other metadata fields like
`payload_bytes` (round to 4KB blocks) and `sender_id` (replaceable
with a per-contact-pair pseudonym known only to that pair).

---

## Findings

### S-1. (serious) Default build is unauthenticated dev-only crypto

**Location:** `src/core/AppController.cpp:55-61`

```cpp
#ifdef SNAPCLONE_RUST_CRYPTO
    d->crypto = std::make_unique<Crypto::RustCryptoService>();
#else
    d->crypto = std::make_unique<Crypto::CryptoService>();  // Qt placeholder
#endif
```

`CryptoService` is a placeholder explicitly documented as "NOT
cryptographically secure" (lines 19, 26, 169 of CryptoService.cpp).
Its session key derivation is `SHA-256(sort(pubA, pubB) ++ "label")`
— **anyone who knows both public keys can compute the session key.**
Since both public keys are exchanged in cleartext during SHAK, ANYONE
ON THE WIRE who sees the SHAK can compute the session key and decrypt
the entire conversation. Its "encryption" is also an HMAC-keystream
XOR with separate HMAC — not authenticated together unless wrapped
carefully (it is, but the construction is amateur).

The default build (no `-DSNAPCLONE_RUST_CRYPTO=ON`) uses this. Anyone
running a quick `cmake -B build && cmake --build build` with no special
flags ships with the placeholder and gets log line "Using Qt HMAC-SHA256
placeholder crypto backend." Easy to miss in noise.

**Why it matters:** A build accidentally shipped to users without the
flag would have effectively no transport encryption. The log message is
the only indicator. Linux distribution packagers downstream might miss
this.

**Suggested fix:** Either
- (a) Make Rust crypto the default (`#ifndef SNAPCLONE_RUST_CRYPTO=OFF`
  and require explicit opt-out for testing), OR
- (b) Refuse to start when the placeholder is selected unless a
  `SNAPCLONE_ALLOW_PLACEHOLDER_CRYPTO=1` env is set, OR
- (c) At minimum, surface this prominently in the UI (the Settings
  page already has a "Rebuild with -DSNAPCLONE_RUST_CRYPTO=ON" hint
  in the crypto section; promote to a startup banner / warning).

Of these, (a) is cleanest and matches user expectations.

---

### S-2. (serious) SHAK handshake has no authentication / proof-of-possession

**Location:** `src/core/AppController_connect.cpp:14-156`

The SHAK wire format is:
```
[magic:4][idLen:2][id:N][pubKey:32]
```

There is no signature, MAC, or challenge-response. **Any actor who
knows or guesses or replays a user's display name + public key can
present a SHAK claiming to be that user.** The receiver derives a
session key (peer.pub × self.priv), but the spoofer cannot derive
the matching key (they don't have peer.priv), so they can't read
replies — but they CAN:

1. **Hijack the inbound transport mapping.** The handler at line 91
   does `d->transportToRealId[sender.value] = realId;` and promotes
   the inbound session to the spoofed real-ID. The real user can now
   no longer connect inbound under that ID until the spoof socket
   closes (one session per real-ID).
2. **Drain the responder's outbound SHAK budget.** Line 157
   `sendShak(sender)` runs once per transport ID. Spoofers attacking
   from many addresses consume the responder's resources.
3. **Force a contact entry to be saved with attacker-chosen
   `displayName`.** Lines 117-135 save a Contact with the SHAK-supplied
   display string. Future UI shows the attacker's chosen name.
4. **Worse than (3): contact saved as `ContactState::Known`** (line 121).
   In the application's contact model, Known typically means
   "verified" — by being added directly through SHAK, an attacker
   can promote any spoofed identity straight to Known.

**Why it matters:** The threat is not eavesdropping (ECDH protects
that); it's identity confusion, denial of service, and bypassing
contact-verification gates.

**Suggested fix:** Add a signature to SHAK that proves possession of
the private key matching the public key being advertised. Concretely:

- Include a 64-byte Ed25519 signature of `(magic || idLen || id ||
  pubKey || responder_nonce_or_timestamp)`.
- Receiver verifies signature against the advertised key BEFORE
  registering the peer pub key or saving a contact.
- Each peer has BOTH an X25519 keypair (for ECDH) AND an Ed25519
  keypair (for signing). The crypto crate already exposes both
  primitives — they just aren't wired into SHAK.

The receiver's challenge would be more secure (true challenge-response),
but a timestamp+window also works and avoids an extra round-trip.

---

### S-3. (serious) AEAD ciphertext is not bound to sender/recipient/sequence

**Location:** `rust/snapclone-crypto/src/lib.rs:184`,
`src/core/SnapRouter.cpp:470-490` + `683-733`

`Aes256Gcm::encrypt(&nonce, pt_slice)` is called without associated
data (AAD). The AEAD only binds nonce + ciphertext. Wire format
metadata — sender id, recipient id, sequence number — sits in the
unencrypted plaintext frame but is NOT bound to the AEAD.

Consequence: an attacker who can replay an old ciphertext at the
transport layer (e.g. injecting an old encrypted frame into a current
session) will have the AEAD MAC verify. The application-level seq/ack
fields (in plaintext after decryption) are visible to dedupe logic
(`isRecentInbound`, `m_recentInbound`) but that's a UI-emission dedup,
not a security check.

More subtle: since session keys are symmetric and the AEAD doesn't
know who's encrypting, **any message Alice sent to Bob can in principle
be decrypted by Alice herself if a later attacker re-injects it under
sender="Bob" routing**. The routing assigns sender via the transport
socket's identity (which is bound by SHAK), so this requires an
attacker who can also forge the transport. Defense-in-depth says
the AEAD should rule this out itself.

**Why it matters:**
- Replay protection currently relies on `m_recentInbound` (in-memory,
  bounded, cleared on restart). An attacker who waits for that cache
  to roll over can replay old snaps.
- A snap stored by SnapRouter (line 859) BEFORE the dedup check (line
  869) — so the storage layer accepts replays, only the UI emission is
  suppressed. The recipient sees no notification but the snap is in
  their DB. If `m_storage->saveSnap` triggers any side effects (it
  does — through ContactStore lastSeen updates), replays still
  influence state.

**Suggested fix:**
- Encrypt with AAD = `(sender_sc_id || recipient_sc_id || seq || version)`.
- Decrypt with the SAME AAD; if the sender's view and receiver's view
  differ, MAC fails and the message is dropped.
- Bonus: replays under a different sender ID would also fail.
- Persist the highest-received seq per sender across restarts.
  Reject `seq <= last_seen_seq` BEFORE storage write.

---

### S-4. (serious) sc-ID is 32 bits — collision space is small

**Location:** `rust/snapclone-identity/src/lib.rs:11-13`

```rust
pub fn generate_sc_id() -> String {
    let random: u32 = rand::thread_rng().gen();
    format!("sc-{:08x}", random)
}
```

32-bit ID space = 2^32 = 4.3 billion. Birthday collision (any two
users colliding) at ~65k users. **Adversarial collision** (an attacker
wants to forge an sc-ID matching a SPECIFIC target user) requires
2^32 trial generations + hashes — a single laptop will produce a
collision in under an hour.

The sc-ID is used:
- As a peer identifier in network routing (`UserId.value`).
- As the seed for `MailboxId::for_recipient` (then SHA-256-truncated
  to 64-bit DHT key — see S-5).
- As a stable identity across restarts (persisted, used by contact DB).

Adversarial collision means an attacker can deliberately generate
an sc-ID that matches a victim's, then announce themselves at that
sc-ID in the DHT — the victim's mailbox in the DHT becomes
attacker-controlled (writes from anyone arrive at the attacker too).

**Why it matters:** This is the foundational identity. Once
collisions are possible, the social-graph and DHT discoverability
features become attacker-controllable. Snaps could be routed to
attackers who lottery-won the right sc-ID.

**Suggested fix:** Move to 128-bit minimum. Use either:
- 16 bytes random ⇒ `sc-` + 32 hex chars. Display can still truncate
  visually but internally use full.
- Or: derive sc-ID from the X25519 public key (e.g. truncated SHA-256
  of the pubkey). This binds sc-ID to the keypair so an attacker
  who forges sc-ID needs to also break the hash (or just generate
  many keypairs until one hashes to the target prefix — same birthday
  problem unless the hash output is long enough).

Migration path: existing 32-bit sc-IDs need a strategy. Probably the
new format is a different prefix (e.g. `sc2-...`) so old and new
coexist, with a forced regeneration on next launch flagged as a
breaking change.

---

### S-5. (serious) MailboxId is 64 bits — collisions feasible

**Location:** `rust/snapclone-mailbox/src/entry.rs:22-26`

```rust
pub fn for_recipient(sc_id: &str) -> Self {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(format!("mailbox:{}", sc_id).as_bytes());
    Self(hex::encode(&hash[..8])) // 16 hex chars = 64-bit key space
}
```

64-bit truncation. Birthday collision at sqrt(2^64) ≈ 4.3 billion
users; targeted second-preimage at 2^64 hashes (effectively infeasible
for a single laptop but well within reach of a determined adversary
with cloud resources — Bitcoin miners produce 2^64 hashes per minute).

Combined with S-4: an attacker who generates many sc-IDs can find
one whose `mailbox:sc-XXXXXXXX` hash collides with the target's
mailbox ID. They then write to the same DHT key as the victim's
mailbox. The DHT presumably doesn't enforce single-writer (most
opendht-style DHTs don't), so the attacker can pollute the mailbox.

**Why it matters:** The DHT is the discoverability and mailbox layer.
Polluting it with collision entries lets an attacker plant fake
"someone sent you a snap" entries that, when fetched, fail (because
the attacker doesn't have the real session key) but cost the recipient
time + radio energy investigating.

**Suggested fix:** Use the full 32-byte SHA-256 as the DHT key (no
truncation). Storage size cost is one extra DHT entry-key allocation,
negligible.

---

### S-6. (high) Envelope padding bytes are not random

**Location:** `rust/snapclone-mailbox/src/envelope.rs:36-44`

```rust
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
let mut h = DefaultHasher::new();
std::time::SystemTime::now().hash(&mut h);
let seed = h.finish();
for (i, b) in buf[4 + payload.len()..].iter_mut().enumerate() {
    *b = ((seed >> (i % 8)) as u8).wrapping_add(i as u8);
}
```

The padding is documented as "Fill padding with random bytes (prevents
compression attacks)" but it isn't random. The seed is `SystemTime::now()`
hashed with `DefaultHasher` (a non-cryptographic hash, SipHash internally
with random keys *per process* — so within a session it's deterministic
from the timestamp). The padding pattern is then `(seed >> (i % 8)) +
i` — a known-structure ramp from a known seed.

**Why it matters:**
- If the envelope is later compressed (e.g. by a transport layer below),
  the structured padding interacts with the real-payload prefix in
  predictable ways. An attacker who knows the timestamp (which is
  often leaked at the network layer) can predict the exact padding
  bytes and use a CRIME-style chosen-plaintext attack to recover
  parts of the real payload.
- More importantly, the padding currently happens BEFORE encryption
  per `fixed_size_envelope`'s contract. AES-GCM is a stream cipher
  in CTR mode internally — encrypting predictable-pattern padding
  is not a confidentiality problem per se (the ciphertext is still
  random-looking) but it removes one defense layer.
- AEAD integrity is unaffected. Confidentiality is mostly fine.

**Why I still rate it high:** the COMMENT says the goal is preventing
compression attacks. The implementation doesn't achieve that goal.
That's a falsely-named defense — worse than a missing defense because
it tells future code reviewers "this is handled."

**Suggested fix:** Use `getrandom::getrandom(&mut buf[4 + payload.len()..])`
or any CSPRNG. Already pulled in via `rand_core`/`getrandom` in
neighbouring crates.

---

### S-7. (medium) Private key bytes live in non-zeroizing C++ heap

**Location:** `src/crypto/RustCryptoService.cpp:77-82, 149`

```cpp
QByteArray RustCryptoService::loadPrivKey(const Core::UserId &userId) const
{
    QFile f(keyPath(userId) + QStringLiteral(".priv"));
    if (!f.open(QIODevice::ReadOnly)) return {};
    return QByteArray::fromHex(f.readAll().trimmed());
}
```

The returned `QByteArray` holds 32 bytes of X25519 private key
material on the C++ heap. `QByteArray` is NOT zeroized on destruction;
when the QByteArray goes out of scope and its allocation is freed,
the bytes remain in the freed-but-not-yet-reused heap pages.

The Rust side does zeroize (StaticSecret, Zeroizing wrappers), but
the C++ wrapper that holds the key for the duration of the call does
not.

Consequence: A core dump, a crash with un-cleaned memory, or a
process inspector with permission to read heap pages can recover
private key material that has been "deleted."

**Why it matters:** Defense-in-depth. The threat is post-compromise
forensics or an attacker who briefly gains read-only memory access
(via a separate vulnerability). The risk is small for a consumer
app on Linux desktop with proper user-isolation, but mobile
operating systems with shared sandboxes and forensic tools mean
this is worth fixing.

**Suggested fix:** Wrap the private-key QByteArray load in a
`Zeroizing<QByteArray>` equivalent — easiest is a tiny RAII helper:

```cpp
class ZeroizingKeyBuffer {
public:
    ZeroizingKeyBuffer(QByteArray data) : m_data(std::move(data)) {}
    ~ZeroizingKeyBuffer() {
        // Overwrite before release. volatile prevents compiler elision.
        volatile char *p = m_data.data();
        for (qsizetype i = 0; i < m_data.size(); ++i) p[i] = 0;
    }
    const uint8_t *constData() const { return reinterpret_cast<const uint8_t*>(m_data.constData()); }
    int size() const { return m_data.size(); }
private:
    QByteArray m_data;
};
```

Use it in `loadPrivKey`'s return type and in `registerPeerPublicKey`'s
`ourPriv` local. Probably 30 lines of C++ to do this site by site.

---

### S-8. (medium) Session keys live in QHash with no zeroize

**Location:** `src/crypto/RustCryptoService.cpp:167` and equivalent
in `CryptoService.cpp`

```cpp
m_sessionKeys[peerId.value] = sessionKey;
```

`m_sessionKeys` is `QHash<QString, QByteArray>` (inferred). Session
keys (32 bytes of AES-256 key material) live in heap-allocated
QByteArrays for the lifetime of the RustCryptoService object. On
destruction, none of this is zeroized.

For a long-lived process (the SnapClone app may run for hours), every
session key for every contact ever talked to is held in memory in
clear form. A process inspector observing the heap can extract every
session key.

**Why it matters:** Same threat model as S-7, larger surface (every
contact, not just self). Forward secrecy is also degraded: an
attacker who compromises memory at time T can decrypt everything
sent under those keys forever (no key rotation exists; see S-9).

**Suggested fix:**
- Wrap m_sessionKeys values in the ZeroizingKeyBuffer above.
- AND consider key rotation per S-9 — even without zeroization, an
  ephemeral session key derived per snap (or per N snaps) bounds
  the exposure.

---

### S-9. (medium) No session-key rotation or forward secrecy

**Location:** Architectural — `rust/snapclone-crypto/src/lib.rs:124`
and SnapRouter's reuse of `m_sessionKeys` across all snaps.

The session key is derived once from `X25519(self.priv, peer.pub)`
and reused for ALL snaps between that pair, ever. No ratchet, no
ephemeral keys, no rotation.

Consequence: a key compromise at time T decrypts every snap ever
sent in either direction between that pair. This is the
"forward secrecy" property modern messengers (Signal, Wire, iMessage
PQ) have but SnapClone lacks.

**Why it matters:** Snap deletion (the product promise of "ephemeral
content") is not honored at the crypto layer. A passive recorder of
ciphertext + a single later key compromise = total transcript
recovery. This contradicts the user-visible promise.

**Suggested fix:** Move to ephemeral DH per session, or implement
a Double Ratchet (Signal-style) — the latter is significant work,
the former is a small change:
- SHAK exchanges an ephemeral X25519 pub in addition to the
  long-term identity Ed25519 pub.
- Session key = HKDF(static-ECDH || ephemeral-ECDH).
- New ephemeral on each SHAK (each TCP connection).

This gives "lazy forward secrecy" — keys rotate on reconnection. Not
as strong as Double Ratchet but much better than current.

The blocking design discussion in the prior session ("per-contact
derived key rotation" as a block-evasion defense) is the same
infrastructure — wire both at once.

---

### S-10. (medium) Pubkey validation on registration: no small-subgroup check

**Location:** `src/core/AppController_connect.cpp:54-55, 76`,
`rust/snapclone-crypto/src/lib.rs:135-140`

The receiver gets a 32-byte `peerPubKey` from the SHAK and passes
it straight to `sc_derive_session_key`. The Rust function does
`PublicKey::from(pub_bytes)` — `x25519-dalek` v2's `PublicKey::from`
does NOT reject small-subgroup or low-order points by default.

Consequence: an attacker can send a public key like 0x0000... (the
identity element) or one of the few small-order points, and the
resulting shared secret is a known constant (0 for identity).
The session key becomes `HKDF(0, "snapclone-v1-session")` — known
to the attacker. They can then decrypt everything sent under this
"session key."

**Why it matters:** Combined with S-2 (no SHAK signature), this is
a direct compromise vector: spoofer sends a SHAK with public key
= zero. Receiver derives the predictable session key. Attacker now
holds the session key Bob will use to encrypt to "Alice." Bob's
outgoing snaps are decryptable by the attacker.

**Suggested fix:**
- In `sc_derive_session_key`, reject obviously-bad keys: zero,
  small-order points. The `x25519-dalek` crate has a
  `check_for_small_order` or similar; `Montgomery::is_torsion_free`
  exists in `curve25519-dalek`.
- More robustly: switch to an authenticated KEM (e.g. ECDH +
  Ed25519 sig binding) so the peer must prove they generated the
  ephemeral, not just send arbitrary bytes.

---

### S-11. (medium) Ed25519 verify accepts malleable signatures

**Location:** `rust/snapclone-crypto/src/lib.rs:304-309`

```rust
let signature = Signature::from_bytes(&sig_bytes);
match verifying.verify(data_slice, &signature) {
    Ok(_)  => CryptoError::Ok,
    Err(_) => CryptoError::InvalidSignature,
}
```

`ed25519-dalek 2` defaults to `verify_strict` only if you call it;
`verify` accepts the legacy non-strict form (signature malleability:
valid `(R, s)` and `(R, s+L)` both verify for the same message).

**Why it matters:** If signatures are ever used as deduplication or
integrity keys (e.g. "have I seen this signature before"), malleability
breaks that. Today signing isn't wired into SHAK / snaps (per landmark
discussion in transcripts, ed25519 use is deferred); fixing this is
cheap now and free later.

**Suggested fix:** Replace `verify` with `verify_strict`. One-line
change.

---

### S-12. (medium) MailboxEntry from the DHT is unsigned

**Location:** `rust/snapclone-mailbox/src/entry.rs:43-101`

```rust
pub struct MailboxEntry {
    pub snap_id: String,
    pub content_hash: String,
    pub sender_id: String,
    ...
}
```

The DHT stores `MailboxEntry` as serialized JSON. The `sender_id`
field is self-asserted — there is no signature binding the entry to
the claimed sender's key. **Anyone with DHT write access can plant
entries claiming to be from anyone.**

The recipient sees a "snap arrived from sc-XXXX" notification, attempts
to fetch from `delivery_hints`, and either:
- Reaches a real attacker server that serves a malicious payload
  (signed by attacker, will fail recipient's MAC check — see S-3
  caveat — so the snap fails to decrypt, but the recipient has
  spent time on fetch attempts).
- Reaches nothing, and the notification stays as a phantom unread.

Worse: combined with S-4/S-5, plant entries in the right mailbox
bucket and you can DoS-spam someone's notifications.

**Why it matters:** This is the discoverability layer's authenticity.
Without it, the DHT can deliver malicious-looking "messages" before
the AEAD even gets involved.

**Suggested fix:** Sign the MailboxEntry. Add a `signature` field
that is `Ed25519(sender_priv, hash(snap_id || content_hash || sender_id
|| recipient_id || payload_bytes || expires_at || created_at ||
delivery_hints))`. Recipient verifies signature against sender's
Ed25519 pubkey (from contact DB or DHT-published pubkey record) before
acting on the entry.

The DHT_DISCOVERABILITY.md design doc from V32.33 already lays the
groundwork — per-contact salts assume known pubkeys exist; once that
infra is in place, signing is a small addition.

---

### S-13. (medium) `try_into().unwrap()` panics on attacker-controlled lengths

**Location:** `rust/snapclone-crypto/src/lib.rs:133-136, 176-178, 222-227, 267-298`

Multiple sites use `slice::from_raw_parts(ptr, KNOWN_LEN).try_into().unwrap()`.
The known-length parameter comes from caller-supplied size that the
FFI function trusts. If a future caller sends a shorter buffer than
declared (size says 32 but buffer is 28), the slice is created with
the requested 32 (so the C++ side allocated 32 — fine), but if the
length is checked at the C++ side and trusted at the Rust side, a
mismatch from a buggy caller is a panic.

A panic across an FFI boundary is **undefined behavior**: the panic
unwinds, hits the C ABI boundary, and the behavior is unspecified
(may abort, may corrupt). Rust 1.71+ aborts by default in this case
(safer) but the spec doesn't require it.

**Why it matters:** Defense-in-depth. The FFI surface is the trust
boundary between C++ (which can have memory safety bugs) and Rust
(which assumes lengths are correct). One C++ bug + the panic-on-bad-len
combination = aborted process at best, exploitable UB at worst.

**Suggested fix:** Replace `try_into().unwrap()` with explicit length
checks that return `CryptoError::InvalidInput`. The pattern is:
```rust
if cipher_len != EXPECTED { return CryptoError::InvalidInput; }
let bytes: &[u8; N] = slice::from_raw_parts(ptr, N).try_into()
    .unwrap_or_else(|_| unreachable!());
```
Or write a small helper that returns `Result<[u8;N], CryptoError>`.

---

### S-14. (low) HKDF derivation has no salt

**Location:** `rust/snapclone-crypto/src/lib.rs:58-63`

```rust
fn derive_session_key(shared: &[u8], info: &[u8]) -> Zeroizing<[u8; KEY_LEN]> {
    let hk = Hkdf::<Sha256>::new(None, shared);  // None = no salt
    ...
}
```

HKDF without salt is technically fine when the input keying material
(IKM) is already uniformly random — the X25519 shared secret has
about 252 bits of entropy and is suitable for direct use as a key
without HKDF. So HKDF with no salt is acceptable here.

But: omitting salt forecloses the use of HKDF for two derivations
that share an IKM but differ in salt (e.g. per-conversation salt =
hash of session metadata). If you later want salted derivation for
per-message keys or partner-pair sub-derivations, this won't compose.

**Why it matters:** This isn't a vulnerability today, it's a forward-
compatibility limit. Flagged so you know it's intentional and
consider salting if the key hierarchy grows.

**Suggested fix:** Optional. If/when per-session, per-channel, or
per-snap keys are derived, pass a salt = `(sender_id || recipient_id
|| seq_epoch)` or similar binding. Document the choice.

---

### S-15. (low) Identity key file format: hex on disk, world-readable parent dir

**Location:** `src/crypto/RustCryptoService.cpp:111-117`,
`src/crypto/CryptoService.cpp:115-127`

The .priv file gets 0600 permissions explicitly (good). However:
- The parent directory `<AppDataLocation>/keys/` may have default
  umask permissions (often 0755), letting other local users `ls`
  the directory and learn which sc-IDs exist on the device.
- Hex encoding doubles the size on disk vs raw bytes. Minor, but
  also means tools like `cat` and `xxd` are happy to dump the key
  to a terminal scrollback or log inadvertently.

**Why it matters:** Low. Local-user attacks on a multi-user device.

**Suggested fix:** `QDir().mkpath(dir)` doesn't set permissions;
set them explicitly to 0700 on the keys dir. Hex-on-disk is a
stylistic choice — base64 is shorter, raw bytes are smallest.

---

### S-16. (low) `extract_sc_id` matches any parenthesized sc- pattern

**Location:** `rust/snapclone-identity/src/lib.rs:25-30`

```rust
pub fn extract_sc_id(display: &str) -> Option<String> {
    let start = display.find("(sc-")?;
    let end   = display[start..].find(')')? + start;
    ...
}
```

`find` returns the FIRST `(sc-`. If a display name like
`Bob (sc-evilevil) (sc-12345678)` arrives over SHAK, the function
extracts the FIRST sc-ID, not the legit suffix. Receivers should
treat the SHAK-supplied display name as untrusted (it is — comes
from the wire) and validate that the extracted sc-ID is the bare
suffix.

C++ version (`src/core/AppController_connect.cpp:65-71`) uses a
regex that captures `(sc-[0-9a-f]{8})` — same risk: multiple matches
return the first. Both should anchor to the END of the string or
the EXACT suffix position.

**Why it matters:** Combined with S-2 and S-4, an attacker can craft
display names to confuse the sc-ID extraction. If contact lookup
later uses the wrong sc-ID, snaps may be routed wrong or
contact-confusion attacks succeed.

**Suggested fix:** In `extract_sc_id`, require the sc-ID to be at
the end of the string: check `display.ends_with(format!("({})", id))`
or use a regex anchored with `$`. Same in C++.

---

### S-17. (low) Padding random-seed-as-DOS-vector via SystemTime

**Location:** `rust/snapclone-mailbox/src/envelope.rs:39`

`std::time::SystemTime::now()` can fail / return a `SystemTimeError`
if the system clock is set before UNIX_EPOCH. The `.hash(&mut h)`
call is fine (Hash is impl'd on SystemTime), but in unusual clock
states the padding seed could be highly predictable (e.g. 0).
Not security-critical (padding isn't a defense per S-6 either way),
but flagging for completeness.

---

### S-18. (informational) `CryptoError` enum is not exhaustive at FFI

**Location:** `rust/snapclone-crypto/src/lib.rs:44-52`

The error enum has a defined set of variants. The cbindgen-generated
header may or may not enforce exhaustiveness on the C++ side. If a
future Rust change adds a variant without rebuilding the C++ side,
the comparison `err != CryptoError_Ok` still works but the new
variant may be silently misinterpreted as one of the existing values.

**Why it matters:** Forward compat. Low risk.

**Suggested fix:** Add a `_NonExhaustive` variant or convention; OR
ensure cbindgen is always run when Rust changes.

---

## Cross-cutting themes

### Identity is the weakest layer

S-2, S-4, S-5, S-12, S-16 are all about identity being weakly
authenticated or weakly unique. The crypto primitives themselves are
solid (S-1 caveat aside) but they're attached to identities that
attackers can forge, collide, or confuse. **This is the highest-value
area to fix.** A single round of work would:

- Increase sc-ID to 128 bits derived from public key (S-4, S-5)
- Add Ed25519 signatures to SHAK (S-2)
- Add Ed25519 signatures to MailboxEntry (S-12)
- Reject small-order pubkeys (S-10)

That's maybe a week of work, ~200 lines of Rust + ~100 lines of C++,
gates a real security model.

### The AEAD is not bound to context

S-3 is the missing-AAD problem. Cheap to fix (literally add an
`aad: *const u8` parameter to the FFI functions and pass
`sender || recipient || seq`). Has cascading benefits — replay
protection becomes natural, downgrade attacks become harder.

### Memory hygiene is partial

S-7, S-8 are about C++ heap holding plaintext key material. Rust
side is clean; C++ side isn't. A small RAII wrapper fixes most of
this. Worth doing alongside the Rust-FFI consolidation discussed in
the prior session (the four-crate refactor).

### Defaults matter

S-1 is purely about defaults: the placeholder ships unless you
opt in. The Settings UI has a warning, but build users skip the UI.
Inverting the default (Rust-on by default, dev opts out) is a
one-line cmake change with large impact.

---

## What a real audit would cover that this doesn't

- Side-channel timing analysis of constant-time operations.
- Fault-injection / glitching resilience (mobile devices on hostile
  power supplies).
- Memory-layout analysis after compiler optimization (some constructs
  the source-level reading misses).
- Protocol-level interaction analysis (ProVerif / Tamarin).
- Live cargo audit run (CVE list as of the day of audit).
- Fuzz testing the FFI boundary with arbitrary inputs.
- Formal verification of the parsing in `routeInbound` and
  `handleV3Message` — these accept attacker-controlled bytes and
  bugs there bypass everything else.
- Threat modeling sessions with security engineers.

This document is a starting point, not a substitute. Treat it as
input to a real audit, not a final word.

---

## Addendum (2026-05-15 follow-up)

Two findings added after the user pushed on DHT layering and MITM
specifics. The audit above stays as-was; these are appended rather
than re-numbered.

### S-19. (serious) DHT mailbox entries are plaintext metadata broadcast

**Location:** `rust/snapclone-mailbox/src/entry.rs:43-101`,
serialised via `to_dht_bytes()` and stored in the DHT directly.

The original audit treated "the crypto" as one thing. It is two:

| Layer | What's encrypted | Visible to | Quantum-sensitive |
|---|---|---|---|
| UDP/TCP snap payload | AEAD ciphertext | Wire observers | Yes (HNDL) |
| DHT mailbox entry | **Nothing — plaintext JSON** | Anyone with sc-ID or DHT-scrape | No (already public) |

The mailbox entry contains: `snap_id`, `content_hash`, `sender_id`,
`recipient_id`, `payload_bytes`, `expires_at`, `created_at`, and
`delivery_hints` (potentially the sender's current IP and port).

**Why it matters:** Today threat, no crypto break required. Anyone
scraping the DHT learns the social graph, when conversations happen,
how much data is exchanged, and where senders connect from. The
512-byte size limit isn't a crypto weakness — there's no encryption
to brute-force. The entries are simply public.

Quantum makes this WORSE only in the sense that it tags which
encrypted payloads to record for HNDL — "Bob sent Alice 2 MB at time
T from address A" tells a future-quantum attacker which ciphertexts
on the wire are worth keeping.

**Suggested fix:** Several options, increasing in scope:

1. **Encrypt the entry to the recipient.** Add a long-term encryption
   pubkey published per user. Sender encrypts the JSON to that pubkey.
   DHT stores opaque ciphertext + a metadata stub. Modest cost.
   Asymmetric → Shor-broken → HNDL still applies.

2. **Hybrid PQ envelope.** Same as (1) but using hybrid X25519 + ML-KEM.
   Mailbox metadata becomes quantum-protected at rest. Kyber-768 adds
   ~1184 bytes; bumps the 512-byte cap. Need to widen.

3. **Architectural: eliminate identifying metadata from DHT.** The
   `DHT_DISCOVERABILITY.md` design (V32.33) moves toward per-contact
   salted DHT keys. Next step is opaque entries so even bucket
   discovery doesn't leak content.

(2) and (3) compose: per-contact salted key + hybrid PQ envelope.

### S-20. (serious) No TOFU check on peer pubkey changes — silent replacement

**Location:** `src/storage/StorageManager.cpp:1337-1352`,
called from `src/core/AppController_connect.cpp:77`.

```sql
INSERT INTO peer_public_keys (peer_id, public_key, updated_at)
VALUES (?, ?, datetime('now'))
ON CONFLICT(peer_id) DO UPDATE SET
  public_key = excluded.public_key,
  updated_at = excluded.updated_at
```

Every SHAK silently overwrites the stored pubkey for that peer ID.
**There is no Trust-On-First-Use (TOFU) comparison: the code does not
check whether the incoming pubkey matches the previously-stored one.**

**Why it matters:** This converts S-2 (no SHAK signature) from
"attacker can spoof at first contact" to "attacker can MITM at ANY
point in the relationship." Standard messenger practice:

- First time we see Alice's pubkey: store it.
- Subsequent times: REJECT a different pubkey unless the user
  explicitly accepts it ("Alice's key has changed").

Without this check, an active MITM (Mallory) can intercept Bob→Alice
at any time, send a SHAK with Mallory's pubkey claiming to be Alice,
and Bob's storage silently replaces real-Alice's stored pubkey with
Mallory's. From that moment, Bob encrypts to Mallory thinking it's
Alice. **This is the persistent-MITM path.**

**Suggested fix:**
- BEFORE registering new pubkey, compare with
  `storage->loadPeerPublicKey(peerId)`.
- If non-empty and different: log warning, DO NOT auto-save, surface
  as UI event requiring user confirmation.
- If user confirms (legitimate reinstall), update.
- Optional: persist a key-change history for audit.

Combined with S-2's signed-SHAK fix, the model becomes:
1. Incoming SHAK signature verifies against advertised pubkey.
2. Advertised pubkey matches stored one for this peer (or is first
   ever seen).
3. If pubkey differs, prompt user.

That's TOFU + sig-based proof-of-possession — what Signal etc. do
at this layer.

---

## Active MITM analysis (per-position attack capability)

Added after user asked about MITM specifics. Summarises what each
attacker position can actually do today.

### Position 1: Network observer (passive on-path)

- Read content: **with placeholder crypto, yes** (S-1). With Rust
  ECDH, no.
- Modify in flight: no (AEAD MAC).
- Replay: yes (S-3 — no AAD binding; m_recentInbound dedup is
  in-memory and storage write precedes dedup check).
- Block: yes (network-layer, no crypto prevents).

### Position 2: DHT participant

- Read entries: yes (S-19, plaintext).
- Plant fake entries: yes (S-12, unsigned).
- Block legit entries: requires eclipsing keyspace region; expensive
  but possible.

### Position 3: Active MITM (on-path interception)

**Case 3a** — Mallory uses Alice's REAL pubkey in SHAK to Bob:
Bob derives the real session key. Mallory can't decrypt (no
Alice.priv) but CAN hijack inbound transport mapping so Bob's
`sendToPeer(Alice)` routes to Mallory. **Blackhole DoS, no read.**

**Case 3b** — Mallory uses Mallory's OWN pubkey in SHAK,
claiming to be Alice: Bob derives session key from Mallory's
pubkey. Mallory has matching priv, decrypts. Bob's storage
silently replaces real-Alice's pubkey with Mallory's (S-20).
**Full MITM. Persistent.**

Only defense today is out-of-band pubkey fingerprint verification,
which the app doesn't expose.

### Position 4: Random internet attacker

- Inject fake snap claiming to be Alice:
  - **Placeholder crypto: yes** (S-1; session key computable from
    pubkeys alone).
  - **Rust crypto: no, IF S-10 fixed**. Without S-10's small-order
    check, attacker sends pubkey=0, Bob derives predictable key,
    attacker uses same predictable key. Inject succeeds. So S-10
    is critical for blocking random-internet injection on Rust
    builds.

### Replay across restarts

`m_recentInbound` cache:
- Clears on restart.
- Bounded; older entries flushed.
- Storage write precedes dedup check.

Attacker recording ciphertext from an old session can replay after
recipient restarts. UI emits old snap as new. Storage side-effects
(lastSeen, contact state) re-fire on each replay. Seq numbers are
plaintext after decrypt but no `seq <= last_seen_seq` check rejects.

### Quantum interaction (cross-reference)

Per user's earlier question: today's crypto is fully quantum-
vulnerable on the asymmetric side. HNDL applies to all ciphertext
recorded today. The mailbox plaintext (S-19) makes HNDL more
efficient by tagging which ciphertexts are worth recording —
attackers don't need to record everything, just the entries the
DHT advertises as belonging to interesting recipients.

### S-21. (serious) Stable ID is random, not cryptographically bound to keypair

**Location:** `src/core/Identity.cpp:42-43`

```cpp
const quint32 rnd = QRandomGenerator::securelySeeded().generate();
const QString id  = QStringLiteral("sc-%1").arg(rnd, 8, 16, QLatin1Char('0'));
```

The stable node identifier (`sc-XXXXXXXX`) is a random 32-bit value
generated and persisted to disk on first launch. It is NOT derived
from the user's public key.

**Why this is a problem:**

1. **Identity is unverifiable.** A peer who receives a SHAK from
   `sc-deadbeef` has no way to confirm the sender actually controls
   that identity. The sc-ID is just a self-chosen label, not a
   commitment to a key. Anyone can edit their identity.txt to claim
   any sc-XXXXXXXX they want.

2. **First-contact attacks.** An attacker who learns a target's
   sc-ID (e.g. via Settings screenshot, a shared contact link, or
   the DHT) can stand up an identity with the SAME sc-ID and their
   OWN pubkey. The first time the target's contacts try to reach
   the attacker's instance — or the attacker's instance reaches the
   target's contacts — TOFU will record THIS pubkey for that sc-ID.
   Now the attacker is "the trusted version" of that sc-ID for
   those contacts.

3. **32-bit collision space.** With ~65,000 active users the
   birthday paradox produces collisions by chance. Two unrelated
   users would happen to share a sc-XXXXXXXX, and contacts trying
   to reach one might TOFU the other.

4. **Identity persistence weaker than expected.** A user who
   reinstalls and loses identity.txt gets a new random sc-ID with
   no relationship to their previous one. Contacts see them as a
   new person. That's by design, but the design has no mechanism
   to PROVE "this new sc-ID is the same person you knew." With a
   pubkey-derived sc-ID, the new install would have a new sc-ID
   AND a new pubkey, and contacts could verify "the pubkey
   actually does hash to this sc-ID" — which is a weaker guarantee
   than identity continuity but at least binds the namespace
   correctly.

**The right fix:** derive the sc-ID from the public key. Standard
approach: `sc-<first-8-hex-chars-of-SHA-256(pubkey)>`. This makes
sc-ID a fingerprint. To claim someone's sc-ID, an attacker would
need to find a SHA-256 collision in the first 32 bits — birthday
paradox says ~65K attempts to maybe collide a SPECIFIC target.
That's not great either; better to use the first 64 bits (16 hex
chars), giving 4 billion attempts to maybe collide a specific
target. For a fully-secure fingerprint use the full 64-char
SHA-256, but that's unwieldy for display.

Realistic recommendation: `sc-<first-12-hex-chars-of-SHA-256(pubkey)>`
gives ~6 billion bits of collision resistance and stays at a
12-character display string. Birthday paradox: collisions at
2^24 = 16M random users, which is past any realistic SnapClone
deployment scale. Targeted collision (find a pubkey hashing to
specific 12 hex chars): 2^48 attempts on average, infeasible.

**Migration challenge:** existing users have random sc-IDs in their
identity.txt files. A flag-day switch to derived sc-IDs would break
trust relationships. Options:

1. **One-time migration:** on first launch after upgrade, save BOTH
   the random sc-ID (legacy) and the derived one (new). Accept either
   from peers. Eventually phase out legacy after some grace period.

2. **Per-install regeneration:** new installs use derived IDs, but
   no migration for existing installs. Doesn't fix existing users'
   vulnerability but doesn't break their trust either. Over time
   most installs become derived as users reinstall.

3. **Full migration with key generation:** treat this as a major
   version break. Have users re-add contacts. Painful but cleanest.

Filed at the bottom of the findings list because it's a real
architectural finding rather than a code bug to fix in a turn.
Requires design discussion with full crypto context, and the
migration plan is non-trivial.

**Relationship to S-2 + S-20:** the signed-SHAK + TOFU fixes don't
address this — they ensure pubkey continuity for a GIVEN sc-ID, but
do nothing about the sc-ID being attacker-chooseable. To fully close
the persistent-MITM family of attacks, S-21 needs to be addressed.
The current S-2 + S-20 work is necessary but not sufficient.

