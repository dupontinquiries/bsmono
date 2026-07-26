/* behavior.h -- Markov behaviour FSM for enemy AI.
 *
 * Enemies run a Markov chain over EX_BEH_* behaviours: each decision tick they sample
 * the next behaviour from a transition matrix picked by (game context, vehicle
 * archetype). The matrices are TUNED offline by tools/tune_fsm.py (headless combat
 * simulation + evolutionary search per context/archetype) and generated into
 * behavior_gen.h. This header exposes the enums, the tuned tables, and the runtime
 * accessors the AI calls. */
#ifndef EX_BEHAVIOR_H
#define EX_BEHAVIOR_H

#include <stdint.h>
#include "behavior_gen.h"   /* ex_behavior_t, ex_beh_ctx_t, ex_beh_arch_t + tuned tables */

/* Map a live game mode (+ campaign mission) to a tuned context. */
ex_beh_ctx_t  ex_behavior_ctx(int mode, int in_campaign, int mission_index);

/* Map a vehicle type (ex_vehicle_type order) to its behaviour archetype. */
ex_beh_arch_t ex_behavior_arch(int vehicle_type);

/* Sample the next behaviour from `cur` via the tuned matrix for (ctx, arch).
 * rng is an xorshift state the caller keeps per-enemy (seed it non-zero). */
int ex_behavior_step(ex_beh_ctx_t ctx, ex_beh_arch_t arch, int cur, uint32_t *rng);

/* Human-readable behaviour name (for HUD/debug). */
const char *ex_behavior_name(int b);

#endif /* EX_BEHAVIOR_H */
