/* test_hive_ai -- the sneaky infected swarm engine.
 * Guards the crawl -> waggle-dance -> feed progression and stigmergic
 * recruitment, all expressed through the shared combat_json command vocabulary. */
#include "hive_ai.h"
#include "combat_json.h"
#include "enemy.h"
#include <stdio.h>
#include <string.h>
#include <math.h>
#include <assert.h>

/* build a snapshot with n infected clustered near a center point. */
static void make_hive_snap(ex_cj_snapshot_t *s, int n, float cx, float cz, float spread) {
  memset(s, 0, sizeof(s[0]));
  s->tick = 1;
  s->player_pos[0] = 0; s->player_pos[2] = 0;
  for (int i = 0; i < n; i++) {
    ex_cj_unit_t *u = &s->units[s->n_units++];
    u->id = 100 + i;
    u->kind = EX_ENEMY_CORRUPTED;
    u->alive = 1;
    u->hp = 90; u->max_hp = 90;
    /* clustered around (cx,cz) with a little spread */
    float a = (float)i / (float)(n>0?n:1) * 6.28318f;
    u->pos[0] = cx + cosf(a)*spread;
    u->pos[2] = cz + sinf(a)*spread;
  }
}

static int decide(ex_hive_t *h, const ex_cj_snapshot_t *s, ex_cj_command_t *out) {
  return ex_hive_think(h, s, 10.0f, out, 4);   /* big dt skips the cadence gate */
}

/* infected FAR from the player -> crawl quietly (hold), don't charge. */
static void test_crawls_when_far(void) {
  ex_cj_snapshot_t s;
  ex_cj_command_t cmd[4];
  make_hive_snap(&s, 6, 80.0f, 0.0f, 5.0f);   /* 80u away */
  ex_hive_t h; ex_hive_init(&h, NULL);
  int n = decide(&h, &s, cmd);
  assert(n == 1 && cmd[0].type == EX_CJ_CMD_HOLD);
  assert(h.state == EX_HIVE_CRAWL);
  printf("test_hive_ai: far swarm crawls (%s)\n", ex_hive_state_name(h.state));
}

/* a scout gets close -> the hive starts dancing (recruit toward the scout). */
static void test_scout_dances(void) {
  ex_cj_snapshot_t s;
  ex_cj_command_t cmd[4];
  /* small swarm close in, but below feed_mass so it can't feed yet: it should
     DANCE (recruit), not immediately feed. */
  make_hive_snap(&s, 2, 10.0f, 0.0f, 3.0f);   /* close, only 2 infected */
  ex_hive_t h; ex_hive_init(&h, NULL);
  int n = decide(&h, &s, cmd);
  assert(n == 1);
  assert(h.state == EX_HIVE_DANCE);
  assert(cmd[0].type == EX_CJ_CMD_FLOCK_TO);   /* recruiting toward the dance */
  printf("test_hive_ai: close scout -> dance (%s), recruited=%d\n",
         ex_cj_cmd_name(cmd[0].type), h.recruited);
}

/* enough infected massed near the prey -> tip to FEED. */
static void test_critical_mass_feeds(void) {
  ex_cj_snapshot_t s;
  ex_cj_command_t cmd[4];
  /* 6 infected all clustered close to the player: scout is in range AND enough
     are within recruit radius -> should reach FEED. */
  make_hive_snap(&s, 6, 8.0f, 0.0f, 4.0f);
  ex_hive_t h; ex_hive_init(&h, NULL);
  /* first think enters DANCE and counts recruits; a second (past cadence) tips
     to FEED once recruited >= feed_mass. */
  decide(&h, &s, cmd);
  h.think_cd = 0.0f;
  int n = decide(&h, &s, cmd);
  assert(n == 1);
  assert(h.state == EX_HIVE_FEED);
  assert(cmd[0].type == EX_CJ_CMD_FOCUS);
  printf("test_hive_ai: critical mass -> feed (%s)\n", ex_cj_cmd_name(cmd[0].type));
}

