
struct D { n_batch:u32,h:u32,w:u32,cin:u32,cout:u32,k:u32,stride:u32,pad:u32,hout:u32,wout:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> wt: array<f32>;
@group(0)@binding(2) var<storage,read> bias: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; let total=d.n_batch*d.hout*d.wout*d.cout; if(i>=total){return;}
  let co=i%d.cout; let rest1=i/d.cout;
  let wo=rest1%d.wout; let rest2=rest1/d.wout;
  let ho=rest2%d.hout; let n=rest2/d.hout;
  var acc=bias[co];
  for(var ci=0u;ci<d.cin;ci=ci+1u){
    for(var kh=0u;kh<d.k;kh=kh+1u){
      let hi_s=i32(ho*d.stride+kh)-i32(d.pad); if(hi_s<0||hi_s>=i32(d.h)){continue;}
      let hi=u32(hi_s);
      for(var kw=0u;kw<d.k;kw=kw+1u){
        let wi_s=i32(wo*d.stride+kw)-i32(d.pad); if(wi_s<0||wi_s>=i32(d.w)){continue;}
        let wi=u32(wi_s);
        let xv=x[((n*d.h+hi)*d.w+wi)*d.cin+ci];
        let wv=wt[((co*d.cin+ci)*d.k+kh)*d.k+kw];
        acc=acc+xv*wv;
      }
    }
  }
  o[i]=acc;
}