/*
 * bpe_train.c — Byte-level BPE tokenizer trainer
 *
 * Walks a corpus directory, pre-tokenizes by character class (alpha,
 * whitespace, other), trains byte-level BPE merges, and writes a
 * compressed tokenizer.bin containing the merge rules.
 *
 * Build:
 *   gcc -std=c99 -O3 -Wall -Wextra -o bpe_train bpe_train.c -lz
 *
 * Usage:
 *   ./bpe_train <corpus_dir> <tokenizer.bin> [num_merges]
 *     num_merges: number of merge rules to learn (default 8192)
 *     final vocab size = 256 + num_merges
 *
 * File format:
 *   Header (40 bytes):
 *     char[8]   magic        = "BPETK01\0"
 *     uint32    version      = 1
 *     uint32    vocab_size   = 256 + num_merges_actual
 *     uint32    num_merges
 *     uint64    corpus_bytes (informational)
 *     uint64    payload_uncompressed_size
 *     uint64    payload_compressed_size
 *
 *   Payload (zlib level 9):
 *     For each merge in order (new_id = 256 + index):
 *       uint32 a
 *       uint32 b
 *
 * Algorithm:
 *   1. Pre-tokenize corpus into character-class chunks.
 *   2. Deduplicate chunks into a word table (chunk -> count).
 *   3. Each chunk starts as a sequence of bytes (token IDs 0..255).
 *   4. For num_merges iterations:
 *        - Count adjacent pair frequencies across all unique chunks,
 *          weighted by chunk count.
 *        - Pick the pair with highest weighted count.
 *        - Add merge rule (a, b) -> next_id.
 *        - Apply the merge in all chunks containing it.
 *   5. Serialize merge rules.
 */

#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <stdbool.h>
#include <ctype.h>
#include <dirent.h>
#include <sys/stat.h>
#include <sys/mman.h>
#include <fcntl.h>
#include <unistd.h>
#include <time.h>
#include <zlib.h>

#define BPE_MAGIC           "BPETK01"
#define BPE_MAGIC_LEN       8
#define BPE_VERSION         1u
#define BPE_BASE_VOCAB      256u
#define DEFAULT_NUM_MERGES  8192u

/* ============================================================== */
/* FNV-1a hash                                                     */
/* ============================================================== */

static uint64_t fnv1a(const void *data, size_t len) {
    const uint8_t *p = (const uint8_t *)data;
    uint64_t h = 0xcbf29ce484222325ULL;
    for (size_t i = 0; i < len; i++) { h ^= p[i]; h *= 0x100000001b3ULL; }
    return h;
}

/* ============================================================== */
/* Character class pre-tokenization                                */
/* ============================================================== */

typedef enum { CC_ALPHA, CC_WHITE, CC_OTHER } char_class_t;

static char_class_t classify(uint8_t c) {
    if ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
        (c >= '0' && c <= '9') || c == '_') return CC_ALPHA;
    if (c == ' ' || c == '\t' || c == '\n' || c == '\r') return CC_WHITE;
    return CC_OTHER;
}

/* ============================================================== */
/* Word table: deduplicated chunks with counts                     */
/* ============================================================== */

typedef struct {
    uint32_t *tokens;   /* current token sequence (starts as bytes) */
    uint32_t  len;
    uint32_t  cap;
    uint64_t  count;
} word_t;

typedef struct {
    word_t   *words;
    uint32_t  size;
    uint32_t  cap;
    uint32_t *ht_buckets;   /* index into words[], UINT32_MAX = empty */
    uint64_t *ht_hashes;
    uint32_t  ht_cap;
} word_table_t;

static void wt_init(word_table_t *wt) {
    wt->size = 0;
    wt->cap = 65536;
    wt->words = calloc(wt->cap, sizeof(word_t));
    wt->ht_cap = 65536;
    wt->ht_buckets = malloc(wt->ht_cap * sizeof(uint32_t));
    wt->ht_hashes = calloc(wt->ht_cap, sizeof(uint64_t));
    if (!wt->words || !wt->ht_buckets || !wt->ht_hashes) {
        perror("wt_init"); exit(1);
    }
    for (uint32_t i = 0; i < wt->ht_cap; i++) wt->ht_buckets[i] = UINT32_MAX;
}

