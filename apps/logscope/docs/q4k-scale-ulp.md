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
