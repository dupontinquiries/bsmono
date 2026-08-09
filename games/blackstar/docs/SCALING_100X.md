# What 100× the character space actually costs

The web is not really about today's 500MB. It is about whether the character
space can grow two orders of magnitude without colliding with storage. So the
number that matters is the **marginal cost of the next character**, and it is
measurable.

## The part library saturates

Harvested at increasing corpus sizes, identical settings:

| models | part types | parts/model | marginal parts per new model |
|---:|---:|---:|---:|
| 30 | 3,788 | 126.3 | — |
| 60 | 6,461 | 107.7 | 89.1 |
| 120 | 11,879 | 99.0 | 90.3 |
| 240 | 13,900 | 57.9 | **16.8** |
| 1,012 | 40,742 | 40.3 | 34.8 |

Fitting the measured points: **parts = 434 × models^0.656**.

That exponent is the whole answer to the scaling question. Sublinear means
**100× the characters costs about 20× the parts**, not 100×, because new
vehicles are mostly made of parts that already exist. It is the thesis holding
at scale.

## But it does not fit yet

```
  1,012 models →    40,543 parts →   36 MB
 10,120 models →   183,444 parts →  161 MB
101,200 models →   830,034 parts →  730 MB
```

At today's **824 bytes of geometry per part**, 100× lands at ~730MB — over any
sane budget. The metadata is not the problem: the index is 71 B/part gzipped
against the payload's 824.

## Where the 824 bytes are, and the lever

Triangle counts are extremely skewed:

```
median   59 tris      p90  288      p99 2,022
top  5% of parts hold 57% of all triangles
top 25% of parts hold 80%
```

**Half the payload is in one part in twenty.** A per-part triangle budget —
decimating only the top 5% — is therefore the lever that reaches the target,
and it touches the parts where a lower resolution is least visible per byte
saved. Getting 824 down to ~450 B/part puts 100× at roughly 400MB.

That is the next experiment, and it has to be measured through `render_parity`
like everything else: decimation is exactly the kind of change that looks free
in a size table and shows up on screen.

## The engine side, done here

`forward.glsl` now decodes **both** normal formats:

```glsl
vec3 decode_normal(vec3 t) {
  if (t.b < 0.03) {                     // two-channel: derive Z
    vec2 nxy = t.rg * 2.0 - 1.0;
    return vec3(nxy, sqrt(max(0.0, 1.0 - dot(nxy, nxy))));
  }
  return t * 2.0 - 1.0;                 // three-channel: as stored
}
```

Both accepted rather than switching over, because the alternative is a flag
day — every normal map repacked in the same commit as the shader or the game
renders wrong. The test is `b`, not a uniform: a two-channel texture sampled as
RGB returns `b == 0` everywhere, and a real three-channel map never does, since
Z points out of the surface and encodes at or above 128.

Placed in the **FS** block. The first attempt put it in the vertex stage, where
it would not have compiled — the file splits on `#START VS` / `#START FS` and
both call sites are in the fragment half.

Unverified: the C engine does not build in this container. The Rust engine loads
glTF and needs the same decode at its own sampling site.
