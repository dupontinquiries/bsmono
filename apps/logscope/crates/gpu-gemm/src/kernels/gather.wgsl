
struct D { n:u32,d:u32,id_off:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> table: array<f32>;
@group(0)@binding(1) var<storage,read> ids: array<u32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let p=g.x; if(p>=dd.n*dd.d){return;}
  let i=p/dd.d; let c=p%dd.d;
  o[p]=table[ids[dd.id_off+i]*dd.d+c];
}