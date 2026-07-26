
struct D { n:u32, out_slot:u32, _b:u32, _c:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<u32>;
@group(0)@binding(2) var<uniform> d: D;
var<workgroup> sv: array<f32,256>;
var<workgroup> si: array<u32,256>;
@compute @workgroup_size(256,1,1)
fn main(@builtin(local_invocation_id) lid:vec3<u32>){
  let t = lid.x;
  var bv = -3.4028235e38;
  var bi = 0u;
  var i = t;
  loop {
    if (i >= d.n) { break; }
    let v = x[i];
    if (v > bv) { bv = v; bi = i; }
    i = i + 256u;
  }
  sv[t] = bv; si[t] = bi;
  workgroupBarrier();
  var stride = 128u;
  loop {
    if (stride == 0u) { break; }
    if (t < stride) {
      let ov = sv[t + stride]; let oi = si[t + stride];
      if (ov > sv[t] || (ov == sv[t] && oi < si[t])) { sv[t] = ov; si[t] = oi; }
    }
    workgroupBarrier();
    stride = stride >> 1u;
  }
  if (t == 0u) { o[d.out_slot] = si[0]; }
}