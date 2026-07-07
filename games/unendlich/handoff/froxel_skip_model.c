// Measures the "skip empty froxels" early-out: what fraction of the
// per-slice 9-tap shadow lookups it removes, and the worst-case in-scatter
// error it introduces. Ports cell_density()'s exact math (incl. fbm + blob
// contrast + wind streak) and the log-Z step grid from froxel_fill.comp,
// in `float` to match the GPU. Marches representative daytime view rays
// across the frustum.
#include <stdio.h>
#include <math.h>
#include <stdlib.h>

static float fract1(float x){ return x - floorf(x); }
static float clampf(float x,float a,float b){ return x<a?a:(x>b?b:x); }
static float mixf(float a,float b,float t){ return a+(b-a)*t; }
static float smoothstepf(float e0,float e1,float x){ float t=clampf((x-e0)/(e1-e0),0.f,1.f); return t*t*(3.f-2.f*t); }

// hash13 / value_noise_3d / fbm2 — identical to the shader
static float hash13(float px,float py,float pz){
    float x=fract1(px*0.1031f), y=fract1(py*0.1031f), z=fract1(pz*0.1031f);
    float d = x*(z+31.32f)+y*(y+31.32f)+z*(x+31.32f); // p3 += dot(p3, p3.zyx+31.32)
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

#define FN 0.5f      // FROXEL_NEAR_M
#define FF 200.0f    // FROXEL_FAR_M
#define NZ 64
static float view_z(float zn){ return FN*powf(FF/FN, zn); }

// cell_density — exact port
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
static float hg(float c,float g){ float g2=g*g; return (1.f-g2)/powf(1.f+g2-2.f*g*c,1.5f); }

int main(){
    float steps[NZ];
    for(int z=0;z<NZ;z++) steps[z]=view_z((z+1)/(float)NZ)-view_z(z/(float)NZ);
    // representative daytime: sun at 45deg elevation toward +x
    float sunx=cosf(0.785f), suny=sinf(0.785f), sunz=0.f;
    float OD[3]={5e-5f,7e-5f,1e-4f};
    float humids[3]={0.2f,0.5f,0.8f};
    float alts[2]={70.f,120.f};
    float base=0.012f, windx=0.30f, windz=0.0f, t=137.0f; // base wind, arbitrary time
    int NR=4000;
    srand(12345);
    printf("FROXEL_NEAR=%.2f FAR=%.1f slices=%d   sun elev=45deg   wind=0.30\n",FN,FF,NZ);
    printf("near-field: %d of %d slices lie within 3 m (proximity-clear ramp)\n",
        ({int c=0; for(int z=0;z<NZ;z++) if(view_z((z+0.5f)/NZ)<3.f) c++; c;}), NZ);
    for(int oi=0;oi<3;oi++){
      printf("\n=== OD_EPS = %.0e ===\n",OD[oi]);
      for(int hi=0;hi<3;hi++){
        for(int ai=0;ai<2;ai++){
          long tot=0,skip=0; double maxerr=0;
          float cx=0,cy=alts[ai],cz=0;
          for(int r=0;r<NR;r++){
            float pitch=(-30.f+60.f*(rand()/(float)RAND_MAX))*0.0174533f;
            float yaw=6.2831853f*(rand()/(float)RAND_MAX);
            float dx=cosf(pitch)*cosf(yaw), dy=sinf(pitch), dz=cosf(pitch)*sinf(yaw);
            // shift camera xz around so we sample different blob regions
            float scx=cx+ (rand()/(float)RAND_MAX)*4000.f-2000.f;
            float scz=cz+ (rand()/(float)RAND_MAX)*4000.f-2000.f;
            float cosT=dx*(-sunx)+dy*(-suny)+dz*(-sunz); // view_dir . to_sun (to_cam path → use -dir)
            float phase=hg(clampf(-cosT,-1.f,1.f),0.6f); // view toward cam from cell; sign approx
            double rayerr=0; float trans=1.f;
            for(int z=0;z<NZ;z++){
              float d=view_z((z+0.5f)/NZ);
              float wx=scx+dx*d, wy=cy+dy*d, wz=scz+dz*d;
              float dens=cell_density(wx,wy,wz,scx,cy,scz,humids[hi],base,windx,windz,t);
              float od=dens*steps[z];
              tot++;
              if(!(od>OD[oi] && trans>2e-3f)){
                skip++;
                // in-scatter we WOULD have added (sun term, lit worst case shadow=1)
                float insc=1.0f*1.0f*phase*1.0f + 0.15f; // sun_color*shadow*phase*intensity + ambient
                if(insc<0) insc=0.15f;
                rayerr += (double)insc*od*trans;
              }
              trans*=expf(-od);
            }
            if(rayerr>maxerr) maxerr=rayerr;
          }
          printf("  humidity=%.1f alt=%.0f : skip %5.1f%% of shadow taps   worst-ray in-scatter error=%.5f (1 LSB=0.0039)\n",
            humids[hi],alts[ai],100.0*skip/tot,maxerr);
        }
      }
    }
    return 0;
}
