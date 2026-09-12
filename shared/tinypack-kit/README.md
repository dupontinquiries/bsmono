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
