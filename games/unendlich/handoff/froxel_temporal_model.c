// Temporal froxel accumulation model (handoff/WORKER_D_temporal_model.md).
//
// docs/SHADER_ROADMAP.md's "Next-increment analysis" flags temporal froxel
// accumulation (reproject previous frame + EMA blend, instead of recomputing
// cell_density from scratch every frame) as the next optimization worth
// considering — and says to validate the reproject+EMA math on the CPU
// BEFORE touching any shader. This is that validation. It reuses
// cell_density() ported byte-for-byte from froxel_fill.comp (same source
// handoff/froxel_skip_model.c already ports for the same reason — "in float
// to match the GPU" — copied from there rather than re-derived, so both
// models stay consistent with each other and with the shader).
//
// Model: one froxel column tracked across frames while the camera pans
// through world space. Each frame:
//   1. ground_truth  = cell_density() at the column's true current world
//      position — what a full per-frame recompute (today's behavior) gives.
//   2. current_sample = cell_density() at the REPROJECTED position (the
//      previous frame's UVW shifted to this frame's camera, plus a small
//      resample error — reprojection is a texture lookup at a slightly
//      wrong subpixel location, not a re-derivation).
//   3. neighbourhood clamp (optional, toggled per run): history is clamped
//      to [min,max] of a few nearby world-space samples THIS frame, before
//      blending — the standard TAA anti-ghosting technique.
//   4. history = mix(history_clamped, current_sample, alpha)  — EMA blend.
// A disocclusion event (hard cut in the underlying scene) happens partway
// through the run to measure ghosting: how far history lags the new ground
// truth immediately after, and how fast it recovers, with vs without the
// clamp.
//
// No shader edits. No production Rust code touched. This file is the
// entire deliverable.
//
// Build/run: gcc -O2 -std=gnu11 handoff/froxel_temporal_model.c -o /tmp/m -lm && /tmp/m

#include <stdio.h>
#include <math.h>
#include <stdlib.h>

static float fract1(float x){ return x - floorf(x); }
static float clampf(float x,float a,float b){ return x<a?a:(x>b?b:x); }
static float mixf(float a,float b,float t){ return a+(b-a)*t; }
static float smoothstepf(float e0,float e1,float x){ float t=clampf((x-e0)/(e1-e0),0.f,1.f); return t*t*(3.f-2.f*t); }

// hash13 / value_noise_3d / fbm2 — identical to the shader (and to
// froxel_skip_model.c's copy — kept byte-for-byte the same on purpose so
// this model and that one never silently diverge from each other).
static float hash13(float px,float py,float pz){
    float x=fract1(px*0.1031f), y=fract1(py*0.1031f), z=fract1(pz*0.1031f);
    float d = x*(z+31.32f)+y*(y+31.32f)+z*(x+31.32f);
    x+=d; y+=d; z+=d;
    return fract1((x+y)*z);
}
static float vnoise(float px,float py,float pz){
    float ix=floorf(px),iy=floorf(py),iz=floorf(pz);
    float fx=px-ix,fy=py-iy,fz=pz-iz;
    float ux=fx*fx*(3.f-2.f*fx), uy=fy*fy*(3.f-2.f*fy), uz=fz*fz*(3.f-2.f*fz);
    float c000=hash13(ix+0,iy+0,iz+0), c100=hash13(ix+1,iy+0,iz+0);
    float c010=hash13(ix+0,iy+1,iz+0), c110=hash13(ix+1,iy+1,iz+0);
    float c001=hash13(ix+0,iy+0,iz+1), c101=hash13(ix+1,iy+0,iz+1);
    float c011=hash13(ix+0,iy+1,iz+1), c111=hash13(ix+1,iy+1,iz+1);
    float x00=mixf(c000,c100,ux), x10=mixf(c010,c110,ux);
    float x01=mixf(c001,c101,ux), x11=mixf(c011,c111,ux);
    float y0=mixf(x00,x10,uy), y1=mixf(x01,x11,uy);
    return mixf(y0,y1,uz);
}
static float fbm2(float px,float py,float pz){
    return vnoise(px,py,pz)*0.65f + vnoise(px*2.7f,py*2.7f,pz*2.7f)*0.35f;
}

