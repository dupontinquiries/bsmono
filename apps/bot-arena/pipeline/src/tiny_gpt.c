/*
 * tiny_gpt.c — a tiny but real GPT in pure C99
 *
 * A working decoder-only transformer with full backpropagation and Adam.
 * Reuses your existing tokenizer.bin (BPETK01). Trains on a corpus directory,
 * saves a checkpoint, and generates text. No dependencies beyond zlib + libm.
 *
 * This is the bridge from "referencing" (n-gram) to "thinking" (attention can
 * track long-range dependencies an n-gram cannot). It is small enough to train
 * on a CPU in minutes for a modest corpus, large enough to show the qualitative
 * difference.
 *
 * Build:
 *   gcc -std=c99 -O3 -Wall -Wextra -o tiny_gpt tiny_gpt.c -lz -lm
 *
 * Train:
 *   ./tiny_gpt train <corpus_dir> <tokenizer.bin> <checkpoint.bin> \
 *       [steps] [dim] [layers] [heads] [context] [batch] [lr]
 *   defaults: steps=2000 dim=128 layers=4 heads=4 context=128 batch=16 lr=3e-4
 *
 * Generate:
 *   ./tiny_gpt generate <tokenizer.bin> <checkpoint.bin> <prompt> [n_tokens] [temperature]
 *
 * Corpus file extensions are controlled by the CORPUS_EXT environment variable
 * (comma-separated, e.g. CORPUS_EXT=.md,.c,.py). Defaults to ".md".
 *
 * Checkpoint format (checkpoint.bin):
 *   char[8]  magic = "TGPT01\0\0"
 *   int32    dim, n_layers, n_heads, context_len, vocab_size
 *   then all parameters as float32 (uncompressed; the model is small)
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

/* Optional OpenMP for batch-parallel training. When compiled without -fopenmp the
 * stubs make it run identically (single-threaded). */
#ifdef _OPENMP
#  include <omp.h>
#else
static inline int omp_get_max_threads(void) { return 1; }
static inline int omp_get_thread_num(void) { return 0; }
#endif

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

#define BPE_MAGIC        "BPETK01"
#define BPE_MAGIC_LEN    8
#define BPE_BASE_VOCAB   256u
#define TGPT_MAGIC       "TGPT01"
#define TGPT_MAGIC_LEN   8

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
/* standard normal via Box-Muller */
static double rng_normal(void) {
    double u1 = rng_uniform(); if (u1 < 1e-12) u1 = 1e-12;
    double u2 = rng_uniform();
    return sqrt(-2.0 * log(u1)) * cos(2.0 * M_PI * u2);
}

/* ===== BPE tokenizer (load + encode + decode) ===== */
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
        tk->token_bytes[i] = malloc(1); tk->token_bytes[i][0] = (uint8_t)i;
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
        tk->token_bytes[new_id] = bytes; tk->token_byte_lens[new_id] = ln;
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
            if (bpe_lookup(tk, tokens[i], tokens[i+1], &r, &nid)) if (r < br) { br = r; bp = i; bn = nid; }
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
        uint32_t id = tokens[i]; if (id >= tk->vocab_size) continue;
        fwrite(tk->token_bytes[id], 1, tk->token_byte_lens[id], out);
    }
    fflush(out);
}

/* ===== Corpus walking with configurable extensions ===== */
static char g_exts[1024] = ".md";
static void load_exts_from_env(void) {
    const char *e = getenv("CORPUS_EXT");
    if (e && *e) { strncpy(g_exts, e, sizeof(g_exts) - 1); g_exts[sizeof(g_exts)-1] = 0; }
}
static bool name_has_wanted_ext(const char *name) {
    size_t nlen = strlen(name);
    char buf[1024]; strncpy(buf, g_exts, sizeof(buf) - 1); buf[sizeof(buf)-1] = 0;
    char *save = NULL;
    for (char *tok = strtok_r(buf, ",", &save); tok; tok = strtok_r(NULL, ",", &save)) {
        while (*tok == ' ') tok++;
        size_t elen = strlen(tok);
        while (elen > 0 && (tok[elen-1] == ' ')) elen--;
        if (elen > 0 && nlen >= elen && memcmp(name + nlen - elen, tok, elen) == 0) return true;
    }
    return false;
}

typedef struct { uint32_t *ids; size_t size, cap; } tokens_t;
static void tokens_push_n(tokens_t *t, const uint32_t *src, size_t n) {
    if (t->size + n > t->cap) {
        while (t->size + n > t->cap) t->cap = t->cap ? t->cap * 2 : (1u << 20);
        t->ids = realloc(t->ids, t->cap * sizeof(uint32_t));
        if (!t->ids) { perror("realloc"); exit(1); }
    }
    memcpy(t->ids + t->size, src, n * sizeof(uint32_t));
    t->size += n;
}
static void process_file(const char *path, const bpe_tokenizer_t *tk, tokens_t *t) {
    int fd = open(path, O_RDONLY); if (fd < 0) return;
    struct stat st; if (fstat(fd,&st)<0 || st.st_size==0) { close(fd); return; }
    void *p = mmap(NULL, st.st_size, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED) { close(fd); return; }
    uint32_t buf[8192]; size_t off = 0; const uint8_t *text = p; size_t len = st.st_size;
    /* tokenize in blocks to bound chunk size */
    while (off < len) {
        size_t take = len - off; if (take > 65536) take = 65536;
        uint32_t n = tokenize(tk, text + off, take, buf, 8192);
        tokens_push_n(t, buf, n);
        off += take;
    }
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
        else if (S_ISREG(st.st_mode) && name_has_wanted_ext(e->d_name)) { process_file(path,tk,t); (*files)++; }
    }
    closedir(d);
}

/* ===== GPT model ===== */
typedef struct {
    int dim, n_layers, n_heads, context_len, vocab_size;
} gpt_config_t;

/* Parameter block: one flat array + named offsets. Output head tied to tok_emb. */
typedef struct {
    int n;                 /* total floats */
    float *mem;            /* flat parameters */
    /* offsets (in floats) */
    int o_tok_emb;         /* vocab*dim */
    int o_pos_emb;         /* context*dim */
    int o_ln1_g, o_ln1_b;  /* layers*dim each */
    int o_qkv_w, o_qkv_b;  /* layers*dim*3dim , layers*3dim */
    int o_proj_w, o_proj_b;/* layers*dim*dim , layers*dim */
    int o_ln2_g, o_ln2_b;  /* layers*dim each */
    int o_fc_w, o_fc_b;    /* layers*dim*4dim , layers*4dim */
    int o_fcp_w, o_fcp_b;  /* layers*4dim*dim , layers*dim */
    int o_lnf_g, o_lnf_b;  /* dim each */
} params_t;

