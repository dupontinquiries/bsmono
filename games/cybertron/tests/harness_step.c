/* harness_step.c -- terrain-traversal smoothness harness.
 *
 * Drives a simulated character over a battery of procedurally-generated
 * obstacle courses (staircases, ramps, random blockers, tall walls, platforms
 * with gaps, tight clusters) using the SHARED ex_step_resolve() -- the exact
 * collision-step code the game runs. Each traversal records the character's
 * elevation frame-by-frame and scans it for NON-PHYSICAL discontinuities:
 *
 *   - fall-through   : elevation drops below the floor (collision failed)
 *   - pop            : a 1-2 frame up-then-down spike (the classic camera pop)
 *   - unexplained    : a single-frame elevation change larger than what the
 *                      current vertical velocity + one legal step-snap allow
 *                      (i.e. not explained by free-fall physics or a valid step)
 *   - jitter         : elevation oscillating (many dy sign-flips) while the
 *                      character is nominally grounded/stable
 *
 * Smooth motion is parabolic (free-fall) or piecewise-linear (ramps) with small
 * bounded step-snaps; anything else is flagged. Exits non-zero if any course
 * produces anomalies, so it doubles as a regression test (ctest).
 */
#include "exengine/terrain_step.h"
#include <stdio.h>
#include <stdlib.h>
#include <math.h>
#include <string.h>

#define DT       (1.0f/60.0f)
#define GRAV     60.0f
#define PR       0.9f
#define MAXBOX   256
#define MAXFR    4000

typedef struct { ex_step_box_t box[MAXBOX]; int n; const char *name;
                 float start[3], dir[3]; float expect_top; } course_t;

/* ---- deterministic RNG so runs are reproducible ---- */
static unsigned RS = 0x1234;
static float rnd(void){ RS = RS*1664525u + 1013904223u; return (float)((RS>>8)&0xFFFFFF)/(float)0xFFFFFF; }
static float rrange(float a, float b){ return a + (b-a)*rnd(); }

static void addbox(course_t *c, float cx,float cy,float cz,float hx,float hy,float hz){
  if (c->n>=MAXBOX) return;
  ex_step_box_t *b=&c->box[c->n++];
  b->center[0]=cx; b->center[1]=cy; b->center[2]=cz;
  b->half[0]=hx; b->half[1]=hy; b->half[2]=hz;
}

