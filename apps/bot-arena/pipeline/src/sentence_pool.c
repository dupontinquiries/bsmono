/*
 * sentence_pool.c — Pool word vectors into paragraph vectors
 *
 * Loads tokenizer.bin and vectors.bin (word2vec output), walks corpus,
 * splits each file on blank lines, tokenizes each paragraph via BPE,
 * mean-pools the word vectors, and writes paragraph_vectors.bin.
 *
 * Preserves document_id and position_in_doc so v1.3 (transitions) can
 * compute differences between consecutive paragraphs within a document.
 *
 * Build:
 *   gcc -std=c99 -O3 -Wall -Wextra -o sentence_pool sentence_pool.c -lz -lm
 *
 * Usage:
 *   ./sentence_pool <corpus_dir> <tokenizer.bin> <vectors.bin> <paragraph_vectors.bin>
 *
 * Output format (paragraph_vectors.bin):
 *   Header (24 bytes):
 *     char[8]  magic = "PARAG01\0"
 *     uint32   version = 1
 *     uint32   dim
 *     uint64   payload_compressed_size
 *   Payload (zlib level 9):
 *     uint32 n_paragraphs
 *     uint32 n_documents
 *     For each paragraph:
 *       uint32 doc_id
 *       uint32 pos_in_doc
 *       uint32 src_path_len ; bytes
 *       uint32 text_len     ; bytes (capped at 1024)
 *       float[dim] vector
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

#define PARAG_MAGIC          "PARAG01"
#define PARAG_MAGIC_LEN      8
#define PARAG_VERSION        1u
#define WORDV_MAGIC          "WORDV01"
#define WORDV_MAGIC_LEN      8
#define BPE_MAGIC            "BPETK01"
#define BPE_MAGIC_LEN        8
#define BPE_BASE_VOCAB       256u
#define MAX_TEXT_EXCERPT     1024

static uint64_t fnv1a(const void *data, size_t len) {
    const uint8_t *p = (const uint8_t *)data;
    uint64_t h = 0xcbf29ce484222325ULL;
    for (size_t i = 0; i < len; i++) { h ^= p[i]; h *= 0x100000001b3ULL; }
    return h;
}

/* ===== Tokenizer (same as elsewhere) ===== */
typedef struct { uint64_t key; uint32_t rank; uint32_t new_id; } bpe_merge_entry_t;
typedef struct {
    uint32_t vocab_size, num_merges;
    bpe_merge_entry_t *table; uint32_t table_cap;
} bpe_tokenizer_t;

