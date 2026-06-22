# Media policy (decided)

Resolves the earlier open question. Per-type, because photos and video
want different treatment.

## Video → AV1 (lossy accepted)

Transcode ingested video to AV1; lossy is acceptable here. Encoder:
SVT-AV1 / libaom, GPU-assisted via Vulkan or VAAPI where the hardware
supports it. Optional `--keep-original` flag retains the source in the
chunk store for anyone who wants the untouched bytes; default is the AV1
as the stored form.

## JPEG / photos → near-lossless + 99%-match tracking

Goal: shrink and dedup photos *without* introducing visible recompression
drift, while accepting that bit-exactness across editor re-saves can't be
guaranteed.

- **First-class store**: keep the JPEG losslessly. A lossless JPEG
  *transcode* (JPEG XL lossless / brunsli-style repacking) shrinks ~20%
  with the original pixels exactly recoverable — no generational artifacts.
- **Near-duplicate tracking (built)**: when a photo is re-saved or
  retranscoded and is *not* bit-identical, `similarity.rs` measures how
  much it overlaps existing blobs via shared chunks. At ≥ 0.99 the store
  links them as near-duplicates and keeps the shared chunks once, storing
  only the residual chunks plus metadata (original name, similarity score,
  source blob). This is the "long-term diff for a few residuals" — drift
  is captured as a small delta against the matched original, not a fresh
  full copy.
- **No false promises**: we record similarity + provenance metadata and
  do *not* claim bit-exactness across recompressions. Exact originals are
  available whenever `--keep-original` is set.

## What's implemented now

- `similarity.rs`: byte-weighted chunk-overlap similarity + a near-dup
  gate, unit-tested (identical → 1.0, disjoint → 0.0, one-chunk-diff → high).
- `store.list_blobs()`: enumerate stored blobs so ingest can find the most
  similar existing photo and link instead of re-storing.

## What's deferred (needs codecs, not in this sandbox)

- The AV1 encoder and JPEG-XL lossless transcoder behind the
  `media-proxy` feature (libaom/SVT-AV1, jxl). Interface is in place in
  `store.rs`; wiring the codecs is the media phase.