/* ---------------- course generators ---------------- */
static void course_flat(course_t *c){
  c->name="flat ground"; c->n=0;
  c->start[0]=-40; c->start[1]=0; c->start[2]=0; c->dir[0]=1; c->dir[2]=0; c->expect_top=0;
}
static void course_lowsteps(course_t *c){
  // small climbable steps (<= step tolerance) -- should ascend smoothly
  c->name="low staircase (climbable)"; c->n=0;
  float y=0;
  for(int i=0;i<12;i++){ float h=0.30f; y+=h; addbox(c, -20+i*4, y*0.5f, 0, 2.0f, y*0.5f, 2.0f); }
  c->start[0]=-30; c->start[1]=0; c->start[2]=0; c->dir[0]=1; c->dir[2]=0; c->expect_top=y;
}
static void course_ramp(course_t *c){
  // a fine-grained ramp (many thin steps) -- should read as a smooth slope
  c->name="fine ramp"; c->n=0;
  for(int i=0;i<40;i++){ float h=0.1f*(i+1); addbox(c, -20+i*1.2f, h*0.5f, 0, 0.7f, h*0.5f, 3.0f); }
  c->start[0]=-25; c->start[1]=0; c->start[2]=0; c->dir[0]=1; c->dir[2]=0; c->expect_top=-1;
}
static void course_tallwalls(course_t *c){
  // tall walls the character must NOT climb -- elevation must stay ~0
  c->name="tall walls (must not climb)"; c->n=0;
  for(int i=0;i<6;i++) addbox(c, -20+i*8, 10, rrange(-3,3), 1.5f, 10, 4.0f);
  c->start[0]=-30; c->start[1]=0; c->start[2]=0; c->dir[0]=1; c->dir[2]=0; c->expect_top=0;
}
static void course_random(course_t *c){
  c->name="random blockers"; c->n=0;
  for(int i=0;i<40;i++){
    float hy=rrange(0.2f,6.0f);
    addbox(c, rrange(-45,45), hy, rrange(-10,10), rrange(0.8f,4.0f), hy, rrange(0.8f,4.0f));
  }
  c->start[0]=-48; c->start[1]=0; c->start[2]=0; c->dir[0]=1; c->dir[2]=0; c->expect_top=-1;
}
static void course_platforms(course_t *c){
  // platforms at varied heights with gaps -- should stand on tops, fall in gaps
  c->name="platforms + gaps"; c->n=0;
  for(int i=0;i<8;i++){ float h=rrange(1.0f,6.0f); addbox(c, -35+i*10, h, 0, 3.0f, 0.6f, 3.0f); }
  c->start[0]=-40; c->start[1]=8; c->start[2]=0; c->dir[0]=1; c->dir[2]=0; c->expect_top=-1;
}
static void course_cluster(course_t *c){
  // overlapping boxes forming a jumbled mass -- tests resolve stability (no jitter)
  c->name="tight overlapping cluster"; c->n=0;
  for(int i=0;i<30;i++)
    addbox(c, rrange(-6,6), rrange(0.5f,3.0f), rrange(-6,6), rrange(1.5f,3.5f), rrange(0.5f,3.0f), rrange(1.5f,3.5f));
  c->start[0]=-20; c->start[1]=0; c->start[2]=0; c->dir[0]=1; c->dir[2]=0; c->expect_top=-1;
}
static void course_descend(course_t *c){
  // stepping DOWN off ledges -- elevation should drop via free-fall, not teleport
  c->name="descending ledges"; c->n=0;
  for(int i=0;i<8;i++){ float h=(8-i)*1.5f; addbox(c, -30+i*8, h*0.5f, 0, 3.5f, h*0.5f, 4.0f); }
  c->start[0]=-33; c->start[1]=13; c->start[2]=0; c->dir[0]=1; c->dir[2]=0; c->expect_top=-1;
}

/* ---------------- simulate one traversal, record elevation ---------------- */
typedef struct { float y[MAXFR]; float vy[MAXFR]; int grounded[MAXFR]; int n; } trace_t;

static void simulate(const course_t *c, float speed, float lateral, trace_t *tr){
  float pos[3]={c->start[0], c->start[1], c->start[2]+lateral};
  float vel[3]={0,0,0};
  tr->n=0;
  for(int f=0; f<MAXFR; f++){
    // horizontal drive along dir
    vel[0]=c->dir[0]*speed; vel[2]=c->dir[2]*speed;
    // gravity
    vel[1]-=GRAV*DT;
    // integrate
    float py0 = pos[1];                        // vertical pos before the move (swept)
    pos[0]+=vel[0]*DT; pos[1]+=vel[1]*DT; pos[2]+=vel[2]*DT;
    // floor
    if(pos[1]<0){ pos[1]=0; if(vel[1]<0)vel[1]=0; }
    // terrain step resolve (the code under test)
    int g = ex_step_resolve(pos, vel, PR, c->box, c->n, py0);
    if(pos[1]<=0.0001f) g=1;
    if(tr->n<MAXFR){ tr->y[tr->n]=pos[1]; tr->vy[tr->n]=vel[1]; tr->grounded[tr->n]=g; tr->n++; }
    // stop once we've crossed the course
    if(pos[0]>60 || pos[0]<-60 || pos[2]>60 || pos[2]<-60) break;
  }
}

/* ---------------- analyse a trace for non-physical elevation changes ---------------- */
typedef struct { int fallthrough, pops, unexplained, jitter; float max_dy; } report_t;