static int params_layout(params_t *P, const gpt_config_t *c) {
    int D = c->dim, L = c->n_layers, V = c->vocab_size, T = c->context_len;
    int off = 0;
    P->o_tok_emb = off; off += V * D;
    P->o_pos_emb = off; off += T * D;
    P->o_ln1_g = off; off += L * D;
    P->o_ln1_b = off; off += L * D;
    P->o_qkv_w = off; off += L * D * (3*D);
    P->o_qkv_b = off; off += L * (3*D);
    P->o_proj_w = off; off += L * D * D;
    P->o_proj_b = off; off += L * D;
    P->o_ln2_g = off; off += L * D;
    P->o_ln2_b = off; off += L * D;
    P->o_fc_w = off; off += L * D * (4*D);
    P->o_fc_b = off; off += L * (4*D);
    P->o_fcp_w = off; off += L * (4*D) * D;
    P->o_fcp_b = off; off += L * D;
    P->o_lnf_g = off; off += D;
    P->o_lnf_b = off; off += D;
    P->n = off;
    return off;
}
static void params_alloc(params_t *P, const gpt_config_t *c) {
    params_layout(P, c);
    P->mem = calloc(P->n, sizeof(float));
}
static void params_init(params_t *P, const gpt_config_t *c) {
    int D = c->dim, L = c->n_layers, V = c->vocab_size, T = c->context_len;
    float *m = P->mem;
    for (int i = 0; i < V*D; i++) m[P->o_tok_emb + i] = (float)(rng_normal() * 0.02);
    for (int i = 0; i < T*D; i++) m[P->o_pos_emb + i] = (float)(rng_normal() * 0.02);
    for (int i = 0; i < L*D; i++) { m[P->o_ln1_g+i]=1; m[P->o_ln2_g+i]=1; }
    /* ln biases already zero; lnf */
    for (int i = 0; i < D; i++) m[P->o_lnf_g+i] = 1;
    double resid_scale = 0.02 / sqrt(2.0 * (double)L);
    for (int i = 0; i < L*D*(3*D); i++) m[P->o_qkv_w+i] = (float)(rng_normal() * 0.02);
    for (int i = 0; i < L*D*D;      i++) m[P->o_proj_w+i] = (float)(rng_normal() * resid_scale);
    for (int i = 0; i < L*D*(4*D);  i++) m[P->o_fc_w+i]  = (float)(rng_normal() * 0.02);
    for (int i = 0; i < L*(4*D)*D;  i++) m[P->o_fcp_w+i] = (float)(rng_normal() * resid_scale);
    /* all biases zero from calloc */
}

/* Activation caches for a single sequence of length T */
typedef struct {
    int T, D, L, H, V;
    float *emb;       /* T*D */
    float *ln1, *ln1_mean, *ln1_rstd;   /* L*T*D, L*T, L*T */
    float *qkv;       /* L*T*3D */
    float *att;       /* L*H*T*T (softmax weights) */
    float *attn_out;  /* L*T*D */
    float *res1;      /* L*T*D (after attn residual) */
    float *ln2, *ln2_mean, *ln2_rstd;   /* L*T*D, L*T, L*T */
    float *fc;        /* L*T*4D (pre-gelu) */
    float *fcg;       /* L*T*4D (post-gelu) */
    float *res2;      /* L*T*D (after mlp residual) = next layer input */
    float *lnf, *lnf_mean, *lnf_rstd;   /* T*D, T, T */
    float *logits;    /* T*V */
    float *probs;     /* T*V */
} acts_t;

static void acts_alloc(acts_t *a, const gpt_config_t *c) {
    int D=c->dim,L=c->n_layers,H=c->n_heads,V=c->vocab_size,T=c->context_len;
    a->D=D;a->L=L;a->H=H;a->V=V;a->T=T;
    a->emb = malloc((size_t)T*D*sizeof(float));
    a->ln1 = malloc((size_t)L*T*D*sizeof(float));
    a->ln1_mean = malloc((size_t)L*T*sizeof(float));
    a->ln1_rstd = malloc((size_t)L*T*sizeof(float));
    a->qkv = malloc((size_t)L*T*3*D*sizeof(float));
    a->att = malloc((size_t)L*H*T*T*sizeof(float));
    a->attn_out = malloc((size_t)L*T*D*sizeof(float));
    a->res1 = malloc((size_t)L*T*D*sizeof(float));
    a->ln2 = malloc((size_t)L*T*D*sizeof(float));
    a->ln2_mean = malloc((size_t)L*T*sizeof(float));
    a->ln2_rstd = malloc((size_t)L*T*sizeof(float));
    a->fc = malloc((size_t)L*T*4*D*sizeof(float));
    a->fcg = malloc((size_t)L*T*4*D*sizeof(float));
    a->res2 = malloc((size_t)L*T*D*sizeof(float));
    a->lnf = malloc((size_t)T*D*sizeof(float));
    a->lnf_mean = malloc((size_t)T*sizeof(float));
    a->lnf_rstd = malloc((size_t)T*sizeof(float));
    a->logits = malloc((size_t)T*V*sizeof(float));
    a->probs = malloc((size_t)T*V*sizeof(float));
}

