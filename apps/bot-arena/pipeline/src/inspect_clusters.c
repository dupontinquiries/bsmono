/*
 * inspect_clusters.c — View transition clusters with representative paragraph pairs
 *
 * Reads paragraph_vectors.bin and transitions.bin produced by sentence_pool and
 * transitions. For each cluster, prints the size and several representative
 * transition pairs (paragraphs whose transition vectors are closest to the centroid).
 *
 * Build:
 *   gcc -std=c99 -O3 -Wall -Wextra -o inspect_clusters inspect_clusters.c -lz -lm
 *
 * Usage:
 *   ./inspect_clusters <paragraph_vectors.bin> <transitions.bin> [cluster_id] [n_examples]
 *     cluster_id: if given, show only this cluster; otherwise show all
 *     n_examples: number of representative pairs per cluster (default 3)
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

#define PARAG_MAGIC        "PARAG01"
#define PARAG_MAGIC_LEN    8
#define TRANS_MAGIC        "TRANS01"
#define TRANS_MAGIC_LEN    8
#define DEFAULT_EXAMPLES   3
#define MAX_DISPLAY_CHARS  300

/* ===== Paragraph load (full with text excerpts) ===== */
typedef struct {
    uint32_t doc_id, pos_in_doc;
    char    *src_path;
    char    *text_excerpt;
    uint32_t text_len;
    float   *vector;
} para_t;

typedef struct {
    para_t *paragraphs;
    uint32_t n_paragraphs, n_documents, dim;
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

    size_t guess = (size_t)compressed * 8;
    uint8_t *ubuf = NULL; uLongf ulen = 0;
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
    ps->paragraphs = malloc(n_para * sizeof(para_t));
    for (uint32_t i = 0; i < n_para; i++) {
        para_t *pp = &ps->paragraphs[i];
        memcpy(&pp->doc_id, p, 4); p += 4;
        memcpy(&pp->pos_in_doc, p, 4); p += 4;
        uint32_t src_len; memcpy(&src_len, p, 4); p += 4;
        pp->src_path = malloc(src_len + 1);
        memcpy(pp->src_path, p, src_len);
        pp->src_path[src_len] = 0;
        p += src_len;
        memcpy(&pp->text_len, p, 4); p += 4;
        pp->text_excerpt = malloc(pp->text_len + 1);
        memcpy(pp->text_excerpt, p, pp->text_len);
        pp->text_excerpt[pp->text_len] = 0;
        p += pp->text_len;
        pp->vector = malloc(dim * sizeof(float));
        memcpy(pp->vector, p, dim * sizeof(float)); p += dim * sizeof(float);
    }
    free(ubuf);
}

/* ===== Transitions load ===== */
typedef struct {
    uint32_t from_idx;
    uint32_t cluster;
    float   *vector;
} transition_t;

typedef struct {
    transition_t *trans;
    uint32_t      n_trans;
    uint32_t      n_clusters;
    uint32_t      dim;
    float        *centroids;     /* n_clusters * dim */
    uint32_t     *cluster_sizes; /* n_clusters */
} trans_set_t;

static void trans_load(trans_set_t *ts, const char *path) {
    FILE *f = fopen(path, "rb"); if (!f) { perror("transitions"); exit(1); }
    char magic[TRANS_MAGIC_LEN];
    if (fread(magic,1,TRANS_MAGIC_LEN,f) != TRANS_MAGIC_LEN || memcmp(magic, TRANS_MAGIC, 7) != 0)
        { fprintf(stderr,"bad trans magic\n"); exit(1); }
    uint32_t version, dim, n_trans, n_clusters;
    uint64_t compressed;
    if (fread(&version,4,1,f)!=1 || fread(&dim,4,1,f)!=1 || fread(&n_trans,4,1,f)!=1 ||
        fread(&n_clusters,4,1,f)!=1 || fread(&compressed,8,1,f)!=1)
        { fprintf(stderr,"trans header\n"); exit(1); }
    (void)version;
    uint8_t *cbuf = malloc(compressed);
    if (!cbuf || fread(cbuf,1,compressed,f) != compressed) { fprintf(stderr,"trans read\n"); exit(1); }
    fclose(f);

    size_t guess = (size_t)compressed * 8;
    uint8_t *ubuf = NULL; uLongf ulen = 0;
    for (;;) {
        ubuf = realloc(ubuf, guess);
        ulen = (uLongf)guess;
        int rc = uncompress(ubuf, &ulen, cbuf, (uLong)compressed);
        if (rc == Z_OK) break;
        if (rc == Z_BUF_ERROR) { guess *= 2; continue; }
        fprintf(stderr, "trans decomp failed: %d\n", rc); exit(1);
    }
    free(cbuf);

    ts->dim = dim; ts->n_trans = n_trans; ts->n_clusters = n_clusters;
    ts->trans = malloc(n_trans * sizeof(transition_t));
    ts->centroids = malloc((size_t)n_clusters * dim * sizeof(float));
    ts->cluster_sizes = malloc(n_clusters * sizeof(uint32_t));

    uint8_t *p = ubuf;
    for (uint32_t i = 0; i < n_trans; i++) {
        transition_t *t = &ts->trans[i];
        memcpy(&t->from_idx, p, 4); p += 4;
        memcpy(&t->cluster,  p, 4); p += 4;
        t->vector = malloc(dim * sizeof(float));
        memcpy(t->vector, p, dim * sizeof(float)); p += dim * sizeof(float);
    }
    for (uint32_t k = 0; k < n_clusters; k++) {
        memcpy(&ts->cluster_sizes[k], p, 4); p += 4;
        memcpy(ts->centroids + (size_t)k * dim, p, dim * sizeof(float)); p += dim * sizeof(float);
    }
    free(ubuf);
}

