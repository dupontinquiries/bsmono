
struct D { t:u32,d:u32,base:f32,pos0:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let half=dd.d/2u; let p=g.x; if(p>=dd.t*half){return;}
  let row=p/half; let i=p%half;
  let freq=f32(row+dd.pos0)/pow(dd.base,(2.0*f32(i))/f32(dd.d));
  let c=cos(freq); let s=sin(freq);
  let d0=row*dd.d+2u*i;
  let x0=x[d0]; let x1=x[d0+1u];
  o[d0]=x0*c-x1*s;
  o[d0+1u]=x0*s+x1*c;
}