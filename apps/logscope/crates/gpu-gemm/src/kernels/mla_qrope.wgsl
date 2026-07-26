
struct D { nh:u32, qk_nope:u32, qk_rope:u32, pos:u32, base:f32, _a:u32, _b:u32, _c:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let qk=d.qk_nope+d.qk_rope; let half=d.qk_rope/2u;
  let n=d.nh*qk;
  let id=g.x; if(id>=n){return;}
  let hh=id/qk; let c=id%qk;
  // copy through by default; overwrite the rope-half pairs
  o[id]=x[id];
  if(c>=d.qk_nope && c<d.qk_nope+half){
    let i=c-d.qk_nope;
    let base_i=hh*qk+d.qk_nope;
    let freq=f32(d.pos)/pow(d.base,(2.0*f32(i))/f32(d.qk_rope));
    let cs=cos(freq); let sn=sin(freq);
    let x0=x[base_i+i]; let x1=x[base_i+i+half];
    o[base_i+i]=x0*cs-x1*sn;
    o[base_i+i+half]=x0*sn+x1*cs;
  }
}