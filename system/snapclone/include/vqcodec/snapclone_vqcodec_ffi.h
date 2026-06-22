/* snapclone_vqcodec_ffi.h — C FFI header for the snapclone-vqcodec Rust library.
 *
 * This file is committed to the repo so the project compiles without running
 * cargo first. The Rust side is in `rust/snapclone-vqcodec/src/ffi.rs`; keep
 * the two in sync by hand (cbindgen integration could automate this later).
 *
 * snapclone-vqcodec is the shared-codebook (vector-quantization) luma image
 * codec: both peers hold the same codebook (the lookup table), then a frame is
 * transmitted as one small index per block and rebuilt locally — the basis for
 * pushing images through very low-bandwidth links. See docs/CODEBOOK_*.
 *
 * Memory model:
 *   - The codebook is the only object the codec owns: create it with
 *     sc_vq_codebook_train() or sc_vq_codebook_from_bytes(), release it with
 *     sc_vq_codebook_free(). Everything else is caller-owned.
 *   - Variable-length outputs use the two-call pattern: call once with
 *     out == NULL to get the required length in *out_len, allocate that many
 *     bytes, then call again with the buffer. A buffer that is present but too
 *     small returns ScVqStatus_BufferTooSmall (with *out_len set to the need).
 */

#ifndef SNAPCLONE_VQCODEC_FFI_H
#define SNAPCLONE_VQCODEC_FFI_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ─── Status codes ───────────────────────────────────────────────────────── */

typedef enum ScVqStatus {
    ScVqStatus_Ok             = 0,
    ScVqStatus_InvalidInput   = 1,  /* null required pointer / bad dimension   */
    ScVqStatus_ParseError     = 2,  /* a serialized blob failed to parse       */
    ScVqStatus_BufferTooSmall = 3,  /* caller buffer too small; *out_len = need*/
    ScVqStatus_Internal       = 99,
} ScVqStatus;

/* Opaque handle to a codebook (the shared lookup table). */
typedef struct ScVqCodebook ScVqCodebook;

/* ─── Codebook lifecycle ─────────────────────────────────────────────────── */

/* Train a codebook from a single luma image (width*height bytes, row-major).
 * Returns NULL on invalid input. */
ScVqCodebook *sc_vq_codebook_train(const uint8_t *luma, size_t len,
                                   uint32_t width, uint32_t height,
                                   uint32_t block, uint32_t k,
                                   uint32_t iters, uint64_t seed);

/* Load a codebook from a blob produced by sc_vq_codebook_to_bytes().
 * Returns NULL on a bad/short blob. */
ScVqCodebook *sc_vq_codebook_from_bytes(const uint8_t *data, size_t len);

/* Release a codebook handle. NULL is a no-op. */
void sc_vq_codebook_free(ScVqCodebook *cb);

/* Bits per block index (ceil(log2(k))). 0 if cb is NULL. */
uint32_t sc_vq_codebook_index_bits(const ScVqCodebook *cb);

/* Serialize the codebook to its shareable blob. Two-call (out == NULL → size). */
ScVqStatus sc_vq_codebook_to_bytes(const ScVqCodebook *cb,
                                   uint8_t *out, size_t cap, size_t *out_len);

/* ─── Encode / decode ────────────────────────────────────────────────────── */

/* Encode a luma image (width*height bytes) against cb into a transmittable
 * frame blob. Two-call (out == NULL → size in *out_len). */
ScVqStatus sc_vq_encode(const ScVqCodebook *cb,
                        const uint8_t *luma, size_t len,
                        uint32_t width, uint32_t height,
                        uint8_t *out, size_t cap, size_t *out_len);

/* Decode a frame blob (from sc_vq_encode) against cb into luma. Two-call
 * (out == NULL → size). out_width / out_height receive the dimensions and may
 * be NULL. */
ScVqStatus sc_vq_decode(const ScVqCodebook *cb,
                        const uint8_t *enc, size_t enc_len,
                        uint8_t *out, size_t cap, size_t *out_len,
                        uint32_t *out_width, uint32_t *out_height);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* SNAPCLONE_VQCODEC_FFI_H */
