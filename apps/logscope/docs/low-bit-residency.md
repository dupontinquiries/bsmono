# Scout at q2, GLM-5.2 at q1: what actually fits

All figures against the **measured** 31 GiB concurrent-buffer ceiling, not the 63 GiB `--gate-vram`
reports — the probe allocates and releases, the arena must hold.

## Scout (56.6 GiB measured at q4)

```
q4      4.5 bpw    56.6 GiB    over by 25.6
Q2_K    2.5625     32.2 GiB    over by  1.2     <- so close
q2      2.5        31.4 GiB    over by  0.4     <- closer
q2.25   2.25       28.3 GiB    FITS
IQ1_S   1.5625     19.7 GiB    FITS
```

**Q2_K misses by 1.2 GiB and a 2.5 bpw format by 0.4.** That is within the noise of what the KV cache
and transients occupy, so the honest statement is: *Scout at q2 fits or does not depending on context
length*, and no format between 2.25 and 2.5 bpw makes that comfortable.

## GLM-5.2 (136B)

```
q4      4.5 bpw    71.2 GiB    over by 40.2
Q2_K    2.5625     40.6 GiB    over by  9.6
IQ1_S   1.5625     24.7 GiB    FITS
```

**GLM-5.2 at IQ1_S is the one that clearly fits**, with 6 GiB of headroom. Q2_K does not come close.

## The accuracy side, and it is not encouraging for the simple format

`q2demote::relative_error` measured the naive 2-bit format — symmetric, one f16 scale per 32 — on
realistic weight distributions:

```
uniform noise        0.316
gaussian-ish         0.444    <- closest to real weights
sparse w/ outliers   1.659    <- error exceeds the signal
```

44% relative L2 is destructive. The mechanism is in the module: **one large weight sets the group's
scale and the other 31 land on the two inner levels**, which are then nowhere near where the mass
sits. Q2_K's per-sub-block *minimum* exists precisely to stop that, and at four levels there is no
headroom to absorb it any other way.

So a q2 attempt needs the Q2_K-shaped format — which is the 2.5625 bpw one that misses by 1.2 GiB.

## The reason to want it anyway

Resident is not just about memory. The GPU-routed MoE path is gated on:

```rust
t == 1 && m.stream.is_none() && !m.cfg.weight_before
```

Scout fails **both** conditions today. Resident satisfies `m.stream.is_none()`, leaving only
`weight_before` — which needs a pre-scale of one activation row, and is small because Scout routes
top-1.

**That matters more than the memory.** Scout's decode is 99.9% CPU-wait from one blocking readback
per layer, and the routed path removes it. Getting to resident is the cheaper half of getting off the
stalling path.

## What I would do, in order

1. **GLM-5.2 at IQ1_S first.** It fits with headroom, `dot_iq1_s` already exists as a bit-exact CPU
   reference, and it needs no new format — only a GPU kernel gated on `--gate-bytes`, now green.
2. **Measure Q2_K-shaped error before building it.** `relative_error` takes a format function; a
   sub-block-minimum variant can be scored against the same distributions in minutes, and if it is
   not well under 0.1 the memory question is moot.
3. **Scout at q2 only if that error is acceptable AND the 1.2 GiB can be found** — a shorter context,
   a smaller KV pool, or accepting that long conversations spill.

For Kimi-K3 or anything larger, the arithmetic is the same: divide the parameter count by the ceiling
and read off the bpw you need. At 31 GiB, a 200B model needs **1.24 bpw** — below IQ1_S, so it does
not fit resident at any published format and streaming is the only path.