/* ===== Math primitives ===== */
static void layernorm_fwd(float *out, float *mean_c, float *rstd_c,
                          const float *inp, const float *g, const float *b, int T, int C) {
    for (int t = 0; t < T; t++) {
        const float *x = inp + (size_t)t*C;
        float mean = 0; for (int i=0;i<C;i++) mean += x[i]; mean /= C;
        float var = 0; for (int i=0;i<C;i++){ float d=x[i]-mean; var += d*d; } var /= C;
        float rstd = 1.0f / sqrtf(var + 1e-5f);
        float *o = out + (size_t)t*C;
        for (int i=0;i<C;i++) o[i] = g[i]*((x[i]-mean)*rstd) + b[i];
        mean_c[t] = mean; rstd_c[t] = rstd;
    }
}
static void layernorm_bwd(float *d_inp, float *d_g, float *d_b,
                          const float *d_out, const float *inp,
                          const float *g, const float *mean_c, const float *rstd_c, int T, int C) {
    for (int t = 0; t < T; t++) {
        const float *x = inp + (size_t)t*C;
        const float *dy = d_out + (size_t)t*C;
        float mean = mean_c[t], rstd = rstd_c[t];
        float sum1 = 0, sum2 = 0;
        for (int i=0;i<C;i++) {
            float xhat = (x[i]-mean)*rstd;
            float dxhat = dy[i]*g[i];
            sum1 += dxhat; sum2 += dxhat*xhat;
        }
        sum1 /= C; sum2 /= C;
        float *di = d_inp + (size_t)t*C;
        for (int i=0;i<C;i++) {
            float xhat = (x[i]-mean)*rstd;
            float dxhat = dy[i]*g[i];
            di[i] += rstd * (dxhat - sum1 - xhat*sum2);
            d_g[i] += dy[i]*xhat;
            d_b[i] += dy[i];
        }
    }
}
/* out[M,N] = inp[M,K] @ W[K,N] + bias[N] */
/* ---- training hot-path profiler (enable with TINYGPT_PROFILE=1) -----------
 * Accumulates per-thread wall-time and FLOPs in the dense GEMMs vs the whole
 * forward/backward, plus serial Adam time, so you can read the *matmul share* of
 * training compute — the Amdahl ceiling on what GPU matmul offload could buy. */
#define PROF_MAXTHR 256
static int g_prof = 0;
static double g_fwd_s[PROF_MAXTHR], g_bwd_s[PROF_MAXTHR];
static double g_mmf_s[PROF_MAXTHR], g_mmb_s[PROF_MAXTHR];
static double g_mmf_flop[PROF_MAXTHR], g_mmb_flop[PROF_MAXTHR];
static double g_adam_s = 0.0;

static void matmul_fwd(float *out, const float *inp, const float *W, const float *bias,
                       int M, int K, int N) {
    double _t0 = g_prof ? omp_get_wtime() : 0.0;
    for (int m = 0; m < M; m++) {
        const float *xi = inp + (size_t)m*K;
        float *o = out + (size_t)m*N;
        for (int n = 0; n < N; n++) o[n] = bias ? bias[n] : 0.0f;
        for (int k = 0; k < K; k++) {
            float xv = xi[k];
            if (xv == 0.0f) continue;
            const float *Wr = W + (size_t)k*N;
            for (int n = 0; n < N; n++) o[n] += xv * Wr[n];
        }
    }
    if (g_prof) { int _t=omp_get_thread_num(); g_mmf_s[_t]+=omp_get_wtime()-_t0; g_mmf_flop[_t]+=2.0*M*K*N; }
}
static void matmul_bwd(float *d_inp, float *d_W, float *d_bias,
                       const float *d_out, const float *inp, const float *W,
                       int M, int K, int N) {
    double _t0 = g_prof ? omp_get_wtime() : 0.0;
    for (int m = 0; m < M; m++) {
        const float *go = d_out + (size_t)m*N;
        if (d_bias) for (int n=0;n<N;n++) d_bias[n] += go[n];
        const float *xi = inp + (size_t)m*K;
        float *gi = d_inp + (size_t)m*K;
        for (int k=0;k<K;k++) {
            const float *Wr = W + (size_t)k*N;
            float acc = 0; float xv = xi[k];
            float *gWr = d_W + (size_t)k*N;
            for (int n=0;n<N;n++) { acc += go[n]*Wr[n]; gWr[n] += xv*go[n]; }
            gi[k] += acc;
        }
    }
    if (g_prof) { int _t=omp_get_thread_num(); g_mmb_s[_t]+=omp_get_wtime()-_t0; g_mmb_flop[_t]+=4.0*M*K*N; }
}
static const double GELU_C = 0.7978845608028654; /* sqrt(2/pi) */
static float gelu_fwd_one(float x) {
    double inner = GELU_C * (x + 0.044715 * x*x*x);
    return (float)(0.5 * x * (1.0 + tanh(inner)));
}
static float gelu_bwd_one(float x) {
    double x3 = x*x*x;
    double inner = GELU_C * (x + 0.044715 * x3);
    double t = tanh(inner);
    double sech2 = 1.0 - t*t;
    double dinner = GELU_C * (1.0 + 3.0*0.044715*x*x);
    return (float)(0.5*(1.0+t) + 0.5*x*sech2*dinner);
}