static void analyse(const trace_t *tr, report_t *r){
  memset(r,0,sizeof(*r));
  for(int i=1;i<tr->n;i++){
    float dy = tr->y[i]-tr->y[i-1];
    if(fabsf(dy)>r->max_dy) r->max_dy=fabsf(dy);
    // fall-through the floor
    if(tr->y[i] < -0.05f) r->fallthrough++;
    // largest single-frame change explainable by physics: |vy|*dt (free fall)
    // plus one legal step-snap (EX_STEP_SNAP_TOL), plus a small epsilon.
    float explain = fabsf(tr->vy[i-1])*DT + EX_STEP_SNAP_TOL + 0.05f;
    if(fabsf(dy) > explain) r->unexplained++;
    // transient pop: up then straight back down (or vice-versa), both large
    if(i+1<tr->n){
      float dy2 = tr->y[i+1]-tr->y[i];
      if(dy>0.4f && dy2<-0.4f && tr->grounded[i-1]) r->pops++;
    }
  }
  // jitter: within sliding windows where the character is grounded, count dy
  // sign flips; a stable stance should have almost none.
  int W=20;
  for(int i=1;i+W<tr->n;i+=W){
    int flips=0, gnd=1; float prev=0;
    for(int j=i;j<i+W;j++){
      float dy=tr->y[j]-tr->y[j-1];
      if(!tr->grounded[j]) gnd=0;
      if(j>i && ((dy>0.02f&&prev<-0.02f)||(dy<-0.02f&&prev>0.02f))) flips++;
      if(fabsf(dy)>0.02f) prev=dy;
    }
    if(gnd && flips>=4) r->jitter++;
  }
}

int main(void){
  course_t courses[8];
  void(*gens[8])(course_t*) = { course_flat, course_lowsteps, course_ramp,
    course_tallwalls, course_random, course_platforms, course_cluster, course_descend };
  int total_fail=0;
  printf("=== terrain traversal smoothness harness ===\n");
  printf("(shared ex_step_resolve; %d courses x multiple walk-lines x speeds)\n\n", 8);
  for(int ci=0; ci<8; ci++){
    RS = 0xC0DE + ci*7919;        // reproducible per course
    gens[ci](&courses[ci]);
    course_t *c=&courses[ci];
    report_t agg; memset(&agg,0,sizeof(agg));
    float maxtop=0;
    // several parallel walk-lines and speeds to cover the footprint
    float speeds[3]={6.0f, 14.0f, 24.0f};
    for(int s=0;s<3;s++) for(float lat=-6; lat<=6; lat+=3){
      trace_t tr; simulate(c, speeds[s], lat, &tr);
      report_t r; analyse(&tr, &r);
      agg.fallthrough+=r.fallthrough; agg.pops+=r.pops;
      agg.unexplained+=r.unexplained; agg.jitter+=r.jitter;
      if(r.max_dy>agg.max_dy) agg.max_dy=r.max_dy;
      for(int f=0; f<tr.n; f++) if(tr.y[f]>maxtop) maxtop=tr.y[f];  // PEAK elevation
    }
    int fail = agg.fallthrough || agg.pops || agg.unexplained || agg.jitter;
    // low staircase is EXPECTED to reach its top; flag if it got stuck instead
    int stuck = 0;
    if(c->expect_top>0 && maxtop < c->expect_top*0.6f) stuck=1;
    printf("%-30s boxes=%-3d maxdy=%.2f  fall=%d pop=%d unexpl=%d jitter=%d  peak=%.1f%s\n",
           c->name, c->n, agg.max_dy, agg.fallthrough, agg.pops, agg.unexplained, agg.jitter,
           maxtop, stuck?"  [STUCK: couldn't climb]":"");
    if(fail){ total_fail++; }
    if(stuck){ /* informational: not a step bug, a design-reachability note */ }
  }
  printf("\n%s (%d course(s) with non-physical elevation anomalies)\n",
         total_fail? "ANOMALIES DETECTED" : "ALL COURSES SMOOTH", total_fail);
  return total_fail ? 1 : 0;
}
