/* oracle-ref — the llama.cpp side of the cross-engine forward oracle.
 *
 * Teacher-forced, per-position: feed a FIXED synthetic id sequence (the same one the
 * gpu-gemm driver uses: ids[i] = 1 + (i*7) % (vocab-16)) in one batch with logits at
 * every position, print "pos argmax margin" per line. Argmax ids are the contract;
 * the raw-logit margin conditions tie handling on the comparison side. Vocab size is
 * an ARGUMENT because the tiny models load with tokenizer.ggml.model = "none" and the
 * NONE vocab reports zero tokens.
 *
 *   gcc oracle-ref.c -I <llama>/include -I <llama>/ggml/include \
 *       -L <llama>/build/bin -lllama -Wl,-rpath,<llama>/build/bin -o oracle-ref
 *   ./oracle-ref tiny.gguf 48 256
 */
#include "llama.h"
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    if (argc < 4) { fprintf(stderr, "usage: %s model.gguf n_tokens vocab\n", argv[0]); return 2; }
    const char *path = argv[1];
    int K = atoi(argv[2]);
    int V = atoi(argv[3]);

    llama_backend_init();
    struct llama_model_params mp = llama_model_default_params();
    mp.n_gpu_layers = 0;
    struct llama_model *model = llama_model_load_from_file(path, mp);
    if (!model) { fprintf(stderr, "oracle-ref: model load failed\n"); return 1; }

    struct llama_context_params cp = llama_context_default_params();
    cp.n_ctx = 256; cp.n_batch = K; cp.n_ubatch = K;
    /* Plainest reference path — and a build-ISA guard: the native ggml build emits f16
     * vector ops this VM lacks, and only some archs (glm4moe) route attention through
     * flash_attn_ext. SIGILL in ggml_vec_dot_f16 was the symptom. */
    cp.flash_attn_type = LLAMA_FLASH_ATTN_TYPE_DISABLED;
    struct llama_context *ctx = llama_init_from_model(model, cp);
    if (!ctx) { fprintf(stderr, "oracle-ref: context init failed\n"); return 1; }

    struct llama_batch b = llama_batch_init(K, 0, 1);
    b.n_tokens = K;
    for (int i = 0; i < K; i++) {
        b.token[i] = 1 + (i * 7) % (V - 16);
        b.pos[i] = i;
        b.n_seq_id[i] = 1;
        b.seq_id[i][0] = 0;
        b.logits[i] = 1;                 /* logits at EVERY position */
    }
    if (llama_decode(ctx, b) != 0) { fprintf(stderr, "oracle-ref: decode failed\n"); return 1; }

    for (int i = 0; i < K; i++) {
        const float *l = llama_get_logits_ith(ctx, i);
        int a1 = 0, a2 = -1;
        for (int j = 1; j < V; j++) if (l[j] > l[a1]) a1 = j;
        for (int j = 0; j < V; j++) if (j != a1 && (a2 < 0 || l[j] > l[a2])) a2 = j;
        printf("%d %d %.6f\n", i, a1, l[a1] - l[a2]);
    }
    llama_batch_free(b);
    llama_free(ctx);
    llama_model_free(model);
    return 0;
}
