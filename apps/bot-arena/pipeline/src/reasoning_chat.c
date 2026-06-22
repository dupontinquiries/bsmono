/*
 * reasoning_chat.c — Chat REPL v1.4: cache-resident neural additions
 *
 * Same n-gram core as chat.c, augmented with small floating-point operations
 * that add reasoning structure without breaking the cache-resident speed budget:
 *
 *   - Mean-pool word vectors to embed any token sequence
 *   - Score candidates by both perplexity AND semantic similarity to prompt
 *   - Beam search: explore K continuations per step, prune to top B, repeat D times
 *   - Transition-conditioned generation: produce one continuation per v1.3 cluster
 *
 * Per-token FLOPs added:
 *   - Mean-pool 10-token chunk in 128-dim: ~1280 ops (~250 ns)
 *   - Cosine similarity in 128-dim: ~256 ops (~50 ns)
 *   - Multi-criterion scoring of a candidate: ~10K ops total
 * All comfortably cache-resident. The n-gram is still the slow part of inference.
 *
 * Build:
 *   gcc -std=c99 -O3 -Wall -Wextra -o reasoning_chat reasoning_chat.c -lz -lm
 *
 * Usage:
 *   ./reasoning_chat <tokenizer.bin> <model.bin> <vectors.bin> [paragraph_vectors.bin] [transitions.bin]
 *
 * Commands (new in v1.4 in bold; v1.0 commands also present):
 *   <text>                  generate continuation
 *   /sample N <text>        generate N tokens
 *   /think <query>          beam search: B=3, K=4, D=3, span=24 (combined-score best path)
 *   /think B K D S <query>  beam search with explicit B, K, D, S
 *   /branch <query>         generate one continuation steered per transition cluster
 *   /score <text>           multi-metric breakdown for analysis
 *   /semantic <query>       top-5 paragraphs by paragraph_vector cosine (not TF-IDF)
 *   /perplexity <text>      per-token perplexity under the model
 *   /tokens <text>          tokenization view
 *   /temp X                 set temperature (default 0.8)
 *   /weights P S T          set score weights: perplexity, semantic, transition (default 1 1 0.5)
 *   /seed X                 reseed RNG
 *   /help | /quit
 */

#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <stdbool.h>
#include <ctype.h>
#include <math.h>
#include <time.h>
#include <unistd.h>
#include <zlib.h>

#define BPE_MAGIC          "BPETK01"
#define BPE_MAGIC_LEN      8
#define BPE_BASE_VOCAB     256u
#define NGRAM_MAGIC        "NGRAM02"
#define NGRAM_MAGIC_LEN    8
#define WORDV_MAGIC        "WORDV01"
#define WORDV_MAGIC_LEN    8
#define PARAG_MAGIC        "PARAG01"
#define PARAG_MAGIC_LEN    8
#define TRANS_MAGIC        "TRANS01"
#define TRANS_MAGIC_LEN    8

#define MAX_HISTORY        8192
#define DEFAULT_SAMPLE_N   60
#define BACKOFF_ALPHA      0.4
#define DEFAULT_BEAM_B     3
#define DEFAULT_BEAM_K     4
#define DEFAULT_BEAM_D     3
#define DEFAULT_BEAM_SPAN  24
#define SEMANTIC_TOP_K     5

static uint64_t fnv1a(const void *data, size_t len) {
    const uint8_t *p = (const uint8_t *)data;
    uint64_t h = 0xcbf29ce484222325ULL;
    for (size_t i = 0; i < len; i++) { h ^= p[i]; h *= 0x100000001b3ULL; }
    return h;
}

/* ===== PRNG ===== */
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
static double rng_uniform(void) { return (rng_next() >> 11) * (1.0 / (double)(1ULL << 53)); }

/* ===== Tokenizer ===== */
typedef struct { uint64_t key; uint32_t rank; uint32_t new_id; } bpe_merge_entry_t;
typedef struct {
    uint32_t vocab_size, num_merges;
    bpe_merge_entry_t *table; uint32_t table_cap;
    uint8_t **token_bytes; uint32_t *token_byte_lens;
} bpe_tokenizer_t;

static void bpe_load(bpe_tokenizer_t *tk, const char *path) {
    FILE *f = fopen(path, "rb"); if (!f) { perror(path); exit(1); }
    char magic[BPE_MAGIC_LEN];
    if (fread(magic,1,BPE_MAGIC_LEN,f) != BPE_MAGIC_LEN || memcmp(magic, BPE_MAGIC, 7) != 0)
        { fprintf(stderr,"bad tokenizer magic\n"); exit(1); }
    uint32_t version, vocab_size, num_merges;
    uint64_t corpus_bytes, uncompressed, compressed;
    if (fread(&version,4,1,f)!=1 || fread(&vocab_size,4,1,f)!=1 || fread(&num_merges,4,1,f)!=1 ||
        fread(&corpus_bytes,8,1,f)!=1 || fread(&uncompressed,8,1,f)!=1 || fread(&compressed,8,1,f)!=1)
        { fprintf(stderr,"tk header\n"); exit(1); }
    (void)version; (void)corpus_bytes;
    uint8_t *cbuf = malloc(compressed);
    if (!cbuf || fread(cbuf,1,compressed,f) != compressed) { fprintf(stderr,"tk read\n"); exit(1); }
    fclose(f);
    uint8_t *ubuf = malloc(uncompressed);
    uLongf ulen = (uLongf)uncompressed;
    if (uncompress(ubuf,&ulen,cbuf,(uLong)compressed) != Z_OK) { fprintf(stderr,"tk decomp\n"); exit(1); }
    free(cbuf);
    tk->vocab_size = vocab_size; tk->num_merges = num_merges;
    uint32_t cap = 1; while (cap < num_merges * 2) cap <<= 1; if (cap < 1024) cap = 1024;
    tk->table_cap = cap;
    tk->table = calloc(cap, sizeof(bpe_merge_entry_t));
    tk->token_bytes = calloc(vocab_size, sizeof(uint8_t *));
    tk->token_byte_lens = calloc(vocab_size, sizeof(uint32_t));
    for (uint32_t i = 0; i < BPE_BASE_VOCAB; i++) {
        tk->token_bytes[i] = malloc(1);
        tk->token_bytes[i][0] = (uint8_t)i;
        tk->token_byte_lens[i] = 1;
    }
    uint32_t *p32 = (uint32_t *)ubuf;
    uint32_t mask = cap - 1;
    for (uint32_t i = 0; i < num_merges; i++) {
        uint32_t a = p32[i*2], b = p32[i*2+1], new_id = BPE_BASE_VOCAB + i;
        uint64_t key = ((uint64_t)a << 32) | b;
        uint64_t hash = fnv1a(&key, 8);
        uint32_t idx = (uint32_t)(hash & mask);
        while (tk->table[idx].new_id != 0) idx = (idx + 1) & mask;
        tk->table[idx].key = key; tk->table[idx].rank = i; tk->table[idx].new_id = new_id;
        uint32_t la = tk->token_byte_lens[a], lb = tk->token_byte_lens[b], ln = la + lb;
        uint8_t *bytes = malloc(ln);
        memcpy(bytes, tk->token_bytes[a], la);
        memcpy(bytes + la, tk->token_bytes[b], lb);
        tk->token_bytes[new_id] = bytes;
        tk->token_byte_lens[new_id] = ln;
    }
    free(ubuf);
}

