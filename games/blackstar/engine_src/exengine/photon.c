#include "photon.h"

void ex_photon_reset(ex_photon_t *p)
{
  if (!p) return;
  p->state = 0; p->timer = 0.0f; p->total = EX_PHOTON_DURATION;
}

int ex_photon_update(ex_photon_t *p, float hp_frac, int under_threat, int eligible, float dt)
{
  if (!p) return 0;
  if (p->state == 1) {                 /* active -> count down */
    p->timer -= dt;
    if (p->timer <= 0.0f) { p->timer = 0.0f; p->state = 2; }   /* spent for this life */
    return 0;
  }
  if (p->state == 2) return 0;         /* already used -> the Watch stays cold */
  /* idle: the Watch decides the moment -- only when nothing else is enough */
  if (eligible && under_threat && hp_frac > 0.0f && hp_frac <= EX_PHOTON_HP_TRIGGER) {
    p->state = 1; p->timer = EX_PHOTON_DURATION; p->total = EX_PHOTON_DURATION;
    return 1;
  }
  return 0;
}

int ex_photon_active(const ex_photon_t *p){ return p && p->state == 1; }
int ex_photon_spent (const ex_photon_t *p){ return p && p->state == 2; }

float ex_photon_glow(const ex_photon_t *p)
{
  if (!p || p->state != 1) return 0.0f;
  float elapsed = p->total - p->timer;
  float fin = elapsed / 0.4f;  if (fin > 1.0f) fin = 1.0f;   /* snap on */
  float fout = p->timer / 0.8f; if (fout > 1.0f) fout = 1.0f; /* ebb away */
  float g = (fin < fout) ? fin : fout;
  return (g < 0.0f) ? 0.0f : g;
}
