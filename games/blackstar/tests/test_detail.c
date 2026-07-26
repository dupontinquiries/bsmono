/* Regression test for the "default cube" bug: a greeble generated on a zeroed/
 * collapsed part (e.g. EX_PART_BACK, which the signature draws separately) got
 * dimensions of exactly 0, so the renderer fell back to the model's native full
 * 2-unit size -- a stray default cube stuck on the model. Every greeble must have a
 * non-degenerate scale, and no greebles should land on a zero-area part. */
#include "detail.h"
#include <assert.h>
#include <stdio.h>

int main(void) {
    ex_detail_part_t parts[3];
    for (int i = 0; i < 3; i++) parts[i] = (ex_detail_part_t){0};
    parts[0].sx = 0.5f; parts[0].sy = 0.4f; parts[0].sz = 0.3f;   // normal chest
    parts[1].sx = 0.0f; parts[1].sy = 0.0f; parts[1].sz = 0.0f;   // zeroed BACK
    parts[2].sx = 0.2f; parts[2].sy = 0.2f; parts[2].sz = 0.2f;   // small joint

    ex_greeble_t out[400];
    uint8_t acc[3] = {200, 200, 200};
    int n = ex_detail_generate(out, 400, parts, 3, 1234, 300, acc);
    assert(n > 0);

    for (int i = 0; i < n; i++) {
        assert(out[i].parent != 1 && "no greeble may land on the zeroed part");
        // no degenerate box: at least one dimension must be clearly non-zero
        float mx = out[i].sx; if (out[i].sy > mx) mx = out[i].sy; if (out[i].sz > mx) mx = out[i].sz;
        assert(mx > 1e-4f && "greeble collapsed to zero size -> would render as a full cube");
    }
    printf("detail: %d greebles, none on the zeroed part, none zero-scale\n", n);
    printf("ALL ASSERTIONS PASSED\n");
    return 0;
}