static bool bpe_lookup(const bpe_tokenizer_t *tk, uint32_t a, uint32_t b, uint32_t *r, uint32_t *nid) {
    uint64_t key = ((uint64_t)a << 32) | b;
    uint64_t hash = fnv1a(&key, 8);
    uint32_t mask = tk->table_cap - 1;
    uint32_t idx = (uint32_t)(hash & mask);
    while (tk->table[idx].new_id != 0) {
        if (tk->table[idx].key == key) { *r = tk->table[idx].rank; *nid = tk->table[idx].new_id; return true; }
        idx = (idx + 1) & mask;
    }
    return false;
}
static uint32_t bpe_apply_chunk(const bpe_tokenizer_t *tk, uint32_t *tokens, uint32_t len) {
    while (len >= 2) {
        uint32_t bp = UINT32_MAX, br = UINT32_MAX, bn = 0;
        for (uint32_t i = 0; i + 1 < len; i++) {
            uint32_t r, nid;
            if (bpe_lookup(tk, tokens[i], tokens[i+1], &r, &nid))
                if (r < br) { br = r; bp = i; bn = nid; }
        }
        if (bp == UINT32_MAX) break;
        tokens[bp] = bn;
        for (uint32_t i = bp + 1; i + 1 < len; i++) tokens[i] = tokens[i+1];
        len--;
    }
    return len;
}
typedef enum { CC_ALPHA, CC_WHITE, CC_OTHER } char_class_t;
static char_class_t classify(uint8_t c) {
    if ((c>='a'&&c<='z')||(c>='A'&&c<='Z')||(c>='0'&&c<='9')||c=='_') return CC_ALPHA;
    if (c==' '||c=='\t'||c=='\n'||c=='\r') return CC_WHITE;
    return CC_OTHER;
}
static uint32_t tokenize(const bpe_tokenizer_t *tk, const uint8_t *text, size_t len,
                          uint32_t *out, uint32_t out_cap) {
    uint32_t out_n = 0; size_t i = 0; uint32_t chunk[2048];
    while (i < len) {
        char_class_t cc = classify(text[i]); size_t start = i;
        while (i < len && classify(text[i]) == cc) i++;
        size_t clen = i - start;
        while (clen > 0) {
            uint32_t take = (uint32_t)(clen > 2048 ? 2048 : clen);
            for (uint32_t k = 0; k < take; k++) chunk[k] = text[start + k];
            uint32_t nlen = bpe_apply_chunk(tk, chunk, take);
            for (uint32_t k = 0; k < nlen; k++) { if (out_n >= out_cap) return out_n; out[out_n++] = chunk[k]; }
            start += take; clen -= take;
        }
    }
    return out_n;
}
static void detok_print(const bpe_tokenizer_t *tk, const uint32_t *tokens, uint32_t n, FILE *out) {
    for (uint32_t i = 0; i < n; i++) {
        uint32_t id = tokens[i];
        if (id >= tk->vocab_size) continue;
        fwrite(tk->token_bytes[id], 1, tk->token_byte_lens[id], out);
    }
    fflush(out);
}

/* ===== N-gram model ===== */
typedef struct { uint32_t n, count; uint8_t *entries; size_t entry_size; } ngram_order_t;
typedef struct { double D1, D2, D3plus; } discount_t;
typedef struct {
    uint32_t max_n, vocab_size;
    uint64_t total_tokens;
    ngram_order_t *orders; discount_t *discounts;
} model_t;

