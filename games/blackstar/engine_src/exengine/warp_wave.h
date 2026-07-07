/* warp_wave -- the Ice-Clone quantum leap's signature CONSEQUENCE.
 *
 * From the film script: the jump itself is silent and undetectable -- one moment
 * the bearer isn't there, then they are. The only evidence anything happened is
 * a gravity wave that arrives SECONDS LATER at the landing point. The jumper
 * knows it is coming and can warn allies, but cannot stop or shape it, and it is
 * indifferent to who is standing where it lands.
 *
 * This module is just the pure scheduler: arm a wave at a position with a delay,
 * tick it each frame, and it reports the single frame it fires. No rendering, no
 * game state -- so it is trivially testable.
 */
#ifndef EX_WARP_WAVE_H
#define EX_WARP_WAVE_H

typedef struct {
  int   armed;     /* 1 while a wave is inbound */
  float timer;     /* seconds remaining until it fires */
  float delay;     /* the total delay it was armed with (for telegraph progress) */
  float pos[3];    /* where the wave will land (the leap's destination) */
} ex_warp_wave_t;

/* Arm a wave at `pos`, firing `delay` seconds from now. Overwrites any pending
   wave (a new leap resets the clock). */
void ex_warp_wave_arm(ex_warp_wave_t *w, const float pos[3], float delay);

/* Advance by dt. Returns 1 on the SINGLE frame the wave fires (and disarms);
   0 otherwise. Safe to call when unarmed. */
int ex_warp_wave_tick(ex_warp_wave_t *w, float dt);

/* Telegraph progress 0..1: 0 the instant it's armed, 1 as it fires. 0 if unarmed.
   Use it to grow/tighten the warning ring as the wave closes in. */
float ex_warp_wave_progress(const ex_warp_wave_t *w);

#endif /* EX_WARP_WAVE_H */
