
struct D { rows:u32,cols:u32,eps:f32,_p:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> wt: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let r=g.x; if(r>=d.rows){return;}
  let base=r*d.cols; var ss=0.0;
  for(var c=0u;c<d.cols;c=c+1u){let v=x[base+c]; ss=ss+v*v;}
  let inv=1.0/sqrt(ss/f32(d.cols)+d.eps);
  for(var c=0u;c<d.cols;c=c+1u){o[base+c]=x[base+c]*inv*wt[c];}
}