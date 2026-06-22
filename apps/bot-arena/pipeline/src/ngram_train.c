/*
 * ngram_train.c (v2) — N-gram trainer using BPE tokenization
 *
 * Loads a tokenizer.bin produced by bpe_train, walks the corpus,
 * tokenizes via byte-level BPE, builds 1..max_n gram counts with
 * modified Kneser-Ney discounts, and writes a compressed model.bin.
 *
 * Build:
 *   gcc -std=c99 -O3 -Wall -Wextra -o ngram_train ngram_train.c -lz
 *
 * Usage:
 *   ./ngram_train <corpus_dir> <tokenizer.bin> <model.bin> [max_n]
 *     max_n: 1..5 (default 5)
 *
 * Model file format (binary, little-endian):
 *   Header (40 bytes):
 *     char[8]  magic    = "NGRAM02\0"     (v2: BPE tokens, no vocab strings)
 *     uint32   version  = 2
 *     uint32   max_n
 *     uint32   vocab_size  (= tokenizer vocab_size)
 *     uint64   total_tokens
 *     uint64   payload_uncompressed
 *     uint64   payload_compressed
 *
 *   Payload (zlib level 9):
 *     For n = 1..max_n:
 *       uint32 n
 *       uint32 entry_count
 *       For each entry:
 *         uint32[n] token_ids
 *         uint64    count
 *     Discounts (max_n records):
 *       float64 D1, D2, D3plus
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

#define NGRAM_MAGIC          "NGRAM02"
#define NGRAM_MAGIC_LEN      8
#define NGRAM_VERSION        2u
#define DEFAULT_MAX_N        5u
#define BPE_MAGIC            "BPETK01"
#define BPE_MAGIC_LEN        8
#define BPE_BASE_VOCAB       256u

#define INITIAL_NGRAM_CAP    (1u << 18)
#define INITIAL_TOKEN_CAP    (1u << 20)

/* FNV-1a */
static uint64_t fnv1a(const void *data, size_t len) {
    const uint8_t *p = (const uint8_t *)data;
    uint64_t h = 0xcbf29ce484222325ULL;
    for (size_t i = 0; i < len; i++) { h ^= p[i]; h *= 0x100000001b3ULL; }
    return h;
}

/* ============================================================== */
/* Tokenizer: load tokenizer.bin and apply BPE                     */
/* ============================================================== */

typedef struct {
    uint64_t key;      /* (a << 32) | b */
    uint32_t rank;     /* merge order */
    uint32_t new_id;   /* 0 = empty slot, real entries have new_id >= 256 */
} bpe_merge_entry_t;

typedef struct {
    uint32_t           vocab_size;
    uint32_t           num_merges;
    bpe_merge_entry_t *table;
    uint32_t           table_cap;
} bpe_tokenizer_t;

static void bpe_load(bpe_tokenizer_t *tk, const char *path) {
    FILE *f = fopen(path, "rb");
    if (!f) { perror("open tokenizer"); exit(1); }
    char magic[BPE_MAGIC_LEN];
    if (fread(magic, 1, BPE_MAGIC_LEN, f) != BPE_MAGIC_LEN ||
        memcmp(magic, BPE_MAGIC, 7) != 0)
    { fprintf(stderr, "bad tokenizer magic\n"); exit(1); }
    uint32_t version, vocab_size, num_merges;
    uint64_t corpus_bytes, uncompressed, compressed;
    if (fread(&version,      4, 1, f) != 1 ||
        fread(&vocab_size,   4, 1, f) != 1 ||
        fread(&num_merges,   4, 1, f) != 1 ||
        fread(&corpus_bytes, 8, 1, f) != 1 ||
        fread(&uncompressed, 8, 1, f) != 1 ||
        fread(&compressed,   8, 1, f) != 1)
    { fprintf(stderr, "tokenizer header read failed\n"); exit(1); }
    (void)version; (void)corpus_bytes;

    uint8_t *cbuf = malloc(compressed);
    if (!cbuf) { perror("malloc"); exit(1); }
    if (fread(cbuf, 1, compressed, f) != compressed)
    { fprintf(stderr, "tokenizer payload read failed\n"); exit(1); }
    fclose(f);

    uint8_t *ubuf = malloc(uncompressed);
    if (!ubuf) { perror("malloc"); exit(1); }
    uLongf ulen = (uLongf)uncompressed;
    if (uncompress(ubuf, &ulen, cbuf, (uLong)compressed) != Z_OK)
    { fprintf(stderr, "tokenizer decompress failed\n"); exit(1); }
    free(cbuf);

    tk->vocab_size = vocab_size;
    tk->num_merges = num_merges;
    uint32_t cap = 1;
    while (cap < num_merges * 2) cap <<= 1;
    if (cap < 1024) cap = 1024;
    tk->table_cap = cap;
    tk->table = calloc(cap, sizeof(bpe_merge_entry_t));
    if (!tk->table) { perror("table calloc"); exit(1); }

    uint32_t *p32 = (uint32_t *)ubuf;
    uint32_t mask = cap - 1;
    for (uint32_t i = 0; i < num_merges; i++) {
        uint32_t a = p32[i * 2 + 0];
        uint32_t b = p32[i * 2 + 1];
        uint64_t key = ((uint64_t)a << 32) | (uint64_t)b;
        uint64_t hash = fnv1a(&key, 8);
        uint32_t idx = (uint32_t)(hash & mask);
        while (tk->table[idx].new_id != 0) idx = (idx + 1) & mask;
        tk->table[idx].key    = key;
        tk->table[idx].rank   = i;
        tk->table[idx].new_id = BPE_BASE_VOCAB + i;
    }
    free(ubuf);

    fprintf(stderr, "Loaded tokenizer: vocab=%u, merges=%u\n",
            vocab_size, num_merges);
}

