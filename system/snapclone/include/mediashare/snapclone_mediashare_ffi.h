#ifndef SNAPCLONE_MEDIASHARE_FFI_H
#define SNAPCLONE_MEDIASHARE_FFI_H

/* snapclone-mediashare — shared-conversation distribution for group media.
 *
 * Two layers, both codec- and transport-agnostic:
 *   - content-addressed chunking + dedup: a blob becomes a list of SHA-256-
 *     addressed chunks; members fetch only what they lack, so re-sent media and
 *     media sharing blocks cost almost nothing in a shared conversation.
 *   - multiway key distribution: one per-media content key, wrapped to every
 *     group member with ephemeral X25519 + ChaCha20-Poly1305.
 */

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ─── Content-addressed chunking (shared-conversation dedup) ──────────────────
 *
 * SEND:    sc_chunk_store_add_blob  (chunk + store locally + get manifest to broadcast)
 *          sc_chunk_store_get       (serve a requested chunk by id)
 * RECEIVE: sc_chunk_store_missing   (which chunk ids to fetch)
 *          sc_chunk_store_insert_verified (accept a fetched chunk; hash-checked)
 *          sc_chunk_store_reassemble (rebuild the blob)
 * A chunk id is 32 bytes (SHA-256). Opaque handle. */
typedef struct ChunkStore ChunkStore;

ChunkStore *sc_chunk_store_new(void);
void sc_chunk_store_free(ChunkStore *h);
/* Chunks currently stored (0 on null). */
int sc_chunk_store_len(const ChunkStore *h);

/* SEND: chunk `data`, store the chunks here, write the encoded manifest to
 * out_manifest. chunk_size 0 = default (16 KiB). 0; -1 (null); -2 (manifest
 * buffer too small — out_len = required). */
int sc_chunk_store_add_blob(ChunkStore *h, const uint8_t *data, size_t len, size_t chunk_size,
                            uint8_t *out_manifest, size_t out_cap, size_t *out_len);

/* RECEIVE: distinct chunk ids in the manifest this store lacks, written 32 bytes
 * each into out_ids (capacity max_ids ids); count to out_count. 0; -1 (null/bad
 * manifest); -2 (out_ids too small — out_count = number needed). */
int sc_chunk_store_missing(const ChunkStore *h, const uint8_t *manifest, size_t manifest_len,
                           uint8_t *out_ids, size_t max_ids, size_t *out_count);

/* Insert a fetched chunk, verifying it hashes to id32. 1 accepted, 0 rejected/null. */
int sc_chunk_store_insert_verified(ChunkStore *h, const uint8_t *id32,
                                   const uint8_t *bytes, size_t len);

/* Serve a stored chunk by id. 0; -1 (null); -2 (out too small — out_len = size);
 * -3 (not found). */
int sc_chunk_store_get(const ChunkStore *h, const uint8_t *id32,
                       uint8_t *out, size_t out_cap, size_t *out_len);

/* Rebuild the blob the manifest describes from this store. 0; -1 (null/bad
 * manifest); -2 (out too small — out_len = total size); -3 (a chunk is missing —
 * call sc_chunk_store_missing and fetch first). */
int sc_chunk_store_reassemble(const ChunkStore *h, const uint8_t *manifest, size_t manifest_len,
                              uint8_t *out, size_t out_cap, size_t *out_len);

/* ─── Multiway key distribution ──────────────────────────────────────────────
 *
 * Encrypt the media once with a content key; wrap that key for each member.
 * recipients = recipient_count X25519 public keys (32 bytes each, concatenated). */
#define SC_MEDIA_CONTENT_KEY_LEN 32

/* Write a fresh random 32-byte content key. */
void sc_media_content_key_new(uint8_t *out_key32);

/* Wrap content_key32 for each recipient. 0; -1 (null); -2 (out too small —
 * out_len = required). */
int sc_media_keyshare_seal(const uint8_t *content_key32,
                           const uint8_t *recipients, size_t recipient_count,
                           uint8_t *out, size_t out_cap, size_t *out_len);

/* Recover the content key with your X25519 secret + public key. 0; -1 (null/bad
 * envelope); -3 (no wrap for you / decryption failed). */
int sc_media_keyshare_open(const uint8_t *envelope, size_t env_len,
                           const uint8_t *my_secret32, const uint8_t *my_pub32,
                           uint8_t *out_key32);

/* Seal a media blob with the content key (ChaCha20-Poly1305). Writes
 * nonce||ciphertext to out; output size is always len + 28 (12 nonce + 16 tag).
 * 0; -1 (null); -2 (out too small — out_len = required). This is the bulk media
 * encryption; chunk the RESULT (sc_chunk_store_add_blob), not the plaintext. */
int sc_media_seal(const uint8_t *content_key32, const uint8_t *data, size_t len,
                  uint8_t *out, size_t out_cap, size_t *out_len);

/* Open a blob sealed by sc_media_seal. Writes plaintext (len - 28 bytes) to out.
 * 0; -1 (null); -2 (out too small — out_len = required); -3 (wrong key/tampered).
 * Sizing out_cap = len always suffices. */
int sc_media_open(const uint8_t *content_key32, const uint8_t *sealed, size_t len,
                  uint8_t *out, size_t out_cap, size_t *out_len);

/* ─── Metadata stripping (privacy) ────────────────────────────────────────────
 *
 * Remove EXIF/GPS, XMP/IPTC, comments, timestamps and embedded thumbnails from
 * an image before it leaves the device, structurally (no pixel decode). JPEG and
 * PNG; unknown formats pass through. Strip on capture/select, before chunking. */
#define SC_IMAGE_FORMAT_UNKNOWN 0
#define SC_IMAGE_FORMAT_JPEG    1
#define SC_IMAGE_FORMAT_PNG     2

/* Detect container from magic bytes (SC_IMAGE_FORMAT_*). */
int sc_image_detect_format(const uint8_t *data, size_t len);

/* Strip metadata into out. Returns 0 (JPEG/PNG cleaned) or 1 (unknown — out is a
 * copy of the input); -1 (null); -2 (out too small — out_len = required size).
 * out_removed (segment count) and out_format may be NULL. keep_color_profile != 0
 * retains the ICC profile. */
int sc_image_strip_metadata(const uint8_t *data, size_t len, int keep_color_profile,
                            uint8_t *out, size_t out_cap, size_t *out_len,
                            size_t *out_removed, int *out_format);

#ifdef __cplusplus
} // extern "C"
#endif

#endif // SNAPCLONE_MEDIASHARE_FFI_H
