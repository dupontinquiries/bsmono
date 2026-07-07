# SHAREDLIB_REPORT — r340-sl-net (net codec consolidation)

**WID:** `r340-sl-net` · base = r330b (dispatch base r339 — see "Base mismatch").
**Verified in-sandbox:** `rustc --edition 2021 --test codec.rs` → **9 passed / 0 failed** (rustc 1.96.0),
incl. golden byte-parity vs the `wire` sync packet and `net_rendezvous` blob header, and the alloc-free
reuse parity.

## Base mismatch (read first)
The dispatch names the cluster `sim::net::{wire, chunk_stream, entity_snapshot, world_advert,
world_presence}`. In **this r330b base** only `wire` exists; `chunk_stream`/`entity_snapshot`/
`world_advert`/`world_presence` are r335–r339 additions **not present**. So I grounded the codec
against the modules that ARE here and that exhibit the exact same pattern:
`sim::net::wire`, `sim::net_rendezvous`, `sim::net::dht_mailbox`, `sim::dht_join`. The dispatch
asserts the r339 modules re-implement the same `Reader`/header/capped reads — so the same `codec`
maps onto them unchanged; the coordinator sweeps those call sites on merge.

---

## Candidates

### C1 — Bounded little-endian `Reader` (`take/u8/u16/u32/u64/i64`)  → **EXTRACTED**
The same bounds-checked LE cursor is copied verbatim (semantically) in:
- `sim/net/wire.rs:132-160` — `struct Reader<'a>` + `take` (checked_add + `b.get`), `u8/u16/u32/u64/i64`.
- `sim/net_rendezvous.rs:138-164` — identical `Reader<'a>` + `take`/`u8`/`u16`/`u32`/`u64`
  (impl detail differs — `[s[0],s[1]]` vs `try_into()` — **byte-identical** decode).
- `sim/net/dht_mailbox.rs`, `sim/dht_join.rs` — partial inline length/scalar reads of the same shape.

**Home:** `sim::net::codec::Reader` (extracted, this return). Adds `i8/f32/f64/bool`, `remaining`,
`is_at_end` (for the "no trailing garbage" checks `net_rendezvous::decode` does), `check_header`,
capped `read_str`/`read_vec`. Byte-identical to both copies.

### C2 — `Writer` = `Vec<u8>` + `extend_from_slice(&x.to_le_bytes())` / `push`  → **EXTRACTED**
The encode side is hand-rolled everywhere:
- `wire.rs:55-82` (`encode_input_packet`, `encode_sync_packet`), `wire.rs:118-124` (`write_input`).
- `net_rendezvous.rs:90-106` (`MailboxBlob::encode`).
- plus `dht_mailbox`/`identity`/`session` build frames the same way.

**Home:** `sim::net::codec::Writer` (extracted) — LE `u8..f64/bool/bytes`, `header`, capped
`write_str`/`write_vec`. Produces byte-identical output (golden-vector tests pin it).

### C3 — Frame header `magic 0x554E (u16 LE) + version (u8)`  → **EXTRACTED**
`PROTOCOL_MAGIC=0x554E`/`PROTOCOL_VERSION=1` written in every `wire` encoder and checked in
`wire::decode:85-89`; `net_rendezvous` uses a `version (u8)` blob header (`BLOB_VERSION`).
**Home:** `codec::Writer::header(magic, version)` + `codec::Reader::check_header(magic, version)`.
(Magic stays a parameter — `wire` passes `0x554E`; blob formats pass their own / omit it.)

### C4 — Count-prefixed, length-capped vec/string reads  → **EXTRACTED**
- `wire.rs:96-104` — `u16` count + `MAX_INPUTS_PER_PACKET=256` cap + decode loop.
- `net_rendezvous.rs:97-104,118+` — `u16` count (`.min(u16::MAX)`) + `.take(count)` loop.
- capped `String` reads (name/world-name) live in `identity`/`world_advert` (r339).
**Home:** `codec::Reader::{read_str(max), read_vec(max, f)}` + `Writer::{write_str, write_vec}`.

