#include "exengine/genome.h"
#include <stdio.h>
#include <math.h>
static int fails=0;
#define CHECK(c,m) do{ if(!(c)){ printf("  FAIL: %s\n",m); fails++; } }while(0)
int main(void){
  // every seeded gene stays inside its declared range, for many seeds
  for (uint64_t s=1; s<=2000; s++){
    ex_genome_t g = ex_genome_from_seed(s*2654435761ULL, 1.0f);
    for (int i=0;i<EX_GENE_COUNT;i++)
      CHECK(g.g[i] >= EX_GENES[i].lo-1e-4f && g.g[i] <= EX_GENES[i].hi+1e-4f, "gene out of range");
  }
  // spread=0 reproduces the species default exactly (stock bot)
  ex_genome_t d0 = ex_genome_from_seed(12345, 0.0f);
  for (int i=0;i<EX_GENE_COUNT;i++) CHECK(fabsf(d0.g[i]-EX_GENES[i].def)<1e-4f, "spread 0 must be default");
  // a seed is deterministic (same seed -> same bot)
  ex_genome_t a=ex_genome_from_seed(777,1.0f), b=ex_genome_from_seed(777,1.0f);
  for (int i=0;i<EX_GENE_COUNT;i++) CHECK(a.g[i]==b.g[i], "genome must be deterministic");
  // breeding stays in range and lands between parents (pre-mutation midpoint)
  ex_genome_t p1=ex_genome_from_seed(111,1.0f), p2=ex_genome_from_seed(222,1.0f);
  ex_genome_t kid=ex_genome_breed(&p1,&p2,0.5f,999,0.0f);
  for (int i=0;i<EX_GENE_COUNT;i++){
    float mid=(p1.g[i]+p2.g[i])*0.5f;
    CHECK(fabsf(kid.g[i]-mid)<1e-3f, "no-mutation breed = midpoint");
    CHECK(kid.g[i]>=EX_GENES[i].lo-1e-4f && kid.g[i]<=EX_GENES[i].hi+1e-4f, "bred gene out of range");
  }
  printf(fails? "SOME CHECKS FAILED\n":"ALL GENOME CHECKS PASSED\n");
  return fails?1:0;
}
