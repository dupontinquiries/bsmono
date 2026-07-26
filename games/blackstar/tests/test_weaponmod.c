/* test_weaponmod -- exercises the weapon addon stat math in weaponmod.c.
 * These are balance invariants: slot caps, multiplier stacking, mode
 * conversions, and the output clamps that keep any mod combo from producing
 * a degenerate weapon (0-cooldown machine gun, 200-degree spread, etc.). */
#include "weaponmod.h"
#include "weapon.h"
#include <stdio.h>
#include <math.h>
#include <assert.h>

#define EPS 1e-3f
static int approx(float a, float b) { return fabsf(a - b) < EPS; }

/* ---- slot management ---- */
static void test_slots(void) {
    ex_weapon_mods_t m;
    ex_weapon_mods_init(&m);
    assert(m.count == 0);
    assert(ex_weapon_mods_add(&m, EX_MOD_SCOPE) == 0);
    assert(ex_weapon_mods_add(&m, EX_MOD_STABILIZER) == 1);
    assert(ex_weapon_mods_add(&m, EX_MOD_EXT_MAG) == 2);
    // fourth add past the 3-slot cap must fail
    assert(ex_weapon_mods_add(&m, EX_MOD_AMPLIFIER) == -1);
    assert(m.count == EX_WEAPON_MOD_SLOTS);
    printf("test_weaponmod: slot cap enforced (3 slots, 4th rejected)\n");
}

/* ---- stat multipliers stack ---- */
static void test_multipliers(void) {
    ex_weapon_profile_t base = ex_weapon_profile(EX_FORM_METAL);

    // long barrel: +35% range, +12% damage, +10% cooldown
    ex_weapon_mods_t m; ex_weapon_mods_init(&m);
    ex_weapon_mods_add(&m, EX_MOD_LONG_BARREL);
    ex_weapon_profile_t p = ex_weapon_apply_mods(EX_FORM_METAL, &m);
    assert(approx(p.range, base.range * 1.35f));
    assert(approx(p.damage, base.damage * 1.12f));
    // cooldown may clamp; if under ceiling it should be 1.10x
    if (base.cooldown * 1.10f <= 3.0f) assert(approx(p.cooldown, base.cooldown * 1.10f));

    // two damage mods stack multiplicatively (amplifier 1.30 * energon 1.30)
    ex_weapon_mods_t m2; ex_weapon_mods_init(&m2);
    ex_weapon_mods_add(&m2, EX_MOD_AMPLIFIER);
    ex_weapon_mods_add(&m2, EX_MOD_ENERGON_CELL);
    ex_weapon_profile_t p2 = ex_weapon_apply_mods(EX_FORM_METAL, &m2);
    assert(approx(p2.damage, base.damage * 1.30f * 1.30f));
    printf("test_weaponmod: stat multipliers apply and stack multiplicatively\n");
}

/* ---- NULL mod set returns the base profile untouched ---- */
static void test_null(void) {
    ex_weapon_profile_t base = ex_weapon_profile(EX_FORM_FIRE);
    ex_weapon_profile_t p = ex_weapon_apply_mods(EX_FORM_FIRE, NULL);
    assert(approx(p.damage, base.damage) && approx(p.range, base.range));
    printf("test_weaponmod: NULL mod set == base profile\n");
}

/* ---- fire-mode conversions ---- */
static void test_conversions(void) {
    struct { ex_weapon_mod_id mod; ex_fire_mode_t expect; } cases[] = {
        { EX_MOD_PIERCE_CORE,   EX_FIRE_PIERCE },
        { EX_MOD_RIPPLE_COIL,   EX_FIRE_RICOCHET },
        { EX_MOD_FLAK_CORE,     EX_FIRE_FLAK },
        { EX_MOD_PLASMA_CORE,   EX_FIRE_PLASMA },
        { EX_MOD_SUPPRESS_CORE, EX_FIRE_SUPPRESS },
    };
    for (int i = 0; i < 5; i++) {
        ex_weapon_mods_t m; ex_weapon_mods_init(&m);
        ex_weapon_mods_add(&m, cases[i].mod);
        ex_weapon_profile_t p = ex_weapon_apply_mods(EX_FORM_METAL, &m);
        assert(p.mode == cases[i].expect);
    }
    printf("test_weaponmod: mode conversions (pierce/ricochet/flak/plasma/suppress) OK\n");
}