/* ===== Forward ===== */
static float gpt_forward(const params_t *P, const gpt_config_t *c, acts_t *a,
                         const int *tokens, const int *targets /* may be NULL */) {
    double _f0 = g_prof ? omp_get_wtime() : 0.0;
    int D=c->dim, L=c->n_layers, H=c->n_heads, V=c->vocab_size, T=a->T;
    int hd = D / H;
    float scale = 1.0f / sqrtf((float)hd);
    const float *m = P->mem;

    /* embeddings */
    for (int t=0;t<T;t++) {
        const float *te = m + P->o_tok_emb + (size_t)tokens[t]*D;
        const float *pe = m + P->o_pos_emb + (size_t)t*D;
        float *o = a->emb + (size_t)t*D;
        for (int i=0;i<D;i++) o[i] = te[i] + pe[i];
    }
    const float *x = a->emb;
    for (int l=0;l<L;l++) {
        const float *ln1_g = m + P->o_ln1_g + (size_t)l*D;
        const float *ln1_b = m + P->o_ln1_b + (size_t)l*D;
        float *ln1 = a->ln1 + (size_t)l*T*D;
        layernorm_fwd(ln1, a->ln1_mean+(size_t)l*T, a->ln1_rstd+(size_t)l*T, x, ln1_g, ln1_b, T, D);
        /* qkv */
        const float *qkv_w = m + P->o_qkv_w + (size_t)l*D*(3*D);
        const float *qkv_b = m + P->o_qkv_b + (size_t)l*(3*D);
        float *qkv = a->qkv + (size_t)l*T*(3*D);
        matmul_fwd(qkv, ln1, qkv_w, qkv_b, T, D, 3*D);
        /* attention per head */
        float *attn_out = a->attn_out + (size_t)l*T*D;
        float *att = a->att + (size_t)l*H*T*T;
        memset(attn_out, 0, (size_t)T*D*sizeof(float));
        for (int h=0; h<H; h++) {
            for (int i=0;i<T;i++) {
                const float *qi = qkv + (size_t)i*(3*D) + h*hd;          /* q slice */
                float *aw = att + ((size_t)h*T + i)*T;
                /* scores for j=0..i */
                float maxs = -1e30f;
                for (int j=0;j<=i;j++) {
                    const float *kj = qkv + (size_t)j*(3*D) + D + h*hd;  /* k slice */
                    float s=0; for (int d=0;d<hd;d++) s += qi[d]*kj[d];
                    s *= scale; aw[j] = s; if (s>maxs) maxs=s;
                }
                float sum=0;
                for (int j=0;j<=i;j++){ float e=expf(aw[j]-maxs); aw[j]=e; sum+=e; }
                float inv = 1.0f/sum;
                for (int j=0;j<=i;j++) aw[j]*=inv;
                for (int j=i+1;j<T;j++) aw[j]=0;
                /* weighted sum of v */
                float *oi = attn_out + (size_t)i*D + h*hd;
                for (int j=0;j<=i;j++) {
                    const float *vj = qkv + (size_t)j*(3*D) + 2*D + h*hd;
                    float w = aw[j];
                    for (int d=0;d<hd;d++) oi[d] += w*vj[d];
                }
            }
        }
        /* output projection */
        const float *proj_w = m + P->o_proj_w + (size_t)l*D*D;
        const float *proj_b = m + P->o_proj_b + (size_t)l*D;
        float *res1 = a->res1 + (size_t)l*T*D;
        matmul_fwd(res1, attn_out, proj_w, proj_b, T, D, D);
        for (int i=0;i<T*D;i++) res1[i] += x[i];   /* residual */
        /* mlp */
        const float *ln2_g = m + P->o_ln2_g + (size_t)l*D;
        const float *ln2_b = m + P->o_ln2_b + (size_t)l*D;
        float *ln2 = a->ln2 + (size_t)l*T*D;
        layernorm_fwd(ln2, a->ln2_mean+(size_t)l*T, a->ln2_rstd+(size_t)l*T, res1, ln2_g, ln2_b, T, D);
        const float *fc_w = m + P->o_fc_w + (size_t)l*D*(4*D);
        const float *fc_b = m + P->o_fc_b + (size_t)l*(4*D);
        float *fc = a->fc + (size_t)l*T*(4*D);
        matmul_fwd(fc, ln2, fc_w, fc_b, T, D, 4*D);
        float *fcg = a->fcg + (size_t)l*T*(4*D);
        for (int i=0;i<T*(4*D);i++) fcg[i] = gelu_fwd_one(fc[i]);
        const float *fcp_w = m + P->o_fcp_w + (size_t)l*(4*D)*D;
        const float *fcp_b = m + P->o_fcp_b + (size_t)l*D;
        float *res2 = a->res2 + (size_t)l*T*D;
        matmul_fwd(res2, fcg, fcp_w, fcp_b, T, 4*D, D);
        for (int i=0;i<T*D;i++) res2[i] += res1[i]; /* residual */
        x = res2;
    }
    /* final layernorm */
    const float *lnf_g = m + P->o_lnf_g;
    const float *lnf_b = m + P->o_lnf_b;
    layernorm_fwd(a->lnf, a->lnf_mean, a->lnf_rstd, x, lnf_g, lnf_b, T, D);
    /* logits = lnf @ tok_emb^T (tied) */
    const float *tok_emb = m + P->o_tok_emb;
    for (int t=0;t<T;t++) {
        const float *h = a->lnf + (size_t)t*D;
        float *lg = a->logits + (size_t)t*V;
        for (int v=0;v<V;v++) {
            const float *e = tok_emb + (size_t)v*D;
            float s=0; for (int d=0;d<D;d++) s += h[d]*e[d];
            lg[v]=s;
        }
    }
    /* softmax + loss */
    float loss = 0; int counted = 0;
    for (int t=0;t<T;t++) {
        float *lg = a->logits + (size_t)t*V;
        float *pr = a->probs + (size_t)t*V;
        float mx=-1e30f; for (int v=0;v<V;v++) if (lg[v]>mx) mx=lg[v];
        float sum=0; for (int v=0;v<V;v++){ float e=expf(lg[v]-mx); pr[v]=e; sum+=e; }
        float inv=1.0f/sum; for (int v=0;v<V;v++) pr[v]*=inv;
        if (targets && t < T-1) {
            int tgt = targets[t];
            float p = pr[tgt]; if (p < 1e-12f) p = 1e-12f;
            loss += -logf(p); counted++;
        }
    }
    if (g_prof) { int _t=omp_get_thread_num(); g_fwd_s[_t]+=omp_get_wtime()-_f0; }
    return counted ? loss / counted : 0.0f;
}

/* ===== Backward ===== */
/* scratch gradient buffers (sized once) */
typedef struct {
    int T, D;
    float *d_x;       /* residual stream grad (T*D) */
    float *d_tmp;     /* T*D */
    float *d_ln;      /* T*D */
    float *d_qkv;     /* T*3D */
    float *d_attn_out;/* T*D */
    float *d_fc;      /* T*4D */
    float *d_fcg;     /* T*4D */
    float *d_lnf;     /* T*D */
    float *d_logits;  /* T*V */
} bwd_t;
static void bwd_alloc(bwd_t *b, const gpt_config_t *c) {
    int D=c->dim,V=c->vocab_size,T=c->context_len;
    b->T=T; b->D=D;
    b->d_x = malloc((size_t)T*D*sizeof(float));
    b->d_tmp = malloc((size_t)T*D*sizeof(float));
    b->d_ln = malloc((size_t)T*D*sizeof(float));
    b->d_qkv = malloc((size_t)T*3*D*sizeof(float));
    b->d_attn_out = malloc((size_t)T*D*sizeof(float));
    b->d_fc = malloc((size_t)T*4*D*sizeof(float));
    b->d_fcg = malloc((size_t)T*4*D*sizeof(float));
    b->d_lnf = malloc((size_t)T*D*sizeof(float));
    b->d_logits = malloc((size_t)T*V*sizeof(float));
}

