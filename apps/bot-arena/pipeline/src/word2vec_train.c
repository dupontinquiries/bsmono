/*
 * word2vec_train.c — Skip-gram Word2Vec with negative sampling on BPE tokens
 *
 * Loads tokenizer.bin, walks corpus, tokenizes via BPE, trains word vectors.
 *
 * Build:
 *   gcc -std=c99 -O3 -Wall -Wextra -o word2vec_train word2vec_train.c -lz -lm
 *
 * Usage:
 *   ./word2vec_train <corpus_dir> <tokenizer.bin> <vectors.bin> [dim] [epochs] [window] [k_negative]
 *     dim:        embedding dimension (default 128)
 *     epochs:     training epochs (default 5)
 *     window:     skip-gram window size (default 5)
 *     k_negative: negative samples per positive (default 5)
 *
 * Output format (vectors.bin):
 *   Header (32 bytes):
 *     char[8]  magic = "WORDV01\0"
 *     uint32   version = 1
 *     uint32   vocab_size
 *     uint32   dim
 *     uint64   total_training_tokens
 *     uint64   payload_compressed_size  (uncompressed = vocab_size * dim * 4)
 *   Payload (zlib level 9):
 *     vocab_size * dim float32 values, row-major (word i's vector at offset i*dim)
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

#define WORDV_MAGIC          "WORDV01"
#define WORDV_MAGIC_LEN      8
#define WORDV_VERSION        1u

#define BPE_MAGIC            "BPETK01"
#define BPE_MAGIC_LEN        8
#define BPE_BASE_VOCAB       256u

#define DEFAULT_DIM          128
#define DEFAULT_EPOCHS       5
#define DEFAULT_WINDOW       5
#define DEFAULT_K_NEGATIVE   5
#define NEG_TABLE_SIZE       (1 << 23)   /* ~8M entries for unigram sampling */
#define INITIAL_TOKEN_CAP    (1u << 20)
#define SIGMOID_TABLE_SIZE   1024
#define SIGMOID_MAX          6.0
#define SUBSAMPLE_T          1e-5
#define INITIAL_LR           0.025

/* FNV-1a */
static uint64_t fnv1a(const void *data, size_t len) {
    const uint8_t *p = (const uint8_t *)data;
    uint64_t h = 0xcbf29ce484222325ULL;
    for (size_t i = 0; i < len; i++) { h ^= p[i]; h *= 0x100000001b3ULL; }
    return h;
}

/* PRNG */
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

/* ===== BPE tokenizer (same loader) ===== */
typedef struct { uint64_t key; uint32_t rank; uint32_t new_id; } bpe_merge_entry_t;
typedef struct {
    uint32_t vocab_size, num_merges;
    bpe_merge_entry_t *table; uint32_t table_cap;
} bpe_tokenizer_t;

static void bpe_load(bpe_tokenizer_t *tk, const char *path) {
    FILE *f = fopen(path, "rb"); if (!f) { perror("tokenizer"); exit(1); }
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
    uint32_t *p32 = (uint32_t *)ubuf;
    uint32_t mask = cap - 1;
    for (uint32_t i = 0; i < num_merges; i++) {
        uint32_t a = p32[i*2], b = p32[i*2+1], new_id = BPE_BASE_VOCAB + i;
        uint64_t key = ((uint64_t)a << 32) | b;
        uint64_t hash = fnv1a(&key, 8);
        uint32_t idx = (uint32_t)(hash & mask);
        while (tk->table[idx].new_id != 0) idx = (idx + 1) & mask;
        tk->table[idx].key = key; tk->table[idx].rank = i; tk->table[idx].new_id = new_id;
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
        uint32_t bp = UINT32_MAX, br = UINT32_MAX, bn = 0;
        for (uint32_t i = 0; i + 1 < len; i++) {
            uint32_t r, nid;
            if (bpe_lookup(tk, tokens[i], tokens[i+1], &r, &nid)) {
                if (r < br) { br = r; bp = i; bn = nid; }
            }
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

typedef struct { uint32_t *ids; size_t size, cap; } tokens_t;
static void tokens_push_n(tokens_t *t, const uint32_t *src, size_t n) {
    if (t->size + n > t->cap) {
        while (t->size + n > t->cap) t->cap = t->cap ? t->cap * 2 : INITIAL_TOKEN_CAP;
        t->ids = realloc(t->ids, t->cap * sizeof(uint32_t));
        if (!t->ids) { perror("realloc"); exit(1); }
    }
    memcpy(t->ids + t->size, src, n * sizeof(uint32_t));
    t->size += n;
}

static void tokenize_bytes(const bpe_tokenizer_t *tk, const uint8_t *text, size_t len, tokens_t *t) {
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
            tokens_push_n(t, chunk, nlen);
            start += take; clen -= take;
        }
    }
}

/* ===== File walker ===== */
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
    int fd = open(path, O_RDONLY); if (fd < 0) return;
    struct stat st;
    if (fstat(fd,&st)<0 || st.st_size==0) { close(fd); return; }
    void *p = mmap(NULL, st.st_size, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED) { close(fd); return; }
    tokenize_bytes(tk, (const uint8_t *)p, st.st_size, t);
    munmap(p, st.st_size); close(fd);
}

