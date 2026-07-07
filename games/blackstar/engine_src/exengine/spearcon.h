/* spearcon -- Cryotek's lieutenants' signature weapon (Shardclaw & Icerender).
 *
 * From the film script: sticky spears that cling where they land and count down,
 * with delayed detonation -- "ticking-bomb tension" that forces everyone to keep
 * moving. Stick to a spot, the fuse burns, then it blows.
 *
 * This module is the pure pool: plant a charge at a position with a fuse, tick
 * them all, and it reports which detonate this frame (so the game can blast +
 * damage there). No rendering, no globals -> testable.
 */
#ifndef EX_SPEARCON_H
#define EX_SPEARCON_H

#define EX_SPEARCON_MAX 16

typedef struct {
  int   active;
  float pos[3];
  float fuse;   /* total fuse seconds */
  float t;      /* elapsed */
} ex_spearcon_t;

typedef struct { ex_spearcon_t s[EX_SPEARCON_MAX]; } ex_spearcon_pool_t;

void ex_spearcon_reset(ex_spearcon_pool_t *p);

/* Plant a charge; returns its slot index, or -1 if the pool is full. */
int  ex_spearcon_plant(ex_spearcon_pool_t *p, const float pos[3], float fuse);

/* Advance all charges by dt. For each that detonates this frame, its position is
   appended to out_det (up to max_out). Returns the number that detonated. */
int  ex_spearcon_update(ex_spearcon_pool_t *p, float dt, float out_det[][3], int max_out);

/* Fuse progress 0..1 for a charge (for the shrinking/quickening telegraph). */
float ex_spearcon_progress(const ex_spearcon_t *s);

int  ex_spearcon_active_count(const ex_spearcon_pool_t *p);

#endif /* EX_SPEARCON_H */