static void gpt_backward(const params_t *P, params_t *G, const gpt_config_t *c,
                         const acts_t *a, bwd_t *b,
                         const int *tokens, const int *targets) {
    double _b0 = g_prof ? omp_get_wtime() : 0.0;
    int D=c->dim, L=c->n_layers, H=c->n_heads, V=c->vocab_size, T=a->T;
    int hd = D/H;
    float scale = 1.0f / sqrtf((float)hd);
    const float *m = P->mem;
    float *gm = G->mem;
    int counted = 0; for (int t=0;t<T-1;t++) if (targets) counted++;
    if (counted == 0) counted = 1;

    /* d_logits = (probs - onehot)/counted for t<T-1, else 0 */
    memset(b->d_logits, 0, (size_t)T*V*sizeof(float));
    for (int t=0;t<T-1;t++) {
        const float *pr = a->probs + (size_t)t*V;
        float *dl = b->d_logits + (size_t)t*V;
        for (int v=0;v<V;v++) dl[v] = pr[v] / counted;
        dl[targets[t]] -= 1.0f / counted;
    }
    /* head backward (tied): logits = lnf @ tok_emb^T
       d_lnf[t] = sum_v dl[t][v]*tok_emb[v]
       d_tok_emb[v] += sum_t dl[t][v]*lnf[t] */
    memset(b->d_lnf, 0, (size_t)T*D*sizeof(float));
    const float *tok_emb = m + P->o_tok_emb;
    float *g_tok_emb = gm + P->o_tok_emb;
    for (int t=0;t<T;t++) {
        const float *dl = b->d_logits + (size_t)t*V;
        const float *h = a->lnf + (size_t)t*D;
        float *dh = b->d_lnf + (size_t)t*D;
        for (int v=0;v<V;v++) {
            float g = dl[v]; if (g==0.0f) continue;
            const float *e = tok_emb + (size_t)v*D;
            float *ge = g_tok_emb + (size_t)v*D;
            for (int d=0;d<D;d++) { dh[d] += g*e[d]; ge[d] += g*h[d]; }
        }
    }
    /* final layernorm backward: input was res2 of last layer (x) */
    const float *x_final = (L>0) ? a->res2 + (size_t)(L-1)*T*D : a->emb;
    memset(b->d_x, 0, (size_t)T*D*sizeof(float));
    layernorm_bwd(b->d_x, gm + P->o_lnf_g, gm + P->o_lnf_b,
                  b->d_lnf, x_final, m + P->o_lnf_g, a->lnf_mean, a->lnf_rstd, T, D);

    /* layers in reverse; b->d_x holds gradient wrt that layer's OUTPUT (res2) */
    for (int l=L-1;l>=0;l--) {
        const float *res1 = a->res1 + (size_t)l*T*D;
        const float *ln2 = a->ln2 + (size_t)l*T*D;
        const float *fc = a->fc + (size_t)l*T*(4*D);
        const float *fcg = a->fcg + (size_t)l*T*(4*D);
        const float *attn_out = a->attn_out + (size_t)l*T*D;
        const float *ln1 = a->ln1 + (size_t)l*T*D;
        const float *qkv = a->qkv + (size_t)l*T*(3*D);
        const float *att = a->att + (size_t)l*H*T*T;
        const float *layer_in = (l>0) ? a->res2 + (size_t)(l-1)*T*D : a->emb;

        /* --- mlp block: res2 = res1 + mlp_out ; d_res2 = b->d_x --- */
        /* mlp_out backward through fcp: d_res2 -> d_fcg, grads fcp_w/b */
        memset(b->d_fcg, 0, (size_t)T*(4*D)*sizeof(float));
        matmul_bwd(b->d_fcg, gm + P->o_fcp_w + (size_t)l*(4*D)*D, gm + P->o_fcp_b + (size_t)l*D,
                   b->d_x, fcg, m + P->o_fcp_w + (size_t)l*(4*D)*D, T, 4*D, D);
        /* gelu backward */
        for (int i=0;i<T*(4*D);i++) b->d_fc[i] = b->d_fcg[i] * gelu_bwd_one(fc[i]);
        /* fc backward: d_fc -> d_ln2 (store in d_ln), grads fc_w/b */
        memset(b->d_ln, 0, (size_t)T*D*sizeof(float));
        matmul_bwd(b->d_ln, gm + P->o_fc_w + (size_t)l*D*(4*D), gm + P->o_fc_b + (size_t)l*(4*D),
                   b->d_fc, ln2, m + P->o_fc_w + (size_t)l*D*(4*D), T, D, 4*D);
        /* ln2 backward: input res1 ; produces d_res1 accumulated into d_tmp */
        memset(b->d_tmp, 0, (size_t)T*D*sizeof(float));
        layernorm_bwd(b->d_tmp, gm + P->o_ln2_g + (size_t)l*D, gm + P->o_ln2_b + (size_t)l*D,
                      b->d_ln, res1, m + P->o_ln2_g + (size_t)l*D,
                      a->ln2_mean+(size_t)l*T, a->ln2_rstd+(size_t)l*T, T, D);
        /* residual: d_res1 = d_res2 (b->d_x) + d_tmp */
        for (int i=0;i<T*D;i++) b->d_x[i] += b->d_tmp[i];

        /* --- attn block: res1 = layer_in + attn_proj ; d_res1 = b->d_x --- */
        /* attn_proj backward: d_res1 -> d_attn_out, grads proj_w/b */
        memset(b->d_attn_out, 0, (size_t)T*D*sizeof(float));
        matmul_bwd(b->d_attn_out, gm + P->o_proj_w + (size_t)l*D*D, gm + P->o_proj_b + (size_t)l*D,
                   b->d_x, attn_out, m + P->o_proj_w + (size_t)l*D*D, T, D, D);
        /* attention backward -> d_qkv */
        memset(b->d_qkv, 0, (size_t)T*(3*D)*sizeof(float));
        for (int h=0; h<H; h++) {
            for (int i=0;i<T;i++) {
                const float *aw = att + ((size_t)h*T + i)*T;
                const float *doi = b->d_attn_out + (size_t)i*D + h*hd;
                /* d_v[j] += aw[j]*doi ; d_weight[j] = doi . v[j] */
                /* first compute d_weight and accumulate d_v */
                float dweight[1024]; /* T<=1024 per head assumption for stack; fallback below */
                float *dw = dweight;
                float *dw_heap = NULL;
                if (T > 1024) { dw_heap = malloc(T*sizeof(float)); dw = dw_heap; }
                for (int j=0;j<=i;j++) {
                    const float *vj = qkv + (size_t)j*(3*D) + 2*D + h*hd;
                    float *dvj = b->d_qkv + (size_t)j*(3*D) + 2*D + h*hd;
                    float acc=0;
                    float w = aw[j];
                    for (int d=0;d<hd;d++){ acc += doi[d]*vj[d]; dvj[d] += w*doi[d]; }
                    dw[j]=acc;
                }
                /* softmax backward: d_score[j] = w[j]*(dw[j] - sum_k w[k]*dw[k]) */
                float dot=0; for (int j=0;j<=i;j++) dot += aw[j]*dw[j];
                /* d_q[i] += sum_j dscore*scale*k[j]; d_k[j] += dscore*scale*q[i] */
                const float *qi = qkv + (size_t)i*(3*D) + h*hd;
                float *dqi = b->d_qkv + (size_t)i*(3*D) + h*hd;
                for (int j=0;j<=i;j++) {
                    float dscore = aw[j]*(dw[j]-dot)*scale;
                    const float *kj = qkv + (size_t)j*(3*D) + D + h*hd;
                    float *dkj = b->d_qkv + (size_t)j*(3*D) + D + h*hd;
                    for (int d=0;d<hd;d++){ dqi[d] += dscore*kj[d]; dkj[d] += dscore*qi[d]; }
                }
                if (dw_heap) free(dw_heap);
            }
        }
        /* qkv backward: d_qkv -> d_ln1 (d_ln), grads qkv_w/b */
        memset(b->d_ln, 0, (size_t)T*D*sizeof(float));
        matmul_bwd(b->d_ln, gm + P->o_qkv_w + (size_t)l*D*(3*D), gm + P->o_qkv_b + (size_t)l*(3*D),
                   b->d_qkv, ln1, m + P->o_qkv_w + (size_t)l*D*(3*D), T, D, 3*D);
        /* ln1 backward: input layer_in -> d_tmp */
        memset(b->d_tmp, 0, (size_t)T*D*sizeof(float));
        layernorm_bwd(b->d_tmp, gm + P->o_ln1_g + (size_t)l*D, gm + P->o_ln1_b + (size_t)l*D,
                      b->d_ln, layer_in, m + P->o_ln1_g + (size_t)l*D,
                      a->ln1_mean+(size_t)l*T, a->ln1_rstd+(size_t)l*T, T, D);
        /* residual: d_layer_in = d_res1 (b->d_x) + d_tmp */
        for (int i=0;i<T*D;i++) b->d_x[i] += b->d_tmp[i];
        /* b->d_x now holds gradient wrt this layer's input = previous layer output */
    }

    /* embedding backward: d_emb = b->d_x */
    float *g_pos = gm + P->o_pos_emb;
    for (int t=0;t<T;t++) {
        const float *dx = b->d_x + (size_t)t*D;
        float *gte = g_tok_emb + (size_t)tokens[t]*D;
        float *gpe = g_pos + (size_t)t*D;
        for (int d=0;d<D;d++){ gte[d] += dx[d]; gpe[d] += dx[d]; }
    }
    if (g_prof) { int _t=omp_get_thread_num(); g_bwd_s[_t]+=omp_get_wtime()-_b0; }
}

