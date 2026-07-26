/* ctex.h -- CTEX compressed-texture decoder (unpacks on load to RGBA).
 *
 * Mirrors tools/ctex.py. Uses the engine's existing tinfl (miniz) inflate, so no
 * new dependency. Call ex_ctex_decode() on a file's bytes; if it's CTEX it fills
 * an RGBA buffer you can hand straight to glTexImage2D. Returns NULL if the blob
 * isn't CTEX (so the caller falls through to stb_image for .png/.jpg).
 *
 * Format (little-endian): see tools/ctex.py.
 */
#ifndef EX_CTEX_H
#define EX_CTEX_H
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* the engine ships this inflate (inflate-only miniz subset). */
#include "physfs_miniz.h"   /* tinfl_decompress, TINFL_FLAG_PARSE_ZLIB_HEADER */

#define EX_CTEX_F_PALETTE 1u
#define EX_CTEX_F_NORMAL  2u

/* one-shot zlib inflate into a caller-sized buffer (we always know out size). */
static int ex_ctex__inflate(const uint8_t *in, size_t in_len,
                            uint8_t *out, size_t out_len)
{
    tinfl_decompressor d; tinfl_init(&d);
    size_t in_avail = in_len, out_avail = out_len;
    tinfl_status s = tinfl_decompress(&d, in, &in_avail, out, out, &out_avail,
                                      TINFL_FLAG_PARSE_ZLIB_HEADER);
    /* DONE means the whole stream decoded; out_avail is bytes written this call */
    return (s == TINFL_STATUS_DONE && out_avail == out_len) ? 0 : -1;
}

static inline uint32_t ex_ctex__u32(const uint8_t *p)
{ return (uint32_t)p[0] | ((uint32_t)p[1]<<8) | ((uint32_t)p[2]<<16) | ((uint32_t)p[3]<<24); }
static inline uint16_t ex_ctex__u16(const uint8_t *p)
{ return (uint16_t)((uint16_t)p[0] | ((uint16_t)p[1]<<8)); }

/* Decode a CTEX blob to a freshly malloc'd RGBA buffer.
 * Returns 1 and sets *out/*w/*h on success; 0 if not CTEX; -1 on a CTEX that
 * failed to decode. Caller frees *out. */
static int ex_ctex_decode(const uint8_t *blob, size_t len,
                          uint8_t **out_rgba, int *out_w, int *out_h)
{
    if (len < 12 || memcmp(blob, "CTEX", 4) != 0) return 0;   /* not ours */
    uint8_t flags = blob[5];
    int w = ex_ctex__u16(blob + 6);
    int h = ex_ctex__u16(blob + 8);
    int ncol = ex_ctex__u16(blob + 10);
    size_t off = 12;
    size_t npix = (size_t)w * (size_t)h;
    uint8_t *rgba = (uint8_t*)malloc(npix * 4);
    if (!rgba) return -1;

    if (flags & EX_CTEX_F_PALETTE) {
        if (off + (size_t)ncol*4 + 4 > len) { free(rgba); return -1; }
        const uint8_t *pal = blob + off; off += (size_t)ncol*4;
        uint32_t idxlen = ex_ctex__u32(blob + off); off += 4;
        if (off + idxlen > len) { free(rgba); return -1; }
        uint8_t *idx = (uint8_t*)malloc(npix);
        if (!idx) { free(rgba); return -1; }
        if (ex_ctex__inflate(blob + off, idxlen, idx, npix) != 0) { free(idx); free(rgba); return -1; }
        for (size_t i = 0; i < npix; i++) {
            uint8_t k = idx[i]; if (k >= ncol) k = 0;
            const uint8_t *c = pal + (size_t)k*4;
            rgba[i*4+0]=c[0]; rgba[i*4+1]=c[1]; rgba[i*4+2]=c[2]; rgba[i*4+3]=c[3];
        }
        free(idx);
    } else {
        if (off + 4 > len) { free(rgba); return -1; }
        uint32_t rawlen = ex_ctex__u32(blob + off); off += 4;
        if (off + rawlen > len) { free(rgba); return -1; }
        size_t rgb_len = npix * 3;
        uint8_t *rgb = (uint8_t*)malloc(rgb_len);
        if (!rgb) { free(rgba); return -1; }
        if (ex_ctex__inflate(blob + off, rawlen, rgb, rgb_len) != 0) { free(rgb); free(rgba); return -1; }
        for (size_t i = 0; i < npix; i++) {
            rgba[i*4+0]=rgb[i*3+0]; rgba[i*4+1]=rgb[i*3+1]; rgba[i*4+2]=rgb[i*3+2]; rgba[i*4+3]=255;
        }
        free(rgb);
    }
    *out_rgba = rgba; *out_w = w; *out_h = h;
    return 1;
}

#endif /* EX_CTEX_H */