static size_t g_es;
static int cmp_entry(const void *a, const void *b) {
    const uint32_t *ta = (const uint32_t *)a, *tb = (const uint32_t *)b;
    size_t n = (g_es - 8) / 4;
    for (size_t i = 0; i < n; i++) {
        if (ta[i] < tb[i]) return -1;
        if (ta[i] > tb[i]) return 1;
    }
    return 0;
}
static void model_load(model_t *m, const char *path) {
    FILE *f = fopen(path, "rb"); if (!f) { perror(path); exit(1); }
    char magic[NGRAM_MAGIC_LEN];
    if (fread(magic,1,NGRAM_MAGIC_LEN,f) != NGRAM_MAGIC_LEN || memcmp(magic, NGRAM_MAGIC, 7) != 0)
        { fprintf(stderr,"bad model magic\n"); exit(1); }
    uint32_t version, max_n, vocab_size;
    uint64_t total_tokens, uncompressed, compressed;
    if (fread(&version,4,1,f)!=1 || fread(&max_n,4,1,f)!=1 || fread(&vocab_size,4,1,f)!=1 ||
        fread(&total_tokens,8,1,f)!=1 || fread(&uncompressed,8,1,f)!=1 || fread(&compressed,8,1,f)!=1)
        { fprintf(stderr,"model header\n"); exit(1); }
    (void)version;
    uint8_t *cbuf = malloc(compressed);
    if (!cbuf || fread(cbuf,1,compressed,f) != compressed) { fprintf(stderr,"model read\n"); exit(1); }
    fclose(f);
    uint8_t *ubuf = malloc(uncompressed);
    uLongf ulen = (uLongf)uncompressed;
    if (uncompress(ubuf,&ulen,cbuf,(uLong)compressed) != Z_OK) { fprintf(stderr,"model decomp\n"); exit(1); }
    free(cbuf);
    m->max_n = max_n; m->vocab_size = vocab_size; m->total_tokens = total_tokens;
    m->orders = calloc(max_n, sizeof(ngram_order_t));
    m->discounts = calloc(max_n, sizeof(discount_t));
    uint8_t *p = ubuf;
    for (uint32_t oi = 0; oi < max_n; oi++) {
        uint32_t n, cnt;
        memcpy(&n, p, 4); p += 4; memcpy(&cnt, p, 4); p += 4;
        size_t es = (size_t)n * 4 + 8;
        uint8_t *entries = malloc((size_t)cnt * es + 1);
        memcpy(entries, p, (size_t)cnt * es); p += (size_t)cnt * es;
        m->orders[oi].n = n; m->orders[oi].count = cnt;
        m->orders[oi].entries = entries; m->orders[oi].entry_size = es;
    }
    for (uint32_t oi = 0; oi < max_n; oi++) {
        memcpy(&m->discounts[oi].D1, p, 8); p += 8;
        memcpy(&m->discounts[oi].D2, p, 8); p += 8;
        memcpy(&m->discounts[oi].D3plus, p, 8); p += 8;
    }
    free(ubuf);
    for (uint32_t i = 0; i < max_n; i++) {
        if (m->orders[i].count <= 1) continue;
        g_es = m->orders[i].entry_size;
        qsort(m->orders[i].entries, m->orders[i].count, m->orders[i].entry_size, cmp_entry);
    }
}
static inline const uint32_t *order_tokens(const ngram_order_t *o, uint32_t idx) {
    return (const uint32_t *)(o->entries + (size_t)idx * o->entry_size);
}
static inline uint64_t order_count(const ngram_order_t *o, uint32_t idx) {
    uint64_t c;
    memcpy(&c, o->entries + (size_t)idx * o->entry_size + o->n * 4, 8);
    return c;
}
static uint32_t order_lower_bound(const ngram_order_t *o, const uint32_t *prefix, uint32_t plen) {
    uint32_t lo = 0, hi = o->count;
    while (lo < hi) {
        uint32_t mid = lo + (hi - lo) / 2;
        const uint32_t *t = order_tokens(o, mid);
        int cmp = 0;
        for (uint32_t i = 0; i < plen; i++) {
            if (t[i] < prefix[i]) { cmp = -1; break; }
            if (t[i] > prefix[i]) { cmp = 1; break; }
        }
        if (cmp < 0) lo = mid + 1; else hi = mid;
    }
    return lo;
}
static uint32_t order_continuations(const ngram_order_t *o, const uint32_t *prefix, uint32_t plen,
                                     uint32_t *out_next, uint64_t *out_count, uint32_t out_cap,
                                     uint64_t *out_total) {
    if (o->n != plen + 1 || o->count == 0) { *out_total = 0; return 0; }
    uint32_t idx = order_lower_bound(o, prefix, plen);
    uint64_t total = 0; uint32_t num = 0;
    while (idx < o->count) {
        const uint32_t *t = order_tokens(o, idx);
        bool match = true;
        for (uint32_t i = 0; i < plen; i++) { if (t[i] != prefix[i]) { match = false; break; } }
        if (!match) break;
        uint64_t c = order_count(o, idx);
        total += c;
        if (num < out_cap) { out_next[num] = t[plen]; out_count[num] = c; num++; }
        idx++;
    }
    *out_total = total;
    return num;
}
static uint32_t sample_next(const model_t *m, const uint32_t *history, uint32_t hlen,
                             double temperature) {
    static uint32_t cand_next[16384];
    static uint64_t cand_count[16384];
    for (uint32_t n = m->max_n; n >= 2; n--) {
        if (n - 1 > hlen) continue;
        const uint32_t *ctx = history + (hlen - (n - 1));
        uint64_t total;
        uint32_t num = order_continuations(&m->orders[n-1], ctx, n-1, cand_next, cand_count, 16384, &total);
        if (num > 0) {
            if (temperature <= 0.0) temperature = 1e-3;
            double *w = malloc(num * sizeof(double));
            double sum = 0;
            for (uint32_t i = 0; i < num; i++) {
                double p = (double)cand_count[i] / (double)total;
                w[i] = exp(log(p) / temperature);
                sum += w[i];
            }
            double r = rng_uniform() * sum;
            uint32_t pick = num - 1;
            for (uint32_t i = 0; i < num; i++) { r -= w[i]; if (r <= 0) { pick = i; break; } }
            uint32_t tok = cand_next[pick]; free(w);
            return tok;
        }
    }
    const ngram_order_t *o1 = &m->orders[0];
    if (o1->count == 0) return 0;
    uint64_t total = m->total_tokens;
    uint64_t r = rng_next() % (total ? total : 1);
    for (uint32_t i = 0; i < o1->count; i++) {
        uint64_t c = order_count(o1, i);
        if (r < c) return *order_tokens(o1, i);
        r -= c;
    }
    return *order_tokens(o1, 0);
}
static double logprob_next(const model_t *m, const uint32_t *history, uint32_t hlen, uint32_t next) {
    double backoff = 1.0;
    for (uint32_t n = m->max_n; n >= 2; n--) {
        if (n - 1 > hlen) { backoff *= BACKOFF_ALPHA; continue; }
        const uint32_t *ctx = history + (hlen - (n - 1));
        uint32_t key[6];
        for (uint32_t i = 0; i < n - 1; i++) key[i] = ctx[i];
        key[n - 1] = next;
        uint32_t idx = order_lower_bound(&m->orders[n-1], key, n);
        bool exact = false;
        if (idx < m->orders[n-1].count) {
            const uint32_t *t = order_tokens(&m->orders[n-1], idx);
            bool eq = true;
            for (uint32_t i = 0; i < n; i++) if (t[i] != key[i]) { eq = false; break; }
            exact = eq;
        }
        if (exact) {
            uint64_t c_full = order_count(&m->orders[n-1], idx);
            uint64_t total;
            uint32_t dn; uint64_t dc;
            (void)order_continuations(&m->orders[n-1], ctx, n-1, &dn, &dc, 0, &total);
            if (total > 0) return log((double)c_full / (double)total * backoff);
        }
        backoff *= BACKOFF_ALPHA;
    }
    const ngram_order_t *o1 = &m->orders[0];
    uint32_t idx = order_lower_bound(o1, &next, 1);
    if (idx < o1->count) {
        const uint32_t *t = order_tokens(o1, idx);
        if (t[0] == next) {
            uint64_t c = order_count(o1, idx);
            return log((double)c / (double)m->total_tokens * backoff);
        }
    }
    return log(backoff / ((double)m->vocab_size * (double)m->total_tokens));
}
static double sequence_perplexity(const model_t *m, const uint32_t *tokens, uint32_t n, uint32_t skip) {
    if (n <= skip + 1) return INFINITY;
    double sum = 0; uint32_t scored = 0;
    for (uint32_t i = skip + 1; i < n; i++) { sum += logprob_next(m, tokens, i, tokens[i]); scored++; }
    return exp(-sum / (double)scored);
}