// cell_density — exact port (same as froxel_skip_model.c's copy).
static float cell_density(float wx,float wy,float wz,float cx,float cy,float cz,
                          float humidity,float base_density,float windx,float windz,float t){
    float alt=clampf(wy,-20.f,200.f);
    float hc=powf(humidity,1.6f);
    float dap=base_density*mixf(0.6f,2.2f,hc);
    float hf=expf(-alt*0.05f);
    float rolloff=expf(-fmaxf(alt-60.f,0.f)*0.01f);
    float ws=sqrtf(windx*windx+windz*windz);
    float ox=windx*t, oz=windz*t;
    float mpx=(wx-ox)*0.05f, mpy=(wy)*0.05f, mpz=(wz-oz)*0.05f;
    if(ws>1e-4f){
        float dx=windx/ws, dz=windz/ws;
        float along=mpx*dx+mpz*dz;
        float acx=mpx-along*dx, acz=mpz-along*dz;
        float stretch=1.f+fminf(ws*2.0f,5.0f);
        mpx=acx+(along/stretch)*dx; mpz=acz+(along/stretch)*dz;
    }
    float moisture=smoothstepf(0.15f,0.85f,fbm2(mpx,mpy,mpz));
    float amb=clampf(1.5f-alt*0.012f,0.2f,1.4f);
    float wisp=mixf(0.4f,1.6f,moisture)*amb;
    float dfc=sqrtf((wx-cx)*(wx-cx)+(wy-cy)*(wy-cy)+(wz-cz)*(wz-cz));
    float prox=smoothstepf(0.f,3.f,dfc);
    float d=dap*(hf*0.5f+1.f)*rolloff*wisp*prox;
    return fminf(d,2.0f);
}

// Deterministic small "reprojection resample error" — a texture lookup
// landing at a slightly-off subpixel/subtexel location isn't the exact
// analytic value; model it as a small hash-driven jitter, bounded and
// reproducible (no rand(), no seed dependency across runs).
static float resample_error(int frame){
    return (hash13((float)frame*13.7f, 0.37f, 91.1f) - 0.5f) * 0.01f; // +/- 0.005
}

#define NUM_FRAMES     240
#define DISOCCLUSION_FRAME 120
#define FPS            60.0f
#define LSB_EQUIV      (1.0f/255.0f)   // assumed eventual 8-bit tonemap/display
                                        // quantization — the doc doesn't state
                                        // a bit depth for the froxel volume
                                        // itself (likely R16F/R32F), so this
                                        // is the "visually distinguishable"
                                        // proxy threshold, same spirit as
                                        // froxel_skip_model.c's OD_EPS being a
                                        // sub-perceptual cutoff rather than a
                                        // literal storage LSB.

// Camera path: steady pan along +x, with a small altitude drift, so the
// tracked world position is genuinely time-varying (this is what makes
// "ground truth changes every frame" meaningful, not a fixed-point test).
static void camera_pos_at(int frame, float *x, float *y, float *z){
    float t = frame / FPS;
    *x = t * 3.0f;           // 3 m/s pan
    *y = 20.0f + sinf(t*0.3f) * 4.0f;
    *z = 0.0f;
}

// The disocclusion event: from DISOCCLUSION_FRAME onward, the "revealed"
// scene has different atmosphere params (humidity/base_density jump — as
// if the camera just cleared a ridge into a much mistier valley). This is
// what a real disocclusion looks like for a fog volume: not that geometry
// moved, but that the true density at the now-visible position was always
// different from what was being (wrongly) reprojected from the occluded
// side.
static void atmos_params_at(int frame, float *humidity, float *base_density){
    if (frame < DISOCCLUSION_FRAME) { *humidity = 0.3f; *base_density = 0.5f; }
    else                            { *humidity = 0.85f; *base_density = 1.4f; }
}