static bool bpe_lookup(const bpe_tokenizer_t *tk, uint32_t a, uint32_t b,
                       uint32_t *out_rank, uint32_t *out_new_id)
{
    uint64_t key = ((uint64_t)a << 32) | (uint64_t)b;
    uint64_t hash = fnv1a(&key, 8);
    uint32_t mask = tk->table_cap - 1;
    uint32_t idx = (uint32_t)(hash & mask);
    while (tk->table[idx].new_id != 0) {
        if (tk->table[idx].key == key) {
            *out_rank   = tk->table[idx].rank;
            *out_new_id = tk->table[idx].new_id;
            return true;
        }
        idx = (idx + 1) & mask;
    }
    return false;
}

static uint32_t bpe_apply_chunk(const bpe_tokenizer_t *tk,
                                uint32_t *tokens, uint32_t len)
{
    while (len >= 2) {
        uint32_t best_pos    = UINT32_MAX;
        uint32_t best_rank   = UINT32_MAX;
        uint32_t best_new_id = 0;
        for (uint32_t i = 0; i + 1 < len; i++) {
            uint32_t r, nid;
            if (bpe_lookup(tk, tokens[i], tokens[i + 1], &r, &nid)) {
                if (r < best_rank) {
                    best_rank = r;
                    best_pos  = i;
                    best_new_id = nid;
                }
            }
        }
        if (best_pos == UINT32_MAX) break;
        tokens[best_pos] = best_new_id;
        for (uint32_t i = best_pos + 1; i + 1 < len; i++) tokens[i] = tokens[i + 1];
        len--;
    }
    return len;
}

/* ============================================================== */
/* Char class                                                      */
/* ============================================================== */

typedef enum { CC_ALPHA, CC_WHITE, CC_OTHER } char_class_t;
static char_class_t classify(uint8_t c) {
    if ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
        (c >= '0' && c <= '9') || c == '_') return CC_ALPHA;
    if (c == ' ' || c == '\t' || c == '\n' || c == '\r') return CC_WHITE;
    return CC_OTHER;
}

/* ============================================================== */
/* Token stream                                                    */
/* ============================================================== */

typedef struct { uint32_t *ids; size_t size, cap; } tokens_t;

static void tokens_push_n(tokens_t *t, const uint32_t *src, size_t n) {
    if (t->size + n > t->cap) {
        while (t->size + n > t->cap) t->cap = t->cap ? t->cap * 2 : INITIAL_TOKEN_CAP;
        t->ids = realloc(t->ids, t->cap * sizeof(uint32_t));
        if (!t->ids) { perror("tokens_push_n"); exit(1); }
    }
    memcpy(t->ids + t->size, src, n * sizeof(uint32_t));
    t->size += n;
}