/* if the swarm is thinned below mass while feeding, it disperses back to crawl. */
static void test_thinned_disperses(void) {
  ex_cj_snapshot_t s;
  ex_cj_command_t cmd[4];
  make_hive_snap(&s, 6, 8.0f, 0.0f, 4.0f);
  ex_hive_t h; ex_hive_init(&h, NULL);
  decide(&h, &s, cmd); h.think_cd = 0.0f;
  decide(&h, &s, cmd);                       /* now FEED */
  assert(h.state == EX_HIVE_FEED);
  /* kill most of the swarm -> below feed_mass */
  for (int i = 0; i < 5; i++) s.units[i].alive = 0;
  h.think_cd = 0.0f;
  decide(&h, &s, cmd);
  assert(h.state == EX_HIVE_CRAWL);
  printf("test_hive_ai: thinned swarm disperses back to crawl\n");
}

static void test_cadence(void) {
  ex_cj_snapshot_t s;
  ex_cj_command_t cmd[4];
  make_hive_snap(&s, 3, 80.0f, 0.0f, 5.0f);
  ex_hive_t h; ex_hive_init(&h, NULL);
  int n1 = ex_hive_think(&h, &s, 10.0f, cmd, 4);
  int n2 = ex_hive_think(&h, &s, 0.001f, cmd, 4);
  assert(n1 == 1 && n2 == 0);
  printf("test_hive_ai: think cadence gates decisions\n");
}

/* ---- strains are behaviorally distinct ---- */
static void test_strains_differ(void) {
  ex_cj_snapshot_t s;
  ex_cj_command_t cmd[4];
  /* a mid-size swarm (3) clustered close to the player. */
  make_hive_snap(&s, 3, 9.0f, 0.0f, 4.0f);

  /* RAVENER: feed_mass 2 -> 3 clustered infected should tip it to FEED. */
  ex_hive_t rav; ex_hive_init_strain(&rav, EX_STRAIN_RAVENER);
  decide(&rav, &s, cmd); rav.think_cd = 0.0f;
  int nr = decide(&rav, &s, cmd);
  assert(nr == 1 && rav.state == EX_HIVE_FEED);

  /* CREEPER: feed_mass 6 -> the same 3 infected are NOT enough; it keeps
     dancing/recruiting, not feeding. */
  ex_hive_t crp; ex_hive_init_strain(&crp, EX_STRAIN_CREEPER);
  decide(&crp, &s, cmd); crp.think_cd = 0.0f;
  int nc = decide(&crp, &s, cmd);
  assert(nc == 1 && crp.state != EX_HIVE_FEED);   /* still massing */
  printf("test_hive_ai: ravener feeds where creeper still recruits (%s vs %s)\n",
         ex_hive_state_name(rav.state), ex_hive_state_name(crp.state));

  /* strain names round-trip */
  for (int i = 0; i < EX_STRAIN_COUNT; i++) {
    const char *nm = ex_hive_strain_name((ex_hive_strain)i);
    assert(nm && nm[0]);
    assert(ex_hive_strain_from_name(nm) == (ex_hive_strain)i);
  }

  /* LURE dances from farther out than STALKER: place a scout at ~26u -- inside
     the lure's dance gate but outside the patient stalker's. */
  make_hive_snap(&s, 2, 26.0f, 0.0f, 3.0f);
  ex_hive_t lure; ex_hive_init_strain(&lure, EX_STRAIN_LURE);
  ex_hive_t stk;  ex_hive_init_strain(&stk,  EX_STRAIN_STALKER);
  decide(&lure, &s, cmd);
  decide(&stk,  &s, cmd);
  assert(lure.state == EX_HIVE_DANCE);   /* the lure dances early to draw prey */
  assert(stk.state  == EX_HIVE_CRAWL);   /* the stalker still lurks, unseen */
  printf("test_hive_ai: at 26u lure dances, stalker still lurks\n");
}

int main(void) {
  test_crawls_when_far();
  test_scout_dances();
  test_critical_mass_feeds();
  test_thinned_disperses();
  test_cadence();
  test_strains_differ();
  printf("ALL ASSERTIONS PASSED\n");
  return 0;
}
