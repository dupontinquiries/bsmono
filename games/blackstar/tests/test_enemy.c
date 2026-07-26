/* test_enemy -- the enemy AI state machine + engagement ranges.
 * Notably guards the range fix: grunts/hijacks now detect and open fire from
 * real distance (not a tiny bubble the player could poke from just outside),
 * so ranged enemies actually shoot back. */
#include "enemy.h"
#include "coll_chunk.h"
#include <stdio.h>
#include <math.h>
#include <assert.h>

/* run the AI a few ticks with no LOS-blocking geometry */
static void tick(ex_enemy_t *e, vec3 player, int n) {
    for (int i = 0; i < n; i++) ex_enemy_update(e, NULL, player, 1.0f/60.0f);
}

/* ---- engagement ranges: the fix ---- */
static void test_ranges(void) {
    // a grunt (default ranged kind) must NOTICE and be able to ATTACK from well
    // beyond the old 22u attack bubble -- otherwise you poke it from outside freely.
    ex_enemy_t g = ex_enemy_new_kind(1, (vec3){0,0,0}, 100.0f, EX_ENEMY_GRUNT, EX_ELEM_FIRE);
    assert(g.sight_range >= 100.0f);      // notices from real distance
    assert(g.attack_range >= 60.0f);      // opens fire at range, not point blank

    // place the player at 60u -- inside the new attack range. The grunt should
    // wake (CHASE) and reach ATTACK, wanting to fire, WITHOUT the player closing.
    vec3 player = {60.0f, 0, 0};
    g.state = EX_ENEMY_IDLE;
    tick(&g, player, 10);
    assert(g.state == EX_ENEMY_CHASE || g.state == EX_ENEMY_ATTACK);
    // let it settle/fire
    tick(&g, player, 120);
    assert(g.state == EX_ENEMY_ATTACK || g.wants_to_fire);
    printf("test_enemy: grunt engages from 60u (sight %.0f attack %.0f)\n",
           g.sight_range, g.attack_range);

    // hijack swarm also engages from range now
    ex_enemy_t h = ex_enemy_new_kind(2, (vec3){0,0,0}, 45.0f, EX_ENEMY_HIJACK, EX_ELEM_FIRE);
    assert(h.attack_range >= 50.0f);
    printf("test_enemy: hijack attack range %.0f (was 26)\n", h.attack_range);
}

/* ---- idle when the target is out of sight range ---- */
static void test_out_of_range(void) {
    ex_enemy_t g = ex_enemy_new_kind(3, (vec3){0,0,0}, 100.0f, EX_ENEMY_GRUNT, EX_ELEM_FIRE);
    // player way beyond sight range -> stays idle
    vec3 far = {5000.0f, 0, 0};
    tick(&g, far, 30);
    assert(g.state == EX_ENEMY_IDLE);
    printf("test_enemy: stays idle beyond sight range\n");
}

/* ---- the IDLE->CHASE->ATTACK->DEAD lifecycle ---- */
static void test_lifecycle(void) {
    ex_enemy_t g = ex_enemy_new_kind(4, (vec3){50,0,0}, 100.0f, EX_ENEMY_GRUNT, EX_ELEM_FIRE);
    assert(g.state == EX_ENEMY_IDLE);
    vec3 player = {0,0,0};
    tick(&g, player, 5);
    assert(g.state != EX_ENEMY_IDLE);         // has noticed the player
    // kill it
    int died = ex_enemy_damage(&g, 9999.0f);
    assert(died == 1 && g.state == EX_ENEMY_DEAD);
    // a dead enemy ignores updates (stays dead, doesn't move)
    float px = g.position[0];
    tick(&g, player, 10);
    assert(g.state == EX_ENEMY_DEAD);
    assert(fabsf(g.position[0]-px) < 1e-4f);
    printf("test_enemy: idle->chase->dead lifecycle OK\n");
}

/* ---- corrupted/apex hunt from anywhere (spark resonance) ---- */
static void test_hunters(void) {
    ex_enemy_t c = ex_enemy_new_kind(5, (vec3){0,0,0}, 90.0f, EX_ENEMY_CORRUPTED, EX_ELEM_VINE);
    assert(c.sight_range > 1e6f);             // notices from anywhere
    assert(c.attack_range < 10.0f);           // but melee-only
    // even from very far, it wakes and starts closing
    vec3 player = {400.0f, 0, 0};
    tick(&c, player, 5);
    assert(c.state != EX_ENEMY_IDLE);
    float d0 = c.position[0];
    tick(&c, player, 60);
    assert(c.position[0] > d0);               // it advanced toward the player
    printf("test_enemy: corrupted hunts + closes from any range\n");
}

int main(void) {
    test_ranges();
    test_out_of_range();
    test_lifecycle();
    test_hunters();
    printf("ALL ASSERTIONS PASSED\n");
    return 0;
}
