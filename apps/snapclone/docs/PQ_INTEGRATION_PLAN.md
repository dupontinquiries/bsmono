# PQ_INTEGRATION_PLAN.md

**Status:** primitives landed in Rust + C++; SHAK protocol upgrade pending
**Filed:** V32.33 session, 2026-05-16
**Scope:** wire-format break, multi-phase rollout
**Estimated effort:** 4-6 turns across multiple sessions

---

## Why this exists

The Rust crypto crate now exposes `sc_mlkem_generate_keypair`,
`sc_mlkem_encapsulate`, `sc_mlkem_decapsulate`, and
`sc_derive_hybrid_session_key`. The C++ wrapper (RustCryptoService)
exposes them as `mlkemGenerateEphemeral`, `mlkemEncapsulate`,
`mlkemDecapsulate`, `deriveHybridSessionKey`.

But none of this is yet **wired into the SHAK handshake**. The
existing flow is still pure X25519:

```
Alice → Bob: SHAK { x25519_pub_A, sender_id }
Bob:   derive session_key = HKDF(x25519_DH(priv_B, pub_A), "snapclone-v1-session")

Bob → Alice: SHAK { x25519_pub_B, sender_id }
Alice: derive session_key (same as Bob's)
```

For PQ, we need a round trip that includes ML-KEM:

```
Alice → Bob: SHAK_v2 { x25519_pub_A, mlkem_ek_A (ephemeral!), sender_id, signed_by_A }
Bob:   compute (ct, ss_pq) = mlkem_encap(mlkem_ek_A)
       compute ss_dh = x25519_DH(priv_B, pub_A)
       compute session_key = HKDF(ss_dh || ss_pq, "snapclone-v1-pq-session")

Bob → Alice: SHAK_v2 { x25519_pub_B, mlkem_ct, sender_id, signed_by_B }
Alice: compute ss_pq = mlkem_decap(my_ephemeral_dk, mlkem_ct)
       compute ss_dh = x25519_DH(priv_A, pub_B)
       compute session_key (same as Bob's)
```

This is a **wire-format break**. Existing peers won't understand SHAK_v2.

## Why ephemeral on Alice's side

Forward secrecy. Alice generates a FRESH ML-KEM keypair per session.
`mlkem_dk_A` exists only for the duration of the handshake, then is
discarded once she's derived the session key.

If an attacker later compromises Alice's long-term storage (steals her
identity.txt and X25519 private key), they can decrypt the x25519 half
of the hybrid session key — but not the ML-KEM half, because the
ephemeral `mlkem_dk_A` was never stored.

**This gives us PQ-FS without classical-FS** — the X25519 side still
uses a static long-term key, so a future quantum break of Curve25519
combined with a long-term key leak would expose the session. Classical
FS via ephemeral X25519 keys is filed separately.

## Why ML-KEM-768 and not 512 or 1024

NIST recommends 768 for general use:
- 512: ~AES-128-equivalent. Faster, smaller keys, but margin shrinks.
- 768: ~AES-192-equivalent. Recommended general-purpose level.
- 1024: ~AES-256-equivalent. Used where cryptanalysis margin matters.

SnapClone messages have low individual value but large aggregate
volume. 768 is the right point. Wire-size cost vs 512: ~256 extra
bytes per SHAK (acceptable). Wire-size cost vs 1024: ~640 fewer bytes
per SHAK (worth taking).

## Protocol negotiation

Can't break existing peers. Options:

1. **Version flag in SHAK header.** New magic "SHK2" + version byte
   + capability flags + the rest. Peers seeing unknown magic ignore
   the packet; older peers respond with SHAK_v1.

2. **Capability advertisement during DHT announce.** Peers advertise
   `pq_support=1` in DHT metadata. Initiators check before sending v2.

3. **Both peers always send v2 when capable; receivers handle both.**
   Simplest. Newer clients ALWAYS send SHAK_v2. Older clients see v2
   magic, ignore the packet, and reconnect later with v1 from us as
   fallback.

