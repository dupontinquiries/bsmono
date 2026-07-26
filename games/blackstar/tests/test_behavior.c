/* test_behavior.c -- validate the tuned Markov behaviour FSM. */
#include "behavior.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>

int main(void) {
    /* 1) every generated matrix is row-stochastic (rows sum to ~1, no negatives). */
    for (int c = 0; c < EX_BEH_CTX_COUNT; c++)
        for (int a = 0; a < EX_BEH_ARCH_COUNT; a++)
            for (int i = 0; i < EX_BEH_COUNT; i++) {
                float s = 0.0f;
                for (int j = 0; j < EX_BEH_COUNT; j++) {
                    float w = ex_beh_matrix[c][a][i][j];
                    assert(w >= 0.0f && w <= 1.0001f);
                    s += w;
                }
                assert(fabsf(s - 1.0f) < 1e-3f && "matrix row not normalised");
            }

    /* 2) the vehicle->archetype map covers all vehicle types with valid archetypes. */
    for (int v = 0; v < (int)sizeof(ex_beh_vehicle_arch); v++) {
        ex_beh_arch_t ar = ex_behavior_arch(v);
        assert(ar >= 0 && ar < EX_BEH_ARCH_COUNT);
    }

    /* 3) the mode/mission -> context map is in range + distinct per mission. */
    assert(ex_behavior_ctx(0,0,0) == EX_BCTX_ARENA);
    assert(ex_behavior_ctx(4,0,0) == EX_BCTX_ESCALATION);
    assert(ex_behavior_ctx(0,1,0) == EX_BCTX_M1_PLUTO);
    assert(ex_behavior_ctx(0,1,7) == EX_BCTX_M8_CYBERTRON);

    /* 4) the step samples valid behaviours and, over many draws, follows the row's
     *    distribution (the dominant target of a row is drawn most often). */
    uint32_t rng = 12345u;
    int counts[EX_BEH_COUNT] = {0};
    for (int n = 0; n < 20000; n++) {
        int nb = ex_behavior_step(EX_BCTX_ARENA, EX_BARCH_FLYER, EX_BEH_ADVANCE, &rng);
        assert(nb >= 0 && nb < EX_BEH_COUNT);
        counts[nb]++;
    }
    /* the argmax of the sampled histogram must equal the argmax of the ADVANCE row */
    const float *row = ex_beh_matrix[EX_BCTX_ARENA][EX_BARCH_FLYER][EX_BEH_ADVANCE];
    int row_best = 0, cnt_best = 0;
    for (int j = 1; j < EX_BEH_COUNT; j++) { if (row[j] > row[row_best]) row_best = j;
                                             if (counts[j] > counts[cnt_best]) cnt_best = j; }
    assert(row_best == cnt_best && "sampled distribution does not match the matrix row");

    /* 5) contexts are actually tuned differently (arena flyer != defend flyer). */
    int diff = 0;
    for (int i = 0; i < EX_BEH_COUNT && !diff; i++)
        for (int j = 0; j < EX_BEH_COUNT; j++)
            if (fabsf(ex_beh_matrix[EX_BCTX_ARENA][EX_BARCH_FLYER][i][j]
                    - ex_beh_matrix[EX_BCTX_DEFEND][EX_BARCH_FLYER][i][j]) > 0.02f) { diff = 1; break; }
    assert(diff && "arena and defend produced identical matrices -- tuning did nothing");

    printf("behavior: %d contexts x %d archetypes x %dx%d matrices all valid + tuned\n",
           EX_BEH_CTX_COUNT, EX_BEH_ARCH_COUNT, EX_BEH_COUNT, EX_BEH_COUNT);
    printf("ALL ASSERTIONS PASSED\n");
    return 0;
}