static void walk_dir(const char *dir, const bpe_tokenizer_t *tk, tokens_t *t, uint64_t *files) {
    DIR *d = opendir(dir); if (!d) return;
    struct dirent *e; char path[4096];
    while ((e = readdir(d))) {
        if (e->d_name[0] == '.') continue;
        int n = snprintf(path,sizeof(path),"%s/%s",dir,e->d_name);
        if (n < 0 || (size_t)n >= sizeof(path)) continue;
        struct stat st; if (stat(path,&st)<0) continue;
        if (S_ISDIR(st.st_mode)) walk_dir(path,tk,t,files);
        else if (S_ISREG(st.st_mode) && has_corpus_ext(e->d_name)) {
            process_file(path,tk,t); (*files)++;
        }
    }
    closedir(d);
}

/* ===== Sigmoid lookup table ===== */
static float sigmoid_table[SIGMOID_TABLE_SIZE];
static void init_sigmoid_table(void) {
    for (int i = 0; i < SIGMOID_TABLE_SIZE; i++) {
        double x = ((double)i / SIGMOID_TABLE_SIZE * 2.0 - 1.0) * SIGMOID_MAX;
        sigmoid_table[i] = (float)(1.0 / (1.0 + exp(-x)));
    }
}
static inline float fast_sigmoid(float x) {
    if (x >= SIGMOID_MAX)  return 1.0f;
    if (x <= -SIGMOID_MAX) return 0.0f;
    int idx = (int)((x + SIGMOID_MAX) / (2.0 * SIGMOID_MAX) * SIGMOID_TABLE_SIZE);
    if (idx < 0) idx = 0;
    if (idx >= SIGMOID_TABLE_SIZE) idx = SIGMOID_TABLE_SIZE - 1;
    return sigmoid_table[idx];
}

/* ===== Training ===== */
static double secs_since(clock_t t0) { return (double)(clock() - t0) / (double)CLOCKS_PER_SEC; }

