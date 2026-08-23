/* provenance
  The archaeological map's inference, C side.

  The Rust fork (games/blackstar-rs/src/provenance.rs) owns the design; this
  is the same ladder so the CODEX screen can show a claim's standing instead
  of a flat page count. The two are pinned together rather than trusted to
  agree: the tables come from ex_archive_gen.h, which is GENERATED from the
  Rust content by `skydribble --arch emit-c`, and tests/test_provenance.c
  replays fixtures the Rust test suite also asserts.

  Why the standing is computed rather than stored: an entry's confidence is a
  function of what the player has found, and the codex should not need a
  second copy of that answer that can go stale.

  Two rules carry the weight, and both exist because the obvious version was
  wrong:

  * Corroboration counts DISTINCT SOURCES, not findings. Three readings off
    one instrument are one observation; without this a player farms certainty
    out of a single wreck.

  * A claim marked `asserted` (an official date, a published figure) stays
    dark until somebody asserts it, however much evidence stacks against it.
    You cannot refute a record you have never seen.
*/
#ifndef EX_PROVENANCE_H
#define EX_PROVENANCE_H

/* Bearing of one finding on one claim. */
typedef enum {
  EX_BEARING_SUPPORTS = 0,
  EX_BEARING_CONTRADICTS = 1,
  EX_BEARING_MENTIONS = 2
} ex_bearing_t;

/* Where a claim stands. Ordered by strength; EX_STANDING_CONTESTED is last
   because it is the most interesting, not the strongest. */
typedef enum {
  EX_STANDING_DARK = 0,
  EX_STANDING_UNKNOWN = 1,
  EX_STANDING_POSSIBLE = 2,
  EX_STANDING_HIGH = 3,
  EX_STANDING_CONTESTED = 4
} ex_standing_t;

typedef struct {
  const char *claim;
  ex_bearing_t bearing;
} ex_link_t;

/* How the evidence is physically encountered, which decides WHERE it is
   read: artifacts come home to the ship's chartroom, terminals and sites are
   read in place. Presentation only; the ladder ignores it. */
typedef enum {
  EX_MEDIUM_ARTIFACT = 0,
  EX_MEDIUM_TERMINAL = 1,
  EX_MEDIUM_SITE = 2
} ex_medium_t;

typedef struct {
  const char *id;
  const char *site;
  const char *source;   /* independence key */
  ex_medium_t medium;
  const char *gate;     /* NULL, or a campaign mission id */
  const ex_link_t *links;
  int link_count;
} ex_finding_t;

typedef struct {
  const char *id;
  int layer;            /* 0..5, see ex_stratum_name */
  int asserted;         /* 1 = somebody's assertion, not a fact about the world */
  const char *text;
} ex_claim_t;

/* Received knowledge: what the mystery anchor hands the player already
   believing. Enters the tally as one contradictable source. */
typedef struct {
  const char *claim;
  ex_bearing_t bearing;
  const char *source;
  float at;             /* anchor level 0..1 at which this is known */
  const char *gloss;
} ex_received_t;

/* The tally behind one claim, kept so the codex can show its work. */
typedef struct {
  int support;          /* distinct sources for */
  int opposition;       /* distinct sources against */
  int mentions;         /* distinct sources naming it without settling it */
  int needs_claimant;   /* mirrored from ex_claim_t.asserted */
} ex_assay_t;

ex_standing_t ex_standing_of(const ex_assay_t *a);
const char*   ex_standing_label(ex_standing_t s);
int           ex_standing_drawn(ex_standing_t s);
const char*   ex_stratum_name(int layer);

/* Tally one claim over the findings whose ids appear in `found`.
   `found` is an array of `found_count` id strings; `priors` may be NULL. */
void ex_assay_claim(const ex_claim_t *claim,
                    const ex_finding_t *findings, int finding_count,
                    const char *const *found, int found_count,
                    const ex_received_t *priors, int prior_count,
                    ex_assay_t *out);

/* Which received entries apply at an anchor level. Returns the count and
   fills `out` with up to `cap` pointers. */
int ex_priors_at(const ex_received_t *received, int received_count,
                 float anchor, const ex_received_t **out, int cap);

const char* ex_anchor_name(float anchor);

#endif /* EX_PROVENANCE_H */
