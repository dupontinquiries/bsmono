/* genome.h -- a seedable, mutatable proportion genome for bots.
 *
 * Design pattern borrowed (as a concept, original code) from a parametric
 * character-forge: every heritable proportion is a GENE with a min/max/default,
 * so a whole distinct bot can be produced from a single 64-bit seed, nudged by
 * mutation, or blended (bred) from two parents. This unifies the loose
 * proportion fields the builder was already carrying and is the foundation for
 * distinct-looking bots + inherited alt-mode traits.
 *
 * Pure logic (no rendering), so the movement/feel harness can exercise it. */
#ifndef EX_GENOME_H
#define EX_GENOME_H

#include <stdint.h>

typedef enum {
  EX_GENE_BULK = 0,   /* torso/limb thickness      */
  EX_GENE_HEIGHT,     /* overall vertical scale    */
  EX_GENE_HEAD,       /* head size                 */
  EX_GENE_SHOULDER,   /* shoulder width            */
  EX_GENE_ARM,        /* arm length                */
  EX_GENE_LEG,        /* leg length                */
  EX_GENE_NECK,       /* neck length (creature)    */
  EX_GENE_TAIL,       /* tail length (creature)    */
  EX_GENE_SNOUT,      /* muzzle length (creature)  */
  EX_GENE_JAW,        /* jaw depth (creature)      */
  EX_GENE_CREST,      /* crest/horn prominence     */
  EX_GENE_GREEBLE,    /* surface detail density    */
  EX_GENE_COUNT
} ex_gene_id;

/* one locus: label + legal range + species default */
typedef struct { const char *name; float lo, hi, def; } ex_gene_spec_t;

/* the canonical gene table (mirrors the builder's proportion sliders + the
 * creature-mode extras). Ranges chosen so extremes read as distinct silhouettes
 * without breaking the rig. */
static const ex_gene_spec_t EX_GENES[EX_GENE_COUNT] = {
  { "bulk",     0.60f, 1.80f, 1.00f },
  { "height",   0.70f, 1.55f, 1.00f },
  { "head",     0.65f, 1.60f, 1.00f },
  { "shoulder", 0.70f, 1.70f, 1.00f },
  { "arm",      0.70f, 1.50f, 1.00f },
  { "leg",      0.70f, 1.55f, 1.00f },
  { "neck",     0.60f, 2.00f, 1.00f },
  { "tail",     0.50f, 2.10f, 1.00f },
  { "snout",    0.50f, 1.90f, 1.00f },
  { "jaw",      0.40f, 2.10f, 1.00f },
  { "crest",    0.00f, 1.60f, 0.50f },
  { "greeble",  0.30f, 1.60f, 1.00f },
};

typedef struct { float g[EX_GENE_COUNT]; } ex_genome_t;

/* deterministic splitmix64 -> a gene value inside its [lo,hi] range */
static inline uint64_t ex__sm64(uint64_t *s){
  uint64_t z = (*s += 0x9E3779B97F4A7C15ULL);
  z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ULL;
  z = (z ^ (z >> 27)) * 0x94D049BB133111EBULL;
  return z ^ (z >> 31);
}
static inline float ex__u01(uint64_t *s){ return (float)((ex__sm64(s) >> 11) & 0xFFFFFF) / (float)0x1000000; }

/* default genome (all species defaults) */
static inline ex_genome_t ex_genome_default(void){
  ex_genome_t o; for (int i=0;i<EX_GENE_COUNT;i++) o.g[i] = EX_GENES[i].def; return o;
}

/* a whole distinct bot from one seed. `spread` (0..1) scales how far genes stray
 * from their defaults, so 0 = stock, 1 = wild variation. */
static inline ex_genome_t ex_genome_from_seed(uint64_t seed, float spread){
  uint64_t s = seed ? seed : 0xC0FFEEULL;
  ex_genome_t o;
  for (int i=0;i<EX_GENE_COUNT;i++){
    float t = ex__u01(&s);                          /* 0..1 across the range   */
    float full = EX_GENES[i].lo + (EX_GENES[i].hi - EX_GENES[i].lo) * t;
    o.g[i] = EX_GENES[i].def + (full - EX_GENES[i].def) * spread;   /* lerp from default */
  }
  return o;
}

/* breed: blend two genomes (0.5 = midpoint) with a little seeded mutation. */
static inline ex_genome_t ex_genome_breed(const ex_genome_t *a, const ex_genome_t *b,
                                          float mix, uint64_t seed, float mutate){
  uint64_t s = seed ? seed : 1ULL;
  ex_genome_t o;
  for (int i=0;i<EX_GENE_COUNT;i++){
    float v = a->g[i] + (b->g[i] - a->g[i]) * mix;
    v += (ex__u01(&s) * 2.0f - 1.0f) * mutate * (EX_GENES[i].hi - EX_GENES[i].lo);
    if (v < EX_GENES[i].lo) v = EX_GENES[i].lo;
    if (v > EX_GENES[i].hi) v = EX_GENES[i].hi;
    o.g[i] = v;
  }
  return o;
}

static inline float ex_gene(const ex_genome_t *gm, ex_gene_id id){ return gm->g[id]; }

#endif