### C5 — `Packet`/`MailboxBlob` enum framing  → **PROPOSE (keep per-module)**
The `kind u8` dispatch + per-variant fields are genuinely module-specific (input vs sync vs blob vs
join). **Not** consolidated — only the primitives under them (C1–C4) are shared. Keep the enums local.

---

## Cross-cluster overlap flags (for the coordinator's matrix)
- **→ sl-math:** none of C1–C4 are math; no overlap with `mathx`/`simvec`. (Codec is byte-plumbing.)
- **→ sl-gpu:** `render::q8` / `neural_gemm_pipeline` packing also does LE byte writes, but with a
  **fixed-layout, no-header** discipline and byte-parity-to-oracle constraints — do **not** route the
  quantizer through this codec (different invariants). Flagging so the matrix doesn't over-merge.
- **→ sl-worldgen / sl-mesh:** save/serialization in those clusters (if any) may reuse `codec::Writer`,
  but their formats are out of scope here — propose only.
- **dht/rendezvous blobs** (`mp_dht`, `net_rendezvous`, `dht_join`) all want C1+C4 — likely the highest-
  overlap consolidation; pick this `codec` as the canonical home.

---

## Call sites this `codec` would replace (coordinator sweep — propose, not rewritten)
| site | replace with |
|---|---|
| `wire.rs:132-160` private `Reader` | `codec::Reader` |
| `wire.rs:55-82` encoders' header+LE writes | `codec::Writer` + `header` |
| `wire.rs:83-104` `decode` header+count loop | `codec::Reader::check_header` + `read_vec` |
| `net_rendezvous.rs:138-164` `Reader` | `codec::Reader` |
| `net_rendezvous.rs:90-130` encode/decode | `codec::Writer`/`Reader` + `write_vec`/`read_vec` |
| `dht_mailbox.rs`, `dht_join.rs` inline reads | `codec::Reader` |
| (r339) `chunk_stream`, `entity_snapshot`, `world_advert`, `world_presence` | same — map on merge |

**Do not change** magic (`0x554E`), version, field order, or LE encoding — the existing per-module
wire tests must keep passing verbatim (that is the parity gate).

## Perf bonus (a) — alloc-free encode path  → **EXTRACTED + parity-proven**
`codec::Writer` borrows `&mut Vec<u8>`; `encode_into(buf, f)` clears + rewrites a **reused** buffer, so
after warm-up repeated encodes allocate nothing. Test `reused_buffer_is_byte_identical_and_allocfree`
proves: (1) reused-buffer output == one-shot `encode` output byte-for-byte over 5 encodes; (2) capacity
is unchanged across 100 reuses (no realloc). This is the small win for the per-frame packet senders
(`session::outgoing_input_packet`, the rendezvous announce loop) — keep one buffer, refill it.

## Byte-parity proof (the RESPECT constraint)
- `golden_parity_wire_sync_packet`: codec emits exactly `[4E 55][01][01][tick u64 LE][cksum u64 LE]`,
  **20 bytes** — matches `wire.rs`'s own `Vec::with_capacity(20)` and its literal write sequence; the
  codec `Reader` decodes the same bytes back to the same fields.
- `golden_parity_rendezvous_blob_header`: codec emits `[ver][peer u64][session u64][nonce u64][count u16]`,
  **27 bytes** — matches `net_rendezvous`'s `Vec::with_capacity(27 + …)`.
- I did **not** mount `wire.rs` directly (it pulls `sim::state`→`sim::physics` via its test module); the
  golden vectors transcribe `wire`'s exact byte sequence and are cross-checked against its capacity
  hints. Final confirmation = the existing `wire`/`net_rendezvous` tests passing verbatim after the
  coordinator's sweep.