static void wt_grow_ht(word_table_t *wt) {
    uint32_t new_cap = wt->ht_cap * 2;
    uint32_t *new_buckets = malloc(new_cap * sizeof(uint32_t));
    uint64_t *new_hashes = calloc(new_cap, sizeof(uint64_t));
    if (!new_buckets || !new_hashes) { perror("wt_grow_ht"); exit(1); }
    for (uint32_t i = 0; i < new_cap; i++) new_buckets[i] = UINT32_MAX;
    uint32_t mask = new_cap - 1;
    for (uint32_t i = 0; i < wt->ht_cap; i++) {
        if (wt->ht_buckets[i] == UINT32_MAX) continue;
        uint32_t idx = (uint32_t)(wt->ht_hashes[i] & mask);
        while (new_buckets[idx] != UINT32_MAX) idx = (idx + 1) & mask;
        new_buckets[idx] = wt->ht_buckets[i];
        new_hashes[idx]  = wt->ht_hashes[i];
    }
    free(wt->ht_buckets); free(wt->ht_hashes);
    wt->ht_buckets = new_buckets;
    wt->ht_hashes = new_hashes;
    wt->ht_cap = new_cap;
}

static void wt_add(word_table_t *wt, const uint8_t *bytes, uint32_t len) {
    if (len == 0) return;
    if (wt->size * 4 >= wt->ht_cap * 3) wt_grow_ht(wt);
    uint64_t hash = fnv1a(bytes, len);
    uint32_t mask = wt->ht_cap - 1;
    uint32_t idx = (uint32_t)(hash & mask);

    while (wt->ht_buckets[idx] != UINT32_MAX) {
        if (wt->ht_hashes[idx] == hash) {
            uint32_t w_idx = wt->ht_buckets[idx];
            word_t *w = &wt->words[w_idx];
            if (w->len == len) {
                bool match = true;
                for (uint32_t i = 0; i < len; i++) {
                    if (w->tokens[i] != bytes[i]) { match = false; break; }
                }
                if (match) { w->count++; return; }
            }
        }
        idx = (idx + 1) & mask;
    }

    /* New word */
    if (wt->size >= wt->cap) {
        wt->cap *= 2;
        wt->words = realloc(wt->words, wt->cap * sizeof(word_t));
        if (!wt->words) { perror("wt_add realloc"); exit(1); }
    }
    word_t *w = &wt->words[wt->size];
    w->cap = len;
    w->len = len;
    w->count = 1;
    w->tokens = malloc(len * sizeof(uint32_t));
    if (!w->tokens) { perror("wt_add tokens"); exit(1); }
    for (uint32_t i = 0; i < len; i++) w->tokens[i] = bytes[i];
    wt->ht_buckets[idx] = wt->size;
    wt->ht_hashes[idx]  = hash;
    wt->size++;
}

static void wt_free(word_table_t *wt) {
    for (uint32_t i = 0; i < wt->size; i++) free(wt->words[i].tokens);
    free(wt->words); free(wt->ht_buckets); free(wt->ht_hashes);
}

/* ============================================================== */
/* Pre-tokenize a byte buffer into word table                      */
/* ============================================================== */

static void pre_tokenize(const uint8_t *text, size_t len, word_table_t *wt) {
    size_t i = 0;
    while (i < len) {
        char_class_t cc = classify(text[i]);
        size_t start = i;
        while (i < len && classify(text[i]) == cc) i++;
        size_t chunk_len = i - start;
        /* Cap chunk size at 1024 bytes to avoid pathological inputs */
        while (chunk_len > 0) {
            uint32_t take = (uint32_t)(chunk_len > 1024 ? 1024 : chunk_len);
            wt_add(wt, text + start, take);
            start += take;
            chunk_len -= take;
        }
    }
}

/* ============================================================== */
/* File walker                                                     */
/* ============================================================== */

/* Matches files against the CORPUS_EXT environment variable (comma-separated,
 * e.g. ".c,.h,.py"), defaulting to ".md" when unset. Case-sensitive suffix match. */
