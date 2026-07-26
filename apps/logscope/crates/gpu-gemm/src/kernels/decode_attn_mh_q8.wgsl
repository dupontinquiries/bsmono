
// `start` is the first KV row to attend to — sliding-window attention for a single query position is a
// contiguous range, so it is a loop bound rather than a mask. Passed as an offset instead of binding a
// sub-view because the q8 cache keeps ONE SCALE PER ROW in a separate tensor, and a view into that would
// need 64-row alignment which a sliding window almost never has. start = 0 is the unwindowed case.
struct D { l:u32,hd:u32,nh:u32,nkv:u32,start:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> kq: array<u32>;
@group(0)@binding(2) var<storage,read> ks: array<f32>;
@group(0)@binding(3) var<storage,read> vq: array<u32>;
@group(0)@binding(4) var<storage,read> vs: array<f32>;
@group(0)@binding(5) var<storage,read_write> o: array<f32>;
@group(0)@binding(6) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let hh=g.x; if(hh>=dd.nh){return;}
  let dm=dd.nh*dd.hd; let kvdm=dd.nkv*dd.hd; let kvw=kvdm/4u;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;
  let scale=1.0/sqrt(f32(dd.hd));
  var mx=-3.4e38;
  for(var j=dd.start;j<dd.l;j=j+1u){
    let ksc=ks[j]; var dot=0.0;
    for(var c=0u;c<dd.hd;c=c+1u){
      let idx=kb+c; let word=kq[j*kvw + idx/4u];
      let ub=extractBits(word,(idx%4u)*8u,8u); let qi=select(i32(ub), i32(ub)-256, ub>=128u);
      dot=dot+q[b+c]*(f32(qi)*ksc);
    }
    mx=max(mx,dot*scale);
  }
  for(var c=0u;c<dd.hd;c=c+1u){o[b+c]=0.0;}
  var sum=0.0;
  for(var j=dd.start;j<dd.l;j=j+1u){
    let ksc=ks[j]; let vsc=vs[j]; var dot=0.0;
    for(var c=0u;c<dd.hd;c=c+1u){
      let idx=kb+c; let word=kq[j*kvw + idx/4u];
      let ub=extractBits(word,(idx%4u)*8u,8u); let qi=select(i32(ub), i32(ub)-256, ub>=128u);
      dot=dot+q[b+c]*(f32(qi)*ksc);
    }
    let p=exp(dot*scale-mx); sum=sum+p;
    for(var c=0u;c<dd.hd;c=c+1u){
      let idx=kb+c; let word=vq[j*kvw + idx/4u];
      let ub=extractBits(word,(idx%4u)*8u,8u); let qi=select(i32(ub), i32(ub)-256, ub>=128u);
      o[b+c]=o[b+c]+p*(f32(qi)*vsc);
    }
  }
  for(var c=0u;c<dd.hd;c=c+1u){o[b+c]=o[b+c]/sum;}
}