/* ===== Word vectors ===== */
typedef struct { uint32_t vocab_size, dim; float *V; } wordvec_t;
static void wordvec_load(wordvec_t *w, const char *path) {
    FILE *f = fopen(path, "rb"); if (!f) { perror(path); exit(1); }
    char magic[WORDV_MAGIC_LEN];
    if (fread(magic,1,WORDV_MAGIC_LEN,f) != WORDV_MAGIC_LEN || memcmp(magic, WORDV_MAGIC, 7) != 0)
        { fprintf(stderr,"bad vectors magic\n"); exit(1); }
    uint32_t version, vocab_size, dim;
    uint64_t total_tokens, compressed;
    if (fread(&version,4,1,f)!=1 || fread(&vocab_size,4,1,f)!=1 || fread(&dim,4,1,f)!=1 ||
        fread(&total_tokens,8,1,f)!=1 || fread(&compressed,8,1,f)!=1)
        { fprintf(stderr,"vec header\n"); exit(1); }
    (void)version; (void)total_tokens;
    uint8_t *cbuf = malloc(compressed);
    if (!cbuf || fread(cbuf,1,compressed,f) != compressed) { fprintf(stderr,"vec read\n"); exit(1); }
    fclose(f);
    size_t uncompressed = (size_t)vocab_size * dim * sizeof(float);
    w->V = malloc(uncompressed);
    uLongf ulen = (uLongf)uncompressed;
    if (uncompress((Bytef *)w->V, &ulen, cbuf, (uLong)compressed) != Z_OK)
        { fprintf(stderr,"vec decomp\n"); exit(1); }
    free(cbuf);
    w->vocab_size = vocab_size; w->dim = dim;
}

/* ===== Paragraph vectors (optional, for /semantic) ===== */
typedef struct {
    uint32_t doc_id, pos_in_doc;
    char    *src_path;
    char    *text;
    uint32_t text_len;
    float   *vector;
} para_t;
typedef struct {
    para_t *list; uint32_t n_paragraphs, n_documents, dim;
    bool loaded;
} para_set_t;

static void para_load(para_set_t *ps, const char *path) {
    ps->loaded = false;
    FILE *f = fopen(path, "rb"); if (!f) { perror(path); return; }
    char magic[PARAG_MAGIC_LEN];
    if (fread(magic,1,PARAG_MAGIC_LEN,f) != PARAG_MAGIC_LEN || memcmp(magic, PARAG_MAGIC, 7) != 0)
        { fprintf(stderr,"bad para magic\n"); fclose(f); return; }
    uint32_t version, dim;
    uint64_t compressed;
    if (fread(&version,4,1,f)!=1 || fread(&dim,4,1,f)!=1 || fread(&compressed,8,1,f)!=1)
        { fprintf(stderr,"para header\n"); fclose(f); return; }
    (void)version;
    uint8_t *cbuf = malloc(compressed);
    if (!cbuf || fread(cbuf,1,compressed,f) != compressed) { fclose(f); free(cbuf); return; }
    fclose(f);
    size_t guess = (size_t)compressed * 8;
    uint8_t *ubuf = NULL; uLongf ulen = 0;
    for (;;) {
        ubuf = realloc(ubuf, guess);
        ulen = (uLongf)guess;
        int rc = uncompress(ubuf, &ulen, cbuf, (uLong)compressed);
        if (rc == Z_OK) break;
        if (rc == Z_BUF_ERROR) { guess *= 2; continue; }
        fprintf(stderr, "para decomp rc=%d\n", rc); free(cbuf); free(ubuf); return;
    }
    free(cbuf);
    uint8_t *p = ubuf;
    uint32_t n_para, n_doc;
    memcpy(&n_para, p, 4); p += 4;
    memcpy(&n_doc,  p, 4); p += 4;
    ps->dim = dim; ps->n_paragraphs = n_para; ps->n_documents = n_doc;
    ps->list = malloc(n_para * sizeof(para_t));
    for (uint32_t i = 0; i < n_para; i++) {
        para_t *pp = &ps->list[i];
        memcpy(&pp->doc_id, p, 4); p += 4;
        memcpy(&pp->pos_in_doc, p, 4); p += 4;
        uint32_t src_len; memcpy(&src_len, p, 4); p += 4;
        pp->src_path = malloc(src_len + 1);
        memcpy(pp->src_path, p, src_len); pp->src_path[src_len] = 0; p += src_len;
        memcpy(&pp->text_len, p, 4); p += 4;
        pp->text = malloc(pp->text_len + 1);
        memcpy(pp->text, p, pp->text_len); pp->text[pp->text_len] = 0; p += pp->text_len;
        pp->vector = malloc(dim * sizeof(float));
        memcpy(pp->vector, p, dim * sizeof(float)); p += dim * sizeof(float);
    }
    free(ubuf);
    ps->loaded = true;
}

/* ===== Transitions (optional, for /branch) ===== */
typedef struct {
    uint32_t n_clusters, dim;
    float    *centroids;     /* n_clusters * dim */
    uint32_t *sizes;
    bool      loaded;
} trans_set_t;

static void trans_load(trans_set_t *ts, const char *path) {
    ts->loaded = false;
    FILE *f = fopen(path, "rb"); if (!f) { perror(path); return; }
    char magic[TRANS_MAGIC_LEN];
    if (fread(magic,1,TRANS_MAGIC_LEN,f) != TRANS_MAGIC_LEN || memcmp(magic, TRANS_MAGIC, 7) != 0)
        { fprintf(stderr,"bad trans magic\n"); fclose(f); return; }
    uint32_t version, dim, n_trans, n_clusters;
    uint64_t compressed;
    if (fread(&version,4,1,f)!=1 || fread(&dim,4,1,f)!=1 || fread(&n_trans,4,1,f)!=1 ||
        fread(&n_clusters,4,1,f)!=1 || fread(&compressed,8,1,f)!=1)
        { fclose(f); return; }
    (void)version;
    uint8_t *cbuf = malloc(compressed);
    if (!cbuf || fread(cbuf,1,compressed,f) != compressed) { fclose(f); free(cbuf); return; }
    fclose(f);
    size_t guess = (size_t)compressed * 8;
    uint8_t *ubuf = NULL; uLongf ulen = 0;
    for (;;) {
        ubuf = realloc(ubuf, guess);
        ulen = (uLongf)guess;
        int rc = uncompress(ubuf, &ulen, cbuf, (uLong)compressed);
        if (rc == Z_OK) break;
        if (rc == Z_BUF_ERROR) { guess *= 2; continue; }
        free(cbuf); free(ubuf); return;
    }
    free(cbuf);
    ts->dim = dim; ts->n_clusters = n_clusters;
    ts->centroids = malloc((size_t)n_clusters * dim * sizeof(float));
    ts->sizes     = malloc(n_clusters * sizeof(uint32_t));
    uint8_t *p = ubuf;
    /* skip transition records */
    p += (size_t)n_trans * (8 + dim * sizeof(float));
    for (uint32_t k = 0; k < n_clusters; k++) {
        memcpy(&ts->sizes[k], p, 4); p += 4;
        memcpy(ts->centroids + (size_t)k * dim, p, dim * sizeof(float));
        p += dim * sizeof(float);
    }
    free(ubuf);
    ts->loaded = true;
}

