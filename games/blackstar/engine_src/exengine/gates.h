/* gates
  Buy-a-door map expansion for Escalation (Black-Ops-Zombies-style, but
  Cybertron): the run starts in a small arena, and each gate -- once you
  can afford it -- opens permanently and pushes the playable perimeter
  outward for the rest of the run, so the encounter has more room and the
  spawns range further. Pure logic; the game supplies the energon wallet
  (Escalation's ex_waves_t.energon) and applies the returned spawn extent
  to its own arena bounds.
*/
#ifndef EX_GATES_H
#define EX_GATES_H

#define EX_GATE_COUNT 3

typedef struct {
  long        cost;     // energon required to open
  float       extent;   // spawn half-extent once open (replaces the current)
  int         opened;
  const char *name;
} ex_gate_t;

/**
 * [ex_gates_init set up the escalating causeway/wing/outer-rim gate chain]
 */
void ex_gates_init(ex_gate_t gates[EX_GATE_COUNT]);

/**
 * [ex_gate_try_open spend `g->cost` from *energon if affordable and the gate
 *  isn't already open. Returns 1 and opens the gate on success, 0 (state
 *  unchanged) otherwise.]
 */
int ex_gate_try_open(ex_gate_t *g, long *energon);

/**
 * [ex_gates_next_locked index of the cheapest still-locked gate, or -1 if
 *  all are open (for UI: "press N to open the next gate").]
 */
int ex_gates_next_locked(const ex_gate_t gates[EX_GATE_COUNT]);

/**
 * [ex_gates_extent the current spawn half-extent: the largest opened gate's
 *  extent, or 60 (the starting arena) if none are open yet.]
 */
float ex_gates_extent(const ex_gate_t gates[EX_GATE_COUNT]);

#endif // EX_GATES_H