/* ===== Display ===== */
static float sqr_dist(const float *a, const float *b, uint32_t dim) {
    float s = 0;
    for (uint32_t d = 0; d < dim; d++) {
        float diff = a[d] - b[d];
        s += diff * diff;
    }
    return s;
}

static float vec_magnitude(const float *v, uint32_t dim) {
    float s = 0;
    for (uint32_t d = 0; d < dim; d++) s += v[d] * v[d];
    return sqrtf(s);
}

static void print_excerpt(const char *text, uint32_t text_len, uint32_t max_chars) {
    uint32_t show = text_len > max_chars ? max_chars : text_len;
    for (uint32_t i = 0; i < show; i++) {
        char c = text[i];
        if (c == '\n') fputc(' ', stdout);
        else fputc(c, stdout);
    }
    if (text_len > max_chars) fprintf(stdout, "...");
}

static void show_cluster(const para_set_t *ps, const trans_set_t *ts,
                          uint32_t cluster_id, uint32_t n_examples) {
    fprintf(stdout, "\n========== Cluster %u ==========\n", cluster_id);
    fprintf(stdout, "Size: %u transitions\n", ts->cluster_sizes[cluster_id]);
    const float *centroid = ts->centroids + (size_t)cluster_id * ts->dim;
    fprintf(stdout, "Centroid magnitude: %.4f\n", vec_magnitude(centroid, ts->dim));

    if (ts->cluster_sizes[cluster_id] == 0) {
        fprintf(stdout, "(empty cluster)\n");
        return;
    }

    /* Find transitions in this cluster, sorted by distance to centroid */
    typedef struct { uint32_t idx; float dist; } entry_t;
    entry_t *ents = malloc(ts->cluster_sizes[cluster_id] * sizeof(entry_t));
    uint32_t n = 0;
    for (uint32_t i = 0; i < ts->n_trans; i++) {
        if (ts->trans[i].cluster != cluster_id) continue;
        ents[n].idx = i;
        ents[n].dist = sqr_dist(ts->trans[i].vector, centroid, ts->dim);
        n++;
    }

    /* Partial sort: just find the top-N closest */
    uint32_t show = n < n_examples ? n : n_examples;
    for (uint32_t k = 0; k < show; k++) {
        uint32_t best = k;
        for (uint32_t j = k + 1; j < n; j++) {
            if (ents[j].dist < ents[best].dist) best = j;
        }
        if (best != k) { entry_t tmp = ents[k]; ents[k] = ents[best]; ents[best] = tmp; }
    }

    for (uint32_t k = 0; k < show; k++) {
        uint32_t fi = ts->trans[ents[k].idx].from_idx;
        uint32_t ti = fi + 1;
        if (fi >= ps->n_paragraphs || ti >= ps->n_paragraphs) continue;
        fprintf(stdout, "\n--- example %u (distance %.4f) ---\n", k + 1, sqrtf(ents[k].dist));
        fprintf(stdout, "FROM [%s @ pos %u]: ",
                ps->paragraphs[fi].src_path, ps->paragraphs[fi].pos_in_doc);
        print_excerpt(ps->paragraphs[fi].text_excerpt, ps->paragraphs[fi].text_len, MAX_DISPLAY_CHARS);
        fprintf(stdout, "\n");
        fprintf(stdout, "TO   [@ pos %u]: ", ps->paragraphs[ti].pos_in_doc);
        print_excerpt(ps->paragraphs[ti].text_excerpt, ps->paragraphs[ti].text_len, MAX_DISPLAY_CHARS);
        fprintf(stdout, "\n");
    }
    free(ents);
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "Usage: %s <paragraph_vectors.bin> <transitions.bin> [cluster_id] [n_examples]\n", argv[0]);
        return 1;
    }
    para_set_t ps;
    trans_set_t ts;

    fprintf(stderr, "Loading paragraphs...\n");
    para_load(&ps, argv[1]);
    fprintf(stderr, "  %u paragraphs, dim=%u\n", ps.n_paragraphs, ps.dim);

    fprintf(stderr, "Loading transitions...\n");
    trans_load(&ts, argv[2]);
    fprintf(stderr, "  %u transitions, %u clusters\n", ts.n_trans, ts.n_clusters);
    if (ts.dim != ps.dim) {
        fprintf(stderr, "dim mismatch: paragraphs %u vs transitions %u\n", ps.dim, ts.dim);
        return 1;
    }

    uint32_t n_examples = (argc >= 5) ? (uint32_t)atoi(argv[4]) : DEFAULT_EXAMPLES;
    if (argc >= 4) {
        uint32_t cluster_id = (uint32_t)atoi(argv[3]);
        if (cluster_id >= ts.n_clusters) {
            fprintf(stderr, "cluster_id %u out of range (0..%u)\n", cluster_id, ts.n_clusters - 1);
            return 1;
        }
        show_cluster(&ps, &ts, cluster_id, n_examples);
    } else {
        for (uint32_t k = 0; k < ts.n_clusters; k++) {
            show_cluster(&ps, &ts, k, n_examples);
        }
    }
    return 0;
}
