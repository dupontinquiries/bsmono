
struct D { n:u32,c:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> q: array<u32>;
@group(0)@binding(1) var<storage,read> scales: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; if(i>=dd.n*dd.c){return;}
  let row=i/dd.c; let col=i%dd.c;
  let word=q[row*(dd.c/4u)+(col/4u)];
  let raw=extractBits(word, (col%4u)*8u, 8u);
  let val=select(f32(raw), f32(raw)-256.0, raw>127u);
  o[i]=val*scales[row];
}