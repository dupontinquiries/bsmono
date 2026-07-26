/* hive_ai -- a sneaky swarm decision engine for the infected (Corrupted).
 *
 * Deliberately DIFFERENT IN KIND from commander_ai's military doctrine FSMs.
 * A commander issues open, top-down squad orders. The hive does the opposite:
 * it is patient and indirect.
 *
 *   CRAWL & ORGANIZE : below critical mass, the infected stay dispersed and
 *                      creep -- massing quietly instead of charging, so a lone
 *                      scout doesn't give the swarm away.
 *   WAGGLE DANCE     : an infected close to prey emits a "dance" that RECRUITS
 *                      nearby infected toward that point. Excitement spreads
 *                      through proximity (stigmergic / bee-inspired), not a
 *                      broadcast order -- the swarm self-organizes.
 *   FEED             : once enough are recruited (critical mass), the hive tips
 *                      and converges to feed.
 *
 * Still emits the same ex_cj_command_t vocabulary as commander_ai + the future
 * LLM brains, so all "brains" are interchangeable behind one interface -- the
 * hive just reaches those commands by a sneakier policy.
 */
#ifndef EX_HIVE_AI_H
#define EX_HIVE_AI_H

#include "combat_json.h"

/* the hive's coarse collective posture. */
typedef enum {
  EX_HIVE_CRAWL = 0,   /* dispersed, creeping, massing quietly */
  EX_HIVE_DANCE,       /* a scout found prey -> recruiting nearby infected */
  EX_HIVE_FEED         /* critical mass reached -> converge and feed */
} ex_hive_state;

/* infected STRAINS -- distinct sneaky styles. All share the crawl/dance/feed
 * skeleton but differ in HOW they lurk and swarm. Each is a doctrine preset
 * (ex_hive_doctrine_for). Mirrors how commanders have archetypes. */
typedef enum {
  EX_STRAIN_STALKER = 0, /* patient ambusher: lurks long, tight pack, sudden feed */
  EX_STRAIN_CREEPER,     /* slow encircler: spreads wide, big mass, low patience gate */
  EX_STRAIN_LURE,        /* a few dance early + loud to draw prey, then the pack tips */
  EX_STRAIN_RAVENER,     /* impatient feeders: small mass, short lurk, quick to swarm */
  EX_STRAIN_COUNT
} ex_hive_strain;

/* tuning for a hive (defaults from ex_hive_default). */
typedef struct {
  int   feed_mass;      /* recruited count that tips CRAWL/DANCE -> FEED */
  float dance_radius;   /* how close a scout must be to prey to start dancing */
  float recruit_radius; /* how near an infected must be to a dance to be recruited */
  float patience;       /* 0..1 -- higher = crawls longer before daring to dance */
} ex_hive_doctrine_t;

/* strain <-> name + its doctrine preset */
const char*        ex_hive_strain_name(ex_hive_strain s);
ex_hive_strain     ex_hive_strain_from_name(const char *s);
ex_hive_doctrine_t ex_hive_doctrine_for(ex_hive_strain s);

/* the hive runtime brain (one per infected group / encounter). */
typedef struct {
  ex_hive_strain     strain;      /* which sneaky style (flavor/debug) */
  ex_hive_doctrine_t doctrine;
  ex_hive_state      state;
  float              think_cd;
  float              state_t;
  /* the current dance focus (a scout's position that's recruiting), valid while
   * state != CRAWL. */
  float              dance_point[3];
  int                recruited;     /* infected within recruit_radius of the dance */
} ex_hive_t;

ex_hive_doctrine_t ex_hive_default(void);
void ex_hive_init(ex_hive_t *h, const ex_hive_doctrine_t *doc /*nullable*/);
/* init a hive with a named strain's doctrine preset. */
void ex_hive_init_strain(ex_hive_t *h, ex_hive_strain strain);

/* One decision step. Reads the snapshot (infected = units of kind
 * EX_ENEMY_CORRUPTED), advances the swarm state, and writes up to max_cmds
 * commands into out. Returns the number written (0 = keep crawling silently).
 * Runs on a think cadence, so calling every frame is fine. */
int ex_hive_think(ex_hive_t *h, const ex_cj_snapshot_t *snap,
                  float dt, ex_cj_command_t *out, int max_cmds);

/* expose state name for logging/debug. */
const char* ex_hive_state_name(ex_hive_state s);

#endif /* EX_HIVE_AI_H */