static bool has_corpus_ext(const char *name) {
    const char *exts = getenv("CORPUS_EXT");
    if (!exts || !*exts) exts = ".md";
    size_t name_len = strlen(name);
    const char *p = exts;
    while (*p) {
        const char *comma = strchr(p, ',');
        size_t ext_len = comma ? (size_t)(comma - p) : strlen(p);
        if (ext_len > 0 && ext_len <= name_len &&
            memcmp(name + name_len - ext_len, p, ext_len) == 0)
            return true;
        if (!comma) break;
        p = comma + 1;
    }
    return false;
}

static void process_file(const char *path, word_table_t *wt) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return;
    struct stat st;
    if (fstat(fd, &st) < 0) { close(fd); return; }
    if (st.st_size == 0)    { close(fd); return; }
    void *p = mmap(NULL, (size_t)st.st_size, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED)    { close(fd); return; }
    pre_tokenize((const uint8_t *)p, (size_t)st.st_size, wt);
    munmap(p, (size_t)st.st_size);
    close(fd);
}

static void walk_dir(const char *dir, word_table_t *wt, uint64_t *files) {
    DIR *d = opendir(dir);
    if (!d) return;
    struct dirent *e;
    char path[4096];
    while ((e = readdir(d))) {
        if (e->d_name[0] == '.') continue;
        int n = snprintf(path, sizeof(path), "%s/%s", dir, e->d_name);
        if (n < 0 || (size_t)n >= sizeof(path)) continue;
        struct stat st;
        if (stat(path, &st) < 0) continue;
        if (S_ISDIR(st.st_mode)) walk_dir(path, wt, files);
        else if (S_ISREG(st.st_mode) && has_corpus_ext(e->d_name)) {
            process_file(path, wt);
            (*files)++;
            if (*files % 500 == 0) {
                fprintf(stderr, "  ...%llu files, %u unique chunks\r",
                        (unsigned long long)*files, wt->size);
                fflush(stderr);
            }
        }
    }
    closedir(d);
}

/* ============================================================== */
/* Pair count table: (a, b) -> weighted count                      */
/* ============================================================== */

typedef struct {
    uint64_t key;   /* (a << 32) | b; 0 means empty (since (0,0) won't occur often) */
    uint64_t count;
} pair_entry_t;

typedef struct {
    pair_entry_t *entries;
    uint32_t      capacity;
    uint32_t      size;
} pair_table_t;

static void pt_init(pair_table_t *pt) {
    pt->capacity = 1u << 16;
    pt->size = 0;
    pt->entries = calloc(pt->capacity, sizeof(pair_entry_t));
    if (!pt->entries) { perror("pt_init"); exit(1); }
}

static void pt_clear(pair_table_t *pt) {
    memset(pt->entries, 0, pt->capacity * sizeof(pair_entry_t));
    pt->size = 0;
}

static void pt_grow(pair_table_t *pt) {
    uint32_t new_cap = pt->capacity * 2;
    pair_entry_t *new_entries = calloc(new_cap, sizeof(pair_entry_t));
    if (!new_entries) { perror("pt_grow"); exit(1); }
    uint32_t mask = new_cap - 1;
    for (uint32_t i = 0; i < pt->capacity; i++) {
        if (pt->entries[i].count == 0) continue;
        uint64_t hash = fnv1a(&pt->entries[i].key, 8);
        uint32_t idx = (uint32_t)(hash & mask);
        while (new_entries[idx].count != 0) idx = (idx + 1) & mask;
        new_entries[idx] = pt->entries[i];
    }
    free(pt->entries);
    pt->entries = new_entries;
    pt->capacity = new_cap;
}

static void pt_add(pair_table_t *pt, uint32_t a, uint32_t b, uint64_t count) {
    if (pt->size * 4 >= pt->capacity * 3) pt_grow(pt);
    uint64_t key = ((uint64_t)a << 32) | (uint64_t)b;
    uint64_t hash = fnv1a(&key, 8);
    uint32_t mask = pt->capacity - 1;
    uint32_t idx = (uint32_t)(hash & mask);
    for (;;) {
        if (pt->entries[idx].count == 0) {
            pt->entries[idx].key   = key;
            pt->entries[idx].count = count;
            pt->size++;
            return;
        }
        if (pt->entries[idx].key == key) {
            pt->entries[idx].count += count;
            return;
        }
        idx = (idx + 1) & mask;
    }
}

