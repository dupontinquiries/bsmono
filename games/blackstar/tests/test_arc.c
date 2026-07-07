/* test_arc.c -- chain-lightning targeting.
   Verifies the chain hops to the nearest unvisited enemy within range, never
   repeats, respects the jump cap, and stops when nothing's in reach. */
#include <stdio.h>
#include <assert.h>
#include "arc.h"

int main(void){
  // enemies in a rough line stepping away along +x
  float epos[6][3] = { {5,0,0}, {12,0,0}, {20,0,0}, {28,0,0}, {200,0,0}, {6,0,3} };
  int alive[6] = {1,1,1,1,1,1};
  int chain[EX_ARC_JUMPS];

  // from origin: nearest is 0 (5u), then 5 (6,3), then 1, then 2, then 3; #4 is 200u away (never)
  int n = ex_arc_chain((float[]){0,0,0}, epos, alive, 6, 30.0f, EX_ARC_JUMPS, chain);
  assert(n >= 3 && "should chain several");
  // no repeats
  for (int i=0;i<n;i++) for(int j=i+1;j<n;j++) assert(chain[i]!=chain[j]);
  // the far outlier (index 4) must never be hit (out of every jump's range)
  for (int i=0;i<n;i++) assert(chain[i]!=4);
  // first hit is the nearest to origin (index 0)
  assert(chain[0]==0);
  printf("nearest-first, no repeats, range-limited OK (n=%d)\n", n);

  // jump cap respected
  float many[10][3]; int al[10];
  for(int i=0;i<10;i++){ many[i][0]=i*4.0f; many[i][1]=0; many[i][2]=0; al[i]=1; }
  int n2 = ex_arc_chain((float[]){0,0,0}, many, al, 10, 30.0f, 3, chain);
  assert(n2==3 && "capped at max_jumps");
  printf("jump cap OK\n");

  // nothing in reach -> empty
  float far[2][3]={{500,0,0},{0,0,500}}; int fa[2]={1,1};
  assert(ex_arc_chain((float[]){0,0,0}, far, fa, 2, 30.0f, EX_ARC_JUMPS, chain)==0);
  // dead enemies skipped
  int dead[6]={0,0,0,0,0,0};
  assert(ex_arc_chain((float[]){0,0,0}, epos, dead, 6, 30.0f, EX_ARC_JUMPS, chain)==0);
  printf("out-of-range / all-dead -> empty OK\n");

  printf("\nARC OK (chain lightning targeting)\n");
  return 0;
}