/* ===== Adam ===== */
typedef struct { float *m, *v; int t; } adam_t;
static void adam_init(adam_t *o, int n) { o->m = calloc(n,sizeof(float)); o->v = calloc(n,sizeof(float)); o->t = 0; }
static void adam_step(adam_t *o, params_t *P, params_t *G, int n, float lr, float clip) {
    /* gradient clipping (global L2 norm) */
    if (clip > 0) {
        double sq = 0; for (int i=0;i<n;i++) sq += (double)G->mem[i]*G->mem[i];
        double norm = sqrt(sq);
        if (norm > clip) { float s=(float)(clip/norm); for (int i=0;i<n;i++) G->mem[i]*=s; }
    }
    o->t++;
    float b1=0.9f,b2=0.999f,eps=1e-8f;
    float bc1 = 1.0f - powf(b1,(float)o->t);
    float bc2 = 1.0f - powf(b2,(float)o->t);
    for (int i=0;i<n;i++) {
        float g = G->mem[i];
        o->m[i] = b1*o->m[i] + (1-b1)*g;
        o->v[i] = b2*o->v[i] + (1-b2)*g*g;
        float mh = o->m[i]/bc1, vh = o->v[i]/bc2;
        P->mem[i] -= lr * mh / (sqrtf(vh)+eps);
    }
}

/* ===== Checkpoint IO ===== */
static void save_checkpoint(const char *path, const gpt_config_t *c, const params_t *P) {
    FILE *f = fopen(path,"wb"); if(!f){perror(path);exit(1);}
    fwrite(TGPT_MAGIC,1,TGPT_MAGIC_LEN,f);
    int32_t hdr[5] = { c->dim, c->n_layers, c->n_heads, c->context_len, c->vocab_size };
    fwrite(hdr,sizeof(int32_t),5,f);
    fwrite(P->mem,sizeof(float),P->n,f);
    fclose(f);
}
static void load_checkpoint(const char *path, gpt_config_t *c, params_t *P) {
    FILE *f = fopen(path,"rb"); if(!f){perror(path);exit(1);}
    char magic[TGPT_MAGIC_LEN];
    if (fread(magic,1,TGPT_MAGIC_LEN,f)!=TGPT_MAGIC_LEN || memcmp(magic,TGPT_MAGIC,6)!=0)
        { fprintf(stderr,"bad checkpoint magic\n"); exit(1); }
    int32_t hdr[5];
    if (fread(hdr,sizeof(int32_t),5,f)!=5){fprintf(stderr,"ckpt header\n");exit(1);}
    c->dim=hdr[0];c->n_layers=hdr[1];c->n_heads=hdr[2];c->context_len=hdr[3];c->vocab_size=hdr[4];
    params_alloc(P,c);
    if (fread(P->mem,sizeof(float),P->n,f)!=(size_t)P->n){fprintf(stderr,"ckpt body\n");exit(1);}
    fclose(f);
}

/* ===== Train ===== */
static double secs(clock_t t0){return (double)(clock()-t0)/CLOCKS_PER_SEC;}

