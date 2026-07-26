/* Verifies the JSON-generated chassis table (content_gen.h, emitted by
   tools/content_build.py --emit-c) matches the hand-written C table value-for-value.
   If the JSON and the C table ever drift apart, this fails the build -- making the
   JSON a *verified* mirror, the first step toward single-source-of-truth. */
#include "chassis.h"
#include "content_gen.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>

int main(void){
    assert(EX_GEN_CHASSIS_COUNT == EX_CHASSIS_COUNT && "chassis count drift");
    for (int i = 0; i < EX_CHASSIS_COUNT; i++){
        const ex_chassis_info_t *c = ex_chassis_info(i);
        const ex_gen_chassis_t *g = &ex_gen_chassis[i];
        assert(fabsf(c->h_mul        - g->h)         < 1e-3f);
        assert(fabsf(c->bulk_mul     - g->bulk)      < 1e-3f);
        assert(fabsf(c->arm_mul      - g->arm)       < 1e-3f);
        assert(fabsf(c->leg_mul      - g->leg)       < 1e-3f);
        assert(fabsf(c->head_mul     - g->head)      < 1e-3f);
        assert(fabsf(c->shoulder_mul - g->shoulder)  < 1e-3f);
        assert(fabsf(c->fold_spin    - (float)g->fold_spin) < 1.0f);
        assert(fabsf(c->xform_dur_mul- g->xform_dur) < 1e-3f);
        assert(c->greeble_boost == g->greeble);
    }
    printf("content_gen: JSON chassis table matches C (%d frames)\n", EX_CHASSIS_COUNT);
    // styling tables present + in sane ranges (JSON ranges are validated separately;
    // here we assert the generated C tables are well-formed).
    assert(EX_GEN_STYLE_COUNT == 10);
    for (int i = 0; i < EX_GEN_STYLE_COUNT; i++) {
        const ex_gen_style_t *s = &ex_gen_style[i];
        assert(s->gait_cadence > 0.3f && s->gait_cadence < 2.0f);
        assert(s->leg_len > 0.6f && s->leg_len < 1.5f);
        assert(s->visor_narrow >= 0.0f && s->visor_narrow <= 1.0f);
    }
    for (int i = 0; i < 6; i++) {
        const ex_gen_style_t *s = &ex_gen_style_body[i];
        assert(s->gait_cadence > 0.3f && s->gait_cadence < 2.0f);
    }
    // frame table (bots.json variants -> per class): -1 base, else 0..1 variant index
    int nonbase = 0;
    for (int i = 0; i < EX_GEN_STYLE_COUNT; i++) {
        assert(ex_gen_frame[i] >= -1 && ex_gen_frame[i] <= 2);
        if (ex_gen_frame[i] >= 0) nonbase++;
    }
    assert(ex_gen_frame[7] == 0);   // the 'prime' class (enum slot 7) must route to the prime frame
    assert(nonbase >= 1);
    printf("content_gen: frame table well-formed (%d classes route to a named frame)\n", nonbase);
    // enemy body-kind frame table (ex_body_kind order): valid indices, APEX is elite
    for (int i = 0; i < 6; i++) assert(ex_gen_frame_body[i] >= -1 && ex_gen_frame_body[i] <= 2);
    assert(ex_gen_frame_body[0] == -1);   // PLAYER body kind never carries an enemy frame here
    assert(ex_gen_frame_body[4] >= 0);    // APEX (elite tier) wears a named frame
    printf("content_gen: styling tables (%d classes + 6 body kinds) well-formed\n", EX_GEN_STYLE_COUNT);
    printf("ALL ASSERTIONS PASSED\n");
    return 0;
}
