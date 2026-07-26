#include "hive_ai.h"
#include "enemy.h"     /* for EX_ENEMY_CORRUPTED */
#include <string.h>
#include <math.h>

const char* ex_hive_state_name(ex_hive_state s) {
  switch (s) {
    case EX_HIVE_CRAWL: return "crawl";
    case EX_HIVE_DANCE: return "dance";
    case EX_HIVE_FEED:  return "feed";
    default:            return "crawl";
  }
}

/* ---- strains: distinct sneaky styles as doctrine presets ---- */
const char* ex_hive_strain_name(ex_hive_strain s) {
  switch (s) {
    case EX_STRAIN_STALKER: return "stalker";
    case EX_STRAIN_CREEPER: return "creeper";
    case EX_STRAIN_LURE:    return "lure";
    case EX_STRAIN_RAVENER: return "ravener";
    default:                return "stalker";
  }
}
ex_hive_strain ex_hive_strain_from_name(const char *s) {
  if (!s) return EX_STRAIN_STALKER;
  for (int i = 0; i < EX_STRAIN_COUNT; i++)
    if (strcmp(s, ex_hive_strain_name((ex_hive_strain)i)) == 0) return (ex_hive_strain)i;
  return EX_STRAIN_STALKER;
}
ex_hive_doctrine_t ex_hive_doctrine_for(ex_hive_strain s) {
  /* fields: { feed_mass, dance_radius, recruit_radius, patience } */
  switch (s) {
    /* STALKER -- patient ambusher: lurks long (high patience -> small dance
       gate, so it only dances when a scout is nearly on top of prey), tight
       recruit radius (a compact pack), moderate mass, sudden feed. */
    case EX_STRAIN_STALKER: return (ex_hive_doctrine_t){ 4, 20.0f, 11.0f, 0.85f };
    /* CREEPER -- slow encircler: dances from farther out (wide dance radius,
       low patience gate) and waits for a BIG mass spread over a large recruit
       radius before committing. Surrounds before it feeds. */
    case EX_STRAIN_CREEPER: return (ex_hive_doctrine_t){ 6, 30.0f, 24.0f, 0.25f };
    /* LURE -- a few dance EARLY and from range to draw prey toward the pack;
       small feed mass so once the prey's close the rest tip fast. */
    case EX_STRAIN_LURE:    return (ex_hive_doctrine_t){ 3, 34.0f, 18.0f, 0.15f };
    /* RAVENER -- impatient feeders: short lurk, small mass, quick to swarm --
       barely sneaky, they can't help themselves. */
    case EX_STRAIN_RAVENER: return (ex_hive_doctrine_t){ 2, 24.0f, 14.0f, 0.10f };
    default:                return ex_hive_default();
  }
}
void ex_hive_init_strain(ex_hive_t *h, ex_hive_strain strain) {
  if (!h) return;
  ex_hive_doctrine_t d = ex_hive_doctrine_for(strain);
  ex_hive_init(h, &d);
  h->strain = strain;
}

ex_hive_doctrine_t ex_hive_default(void) {
  ex_hive_doctrine_t d;
  d.feed_mass      = 4;      /* 4 recruited near a dance -> swarm feeds */
  d.dance_radius   = 22.0f;  /* a scout within 22u of prey starts dancing */
  d.recruit_radius = 16.0f;  /* infected within 16u of the dance get recruited */
  d.patience       = 0.6f;   /* fairly patient -- crawls a while first */
  return d;
}

void ex_hive_init(ex_hive_t *h, const ex_hive_doctrine_t *doc) {
  if (!h) return;
  memset(h, 0, sizeof(*h));
  h->doctrine = doc ? *doc : ex_hive_default();
  h->state = EX_HIVE_CRAWL;
}

static float d2xz(const float a[3], const float b[3]) {
  float dx = a[0]-b[0], dz = a[2]-b[2];
  return dx*dx + dz*dz;
}

/* survey infected in the snapshot: the scout nearest the player, and the swarm
 * size. */
typedef struct {
  int   n_infected;       /* living infected */
  int   scout;            /* snapshot unit index of the nearest-to-player infected, -1 */
  float scout_d2;         /* its squared xz distance to the player */
} hive_survey;

