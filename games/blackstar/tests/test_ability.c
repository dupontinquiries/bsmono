/* test_ability -- exercises the ability cooldown/duration state machine in
 * ability.c. The core lifecycle (ready -> trigger -> active -> cooldown ->
 * ready) drives every player ability; verify it advances correctly over dt. */
#include "ability.h"
#include <stdio.h>
#include <math.h>
#include <assert.h>

/* ---- slot management ---- */
static void test_slots(void) {
    ex_ability_loadout_t l;
    ex_loadout_init(&l);
    assert(l.count == 0);
    assert(ex_loadout_add(&l, EX_ABIL_DASH) == 0);
    assert(ex_loadout_add(&l, EX_ABIL_SHIELD) == 1);
    assert(ex_loadout_add(&l, EX_ABIL_CLOAK) == 2);
    assert(ex_loadout_add(&l, EX_ABIL_HOVER) == -1);  // past 3-slot cap
    assert(l.count == EX_ABILITY_SLOTS);
    assert(ex_loadout_has(&l, EX_ABIL_SHIELD));
    assert(!ex_loadout_has(&l, EX_ABIL_HOVER));
    printf("test_ability: slot cap + loadout_has OK\n");
}

/* ---- defaults are sane ---- */
static void test_defaults(void) {
    ex_ability_t a;
    ex_ability_defaults(&a, EX_ABIL_DASH);
    assert(a.id == EX_ABIL_DASH);
    assert(a.cooldown > 0.0f && a.duration > 0.0f);
    assert(a.cd_timer == 0.0f && a.active_timer == 0.0f);   // starts ready
    ex_ability_defaults(&a, EX_ABIL_RESONANCE);
    assert(a.duration == 0.0f);   // instant burst has no active window
    printf("test_ability: defaults sane (dash sustained, resonance instant)\n");
}

/* ---- the full lifecycle of a SUSTAINED ability (shield) ---- */
static void test_sustained_lifecycle(void) {
    ex_ability_loadout_t l; ex_loadout_init(&l);
    ex_loadout_add(&l, EX_ABIL_SHIELD);  // cooldown 10, duration 5
    int s = 0;

    assert(ex_ability_ready(&l, s));            // starts ready
    assert(!ex_ability_active(&l, s));

    assert(ex_ability_trigger(&l, s) == 1);     // fire
    assert(ex_ability_active(&l, s));           // now active
    assert(!ex_ability_ready(&l, s));           // and not ready

    // cannot re-trigger while active/cooling
    assert(ex_ability_trigger(&l, s) == 0);

    // advance just past the 5s active window. NOTE: 50 exact 0.1f steps leaves
    // the timer at ~+2.5e-6 (float accumulation), so step a hair further --
    // real gameplay uses variable dt and never lands exactly on the boundary.
    for (int i = 0; i < 51; i++) ex_loadout_update(&l, 0.1f);  // ~5.1s
    assert(!ex_ability_active(&l, s));          // active window closed
    assert(!ex_ability_ready(&l, s));           // still on cooldown (cd = 10+5)

    // advance the rest of the cooldown (total cd_timer 15s, ~5.1 elapsed)
    for (int i = 0; i < 100; i++) ex_loadout_update(&l, 0.1f); // +10.0s
    assert(ex_ability_ready(&l, s));            // ready again
    printf("test_ability: sustained lifecycle (ready->active->cooldown->ready) OK\n");
}

/* ---- an INSTANT ability (pulse, duration 0) never reports active ---- */
static void test_instant(void) {
    ex_ability_loadout_t l; ex_loadout_init(&l);
    ex_loadout_add(&l, EX_ABIL_PULSE);  // cooldown 6, duration 0
    assert(ex_ability_trigger(&l, 0) == 1);
    assert(!ex_ability_active(&l, 0));   // instant: no active window
    assert(!ex_ability_ready(&l, 0));    // but on cooldown
    for (int i = 0; i < 61; i++) ex_loadout_update(&l, 0.1f); // 6.1s
    assert(ex_ability_ready(&l, 0));
    printf("test_ability: instant ability (no active window, cooldown only) OK\n");
}

/* ---- active_any finds an active ability by id across slots ---- */
static void test_active_any(void) {
    ex_ability_loadout_t l; ex_loadout_init(&l);
    ex_loadout_add(&l, EX_ABIL_DASH);
    ex_loadout_add(&l, EX_ABIL_CLOAK);   // duration 6
    assert(!ex_ability_active_any(&l, EX_ABIL_CLOAK));
    ex_ability_trigger(&l, 1);
    assert(ex_ability_active_any(&l, EX_ABIL_CLOAK));
    assert(!ex_ability_active_any(&l, EX_ABIL_DASH));   // dash not triggered
    printf("test_ability: active_any locates an active ability by id\n");
}

/* ---- loadout_set can place into an arbitrary slot ---- */
static void test_set(void) {
    ex_ability_loadout_t l; ex_loadout_init(&l);
    ex_loadout_set(&l, 2, EX_ABIL_OVERCLOCK);
    assert(l.count == 3);
    assert(l.slots[2].id == EX_ABIL_OVERCLOCK);
    ex_loadout_set(&l, 99, EX_ABIL_DASH);   // out of range -> no-op, no crash
    assert(l.count == 3);
    printf("test_ability: loadout_set slot placement + bounds guard OK\n");
}

/* ---- names resolve ---- */
static void test_names(void) {
    for (int id = EX_ABIL_HOVER; id <= EX_ABIL_BLINK; id++)
        assert(ex_ability_name((ex_ability_id)id)[0]);
    printf("test_ability: all ability ids resolve to a name\n");
}

int main(void) {
    test_slots();
    test_defaults();
    test_sustained_lifecycle();
    test_instant();
    test_active_any();
    test_set();
    test_names();
    printf("ALL ASSERTIONS PASSED\n");
    return 0;
}
