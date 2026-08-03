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