I prefer **option 3** with **option 2 as optimization**: always send
v2 when capable, but if DHT metadata says peer doesn't support PQ,
skip the wasted v2 send.

## Wire format SHAK_v2

```
Magic:           4 bytes  "SHK2" (0x53484B32)
Version:         1 byte   0x02
Capability flags:1 byte   bit 0 = "PQ-hybrid enabled"
ID length:       2 bytes  big-endian
ID:              N bytes  UTF-8 (same as v1)
X25519 pubkey:   32 bytes
ML-KEM ek:    1184 bytes  (only present if cap bit 0 set AND initiator)
ML-KEM ct:    1088 bytes  (only present if cap bit 0 set AND responder)
Ed25519 sig:    64 bytes  signs the entire blob up to here
```

A direction-bit (initiator vs responder) saves us from having both ek
and ct present — only one is set per SHAK direction. Bikeshed later.

**Signing matters.** S-2 in CRYPTO_REVIEW.md highlights that the
current SHAK has no signature — an attacker can forge SHAKs claiming
to be from any peer. We MUST sign SHAK_v2 with Ed25519 over all
preceding fields. This kills the persistent-MITM attack family along
with the TOFU-on-pubkey-change defense (S-20, already shipped).

## Storage changes

Persist:
- Ed25519 long-term signing key (for SHAK signature)
- X25519 long-term key (existing)
- ML-KEM keys are EPHEMERAL — don't persist them

The Ed25519 signing key currently exists in code but isn't always
generated. The fix: when a user first launches a PQ-aware build,
generate both X25519 and Ed25519 keypairs. Tie the sc-ID to the
Ed25519 pubkey hash (see S-21 — sc-ID-from-pubkey is a related but
separate change).

## Per-session state

The initiator (Alice) needs to keep `mlkem_dk_A` around until Bob's
ct arrives. This is per-PEER, per-PENDING-SHAK state.

We need a `Pending<MLKEM-DK>` map keyed by peer ID that:
- Times out after ~5 minutes (no SHAK reply → drop the ephemeral key)
- Zeroes the key material on drop (use a `Zeroize` wrapper)
- Has a single-flight guard (same peer sends multiple SHAKs in quick
  succession → only one ephemeral pending)

## Migration plan

### Phase A (this session): primitives ✅
- [x] Rust FFI: sc_mlkem_generate_keypair, sc_mlkem_encapsulate,
      sc_mlkem_decapsulate, sc_derive_hybrid_session_key,
      sc_x25519_raw_shared
- [x] C++ wrappers: mlkemGenerateEphemeral / mlkemEncapsulate /
      mlkemDecapsulate / deriveHybridSessionKey / x25519RawShared
- [x] C++ convenience: respondHybridHandshake / finishHybridHandshake
- [x] This document filed

### Phase B (this session): unit-level verification ✅
- [x] Round-trip test: Alice generates → Bob encapsulates → Alice
      decapsulates → both arrive at same SS
- [x] Hybrid derivation matches across both sides (full_hybrid_handshake)
- [x] Concatenation-order regression guard
- [x] Tampered-ciphertext implicit-reject verification
- [x] Null-pointer FFI safety
- [x] Bench: ML-KEM-768 on x86 dev host (Rust 1.75):
        keygen 0.08ms / encap 0.08ms / decap 0.11ms / total ~0.27ms
      Expect 3-10× on ARM (still well under the 50ms phone budget).

### Phase C (2-3 turns): wire format + dual emit
- [ ] Define SHAK_v2 wire format in network/Network.cpp
- [ ] Initiator sends BOTH v1 and v2 (transition period)
- [ ] Responder parses either; prefers v2 when present
- [ ] Capability bit visible in Settings as
      "Post-quantum: ready (peer-pending)" / "active"

### Phase D (1-2 turns): signing
- [ ] Ed25519 keypair generation alongside X25519
- [ ] SHAK_v2 signed-by-sender
- [ ] Verification on receipt; reject unsigned v2