static void tokenize_bytes(const bpe_tokenizer_t *tk,
                           const uint8_t *text, size_t len, tokens_t *t)
{
    size_t i = 0;
    uint32_t chunk[2048];
    while (i < len) {
        char_class_t cc = classify(text[i]);
        size_t start = i;
        while (i < len && classify(text[i]) == cc) i++;
        size_t clen = i - start;
        while (clen > 0) {
            uint32_t take = (uint32_t)(clen > 2048 ? 2048 : clen);
            for (uint32_t k = 0; k < take; k++) chunk[k] = text[start + k];
            uint32_t new_len = bpe_apply_chunk(tk, chunk, take);
            tokens_push_n(t, chunk, new_len);
            start += take;
            clen  -= take;
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

static void process_file(const char *path, const bpe_tokenizer_t *tk, tokens_t *t) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return;
    struct stat st;
    if (fstat(fd, &st) < 0) { close(fd); return; }
    if (st.st_size == 0)    { close(fd); return; }
    void *p = mmap(NULL, (size_t)st.st_size, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED)    { close(fd); return; }
    tokenize_bytes(tk, (const uint8_t *)p, (size_t)st.st_size, t);
    munmap(p, (size_t)st.st_size);
    close(fd);
}

static void walk_dir(const char *dir, const bpe_tokenizer_t *tk,
                     tokens_t *t, uint64_t *files)
{
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
        if (S_ISDIR(st.st_mode)) walk_dir(path, tk, t, files);
        else if (S_ISREG(st.st_mode) && has_corpus_ext(e->d_name)) {
            process_file(path, tk, t);
            (*files)++;
            if (*files % 200 == 0) {
                fprintf(stderr, "  ...%llu files, %zu tokens\r",
                        (unsigned long long)*files, t->size);
                fflush(stderr);
            }
        }
    }
    closedir(d);
}

/* ============================================================== */
/* N-gram hash tables                                              */
/* ============================================================== */

typedef struct {
    uint8_t  *entries;
    uint32_t  capacity;
    uint32_t  size;
    uint32_t  n;
    size_t    entry_size;
} ngram_table_t;

static inline uint8_t *ngt_slot(const ngram_table_t *tab, uint32_t idx) {
    return tab->entries + (size_t)idx * tab->entry_size;
}
static inline uint64_t *ngt_count(const ngram_table_t *tab, uint8_t *slot) {
    return (uint64_t *)(slot + tab->n * sizeof(uint32_t));
}
static inline uint64_t *ngt_hash_p(const ngram_table_t *tab, uint8_t *slot) {
    return (uint64_t *)(slot + tab->n * sizeof(uint32_t) + sizeof(uint64_t));
}

static void ngt_init(ngram_table_t *tab, uint32_t n) {
    tab->capacity   = INITIAL_NGRAM_CAP;
    tab->size       = 0;
    tab->n          = n;
    tab->entry_size = (size_t)n * sizeof(uint32_t) + 2 * sizeof(uint64_t);
    tab->entries    = calloc(tab->capacity, tab->entry_size);
    if (!tab->entries) { perror("ngt_init"); exit(1); }
}

static void ngt_free(ngram_table_t *tab) { free(tab->entries); }

static void ngt_grow(ngram_table_t *tab) {
    uint32_t new_cap = tab->capacity * 2;
    uint8_t *ne = calloc(new_cap, tab->entry_size);
    if (!ne) { perror("ngt_grow"); exit(1); }
    uint32_t mask = new_cap - 1;
    for (uint32_t i = 0; i < tab->capacity; i++) {
        uint8_t *slot = ngt_slot(tab, i);
        uint64_t c = *ngt_count(tab, slot);
        if (c == 0) continue;
        uint64_t h = *ngt_hash_p(tab, slot);
        uint32_t idx = (uint32_t)(h & mask);
        for (;;) {
            uint8_t *ns = ne + (size_t)idx * tab->entry_size;
            uint64_t *nc = (uint64_t *)(ns + tab->n * sizeof(uint32_t));
            if (*nc == 0) { memcpy(ns, slot, tab->entry_size); break; }
            idx = (idx + 1) & mask;
        }
    }
    free(tab->entries);
    tab->entries = ne;
    tab->capacity = new_cap;
}

static void ngt_add(ngram_table_t *tab, const uint32_t *tokens) {
    if (tab->size * 4 >= tab->capacity * 3) ngt_grow(tab);
    uint64_t h = fnv1a(tokens, tab->n * sizeof(uint32_t));
    uint32_t mask = tab->capacity - 1;
    uint32_t idx = (uint32_t)(h & mask);
    for (;;) {
        uint8_t *slot = ngt_slot(tab, idx);
        uint64_t *cp = ngt_count(tab, slot);
        if (*cp == 0) {
            memcpy(slot, tokens, tab->n * sizeof(uint32_t));
            *cp = 1;
            *ngt_hash_p(tab, slot) = h;
            tab->size++;
            return;
        }
        if (*ngt_hash_p(tab, slot) == h &&
            memcmp(slot, tokens, tab->n * sizeof(uint32_t)) == 0)
        { (*cp)++; return; }
        idx = (idx + 1) & mask;
    }
}

static void count_ngrams(const tokens_t *t, ngram_table_t *tabs, uint32_t max_n) {
    for (size_t i = 0; i < t->size; i++) {
        for (uint32_t n = 1; n <= max_n; n++) {
            if (i + n > t->size) break;
            ngt_add(&tabs[n - 1], &t->ids[i]);
        }
    }
}

/* ============================================================== */
/* Modified Kneser-Ney discount                                    */
/* ============================================================== */

typedef struct { double D1, D2, D3plus; } discount_t;

static discount_t compute_discount(const ngram_table_t *tab) {
    uint64_t n1=0, n2=0, n3=0, n4=0;
    for (uint32_t i = 0; i < tab->capacity; i++) {
        uint8_t *slot = ngt_slot(tab, i);
        uint64_t c = *ngt_count(tab, slot);
        if      (c == 1) n1++;
        else if (c == 2) n2++;
        else if (c == 3) n3++;
        else if (c == 4) n4++;
    }
    discount_t d = { 0.5, 1.0, 1.5 };
    if (n1 > 0 && n2 > 0) {
        double Y = (double)n1 / ((double)n1 + 2.0 * (double)n2);
        d.D1 = 1.0 - 2.0 * Y * (double)n2 / (double)n1;
        if (n2 > 0 && n3 > 0) d.D2 = 2.0 - 3.0 * Y * (double)n3 / (double)n2;
        if (n3 > 0 && n4 > 0) d.D3plus = 3.0 - 4.0 * Y * (double)n4 / (double)n3;
    }
    if (d.D1     < 0) d.D1 = 0;
    if (d.D2     < 0) d.D2 = 0;
    if (d.D3plus < 0) d.D3plus = 0;
    return d;
}

/* ============================================================== */
/* Output buffer + serialize                                       */
/* ============================================================== */

typedef struct { uint8_t *buf; size_t size, cap; } buf_t;
static void buf_write(buf_t *b, const void *data, size_t n) {
    if (b->size + n > b->cap) {
        while (b->size + n > b->cap) b->cap = b->cap ? b->cap * 2 : 4096;
        b->buf = realloc(b->buf, b->cap);
        if (!b->buf) { perror("buf_write"); exit(1); }
    }
    memcpy(b->buf + b->size, data, n);
    b->size += n;
}
static void buf_u32(buf_t *b, uint32_t v) { buf_write(b, &v, 4); }
static void buf_u64(buf_t *b, uint64_t v) { buf_write(b, &v, 8); }
static void buf_f64(buf_t *b, double v)   { buf_write(b, &v, 8); }

static void serialize_model(const char *path,
                            uint32_t vocab_size,
                            const ngram_table_t *tabs, uint32_t max_n,
                            const discount_t *discounts,
                            uint64_t total_tokens)
{
    buf_t pl = {0};
    for (uint32_t n = 1; n <= max_n; n++) {
        const ngram_table_t *tab = &tabs[n - 1];
        buf_u32(&pl, n);
        buf_u32(&pl, tab->size);
        for (uint32_t i = 0; i < tab->capacity; i++) {
            uint8_t *slot = ngt_slot(tab, i);
            uint64_t c = *ngt_count(tab, slot);
            if (c == 0) continue;
            buf_write(&pl, slot, n * sizeof(uint32_t));
            buf_u64(&pl, c);
        }
    }
    for (uint32_t n = 1; n <= max_n; n++) {
        buf_f64(&pl, discounts[n - 1].D1);
        buf_f64(&pl, discounts[n - 1].D2);
        buf_f64(&pl, discounts[n - 1].D3plus);
    }

    uLongf cap = compressBound((uLong)pl.size);
    uint8_t *comp = malloc(cap);
    if (!comp) { perror("malloc compressed"); exit(1); }
    uLongf csize = cap;
    if (compress2(comp, &csize, pl.buf, (uLong)pl.size, 9) != Z_OK)
    { fprintf(stderr, "compress2 failed\n"); exit(1); }

    FILE *f = fopen(path, "wb");
    if (!f) { perror("fopen output"); exit(1); }
    fwrite(NGRAM_MAGIC, 1, NGRAM_MAGIC_LEN, f);
    uint32_t v   = NGRAM_VERSION;
    uint32_t mn  = max_n;
    uint32_t vs  = vocab_size;
    uint64_t tt  = total_tokens;
    uint64_t un  = pl.size;
    uint64_t cs  = (uint64_t)csize;
    fwrite(&v,  4, 1, f);
    fwrite(&mn, 4, 1, f);
    fwrite(&vs, 4, 1, f);
    fwrite(&tt, 8, 1, f);
    fwrite(&un, 8, 1, f);
    fwrite(&cs, 8, 1, f);
    fwrite(comp, 1, csize, f);
    fclose(f);

    fprintf(stderr, "Wrote %s: %.2f MB compressed (%.2f MB raw, %.2fx)\n",
            path, csize / (1024.0*1024.0),
            pl.size / (1024.0*1024.0),
            pl.size / (double)csize);

    free(pl.buf);
    free(comp);
}

/* ============================================================== */
/* Main                                                            */
/* ============================================================== */

static double secs_since(clock_t t0) {
    return (double)(clock() - t0) / (double)CLOCKS_PER_SEC;
}

int main(int argc, char **argv) {
    if (argc < 4) {
        fprintf(stderr,
            "Usage: %s <corpus_dir> <tokenizer.bin> <model.bin> [max_n]\n"
            "  max_n: 1..5 (default %u)\n",
            argv[0], DEFAULT_MAX_N);
        return 1;
    }
    const char *corpus_dir = argv[1];
    const char *tk_path    = argv[2];
    const char *out_path   = argv[3];
    uint32_t max_n = (argc >= 5) ? (uint32_t)atoi(argv[4]) : DEFAULT_MAX_N;
    if (max_n < 1 || max_n > 5) { fprintf(stderr, "max_n 1..5\n"); return 1; }

    fprintf(stderr, "Corpus:    %s\n", corpus_dir);
    fprintf(stderr, "Tokenizer: %s\n", tk_path);
    fprintf(stderr, "Output:    %s\n", out_path);
    fprintf(stderr, "max_n:     %u\n\n", max_n);

    clock_t t0 = clock();

    fprintf(stderr, "[1/4] Loading tokenizer...\n");
    bpe_tokenizer_t tk;
    bpe_load(&tk, tk_path);

    fprintf(stderr, "[2/4] Walking corpus and tokenizing with BPE...\n");
    tokens_t toks = {0};
    uint64_t files = 0;
    walk_dir(corpus_dir, &tk, &toks, &files);
    fprintf(stderr, "      %llu files, %zu tokens, %.2fs\n",
            (unsigned long long)files, toks.size, secs_since(t0));
    if (toks.size == 0) {
        fprintf(stderr, "No tokens. Check corpus path.\n"); return 1;
    }

    fprintf(stderr, "[3/4] Counting n-grams 1..%u...\n", max_n);
    clock_t tc = clock();
    ngram_table_t *tabs = malloc(max_n * sizeof(ngram_table_t));
    for (uint32_t n = 1; n <= max_n; n++) ngt_init(&tabs[n - 1], n);
    count_ngrams(&toks, tabs, max_n);
    fprintf(stderr, "      counted in %.2fs\n", secs_since(tc));
    for (uint32_t n = 1; n <= max_n; n++) {
        fprintf(stderr, "        %u-grams: %u unique\n", n, tabs[n - 1].size);
    }

    discount_t *discounts = malloc(max_n * sizeof(discount_t));
    for (uint32_t n = 1; n <= max_n; n++) {
        discounts[n - 1] = compute_discount(&tabs[n - 1]);
        fprintf(stderr, "        %u-gram discounts: D1=%.4f D2=%.4f D3+=%.4f\n",
                n, discounts[n-1].D1, discounts[n-1].D2, discounts[n-1].D3plus);
    }

    fprintf(stderr, "[4/4] Serializing model...\n");
    serialize_model(out_path, tk.vocab_size, tabs, max_n, discounts,
                    (uint64_t)toks.size);

    fprintf(stderr, "\nTotal: %.2fs\n", secs_since(t0));

    free(discounts);
    for (uint32_t n = 1; n <= max_n; n++) ngt_free(&tabs[n - 1]);
    free(tabs);
    free(toks.ids);
    free(tk.table);
    return 0;
}
