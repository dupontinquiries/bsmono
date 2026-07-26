// Row-wise L2 normalization: y = x / sqrt(sum(x^2) + eps). One thread per row
// (rows are heads: small count, short loops). Matches ggml_l2_norm.
struct D { rows:u32,cols:u32,eps:f32,_a:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let r=g.x; if(r>=dd.rows){return;}
  var s=0.0;
  for(var c=0u;c<dd.cols;c=c+1u){ let v=x[r*dd.cols+c]; s=s+v*v; }
  let inv=1.0/sqrt(s+dd.eps);
  for(var c=0u;c<dd.cols;c=c+1u){ o[r*dd.cols+c]=x[r*dd.cols+c]*inv; }
}