/* ===== Embedding operations ===== */
static void embed_tokens(const wordvec_t *w, const uint32_t *tokens, uint32_t n, float *out) {
    memset(out, 0, w->dim * sizeof(float));
    uint32_t counted = 0;
    for (uint32_t i = 0; i < n; i++) {
        uint32_t id = tokens[i];
        if (id >= w->vocab_size) continue;
        const float *v = w->V + (size_t)id * w->dim;
        for (uint32_t d = 0; d < w->dim; d++) out[d] += v[d];
        counted++;
    }
    if (counted == 0) return;
    float inv = 1.0f / (float)counted;
    for (uint32_t d = 0; d < w->dim; d++) out[d] *= inv;
}

static float cosine_sim(const float *a, const float *b, uint32_t dim) {
    float dot = 0, na = 0, nb = 0;
    for (uint32_t d = 0; d < dim; d++) {
        dot += a[d] * b[d];
        na  += a[d] * a[d];
        nb  += b[d] * b[d];
    }
    if (na <= 0 || nb <= 0) return 0;
    return dot / sqrtf(na * nb);
}

/* ===== Generation core ===== */
static double g_temperature = 0.8;
/* Scoring weights */
static double g_w_perplex = 1.0;
static double g_w_semantic = 1.0;
static double g_w_transition = 0.5;

static uint32_t generate_into(const model_t *m, uint32_t *history, uint32_t hlen,
                               uint32_t n_tokens, double temperature) {
    for (uint32_t i = 0; i < n_tokens && hlen < MAX_HISTORY; i++) {
        uint32_t tok = sample_next(m, history, hlen, temperature);
        history[hlen++] = tok;
    }
    return hlen;
}

/* ===== Delimiter-balance constraint (code-aware generation) ===== */
#define DELIM_MAX_DEPTH 512
#define CODE_RESAMPLE_TRIES 12

typedef struct { char stack[DELIM_MAX_DEPTH]; int depth; } delim_state_t;

static char delim_closer(char open) {
    switch (open) { case '(': return ')'; case '[': return ']'; case '{': return '}'; }
    return 0;
}

/* Apply a token to delimiter state without rejecting (used for the prompt):
 * closers that don't match are clamped away rather than failing. */
static void delim_apply_clamped(const bpe_tokenizer_t *tk, uint32_t tok, delim_state_t *st) {
    if (tok >= tk->vocab_size) return;
    const uint8_t *b = tk->token_bytes[tok];
    uint32_t n = tk->token_byte_lens[tok];
    for (uint32_t i = 0; i < n; i++) {
        char ch = (char)b[i];
        if (ch=='('||ch=='['||ch=='{') { if (st->depth < DELIM_MAX_DEPTH) st->stack[st->depth++] = ch; }
        else if (ch==')'||ch==']'||ch=='}') { if (st->depth > 0) st->depth--; }
    }
}

/* Test whether a token keeps delimiters properly nested. On success, updates st.
 * Rejects: closing an unopened delimiter, or a mismatched closer (']' closing '('). */
static bool delim_token_ok(const bpe_tokenizer_t *tk, uint32_t tok, delim_state_t *st) {
    if (tok >= tk->vocab_size) return true;
    const uint8_t *b = tk->token_bytes[tok];
    uint32_t n = tk->token_byte_lens[tok];
    delim_state_t tmp = *st;
    for (uint32_t i = 0; i < n; i++) {
        char ch = (char)b[i];
        if (ch=='('||ch=='['||ch=='{') {
            if (tmp.depth >= DELIM_MAX_DEPTH) return false;
            tmp.stack[tmp.depth++] = ch;
        } else if (ch==')'||ch==']'||ch=='}') {
            if (tmp.depth == 0) return false;
            if (delim_closer(tmp.stack[tmp.depth - 1]) != ch) return false;
            tmp.depth--;
        }
    }
    *st = tmp;
    return true;
}

/* Code-aware generation: resample tokens that would break delimiter nesting,
 * then auto-close any delimiters left open at the end. Returns new length. */
static uint32_t generate_code(const model_t *m, const bpe_tokenizer_t *tk,
                              uint32_t *history, uint32_t hlen, uint32_t n_tokens,
                              double temperature, uint32_t *out_resamples,
                              uint32_t *out_autoclosed) {
    delim_state_t st = {0};
    for (uint32_t i = 0; i < hlen; i++) delim_apply_clamped(tk, history[i], &st);

    uint32_t resamples = 0;
    for (uint32_t i = 0; i < n_tokens && hlen < MAX_HISTORY; i++) {
        uint32_t tok = 0;
        bool ok = false;
        for (uint32_t t = 0; t < CODE_RESAMPLE_TRIES; t++) {
            tok = sample_next(m, history, hlen, temperature);
            delim_state_t probe = st;
            if (delim_token_ok(tk, tok, &probe)) { st = probe; ok = true; break; }
            resamples++;
        }
        if (!ok) {
            /* All tries violated nesting; accept but clamp so state stays consistent. */
            delim_apply_clamped(tk, tok, &st);
        }
        history[hlen++] = tok;
    }

    /* Auto-close whatever is still open, innermost first. Base-vocab token id == byte value. */
    uint32_t autoclosed = 0;
    while (st.depth > 0 && hlen < MAX_HISTORY) {
        char closer = delim_closer(st.stack[--st.depth]);
        if (!closer) continue;
        history[hlen++] = (uint32_t)(uint8_t)closer;
        autoclosed++;
    }
    if (out_resamples)  *out_resamples = resamples;
    if (out_autoclosed) *out_autoclosed = autoclosed;
    return hlen;
}

/* Combined score: lower is better.
 * Components:
 *   - perplexity: lower is better (we use log(ppl) for stable scale)
 *   - semantic similarity to prompt embedding: higher is better -> negate
 *   - if a transition target is provided, distance from desired transition: lower is better
 */
typedef struct {
    double perplexity_logppl;
    double semantic_cos;       /* prompt vs continuation */
    double transition_score;   /* 1 - cos(induced, target), 0 if no target */
    double combined;
} score_t;

