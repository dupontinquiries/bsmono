/*
 * chat.c — Tiny REPL for the BPE n-gram language model
 *
 * Loads tokenizer.bin and model.bin, runs an stdin/stdout loop with
 * generation (stupid-backoff sampling), tokenization inspection,
 * top-K next-token preview, and per-token perplexity scoring.
 *
 * Build:
 *   gcc -std=c99 -O3 -Wall -Wextra -o chat chat.c -lz -lm
 *
 * Usage:
 *   ./chat <tokenizer.bin> <model.bin>
 *
 * Commands:
 *   <anything>           generate continuation (60 tokens default)
 *   /sample N <text>     generate N tokens after <text>
 *   /next <text>         show top-10 next-token candidates
 *   /perplexity <text>   per-token perplexity of <text>
 *   /tokens <text>       show how <text> tokenizes
 *   /temp X              set sampling temperature (default 0.8)
 *   /seed X              reseed the RNG
 *   /help                command list
 *   /quit  or  /q        exit
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
#include <zlib.h>

#define BPE_MAGIC          "BPETK01"
#define BPE_MAGIC_LEN      8
#define BPE_BASE_VOCAB     256u
#define NGRAM_MAGIC        "NGRAM02"
#define NGRAM_MAGIC_LEN    8
#define MAX_HISTORY        4096
#define DEFAULT_SAMPLE_N   60
#define BACKOFF_ALPHA      0.4

/* ===== FNV-1a ===== */
static uint64_t fnv1a(const void *data, size_t len) {
    const uint8_t *p = (const uint8_t *)data;
    uint64_t h = 0xcbf29ce484222325ULL;
    for (size_t i = 0; i < len; i++) { h ^= p[i]; h *= 0x100000001b3ULL; }
    return h;
}

/* ===== xoshiro256** PRNG ===== */
static uint64_t rng_state[4];
static uint64_t rng_next(void) {
    uint64_t r = rng_state[1] * 5;
    r = ((r << 7) | (r >> 57)) * 9;
    uint64_t t = rng_state[1] << 17;
    rng_state[2] ^= rng_state[0];
    rng_state[3] ^= rng_state[1];
    rng_state[1] ^= rng_state[2];
    rng_state[0] ^= rng_state[3];
    rng_state[2] ^= t;
    rng_state[3] = ((rng_state[3] << 45) | (rng_state[3] >> 19));
    return r;
}
static void rng_seed(uint64_t s) {
    for (int i = 0; i < 4; i++) {
        s += 0x9E3779B97F4A7C15ULL;
        uint64_t z = s;
        z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ULL;
        z = (z ^ (z >> 27)) * 0x94D049BB133111EBULL;
        rng_state[i] = z ^ (z >> 31);
    }
    /* Stir a few times */
    for (int i = 0; i < 16; i++) (void)rng_next();
}
static double rng_uniform(void) {
    return (rng_next() >> 11) * (1.0 / (double)(1ULL << 53));
}

/* ===== Tokenizer ===== */
typedef struct {
    uint64_t key;
    uint32_t rank;
    uint32_t new_id;   /* 0 = empty */
} bpe_merge_entry_t;

