
struct D { t:u32,dm:u32,base:f32,_p:u32,hd:u32,neox:u32,msc:f32,rot:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> ff: array<f32>;
@group(0)@binding(2) var<storage,read> pos: array<u32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let half=dd.hd/2u; let rhalf=dd.rot/2u; let per=dd.dm/2u; let p=g.y*nwg.x*64u+g.x; if(p>=dd.t*per){return;}
  let tok=p/per; let within=p%per; let head=within/half; let i=within%half;
  let off=tok*dd.dm + head*dd.hd;
  // Partial rotary (phi3 family): only the first `rot` dims of each head rotate; the
  // tail [rot, hd) passes through unchanged. Threads are one-per-pair over the whole
  // head, so tail threads copy their two adjacent elements — every element written
  // exactly once. rot == hd (the default) reproduces the old kernel bit-for-bit,
  // including the base exponent, which llama.cpp defines over n_dims (= rot), not hd.
  if(i>=rhalf){
    let e0=off + dd.rot + (i-rhalf)*2u;
    o[e0]=x[e0]; o[e0+1u]=x[e0+1u];
    return;
  }
  let freq=f32(pos[tok])/(pow(dd.base,(2.0*f32(i))/f32(dd.rot))*ff[i]);
  let c=cos(freq)*dd.msc; let s=sin(freq)*dd.msc;
  // Branch-free arithmetic pairing (no select — removes any backend-specific miscompile surface):
  //   j0 = i*(2-neox)             -> NORM: 2i     NEOX: i
  //   j1 = j0 + 1 + neox*(half-1) -> NORM: 2i+1   NEOX: i+half
  let j0=off + i*(2u-dd.neox);
  let j1=j0 + 1u + dd.neox*(rhalf-1u);
  let x0=x[j0]; let x1=x[j1];
  o[j0]=x0*c-x1*s;
  o[j1]=x0*s+x1*c;
}