static void score_candidate(const model_t *m, const wordvec_t *w,
                            const uint32_t *full_seq, uint32_t total_len, uint32_t prompt_len,
                            const float *prompt_emb,
                            const float *transition_target /* may be NULL */,
                            score_t *out) {
    double ppl = sequence_perplexity(m, full_seq, total_len, prompt_len);
    out->perplexity_logppl = log(ppl > 0 ? ppl : 1.0);

    float cont_emb[1024];
    if (w->dim > 1024) { out->semantic_cos = 0; out->transition_score = 0; }
    else {
        embed_tokens(w, full_seq + prompt_len, total_len - prompt_len, cont_emb);
        out->semantic_cos = cosine_sim(prompt_emb, cont_emb, w->dim);
        if (transition_target) {
            float induced[1024];
            for (uint32_t d = 0; d < w->dim; d++) induced[d] = cont_emb[d] - prompt_emb[d];
            float tc = cosine_sim(induced, transition_target, w->dim);
            out->transition_score = 1.0 - tc; /* lower better */
        } else {
            out->transition_score = 0;
        }
    }

    out->combined = g_w_perplex * out->perplexity_logppl
                  - g_w_semantic * out->semantic_cos
                  + g_w_transition * out->transition_score;
}

/* ===== Beam search ===== */
typedef struct {
    uint32_t *tokens;
    uint32_t  len;
    score_t   score;
} beam_state_t;

static beam_state_t *bs_new(uint32_t cap) {
    beam_state_t *bs = malloc(sizeof(beam_state_t));
    bs->tokens = malloc(cap * sizeof(uint32_t));
    bs->len = 0;
    memset(&bs->score, 0, sizeof(score_t));
    return bs;
}
static void bs_free(beam_state_t *bs) { free(bs->tokens); free(bs); }
static void bs_copy(beam_state_t *dst, const beam_state_t *src) {
    memcpy(dst->tokens, src->tokens, src->len * sizeof(uint32_t));
    dst->len = src->len;
    dst->score = src->score;
}

static int cmp_beam(const void *a, const void *b) {
    const beam_state_t * const *pa = (const beam_state_t * const *)a;
    const beam_state_t * const *pb = (const beam_state_t * const *)b;
    if ((*pa)->score.combined < (*pb)->score.combined) return -1;
    if ((*pa)->score.combined > (*pb)->score.combined) return  1;
    return 0;
}

static void cmd_think(const model_t *m, const wordvec_t *w, const bpe_tokenizer_t *tk,
                      uint32_t B, uint32_t K, uint32_t D, uint32_t span,
                      const char *query, const float *transition_target) {
    uint32_t prompt_buf[MAX_HISTORY];
    uint32_t prompt_len = tokenize(tk, (const uint8_t *)query, strlen(query), prompt_buf, MAX_HISTORY);

    /* Prompt embedding (for semantic scoring of continuations) */
    float prompt_emb[1024];
    if (w->dim > 1024) { fprintf(stderr,"dim too large for stack buffer\n"); return; }
    embed_tokens(w, prompt_buf, prompt_len, prompt_emb);

    if (B == 0) { fprintf(stderr,"B must be >= 1\n"); return; }

    /* Initial beam: 1 state = the prompt */
    beam_state_t **beam = malloc(B * sizeof(beam_state_t *));
    for (uint32_t i = 0; i < B; i++) beam[i] = bs_new(MAX_HISTORY);
    beam_state_t seed = {0};
    seed.tokens = prompt_buf;
    seed.len = prompt_len;
    bs_copy(beam[0], &seed);
    uint32_t beam_n = 1;

    /* Working space for B*K candidates */
    beam_state_t **cand = malloc(B * K * sizeof(beam_state_t *));
    for (uint32_t i = 0; i < B * K; i++) cand[i] = bs_new(MAX_HISTORY);

    fprintf(stderr, "[/think B=%u K=%u D=%u span=%u]\n", B, K, D, span);

    for (uint32_t depth = 0; depth < D; depth++) {
        uint32_t n_cand = 0;
        for (uint32_t b = 0; b < beam_n; b++) {
            for (uint32_t k = 0; k < K; k++) {
                /* Copy parent, generate span tokens, score */
                beam_state_t *c = cand[n_cand++];
                bs_copy(c, beam[b]);
                c->len = generate_into(m, c->tokens, c->len, span, g_temperature);
                /* Score on the generated portion */
                score_candidate(m, w, c->tokens, c->len, prompt_len, prompt_emb,
                                transition_target, &c->score);
            }
        }
        /* Sort candidates by combined score ascending (best first), keep top B */
        qsort(cand, n_cand, sizeof(beam_state_t *), cmp_beam);
        uint32_t keep = n_cand < B ? n_cand : B;
        /* Swap top keep into beam */
        for (uint32_t i = 0; i < keep; i++) {
            beam_state_t *tmp = beam[i]; beam[i] = cand[i]; cand[i] = tmp;
        }
        beam_n = keep;

        fprintf(stderr, "  depth %u: kept %u branches, best combined=%.3f (logppl=%.3f, sem=%.3f, trans=%.3f)\n",
                depth + 1, beam_n,
                beam[0]->score.combined,
                beam[0]->score.perplexity_logppl,
                beam[0]->score.semantic_cos,
                beam[0]->score.transition_score);
    }

    /* Print the best */
    fprintf(stdout, "\n=== best path ===\n");
    detok_print(tk, beam[0]->tokens, beam[0]->len, stdout);
    fprintf(stdout, "\n");

    for (uint32_t i = 0; i < B; i++) bs_free(beam[i]);
    for (uint32_t i = 0; i < B * K; i++) bs_free(cand[i]);
    free(beam); free(cand);
}

/* /branch: one /think run per transition cluster */
static void cmd_branch(const model_t *m, const wordvec_t *w, const bpe_tokenizer_t *tk,
                       const trans_set_t *ts, const char *query) {
    if (!ts->loaded) { fprintf(stdout, "transitions.bin not loaded; /branch unavailable\n"); return; }
    uint32_t B = 2, K = 3, D = 2, span = 20;
    /* Force transition weight up for this mode */
    double saved = g_w_transition;
    g_w_transition = 2.0;
    for (uint32_t k = 0; k < ts->n_clusters; k++) {
        if (ts->sizes[k] == 0) continue;
        const float *target = ts->centroids + (size_t)k * ts->dim;
        fprintf(stdout, "\n========== cluster %u (size %u) ==========\n", k, ts->sizes[k]);
        cmd_think(m, w, tk, B, K, D, span, query, target);
    }
    g_w_transition = saved;
}

