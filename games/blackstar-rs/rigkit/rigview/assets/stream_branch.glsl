// EXTRACTED from the page by evaluating its voxHook.
float vhash(vec3 p){ return fract(sin(dot(p, vec3(127.1,311.7,74.7)))*43758.5453); }
//@@BRANCH
if (uMorph > 0.5) {
  float mL = dot(transformed, uAxL), mM = dot(transformed, uAxM), mT = dot(transformed, uAxT);
  float ms = clamp(mL / max(uHalf.x, 0.001) * 0.5 + 0.5, 0.0, 1.0);
  float mk = uMorphT * uMorphT * (3.0 - 2.0 * uMorphT);
  if (uMorph < 1.5) {
    float taper = pow(1.0 - ms, 1.25);
    mM *= mix(1.0, 0.12 + 0.88 * taper, mk);
    mT *= mix(1.0, 0.40 + 0.60 * taper, mk);
    mL *= mix(1.0, 1.20, mk);
  } else {
    mM *= mix(1.0, 0.22, mk);
    mT *= mix(1.0, 0.35, mk);
  }
  transformed = uAxL * mL + uAxM * mM + uAxT * mT;
}
if (uStream > 0.5) {
 if (uVox > 0.001) {
  float cell = 5.0;
  vec3 q = floor(aCell / cell + 0.5) * cell;
  vec3 off = transformed - q;
  float h = vhash(q);
  float CS = 16.0;
  vec3 qc = floor(q / CS + 0.5) * CS;
  float hc = vhash(qc + 5.3);
  float dA = floor(h * 5.0) / 5.0 * 0.26;
  float pA = clamp((uVox - dA) / 0.07, 0.0, 1.0);
  float eA = pA * pA * pA;
  vec3 chunk = qc + (q - qc) * 0.82;
  float dB = 0.18 + hc * 0.22;
  float pB = clamp((uVox - dB) / 0.09, 0.0, 1.0);
  float eB = pB * pB * pB;
  vec3 brick = aAnchor + (q - aAnchor) * 0.28;
  vec3 posA = mix(q, chunk, eA);
  vec3 posB = mix(posA, brick, eB);
  float buzzA = smoothstep(dA - 0.05, dA, uVox) * (1.0 - step(dA, uVox));
  float buzzB = smoothstep(dB - 0.05, dB, uVox) * (1.0 - step(dB, uVox));
  vec3 n = vec3(vhash(q + 3.1), vhash(q + 7.7), vhash(q + 13.3)) - 0.5;
  vec3 jit = n * cell * 0.7 * max(buzzA, buzzB) * sin(uTime * 70.0 + h * 50.0);
  float tA = uVox - (dA + 0.07);
  float tB = uVox - (dB + 0.10);
  vec3 travA = normalize(chunk - q + vec3(0.0001));
  vec3 travB = normalize(brick - posA + vec3(0.0001));
  float kA = step(0.0, tA) * exp(-max(tA, 0.0) * 40.0) * sin(max(tA, 0.0) * 90.0);
  float kB = step(0.0, tB) * exp(-max(tB, 0.0) * 40.0) * sin(max(tB, 0.0) * 90.0);
  vec3 kick = travA * cell * 0.5 * kA + travB * cell * 0.8 * kB;
  float sC = 0.46 + h * 0.34;
  float cD = clamp((uVox - sC) / 0.20, 0.0, 1.0);
  vec3 cube = mix(off, sign(off) * cell * 0.48, min(1.0, cD * 3.0));
  float lv = floor(min(cD * 4.0, 3.999));
  float fl = cD * 4.0 - lv;
  float halvings = lv + smoothstep(0.62, 1.0, fl);
  float shrink = pow(0.5, halvings) * (1.0 - smoothstep(0.93, 1.0, cD));
  float scl = cell * pow(0.5, lv);
  vec3 sub = (floor(off / scl) + 0.5) * scl;
  vec3 subCorner = sub + sign(off - sub + vec3(0.0001)) * scl * 0.48;
  vec3 piece = mix(cube, subCorner, min(1.0, lv));
  vec3 ax = normalize(n + vec3(0.0001, 0.0, 0.0));
  float snapAmt = min(1.0, cD * 3.0);
  float ang = snapAmt * (1.5708 * (eA + eB) + 0.8 * halvings);
  vec3 rc = piece * cos(ang) + cross(ax, piece) * sin(ang)
          + ax * dot(ax, piece) * (1.0 - cos(ang));
  transformed = posB + jit + kick + rc * shrink;
 }
 int bi = int(aBone + 0.5);
 vec4 bq = uBQ[bi];
 transformed = transformed + 2.0 * cross(bq.xyz, cross(bq.xyz, transformed) + bq.w * transformed) + uBT[bi].xyz;
}
