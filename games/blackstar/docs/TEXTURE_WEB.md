# The texture set has a material system nobody wrote down

298 PNGs, 53.8MB, addressed only by filename — and more structure in those names
than it looks. Textures come in **material sets**: a base plus optional `norm_`,
`spec_` and `emit_` siblings sharing its stem.

```
materials : 107   (53 with a full base+norm+spec)
emissive  :  23   (>5% of the surface lit)
```

`tools/texture_web.py` groups them and reduces each to inferred properties —
**107 materials described in 15KB** against 53.8MB of pixels:

| property | from | why it is the right statistic |
|---|---|---|
| `reflectivity`, `reflect_variation` | spec mean, spread | uniform bright = polished metal; dark and noisy = worn paint |
| `emissive_coverage` | fraction of emit above threshold | a small bright seam and a dim wash have similar MEANS and completely different looks |
| `saturation` | base channel spread | separates painted panels from bare metal, scale-free |
| `normal_strength` | norm spread | how much relief the surface actually claims |

It separates real cases: `w` and `ball` show 0.000 reflect_variation — uniformly
polished — where `human` varies at 0.185.

## A measurement that was wrong, and the one that held

A 32×32 downsample suggested **172 of 298 textures were near-uniform**, holding
26MB, which looked like an enormous compression win. **It was an artefact.**
Downsampling averages away high-frequency detail, which is exactly what a normal
map or a noise texture is made of. Checked at full resolution, **zero textures
are flat** — the largest channel span never drops below 8.

What held up is structural. A normal map stores a unit vector, so Z is derivable
from X and Y and need not be stored:

```
100 normal maps, RGB      30.85 MB
100 normal maps, XY only  23.18 MB   (-25%)
derived-Z error           median 0.2/255, p95 1.0/255
```

## Why that is NOT applied yet

I repacked all 100 and then checked the consumer. `forward.glsl` lines 158 and
271:

```glsl
vec3 normals = texture(u_norm, uv).rgb;
normals = normalize(normals * 2.0 - 1.0);
```

**The shader samples three channels.** A two-channel texture would render wrong,
and the saving is 7.7MB against breaking every normal-mapped surface in the
game. Reverted.

The change is small — reconstruct Z in the shader:

```glsl
vec2 nxy = texture(u_norm, uv).rg * 2.0 - 1.0;
vec3 normals = vec3(nxy, sqrt(max(0.0, 1.0 - dot(nxy, nxy))));
```

but it must land **with** the repack, and neither can be verified here: the C
engine does not build in this container. That pairing is the gate, and it is the
same shape as the netcode repoint — the edit is easy, the verification is what
is missing.
