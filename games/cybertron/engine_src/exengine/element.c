#include "element.h"

ex_element_profile_t ex_element_profile(ex_element_t e)
{
  switch (e) {
    case EX_ELEM_ICE:      return (ex_element_profile_t){ .damage_mul=0.9f,  .range_mul=1.4f, .cooldown_mul=1.1f, .move_mul=0.95f };
    case EX_ELEM_FIRE:     return (ex_element_profile_t){ .damage_mul=1.5f,  .range_mul=0.7f, .cooldown_mul=1.0f, .move_mul=1.0f  };
    case EX_ELEM_AIR:      return (ex_element_profile_t){ .damage_mul=0.7f,  .range_mul=1.0f, .cooldown_mul=0.6f, .move_mul=1.35f };
    case EX_ELEM_ELECTRIC: return (ex_element_profile_t){ .damage_mul=1.1f,  .range_mul=1.5f, .cooldown_mul=0.9f, .move_mul=1.0f  };
    case EX_ELEM_VINE:     return (ex_element_profile_t){ .damage_mul=1.0f,  .range_mul=1.0f, .cooldown_mul=1.0f, .move_mul=0.9f  };
    case EX_ELEM_ROCK:     return (ex_element_profile_t){ .damage_mul=1.4f,  .range_mul=0.6f, .cooldown_mul=1.3f, .move_mul=0.75f };
    case EX_ELEM_WATER:    return (ex_element_profile_t){ .damage_mul=1.0f,  .range_mul=1.15f,.cooldown_mul=0.95f,.move_mul=1.05f };
    case EX_ELEM_NONE:
    default:               return (ex_element_profile_t){ .damage_mul=1.0f,  .range_mul=1.0f, .cooldown_mul=1.0f, .move_mul=1.0f  };
  }
}

float ex_element_advantage(ex_element_t attacker, ex_element_t defender)
{
  if (attacker == EX_ELEM_NONE || defender == EX_ELEM_NONE)
    return 1.0f;

  // arrange the 7 elements in a cycle; each element is strong against the next
  // two in the ring and weak against the previous two. Index 1..7 map to a 0..6
  // ring (subtract 1 so ICE=0 ... WATER=6).
  int a = (int)attacker - 1;
  int d = (int)defender - 1;
  int n = EX_ELEM_COUNT - 1; // 7 real elements

  int diff = ((d - a) % n + n) % n; // 0..6, how far defender is ahead of attacker in the ring
  if (diff == 1 || diff == 2) return 1.5f;   // attacker beats the next two
  if (diff == n - 1 || diff == n - 2) return 0.66f; // loses to the previous two
  return 1.0f; // same element or the neutral opposite
}

const char* ex_element_name(ex_element_t e)
{
  switch (e) {
    case EX_ELEM_ICE:      return "ice";
    case EX_ELEM_FIRE:     return "fire";
    case EX_ELEM_AIR:      return "air";
    case EX_ELEM_ELECTRIC: return "electric";
    case EX_ELEM_VINE:     return "vine";
    case EX_ELEM_ROCK:     return "rock";
    case EX_ELEM_WATER:    return "water";
    case EX_ELEM_NONE:
    default:               return "none";
  }
}

// ---- The Watch ----

ex_watch_t ex_watch_new(ex_element_t start)
{
  ex_watch_t w;
  w.current = start;
  w.pending = start;
  w.attune_timer = 0.0f;
  w.attuning = 0;
  return w;
}

int ex_watch_switch(ex_watch_t *w, ex_element_t to)
{
  if (w->attuning) return 0;              // already switching
  if (to == w->current) return 0;         // no-op
  if (to <= EX_ELEM_NONE || to >= EX_ELEM_COUNT) return 0;
  w->pending = to;
  w->attuning = 1;
  w->attune_timer = EX_WATCH_ATTUNE_SEC;
  return 1;
}

int ex_watch_cycle(ex_watch_t *w)
{
  ex_element_t next = (ex_element_t)((int)w->current + 1);
  if (next >= EX_ELEM_COUNT) next = EX_ELEM_ICE;
  return ex_watch_switch(w, next);
}

void ex_watch_update(ex_watch_t *w, float dt)
{
  if (!w->attuning) return;
  w->attune_timer -= dt;
  if (w->attune_timer <= 0.0f) {
    w->current = w->pending;
    w->attuning = 0;
    w->attune_timer = 0.0f;
  }
}
