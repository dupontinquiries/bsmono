#include "detail.h"
#include <math.h>
#include <string.h>

static uint32_t rng_next(uint32_t *s){ *s^=*s<<13; *s^=*s>>17; *s^=*s<<5; return *s; }
static float rf(uint32_t *s){ return (float)(rng_next(s)&0xFFFFFF)/(float)0x1000000; }
static float rs(uint32_t *s){ return rf(s)*2.0f-1.0f; }

int ex_detail_generate(ex_greeble_t *out, int max,
                       const ex_detail_part_t *parts, int n_parts,
                       uint32_t seed, int level, const uint8_t accent[3])
{
  if (max > EX_DETAIL_MAX) max = EX_DETAIL_MAX;
  if (level > max) level = max;
  if (level <= 0 || n_parts <= 0) return 0;

  uint32_t s = seed ? seed : 1u;
  int n = 0;

  // distribute the greeble budget across parts proportional to surface area
  float total_area = 0.0f;
  for (int p = 0; p < n_parts; p++) {
    const ex_detail_part_t *bp = &parts[p];
    total_area += bp->sx*bp->sy + bp->sy*bp->sz + bp->sx*bp->sz;
  }
  if (total_area <= 0.0f) return 0;

  for (int p = 0; p < n_parts && n < level; p++) {
    const ex_detail_part_t *bp = &parts[p];
    float area = bp->sx*bp->sy + bp->sy*bp->sz + bp->sx*bp->sz;
    int quota = (int)((area / total_area) * level + 0.5f);
    if (quota < 1) quota = 1;

    for (int k = 0; k < quota && n < level; k++) {
      ex_greeble_t *g = &out[n];
      g->parent = p;
      // PATTERNED placement: bias to the TOP and SIDES (never the underside), and
      // SNAP the position to a grid so greebles line up in rows/columns instead of
      // scattering. Mirror alternate greebles across x for left/right symmetry.
      int fr = rng_next(&s) % 10;
      int face = (fr<4)?2 : (fr<6)?0 : (fr<8)?1 : (fr<9)?4 : 5;   // top-heavy; some sides/front; no bottom
      const int GRID = 4;
      float u = (floorf(rf(&s)*GRID)/(float)(GRID-1) - 0.5f) * 2.0f * 0.82f;
      float v = (floorf(rf(&s)*GRID)/(float)(GRID-1) - 0.5f) * 2.0f * 0.82f;
      if (k & 1) { if (face==0) face=1; else if (face==1) face=0; }   // mirror sides
      float ox=0, oy=0, oz=0;
      switch (face) {
        case 0: ox=bp->sx;  oy=bp->sy*v; oz=bp->sz*u; break; // +x
        case 1: ox=-bp->sx; oy=bp->sy*v; oz=bp->sz*u; break; // -x
        case 2: oy=bp->sy;  ox=bp->sx*u; oz=bp->sz*v; break; // +y
        case 4: oz=bp->sz;  ox=bp->sx*u; oy=bp->sy*v; break; // +z
        default: oz=-bp->sz; ox=bp->sx*u; oy=bp->sy*v; break; // -z
      }

      // choose a shape by roll; sizes scale to the parent part. Flat panels
      // dominate; tall protrusions are rarer and shorter so they don't clutter.
      float roll = rf(&s);
      ex_greeble_shape shape;
      float gx, gy, gz, out_amt;
      float unit = fminf(fminf(bp->sx,bp->sy),bp->sz);
      if (roll < 0.50f)      { shape=EX_GREEBLE_PLATE;  gx=unit*0.38f; gy=unit*0.06f; gz=unit*0.38f; out_amt=unit*0.04f; }
      else if (roll < 0.74f) { shape=EX_GREEBLE_RIVET;  gx=gy=gz=unit*0.10f;          out_amt=unit*0.05f; }
      else if (roll < 0.88f) { shape=EX_GREEBLE_VENT;   gx=unit*0.32f; gy=unit*0.04f; gz=unit*0.09f; out_amt=unit*0.03f; }
      else if (roll < 0.96f) { shape=EX_GREEBLE_RIDGE;  gx=unit*0.55f; gy=unit*0.09f; gz=unit*0.08f; out_amt=unit*0.05f; }
      else if (roll < 0.99f) { shape=EX_GREEBLE_SPIKE;  gx=gy=unit*0.07f; gz=unit*0.22f;             out_amt=unit*0.12f; }
      else                   { shape=EX_GREEBLE_ANTENNA;gx=gz=unit*0.03f; gy=unit*0.42f;             out_amt=unit*0.24f; }

      // push the greeble slightly out along the face normal
      float nx = (face==0)?1:(face==1)?-1:0;
      float ny = (face==2)?1:(face==3)?-1:0;
      float nz = (face==4)?1:(face==5)?-1:0;
      g->offset[0] = bp->offset[0] + ox + nx*out_amt;
      g->offset[1] = bp->offset[1] + oy + ny*out_amt;
      g->offset[2] = bp->offset[2] + oz + nz*out_amt;
      g->sx=gx; g->sy=gy; g->sz=gz;
      g->shape = (uint8_t)shape;

      // colour: mostly the parent tint, occasionally darker panels or accent
      float ct = rf(&s);
      if (ct < 0.15f && accent) { g->color[0]=accent[0]; g->color[1]=accent[1]; g->color[2]=accent[2]; }
      else {
        float shade = 0.7f + rf(&s)*0.5f;
        int r=(int)(bp->color[0]*shade), gg=(int)(bp->color[1]*shade), b=(int)(bp->color[2]*shade);
        g->color[0]=r>255?255:r; g->color[1]=gg>255?255:gg; g->color[2]=b>255?255:b;
      }
      n++;
    }
  }
  return n;
}
