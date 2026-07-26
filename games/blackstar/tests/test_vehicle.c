/* test_vehicle -- exercises the vehicle archetype data + layout in vehicle.c.
 * These drive alt-mode handling feel and the transformed silhouette; the
 * handling stats in particular encode design intent (a rooted sentry, a
 * nimble cycle) that a careless edit could invert. */
#include "vehicle.h"
#include "robot_parts.h"
#include <stdio.h>
#include <math.h>
#include <assert.h>
#include <string.h>

/* ---- every type has a name ---- */
static void test_names(void) {
    for (int t = 0; t < EX_VEH_COUNT; t++) {
        const char *n = ex_vehicle_name((ex_vehicle_type)t);
        assert(n && n[0]);
    }
    printf("test_vehicle: all %d types resolve to a name\n", EX_VEH_COUNT);
}

/* ---- flies() classification is self-consistent ---- */
static void test_flies(void) {
    // the documented flyers must fly; a clearly-grounded one must not
    assert(ex_vehicle_flies(EX_VEH_JET));
    assert(ex_vehicle_flies(EX_VEH_HOVER));
    assert(ex_vehicle_flies(EX_VEH_DRAGON));
    assert(ex_vehicle_flies(EX_VEH_GUNSHIP));
    assert(ex_vehicle_flies(EX_VEH_HAWK));
    assert(!ex_vehicle_flies(EX_VEH_TANK));
    assert(!ex_vehicle_flies(EX_VEH_DINO));
    assert(!ex_vehicle_flies(EX_VEH_SENTRY));
    // flies() must return a clean 0/1 for every type (no garbage)
    for (int t = 0; t < EX_VEH_COUNT; t++) {
        int f = ex_vehicle_flies((ex_vehicle_type)t);
        assert(f == 0 || f == 1);
    }
    printf("test_vehicle: flies() classification consistent\n");
}

/* ---- extra_parts is sane and matches leg-count intent ---- */
static void test_extra_parts(void) {
    for (int t = 0; t < EX_VEH_COUNT; t++) {
        int e = ex_vehicle_extra_parts((ex_vehicle_type)t);
        assert(e >= 0 && e <= 8);   // spider's 8 legs is the documented max
    }
    // the many-legged crawlers carry the most extra parts
    assert(ex_vehicle_extra_parts(EX_VEH_SPIDER) == 8);
    assert(ex_vehicle_extra_parts(EX_VEH_SCORPION) == 8);
    assert(ex_vehicle_extra_parts(EX_VEH_INSECT) == 6);
    // a tank (just treads) carries few
    assert(ex_vehicle_extra_parts(EX_VEH_TANK) <= 2);
    printf("test_vehicle: extra_parts sane (0..8, crawlers highest, tank lowest)\n");
}

/* ---- handling stats encode design intent ---- */
static void test_handling(void) {
    // every archetype: all three stats strictly positive, grip a valid 0..1 hold
    for (int t = 0; t < EX_VEH_COUNT; t++) {
        ex_vehicle_handling_t h = ex_vehicle_handling((ex_vehicle_type)t);
        assert(h.speed_mul > 0.0f && h.speed_mul < 3.0f);
        assert(h.accel > 0.0f && h.accel < 20.0f);
        assert(h.grip > 0.0f && h.grip <= 1.0f);
    }
    ex_vehicle_handling_t sentry = ex_vehicle_handling(EX_VEH_SENTRY);
    ex_vehicle_handling_t cycle  = ex_vehicle_handling(EX_VEH_CYCLE);
    ex_vehicle_handling_t tank   = ex_vehicle_handling(EX_VEH_TANK);
    ex_vehicle_handling_t raptor = ex_vehicle_handling(EX_VEH_RAPTOR);

    // the rooted siege gun is by far the slowest to accelerate and slowest top speed
    assert(sentry.speed_mul < tank.speed_mul);
    assert(sentry.accel < cycle.accel);
    // the nimble cycle out-accelerates and out-runs the heavy tank
    assert(cycle.speed_mul > tank.speed_mul);
    assert(cycle.accel > tank.accel);
    // fast bipeds (raptor/cycle) are quick off the line
    assert(raptor.accel >= 8.0f);
    // heavy walkers grip hard (locked), drifty racers grip loose
    assert(tank.grip >= ex_vehicle_handling(EX_VEH_CAR).grip);
    printf("test_vehicle: handling invariants (sentry rooted, cycle nimble, tank locked)\n");
}

/* ---- layout fills a valid, in-cap, archetype-varying part count ---- */
static void test_layout(void) {
    ex_part_pose_t poses[64];
    ex_build_params_t bp = ex_build_default();
    int n_car = 0, n_spider = 0;

    ex_vehicle_layout(poses, 64, &n_car, EX_VEH_CAR, &bp, 1.0f, 0.0f, 0.0f, 1, EX_BODY_PLAYER);
    assert(n_car > 0 && n_car <= 64);

    ex_vehicle_layout(poses, 64, &n_spider, EX_VEH_SPIDER, &bp, 1.0f, 0.0f, 0.0f, 1, EX_BODY_PLAYER);
    assert(n_spider > 0 && n_spider <= 64);

    // an 8-legged spider should place more parts than a wheeled car
    assert(n_spider > n_car);

    // parts carry non-degenerate scale (the layout actually sized them)
    ex_vehicle_layout(poses, 64, &n_car, EX_VEH_CAR, &bp, 1.0f, 0.0f, 0.0f, 1, EX_BODY_PLAYER);
    int sized = 0;
    for (int i = 0; i < n_car; i++)
        if (poses[i].sx > 0.0f || poses[i].sy > 0.0f || poses[i].sz > 0.0f) sized++;
    assert(sized == n_car);
    printf("test_vehicle: layout fills valid part counts (car=%d, spider=%d, all sized)\n",
           n_car, n_spider);
}

int main(void) {
    test_names();
    test_flies();
    test_extra_parts();
    test_handling();
    test_layout();
    printf("ALL ASSERTIONS PASSED\n");
    return 0;
}
