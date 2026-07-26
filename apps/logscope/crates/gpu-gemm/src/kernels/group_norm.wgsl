
struct D { n_batch:u32,hw:u32,c:u32,groups:u32,eps:f32,_a:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> gamma: array<f32>;
@group(0)@binding(2) var<storage,read> beta: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; if(i>=d.n_batch*d.groups){return;}
  let gi=i%d.groups; let n=i/d.groups;
  let cpg=d.c/d.groups; let c0=gi*cpg;
  var mean=0.0; var cnt=0.0;
  for(var p=0u;p<d.hw;p=p+1u){ for(var cc=0u;cc<cpg;cc=cc+1u){ mean=mean+x[(n*d.hw+p)*d.c+c0+cc]; cnt=cnt+1.0; } }
  mean=mean/cnt;
  var v=0.0;
  for(var p=0u;p<d.hw;p=p+1u){ for(var cc=0u;cc<cpg;cc=cc+1u){ let dd=x[(n*d.hw+p)*d.c+c0+cc]-mean; v=v+dd*dd; } }
  v=v/cnt; let inv=1.0/sqrt(v+d.eps);
  for(var p=0u;p<d.hw;p=p+1u){
    for(var cc=0u;cc<cpg;cc=cc+1u){
      let idx=(n*d.hw+p)*d.c+c0+cc;
      o[idx]=(x[idx]-mean)*inv*gamma[c0+cc]+beta[c0+cc];
    }
  }
}