static hive_survey survey_hive(const ex_cj_snapshot_t *snap) {
  hive_survey s; s.n_infected = 0; s.scout = -1; s.scout_d2 = 1e30f;
  for (int i = 0; i < snap->n_units; i++) {
    const ex_cj_unit_t *u = &snap->units[i];
    if (!u->alive || u->kind != EX_ENEMY_CORRUPTED) continue;
    s.n_infected++;
    float dd = d2xz(u->pos, snap->player_pos);
    if (dd < s.scout_d2) { s.scout_d2 = dd; s.scout = i; }
  }
  return s;
}

/* count infected within recruit_radius of a point (the dance recruits them). */
static int count_recruited(const ex_cj_snapshot_t *snap, const float pt[3], float r) {
  float r2 = r*r; int c = 0;
  for (int i = 0; i < snap->n_units; i++) {
    const ex_cj_unit_t *u = &snap->units[i];
    if (!u->alive || u->kind != EX_ENEMY_CORRUPTED) continue;
    if (d2xz(u->pos, pt) <= r2) c++;
  }
  return c;
}

int ex_hive_think(ex_hive_t *h, const ex_cj_snapshot_t *snap,
                  float dt, ex_cj_command_t *out, int max_cmds) {
  if (!h || !snap || !out || max_cmds < 1) return 0;
  h->state_t += dt;
  h->think_cd -= dt;
  if (h->think_cd > 0.0f) return 0;
  h->think_cd = 0.5f;

  hive_survey sv = survey_hive(snap);
  if (sv.n_infected <= 0 || sv.scout < 0) { h->state = EX_HIVE_CRAWL; return 0; }

  const ex_hive_doctrine_t *d = &h->doctrine;
  float scout_d = sqrtf(sv.scout_d2);
  const float *scout_pos = snap->units[sv.scout].pos;

  /* --- state transitions --- */
  /* a scout close enough to prey starts the waggle dance (recruiting). The
   * patience gate makes the hive hold off until a scout is well inside range,
   * so it doesn't tip its hand early. */
  float dance_gate = d->dance_radius * (1.0f - 0.4f * d->patience);
  if (h->state == EX_HIVE_CRAWL) {
    if (scout_d <= dance_gate) {
      h->state = EX_HIVE_DANCE; h->state_t = 0.0f;
      h->dance_point[0]=scout_pos[0]; h->dance_point[1]=scout_pos[1]; h->dance_point[2]=scout_pos[2];
    }
  }
  if (h->state == EX_HIVE_DANCE) {
    /* the dance follows the scout; recruits accumulate near it. */
    h->dance_point[0]=scout_pos[0]; h->dance_point[1]=scout_pos[1]; h->dance_point[2]=scout_pos[2];
    h->recruited = count_recruited(snap, h->dance_point, d->recruit_radius);
    if (h->recruited >= d->feed_mass) { h->state = EX_HIVE_FEED; h->state_t = 0.0f; }
    else if (scout_d > d->dance_radius * 1.5f) {
      /* prey slipped away -> back to crawling quietly */
      h->state = EX_HIVE_CRAWL; h->state_t = 0.0f;
    }
  }
  if (h->state == EX_HIVE_FEED) {
    /* if the swarm has been thinned below mass, it disperses back to crawling. */
    if (sv.n_infected < d->feed_mass) { h->state = EX_HIVE_CRAWL; h->state_t = 0.0f; }
  }

  /* --- emit the command for the state --- */
  ex_cj_command_t *c = &out[0];
  memset(c, 0, sizeof(*c));
  c->zone = -1;
  switch (h->state) {
    case EX_HIVE_CRAWL:
      /* stay dispersed + creep: a loose hold. Deliberately quiet -- no rush. */
      c->type = EX_CJ_CMD_HOLD;
      c->point[0]=snap->player_pos[0]; c->point[2]=snap->player_pos[2];
      c->param0 = d->patience;                 /* hint: how cautiously to creep */
      break;
    case EX_HIVE_DANCE:
      /* recruit toward the dance point (the scout on the prey). */
      c->type = EX_CJ_CMD_FLOCK_TO;
      c->point[0]=h->dance_point[0]; c->point[1]=h->dance_point[1]; c->point[2]=h->dance_point[2];
      c->param0 = 8.0f;                         /* gather radius */
      c->param1 = (float)h->recruited;          /* how many have answered */
      break;
    case EX_HIVE_FEED:
    default:
      /* critical mass -> converge and feed on the player. */
      c->type = EX_CJ_CMD_FOCUS;
      c->point[0]=snap->player_pos[0]; c->point[2]=snap->player_pos[2];
      c->param0 = 1.0f;                         /* full commitment */
      break;
  }
  return 1;
}