static bool pt_find_max(const pair_table_t *pt,
                        uint32_t *out_a, uint32_t *out_b, uint64_t *out_count)
{
    uint64_t best_count = 0;
    uint64_t best_key = 0;
    for (uint32_t i = 0; i < pt->capacity; i++) {
        if (pt->entries[i].count > best_count) {
            best_count = pt->entries[i].count;
            best_key   = pt->entries[i].key;
        }
    }
    if (best_count == 0) return false;
    *out_a = (uint32_t)(best_key >> 32);
    *out_b = (uint32_t)(best_key & 0xFFFFFFFFu);
    *out_count = best_count;
    return true;
}

/* ============================================================== */
/* Counting and merging                                            */
/* ============================================================== */

static void count_all_pairs(const word_table_t *wt, pair_table_t *pt) {
    pt_clear(pt);
    for (uint32_t i = 0; i < wt->size; i++) {
        const word_t *w = &wt->words[i];
        for (uint32_t j = 0; j + 1 < w->len; j++) {
            pt_add(pt, w->tokens[j], w->tokens[j + 1], w->count);
        }
    }
}

static void word_apply_merge(word_t *w, uint32_t a, uint32_t b, uint32_t new_id) {
    if (w->len < 2) return;
    uint32_t write = 0, read = 0;
    while (read < w->len) {
        if (read + 1 < w->len && w->tokens[read] == a && w->tokens[read + 1] == b) {
            w->tokens[write++] = new_id;
            read += 2;
        } else {
            w->tokens[write++] = w->tokens[read++];
        }
    }
    w->len = write;
}

/* ============================================================== */
/* Serialization                                                   */
/* ============================================================== */

typedef struct { uint32_t a, b; } merge_rule_t;

static void serialize_tokenizer(const char *path,
                                const merge_rule_t *merges,
                                uint32_t num_merges,
                                uint64_t corpus_bytes)
{
    size_t payload_size = (size_t)num_merges * 2 * sizeof(uint32_t);
    uint8_t *payload = malloc(payload_size);
    if (!payload) { perror("payload malloc"); exit(1); }
    uint32_t *p32 = (uint32_t *)payload;
    for (uint32_t i = 0; i < num_merges; i++) {
        p32[i * 2 + 0] = merges[i].a;
        p32[i * 2 + 1] = merges[i].b;
    }

    uLongf cap = compressBound((uLong)payload_size);
    uint8_t *compressed = malloc(cap);
    if (!compressed) { perror("compressed malloc"); exit(1); }
    uLongf csize = cap;
    int rc = compress2(compressed, &csize, payload, (uLong)payload_size, 9);
    if (rc != Z_OK) { fprintf(stderr, "compress2 failed: %d\n", rc); exit(1); }

    FILE *f = fopen(path, "wb");
    if (!f) { perror("fopen"); exit(1); }
    fwrite(BPE_MAGIC, 1, BPE_MAGIC_LEN, f);
    uint32_t version    = BPE_VERSION;
    uint32_t vocab_size = BPE_BASE_VOCAB + num_merges;
    uint32_t nm         = num_merges;
    uint64_t cb         = corpus_bytes;
    uint64_t uncompressed = payload_size;
    uint64_t comp_sz      = csize;
    fwrite(&version,      4, 1, f);
    fwrite(&vocab_size,   4, 1, f);
    fwrite(&nm,           4, 1, f);
    fwrite(&cb,           8, 1, f);
    fwrite(&uncompressed, 8, 1, f);
    fwrite(&comp_sz,      8, 1, f);
    fwrite(compressed, 1, csize, f);
    fclose(f);

    fprintf(stderr, "Wrote %s: %.2f KB compressed (%.2f KB raw, %.2fx)\n",
            path,
            (csize + 40) / 1024.0,
            (payload_size + 40) / 1024.0,
            payload_size / (double)csize);

    free(payload);
    free(compressed);
}

/* ============================================================== */
/* Main                                                            */
/* ============================================================== */