static void train(const tokens_t *toks, uint32_t vocab_size, uint32_t dim,
                  uint32_t epochs, uint32_t window, uint32_t K,
                  float *V, float *U)
{
    /* Token frequencies */
    uint64_t *freq = calloc(vocab_size, sizeof(uint64_t));
    for (size_t i = 0; i < toks->size; i++) freq[toks->ids[i]]++;
    uint64_t total_tokens = toks->size;

    /* Negative sampling table (Mikolov's 3/4 power) */
    fprintf(stderr, "Building negative sampling table...\n");
    double Z = 0;
    for (uint32_t i = 0; i < vocab_size; i++) {
        if (freq[i] > 0) Z += pow((double)freq[i], 0.75);
    }
    uint32_t *neg_table = malloc(NEG_TABLE_SIZE * sizeof(uint32_t));
    {
        uint32_t i_word = 0;
        double cum = 0;
        if (Z > 0) cum = pow((double)freq[0], 0.75) / Z;
        for (uint32_t i = 0; i < NEG_TABLE_SIZE; i++) {
            double r = (double)i / (double)NEG_TABLE_SIZE;
            while (cum < r && i_word + 1 < vocab_size) {
                i_word++;
                cum += pow((double)freq[i_word], 0.75) / Z;
            }
            neg_table[i] = i_word;
        }
    }

    /* Initialize V to small random, U to zero */
    fprintf(stderr, "Initializing vectors...\n");
    for (uint32_t i = 0; i < vocab_size; i++) {
        for (uint32_t d = 0; d < dim; d++) {
            V[i * dim + d] = (float)((rng_uniform() - 0.5) / (double)dim);
        }
    }
    memset(U, 0, (size_t)vocab_size * dim * sizeof(float));

    /* Subsample probability per word */
    double *subs_keep = malloc(vocab_size * sizeof(double));
    for (uint32_t i = 0; i < vocab_size; i++) {
        if (freq[i] == 0) { subs_keep[i] = 1.0; continue; }
        double f = (double)freq[i] / (double)total_tokens;
        double p = (sqrt(f / SUBSAMPLE_T) + 1.0) * SUBSAMPLE_T / f;
        if (p > 1.0) p = 1.0;
        subs_keep[i] = p;
    }

    /* Training loop */
    fprintf(stderr, "Training: %u epochs, dim=%u, window=%u, K_neg=%u\n",
            epochs, dim, window, K);

    uint64_t total_pairs = (uint64_t)total_tokens * (uint64_t)epochs;
    uint64_t pairs_done = 0;
    clock_t t0 = clock();
    clock_t last_log = clock();

    float *grad_V = malloc(dim * sizeof(float));

    for (uint32_t epoch = 0; epoch < epochs; epoch++) {
        double lr_base = INITIAL_LR;
        for (size_t i = 0; i < toks->size; i++) {
            /* Linear LR decay */
            double progress = (double)pairs_done / (double)total_pairs;
            double lr = lr_base * (1.0 - progress);
            if (lr < lr_base * 0.0001) lr = lr_base * 0.0001;

            uint32_t center = toks->ids[i];

            /* Subsample */
            if (subs_keep[center] < 1.0 && rng_uniform() > subs_keep[center]) {
                pairs_done++;
                continue;
            }

            /* Random window size [1..window] */
            uint32_t b = (uint32_t)(rng_next() % window) + 1;

            for (int c = -(int)b; c <= (int)b; c++) {
                if (c == 0) continue;
                long pos = (long)i + c;
                if (pos < 0 || (size_t)pos >= toks->size) continue;
                uint32_t context = toks->ids[(size_t)pos];
                if (subs_keep[context] < 1.0 && rng_uniform() > subs_keep[context]) continue;

                /* Positive sample: center -> context */
                /* Compute gradient for V[center] */
                memset(grad_V, 0, dim * sizeof(float));

                /* Positive */
                float *u_ctx = U + (size_t)context * dim;
                float dot = 0;
                for (uint32_t d = 0; d < dim; d++) dot += V[center*dim+d] * u_ctx[d];
                float g = (float)(lr * (1.0f - fast_sigmoid(dot)));
                for (uint32_t d = 0; d < dim; d++) grad_V[d] += g * u_ctx[d];
                for (uint32_t d = 0; d < dim; d++) u_ctx[d] += g * V[center*dim+d];

                /* Negatives */
                for (uint32_t k = 0; k < K; k++) {
                    uint32_t neg = neg_table[rng_next() & (NEG_TABLE_SIZE - 1)];
                    if (neg == context) continue;
                    float *u_neg = U + (size_t)neg * dim;
                    dot = 0;
                    for (uint32_t d = 0; d < dim; d++) dot += V[center*dim+d] * u_neg[d];
                    g = (float)(lr * (0.0f - fast_sigmoid(dot)));
                    for (uint32_t d = 0; d < dim; d++) grad_V[d] += g * u_neg[d];
                    for (uint32_t d = 0; d < dim; d++) u_neg[d] += g * V[center*dim+d];
                }

                /* Apply V gradient */
                for (uint32_t d = 0; d < dim; d++) V[center*dim+d] += grad_V[d];
            }
            pairs_done++;
            if ((pairs_done & ((1u<<16) - 1)) == 0) {
                clock_t now = clock();
                if ((double)(now - last_log) / CLOCKS_PER_SEC > 1.0) {
                    fprintf(stderr,
                        "  ep %u/%u  %.1f%%  lr=%.5f  %.1fs\r",
                        epoch + 1, epochs,
                        100.0 * progress, lr, secs_since(t0));
                    fflush(stderr);
                    last_log = now;
                }
            }
        }
        fprintf(stderr, "  ep %u/%u done, %.1fs\n", epoch + 1, epochs, secs_since(t0));
    }

    free(grad_V);
    free(neg_table);
    free(subs_keep);
    free(freq);
}