### Phase E (later): retire v1
- [ ] Emit v1 only as fallback when peer's DHT metadata says no PQ
- [ ] Eventually remove v1 path entirely

## Threat model coverage

After all phases land:

| Attack                          | Before     | After (B+C+D)   |
|---------------------------------|------------|-----------------|
| MITM (real-time)                | TOFU       | TOFU + signed   |
| Persistent MITM (key swap)      | TOFU       | TOFU + signed   |
| Classical break of X25519       | broken     | survives via PQ |
| Future quantum break of X25519  | broken     | survives via PQ |
| State actor + Q-computer + HNDL | broken     | survives*       |
| Long-term key leak THEN quantum | broken     | survives PQ-FS  |

*assuming ML-KEM holds. The fips203 crate hasn't been independently
audited; for real production use we should migrate to libcrux-ml-kem
(Cryspen, formally verified) once integration cost is justified.

## Known limitations / future work

- **Classical FS missing.** X25519 is still static. Adding ephemeral
  X25519 keys would mirror the PQ design. Filed separately.
- **S-21 still open.** Random sc-ID. Even with all PQ work, attacker
  can claim someone else's sc-ID by editing identity.txt. Needs
  pubkey-derived sc-IDs.
- **ML-KEM forward secrecy only for newly-generated ephemerals.** If
  Alice's ephemeral dk leaks BEFORE she discards it (e.g. core dump,
  swap), the corresponding session's PQ-FS is broken. Zeroize helps
  but isn't perfect.
- **No protection against active downgrade attacks.** An attacker
  who can MITM the DHT could remove the PQ capability bit from peer
  metadata, forcing v1 fallback. Mitigation: once we've SEEN a peer
  advertise PQ, refuse to fall back. (TOFU on capability.)
- **Ed25519 isn't PQ-secure.** The signature side uses Ed25519, which
  a quantum computer can break. NIST has standardized ML-DSA (FIPS
  204) and SLH-DSA (FIPS 205) for PQ signatures. A future hybrid
  would sign with both Ed25519 and one of these. Out of scope for
  this protocol bump.

## Decision log

- **Why ml-kem in hybrid, not raw replacement of X25519?** Defense in
  depth. Lattice-based crypto is younger than ECC, and cryptanalysis
  may yet find weaknesses. Keeping X25519 means a single break doesn't
  fall the whole protocol. The wire-size cost (~2.3KB extra per SHAK)
  is acceptable for a non-realtime messenger.

- **Why ephemeral on initiator side, not both sides?** ML-KEM is a
  KEM, not a full key agreement. Only the responder side gets to be
  "non-ephemeral" naturally (they receive ek, run encap, derive ss).
  Making the initiator's side ephemeral provides PQ-FS at one extra
  ML-KEM keygen per handshake (~5-30ms on phone hardware).

- **Why 32-byte concatenation order matters.** If Alice and Bob feed
  HKDF with different orderings (x25519 || mlkem vs mlkem || x25519),
  they'll arrive at different session keys. We FIX the order
  protocol-level: x25519 first, then ml-kem. Both implementations
  use sc_derive_hybrid_session_key with that order.

- **Why fips203 crate not ml-kem (RustCrypto)?** Cleaner FFI surface.
  fips203 operates on plain `[u8; N]` arrays for ek/dk/ct/ss; ml-kem
  uses hybrid_array generic-array types that don't cross the FFI
  boundary cleanly. Both crates are pure-Rust FIPS 203 compliant.
  Migration path: keep the FFI signatures stable; swap the crate
  underneath later if the integrity-of-crate criteria change.

- **Why not just use libcrux-ml-kem directly?** It's the
  formally-verified, audited choice — and that's where production
  should land. But libcrux is more complex to integrate (platform-
  specific paths, build.rs surprises on cross-compile to phone
  targets). Starting with fips203 lets us validate the SHAK protocol
  upgrade and then swap underneath. The C++ side never knows the
  difference.