static void bpe_load(bpe_tokenizer_t *tk, const char *path) {
    FILE *f = fopen(path, "rb"); if (!f) { perror("tk"); exit(1); }
    char magic[BPE_MAGIC_LEN];
    if (fread(magic,1,BPE_MAGIC_LEN,f) != BPE_MAGIC_LEN || memcmp(magic,BPE_MAGIC,7) != 0)
        { fprintf(stderr,"bad tk magic\n"); exit(1); }
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

static bool bpe_lookup(const bpe_tokenizer_t *tk, uint32_t a, uint32_t b, uint32_t *r_, uint32_t *nid_) {
    uint64_t key = ((uint64_t)a << 32) | b;
    uint64_t hash = fnv1a(&key, 8);
    uint32_t mask = tk->table_cap - 1;
    uint32_t idx = (uint32_t)(hash & mask);
    while (tk->table[idx].new_id != 0) {
        if (tk->table[idx].key == key) { *r_ = tk->table[idx].rank; *nid_ = tk->table[idx].new_id; return true; }
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

/* ===== Word vector loading ===== */
typedef struct {
    uint32_t vocab_size, dim;
    float *V;  /* vocab_size * dim */
} wordvec_t;

static void wordvec_load(wordvec_t *w, const char *path) {
    FILE *f = fopen(path, "rb"); if (!f) { perror("vectors"); exit(1); }
    char magic[WORDV_MAGIC_LEN];
    if (fread(magic,1,WORDV_MAGIC_LEN,f) != WORDV_MAGIC_LEN || memcmp(magic, WORDV_MAGIC, 7) != 0)
        { fprintf(stderr, "bad vectors magic\n"); exit(1); }
    uint32_t version, vocab_size, dim;
    uint64_t total_tokens, compressed;
    if (fread(&version,4,1,f)!=1 || fread(&vocab_size,4,1,f)!=1 || fread(&dim,4,1,f)!=1 ||
        fread(&total_tokens,8,1,f)!=1 || fread(&compressed,8,1,f)!=1)
        { fprintf(stderr,"vectors header\n"); exit(1); }
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

/* ===== Paragraph collection ===== */
typedef struct {
    uint32_t doc_id;
    uint32_t pos_in_doc;
    char    *src_path;
    uint32_t src_path_len;
    char    *text_excerpt;
    uint32_t text_len;
    float   *vector;
} paragraph_t;

typedef struct {
    paragraph_t *list;
    uint32_t     size;
    uint32_t     cap;
    uint32_t     dim;
    uint32_t     doc_count;
} paragraph_set_t;

static void ps_init(paragraph_set_t *ps, uint32_t dim) {
    ps->list = NULL; ps->size = 0; ps->cap = 0; ps->dim = dim; ps->doc_count = 0;
}

static void ps_add(paragraph_set_t *ps, uint32_t doc_id, uint32_t pos_in_doc,
                   const char *src, const char *text, uint32_t text_len,
                   const float *vec) {
    if (ps->size >= ps->cap) {
        ps->cap = ps->cap ? ps->cap * 2 : 1024;
        ps->list = realloc(ps->list, ps->cap * sizeof(paragraph_t));
    }
    paragraph_t *p = &ps->list[ps->size++];
    p->doc_id = doc_id;
    p->pos_in_doc = pos_in_doc;
    p->src_path = strdup(src);
    p->src_path_len = (uint32_t)strlen(src);
    uint32_t take = text_len > MAX_TEXT_EXCERPT ? MAX_TEXT_EXCERPT : text_len;
    p->text_excerpt = malloc(take);
    memcpy(p->text_excerpt, text, take);
    p->text_len = take;
    p->vector = malloc(ps->dim * sizeof(float));
    memcpy(p->vector, vec, ps->dim * sizeof(float));
}

/* Pool word vectors for a paragraph (mean pooling) */
static bool pool_paragraph(const bpe_tokenizer_t *tk, const wordvec_t *w,
                            const char *text, uint32_t text_len, float *out_vec) {
    uint32_t toks[8192];
    uint32_t n = tokenize(tk, (const uint8_t *)text, text_len, toks, 8192);
    if (n == 0) return false;
    memset(out_vec, 0, w->dim * sizeof(float));
    uint32_t counted = 0;
    for (uint32_t i = 0; i < n; i++) {
        uint32_t id = toks[i];
        if (id >= w->vocab_size) continue;
        const float *v = w->V + (size_t)id * w->dim;
        for (uint32_t d = 0; d < w->dim; d++) out_vec[d] += v[d];
        counted++;
    }
    if (counted == 0) return false;
    float inv = 1.0f / (float)counted;
    for (uint32_t d = 0; d < w->dim; d++) out_vec[d] *= inv;
    return true;
}

/* ===== File walk ===== */
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

static void process_file(const char *path, uint32_t doc_id,
                          const bpe_tokenizer_t *tk, const wordvec_t *w,
                          paragraph_set_t *ps) {
    int fd = open(path, O_RDONLY); if (fd < 0) return;
    struct stat st;
    if (fstat(fd,&st) < 0 || st.st_size == 0) { close(fd); return; }
    void *p = mmap(NULL, st.st_size, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED) { close(fd); return; }
    const char *text = (const char *)p;
    size_t len = st.st_size;
    size_t i = 0;
    uint32_t pos = 0;
    float *vec = malloc(w->dim * sizeof(float));
    while (i < len) {
        size_t start = i;
        while (i < len) {
            if (i + 1 < len && text[i] == '\n' && text[i+1] == '\n') break;
            i++;
        }
        size_t plen = i - start;
        if (plen >= 16) {
            if (pool_paragraph(tk, w, text + start, plen, vec)) {
                ps_add(ps, doc_id, pos++, path, text + start, (uint32_t)plen, vec);
            }
        }
        while (i < len && text[i] == '\n') i++;
    }
    free(vec);
    munmap(p, st.st_size); close(fd);
}

static void walk_dir(const char *dir, const bpe_tokenizer_t *tk,
                     const wordvec_t *w, paragraph_set_t *ps) {
    DIR *d = opendir(dir); if (!d) return;
    struct dirent *e; char path[4096];
    while ((e = readdir(d))) {
        if (e->d_name[0] == '.') continue;
        int n = snprintf(path,sizeof(path),"%s/%s",dir,e->d_name);
        if (n < 0 || (size_t)n >= sizeof(path)) continue;
        struct stat st; if (stat(path,&st)<0) continue;
        if (S_ISDIR(st.st_mode)) walk_dir(path,tk,w,ps);
        else if (S_ISREG(st.st_mode) && has_corpus_ext(e->d_name)) {
            uint32_t doc_id = ps->doc_count++;
            process_file(path, doc_id, tk, w, ps);
        }
    }
    closedir(d);
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

static void serialize(const char *path, const paragraph_set_t *ps) {
    buf_t pl = {0};
    uint32_t n = ps->size;
    uint32_t d = ps->doc_count;
    buf_write(&pl, &n, 4);
    buf_write(&pl, &d, 4);
    for (uint32_t i = 0; i < n; i++) {
        paragraph_t *p = &ps->list[i];
        buf_write(&pl, &p->doc_id, 4);
        buf_write(&pl, &p->pos_in_doc, 4);
        buf_write(&pl, &p->src_path_len, 4);
        buf_write(&pl, p->src_path, p->src_path_len);
        buf_write(&pl, &p->text_len, 4);
        buf_write(&pl, p->text_excerpt, p->text_len);
        buf_write(&pl, p->vector, ps->dim * sizeof(float));
    }

    uLongf cap = compressBound((uLong)pl.size);
    uint8_t *comp = malloc(cap);
    uLongf csize = cap;
    if (compress2(comp, &csize, pl.buf, (uLong)pl.size, 9) != Z_OK)
        { fprintf(stderr,"compress\n"); exit(1); }

    FILE *f = fopen(path, "wb"); if (!f) { perror("output"); exit(1); }
    fwrite(PARAG_MAGIC, 1, PARAG_MAGIC_LEN, f);
    uint32_t v = PARAG_VERSION;
    uint64_t cs = csize;
    fwrite(&v,4,1,f); fwrite(&ps->dim,4,1,f); fwrite(&cs,8,1,f);
    fwrite(comp,1,csize,f);
    fclose(f);

    fprintf(stderr, "Wrote %s: %.2f MB compressed (%.2f MB raw, %u paragraphs, %u documents)\n",
            path, csize / (1024.0*1024.0), pl.size / (1024.0*1024.0),
            ps->size, ps->doc_count);
    free(pl.buf); free(comp);
}

int main(int argc, char **argv) {
    if (argc != 5) {
        fprintf(stderr, "Usage: %s <corpus_dir> <tokenizer.bin> <vectors.bin> <paragraph_vectors.bin>\n", argv[0]);
        return 1;
    }
    bpe_tokenizer_t tk;
    wordvec_t w;
    fprintf(stderr, "Loading tokenizer...\n");
    bpe_load(&tk, argv[2]);
    fprintf(stderr, "  vocab=%u\n", tk.vocab_size);
    fprintf(stderr, "Loading word vectors...\n");
    wordvec_load(&w, argv[3]);
    fprintf(stderr, "  vocab=%u dim=%u\n", w.vocab_size, w.dim);
    if (w.vocab_size != tk.vocab_size) {
        fprintf(stderr, "vocab size mismatch between tokenizer (%u) and vectors (%u)\n",
                tk.vocab_size, w.vocab_size);
        return 1;
    }

    paragraph_set_t ps;
    ps_init(&ps, w.dim);

    fprintf(stderr, "Walking corpus and pooling...\n");
    clock_t t0 = clock();
    walk_dir(argv[1], &tk, &w, &ps);
    fprintf(stderr, "  %u paragraphs from %u documents, %.2fs\n",
            ps.size, ps.doc_count, (double)(clock() - t0) / CLOCKS_PER_SEC);

    serialize(argv[4], &ps);
    return 0;
}