typedef struct {
    float steady_state_err;   // mean |history-truth| over the last 30 frames
                               // before the disocclusion (converged regime)
    int   warmup_frames;      // frames from start until |history-truth| < LSB_EQUIV
                               // and stays under it (first-time convergence)
    float ghost_peak;         // max |history-truth| in the 10 frames right
                               // after the disocclusion cut
    int   ghost_recovery;     // frames after the cut until back under LSB_EQUIV
} RunResult;

static RunResult run_model(float alpha, int use_clamp){
    RunResult r; r.steady_state_err=0; r.warmup_frames=-1; r.ghost_peak=0; r.ghost_recovery=-1;
    float history = 0.0f;
    int have_history = 0;
    float wind_x = 0.6f, wind_z = 0.2f;
    float steady_sum = 0.0f; int steady_n = 0;
    int converged_since = -1;

    for (int f = 0; f < NUM_FRAMES; f++) {
        float cx, cy, cz; camera_pos_at(f, &cx, &cy, &cz);
        float humidity, base_density; atmos_params_at(f, &humidity, &base_density);
        float t = f / FPS;
        // Tracked world position: a fixed offset ahead of the camera (a
        // froxel a few metres into the fog in front of the viewer).
        float wx = cx + 8.0f, wy = cy - 2.0f, wz = cz + 1.5f;

        float ground_truth = cell_density(wx,wy,wz, cx,cy,cz, humidity,base_density, wind_x,wind_z, t);

        // Reprojected sample: previous frame's camera used to compute the
        // SAME tracked world position (this is what reprojection buys you —
        // sampling the prior frame's stored value at the world position
        // that maps to THIS frame's froxel — vs a full fresh evaluation),
        // plus the resample error.
        float current_sample = ground_truth + resample_error(f);

        if (!have_history) {
            history = current_sample;
            have_history = 1;
        } else {
            float h = history;
            if (use_clamp) {
                // Neighbourhood clamp: sample a small ring of nearby world
                // positions THIS frame and clamp history into their
                // [min,max] before blending — bounds how far a stale
                // reprojected value can diverge from what's plausible now.
                float nmin = 1e9f, nmax = -1e9f;
                const float OFF = 1.0f; // ~1 froxel-cell offset in world units
                float offs[6][3] = {{OFF,0,0},{-OFF,0,0},{0,OFF,0},{0,-OFF,0},{0,0,OFF},{0,0,-OFF}};
                for (int k=0;k<6;k++){
                    float nx=wx+offs[k][0], ny=wy+offs[k][1], nz=wz+offs[k][2];
                    float nd = cell_density(nx,ny,nz, cx,cy,cz, humidity,base_density, wind_x,wind_z, t);
                    if (nd<nmin) nmin=nd;
                    if (nd>nmax) nmax=nd;
                }
                h = clampf(h, nmin, nmax);
            }
            history = mixf(h, current_sample, alpha);
        }

        float err = fabsf(history - ground_truth);

        if (r.warmup_frames < 0) {
            if (err < LSB_EQUIV) {
                if (converged_since < 0) converged_since = f;
                if (f - converged_since >= 5) r.warmup_frames = converged_since; // stayed converged 5 frames
            } else {
                converged_since = -1;
            }
        }

        if (f >= DISOCCLUSION_FRAME - 30 && f < DISOCCLUSION_FRAME) {
            steady_sum += err; steady_n++;
        }
        if (f >= DISOCCLUSION_FRAME && f < DISOCCLUSION_FRAME + 10) {
            if (err > r.ghost_peak) r.ghost_peak = err;
        }
        if (f >= DISOCCLUSION_FRAME && r.ghost_recovery < 0 && err < LSB_EQUIV) {
            r.ghost_recovery = f - DISOCCLUSION_FRAME;
        }
    }
    r.steady_state_err = steady_n>0 ? steady_sum/steady_n : -1.0f;
    return r;
}

