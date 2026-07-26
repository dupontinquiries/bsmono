/* pose_dump -- prints the engine's real per-part layout (offset + non-uniform
 * scale) for a body kind, as TSV, so tools/silhouette.py can composite the
 * JSON-compiled part meshes at the exact transforms the game uses. This links
 * the actual robot_parts.c layout -- no reimplementation, no drift.
 *
 * usage: pose_dump <body_kind 0..5> <vehicle 0|1>
 * out:   part_index<TAB>ox<TAB>oy<TAB>oz<TAB>sx<TAB>sy<TAB>sz
 */
#include <stdio.h>
#include <stdlib.h>
#include "robot_parts.h"

int main(int argc, char **argv)
{
    int kind    = (argc > 1) ? atoi(argv[1]) : 0;
    int vehicle = (argc > 2) ? atoi(argv[2]) : 0;
    int alt     = (argc > 3) ? atoi(argv[3]) : 0;
    int cstage  = (argc > 4) ? atoi(argv[4]) : 0;   /* corruption stage 1..3 */
    if (kind < 0 || kind > 5) kind = 0;

    ex_part_pose_t poses[EX_ROBOT_PARTS];
    ex_build_params_t bp = ex_build_default();
    bp.alt_mode = alt;   /* vehicle-mode remnant family (0 car .. dino/jet/tank/etc.) */
    bp.corrupt_stage = cstage;
    /* static neutral standing pose: phase 0, speed 0 (no gait sway) so the
     * silhouette is the rest proportions, which is what we're inspecting. */
    ex_robot_layout_ex(poses, vehicle, (ex_body_kind)kind, &bp,
                        1.0f, 0.0f, 0.0f, 1);

    for (int i = 0; i < EX_ROBOT_PARTS; i++) {
        const ex_part_pose_t *p = &poses[i];
        /* fall back to uniform scale when non-uniform hints are 0 (some paths
         * only set ->scale). */
        float sx = p->sx != 0.0f ? p->sx : p->scale;
        float sy = p->sy != 0.0f ? p->sy : p->scale;
        float sz = p->sz != 0.0f ? p->sz : p->scale;
        printf("%d\t%.5f\t%.5f\t%.5f\t%.5f\t%.5f\t%.5f\t%d\t%d\t%d\n",
               i, p->offset[0], p->offset[1], p->offset[2], sx, sy, sz,
               p->color[0], p->color[1], p->color[2]);
    }
    return 0;
}
