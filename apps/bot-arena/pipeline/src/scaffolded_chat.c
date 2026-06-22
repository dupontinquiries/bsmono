/*
 * scaffolded_chat.c — Chat REPL v1.1: retrieval, templates, best-of-N
 *
 * Same n-gram core as chat.c, plus:
 *   - Corpus paragraph index with TF-IDF retrieval
 *   - /retrieve and /rag commands
 *   - /template command for structured multi-stage generation
 *   - /best command for explicit best-of-N sampling
 *
 * Build:
 *   gcc -std=c99 -O3 -Wall -Wextra -o scaffolded_chat scaffolded_chat.c -lz -lm
 *
 * Usage:
 *   ./scaffolded_chat <tokenizer.bin> <model.bin> <corpus_dir>
 *
 * Commands:
 *   <text>               generate continuation (60 tokens)
 *   /sample N <text>     generate N tokens
 *   /best N <text>       best-of-N: generate N continuations, return lowest-perplexity
 *   /retrieve <query>    top-5 paragraphs by TF-IDF similarity
 *   /rag <query>         retrieve top-3 paragraphs as context, then generate
 *   /template <query>    structured multi-stage generation (restate -> consider -> synth)
 *   /next <text>         top-10 next-token candidates
 *   /perplexity <text>   per-token perplexity of <text>
 *   /tokens <text>       show tokenization
 *   /temp X              set temperature (default 0.8)
 *   /seed X              reseed RNG
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
#include <dirent.h>
#include <sys/stat.h>
#include <sys/mman.h>
#include <fcntl.h>
#include <unistd.h>
#include <zlib.h>

#define BPE_MAGIC          "BPETK01"
#define BPE_MAGIC_LEN      8
#define BPE_BASE_VOCAB     256u
#define NGRAM_MAGIC        "NGRAM02"
#define NGRAM_MAGIC_LEN    8
#define MAX_HISTORY        4096
#define DEFAULT_SAMPLE_N   60
#define DEFAULT_BEST_N     8
#define BACKOFF_ALPHA      0.4
#define RAG_TOP_K          3
#define RETRIEVE_TOP_K     5

static uint64_t fnv1a(const void *data, size_t len) {
    const uint8_t *p = (const uint8_t *)data;
    uint64_t h = 0xcbf29ce484222325ULL;
    for (size_t i = 0; i < len; i++) { h ^= p[i]; h *= 0x100000001b3ULL; }
    return h;
}

/* ===== xoshiro256** PRNG ===== */
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

/* ===== Tokenizer (same as chat.c) ===== */
typedef struct { uint64_t key; uint32_t rank; uint32_t new_id; } bpe_merge_entry_t;
typedef struct {
    uint32_t vocab_size, num_merges;
    bpe_merge_entry_t *table; uint32_t table_cap;
    uint8_t **token_bytes; uint32_t *token_byte_lens;
} bpe_tokenizer_t;

