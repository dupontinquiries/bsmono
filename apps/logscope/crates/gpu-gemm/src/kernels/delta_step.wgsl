// Gated delta rule, ONE token. SINGLE read_write buffer `o` packs
// [ out: H_v*S | state M: H_v*S*S ] — state persists across tokens in place.
// Row (h,j) is fully independent:
//   M[j,:] *= eg_h;  err = be_h*(v[j] - sum_i M[j,i]*k[i]);
//   M[j,i] += err*k[i];  out[j] = sum_i M[j,i]*q[i]/sqrt(S).
// q,k from the GQA-shared k-head; gb = [eg, be] per v-head (precomputed on GPU).
struct D { s:u32,hv:u32,hk:u32,_a:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> k: array<f32>;
@group(0)@binding(2) var<storage,read> v: array<f32>;
@group(0)@binding(3) var<storage,read> gb: array<f32>;
@group(0)@binding(4) var<storage,read_write> o: array<f32>;
@group(0)@binding(5) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let id=g.x; if(id>=dd.hv*dd.s){return;}
  let h=id/dd.s; let j=id%dd.s;
  let kh=h/(dd.hv/dd.hk);
  let eg=gb[h*2u]; let be=gb[h*2u+1u];
  let mb=dd.hv*dd.s + h*dd.s*dd.s + j*dd.s;
  let scale=1.0/sqrt(f32(dd.s));
  var mk=0.0;
  for(var i=0u;i<dd.s;i=i+1u){ mk=mk+o[mb+i]*eg*k[kh*dd.s+i]; }
  let err=be*(v[h*dd.s+j]-mk);
  var oj=0.0;
  for(var i=0u;i<dd.s;i=i+1u){
    let m=o[mb+i]*eg + err*k[kh*dd.s+i];
    o[mb+i]=m;
    oj=oj+m*q[kh*dd.s+i]*scale;
  }
  o[h*dd.s+j]=oj;
}
