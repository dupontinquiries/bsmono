/*
 * transitions.c — Extract and cluster transition vectors
 *
 * Loads paragraph_vectors.bin, computes transition vectors T = E(s_{t+1}) - E(s_t)
 * for every consecutive paragraph pair within the same document, then k-means
 * clusters the transitions to identify recurring "reasoning moves."
 *
 * Build:
 *   gcc -std=c99 -O3 -Wall -Wextra -o transitions transitions.c -lz -lm
 *
 * Usage:
 *   ./transitions <paragraph_vectors.bin> <transitions.bin> [num_clusters] [kmeans_iters]
 *     num_clusters: number of transition clusters (default 16)
 *     kmeans_iters: max iterations (default 50)
 *
 * Output format (transitions.bin):
 *   Header (32 bytes):
 *     char[8]  magic = "TRANS01\0"
 *     uint32   version = 1
 *     uint32   dim
 *     uint32   n_transitions
 *     uint32   n_clusters
 *     uint64   payload_compressed_size
 *   Payload (zlib level 9):
 *     For each transition (n_transitions records):
 *       uint32 from_paragraph_idx
 *       uint32 cluster_id
 *       float[dim] transition_vector
 *     For each cluster (n_clusters records):
 *       uint32 size
 *       float[dim] centroid
 */

#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <stdbool.h>
#include <math.h>
#include <time.h>
#include <zlib.h>

#define TRANS_MAGIC        "TRANS01"
#define TRANS_MAGIC_LEN    8
#define TRANS_VERSION      1u
#define PARAG_MAGIC        "PARAG01"
#define PARAG_MAGIC_LEN    8
#define DEFAULT_K          16
#define DEFAULT_ITERS      50

static uint64_t rng_state[4];
static uint64_t rng_next(void) {
    uint64_t r = rng_state[1] * 5; r = ((r << 7) | (r >> 57)) * 9;
    uint64_t t = rng_state[1] << 17;
    rng_state[2] ^= rng_state[0]; rng_state[3] ^= rng_state[1];
    rng_state[1] ^= rng_state[2]; rng_state[0] ^= rng_state[3];
    rng_state[2] ^= t;
    rng_state[3] = ((rng_state[3] << 45) | (rng_state[3] >> 19));
    return r;
}
static void rng_seed(uint64_t s) {
    for (int i = 0; i < 4; i++) {
        s += 0x9E3779B97F4A7C15ULL; uint64_t z = s;
        z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ULL;
        z = (z ^ (z >> 27)) * 0x94D049BB133111EBULL;
        rng_state[i] = z ^ (z >> 31);
    }
    for (int i = 0; i < 16; i++) (void)rng_next();
}

/* ===== Load paragraph_vectors.bin ===== */
typedef struct {
    uint32_t doc_id, pos_in_doc;
    float   *vector;
} para_lite_t;

typedef struct {
    para_lite_t *paragraphs;
    uint32_t     n_paragraphs;
    uint32_t     n_documents;
    uint32_t     dim;
} para_set_t;

static void para_load(para_set_t *ps, const char *path) {
    FILE *f = fopen(path, "rb"); if (!f) { perror("paragraphs"); exit(1); }
    char magic[PARAG_MAGIC_LEN];
    if (fread(magic,1,PARAG_MAGIC_LEN,f) != PARAG_MAGIC_LEN || memcmp(magic, PARAG_MAGIC, 7) != 0)
        { fprintf(stderr,"bad para magic\n"); exit(1); }
    uint32_t version, dim;
    uint64_t compressed;
    if (fread(&version,4,1,f)!=1 || fread(&dim,4,1,f)!=1 || fread(&compressed,8,1,f)!=1)
        { fprintf(stderr,"para header\n"); exit(1); }
    (void)version;
    uint8_t *cbuf = malloc(compressed);
    if (!cbuf || fread(cbuf,1,compressed,f) != compressed) { fprintf(stderr,"para read\n"); exit(1); }
    fclose(f);
    /* Decompress to grow buffer */
    size_t guess = (size_t)compressed * 8;
    uint8_t *ubuf = NULL;
    uLongf ulen = 0;
    for (;;) {
        ubuf = realloc(ubuf, guess);
        ulen = (uLongf)guess;
        int rc = uncompress(ubuf, &ulen, cbuf, (uLong)compressed);
        if (rc == Z_OK) break;
        if (rc == Z_BUF_ERROR) { guess *= 2; continue; }
        fprintf(stderr, "para decomp failed: %d\n", rc); exit(1);
    }
    free(cbuf);

    uint8_t *p = ubuf;
    uint32_t n_para, n_doc;
    memcpy(&n_para, p, 4); p += 4;
    memcpy(&n_doc,  p, 4); p += 4;
    ps->dim = dim;
    ps->n_paragraphs = n_para;
    ps->n_documents = n_doc;
    ps->paragraphs = malloc(n_para * sizeof(para_lite_t));
    for (uint32_t i = 0; i < n_para; i++) {
        para_lite_t *pp = &ps->paragraphs[i];
        memcpy(&pp->doc_id, p, 4); p += 4;
        memcpy(&pp->pos_in_doc, p, 4); p += 4;
        uint32_t src_len; memcpy(&src_len, p, 4); p += 4;
        p += src_len;
        uint32_t text_len; memcpy(&text_len, p, 4); p += 4;
        p += text_len;
        pp->vector = malloc(dim * sizeof(float));
        memcpy(pp->vector, p, dim * sizeof(float)); p += dim * sizeof(float);
    }
    free(ubuf);
}