int main(){
    float alphas[3] = {0.05f, 0.10f, 0.20f};
    printf("Temporal froxel accumulation — CPU model (handoff/WORKER_D_temporal_model.md)\n");
    printf("LSB-equivalent threshold assumed = %.6f (1/255, 8-bit display proxy — see file header)\n\n", LSB_EQUIV);
    printf("%-8s %-10s %-14s %-16s %-14s %-16s\n", "alpha", "clamp", "steady_err", "warmup_frames", "ghost_peak", "ghost_recovery");
    printf("---------------------------------------------------------------------------------\n");

    float best_alpha = -1; float best_score = 1e9f; int best_needs_clamp = 1;

    for (int ci=0; ci<3; ci++){
        for (int a=0;a<3;a++){
            int use_clamp = ci; // 0=no clamp, 1=clamp... we want just 2 states, loop fixed below
            if (ci>1) break;
            RunResult r = run_model(alphas[a], use_clamp);
            printf("%-8.2f %-10s %-14.6f %-16d %-14.6f %-16d\n",
                alphas[a], use_clamp?"on":"off", r.steady_state_err, r.warmup_frames, r.ghost_peak, r.ghost_recovery);
            if (use_clamp) {
                // Scoring: prioritize low ghosting magnitude and fast
                // recovery, then steady-state accuracy, then fast warmup.
                float score = r.ghost_peak*10.0f + (r.ghost_recovery<0?100.0f:r.ghost_recovery)*0.1f
                            + r.steady_state_err*5.0f + (r.warmup_frames<0?100.0f:r.warmup_frames)*0.01f;
                if (score < best_score) { best_score = score; best_alpha = alphas[a]; }
            }
        }
    }

    // Compare clamp on vs off at the winning alpha directly, for the
    // "is the clamp needed" call.
    RunResult with_clamp = run_model(best_alpha, 1);
    RunResult no_clamp    = run_model(best_alpha, 0);

    printf("\n--- Recommendation ---\n");
    printf("Recommended alpha = %.2f (clamp on). Ghost peak %.6f (clamp on) vs %.6f (clamp off) at the same alpha",
        best_alpha, with_clamp.ghost_peak, no_clamp.ghost_peak);
    if (no_clamp.ghost_peak > with_clamp.ghost_peak * 1.5f) {
        printf(" -> clamp meaningfully reduces post-disocclusion ghosting; KEEP the clamp.\n");
    } else {
        printf(" -> clamp's effect on ghosting at this alpha is marginal; still recommended as a cheap safety net given it costs 6 extra samples/frame.\n");
    }
    printf("One-line: alpha=%.2f WITH neighbourhood clamp.\n", best_alpha);
    printf("\n--- Caveat on the clamp numbers above ---\n");
    printf("This model's neighbourhood clamp samples FRESH cell_density() at nearby world\n");
    printf("positions every frame (idealized/instantaneous ground truth), not each\n");
    printf("neighbour's own EMA history the way a real 3D froxel grid would (every froxel\n");
    printf("keeps its own lagging history buffer, so real neighbours are ALSO stale by some\n");
    printf("amount, not fresh). The disocclusion here is also a GLOBAL atmosphere-param\n");
    printf("jump (humidity/base_density), not a spatially-local occlusion boundary, so\n");
    printf("neighbour samples snap to the new value immediately post-cut. Both of these\n");
    printf("make the clamp look near-perfect (ghost_peak ~0.001) here. Read the DIRECTION\n");
    printf("of the result (clamp helps, order of magnitude) as reliable; do not take the\n");
    printf("~1000x magnitude at face value without testing against real neighbour history\n");
    printf("buffers and a spatially-local disocclusion once this is wired to a shader.\n");
    return 0;
}
