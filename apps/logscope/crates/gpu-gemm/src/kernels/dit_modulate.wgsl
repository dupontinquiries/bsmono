
struct D { rows:u32,cols:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> scale: array<f32>;
@group(0)@binding(2) var<storage,read> shift: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; if(i>=d.rows*d.cols){return;}
  let c=i%d.cols; o[i]=x[i]*(1.0+scale[c])+shift[c];
}