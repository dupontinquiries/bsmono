/* truce -- Megatron's truce.
 *
 * From the film script: Megatron returns changed, and when the devourer proves it
 * consumes even Decepticons, he does the unthinkable -- proposes a truce. In-game
 * that beat fires when a Unicron-tier boss is present and hurt enough that the
 * common threat is undeniable: the Decepticon grunts turn, stop hunting you, and
 * charge the devourer alongside you.
 *
 * This module is the pure trigger decision. No globals, no rendering -> testable.
 */
#ifndef EX_TRUCE_H
#define EX_TRUCE_H

#define EX_TRUCE_HP_TRIGGER 0.7f   /* boss must be worn below this for the truce */

/* 1 when the truce should be active: a boss is present AND its health fraction is
   at or below the trigger. Once ignited the caller latches it (a truce doesn't
   un-happen if the boss is later healed). */
int ex_truce_active(int boss_present, float boss_hp_frac);

#endif /* EX_TRUCE_H */