/* ===== Build transitions ===== */
typedef struct {
    uint32_t from_idx;
    uint32_t cluster;
    float   *vector;
} transition_t;

static uint32_t build_transitions(const para_set_t *ps, transition_t **out) {
    /* Sort paragraphs by (doc_id, pos_in_doc) — but we expect them already in order
     * since sentence_pool writes them in walk order. We'll detect pairs by checking
     * consecutive entries with same doc_id and increasing pos_in_doc. */
    uint32_t cap = ps->n_paragraphs;
    transition_t *trans = malloc(cap * sizeof(transition_t));
    uint32_t n = 0;
    for (uint32_t i = 1; i < ps->n_paragraphs; i++) {
        const para_lite_t *a = &ps->paragraphs[i - 1];
        const para_lite_t *b = &ps->paragraphs[i];
        if (a->doc_id != b->doc_id) continue;
        if (b->pos_in_doc != a->pos_in_doc + 1) continue;
        transition_t *t = &trans[n++];
        t->from_idx = i - 1;
        t->cluster = 0;
        t->vector = malloc(ps->dim * sizeof(float));
        for (uint32_t d = 0; d < ps->dim; d++) {
            t->vector[d] = b->vector[d] - a->vector[d];
        }
    }
    *out = trans;
    return n;
}

/* ===== K-means ===== */
static float sqr_dist(const float *a, const float *b, uint32_t dim) {
    float s = 0;
    for (uint32_t d = 0; d < dim; d++) {
        float diff = a[d] - b[d];
        s += diff * diff;
    }
    return s;
}

static void kmeans(transition_t *trans, uint32_t n, uint32_t dim,
                   uint32_t K, uint32_t max_iters,
                   float *centroids /* K * dim */, uint32_t *sizes /* K */)
{
    if (n == 0 || K == 0) return;
    if (K > n) K = n;

    /* k-means++ initialization */
    /* First centroid: random */
    uint32_t first = (uint32_t)(rng_next() % n);
    memcpy(centroids, trans[first].vector, dim * sizeof(float));

    float *min_dist = malloc(n * sizeof(float));
    for (uint32_t i = 0; i < n; i++) {
        min_dist[i] = sqr_dist(trans[i].vector, centroids, dim);
    }

    for (uint32_t k = 1; k < K; k++) {
        /* Pick next centroid proportional to min_dist squared (k-means++) */
        double total = 0;
        for (uint32_t i = 0; i < n; i++) total += min_dist[i];
        if (total <= 0) {
            /* All transitions identical to a chosen centroid; pick random */
            uint32_t pick = (uint32_t)(rng_next() % n);
            memcpy(centroids + (size_t)k * dim, trans[pick].vector, dim * sizeof(float));
        } else {
            double r = ((double)(rng_next() >> 11) / (double)(1ULL << 53)) * total;
            uint32_t pick = n - 1;
            for (uint32_t i = 0; i < n; i++) {
                r -= min_dist[i];
                if (r <= 0) { pick = i; break; }
            }
            memcpy(centroids + (size_t)k * dim, trans[pick].vector, dim * sizeof(float));
        }
        /* Update min_dist */
        for (uint32_t i = 0; i < n; i++) {
            float d = sqr_dist(trans[i].vector, centroids + (size_t)k * dim, dim);
            if (d < min_dist[i]) min_dist[i] = d;
        }
    }
    free(min_dist);

    /* Lloyd's iterations */
    float *new_centroids = calloc((size_t)K * dim, sizeof(float));
    uint32_t *new_sizes = calloc(K, sizeof(uint32_t));
    for (uint32_t iter = 0; iter < max_iters; iter++) {
        /* Assignment step */
        memset(new_centroids, 0, (size_t)K * dim * sizeof(float));
        memset(new_sizes, 0, K * sizeof(uint32_t));
        uint32_t changed = 0;
        for (uint32_t i = 0; i < n; i++) {
            uint32_t best = 0;
            float best_d = sqr_dist(trans[i].vector, centroids, dim);
            for (uint32_t k = 1; k < K; k++) {
                float d = sqr_dist(trans[i].vector, centroids + (size_t)k * dim, dim);
                if (d < best_d) { best_d = d; best = k; }
            }
            if (trans[i].cluster != best) changed++;
            trans[i].cluster = best;
            float *acc = new_centroids + (size_t)best * dim;
            for (uint32_t d = 0; d < dim; d++) acc[d] += trans[i].vector[d];
            new_sizes[best]++;
        }
        /* Update step */
        for (uint32_t k = 0; k < K; k++) {
            if (new_sizes[k] == 0) continue;
            float inv = 1.0f / (float)new_sizes[k];
            for (uint32_t d = 0; d < dim; d++) {
                new_centroids[(size_t)k * dim + d] *= inv;
            }
        }
        memcpy(centroids, new_centroids, (size_t)K * dim * sizeof(float));
        memcpy(sizes, new_sizes, K * sizeof(uint32_t));

        fprintf(stderr, "  iter %u/%u: %u reassignments\n", iter + 1, max_iters, changed);
        if (changed == 0) break;
    }
    free(new_centroids);
    free(new_sizes);
}

