
struct D { h:u32,w:u32,c:u32,psz:u32 };
@group(0)@binding(0) var<storage,read> img: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let pw=d.w/d.psz; let ph=d.h/d.psz; let n=ph*pw; let plen=d.psz*d.psz*d.c;
  let i=g.y*nwg.x*64u+g.x; if(i>=n*plen){return;}
  let p=i/plen; let within=i%plen;
  let py=p/pw; let px=p%pw;
  let pc=within%d.c; let pp=within/d.c; let pyy=pp/d.psz; let pxx=pp%d.psz;
  let sy=py*d.psz+pyy; let sx=px*d.psz+pxx;
  o[i]=img[(sy*d.w+sx)*d.c+pc];
}