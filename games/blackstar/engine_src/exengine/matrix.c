#include "matrix.h"

void ex_matrix_reset(ex_matrix_t *m){ if (m) m->timer = 0.0f; }

void ex_matrix_claim(ex_matrix_t *m){ if (m) m->timer = EX_MATRIX_DURATION; }

void ex_matrix_update(ex_matrix_t *m, float dt)
{
  if (!m) return;
  if (m->timer > 0.0f) { m->timer -= dt; if (m->timer < 0.0f) m->timer = 0.0f; }
}

int ex_matrix_active(const ex_matrix_t *m){ return m && m->timer > 0.0f; }

float ex_matrix_glow(const ex_matrix_t *m)
{
  if (!m || m->timer <= 0.0f) return 0.0f;
  float elapsed = EX_MATRIX_DURATION - m->timer;
  float fin  = elapsed / 0.5f; if (fin  > 1.0f) fin  = 1.0f;   /* ramp on */
  float fout = m->timer / 1.2f; if (fout > 1.0f) fout = 1.0f;  /* ebb out */
  float g = (fin < fout) ? fin : fout;
  return g < 0.0f ? 0.0f : g;
}
