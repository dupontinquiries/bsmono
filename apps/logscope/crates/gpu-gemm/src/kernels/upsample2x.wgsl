
struct D { n_batch:u32,h:u32,w:u32,c:u32,_a:u32,_b:u32,_c:u32,_d:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; let h2=d.h*2u; let w2=d.w*2u; let total=d.n_batch*h2*w2*d.c; if(i>=total){return;}
  let cc=i%d.c; let rest1=i/d.c;
  let wo=rest1%w2; let rest2=rest1/w2;
  let ho=rest2%h2; let n=rest2/h2;
  let hi=ho/2u; let wi=wo/2u;
  o[i]=x[((n*d.h+hi)*d.w+wi)*d.c+cc];
}