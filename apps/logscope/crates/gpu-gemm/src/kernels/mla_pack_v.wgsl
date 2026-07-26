
struct D { t:u32, nh:u32, qk_nope:u32, qk_rope:u32, v_head:u32, _a:u32, _b:u32, _c:u32 };
@group(0)@binding(0) var<storage,read> rec: array<f32>;
@group(0)@binding(1) var<storage,read_write> vout: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let id=g.x; let n=d.t*d.nh; if(id>=n){return;}
  let row=id/d.nh; let hh=id%d.nh;
  let recw=d.nh*(d.qk_nope+d.v_head); let recb=row*recw+hh*(d.qk_nope+d.v_head);
  let vb=row*(d.nh*d.v_head)+hh*d.v_head;
  for(var c=0u;c<d.v_head;c=c+1u){ vout[vb+c]=rec[recb+d.qk_nope+c]; }
}