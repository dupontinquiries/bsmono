# gate-q4k: one ULP, five wrong theories, and what the evidence actually constrains

## The finding, stated precisely

```
[✓] GPU dequant == CPU dequant: max|diff|=0.000e0, 0 differing
[✓] packed nibbles identical to the CPU path (0 words differ)
[✗] block scales identical: max|diff|=1.192e-7

    cpu amax is element 20 of block 0, value 2.206783295
    gpu implies amax                     2.206783533
    elements matching the GPU's implied amax: [(8, 2.2067833), (20, 2.2067833)]
```

Same block, same element, one ULP apart.

## What five measurements have ruled out

**Not the fma in the dequant.** Explicit `fma()` on both GPU paths and `mul_add` on the CPU changed
nothing. Four source changes produced byte-identical output.

**Not a second unfixed site.** Both GPU dequant expressions are now explicit `fma`; the only other CPU
site is Q5_K (it carries a `+ hi` high-bit term).

**Not block indexing.** The amax lives at element 20 of block 0 on both sides.

**Not the f16 decode.** The shader's decoder is the standard bit-shuffle and is exact for normal
values, which `d` always is here.

**Not the scale rule.** Both sides compute `amax / 7.0`.

## What it still constrains

Two GPU dequant paths exist: the standalone `Q4K_DEQUANT` the gate compares (which matches the CPU
exactly) and the inline `wq` inside `Q4K_TO_Q4` that the converter uses. **The gate tests a path the
converter does not take.** That much is certain and is itself worth fixing — a gate reporting "exact"
about the wrong function is how this hid for four runs.

The remaining candidates, in order of what I would check:

1. **`scale_min`'s 6-bit extraction** producing a different `sm.x` than the CPU's `sc1` for this
   sub-block. Both are integer, so a difference would be a *packing* error, not a rounding one — and a
   packing error that only shifts some blocks is consistent with 57 of 112 differing.
2. **The CPU's `sc1` derivation**, for the same reason and from the other side.

The decisive print is `d`, `sm.x` and `nib` for element 20 on both sides. If `sm.x != sc1`, it is
packing; if they match and the products differ, it is the multiply, and nothing in either source
explains that.

## The honest cost note

Five theories, five wrong, four of them shipped as fixes that each cost a run. Every one reasoned from
the source instead of measuring, and the measurements — when I finally wrote them — were four lines
each and eliminated a theory apiece.

**And there is a route that deletes the question.** A `MATVEC_Q4K` reading the file's blocks directly
removes the converter, both dequant paths, this gate, and the 2.88x memory inflation
`conversion_cost` measured for IQ1_S. A path you do not have cannot disagree with its reference.

That is a larger job than one more diagnostic, and it is probably the better one. This bug is worth
one more print; it is not worth a sixth guess.


---

# RESOLVED: the arithmetic is identical and the compiler is not

The bit-level run closed the chain:

```
cpu amax bits 0x400D3BF0   gpu implied amax bits 0x400D3BF1   ulps apart: 1
cpu scale bits 0x3EA16912  gpu scale bits 0x3EA16913
element 8 bits 0x400D3BF0  element 20 bits 0x400D3BF0   IDENTICAL — the max is ambiguous
```

`0x400D3BF0 / 7 = 0x3EA16912` and `0x400D3BF1 / 7 = 0x3EA16913`, so **the GPU's amax is exactly one
ULP above the CPU's** — and no element has that value. The GPU's `wq` produces a higher result than
the standalone `Q4K_DEQUANT` for the same weight, while that standalone path matches the CPU exactly.

**The two GPU dequant paths disagree with each other.**

And a line-by-line comparison finds no difference to blame:

* `scale_min` — byte-for-byte identical in both shaders
* the `d` and `dmin` reads — identical
* the final multiply-subtract — now explicit `fma` in both
* the `f16` decoder — the standard bit-shuffle, exact for normals

## Which is the finding

Two textually identical computations in different shaders round differently on this driver.
Contraction is decided per compilation unit, so the same source can fuse in one shader and not in
another depending on surrounding loop structure, inlining and register pressure. **No amount of
reading either shader will locate this, because the difference is not in either shader.**

That also explains why six theories died: every one of them assumed the divergence was written down
somewhere.

## The fix is structural

Stop computing the same thing twice.

1. **One dequant, shared.** Extract the block into a single WGSL string concatenated into both
   shaders, so there is one text and one compilation of it per pipeline — this reduces the exposure
   but does not eliminate it, since they are still separate pipelines.
2. **Or better: do not derive the scale on the GPU at all.** The converter needs `amax` only to pick a
   q4 scale. If the CPU's scale is uploaded alongside the weights, the GPU never reduces and the
   question cannot arise.
3. **Or best: `MATVEC_Q4K`.** Consume the file's blocks directly and delete the converter, both
   dequant paths, this gate, and the 2.88x memory inflation `conversion_cost` measured for IQ1_S.

Option 2 is small and closes this specific bug. Option 3 is the one worth doing.

## Still separate, still open

`297430 words differ` at grid scale and `2771` across chunk boundaries are **not** this bug. An ULP
flips the handful of nibbles sitting on a boundary — which is the small-tensor result, where 0 words
differ. A count that explodes with grid size is a dispatch or chunk-offset error and wants its own
investigation.


---

# FIXED, and the separate-bug claim was mostly wrong

The reciprocal fix landed:

```
[✓] block scales identical: max|diff|=0.000e0
[✓] matvec through GPU-converted weights == CPU-converted: max|diff|=0.000e0
[✗] large tensor:  297430 -> 1104 words differ
[✗] chunk boundaries: 2771 -> 114 words differ
```

**99.6% of the grid failures were downstream of the ULP.** I argued twice that they had to be a
separate bug because "an ULP cannot produce 297430 words", and that reasoning was wrong: at 70000
super-blocks, enough values sit near a quantization boundary that a single-ULP scale shift flips
hundreds of thousands of nibbles. The small tensor showed 0 differing words only because 112 blocks is
too few for any of them to land there.

The lesson is the same one as the seven theories, in a different costume: **I reasoned about what an
ULP "could" produce instead of measuring what it did.** The measurement was one fix away the whole
time.

## What actually remains

1104 words at 70000 super-blocks, 114 across chunk boundaries. Small enough to locate rather than
theorise about, and the gate now prints their indices, the gaps between them, the last one, and their
positions within a 32-word block.

The pattern names the class without another guess:

* **regular gaps** — a boundary or stride error
* **all past a fixed index** — a dispatch or grid-limit error
* **scattered, one position per block** — a rounding straggler after all

---

# The last 276: not the scale, and not arithmetic

```
gaps: 8096 exactly, 276 times
positions within a 32-word block: {31}
super-block 245 scale idx [21, 52, 19, 50, 16, 31, 46, 45]
12 scale bytes: [85, 116, 147, 178, 209, 240, 18, 49, 80, 111, 142, 173]
```

**The extraction is correct.** Hand-computing the ggml rule against those twelve bytes reproduces the
printed indices exactly, `j=7` included — `sc = (q[11] & 0xF) | ((q[3] >> 6) << 4) = 45`,
`mn = (q[11] >> 4) | ((q[7] >> 6) << 4) = 10`. Both sides implement it the same way.

**And it cannot be the scale anyway.** Words 28, 29, 30 and 31 all belong to sub-block 7 and share one
scale. A wrong scale corrupts all four. **Only word 31 differs**, in every one of the 276 occurrences.

## What that leaves

Word 31 is weights 248..255 — the final eight of the super-block. Their nibbles live in the **last four
bytes of the 144-byte block**, which is also the last four bytes before the next block begins.

So the remaining candidates are about the **end of the block**, not its contents:

1. A read that straddles the block boundary — `byte_at` fetches a `u32` word, and the final bytes of
   one block share a word with the first bytes of the next when the 144-byte stride is not
   word-aligned. **144 is divisible by 4**, so this should not happen — which is exactly the kind of
   "should" that has been wrong eight times here.
2. The nibble-packing loop's last iteration, where `w == 3` and `b == 7` reach the final byte.
3. The period is the DATA — `(i * 31) % 253` — so whatever it is, it is triggered by specific byte
   values arriving at that position, not by the position alone.

The decisive print is the raw `qs` bytes for weights 248..255 alongside both sides' nibbles for word
31. That distinguishes a bad READ from a bad PACK, and it is one line.

## Trajectory

```
297430  ->  1104  ->  9680  ->  276
        scale ULP   my revert   quantizer reciprocal
                    (ties-even)
```

Nine theories, five measurements. Every measurement narrowed it; four of the theories widened it.

---

# The 276: one nibble, one level, and a contradiction

```
qs bytes for weights 248..255: [0, 31, 62, 93]
gpu word 31 = 0x56781234   cpu word 31 = 0x56782234
gpu nibbles [4, 3, 2, 1, 8, 7, 6, 5]
cpu nibbles [4, 3, 2, 2, 8, 7, 6, 5]
cpu dequantized 248..255: [-8605444, -10756804, -11832484, -13983844, -3.8, -1075683.8, -3227043.8, -5378404]
```

**One nibble differs, by exactly one level, at the largest-magnitude value in the word.** Nibble 1 is
q=-7 (the clamp); nibble 2 is q=-6.

## What that rules out

**Not reciprocal error.** At scale ≈ 2e6 the f32 reciprocal is ≈ 5.0058e-7 and one ULP of it moves the
quotient by ~1e-6. The quotient is -6.9999995, which rounds to -7 on either side. Reciprocal precision
cannot span a level here.

**Not the tie rule.** -6.9999995 is not a tie.

**Not the scale.** Words 28..31 share sub-block 7's scale and only word 31 differs.

## The contradiction

For the CPU to produce q=-6 where the GPU produces q=-7, its `amax` would need to be about **16%
larger** — 1.63e7 against 1.40e7. That is not a rounding difference, it is a different set of values.

**But the `wq` probe reported 0 of 3584 differing.** The two sides dequantize identically and then
reduce to different maxima, which cannot both be true.

Possible resolutions, none yet measured:

1. The probe covered a small tensor and the large one diverges — the same size-dependence that hid the
   tie rule. *The probe should be run at 70000 super-blocks.*
2. The reduction reads a different 32-weight window at `j=7` specifically — an off-by-one in `t0` that
   only shows at the last sub-block, where reading past would take values from the next block.
3. The clamp is applied differently — if one side clamps before the max and the other after.

**Resolution 2 is the one the position data favours**: `j=7` is the only sub-block whose 32-weight
window ends at the block boundary, and the last word is the only part of it that would be affected by
a read that runs over.

The decisive print is `amax` from both sides for that sub-block, which the gate does not yet show.
