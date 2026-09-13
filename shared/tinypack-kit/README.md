# tinypack-kit

Fit the most into a tiny fixed byte budget — an NFC tag, a DHT value, a QR.
Zero dependencies. Headless; the `tinypack` CLI is the visible loop.

| layer | what | measured (this sandbox, x86, release) |
|---|---|---|
| `text` | static-vocab token codec, no header, any bytes | EN chat 3.5 bits/byte (deflate-9: 8.3 — it *expands*); prose 3.0; FR chat 4.5 (corpus-limited) |
| `image` | headerless 8×8-DCT over an adaptive binary arithmetic coder, `fit()` picks resolution × quantiser under a budget | 512² cameraman → 128×128 luma in 734 B; cat in colour 96×64 in 717 B; 48×32 colour in 144 B |
| `drop` | store-carry-forward over passive tags: card log format, the tap transaction, relay policy | fleet of 3 cards / 3 phones delivers across two couriers in a test; four load-bearing rules break-tested |

752 bytes (MIFARE Classic 1K) holds ~2,000 chars of English prose, ~1,700 of
English chat, or a recognisable 128×128 photo. Files never fit; the tag
carries a pointer. That is what `drop` is for: the tag as mailbox, the phone
as courier.

## Retrain the text vocabulary

```
python3 tools/extract_corpus.py <repo-root> corpus.txt          # prose from the repo's markdown
python3 tools/train_vocab.py --corpus corpus.txt --corpus tools/seed_chat.txt --weight 1 --weight 100 \
    --vocab 4096 --out src/text/vocab_data.rs \
    --heldout testdata/heldout_chat_en.txt --heldout testdata/heldout_chat_fr.txt \
    --vectors testdata/tokenpack_vectors.txt
```

Only `vocab_data.rs` and the vectors change; `matches_python_test_vectors`
proves the Rust encoder agrees with the trainer byte for byte. Point
`--corpus` at real chat history (and more French) to fix the FR gap.

## Not verified

No ARM measurements. `drop` has no transport yet — the seam is
`Device::tap(image) -> image`; a libnfc/neard backend maps that to page/block
writes of only the bytes that changed. `Sealer` is a trait; `MacSealer` is a
test stand-in, not crypto — `crypto-kit` (AES-GCM, Ed25519) is the real one.
`fit()`'s cross-resolution PSNR favours the largest rung; read the table.


## `ledger` — who did what, and whose scrip you hold

A personal, offline-first receipt chain. Every person is their own issuer:
an append-only hash-linked signed log where `Gift{to,amount}` hands out your
own scrip and `Attest{subject,tag,obj}` records authorship / an access grant /
a friendship. Single-author chains mean no consensus — verify one signature
and one hash link. Equivocation (two entries at one `seq`) is kept as proof,
not raced. `Store` is a grow-only CRDT: `merge` is commutative + idempotent,
so tapping cards in any order converges. Entries frame into one `drop` pack
via `pack_batch`/`fit_batch`, so receipts propagate on the same cards as
messages; a DHT is just another transport later.

Answers the intended questions directly: `authors_of(obj)` ("who wrote this
voiceterm script"), `attestations_about(me, tag::GRANT)` ("who gave me
logscope access"), `mutual_friends(a,b)` (snapclone). `balance(issuer,holder)`
sums un-revoked gifts; a caught equivocator's balance is zero.

Crypto is the same seam as `drop`: `Sealer` for sign/verify (Ed25519),
`Hasher` for content ids (BLAKE3) — both deployment-wide constants supplied
by `crypto-kit`; tests use `MacSealer` and a labelled non-cryptographic
mixer. Not verified: no real crypto wired yet; no DHT transport; no app
integration (voiceterm/logscope/snapclone) — that is deliberately held off.

## `std` vs `no_std`

The carry layer — `huffman`, `bac`, `drop`, `ledger` — builds `no_std` +
`alloc`, so it targets the ESP32-class anchors and device-carriers the courier
model points at. The phone-side codecs (`text`, `image`) and the CLI need
`std` and sit behind the default-on `std` feature.

```
cargo build --no-default-features   # carry layer only, no_std + alloc
cargo test                          # full suite (std)
```

Verified: `--no-default-features` compiles clean on x86 host (forces the crate
through no_std). NOT verified: a true bare-metal target — the offline toolchain
here only ships the x86_64 std sysroot, so no aarch64/thumb/riscv `core` to
cross-build against. The one place that differs on real hardware is the `vec!`
macro import in `drop`/`ledger`, which the host no_std build resolves via the
sysroot prelude but a bare target needs explicitly (kept, with an
`allow(unused_imports)` for the host false positive). Float transcendentals in
`image` would need `libm` under no_std, which is why `image` stays std-gated.