static double secs_since(clock_t t0) {
    return (double)(clock() - t0) / (double)CLOCKS_PER_SEC;
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr,
            "Usage: %s <corpus_dir> <tokenizer.bin> [num_merges]\n"
            "  num_merges: number of merges to learn (default %u)\n"
            "  final vocab size = 256 + num_merges\n",
            argv[0], DEFAULT_NUM_MERGES);
        return 1;
    }
    const char *corpus_dir = argv[1];
    const char *out_path   = argv[2];
    uint32_t num_merges = DEFAULT_NUM_MERGES;
    if (argc >= 4) {
        char *endp = NULL;
        long parsed = strtol(argv[3], &endp, 10);
        if (endp == argv[3] || *endp != '\0' || parsed <= 0 || parsed > (1L << 24)) {
            fprintf(stderr,
                "Error: could not parse num_merges from '%s'.\n"
                "Usage: %s <corpus_dir> <tokenizer.bin> [num_merges]\n"
                "  bpe_train takes 2 or 3 args; it produces tokenizer.bin only.\n"
                "  ngram_train is the separate step that produces model.bin:\n"
                "    ./ngram_train <corpus_dir> <tokenizer.bin> <model.bin> [max_n]\n",
                argv[3], argv[0]);
            return 1;
        }
        num_merges = (uint32_t)parsed;
    }
    if (argc > 4) {
        fprintf(stderr, "Warning: extra arguments ignored.\n");
    }

    fprintf(stderr, "Corpus: %s\n", corpus_dir);
    fprintf(stderr, "Output: %s\n", out_path);
    fprintf(stderr, "Target merges: %u (final vocab: %u)\n\n",
            num_merges, BPE_BASE_VOCAB + num_merges);

    clock_t t0 = clock();

    /* [1/3] Pre-tokenize and deduplicate */
    fprintf(stderr, "[1/3] Walking corpus and pre-tokenizing...\n");
    word_table_t wt;
    wt_init(&wt);
    uint64_t files = 0;
    walk_dir(corpus_dir, &wt, &files);

    uint64_t total_occurrences = 0;
    uint64_t total_bytes = 0;
    for (uint32_t i = 0; i < wt.size; i++) {
        total_occurrences += wt.words[i].count;
        total_bytes       += wt.words[i].count * wt.words[i].len;
    }
    fprintf(stderr,
        "      %llu files, %u unique chunks, %llu occurrences, %llu bytes, %.2fs\n",
        (unsigned long long)files, wt.size,
        (unsigned long long)total_occurrences,
        (unsigned long long)total_bytes,
        secs_since(t0));

    if (wt.size == 0) {
        fprintf(stderr, "No content found.\n"); return 1;
    }

    /* [2/3] Train BPE */
    fprintf(stderr, "[2/3] Training BPE...\n");
    clock_t t1 = clock();
    merge_rule_t *merges = malloc((size_t)num_merges * sizeof(merge_rule_t));
    if (!merges) { perror("merges malloc"); exit(1); }
    pair_table_t pt;
    pt_init(&pt);
    uint32_t next_id = BPE_BASE_VOCAB;
    uint32_t actual_merges = 0;

    for (uint32_t m = 0; m < num_merges; m++) {
        count_all_pairs(&wt, &pt);
        uint32_t best_a, best_b;
        uint64_t best_count;
        if (!pt_find_max(&pt, &best_a, &best_b, &best_count)) break;
        if (best_count < 2) break;

        merges[m].a = best_a;
        merges[m].b = best_b;
        actual_merges++;

        for (uint32_t i = 0; i < wt.size; i++) {
            word_apply_merge(&wt.words[i], best_a, best_b, next_id);
        }
        next_id++;

        if ((m + 1) % 64 == 0 || m + 1 == num_merges) {
            fprintf(stderr, "  ... %u/%u merges, last pair count: %llu, %.1fs\r",
                    m + 1, num_merges,
                    (unsigned long long)best_count,
                    secs_since(t1));
            fflush(stderr);
        }
    }
    fprintf(stderr, "\n      %u merges complete in %.2fs\n",
            actual_merges, secs_since(t1));

    /* [3/3] Serialize */
    fprintf(stderr, "[3/3] Writing tokenizer...\n");
    serialize_tokenizer(out_path, merges, actual_merges, total_bytes);

    fprintf(stderr, "\nTotal: %.2fs\n", secs_since(t0));

    free(merges);
    free(pt.entries);
    wt_free(&wt);
    return 0;
}