static int do_train(int argc, char **argv) {
    if (argc < 5) {
        fprintf(stderr,"train <corpus_dir> <tokenizer.bin> <checkpoint.bin> [steps dim layers heads context batch lr]\n");
        return 1;
    }
    const char *corpus = argv[2], *tkpath = argv[3], *ckpt = argv[4];
    int steps   = argc>5?atoi(argv[5]):2000;
    int dim     = argc>6?atoi(argv[6]):128;
    int layers  = argc>7?atoi(argv[7]):4;
    int heads   = argc>8?atoi(argv[8]):4;
    int context = argc>9?atoi(argv[9]):128;
    int batch   = argc>10?atoi(argv[10]):16;
    float lr    = argc>11?(float)atof(argv[11]):3e-4f;

    if (dim % heads != 0) { fprintf(stderr,"dim must be divisible by heads\n"); return 1; }

    load_exts_from_env();
    g_prof = getenv("TINYGPT_PROFILE") && atoi(getenv("TINYGPT_PROFILE")) != 0;
    rng_seed((uint64_t)time(NULL));
    fprintf(stderr,"Extensions: %s\n", g_exts);

    bpe_tokenizer_t tk; fprintf(stderr,"Loading tokenizer...\n"); bpe_load(&tk,tkpath);
    fprintf(stderr,"  vocab=%u\n", tk.vocab_size);

    fprintf(stderr,"Tokenizing corpus...\n");
    tokens_t toks={0}; uint64_t files=0; clock_t t0=clock();
    walk_dir(corpus,&tk,&toks,&files);
    fprintf(stderr,"  %llu files, %zu tokens, %.2fs\n",(unsigned long long)files,toks.size,secs(t0));
    if ((int)toks.size < context+2) { fprintf(stderr,"corpus too small\n"); return 1; }

    gpt_config_t c = { dim, layers, heads, context, (int)tk.vocab_size };
    params_t P, G; params_alloc(&P,&c); params_alloc(&G,&c); params_init(&P,&c);
    fprintf(stderr,"Model: dim=%d layers=%d heads=%d ctx=%d vocab=%d -> %d params (%.2f MB)\n",
            dim,layers,heads,context,c.vocab_size,P.n,(double)P.n*4/(1024*1024));

    /* Per-thread scratch: every batch element runs forward+backward independently,
     * so each thread gets its own activations buffer, backward scratch, gradient
     * accumulator, and token buffers. After the parallel region the per-thread
     * gradients are summed into G (the same total as the serial accumulator). */
    int nthr = omp_get_max_threads();
    if (nthr > batch) nthr = batch; /* no point spawning more threads than batch elts */
    acts_t   *as = calloc((size_t)nthr, sizeof(acts_t));
    bwd_t    *bs = calloc((size_t)nthr, sizeof(bwd_t));
    params_t *Gs = calloc((size_t)nthr, sizeof(params_t));
    int     **tokbufs = calloc((size_t)nthr, sizeof(int*));
    int     **tgtbufs = calloc((size_t)nthr, sizeof(int*));
    for (int t = 0; t < nthr; t++) {
        acts_alloc(&as[t], &c);
        bwd_alloc(&bs[t], &c);
        params_alloc(&Gs[t], &c);
        tokbufs[t] = malloc((size_t)context * sizeof(int));
        tgtbufs[t] = malloc((size_t)context * sizeof(int));
    }
    /* Pre-rolled batch start positions: same starts as the serial version regardless
     * of nthr (rng_next() called serially), so the parallel run is reproducible and
     * gives the same gradients up to float-summation order. */
    size_t *starts = malloc((size_t)batch * sizeof(size_t));
    adam_t opt; adam_init(&opt,P.n);

    fprintf(stderr,"Training %d steps, batch=%d, lr=%.1e (%d threads)\n", steps, batch, lr, nthr);
    clock_t tstart = clock();
    double ema = 0; bool ema_init=false;
    for (int step=0; step<steps; step++) {
        memset(G.mem, 0, (size_t)P.n*sizeof(float));
        for (int t = 0; t < nthr; t++) memset(Gs[t].mem, 0, (size_t)P.n*sizeof(float));
        for (int bi = 0; bi < batch; bi++) starts[bi] = (size_t)(rng_next() % (toks.size - context - 1));

        float batch_loss = 0;
        #pragma omp parallel for reduction(+:batch_loss) schedule(static)
        for (int bi=0; bi<batch; bi++) {
            int tid = omp_get_thread_num();
            int *tb = tokbufs[tid], *gb = tgtbufs[tid];
            size_t s = starts[bi];
            for (int i=0;i<context;i++){ tb[i]=(int)toks.ids[s+i]; gb[i]=(int)toks.ids[s+i+1]; }
            batch_loss += gpt_forward(&P, &c, &as[tid], tb, gb);
            gpt_backward(&P, &Gs[tid], &c, &as[tid], &bs[tid], tb, gb);
        }
        /* reduce per-thread grads into G */
        for (int t = 0; t < nthr; t++) {
            const float *src = Gs[t].mem;
            for (int i = 0; i < P.n; i++) G.mem[i] += src[i];
        }
        /* average gradients over batch */
        float inv = 1.0f/batch;
        for (int i=0;i<P.n;i++) G.mem[i]*=inv;
        batch_loss *= inv;
        double _a0 = g_prof ? omp_get_wtime() : 0.0;
        adam_step(&opt,&P,&G,P.n,lr,1.0f);
        if (g_prof) g_adam_s += omp_get_wtime() - _a0;

        if (!ema_init){ema=batch_loss;ema_init=true;} else ema=0.98*ema+0.02*batch_loss;
        if (step%20==0 || step==steps-1) {
            fprintf(stderr,"  step %5d/%d  loss=%.4f  ema=%.4f  ppl=%.2f  %.1fs\n",
                    step,steps,batch_loss,ema,exp(ema),secs(tstart));
        }
    }
    free(starts);
    for (int t = 0; t < nthr; t++) { free(tokbufs[t]); free(tgtbufs[t]); }
    free(tokbufs); free(tgtbufs); free(as); free(bs); free(Gs);

    if (g_prof) {
        double fwd=0, bwd=0, mmf=0, mmb=0, flop=0;
        for (int t=0;t<PROF_MAXTHR;t++){ fwd+=g_fwd_s[t]; bwd+=g_bwd_s[t];
            mmf+=g_mmf_s[t]; mmb+=g_mmb_s[t]; flop+=g_mmf_flop[t]+g_mmb_flop[t]; }
        double mm = mmf+mmb;
        double compute = fwd + bwd + g_adam_s;        /* total CPU-work (threads summed) */
        double nonmm = (fwd+bwd) - mm + g_adam_s;     /* attention/LN/gelu/softmax/embed/adam */
        double pct = compute>0 ? 100.0*mm/compute : 0.0;
        double gflops = mm>0 ? flop/mm/1e9 : 0.0;
        double amdahl = (compute-mm)>0 ? compute/(compute-mm) : 0.0; /* speedup ceiling if GEMM→0 */
        fprintf(stderr,"\n==== tinygpt hot-path profile (TINYGPT_PROFILE) ====\n");
        fprintf(stderr,"  config: dim=%d layers=%d heads=%d ctx=%d vocab=%d batch=%d steps=%d threads=%d\n",
                dim,layers,heads,context,c.vocab_size,batch,steps,nthr);
        fprintf(stderr,"  CPU-work summed over threads (not wall):\n");
        fprintf(stderr,"    forward total      %8.2f s  (matmul %6.2f s)\n", fwd, mmf);
        fprintf(stderr,"    backward total     %8.2f s  (matmul %6.2f s)\n", bwd, mmb);
        fprintf(stderr,"    adam (serial)      %8.2f s\n", g_adam_s);
        fprintf(stderr,"    -------------------------------------------\n");
        fprintf(stderr,"    dense matmul       %8.2f s   = %.1f%% of compute   (%.2f GFLOP/s on this CPU)\n", mm, pct, gflops);
        fprintf(stderr,"    everything else    %8.2f s   = %.1f%%   (attention, layernorm, gelu, softmax, embeddings, adam)\n",
                nonmm, 100.0-pct);
        fprintf(stderr,"  => GPU offload of the dense matmuls is an Amdahl bet: even with infinitely\n");
        fprintf(stderr,"     fast GEMM, the whole training step speeds up at most %.2fx (1/(1-%.2f)).\n", amdahl, mm/compute);
        fprintf(stderr,"  GEMM shapes per layer (M×K · K×N): qkv %d×%d·%d×%d  proj %d×%d·%d×%d  fc %d×%d·%d×%d  fcproj %d×%d·%d×%d ; logits %d×%d·%d×%d\n",
                context,dim,dim,3*dim, context,dim,dim,dim, context,dim,dim,4*dim, context,4*dim,4*dim,dim, context,dim,dim,c.vocab_size);
        fprintf(stderr,"     (run gpu-gemm at these M N K on your GPU to estimate the achievable matmul speedup)\n");
        fprintf(stderr,"====================================================\n");
    }

    fprintf(stderr,"Saving checkpoint to %s\n", ckpt);
    save_checkpoint(ckpt,&c,&P);
    fprintf(stderr,"Done. %.1fs total\n", secs(tstart));
    return 0;
}

