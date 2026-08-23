#include "provenance.h"
#include <string.h>
#include <stddef.h>

/* Distinct-source counting without allocating: the source lists here are
   tiny (a claim has a handful of bearings on it, ever), so a linear scan
   over what we have already counted is both simpler and faster than any
   set. MAX_SRC caps it; overflow degrades to counting duplicates, which
   can only ever OVERSTATE corroboration, so the cap is checked by test
   rather than left as a silent cliff. */
#define EX_MAX_SRC 32

typedef struct {
  const char *seen[EX_MAX_SRC];
  int n;
} srcset_t;

static int srcset_add(srcset_t *s, const char *src) {
  if (!src) return 0;
  for (int i = 0; i < s->n; i++) {
    if (strcmp(s->seen[i], src) == 0) return 0;   /* already counted */
  }
  if (s->n < EX_MAX_SRC) s->seen[s->n++] = src;
  return 1;
}

ex_standing_t ex_standing_of(const ex_assay_t *a) {
  if (!a) return EX_STANDING_DARK;
  /* nobody has made this claim yet, so there is nothing to dispute */
  if (a->needs_claimant && a->support == 0) return EX_STANDING_DARK;
  if (a->support == 0 && a->opposition == 0) {
    return a->mentions > 0 ? EX_STANDING_UNKNOWN : EX_STANDING_DARK;
  }
  /* real disagreement is a dispute, not an average */
  if (a->opposition > 0) return EX_STANDING_CONTESTED;
  if (a->support == 1) return EX_STANDING_UNKNOWN;
  if (a->support == 2) return EX_STANDING_POSSIBLE;
  return EX_STANDING_HIGH;
}

const char* ex_standing_label(ex_standing_t s) {
  switch (s) {
    case EX_STANDING_UNKNOWN:   return "UNKNOWN TECHNOLOGICAL RELATION";
    case EX_STANDING_POSSIBLE:  return "POSSIBLE SHARED ORIGIN";
    case EX_STANDING_HIGH:      return "HIGH-CONFIDENCE CONNECTION";
    case EX_STANDING_CONTESTED: return "SOURCES DISAGREE";
    default:                    return "--";
  }
}

int ex_standing_drawn(ex_standing_t s) { return s != EX_STANDING_DARK; }

const char* ex_stratum_name(int layer) {
  switch (layer) {
    case 0: return "THE PRESENT";
    case 1: return "RECENT HISTORY";
    case 2: return "THE BLACKSTAR ERA";
    case 3: return "THE BLACK ARCHIVE";
    case 4: return "ASTERI";
    case 5: return "BEFORE ASTERI";
    default: return "--";
  }
}

static int was_found(const char *id, const char *const *found, int n) {
  for (int i = 0; i < n; i++) {
    if (found[i] && strcmp(found[i], id) == 0) return 1;
  }
  return 0;
}

void ex_assay_claim(const ex_claim_t *claim,
                    const ex_finding_t *findings, int finding_count,
                    const char *const *found, int found_count,
                    const ex_received_t *priors, int prior_count,
                    ex_assay_t *out) {
  if (!out) return;
  memset(out, 0, sizeof(*out));
  if (!claim) return;
  out->needs_claimant = claim->asserted;

  srcset_t f = {{0},0}, a = {{0},0}, m = {{0},0};

  for (int i = 0; i < prior_count; i++) {
    const ex_received_t *p = &priors[i];
    if (!p->claim || strcmp(p->claim, claim->id) != 0) continue;
    if (p->bearing == EX_BEARING_SUPPORTS)         out->support    += srcset_add(&f, p->source);
    else if (p->bearing == EX_BEARING_CONTRADICTS) out->opposition += srcset_add(&a, p->source);
    else                                           out->mentions   += srcset_add(&m, p->source);
  }

  for (int i = 0; i < finding_count; i++) {
    const ex_finding_t *fd = &findings[i];
    if (!was_found(fd->id, found, found_count)) continue;
    for (int k = 0; k < fd->link_count; k++) {
      if (strcmp(fd->links[k].claim, claim->id) != 0) continue;
      if (fd->links[k].bearing == EX_BEARING_SUPPORTS)         out->support    += srcset_add(&f, fd->source);
      else if (fd->links[k].bearing == EX_BEARING_CONTRADICTS) out->opposition += srcset_add(&a, fd->source);
      else                                                     out->mentions   += srcset_add(&m, fd->source);
    }
  }
}

int ex_priors_at(const ex_received_t *received, int received_count,
                 float anchor, const ex_received_t **out, int cap) {
  int n = 0;
  for (int i = 0; i < received_count; i++) {
    if (anchor < received[i].at) continue;
    if (out && n < cap) out[n] = &received[i];
    n++;
  }
  return n;
}

const char* ex_anchor_name(float anchor) {
  if (anchor < 0.15f) return "FRESH";
  if (anchor < 0.50f) return "SPARSE";
  if (anchor < 0.80f) return "FAMILIAR";
  return "HOMEY";
}
