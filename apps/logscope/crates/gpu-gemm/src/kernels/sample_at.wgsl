
struct D { n:u32, out_slot:u32, top_k:u32, rlast:u32,
           temp:u32, top_p:u32, min_p:u32, penalty:u32,
           seeded:u32, rcap:u32, kids:u32, abs_step:u32,
           _p0:u32, _p1:u32, _p2:u32, _p3:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> uni: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<u32>;
@group(0)@binding(3) var<uniform> d: D;
var<workgroup> sv: array<f32,256>;
var<workgroup> si: array<u32,256>;
var<workgroup> cand_v: array<f32,128>;
var<workgroup> cand_i: array<u32,128>;
var<workgroup> ov_i: array<u32,64>;
var<workgroup> ov_v: array<f32,64>;
var<workgroup> n_over: u32;
var<workgroup> last_v: f32;
var<workgroup> last_i: u32;
@compute @workgroup_size(256,1,1)
fn main(@builtin(local_invocation_id) lid:vec3<u32>){
  let t = lid.x;
  let temp = bitcast<f32>(d.temp);
  let pen  = bitcast<f32>(d.penalty);
  // ── phase 1 (thread 0, serial): replay the CPU penalty over the last rlast ring entries ──
  if (t == 0u) {
    n_over = 0u;
    let valid = min(d.rcap, d.seeded + d.abs_step);
    let neff = min(d.rlast, valid);
    if (pen != 1.0 && neff > 0u) {
      let head = d.seeded + d.abs_step; // next write position (ring index modulo rcap)
      var j: u32 = neff;
      loop { // oldest → newest, exactly the CPU slice order (order is commutative but kept anyway)
        if (j == 0u) { break; }
        let tok = o[d.kids + ((head - j) % d.rcap)];
        if (tok < d.n) {
          // current value: staged override if present, else the raw logit
          var cur = x[tok]; var slot = n_over;
          var q = 0u;
          loop { if (q >= n_over) { break; }
                 if (ov_i[q] == tok) { cur = ov_v[q]; slot = q; break; } q = q + 1u; }
          if (cur > 0.0) { cur = cur / pen; } else { cur = cur * pen; }
          ov_i[slot] = tok; ov_v[slot] = cur;
          if (slot == n_over) { n_over = n_over + 1u; }
        }
        j = j - 1u;
      }
    }
    last_v = 3.4028235e38; last_i = 0xffffffffu; // sentinel: nothing picked yet
  }
  workgroupBarrier();
  // ── phase 2: iterate raw maxima in exact (value desc, index asc) order ──
  let want = min(d.top_k + n_over, min(128u, d.n));
  var c: u32 = 0u;
  loop {
    if (c >= want) { break; }
    var bv = -3.4028235e38;
    var bi = 0xffffffffu;
    var i = t;
    loop {
      if (i >= d.n) { break; }
      // NaN/Inf guard (Colibri #369): a poisoned logit must not be selected. Reading non-finite as
      // -inf excludes it from the max so the pick stays a finite, in-range token id (a lone +Inf would
      // otherwise win and skew the softmax; a NaN sentinel could leave bi out of range).
      var v = x[i];
      if (!(v == v) || v > 3.0e38 || v < -3.0e38) { v = -3.4028235e38; }
      // exclusion by order: already-picked ⇔ NOT (v < last_v || (v == last_v && i > last_i))
      if (v < last_v || (v == last_v && i > last_i)) {
        if (v > bv || (v == bv && i < bi)) { bv = v; bi = i; }
      }
      i = i + 256u;
    }
    sv[t] = bv; si[t] = bi;
    workgroupBarrier();
    var stride = 128u;
    loop {
      if (stride == 0u) { break; }
      if (t < stride) {
        let ovv = sv[t + stride]; let oii = si[t + stride];
        if (ovv > sv[t] || (ovv == sv[t] && oii < si[t])) { sv[t] = ovv; si[t] = oii; }
      }
      workgroupBarrier();
      stride = stride >> 1u;
    }
    if (t == 0u) {
      cand_v[c] = sv[0]; cand_i[c] = si[0];
      last_v = sv[0]; last_i = si[0];
    }
    workgroupBarrier();
    c = c + 1u;
  }
  // ── phase 3 (thread 0, serial): overrides → stable re-sort → the CPU sampler tail verbatim ──
  if (t == 0u) {
    // apply penalty overrides to the candidate set
    var a: u32 = 0u;
    loop { if (a >= want) { break; }
      var q = 0u;
      loop { if (q >= n_over) { break; }
        if (ov_i[q] == cand_i[a]) { cand_v[a] = ov_v[q]; break; } q = q + 1u; }
      a = a + 1u; }
    // insertion sort by (value desc, index asc) — the CPU's stable descending order
    a = 1u;
    loop { if (a >= want) { break; }
      let vv = cand_v[a]; let ii = cand_i[a];
      var b = a;
      loop { if (b == 0u) { break; }
        let pv = cand_v[b - 1u]; let pi = cand_i[b - 1u];
        if (pv > vv || (pv == vv && pi < ii)) { break; }
        cand_v[b] = pv; cand_i[b] = pi; b = b - 1u; }
      cand_v[b] = vv; cand_i[b] = ii;
      a = a + 1u; }
    let k = min(d.top_k, want);
    // temperature softmax over the k picks (same op order as CPU: exp((v-mx)/T), sum, divide)
    let mx = cand_v[0];
    var sum = 0.0;
    var pi2: u32 = 0u;
    loop { if (pi2 >= k) { break; }
      let e = exp((cand_v[pi2] - mx) / temp);
      cand_v[pi2] = e; sum = sum + e; pi2 = pi2 + 1u; }
    pi2 = 0u;
    loop { if (pi2 >= k) { break; } cand_v[pi2] = cand_v[pi2] / sum; pi2 = pi2 + 1u; }
    var cut = k;
    let minp = bitcast<f32>(d.min_p);
    if (minp > 0.0) {
      let thresh = minp * cand_v[0];
      var f = cut; pi2 = 0u;
      loop { if (pi2 >= cut) { break; }
        if (cand_v[pi2] < thresh) { f = pi2; break; } pi2 = pi2 + 1u; }
      cut = max(f, 1u);
      var s2 = 0.0; pi2 = 0u;
      loop { if (pi2 >= cut) { break; } s2 = s2 + cand_v[pi2]; pi2 = pi2 + 1u; }
      pi2 = 0u;
      loop { if (pi2 >= cut) { break; } cand_v[pi2] = cand_v[pi2] / s2; pi2 = pi2 + 1u; }
    }
    let topp = bitcast<f32>(d.top_p);
    if (topp < 1.0) {
      var cum = 0.0; var f = cut; pi2 = 0u;
      loop { if (pi2 >= cut) { break; }
        cum = cum + cand_v[pi2];
        if (cum >= topp) { f = pi2 + 1u; break; } pi2 = pi2 + 1u; }
      cut = f;
      var s3 = 0.0; pi2 = 0u;
      loop { if (pi2 >= cut) { break; } s3 = s3 + cand_v[pi2]; pi2 = pi2 + 1u; }
      pi2 = 0u;
      loop { if (pi2 >= cut) { break; } cand_v[pi2] = cand_v[pi2] / s3; pi2 = pi2 + 1u; }
    }
    let r = uni[d.out_slot];
    var picked = cand_i[cut - 1u];
    var cum2 = 0.0; pi2 = 0u;
    loop { if (pi2 >= cut) { break; }
      cum2 = cum2 + cand_v[pi2];
      if (r < cum2) { picked = cand_i[pi2]; break; } pi2 = pi2 + 1u; }
    o[d.out_slot] = picked;
    o[d.kids + ((d.seeded + d.abs_step) % d.rcap)] = picked; // extend the ring for the NEXT step
  }
}