/* behavior.c -- runtime for the tuned Markov behaviour FSM (see behavior.h). */
#include "behavior.h"

/* Live mode enum order (mirrors menu.h ex_mode_t):
 *   0 ARENA_FIGHT, 1 SURVIVAL, 2 STALKERS, 3 DEFEND, 4 ESCALATION. */
ex_beh_ctx_t ex_behavior_ctx(int mode, int in_campaign, int mission_index) {
    if (in_campaign) {
        /* campaign missions 0..7 -> EX_BCTX_M1..M8 (contiguous after the modes) */
        int mi = mission_index; if (mi < 0) mi = 0; if (mi > 7) mi = 7;
        return (ex_beh_ctx_t)(EX_BCTX_M1_PLUTO + mi);
    }
    switch (mode) {
        case 0: return EX_BCTX_ARENA;
        case 1: return EX_BCTX_SURVIVAL;
        case 2: return EX_BCTX_STALKERS;
        case 3: return EX_BCTX_DEFEND;
        case 4: return EX_BCTX_ESCALATION;
        default: return EX_BCTX_ARENA;
    }
}

ex_beh_arch_t ex_behavior_arch(int vehicle_type) {
    if (vehicle_type < 0 || vehicle_type >= (int)sizeof(ex_beh_vehicle_arch))
        return EX_BARCH_HEAVY;
    return (ex_beh_arch_t)ex_beh_vehicle_arch[vehicle_type];
}

int ex_behavior_step(ex_beh_ctx_t ctx, ex_beh_arch_t arch, int cur, uint32_t *rng) {
    if (ctx  < 0 || ctx  >= EX_BEH_CTX_COUNT)  ctx  = EX_BCTX_ARENA;
    if (arch < 0 || arch >= EX_BEH_ARCH_COUNT) arch = EX_BARCH_HEAVY;
    if (cur  < 0 || cur  >= EX_BEH_COUNT)      cur  = EX_BEH_ADVANCE;
    const float *row = ex_beh_matrix[ctx][arch][cur];
    uint32_t x = *rng ? *rng : 0x9e3779b9u;      /* never let the state stick at 0 */
    x ^= x << 13; x ^= x >> 17; x ^= x << 5; *rng = x;
    float r = (float)(x & 0xFFFFFF) / (float)0x1000000;
    float acc = 0.0f;
    for (int j = 0; j < EX_BEH_COUNT; j++) { acc += row[j]; if (r <= acc) return j; }
    return cur;                                   /* rounding fallback */
}

const char *ex_behavior_name(int b) {
    static const char *N[EX_BEH_COUNT] = {
        "ADVANCE","STRAFE","HOLD","FLANK","CHARGE","RETREAT","HARASS","REGROUP" };
    return (b >= 0 && b < EX_BEH_COUNT) ? N[b] : "?";
}