typedef struct {
    uint32_t            vocab_size;
    uint32_t            num_merges;
    bpe_merge_entry_t  *table;
    uint32_t            table_cap;
    uint8_t           **token_bytes;
    uint32_t           *token_byte_lens;
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
    if (fread(&version, 4, 1, f) != 1 ||
        fread(&vocab_size, 4, 1, f) != 1 ||
        fread(&num_merges, 4, 1, f) != 1 ||
        fread(&corpus_bytes, 8, 1, f) != 1 ||
        fread(&uncompressed, 8, 1, f) != 1 ||
        fread(&compressed, 8, 1, f) != 1)
    { fprintf(stderr, "tokenizer header failed\n"); exit(1); }
    (void)version; (void)corpus_bytes;

    uint8_t *cbuf = malloc(compressed);
    if (!cbuf || fread(cbuf, 1, compressed, f) != compressed)
    { fprintf(stderr, "tokenizer read failed\n"); exit(1); }
    fclose(f);

    uint8_t *ubuf = malloc(uncompressed);
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
    tk->token_bytes     = calloc(vocab_size, sizeof(uint8_t *));
    tk->token_byte_lens = calloc(vocab_size, sizeof(uint32_t));
    if (!tk->table || !tk->token_bytes || !tk->token_byte_lens)
    { perror("bpe_load alloc"); exit(1); }

    for (uint32_t i = 0; i < BPE_BASE_VOCAB; i++) {
        tk->token_bytes[i] = malloc(1);
        tk->token_bytes[i][0] = (uint8_t)i;
        tk->token_byte_lens[i] = 1;
    }

    uint32_t *p32 = (uint32_t *)ubuf;
    uint32_t mask = cap - 1;
    for (uint32_t i = 0; i < num_merges; i++) {
        uint32_t a = p32[i * 2 + 0];
        uint32_t b = p32[i * 2 + 1];
        uint32_t new_id = BPE_BASE_VOCAB + i;
        uint64_t key = ((uint64_t)a << 32) | (uint64_t)b;
        uint64_t hash = fnv1a(&key, 8);
        uint32_t idx = (uint32_t)(hash & mask);
        while (tk->table[idx].new_id != 0) idx = (idx + 1) & mask;
        tk->table[idx].key    = key;
        tk->table[idx].rank   = i;
        tk->table[idx].new_id = new_id;

        uint32_t la = tk->token_byte_lens[a];
        uint32_t lb = tk->token_byte_lens[b];
        uint32_t ln = la + lb;
        uint8_t *bytes = malloc(ln);
        memcpy(bytes, tk->token_bytes[a], la);
        memcpy(bytes + la, tk->token_bytes[b], lb);
        tk->token_bytes[new_id] = bytes;
        tk->token_byte_lens[new_id] = ln;
    }
    free(ubuf);
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
            *out_rank = tk->table[idx].rank;
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
        uint32_t best_pos = UINT32_MAX, best_rank = UINT32_MAX, best_new_id = 0;
        for (uint32_t i = 0; i + 1 < len; i++) {
            uint32_t r, nid;
            if (bpe_lookup(tk, tokens[i], tokens[i + 1], &r, &nid)) {
                if (r < best_rank) {
                    best_rank = r;
                    best_pos = i;
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

typedef enum { CC_ALPHA, CC_WHITE, CC_OTHER } char_class_t;
static char_class_t classify(uint8_t c) {
    if ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
        (c >= '0' && c <= '9') || c == '_') return CC_ALPHA;
    if (c == ' ' || c == '\t' || c == '\n' || c == '\r') return CC_WHITE;
    return CC_OTHER;
}

static uint32_t tokenize(const bpe_tokenizer_t *tk,
                         const uint8_t *text, size_t len,
                         uint32_t *out, uint32_t out_cap)
{
    uint32_t out_n = 0;
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
            uint32_t nlen = bpe_apply_chunk(tk, chunk, take);
            for (uint32_t k = 0; k < nlen; k++) {
                if (out_n >= out_cap) return out_n;
                out[out_n++] = chunk[k];
            }
            start += take;
            clen -= take;
        }
    }
    return out_n;
}

static void detok_print(const bpe_tokenizer_t *tk,
                        const uint32_t *tokens, uint32_t n, FILE *out)
{
    for (uint32_t i = 0; i < n; i++) {
        uint32_t id = tokens[i];
        if (id >= tk->vocab_size) continue;
        fwrite(tk->token_bytes[id], 1, tk->token_byte_lens[id], out);
    }
    fflush(out);
}

/* ===== N-gram model ===== */
typedef struct {
    uint32_t  n;
    uint32_t  count;
    uint8_t  *entries;
    size_t    entry_size;
} ngram_order_t;

typedef struct { double D1, D2, D3plus; } discount_t;

typedef struct {
    uint32_t        max_n;
    uint32_t        vocab_size;
    uint64_t        total_tokens;
    ngram_order_t  *orders;
    discount_t     *discounts;
} model_t;

/* qsort comparator with size carried in a file-scope variable */
static size_t g_es;
static int cmp_entry(const void *a, const void *b) {
    const uint32_t *ta = (const uint32_t *)a;
    const uint32_t *tb = (const uint32_t *)b;
    size_t n = (g_es - 8) / 4;
    for (size_t i = 0; i < n; i++) {
        if (ta[i] < tb[i]) return -1;
        if (ta[i] > tb[i]) return 1;
    }
    return 0;
}

static void model_load(model_t *m, const char *path) {
    FILE *f = fopen(path, "rb");
    if (!f) { perror("open model"); exit(1); }
    char magic[NGRAM_MAGIC_LEN];
    if (fread(magic, 1, NGRAM_MAGIC_LEN, f) != NGRAM_MAGIC_LEN ||
        memcmp(magic, NGRAM_MAGIC, 7) != 0)
    { fprintf(stderr, "bad model magic\n"); exit(1); }
    uint32_t version, max_n, vocab_size;
    uint64_t total_tokens, uncompressed, compressed;
    if (fread(&version, 4, 1, f) != 1 ||
        fread(&max_n, 4, 1, f) != 1 ||
        fread(&vocab_size, 4, 1, f) != 1 ||
        fread(&total_tokens, 8, 1, f) != 1 ||
        fread(&uncompressed, 8, 1, f) != 1 ||
        fread(&compressed, 8, 1, f) != 1)
    { fprintf(stderr, "model header failed\n"); exit(1); }
    (void)version;

    uint8_t *cbuf = malloc(compressed);
    if (!cbuf || fread(cbuf, 1, compressed, f) != compressed)
    { fprintf(stderr, "model read failed\n"); exit(1); }
    fclose(f);

    uint8_t *ubuf = malloc(uncompressed);
    uLongf ulen = (uLongf)uncompressed;
    if (uncompress(ubuf, &ulen, cbuf, (uLong)compressed) != Z_OK)
    { fprintf(stderr, "model decompress failed\n"); exit(1); }
    free(cbuf);

    m->max_n = max_n;
    m->vocab_size = vocab_size;
    m->total_tokens = total_tokens;
    m->orders = calloc(max_n, sizeof(ngram_order_t));
    m->discounts = calloc(max_n, sizeof(discount_t));

    uint8_t *p = ubuf;
    for (uint32_t oi = 0; oi < max_n; oi++) {
        uint32_t n, cnt;
        memcpy(&n, p, 4); p += 4;
        memcpy(&cnt, p, 4); p += 4;
        size_t es = (size_t)n * 4 + 8;
        uint8_t *entries = malloc((size_t)cnt * es + 1);
        memcpy(entries, p, (size_t)cnt * es);
        p += (size_t)cnt * es;
        m->orders[oi].n = n;
        m->orders[oi].count = cnt;
        m->orders[oi].entries = entries;
        m->orders[oi].entry_size = es;
    }
    for (uint32_t oi = 0; oi < max_n; oi++) {
        memcpy(&m->discounts[oi].D1, p, 8); p += 8;
        memcpy(&m->discounts[oi].D2, p, 8); p += 8;
        memcpy(&m->discounts[oi].D3plus, p, 8); p += 8;
    }
    free(ubuf);

    /* Sort each order's entries lexicographically */
    for (uint32_t i = 0; i < max_n; i++) {
        if (m->orders[i].count <= 1) continue;
        g_es = m->orders[i].entry_size;
        qsort(m->orders[i].entries, m->orders[i].count,
              m->orders[i].entry_size, cmp_entry);
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

/* Find the first index in `o` whose first `plen` tokens are >= prefix. */
static uint32_t order_lower_bound(const ngram_order_t *o,
                                   const uint32_t *prefix, uint32_t plen)
{
    uint32_t lo = 0, hi = o->count;
    while (lo < hi) {
        uint32_t mid = lo + (hi - lo) / 2;
        const uint32_t *t = order_tokens(o, mid);
        int cmp = 0;
        for (uint32_t i = 0; i < plen; i++) {
            if (t[i] < prefix[i]) { cmp = -1; break; }
            if (t[i] > prefix[i]) { cmp =  1; break; }
        }
        if (cmp < 0) lo = mid + 1; else hi = mid;
    }
    return lo;
}

/* Find total count + list of continuations for prefix in order `o` (n-grams).
 * Prefix length must be o->n - 1. Returns number of continuations. Writes them
 * into out_next[] and out_count[] up to out_cap entries (may be truncated). */
static uint32_t order_continuations(const ngram_order_t *o,
                                     const uint32_t *prefix,
                                     uint32_t plen,
                                     uint32_t *out_next,
                                     uint64_t *out_count,
                                     uint32_t out_cap,
                                     uint64_t *out_total)
{
    if (o->n != plen + 1 || o->count == 0) { *out_total = 0; return 0; }
    uint32_t idx = order_lower_bound(o, prefix, plen);
    uint64_t total = 0;
    uint32_t num = 0;
    while (idx < o->count) {
        const uint32_t *t = order_tokens(o, idx);
        bool match = true;
        for (uint32_t i = 0; i < plen; i++) {
            if (t[i] != prefix[i]) { match = false; break; }
        }
        if (!match) break;
        uint64_t c = order_count(o, idx);
        total += c;
        if (num < out_cap) {
            out_next[num] = t[plen];
            out_count[num] = c;
            num++;
        }
        idx++;
    }
    *out_total = total;
    return num;
}

/* ===== Sampling (stupid backoff) ===== */
/* Find longest matching context that has at least one continuation, then
 * sample weighted by counts (with temperature applied). Returns the token. */
static uint32_t sample_next(const model_t *m,
                            const uint32_t *history, uint32_t hlen,
                            double temperature,
                            /* scratch buffers */
                            uint32_t *cand_next, uint64_t *cand_count,
                            uint32_t cand_cap)
{
    for (uint32_t n = m->max_n; n >= 2; n--) {
        if (n - 1 > hlen) continue;
        const uint32_t *ctx = history + (hlen - (n - 1));
        uint64_t total;
        uint32_t num = order_continuations(&m->orders[n - 1],
                                            ctx, n - 1,
                                            cand_next, cand_count,
                                            cand_cap, &total);
        if (num > 0) {
            /* Sample with temperature */
            if (temperature <= 0.0) temperature = 1e-3;
            double *w = malloc(num * sizeof(double));
            double sum = 0;
            for (uint32_t i = 0; i < num; i++) {
                double p = (double)cand_count[i] / (double)total;
                double lp = log(p) / temperature;
                w[i] = exp(lp);
                sum += w[i];
            }
            double r = rng_uniform() * sum;
            uint32_t pick = num - 1;
            for (uint32_t i = 0; i < num; i++) {
                r -= w[i];
                if (r <= 0) { pick = i; break; }
            }
            uint32_t tok = cand_next[pick];
            free(w);
            return tok;
        }
    }
    /* Fall back to unigram */
    const ngram_order_t *o1 = &m->orders[0];
    if (o1->count == 0) return 0;
    /* Sample weighted by unigram counts */
    uint64_t total = m->total_tokens;
    uint64_t r = rng_next() % (total ? total : 1);
    for (uint32_t i = 0; i < o1->count; i++) {
        uint64_t c = order_count(o1, i);
        if (r < c) return *order_tokens(o1, i);
        r -= c;
    }
    return *order_tokens(o1, 0);
}

/* ===== Probability for perplexity (stupid backoff log-prob) ===== */
static double logprob_next(const model_t *m,
                           const uint32_t *history, uint32_t hlen,
                           uint32_t next)
{
    uint32_t scratch_next[1];
    uint64_t scratch_cnt[1];
    (void)scratch_next; (void)scratch_cnt;
    /* Try longest matching context with non-empty continuation set */
    double backoff = 1.0;
    for (uint32_t n = m->max_n; n >= 2; n--) {
        if (n - 1 > hlen) { backoff *= BACKOFF_ALPHA; continue; }
        const uint32_t *ctx = history + (hlen - (n - 1));
        /* Look up exact ngram (ctx + next) */
        uint32_t key[6];
        for (uint32_t i = 0; i < n - 1; i++) key[i] = ctx[i];
        key[n - 1] = next;
        uint32_t idx = order_lower_bound(&m->orders[n - 1], key, n);
        bool exact_match = false;
        if (idx < m->orders[n - 1].count) {
            const uint32_t *t = order_tokens(&m->orders[n - 1], idx);
            bool eq = true;
            for (uint32_t i = 0; i < n; i++) if (t[i] != key[i]) { eq = false; break; }
            exact_match = eq;
        }
        if (exact_match) {
            uint64_t c_full = order_count(&m->orders[n - 1], idx);
            /* Total count of context */
            uint64_t total;
            uint32_t dummy_next; uint64_t dummy_cnt;
            (void)order_continuations(&m->orders[n - 1], ctx, n - 1,
                                       &dummy_next, &dummy_cnt, 0, &total);
            if (total > 0) {
                double p = (double)c_full / (double)total;
                return log(p * backoff);
            }
        }
        backoff *= BACKOFF_ALPHA;
    }
    /* Unigram fallback */
    const ngram_order_t *o1 = &m->orders[0];
    uint32_t idx = order_lower_bound(o1, &next, 1);
    if (idx < o1->count) {
        const uint32_t *t = order_tokens(o1, idx);
        if (t[0] == next) {
            uint64_t c = order_count(o1, idx);
            double p = (double)c / (double)m->total_tokens;
            return log(p * backoff);
        }
    }
    /* Unseen unigram: very small probability */
    return log(backoff / ((double)m->vocab_size * (double)m->total_tokens));
}

/* ===== Commands ===== */
static double g_temperature = 0.8;

static void cmd_help(void) {
    fprintf(stdout,
        "commands:\n"
        "  <text>               generate continuation (60 tokens)\n"
        "  /sample N <text>     generate N tokens after <text>\n"
        "  /next <text>         top-10 next-token candidates\n"
        "  /perplexity <text>   per-token perplexity of <text>\n"
        "  /tokens <text>       show how <text> tokenizes\n"
        "  /temp X              set sampling temperature (now %.2f)\n"
        "  /seed X              reseed the RNG\n"
        "  /help                this list\n"
        "  /quit | /q           exit\n",
        g_temperature);
}

static void generate(const model_t *m, const bpe_tokenizer_t *tk,
                     const char *prompt, uint32_t n_tokens)
{
    uint32_t history[MAX_HISTORY];
    uint32_t hlen = tokenize(tk, (const uint8_t *)prompt,
                              strlen(prompt), history, MAX_HISTORY);
    detok_print(tk, history, hlen, stdout);

    uint32_t cand_next[16384];
    uint64_t cand_count[16384];
    for (uint32_t i = 0; i < n_tokens && hlen < MAX_HISTORY; i++) {
        uint32_t tok = sample_next(m, history, hlen, g_temperature,
                                    cand_next, cand_count, 16384);
        history[hlen++] = tok;
        detok_print(tk, &tok, 1, stdout);
    }
    fputc('\n', stdout);
}

static void cmd_next(const model_t *m, const bpe_tokenizer_t *tk,
                     const char *prompt)
{
    uint32_t history[MAX_HISTORY];
    uint32_t hlen = tokenize(tk, (const uint8_t *)prompt,
                              strlen(prompt), history, MAX_HISTORY);
    uint32_t cand_next[16384];
    uint64_t cand_count[16384];
    uint32_t used_n = 0;
    uint32_t num = 0;
    uint64_t total = 0;
    for (uint32_t n = m->max_n; n >= 2; n--) {
        if (n - 1 > hlen) continue;
        const uint32_t *ctx = history + (hlen - (n - 1));
        num = order_continuations(&m->orders[n - 1], ctx, n - 1,
                                   cand_next, cand_count, 16384, &total);
        if (num > 0) { used_n = n; break; }
    }
    if (num == 0) {
        fprintf(stdout, "no n-gram match for context.\n");
        return;
    }
    /* Sort by count desc, show top 10 */
    /* Use simple insertion sort top-K to avoid full sort */
    uint32_t topk = num < 10 ? num : 10;
    for (uint32_t k = 0; k < topk; k++) {
        uint32_t best = k;
        for (uint32_t j = k + 1; j < num; j++) {
            if (cand_count[j] > cand_count[best]) best = j;
        }
        if (best != k) {
            uint64_t tc = cand_count[best]; cand_count[best] = cand_count[k]; cand_count[k] = tc;
            uint32_t tn = cand_next[best]; cand_next[best] = cand_next[k]; cand_next[k] = tn;
        }
    }
    fprintf(stdout, "context order %u, %u candidates, total %llu:\n",
            used_n, num, (unsigned long long)total);
    for (uint32_t i = 0; i < topk; i++) {
        double p = (double)cand_count[i] / (double)total;
        fprintf(stdout, "  %6.3f  (%llu)  '", p,
                (unsigned long long)cand_count[i]);
        detok_print(tk, &cand_next[i], 1, stdout);
        fprintf(stdout, "'\n");
    }
}

static void cmd_perplexity(const model_t *m, const bpe_tokenizer_t *tk,
                            const char *text)
{
    uint32_t history[MAX_HISTORY];
    uint32_t hlen = tokenize(tk, (const uint8_t *)text,
                              strlen(text), history, MAX_HISTORY);
    if (hlen < 2) {
        fprintf(stdout, "need at least 2 tokens\n");
        return;
    }
    double sum_logp = 0;
    uint32_t n_scored = 0;
    for (uint32_t i = 1; i < hlen; i++) {
        double lp = logprob_next(m, history, i, history[i]);
        sum_logp += lp;
        n_scored++;
    }
    double avg = sum_logp / (double)n_scored;
    double ppl = exp(-avg);
    fprintf(stdout, "tokens=%u  avg_logprob=%.4f  perplexity=%.2f\n",
            hlen, avg, ppl);
}

static void cmd_tokens(const bpe_tokenizer_t *tk, const char *text) {
    uint32_t history[MAX_HISTORY];
    uint32_t hlen = tokenize(tk, (const uint8_t *)text,
                              strlen(text), history, MAX_HISTORY);
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

/* ===== Main loop ===== */
static char *trim(char *s) {
    while (*s && isspace((unsigned char)*s)) s++;
    char *end = s + strlen(s);
    while (end > s && isspace((unsigned char)end[-1])) *--end = 0;
    return s;
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "Usage: %s <tokenizer.bin> <model.bin>\n", argv[0]);
        return 1;
    }
    bpe_tokenizer_t tk;
    model_t m;
    bpe_load(&tk, argv[1]);
    model_load(&m, argv[2]);
    fprintf(stdout,
        "loaded tokenizer: %u tokens, %u merges\n"
        "loaded model:     %u-gram over %llu corpus tokens, vocab %u\n",
        tk.vocab_size, tk.num_merges,
        m.max_n, (unsigned long long)m.total_tokens, m.vocab_size);
    for (uint32_t n = 1; n <= m.max_n; n++) {
        fprintf(stdout, "  %u-grams: %u\n", n, m.orders[n - 1].count);
    }
    fprintf(stdout, "type something, or /help for commands. /quit to exit.\n\n");

    rng_seed((uint64_t)time(NULL));

    char buf[8192];
    for (;;) {
        fprintf(stdout, "> "); fflush(stdout);
        if (!fgets(buf, sizeof(buf), stdin)) break;
        char *line = trim(buf);
        if (!*line) continue;

        if (line[0] == '/') {
            if (strcmp(line, "/quit") == 0 || strcmp(line, "/q") == 0) break;
            if (strcmp(line, "/help") == 0) { cmd_help(); continue; }
            if (strncmp(line, "/temp ", 6) == 0) {
                g_temperature = atof(line + 6);
                if (g_temperature <= 0) g_temperature = 0.01;
                fprintf(stdout, "temperature = %.3f\n", g_temperature);
                continue;
            }
            if (strncmp(line, "/seed ", 6) == 0) {
                rng_seed((uint64_t)atoll(line + 6));
                fprintf(stdout, "reseeded\n");
                continue;
            }
            if (strncmp(line, "/sample ", 8) == 0) {
                char *p = line + 8;
                uint32_t n = (uint32_t)strtoul(p, &p, 10);
                while (*p == ' ') p++;
                if (n == 0) n = DEFAULT_SAMPLE_N;
                generate(&m, &tk, p, n);
                continue;
            }
            if (strncmp(line, "/next ", 6) == 0) { cmd_next(&m, &tk, line + 6); continue; }
            if (strncmp(line, "/perplexity ", 12) == 0) { cmd_perplexity(&m, &tk, line + 12); continue; }
            if (strncmp(line, "/tokens ", 8) == 0) { cmd_tokens(&tk, line + 8); continue; }
            fprintf(stdout, "unknown command. /help for list.\n");
            continue;
        }
        generate(&m, &tk, line, DEFAULT_SAMPLE_N);
    }
    return 0;
}
