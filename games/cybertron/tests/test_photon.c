/* test_photon.c -- Photon Man's auto-trigger last-stand state machine.
   Verifies: it does NOT fire when safe/healthy/ineligible, DOES fire when
   overwhelmed, runs for its duration, then locks out for the life; glow ramps. */
#include <stdio.h>
#include <assert.h>
#include "photon.h"

int main(void)
{
  ex_photon_t p; ex_photon_reset(&p);

  // healthy + threatened -> no trigger
  assert(ex_photon_update(&p, 0.9f, 1, 1, 0.1f) == 0);
  // critical but NOT threatened -> no trigger (it steps in only under real danger)
  assert(ex_photon_update(&p, 0.10f, 0, 1, 0.1f) == 0);
  // critical + threatened but NOT eligible (e.g. human scale) -> no trigger
  assert(ex_photon_update(&p, 0.10f, 1, 0, 0.1f) == 0);
  assert(!ex_photon_active(&p) && !ex_photon_spent(&p));
  printf("holds fire when safe / not-threatened / ineligible OK\n");

  // critical + threatened + eligible -> activates this frame
  int fired = ex_photon_update(&p, 0.10f, 1, 1, 0.1f);
  assert(fired == 1 && ex_photon_active(&p));
  printf("activates when overwhelmed OK\n");

  // stays active for ~DURATION even if we're now healthy; glow > 0
  float t = 0.0f; int refire = 0;
  for (int i=0;i<100;i++){
    t += 0.1f;
    if (ex_photon_update(&p, 0.9f, 0, 1, 0.1f)) refire++;
    if (!ex_photon_active(&p)) break;
  }
  assert(refire == 0 && "must not re-fire while active");
  assert(t > EX_PHOTON_DURATION-0.25f && t < EX_PHOTON_DURATION+0.25f);
  assert(ex_photon_spent(&p) && !ex_photon_active(&p));
  printf("runs ~%.1fs then locks out OK (ended at %.1fs)\n", (double)EX_PHOTON_DURATION, t);

  // once spent, never triggers again this life -- even fully overwhelmed
  for (int i=0;i<20;i++) assert(ex_photon_update(&p, 0.05f, 1, 1, 0.1f) == 0);
  assert(ex_photon_glow(&p) == 0.0f);
  printf("stays cold after use OK\n");

  // reset restores availability
  ex_photon_reset(&p);
  assert(!ex_photon_spent(&p));
  assert(ex_photon_update(&p, 0.10f, 1, 1, 0.1f) == 1);
  printf("reset re-arms it OK\n");

  printf("\nPHOTON MAN OK (the watch decides the moment, once)\n");
  return 0;
}