/* /score: report all metrics for a piece of text against an implicit "self" prompt */
static void cmd_score(const model_t *m, const wordvec_t *w, const bpe_tokenizer_t *tk,
                       const char *text) {
    uint32_t toks[MAX_HISTORY];
    uint32_t n = tokenize(tk, (const uint8_t *)text, strlen(text), toks, MAX_HISTORY);
    if (n < 2) { fprintf(stdout, "need at least 2 tokens\n"); return; }
    double ppl = sequence_perplexity(m, toks, n, 0);
    float emb[1024];
    embed_tokens(w, toks, n, emb);
    /* For "is this you-like": cosine to all-ones direction is meaningless;
     * better: just show the embedding norm and a few stats */
    float norm = 0;
    for (uint32_t d = 0; d < w->dim; d++) norm += emb[d] * emb[d];
    norm = sqrtf(norm);
    fprintf(stdout, "tokens: %u\n", n);
    fprintf(stdout, "perplexity: %.2f (logppl=%.3f)\n", ppl, log(ppl));
    fprintf(stdout, "embedding norm: %.4f\n", norm);
}

/* /semantic: top-K paragraphs by paragraph_vector cosine to query (semantic, not lexical) */
static void cmd_semantic(const wordvec_t *w, const para_set_t *ps, const bpe_tokenizer_t *tk,
                          const char *query) {
    if (!ps->loaded) { fprintf(stdout, "paragraph_vectors.bin not loaded; /semantic unavailable\n"); return; }
    uint32_t qtoks[8192];
    uint32_t qn = tokenize(tk, (const uint8_t *)query, strlen(query), qtoks, 8192);
    float qemb[1024];
    embed_tokens(w, qtoks, qn, qemb);

    /* Top-K by cosine */
    uint32_t top_idx[SEMANTIC_TOP_K];
    float    top_sim[SEMANTIC_TOP_K];
    uint32_t found = 0;
    for (uint32_t i = 0; i < ps->n_paragraphs; i++) {
        float s = cosine_sim(qemb, ps->list[i].vector, w->dim);
        if (found < SEMANTIC_TOP_K) {
            uint32_t pos = found;
            while (pos > 0 && top_sim[pos - 1] < s) {
                top_sim[pos] = top_sim[pos - 1];
                top_idx[pos] = top_idx[pos - 1];
                pos--;
            }
            top_sim[pos] = s; top_idx[pos] = i; found++;
        } else if (s > top_sim[SEMANTIC_TOP_K - 1]) {
            uint32_t pos = SEMANTIC_TOP_K - 1;
            while (pos > 0 && top_sim[pos - 1] < s) {
                top_sim[pos] = top_sim[pos - 1];
                top_idx[pos] = top_idx[pos - 1];
                pos--;
            }
            top_sim[pos] = s; top_idx[pos] = i;
        }
    }
    if (found == 0) { fprintf(stdout, "(no paragraphs)\n"); return; }
    for (uint32_t i = 0; i < found; i++) {
        const para_t *p = &ps->list[top_idx[i]];
        fprintf(stdout, "\n[%u] cos=%.4f  %s\n", i + 1, top_sim[i], p->src_path);
        uint32_t show = p->text_len > 300 ? 300 : p->text_len;
        fwrite(p->text, 1, show, stdout);
        if (p->text_len > 300) fprintf(stdout, "...");
        fputc('\n', stdout);
    }
}

static void cmd_generate(const model_t *m, const bpe_tokenizer_t *tk, const char *prompt, uint32_t n) {
    uint32_t history[MAX_HISTORY];
    uint32_t hlen = tokenize(tk, (const uint8_t *)prompt, strlen(prompt), history, MAX_HISTORY);
    detok_print(tk, history, hlen, stdout);
    uint32_t new_hlen = generate_into(m, history, hlen, n, g_temperature);
    detok_print(tk, history + hlen, new_hlen - hlen, stdout);
    fputc('\n', stdout);
}
static void cmd_code(const model_t *m, const bpe_tokenizer_t *tk, const char *prompt, uint32_t n) {
    uint32_t history[MAX_HISTORY];
    uint32_t hlen = tokenize(tk, (const uint8_t *)prompt, strlen(prompt), history, MAX_HISTORY);
    detok_print(tk, history, hlen, stdout);
    uint32_t resamples = 0, autoclosed = 0;
    uint32_t new_hlen = generate_code(m, tk, history, hlen, n, g_temperature, &resamples, &autoclosed);
    detok_print(tk, history + hlen, new_hlen - hlen, stdout);
    fprintf(stdout, "\n[%u resamples to keep delimiters nested, %u auto-closed at end]\n",
            resamples, autoclosed);
}
static void cmd_perplexity(const model_t *m, const bpe_tokenizer_t *tk, const char *text) {
    uint32_t toks[MAX_HISTORY];
    uint32_t n = tokenize(tk, (const uint8_t *)text, strlen(text), toks, MAX_HISTORY);
    if (n < 2) { fprintf(stdout,"need >= 2 tokens\n"); return; }
    fprintf(stdout, "tokens=%u  ppl=%.2f\n", n, sequence_perplexity(m, toks, n, 0));
}
static void cmd_tokens(const bpe_tokenizer_t *tk, const char *text) {
    uint32_t toks[MAX_HISTORY];
    uint32_t n = tokenize(tk, (const uint8_t *)text, strlen(text), toks, MAX_HISTORY);
    fprintf(stdout, "%u tokens\n", n);
    for (uint32_t i = 0; i < n; i++) {
        fprintf(stdout, "  [%5u] '", toks[i]);
        uint8_t *b = tk->token_bytes[toks[i]];
        uint32_t bn = tk->token_byte_lens[toks[i]];
        for (uint32_t j = 0; j < bn; j++) {
            uint8_t c = b[j];
            if (c == '\n') fprintf(stdout,"\\n");
            else if (c == '\t') fprintf(stdout,"\\t");
            else if (c < 32 || c >= 127) fprintf(stdout,"\\x%02x", c);
            else fputc(c, stdout);
        }
        fprintf(stdout, "'\n");
    }
}

static void cmd_help(const para_set_t *ps, const trans_set_t *ts) {
    fprintf(stdout,
        "commands:\n"
        "  <text>                generate 60-token continuation\n"
        "  /sample N <text>      generate N tokens\n"
        "  /code N <text>        generate N tokens with delimiter balancing + auto-close\n"
        "  /think <query>        beam search (B=%u K=%u D=%u span=%u)\n"
        "  /think B K D S <q>    beam search with explicit params\n"
        "  /branch <query>       %s\n"
        "  /score <text>         multi-metric breakdown\n"
        "  /semantic <query>     %s\n"
        "  /perplexity <text>    per-token perplexity\n"
        "  /tokens <text>        tokenization\n"
        "  /temp X               temperature (now %.2f)\n"
        "  /weights P S T        score weights (now P=%.2f S=%.2f T=%.2f)\n"
        "  /seed X               reseed\n"
        "  /help | /quit\n",
        DEFAULT_BEAM_B, DEFAULT_BEAM_K, DEFAULT_BEAM_D, DEFAULT_BEAM_SPAN,
        ts->loaded ? "one continuation per transition cluster" : "(transitions.bin not loaded)",
        ps->loaded ? "top-5 paragraphs by embedding cosine" : "(paragraph_vectors.bin not loaded)",
        g_temperature, g_w_perplex, g_w_semantic, g_w_transition);
}