static void bpe_load(bpe_tokenizer_t *tk, const char *path) {
    FILE *f = fopen(path, "rb"); if (!f) { perror("tokenizer"); exit(1); }
    char magic[BPE_MAGIC_LEN];
    if (fread(magic, 1, BPE_MAGIC_LEN, f) != BPE_MAGIC_LEN || memcmp(magic, BPE_MAGIC, 7) != 0)
        { fprintf(stderr, "bad tokenizer magic\n"); exit(1); }
    uint32_t version, vocab_size, num_merges;
    uint64_t corpus_bytes, uncompressed, compressed;
    if (fread(&version,4,1,f)!=1 || fread(&vocab_size,4,1,f)!=1 || fread(&num_merges,4,1,f)!=1 ||
        fread(&corpus_bytes,8,1,f)!=1 || fread(&uncompressed,8,1,f)!=1 || fread(&compressed,8,1,f)!=1)
        { fprintf(stderr, "tokenizer header\n"); exit(1); }
    (void)version; (void)corpus_bytes;
    uint8_t *cbuf = malloc(compressed);
    if (!cbuf || fread(cbuf,1,compressed,f) != compressed) { fprintf(stderr,"tk read\n"); exit(1); }
    fclose(f);
    uint8_t *ubuf = malloc(uncompressed);
    uLongf ulen = (uLongf)uncompressed;
    if (uncompress(ubuf, &ulen, cbuf, (uLong)compressed) != Z_OK) { fprintf(stderr,"tk decomp\n"); exit(1); }
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

static bool bpe_lookup(const bpe_tokenizer_t *tk, uint32_t a, uint32_t b, uint32_t *out_rank, uint32_t *out_new_id) {
    uint64_t key = ((uint64_t)a << 32) | b;
    uint64_t hash = fnv1a(&key, 8);
    uint32_t mask = tk->table_cap - 1;
    uint32_t idx = (uint32_t)(hash & mask);
    while (tk->table[idx].new_id != 0) {
        if (tk->table[idx].key == key) { *out_rank = tk->table[idx].rank; *out_new_id = tk->table[idx].new_id; return true; }
        idx = (idx + 1) & mask;
    }
    return false;
}

static uint32_t bpe_apply_chunk(const bpe_tokenizer_t *tk, uint32_t *tokens, uint32_t len) {
    while (len >= 2) {
        uint32_t best_pos = UINT32_MAX, best_rank = UINT32_MAX, best_new_id = 0;
        for (uint32_t i = 0; i + 1 < len; i++) {
            uint32_t r, nid;
            if (bpe_lookup(tk, tokens[i], tokens[i+1], &r, &nid)) {
                if (r < best_rank) { best_rank = r; best_pos = i; best_new_id = nid; }
            }
        }
        if (best_pos == UINT32_MAX) break;
        tokens[best_pos] = best_new_id;
        for (uint32_t i = best_pos + 1; i + 1 < len; i++) tokens[i] = tokens[i+1];
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
    uint32_t out_n = 0;
    size_t i = 0;
    uint32_t chunk[2048];
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

/* Detokenize to a string buffer */
static uint32_t detok_to_buf(const bpe_tokenizer_t *tk, const uint32_t *tokens, uint32_t n,
                              char *out, uint32_t out_cap) {
    uint32_t pos = 0;
    for (uint32_t i = 0; i < n; i++) {
        uint32_t id = tokens[i];
        if (id >= tk->vocab_size) continue;
        uint32_t l = tk->token_byte_lens[id];
        if (pos + l >= out_cap) break;
        memcpy(out + pos, tk->token_bytes[id], l);
        pos += l;
    }
    out[pos] = 0;
    return pos;
}

/* ===== N-gram model (same as chat.c) ===== */
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
    FILE *f = fopen(path, "rb"); if (!f) { perror("model"); exit(1); }
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
                             double temperature, uint32_t *cand_next, uint64_t *cand_count,
                             uint32_t cand_cap) {
    for (uint32_t n = m->max_n; n >= 2; n--) {
        if (n - 1 > hlen) continue;
        const uint32_t *ctx = history + (hlen - (n - 1));
        uint64_t total;
        uint32_t num = order_continuations(&m->orders[n-1], ctx, n-1, cand_next, cand_count, cand_cap, &total);
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

/* ===== Corpus paragraph index with TF-IDF ===== */
typedef struct {
    uint32_t *tokens;
    uint32_t  ntokens;
    char     *text;        /* original text */
    uint32_t  text_len;
    char     *source;      /* filepath */
} paragraph_t;

typedef struct {
    paragraph_t *paragraphs;
    uint32_t     n_paragraphs;
    uint32_t     cap;
    double      *idf;          /* idf[vocab_id] */
    uint32_t     vocab_size;
} corpus_index_t;

static void ci_init(corpus_index_t *ci, uint32_t vocab_size) {
    ci->paragraphs = NULL; ci->n_paragraphs = 0; ci->cap = 0;
    ci->vocab_size = vocab_size;
    ci->idf = calloc(vocab_size, sizeof(double));
}

static void ci_add_paragraph(corpus_index_t *ci, const bpe_tokenizer_t *tk,
                              const char *src, const char *text, uint32_t text_len) {
    if (text_len < 16) return;
    if (ci->n_paragraphs >= ci->cap) {
        ci->cap = ci->cap ? ci->cap * 2 : 1024;
        ci->paragraphs = realloc(ci->paragraphs, ci->cap * sizeof(paragraph_t));
    }
    paragraph_t *p = &ci->paragraphs[ci->n_paragraphs++];
    p->text = malloc(text_len + 1);
    memcpy(p->text, text, text_len);
    p->text[text_len] = 0;
    p->text_len = text_len;
    p->source = strdup(src);
    /* Tokenize */
    uint32_t buf[8192];
    p->ntokens = tokenize(tk, (const uint8_t *)text, text_len, buf, 8192);
    p->tokens = malloc(p->ntokens * sizeof(uint32_t));
    memcpy(p->tokens, buf, p->ntokens * sizeof(uint32_t));
}

static void ci_compute_idf(corpus_index_t *ci) {
    uint32_t *df = calloc(ci->vocab_size, sizeof(uint32_t));
    uint8_t *seen = malloc(ci->vocab_size);
    for (uint32_t pi = 0; pi < ci->n_paragraphs; pi++) {
        memset(seen, 0, ci->vocab_size);
        paragraph_t *p = &ci->paragraphs[pi];
        for (uint32_t i = 0; i < p->ntokens; i++) {
            uint32_t t = p->tokens[i];
            if (!seen[t]) { df[t]++; seen[t] = 1; }
        }
    }
    for (uint32_t i = 0; i < ci->vocab_size; i++) {
        if (df[i] == 0) { ci->idf[i] = 0; continue; }
        ci->idf[i] = log(1.0 + (double)ci->n_paragraphs / (double)df[i]);
    }
    free(df); free(seen);
}

/* TF-IDF cosine similarity between paragraph and query token list */
static double ci_score(const corpus_index_t *ci, const paragraph_t *p,
                       const uint32_t *qtoks, uint32_t qn) {
    /* Compute query TF as histogram (sparse via temporary) */
    /* For prototype: O(qn * p->ntokens) direct */
    /* Build paragraph histogram */
    /* We'll use a quick stack-allocated approach: */
    /* Better: precompute paragraph TF norms and use only matching tokens */

    /* Simplest: compute dot of TF-IDF vectors via O(p->ntokens) walk */
    /* Use a temporary histogram for query */
    static uint32_t q_hist[65536]; /* assumes vocab_size <= 65536 */
    static uint32_t p_hist[65536];
    if (ci->vocab_size > 65536) return 0;
    memset(q_hist, 0, ci->vocab_size * sizeof(uint32_t));
    memset(p_hist, 0, ci->vocab_size * sizeof(uint32_t));
    for (uint32_t i = 0; i < qn; i++) q_hist[qtoks[i]]++;
    for (uint32_t i = 0; i < p->ntokens; i++) p_hist[p->tokens[i]]++;
    double dot = 0, qnorm = 0, pnorm = 0;
    for (uint32_t i = 0; i < ci->vocab_size; i++) {
        if (q_hist[i] == 0 && p_hist[i] == 0) continue;
        double idf = ci->idf[i];
        double qv = (double)q_hist[i] * idf;
        double pv = (double)p_hist[i] * idf;
        dot += qv * pv;
        qnorm += qv * qv;
        pnorm += pv * pv;
    }
    if (qnorm == 0 || pnorm == 0) return 0;
    return dot / sqrt(qnorm * pnorm);
}

static uint32_t ci_retrieve(const corpus_index_t *ci, const bpe_tokenizer_t *tk,
                             const char *query, uint32_t *out_idx, double *out_score,
                             uint32_t k) {
    uint32_t qtoks[8192];
    uint32_t qn = tokenize(tk, (const uint8_t *)query, strlen(query), qtoks, 8192);
    if (qn == 0) return 0;
    /* Compute all scores */
    double *scores = malloc(ci->n_paragraphs * sizeof(double));
    for (uint32_t i = 0; i < ci->n_paragraphs; i++) {
        scores[i] = ci_score(ci, &ci->paragraphs[i], qtoks, qn);
    }
    /* Top-k selection (insertion-based, k is small) */
    uint32_t found = 0;
    for (uint32_t i = 0; i < ci->n_paragraphs; i++) {
        if (scores[i] <= 0) continue;
        if (found < k) {
            /* Insert in sorted position */
            uint32_t pos = found;
            while (pos > 0 && out_score[pos - 1] < scores[i]) {
                out_score[pos] = out_score[pos - 1];
                out_idx[pos] = out_idx[pos - 1];
                pos--;
            }
            out_score[pos] = scores[i];
            out_idx[pos] = i;
            found++;
        } else if (scores[i] > out_score[k - 1]) {
            uint32_t pos = k - 1;
            while (pos > 0 && out_score[pos - 1] < scores[i]) {
                out_score[pos] = out_score[pos - 1];
                out_idx[pos] = out_idx[pos - 1];
                pos--;
            }
            out_score[pos] = scores[i];
            out_idx[pos] = i;
        }
    }
    free(scores);
    return found;
}

/* Walk corpus directory, split files into paragraphs, add to index */
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

static void index_file(const char *path, const bpe_tokenizer_t *tk, corpus_index_t *ci) {
    int fd = open(path, O_RDONLY); if (fd < 0) return;
    struct stat st;
    if (fstat(fd, &st) < 0 || st.st_size == 0) { close(fd); return; }
    void *p = mmap(NULL, st.st_size, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED) { close(fd); return; }
    const char *text = (const char *)p;
    size_t len = st.st_size;
    /* Split on blank lines (paragraph boundary = \n\n) */
    size_t i = 0;
    while (i < len) {
        size_t start = i;
        while (i < len) {
            if (i + 1 < len && text[i] == '\n' && text[i+1] == '\n') break;
            i++;
        }
        if (i > start) {
            ci_add_paragraph(ci, tk, path, text + start, (uint32_t)(i - start));
        }
        while (i < len && text[i] == '\n') i++;
    }
    munmap(p, st.st_size);
    close(fd);
}

static void index_walk(const char *dir, const bpe_tokenizer_t *tk, corpus_index_t *ci) {
    DIR *d = opendir(dir); if (!d) return;
    struct dirent *e; char path[4096];
    while ((e = readdir(d))) {
        if (e->d_name[0] == '.') continue;
        int n = snprintf(path, sizeof(path), "%s/%s", dir, e->d_name);
        if (n < 0 || (size_t)n >= sizeof(path)) continue;
        struct stat st; if (stat(path, &st) < 0) continue;
        if (S_ISDIR(st.st_mode)) index_walk(path, tk, ci);
        else if (S_ISREG(st.st_mode) && has_corpus_ext(e->d_name)) index_file(path, tk, ci);
    }
    closedir(d);
}

/* ===== Generation + scoring ===== */
static double g_temperature = 0.8;

/* Generate continuation, append to history buffer, return final length */
static uint32_t generate_into(const model_t *m, const bpe_tokenizer_t *tk,
                               uint32_t *history, uint32_t hlen, uint32_t n_tokens,
                               double temperature) {
    uint32_t cand_next[16384];
    uint64_t cand_count[16384];
    (void)tk;
    for (uint32_t i = 0; i < n_tokens && hlen < MAX_HISTORY; i++) {
        uint32_t tok = sample_next(m, history, hlen, temperature, cand_next, cand_count, 16384);
        history[hlen++] = tok;
    }
    return hlen;
}

/* Compute total perplexity of a token sequence (from start to end) */
static double sequence_perplexity(const model_t *m, const uint32_t *tokens, uint32_t n, uint32_t skip) {
    if (n <= skip + 1) return INFINITY;
    double sum = 0;
    uint32_t scored = 0;
    for (uint32_t i = skip + 1; i < n; i++) {
        sum += logprob_next(m, tokens, i, tokens[i]);
        scored++;
    }
    return exp(-sum / (double)scored);
}

static void cmd_generate(const model_t *m, const bpe_tokenizer_t *tk,
                          const char *prompt, uint32_t n_tokens) {
    uint32_t history[MAX_HISTORY];
    uint32_t hlen = tokenize(tk, (const uint8_t *)prompt, strlen(prompt), history, MAX_HISTORY);
    detok_print(tk, history, hlen, stdout);
    uint32_t new_hlen = generate_into(m, tk, history, hlen, n_tokens, g_temperature);
    detok_print(tk, history + hlen, new_hlen - hlen, stdout);
    fputc('\n', stdout);
}

/* Best-of-N: generate N completions, score each by perplexity of just the generated portion,
 * return the one with lowest perplexity (most "you"-like by your own model) */
static void cmd_best(const model_t *m, const bpe_tokenizer_t *tk,
                      const char *prompt, uint32_t N, uint32_t n_tokens) {
    if (N == 0) N = DEFAULT_BEST_N;
    uint32_t prompt_history[MAX_HISTORY];
    uint32_t prompt_len = tokenize(tk, (const uint8_t *)prompt, strlen(prompt), prompt_history, MAX_HISTORY);

    fprintf(stdout, "[generating %u candidates]\n", N);
    fprintf(stdout, "----\n");
    detok_print(tk, prompt_history, prompt_len, stdout);

    uint32_t *best_history = malloc(MAX_HISTORY * sizeof(uint32_t));
    uint32_t best_hlen = 0;
    double best_ppl = INFINITY;

    uint32_t *cur = malloc(MAX_HISTORY * sizeof(uint32_t));
    for (uint32_t i = 0; i < N; i++) {
        memcpy(cur, prompt_history, prompt_len * sizeof(uint32_t));
        uint32_t hlen = generate_into(m, tk, cur, prompt_len, n_tokens, g_temperature);
        double ppl = sequence_perplexity(m, cur, hlen, prompt_len);
        fprintf(stdout, "[candidate %u, ppl=%.2f, %u toks generated]\n", i + 1, ppl, hlen - prompt_len);
        if (ppl < best_ppl) {
            best_ppl = ppl; best_hlen = hlen;
            memcpy(best_history, cur, hlen * sizeof(uint32_t));
        }
    }
    fprintf(stdout, "----\n[best: ppl=%.2f]\n", best_ppl);
    detok_print(tk, best_history + prompt_len, best_hlen - prompt_len, stdout);
    fputc('\n', stdout);
    free(best_history); free(cur);
}

static void cmd_retrieve(const corpus_index_t *ci, const bpe_tokenizer_t *tk, const char *query) {
    uint32_t idx[RETRIEVE_TOP_K];
    double score[RETRIEVE_TOP_K];
    uint32_t found = ci_retrieve(ci, tk, query, idx, score, RETRIEVE_TOP_K);
    if (found == 0) { fprintf(stdout, "no matches.\n"); return; }
    for (uint32_t i = 0; i < found; i++) {
        const paragraph_t *p = &ci->paragraphs[idx[i]];
        fprintf(stdout, "\n[%u] score=%.4f  source=%s\n", i + 1, score[i], p->source);
        /* Print up to 300 chars */
        uint32_t show = p->text_len > 300 ? 300 : p->text_len;
        fwrite(p->text, 1, show, stdout);
        if (p->text_len > 300) fprintf(stdout, "...");
        fputc('\n', stdout);
    }
}

static void cmd_rag(const model_t *m, const corpus_index_t *ci, const bpe_tokenizer_t *tk,
                     const char *query) {
    uint32_t idx[RAG_TOP_K];
    double score[RAG_TOP_K];
    uint32_t found = ci_retrieve(ci, tk, query, idx, score, RAG_TOP_K);
    if (found == 0) { fprintf(stdout, "no context found, generating without RAG\n"); }

    /* Build augmented prompt: concatenate retrieved paragraphs + query */
    char prompt[16384] = {0};
    size_t pos = 0;
    for (uint32_t i = 0; i < found; i++) {
        const paragraph_t *p = &ci->paragraphs[idx[i]];
        uint32_t take = p->text_len > 1024 ? 1024 : p->text_len;
        if (pos + take + 4 >= sizeof(prompt)) break;
        memcpy(prompt + pos, p->text, take); pos += take;
        prompt[pos++] = '\n'; prompt[pos++] = '\n';
    }
    size_t qlen = strlen(query);
    if (pos + qlen < sizeof(prompt)) {
        memcpy(prompt + pos, query, qlen); pos += qlen;
    }
    prompt[pos] = 0;

    fprintf(stdout, "[retrieved %u contexts, generating]\n----\n", found);
    cmd_generate(m, tk, prompt, DEFAULT_SAMPLE_N);
}

/* Structured template: restate -> consider (3) -> synthesize */
static void cmd_template(const model_t *m, const bpe_tokenizer_t *tk, const char *query) {
    char buf[16384];
    uint32_t history[MAX_HISTORY];
    uint32_t hlen;

    fprintf(stdout, "[structured template]\n\n");

    /* Stage 1: restate */
    snprintf(buf, sizeof(buf), "%s\n\nIn other words: ", query);
    hlen = tokenize(tk, (const uint8_t *)buf, strlen(buf), history, MAX_HISTORY);
    fprintf(stdout, "Restated: ");
    uint32_t mark = hlen;
    hlen = generate_into(m, tk, history, hlen, 40, g_temperature);
    detok_print(tk, history + mark, hlen - mark, stdout);
    fprintf(stdout, "\n\n");

    /* Stage 2-4: three considerations */
    for (int c = 1; c <= 3; c++) {
        char so_far[16384] = {0};
        detok_to_buf(tk, history, hlen, so_far, sizeof(so_far) - 1);
        snprintf(buf, sizeof(buf), "%s\n\nConsideration %d: ", so_far, c);
        hlen = tokenize(tk, (const uint8_t *)buf, strlen(buf), history, MAX_HISTORY);
        fprintf(stdout, "Consideration %d: ", c);
        mark = hlen;
        hlen = generate_into(m, tk, history, hlen, 50, g_temperature);
        detok_print(tk, history + mark, hlen - mark, stdout);
        fprintf(stdout, "\n\n");
    }

    /* Stage 5: synthesis */
    char so_far[16384] = {0};
    detok_to_buf(tk, history, hlen, so_far, sizeof(so_far) - 1);
    snprintf(buf, sizeof(buf), "%s\n\nSynthesis: ", so_far);
    hlen = tokenize(tk, (const uint8_t *)buf, strlen(buf), history, MAX_HISTORY);
    fprintf(stdout, "Synthesis: ");
    mark = hlen;
    hlen = generate_into(m, tk, history, hlen, 80, g_temperature);
    detok_print(tk, history + mark, hlen - mark, stdout);
    fprintf(stdout, "\n");
}

static void cmd_next(const model_t *m, const bpe_tokenizer_t *tk, const char *prompt) {
    uint32_t history[MAX_HISTORY];
    uint32_t hlen = tokenize(tk, (const uint8_t *)prompt, strlen(prompt), history, MAX_HISTORY);
    uint32_t cn[16384]; uint64_t cc[16384];
    uint32_t used_n = 0, num = 0; uint64_t total = 0;
    for (uint32_t n = m->max_n; n >= 2; n--) {
        if (n - 1 > hlen) continue;
        const uint32_t *ctx = history + (hlen - (n - 1));
        num = order_continuations(&m->orders[n-1], ctx, n-1, cn, cc, 16384, &total);
        if (num > 0) { used_n = n; break; }
    }
    if (num == 0) { fprintf(stdout, "no n-gram match\n"); return; }
    uint32_t topk = num < 10 ? num : 10;
    for (uint32_t k = 0; k < topk; k++) {
        uint32_t best = k;
        for (uint32_t j = k + 1; j < num; j++) if (cc[j] > cc[best]) best = j;
        if (best != k) {
            uint64_t tc = cc[best]; cc[best] = cc[k]; cc[k] = tc;
            uint32_t tn = cn[best]; cn[best] = cn[k]; cn[k] = tn;
        }
    }
    fprintf(stdout, "order %u, %u cands, total %llu:\n", used_n, num, (unsigned long long)total);
    for (uint32_t i = 0; i < topk; i++) {
        fprintf(stdout, "  %6.3f  '", (double)cc[i] / (double)total);
        detok_print(tk, &cn[i], 1, stdout);
        fprintf(stdout, "'\n");
    }
}

static void cmd_perplexity(const model_t *m, const bpe_tokenizer_t *tk, const char *text) {
    uint32_t history[MAX_HISTORY];
    uint32_t hlen = tokenize(tk, (const uint8_t *)text, strlen(text), history, MAX_HISTORY);
    if (hlen < 2) { fprintf(stdout, "need >= 2 tokens\n"); return; }
    double ppl = sequence_perplexity(m, history, hlen, 0);
    fprintf(stdout, "tokens=%u  ppl=%.2f\n", hlen, ppl);
}

static void cmd_tokens(const bpe_tokenizer_t *tk, const char *text) {
    uint32_t history[MAX_HISTORY];
    uint32_t hlen = tokenize(tk, (const uint8_t *)text, strlen(text), history, MAX_HISTORY);
    fprintf(stdout, "%u tokens:\n", hlen);
    for (uint32_t i = 0; i < hlen; i++) {
        fprintf(stdout, "  [%5u] '", history[i]);
        uint8_t *b = tk->token_bytes[history[i]];
        uint32_t n = tk->token_byte_lens[history[i]];
        for (uint32_t j = 0; j < n; j++) {
            uint8_t c = b[j];
            if (c == '\n') fprintf(stdout, "\\n");
            else if (c == '\t') fprintf(stdout, "\\t");
            else if (c < 32 || c >= 127) fprintf(stdout, "\\x%02x", c);
            else fputc(c, stdout);
        }
        fprintf(stdout, "'\n");
    }
}

static void cmd_help(void) {
    fprintf(stdout,
        "commands:\n"
        "  <text>              generate 60-token continuation\n"
        "  /sample N <text>    generate N tokens\n"
        "  /best N <text>      best-of-N: lowest-perplexity continuation\n"
        "  /retrieve <query>   top-5 paragraphs by TF-IDF\n"
        "  /rag <query>        retrieve top-3 paragraphs as context, then generate\n"
        "  /template <query>   structured: restate -> 3 considerations -> synth\n"
        "  /next <text>        top-10 next-token candidates\n"
        "  /perplexity <text>  per-token perplexity\n"
        "  /tokens <text>      tokenization view\n"
        "  /temp X             set temperature (now %.2f)\n"
        "  /seed X             reseed RNG\n"
        "  /help | /quit\n", g_temperature);
}

static char *trim(char *s) {
    while (*s && isspace((unsigned char)*s)) s++;
    char *end = s + strlen(s);
    while (end > s && isspace((unsigned char)end[-1])) *--end = 0;
    return s;
}

int main(int argc, char **argv) {
    if (argc != 4) {
        fprintf(stderr, "Usage: %s <tokenizer.bin> <model.bin> <corpus_dir>\n", argv[0]);
        return 1;
    }
    bpe_tokenizer_t tk;
    model_t m;
    corpus_index_t ci;
    fprintf(stderr, "Loading tokenizer...\n");
    bpe_load(&tk, argv[1]);
    fprintf(stderr, "  vocab=%u merges=%u\n", tk.vocab_size, tk.num_merges);
    fprintf(stderr, "Loading model...\n");
    model_load(&m, argv[2]);
    fprintf(stderr, "  %u-gram, %llu corpus tokens\n", m.max_n, (unsigned long long)m.total_tokens);
    fprintf(stderr, "Indexing corpus paragraphs...\n");
    ci_init(&ci, tk.vocab_size);
    index_walk(argv[3], &tk, &ci);
    ci_compute_idf(&ci);
    fprintf(stderr, "  %u paragraphs indexed\n", ci.n_paragraphs);
    fprintf(stdout, "ready. type something, or /help for commands.\n\n");

    rng_seed((uint64_t)time(NULL));

    char buf[16384];
    for (;;) {
        fprintf(stdout, "> "); fflush(stdout);
        if (!fgets(buf, sizeof(buf), stdin)) break;
        char *line = trim(buf);
        if (!*line) continue;
        if (line[0] == '/') {
            if (!strcmp(line, "/quit") || !strcmp(line, "/q")) break;
            if (!strcmp(line, "/help")) { cmd_help(); continue; }
            if (!strncmp(line, "/temp ", 6)) {
                g_temperature = atof(line + 6);
                if (g_temperature <= 0) g_temperature = 0.01;
                fprintf(stdout, "temperature = %.3f\n", g_temperature); continue;
            }
            if (!strncmp(line, "/seed ", 6)) {
                rng_seed((uint64_t)atoll(line + 6));
                fprintf(stdout, "reseeded\n"); continue;
            }
            if (!strncmp(line, "/sample ", 8)) {
                char *p = line + 8;
                uint32_t n = (uint32_t)strtoul(p, &p, 10);
                while (*p == ' ') p++;
                if (n == 0) n = DEFAULT_SAMPLE_N;
                cmd_generate(&m, &tk, p, n);
                continue;
            }
            if (!strncmp(line, "/best ", 6)) {
                char *p = line + 6;
                uint32_t n = (uint32_t)strtoul(p, &p, 10);
                while (*p == ' ') p++;
                cmd_best(&m, &tk, p, n, DEFAULT_SAMPLE_N);
                continue;
            }
            if (!strncmp(line, "/retrieve ", 10)) { cmd_retrieve(&ci, &tk, line + 10); continue; }
            if (!strncmp(line, "/rag ", 5))      { cmd_rag(&m, &ci, &tk, line + 5); continue; }
            if (!strncmp(line, "/template ", 10)){ cmd_template(&m, &tk, line + 10); continue; }
            if (!strncmp(line, "/next ", 6))     { cmd_next(&m, &tk, line + 6); continue; }
            if (!strncmp(line, "/perplexity ", 12)) { cmd_perplexity(&m, &tk, line + 12); continue; }
            if (!strncmp(line, "/tokens ", 8))   { cmd_tokens(&tk, line + 8); continue; }
            fprintf(stdout, "unknown command. /help\n"); continue;
        }
        cmd_generate(&m, &tk, line, DEFAULT_SAMPLE_N);
    }
    return 0;
}
