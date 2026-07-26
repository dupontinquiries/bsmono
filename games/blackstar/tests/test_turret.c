/* test_turret -- exercises the deployable auto-gun logic in turret.c.
 * Target pick (nearest in range), the fire-rate timer, and lifetime expiry
 * are all timer/geometry logic that drives the TURRET ability every match. */
#include "turret.h"
#include "combat.h"   /* ex_target_t */
#include <stdio.h>
#include <math.h>
#include <assert.h>

#define EPS 1e-3f
static int approx(float a, float b) { return fabsf(a - b) < EPS; }

/* ---- deploy sets sane defaults + lifts the muzzle ---- */
static void test_deploy(void) {
    ex_turret_t t;
    float pos[3] = {5, 0, 5};
    ex_turret_deploy(&t, pos, 8.0f);
    assert(t.active == 1);
    assert(t.life == 8.0f);
    assert(t.target_id == -1);
    assert(t.range > 0.0f && t.damage > 0.0f && t.cooldown > 0.0f);
    assert(approx(t.pos[0], 5.0f) && approx(t.pos[2], 5.0f));
    assert(approx(t.pos[1], 1.0f));   // muzzle raised +1 off the ground
    assert(t.fire_timer > 0.0f);      // initial spin-up delay before first shot
    printf("test_turret: deploy defaults + raised muzzle + spin-up delay OK\n");
}

/* ---- target pick: nearest alive target within range ---- */
static void test_pick(void) {
    float pos[3] = {0, 1, 0};
    ex_target_t tg[3] = {
        { {30, 0, 0}, 1.0f, 100, 1 },   // far
        { {10, 0, 0}, 1.0f, 200, 1 },   // near -- should win
        { { 5, 0, 0}, 1.0f, 300, 0 },   // nearest but DEAD -- skip
    };
    int idx = ex_turret_pick(tg, 3, pos, 60.0f);
    assert(idx == 1);   // the near live one

    // all out of range -> -1
    float far_pos[3] = {1000, 1, 1000};
    assert(ex_turret_pick(tg, 3, far_pos, 60.0f) == -1);

    // empty target list -> -1
    assert(ex_turret_pick(tg, 0, pos, 60.0f) == -1);

    // range boundary: a target just inside range is picked, just outside is not
    ex_target_t one[1] = { { {50, 0, 0}, 1.0f, 7, 1 } };
    assert(ex_turret_pick(one, 1, pos, 60.0f) == 0);   // 50 < 60
    assert(ex_turret_pick(one, 1, pos, 40.0f) == -1);  // 50 > 40
    printf("test_turret: pick nearest-alive-in-range (far/dead/empty/boundary) OK\n");
}

/* ---- update: spin-up, then fire-rate-gated shots ---- */
static void test_update_fires(void) {
    ex_turret_t t;
    float pos[3] = {0, 0, 0};
    ex_turret_deploy(&t, pos, 10.0f);
    ex_target_t tg[1] = { { {10, 0, 0}, 1.0f, 42, 1 } };

    // during spin-up (fire_timer 0.3) no shot yet
    assert(ex_turret_update(&t, 0.1f, tg, 1) == 0);
    assert(ex_turret_update(&t, 0.1f, tg, 1) == 0);
    // after spin-up elapses, the next update fires and acquires the target
    int fired = 0;
    for (int i = 0; i < 5 && !fired; i++)
        fired = ex_turret_update(&t, 0.1f, tg, 1);
    assert(fired == 1);
    assert(t.target_id == 42);

    // immediately after firing, it's cooling -> no back-to-back shot
    assert(ex_turret_update(&t, 0.01f, tg, 1) == 0);

    // after the cooldown elapses, it fires again
    fired = 0;
    for (int i = 0; i < 10 && !fired; i++)
        fired = ex_turret_update(&t, 0.1f, tg, 1);
    assert(fired == 1);
    printf("test_turret: update fires after spin-up, then fire-rate gated\n");
}

/* ---- update: no targets -> no fire, target_id cleared ---- */
static void test_update_no_target(void) {
    ex_turret_t t;
    float pos[3] = {0, 0, 0};
    ex_turret_deploy(&t, pos, 10.0f);
    for (int i = 0; i < 5; i++) ex_turret_update(&t, 0.1f, NULL, 0);  // spin-up w/ no targets
    assert(ex_turret_update(&t, 0.1f, NULL, 0) == 0);
    assert(t.target_id == -1);
    printf("test_turret: no targets -> no fire, target cleared\n");
}

/* ---- lifetime expiry deactivates ---- */
static void test_expiry(void) {
    ex_turret_t t;
    float pos[3] = {0, 0, 0};
    ex_turret_deploy(&t, pos, 1.0f);   // 1 second life
    ex_target_t tg[1] = { { {10, 0, 0}, 1.0f, 42, 1 } };
    // run past its lifetime
    for (int i = 0; i < 15; i++) ex_turret_update(&t, 0.1f, tg, 1);  // 1.5s
    assert(t.active == 0);
    // an expired turret never fires again
    assert(ex_turret_update(&t, 0.1f, tg, 1) == 0);
    printf("test_turret: lifetime expiry deactivates the turret\n");
}

int main(void) {
    test_deploy();
    test_pick();
    test_update_fires();
    test_update_no_target();
    test_expiry();
    printf("ALL ASSERTIONS PASSED\n");
    return 0;
}
