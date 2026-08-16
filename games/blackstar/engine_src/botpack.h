/* botpack.h — read blackstar-rs's `bots.pack` from C.
 *
 * WHY THIS EXISTS
 * ---------------
 * `imported_models.h` lists 1,012 `data/imported_*.iqm` and NOT ONE of them is
 * in the repo — checked, all 1,012 missing. Only 21 survive under
 * `data/examples/`. So the C engine's imported roster currently loads nothing.
 *
 * Meanwhile blackstar-rs carries 174 chassis as quantised fold strips, and all
 * 173 fleet chassis were generated FROM models on that same wanted list. So
 * reading bots.pack does not duplicate anything the C engine has — it gives the
 * C engine 173 models it wants and cannot currently load, at 33% of the size
 * the .iqm would have been.
 *
 * THE FORMAT (written by tools/pack_bots.py, mirrored in botpack.rs)
 * ------------------------------------------------------------------
 *   "BSPK"  u16 version  u16 poses  f32 lo  f32 hi
 *   u32 nranges, then nranges x (u32 start, u32 len)   <- float32 byte ranges
 *   poses x u32 original buffer length
 *   poses x (u32 qlen, u32 rlen, zlib(u16 stream), zlib(rest))
 *
 * Only the FLOAT ranges are quantised. Everything else — crucially the u32
 * triangle indices — travels verbatim, because quantising those through a
 * 16-bit range produces indices like 929,169,408 and the loader walks off the
 * end of the vertex array. That was a real bug, not a hypothetical.
 *
 * ONE BOX PER CHASSIS
 * -------------------
 * `lo`/`hi` cover every pose in the strip, not each pose separately. The fold
 * interpolates vertex i of pose k toward vertex i of pose k+1, so the same
 * world position must decode to the same value in every pose. A per-pose box
 * compresses a little better and warps the fold — correct at both ends, wrong
 * in the middle.
 *
 * NO NEW DEPENDENCY: inflate comes from physfs's bundled miniz, which this
 * engine already compiles.
 */
#ifndef BOTPACK_H
#define BOTPACK_H

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define BOTPACK_MAGIC 0x4B505342u /* "BSPK" little-endian */
#define BOTPACK_VERSION 1

typedef struct {
    uint8_t *data;   /* decoded float32 buffer, caller frees */
    uint32_t len;
} botpack_buf;

/* Read little-endian scalars without assuming host order or alignment. */
static uint16_t bp_u16(const uint8_t *p) { return (uint16_t)(p[0] | (p[1] << 8)); }
static uint32_t bp_u32(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}
static float bp_f32(const uint8_t *p) {
    uint32_t u = bp_u32(p);
    float f;
    memcpy(&f, &u, 4);   /* not a cast: type-punning through a pointer is UB */
    return f;
}

/* One-shot zlib inflate onto a buffer of known size.
 *
 * physfs bundles the INFLATE-ONLY subset of miniz, which has tinfl_decompress
 * but not the mem_to_mem convenience wrapper — so this is that wrapper. Written
 * against the size we already know from the pack header, which means a stream
 * that decompresses to the wrong length is caught here rather than becoming a
 * short read nobody notices.
 */
static int bp_inflate(const uint8_t *in, size_t in_len, uint8_t *out, size_t out_len) {
    tinfl_decompressor d;
    tinfl_init(&d);
    size_t in_n = in_len, out_n = out_len;
    tinfl_status st = tinfl_decompress(&d, in, &in_n, out, out, &out_n,
                                       TINFL_FLAG_PARSE_ZLIB_HEADER |
                                       TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF);
    /* Exactly DONE and exactly the expected length. A truncated pack that
     * happens to inflate part-way would otherwise leave a plausible mesh. */
    return st == TINFL_STATUS_DONE && out_n == out_len;
}

/* Decode pose `idx` from a pack already in memory.
 *
 * Returns a zeroed buf on any malformed input rather than a partial one — a
 * half-decoded vertex buffer renders as geometry and hides the failure.
 */
static botpack_buf botpack_decode(const uint8_t *blob, size_t n, unsigned idx) {
    botpack_buf out = {NULL, 0};
    if (n < 20 || bp_u32(blob) != BOTPACK_MAGIC) return out;
    if (bp_u16(blob + 4) != BOTPACK_VERSION) return out;   /* refuse, never guess */

    unsigned poses = bp_u16(blob + 6);
    if (idx >= poses) return out;
    float lo = bp_f32(blob + 8), hi = bp_f32(blob + 12);
    float span = (hi > lo) ? (hi - lo) : 1.0f;   /* degenerate model: match the packer */

    size_t off = 16;
    if (off + 4 > n) return out;
    uint32_t nranges = bp_u32(blob + off); off += 4;
    if (nranges > 4096 || off + (size_t)nranges * 8 > n) return out;
    const uint8_t *ranges = blob + off;
    off += (size_t)nranges * 8;

    if (off + (size_t)poses * 4 > n) return out;
    const uint8_t *lens = blob + off;
    off += (size_t)poses * 4;

    for (unsigned i = 0; i < poses; i++) {
        if (off + 8 > n) return out;
        uint32_t qlen = bp_u32(blob + off), rlen = bp_u32(blob + off + 4);
        off += 8;
        if (off + (size_t)qlen + rlen > n) return out;
        if (i != idx) { off += qlen + rlen; continue; }

        uint32_t want = bp_u32(lens + i * 4);
        uint8_t *buf = (uint8_t *)calloc(want ? want : 1, 1);
        if (!buf) return out;

        /* The non-float bytes first: they are the frame the floats sit in. */
        if (!bp_inflate(blob + off + qlen, rlen, buf, want)) {
            free(buf);
            return out;
        }

        /* Then the quantised floats back into their ranges. */
        size_t qcount = 0;
        for (uint32_t r = 0; r < nranges; r++) qcount += bp_u32(ranges + r * 8 + 4) / 4;
        uint16_t *q = (uint16_t *)malloc(qcount * 2 ? qcount * 2 : 1);
        if (!q) { free(buf); return out; }
        if (!bp_inflate(blob + off, qlen, (uint8_t *)q, qcount * 2)) {
            free(q); free(buf);
            return out;
        }

        size_t k = 0;
        for (uint32_t r = 0; r < nranges; r++) {
            uint32_t st = bp_u32(ranges + r * 8), ln = bp_u32(ranges + r * 8 + 4);
            if ((size_t)st + ln > want) { free(q); free(buf); return out; }
            for (uint32_t j = 0; j < ln / 4; j++, k++) {
                float v = lo + ((float)q[k] / 65535.0f) * span;
                memcpy(buf + st + j * 4, &v, 4);
            }
        }
        free(q);
        out.data = buf;
        out.len = want;
        return out;
    }
    return out;
}

/* Convenience: read `<chassis>/bots.pack` and decode one pose. */
static botpack_buf botpack_load(const char *chassis_dir, unsigned pose) {
    botpack_buf out = {NULL, 0};
    char path[1024];
    snprintf(path, sizeof path, "%s/bots.pack", chassis_dir);
    FILE *f = fopen(path, "rb");
    if (!f) return out;
    fseek(f, 0, SEEK_END);
    long n = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (n <= 0) { fclose(f); return out; }
    uint8_t *blob = (uint8_t *)malloc((size_t)n);
    if (!blob) { fclose(f); return out; }
    if (fread(blob, 1, (size_t)n, f) != (size_t)n) { free(blob); fclose(f); return out; }
    fclose(f);
    out = botpack_decode(blob, (size_t)n, pose);
    free(blob);
    return out;
}

#endif /* BOTPACK_H */
