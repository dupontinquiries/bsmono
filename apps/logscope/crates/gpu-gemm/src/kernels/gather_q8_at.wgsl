
struct D { k:u32, id_off:u32, kb:u32, _c:u32 };
@group(0)@binding(0) var<storage,read> qw: array<u32>;
@group(0)@binding(1) var<storage,read> qs: array<f32>;
@group(0)@binding(2) var<storage,read> ids: array<u32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let c=g.x; if(c>=d.k){return;}
  let row=ids[d.id_off];
  let word=qw[row*(d.k/4u)+c/4u];
  let ub=extractBits(word,(c%4u)*8u,8u);
  let q=select(i32(ub), i32(ub)-256, ub>=128u);
  o[c]=qs[row*d.kb+(c/32u)]*f32(q);
}