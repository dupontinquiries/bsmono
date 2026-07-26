/* test_wheels -- guards that vehicle wheels are symmetric discs.
 * The user reported wheels looking asymmetric / non-disc; the fix routes wheel
 * poses through W() (which forces sy==sz and tags EX_PSHAPE_WHEEL) and a real
 * disc mesh. This locks that in: every wheel pose must be a symmetric disc. */
#include "robot_parts.h"
#include "vehicle.h"
#include <stdio.h>
#include <math.h>
#include <assert.h>

static int check(ex_vehicle_type t, const char *name, int min_wheels) {
    ex_part_pose_t poses[EX_ROBOT_PARTS];
    int n = 0;
    ex_build_params_t bp = ex_build_default();
    ex_vehicle_layout(poses, EX_ROBOT_PARTS, &n, t, &bp, 1.0f, 0.0f, 2.0f, 1, EX_BODY_PLAYER);
    int nwheel = 0;
    for (int i = 0; i < n; i++) {
        if (poses[i].shape == EX_PSHAPE_WHEEL) {
            nwheel++;
            // a disc: the two radial axes must be equal (never an ellipse)
            assert(fabsf(poses[i].sy - poses[i].sz) < 1e-4f);
            // and it must have real size
            assert(poses[i].sy > 0.0f && poses[i].sx > 0.0f);
        }
    }
    printf("test_wheels: %-6s has %d symmetric-disc wheels\n", name, nwheel);
    assert(nwheel >= min_wheels);
    return nwheel;
}

int main(void) {
    check(EX_VEH_CAR,   "car",   4);
    check(EX_VEH_CYCLE, "cycle", 2);
    check(EX_VEH_WHEEL, "wheel", 2);
    // a non-wheeled vehicle (jet) should have NO wheel-shaped poses
    {
        ex_part_pose_t poses[EX_ROBOT_PARTS]; int n = 0;
        ex_build_params_t bp = ex_build_default();
        ex_vehicle_layout(poses, EX_ROBOT_PARTS, &n, EX_VEH_JET, &bp, 1.0f, 0.0f, 2.0f, 1, EX_BODY_PLAYER);
        int w = 0; for (int i = 0; i < n; i++) if (poses[i].shape == EX_PSHAPE_WHEEL) w++;
        assert(w == 0);
        printf("test_wheels: jet has 0 wheels (correct)\n");
    }
    printf("ALL ASSERTIONS PASSED\n");
    return 0;
}