static char *trim(char *s) {
    while (*s && isspace((unsigned char)*s)) s++;
    char *end = s + strlen(s);
    while (end > s && isspace((unsigned char)end[-1])) *--end = 0;
    return s;
}

int main(int argc, char **argv) {
    if (argc < 4 || argc > 6) {
        fprintf(stderr,
            "Usage: %s <tokenizer.bin> <model.bin> <vectors.bin> [paragraph_vectors.bin] [transitions.bin]\n",
            argv[0]);
        return 1;
    }
    bpe_tokenizer_t tk;
    model_t m;
    wordvec_t w;
    para_set_t ps = {0};
    trans_set_t ts = {0};

    fprintf(stderr, "Loading tokenizer...\n");
    bpe_load(&tk, argv[1]);
    fprintf(stderr, "  vocab=%u\n", tk.vocab_size);
    fprintf(stderr, "Loading model...\n");
    model_load(&m, argv[2]);
    fprintf(stderr, "  %u-gram\n", m.max_n);
    fprintf(stderr, "Loading word vectors...\n");
    wordvec_load(&w, argv[3]);
    fprintf(stderr, "  vocab=%u dim=%u\n", w.vocab_size, w.dim);
    if (w.vocab_size != tk.vocab_size) {
        fprintf(stderr, "vocab mismatch\n"); return 1;
    }
    if (argc >= 5) {
        fprintf(stderr, "Loading paragraph vectors...\n");
        para_load(&ps, argv[4]);
        fprintf(stderr, "  %s (%u paragraphs)\n", ps.loaded ? "ok" : "FAILED", ps.loaded ? ps.n_paragraphs : 0);
    }
    if (argc >= 6) {
        fprintf(stderr, "Loading transitions...\n");
        trans_load(&ts, argv[5]);
        fprintf(stderr, "  %s (%u clusters)\n", ts.loaded ? "ok" : "FAILED", ts.loaded ? ts.n_clusters : 0);
    }

    fprintf(stdout, "ready. /help for commands.\n\n");
    rng_seed((uint64_t)time(NULL));

    char buf[16384];
    for (;;) {
        fprintf(stdout, "> "); fflush(stdout);
        if (!fgets(buf, sizeof(buf), stdin)) break;
        char *line = trim(buf);
        if (!*line) continue;
        if (line[0] == '/') {
            if (!strcmp(line, "/quit") || !strcmp(line, "/q")) break;
            if (!strcmp(line, "/help")) { cmd_help(&ps, &ts); continue; }
            if (!strncmp(line, "/temp ", 6)) {
                g_temperature = atof(line + 6); if (g_temperature <= 0) g_temperature = 0.01;
                fprintf(stdout, "temperature = %.3f\n", g_temperature); continue;
            }
            if (!strncmp(line, "/weights ", 9)) {
                char *p = line + 9;
                g_w_perplex   = atof(p); p = strchr(p, ' '); if (!p) { fprintf(stdout,"need P S T\n"); continue; }
                g_w_semantic  = atof(p+1); p = strchr(p+1, ' '); if (!p) { fprintf(stdout,"need P S T\n"); continue; }
                g_w_transition= atof(p+1);
                fprintf(stdout, "weights: P=%.2f S=%.2f T=%.2f\n", g_w_perplex, g_w_semantic, g_w_transition);
                continue;
            }
            if (!strncmp(line, "/seed ", 6)) {
                rng_seed((uint64_t)atoll(line + 6)); fprintf(stdout,"reseeded\n"); continue;
            }
            if (!strncmp(line, "/sample ", 8)) {
                char *p = line + 8;
                uint32_t n = (uint32_t)strtoul(p, &p, 10);
                while (*p == ' ') p++;
                if (n == 0) n = DEFAULT_SAMPLE_N;
                cmd_generate(&m, &tk, p, n); continue;
            }
            if (!strncmp(line, "/code ", 6)) {
                char *p = line + 6;
                /* Optional leading count: "/code 120 <prompt>" */
                char *after = p;
                uint32_t n = (uint32_t)strtoul(p, &after, 10);
                if (after != p && (*after == ' ')) { while (*after == ' ') after++; p = after; }
                else n = 0;
                if (n == 0) n = DEFAULT_SAMPLE_N;
                cmd_code(&m, &tk, p, n); continue;
            }
            if (!strncmp(line, "/think ", 7)) {
                char *p = line + 7;
                /* Try to parse "B K D S query" */
                uint32_t vals[4]; int parsed = 0;
                char *q = p;
                while (parsed < 4 && *q) {
                    char *endptr;
                    long v = strtol(q, &endptr, 10);
                    if (endptr == q || v < 0 || (*endptr != ' ' && *endptr != '\0')) break;
                    vals[parsed++] = (uint32_t)v;
                    q = endptr; while (*q == ' ') q++;
                }
                if (parsed == 4 && *q) {
                    cmd_think(&m, &w, &tk, vals[0], vals[1], vals[2], vals[3], q, NULL);
                } else {
                    cmd_think(&m, &w, &tk, DEFAULT_BEAM_B, DEFAULT_BEAM_K, DEFAULT_BEAM_D, DEFAULT_BEAM_SPAN, p, NULL);
                }
                continue;
            }
            if (!strncmp(line, "/branch ", 8))    { cmd_branch(&m, &w, &tk, &ts, line + 8); continue; }
            if (!strncmp(line, "/score ", 7))     { cmd_score(&m, &w, &tk, line + 7); continue; }
            if (!strncmp(line, "/semantic ", 10)) { cmd_semantic(&w, &ps, &tk, line + 10); continue; }
            if (!strncmp(line, "/perplexity ", 12)) { cmd_perplexity(&m, &tk, line + 12); continue; }
            if (!strncmp(line, "/tokens ", 8))    { cmd_tokens(&tk, line + 8); continue; }
            fprintf(stdout, "unknown command. /help\n"); continue;
        }
        cmd_generate(&m, &tk, line, DEFAULT_SAMPLE_N);
    }
    return 0;
}
