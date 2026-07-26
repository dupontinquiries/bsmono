/* test_bodykind -- the per-kind silhouette contract: enemy body kinds must read
 * DISTINCTLY. Player symmetric; Corrupted asymmetric (the infection's tell);
 * Apex much larger; Hijack smaller. Pure logic over ex_robot_layout_ex, no GL. */
#include <stdio.h>
#include <string.h>
#include <assert.h>
#include <math.h>
#include "robot_parts.h"

static void layout(ex_part_pose_t p[EX_ROBOT_PARTS], ex_body_kind kind) {
    ex_build_params_t bp = ex_build_default();
    ex_robot_layout_ex(p, 0, kind, &bp, 1.0f, 0.0f, 0.0f, 0);
}

/* total vertical extent of a body (rough height proxy: highest part top). */
static float body_height(const ex_part_pose_t p[EX_ROBOT_PARTS]) {
    float top = 0.0f;
    for (int i = 0; i < EX_ROBOT_PARTS; i++) {
        float t = p[i].offset[1] + p[i].sy;
        if (t > top) top = t;
    }
    return top;
}

/* left/right arm asymmetry: how different are the two arms in size + placement. */
static float arm_asymmetry(const ex_part_pose_t p[EX_ROBOT_PARTS]) {
    const ex_part_pose_t *l = &p[EX_PART_ARM_L], *r = &p[EX_PART_ARM_R];
    float d = fabsf(l->sx-r->sx) + fabsf(l->sy-r->sy) + fabsf(l->sz-r->sz);
    d += fabsf(fabsf(l->offset[0]) - fabsf(r->offset[0]));   // mirror-x placement
    d += fabsf(l->offset[1] - r->offset[1]);                // height of each arm
    return d;
}

int main(void) {
    ex_part_pose_t player[EX_ROBOT_PARTS], corrupt[EX_ROBOT_PARTS];
    ex_part_pose_t apex[EX_ROBOT_PARTS], hijack[EX_ROBOT_PARTS];
    layout(player,  EX_BODY_PLAYER);
    layout(corrupt, EX_BODY_CORRUPTED);
    layout(apex,    EX_BODY_APEX);
    layout(hijack,  EX_BODY_HIJACK);

    /* the player is essentially symmetric (arms match). */
    float p_asym = arm_asymmetry(player);
    printf("player arm asymmetry: %.3f (want ~0)\n", p_asym);
    assert(p_asym < 0.15f);

    /* the CORRUPTED is strongly asymmetric -- its defining infected silhouette.
       Must be many times more lopsided than the player. */
    float c_asym = arm_asymmetry(corrupt);
    printf("corrupted arm asymmetry: %.3f (want >> player)\n", c_asym);
    assert(c_asym > 0.4f);
    assert(c_asym > p_asym * 4.0f);

    /* the APEX towers over the player (boss reads big). */
    float ph = body_height(player), ah = body_height(apex);
    printf("player h=%.2f  apex h=%.2f (apex must be taller)\n", ph, ah);
    assert(ah > ph * 1.25f);

    /* the HIJACK is smaller than the player (spindly swarm reads small). */
    float hh = body_height(hijack);
    printf("hijack h=%.2f (must be shorter than player %.2f)\n", hh, ph);
    assert(hh < ph);

    /* the HIJACK reads INSECTILE: an oversized sensor head vs its chest, and
       spindlier (thinner) arms than the player. Its defining swarm-bug look. */
    float hj_head = hijack[EX_PART_HEAD].sy / hijack[EX_PART_CHEST].sy;
    float pl_head = player[EX_PART_HEAD].sy / player[EX_PART_CHEST].sy;
    printf("hijack head/chest=%.2f  player head/chest=%.2f (hijack bigger)\n", hj_head, pl_head);
    assert(hj_head > pl_head * 1.5f);          /* clearly oversized sensor head */
    float hj_arm = hijack[EX_PART_ARM_L].sy / hijack[EX_PART_ARM_L].sx;
    float pl_arm = player[EX_PART_ARM_L].sy / player[EX_PART_ARM_L].sx;
    printf("hijack arm aspect=%.2f  player=%.2f (hijack spindlier)\n", hj_arm, pl_arm);
    assert(hj_arm > pl_arm);                   /* thinner, insect-limb arms */

    /* the CORRUPTED infection VISIBLY PROGRESSES: asymmetry grows monotonically
       from stage 1 (just-turned, subtle) to stage 3 (fully overgrown horror). */
    ex_part_pose_t cs[4][EX_ROBOT_PARTS];
    float asym_stage[4];
    for (int st = 1; st <= 3; st++) {
        ex_build_params_t bp = ex_build_default();
        bp.corrupt_stage = st;
        ex_robot_layout_ex(cs[st], 0, EX_BODY_CORRUPTED, &bp, 1.0f, 0.0f, 0.0f, 0);
        asym_stage[st] = arm_asymmetry(cs[st]);
        printf("corrupted stage %d asymmetry: %.3f\n", st, asym_stage[st]);
    }
    assert(asym_stage[1] < asym_stage[2]);   // infection deepens
    assert(asym_stage[2] < asym_stage[3]);
    assert(asym_stage[1] > p_asym);          // even stage 1 is already wrong

    printf("ALL ASSERTIONS PASSED\n");
    return 0;
}
