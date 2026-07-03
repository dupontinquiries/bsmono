/* snapclone_wirecodec_ffi.h — content-aware wire compression for snap payloads.
 *
 * Encode a payload before it goes on the wire, decode after it arrives. Adds a
 * 1-byte codec tag and deflates ONLY when the content class is compressible and
 * the result is actually smaller — already-compressed media (AV1/JPEG) is stored
 * verbatim, and nothing ever expands beyond +1 byte. Link libsnapclone_wirecodec.a.
 */
#ifndef SNAPCLONE_WIRECODEC_FFI_H
#define SNAPCLONE_WIRECODEC_FFI_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* result codes */
#define SC_WIRE_OK               0
#define SC_WIRE_NULL             1
#define SC_WIRE_BUFFER_TOO_SMALL 2  /* *out_len holds the required size */
#define SC_WIRE_DECODE_FAILED    3
#define SC_WIRE_INTERNAL         4

/* content classes (mirror Core::MediaType; -1 = Unknown) */
#define SC_WIRE_CONTENT_UNKNOWN  (-1)
#define SC_WIRE_CONTENT_IMAGE    0
#define SC_WIRE_CONTENT_VIDEO    1
#define SC_WIRE_CONTENT_TEXT     2
#define SC_WIRE_CONTENT_LINK     3
#define SC_WIRE_CONTENT_LOCATION 4

/* Encode `data` (content-aware compress + tag) into `out`. Worst case is
 * data_len + 1. On SC_WIRE_BUFFER_TOO_SMALL, *out_len holds the required size. */
int sc_wire_encode(int content,
                   const unsigned char *data, size_t data_len,
                   unsigned char *out, size_t out_cap, size_t *out_len);

/* Decode a wire payload back to the original bytes. Decoded size isn't known
 * until inflate runs; on BufferTooSmall, retry with a generous buffer (or the
 * known max snap size). */
int sc_wire_decode(const unsigned char *wire, size_t wire_len,
                   unsigned char *out, size_t out_cap, size_t *out_len);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* SNAPCLONE_WIRECODEC_FFI_H */