static void serialize(const char *path, uint32_t vocab_size, uint32_t dim,
                       uint64_t total_tokens, const float *V) {
    size_t payload_size = (size_t)vocab_size * dim * sizeof(float);
    uLongf cap = compressBound((uLong)payload_size);
    uint8_t *comp = malloc(cap);
    uLongf csize = cap;
    if (compress2(comp, &csize, (const Bytef *)V, (uLong)payload_size, 9) != Z_OK)
        { fprintf(stderr,"compress\n"); exit(1); }

    FILE *f = fopen(path, "wb"); if (!f) { perror("output"); exit(1); }
    fwrite(WORDV_MAGIC, 1, WORDV_MAGIC_LEN, f);
    uint32_t v = WORDV_VERSION, vs = vocab_size, d = dim;
    uint64_t tt = total_tokens, cs = csize;
    fwrite(&v,4,1,f); fwrite(&vs,4,1,f); fwrite(&d,4,1,f);
    fwrite(&tt,8,1,f); fwrite(&cs,8,1,f);
    fwrite(comp,1,csize,f);
    fclose(f);
    fprintf(stderr, "Wrote %s: %.2f MB compressed (%.2f MB raw)\n",
            path, csize/(1024.0*1024.0), payload_size/(1024.0*1024.0));
    free(comp);
}

int main(int argc, char **argv) {
    if (argc < 4) {
        fprintf(stderr,
            "Usage: %s <corpus_dir> <tokenizer.bin> <vectors.bin> [dim] [epochs] [window] [k_neg]\n"
            "  dim=%u epochs=%u window=%u k_neg=%u (defaults)\n",
            argv[0], DEFAULT_DIM, DEFAULT_EPOCHS, DEFAULT_WINDOW, DEFAULT_K_NEGATIVE);
        return 1;
    }
    const char *corpus_dir = argv[1];
    const char *tk_path = argv[2];
    const char *out_path = argv[3];
    uint32_t dim    = (argc >= 5) ? (uint32_t)atoi(argv[4]) : DEFAULT_DIM;
    uint32_t epochs = (argc >= 6) ? (uint32_t)atoi(argv[5]) : DEFAULT_EPOCHS;
    uint32_t window = (argc >= 7) ? (uint32_t)atoi(argv[6]) : DEFAULT_WINDOW;
    uint32_t K_neg  = (argc >= 8) ? (uint32_t)atoi(argv[7]) : DEFAULT_K_NEGATIVE;

    fprintf(stderr, "Corpus:    %s\nTokenizer: %s\nOutput:    %s\n", corpus_dir, tk_path, out_path);
    fprintf(stderr, "dim=%u epochs=%u window=%u K_neg=%u\n\n", dim, epochs, window, K_neg);

    init_sigmoid_table();
    rng_seed((uint64_t)time(NULL));

    fprintf(stderr, "Loading tokenizer...\n");
    bpe_tokenizer_t tk;
    bpe_load(&tk, tk_path);
    fprintf(stderr, "  vocab=%u merges=%u\n", tk.vocab_size, tk.num_merges);

    fprintf(stderr, "Walking corpus and tokenizing...\n");
    tokens_t toks = {0};
    uint64_t files = 0;
    clock_t t0 = clock();
    walk_dir(corpus_dir, &tk, &toks, &files);
    fprintf(stderr, "  %llu files, %zu tokens, %.2fs\n",
            (unsigned long long)files, toks.size, secs_since(t0));
    if (toks.size < 100) {
        fprintf(stderr, "Too few tokens for training (need >= 100)\n");
        return 1;
    }

    /* Allocate vectors */
    size_t vec_size = (size_t)tk.vocab_size * dim;
    float *V = malloc(vec_size * sizeof(float));
    float *U = malloc(vec_size * sizeof(float));
    if (!V || !U) { perror("vectors"); return 1; }
    fprintf(stderr, "Allocated %.1f MB for vectors\n",
            (double)(vec_size * 2 * sizeof(float)) / (1024.0 * 1024.0));

    train(&toks, tk.vocab_size, dim, epochs, window, K_neg, V, U);

    fprintf(stderr, "Saving vectors...\n");
    serialize(out_path, tk.vocab_size, dim, (uint64_t)toks.size, V);

    fprintf(stderr, "\nTotal: %.2fs\n", secs_since(t0));

    free(V); free(U); free(toks.ids); free(tk.table);
    return 0;
}