/* ---- BOLT -> BURST promotion when a pellet-adding mod is applied ---- */
static void test_bolt_promotion(void) {
    // find a form whose base mode is BOLT
    int found = 0;
    for (int f = 0; f < EX_FORM_COUNT; f++) {
        ex_weapon_profile_t base = ex_weapon_profile((ex_watch_form_t)f);
        if (base.mode != EX_FIRE_BOLT) continue;
        found = 1;
        ex_weapon_mods_t m; ex_weapon_mods_init(&m);
        ex_weapon_mods_add(&m, EX_MOD_UNDERBARREL);
        ex_weapon_profile_t p = ex_weapon_apply_mods((ex_watch_form_t)f, &m);
        assert(p.mode == EX_FIRE_BURST);        // BOLT was promoted
        assert(p.pellets >= base.pellets + 1);  // and gained a pellet
        break;
    }
    printf("test_weaponmod: BOLT->BURST promotion on pellet mod (%s)\n",
           found ? "verified" : "no BOLT form -- skipped");
}

/* ---- output clamps: no mod stack can produce a degenerate weapon ---- */
static void test_clamps(void) {
    // stack three cooldown-reducers to try to drive cooldown below the 0.08 floor
    ex_weapon_mods_t m; ex_weapon_mods_init(&m);
    ex_weapon_mods_add(&m, EX_MOD_EXT_MAG);
    ex_weapon_mods_add(&m, EX_MOD_RAPID_COIL);
    ex_weapon_mods_add(&m, EX_MOD_EXT_MAG);
    ex_weapon_profile_t p = ex_weapon_apply_mods(EX_FORM_METAL, &m);
    assert(p.cooldown >= 0.08f - EPS);
    assert(p.spread_deg >= 0.0f && p.spread_deg <= 40.0f + EPS);
    assert(p.pellets >= 1);
    assert(p.slow >= 0.0f && p.slow <= 0.95f + EPS);

    // stack spread-adders to try to blow past the 40-degree ceiling
    ex_weapon_mods_t m2; ex_weapon_mods_init(&m2);
    ex_weapon_mods_add(&m2, EX_MOD_SCATTER_BARREL);
    ex_weapon_mods_add(&m2, EX_MOD_UNDERBARREL);
    ex_weapon_mods_add(&m2, EX_MOD_SCATTER_BARREL);
    ex_weapon_profile_t p2 = ex_weapon_apply_mods(EX_FORM_METAL, &m2);
    assert(p2.spread_deg <= 40.0f + EPS);

    // amplifier stack to try to overrun the cooldown ceiling of 3.0
    ex_weapon_mods_t m3; ex_weapon_mods_init(&m3);
    ex_weapon_mods_add(&m3, EX_MOD_AMPLIFIER);
    ex_weapon_mods_add(&m3, EX_MOD_PLASMA_CORE);
    ex_weapon_mods_add(&m3, EX_MOD_AMPLIFIER);
    ex_weapon_profile_t p3 = ex_weapon_apply_mods(EX_FORM_METAL, &m3);
    assert(p3.cooldown <= 3.0f + EPS);
    printf("test_weaponmod: output clamps hold (cooldown 0.08..3.0, spread 0..40, pellets>=1, slow<=0.95)\n");
}

/* ---- every mod id has a name (no crash / no 'none' for real ids) ---- */
static void test_names(void) {
    for (int id = EX_MOD_LONG_BARREL; id < EX_MOD_COUNT; id++) {
        const char *n = ex_weapon_mod_name((ex_weapon_mod_id)id);
        assert(n && n[0]);
    }
    printf("test_weaponmod: all mod ids resolve to a name\n");
}

int main(void) {
    test_slots();
    test_multipliers();
    test_null();
    test_conversions();
    test_bolt_promotion();
    test_clamps();
    test_names();
    printf("ALL ASSERTIONS PASSED\n");
    return 0;
}
