#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "ggml-quants.h"

// Dump reference dequant output as text: one float per line, "%.9g".
int main(int argc, char** argv) {
    const char* ty = argv[1];
    long n = atol(argv[2]);          // element count
    // read raw block bytes from stdin
    static unsigned char buf[1<<22];
    size_t got = fread(buf, 1, sizeof buf, stdin);
    (void)got;
    float* out = malloc(sizeof(float) * n);
    #define D(name, fn) if (!strcmp(ty, name)) { fn((const void*)buf, out, n); } else
    D("q2_k",  dequantize_row_q2_K)
    D("q3_k",  dequantize_row_q3_K)
    D("q4_k",  dequantize_row_q4_K)
    D("q5_k",  dequantize_row_q5_K)
    D("q6_k",  dequantize_row_q6_K)
    D("iq2_xxs", dequantize_row_iq2_xxs)
    D("iq2_xs", dequantize_row_iq2_xs)
    D("iq1_s", dequantize_row_iq1_s)
    D("iq2_s", dequantize_row_iq2_s)
    D("iq3_xxs", dequantize_row_iq3_xxs)
    D("iq3_s", dequantize_row_iq3_s)
    D("iq1_m", dequantize_row_iq1_m)
    D("iq4_nl", dequantize_row_iq4_nl)
    D("iq4_xs", dequantize_row_iq4_xs)
    { fprintf(stderr, "unknown type %s\n", ty); return 2; }
    for (long i = 0; i < n; i++) printf("%.9g\n", out[i]);
    return 0;
}

// Link stubs: the dequant paths never reach these (they are in the QUANTIZE side).
void ggml_abort(const char* f, int l, const char* fmt, ...) { fprintf(stderr, "abort %s:%d\n", f, l); exit(3); }
size_t ggml_row_size(enum ggml_type t, int64_t n) { (void)t; return (size_t)n; }
size_t ggml_type_size(enum ggml_type t) { (void)t; return 1; }
const char* ggml_type_name(enum ggml_type t) { (void)t; return "?"; }