/* ===== Generate ===== */
static int do_generate(int argc, char **argv) {
    if (argc < 5) {
        fprintf(stderr,"generate <tokenizer.bin> <checkpoint.bin> <prompt> [n_tokens] [temperature]\n");
        return 1;
    }
    const char *tkpath=argv[2], *ckpt=argv[3], *prompt=argv[4];
    int n_tokens = argc>5?atoi(argv[5]):100;
    float temp = argc>6?(float)atof(argv[6]):0.8f;

    rng_seed((uint64_t)time(NULL));
    bpe_tokenizer_t tk; bpe_load(&tk,tkpath);
    gpt_config_t c; params_t P; load_checkpoint(ckpt,&c,&P);
    if (c.vocab_size != (int)tk.vocab_size) { fprintf(stderr,"vocab mismatch tk=%u ckpt=%d\n",tk.vocab_size,c.vocab_size); return 1; }
    fprintf(stderr,"Model: dim=%d layers=%d heads=%d ctx=%d vocab=%d\n",
            c.dim,c.n_layers,c.n_heads,c.context_len,c.vocab_size);

    acts_t a; acts_alloc(&a,&c);
    int T = c.context_len;

    /* token buffer (sliding window of last T tokens) */
    int *window = malloc(T*sizeof(int));
    uint32_t enc[8192];
    uint32_t enc_n = tokenize(&tk,(const uint8_t*)prompt,strlen(prompt),enc,8192);
    if (enc_n==0){ enc[0]=' '; enc_n=1; }

    /* print prompt */
    detok_print(&tk,enc,enc_n,stdout);

    /* fill window with the prompt (truncate to last T) */
    int wlen = 0;
    for (uint32_t i=0;i<enc_n;i++){ if(wlen<T) window[wlen++]=(int)enc[i]; else { for(int k=1;k<T;k++)window[k-1]=window[k]; window[T-1]=(int)enc[i]; } }

    for (int gen=0; gen<n_tokens; gen++) {
        int curT = wlen<T?wlen:T;
        /* forward on the current window (curT positions); pad acts T but only use curT */
        /* We run forward with a temporary acts size curT by setting a.T */
        int saved_T = a.T; a.T = curT;
        gpt_forward(&P,&c,&a,window,NULL);
        a.T = saved_T;
        /* sample from logits at position curT-1 */
        float *lg = a.logits + (size_t)(curT-1)*c.vocab_size;
        /* temperature softmax */
        float mx=-1e30f; for (int v=0;v<c.vocab_size;v++) if(lg[v]>mx)mx=lg[v];
        double sum=0; for (int v=0;v<c.vocab_size;v++){ double e=exp((lg[v]-mx)/ (temp>0?temp:1e-3)); lg[v]=(float)e; sum+=e; }
        double r=rng_uniform()*sum; int pick=c.vocab_size-1;
        for (int v=0;v<c.vocab_size;v++){ r-=lg[v]; if(r<=0){pick=v;break;} }
        uint32_t tok=(uint32_t)pick;
        detok_print(&tk,&tok,1,stdout);
        /* slide window */
        if (wlen<T) window[wlen++]=pick; else { for(int k=1;k<T;k++)window[k-1]=window[k]; window[T-1]=pick; }
    }
    fputc('\n',stdout);
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr,"Usage:\n  %s train <corpus_dir> <tokenizer.bin> <checkpoint.bin> [steps dim layers heads context batch lr]\n"
                       "  %s generate <tokenizer.bin> <checkpoint.bin> <prompt> [n_tokens] [temperature]\n",
                argv[0],argv[0]);
        return 1;
    }
    if (!strcmp(argv[1],"train"))    return do_train(argc,argv);
    if (!strcmp(argv[1],"generate")) return do_generate(argc,argv);
    fprintf(stderr,"unknown subcommand '%s'\n", argv[1]);
    return 1;
}