/* ===== Serialize ===== */
typedef struct { uint8_t *buf; size_t size, cap; } buf_t;
static void buf_write(buf_t *b, const void *data, size_t n) {
    if (b->size + n > b->cap) {
        while (b->size + n > b->cap) b->cap = b->cap ? b->cap * 2 : 4096;
        b->buf = realloc(b->buf, b->cap);
    }
    memcpy(b->buf + b->size, data, n);
    b->size += n;
}

static void serialize(const char *path, const transition_t *trans, uint32_t n,
                       uint32_t dim, const float *centroids, const uint32_t *sizes, uint32_t K) {
    buf_t pl = {0};
    for (uint32_t i = 0; i < n; i++) {
        buf_write(&pl, &trans[i].from_idx, 4);
        buf_write(&pl, &trans[i].cluster, 4);
        buf_write(&pl, trans[i].vector, dim * sizeof(float));
    }
    for (uint32_t k = 0; k < K; k++) {
        buf_write(&pl, &sizes[k], 4);
        buf_write(&pl, centroids + (size_t)k * dim, dim * sizeof(float));
    }

    uLongf cap = compressBound((uLong)pl.size);
    uint8_t *comp = malloc(cap);
    uLongf csize = cap;
    if (compress2(comp, &csize, pl.buf, (uLong)pl.size, 9) != Z_OK)
        { fprintf(stderr,"compress\n"); exit(1); }

    FILE *f = fopen(path, "wb"); if (!f) { perror("output"); exit(1); }
    fwrite(TRANS_MAGIC, 1, TRANS_MAGIC_LEN, f);
    uint32_t v = TRANS_VERSION;
    uint32_t nt = n;
    uint32_t nc = K;
    uint64_t cs = csize;
    fwrite(&v,4,1,f); fwrite(&dim,4,1,f); fwrite(&nt,4,1,f); fwrite(&nc,4,1,f); fwrite(&cs,8,1,f);
    fwrite(comp,1,csize,f);
    fclose(f);
    fprintf(stderr, "Wrote %s: %.2f KB compressed (%.2f KB raw)\n",
            path, csize/1024.0, pl.size/1024.0);
    free(pl.buf); free(comp);
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr,
            "Usage: %s <paragraph_vectors.bin> <transitions.bin> [num_clusters] [iters]\n",
            argv[0]);
        return 1;
    }
    uint32_t K     = (argc >= 4) ? (uint32_t)atoi(argv[3]) : DEFAULT_K;
    uint32_t iters = (argc >= 5) ? (uint32_t)atoi(argv[4]) : DEFAULT_ITERS;

    rng_seed((uint64_t)time(NULL));

    para_set_t ps;
    fprintf(stderr, "Loading paragraph vectors...\n");
    para_load(&ps, argv[1]);
    fprintf(stderr, "  %u paragraphs, %u documents, dim=%u\n",
            ps.n_paragraphs, ps.n_documents, ps.dim);

    transition_t *trans = NULL;
    uint32_t n_trans = build_transitions(&ps, &trans);
    fprintf(stderr, "Built %u transitions (consecutive within-document pairs)\n", n_trans);
    if (n_trans < 2) {
        fprintf(stderr, "Need at least 2 transitions. Are your documents single-paragraph?\n");
        return 1;
    }
    if (K > n_trans) K = n_trans;

    fprintf(stderr, "K-means clustering: K=%u, max_iters=%u\n", K, iters);
    float *centroids = calloc((size_t)K * ps.dim, sizeof(float));
    uint32_t *sizes  = calloc(K, sizeof(uint32_t));
    kmeans(trans, n_trans, ps.dim, K, iters, centroids, sizes);

    fprintf(stderr, "Cluster sizes:\n");
    for (uint32_t k = 0; k < K; k++) {
        fprintf(stderr, "  cluster %2u: %u transitions\n", k, sizes[k]);
    }

    serialize(argv[2], trans, n_trans, ps.dim, centroids, sizes, K);
    return 0;
}
