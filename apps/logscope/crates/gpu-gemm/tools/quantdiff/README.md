# quantdiff — differential dequantization against llama.cpp

The same bytes through llama.cpp's `dequantize_row_*` and through this crate's
`gguf::dequantize`. Any divergence is a bug in one of them, and the reference is
authoritative: llama.cpp's dequantizer is what produced every GGUF file in
existence.

This is the cheapest strong oracle available for quant work. A dequantizer is a
pure function of bytes, so it needs no model, no weights and no judgement —
random bytes are a valid input, and the two implementations must agree on all of
them.

## Running it

    # 1. reference, from a llama.cpp checkout
    cd <llama.cpp>/ggml/src
    gcc -O1 -I. -I../include -Icpu -o /tmp/qref <this dir>/qref.c ggml-quants.c -lm

    # 2. ours: a bin crate depending on gpu-gemm, with qdiff_main.rs as its main
    # 3. sweep
    sh sweep.sh

## What it found

Run against llama.cpp master, twelve types, four seeds:

  * **IQ2_S** — `qh` and `signs` swapped. ggml-common.h declares
    `{ d; qs[QK_K/4]; qh[QK_K/32]; scales[QK_K/32]; }`, which reads as
    qh-then-signs, but `dequantize_row_iq2_s` takes `signs = qs + QK_K/8` — out
    of the SECOND HALF of qs. Real order: `d | qs[32] | signs[32] | qh[8] |
    scales[8]`. 186 of the first 256 weights were wrong in both sign and
    magnitude.
  * **IQ1_M** — two scale bugs. The 16-bit block scale comes from the top
    nibbles of the ODD scale bytes (llama.cpp reads `scales` as four u16s and
    takes the top nibble of each); this crate used the even ones. And the
    sub-block scales are 3-bit fields, two per 32 weights at bit offsets
    `6*(i%2)` and `+3`; this crate read 4-bit nibbles, taking one bit belonging
    to the neighbouring scale.

Both produce plausible wrong numbers rather than crashes or zeros, which is why
they survived: a model in either format loads, runs, and is merely bad. Neither
is reachable by reading the struct definition — in both cases the declaration
and the dequantizer disagree, and the dequantizer is the truth.

The fixes are pinned in-tree by `iq2_s_matches_the_llama_cpp_reference_vector`
and `iq1_m_matches_the_llama_cpp_reference_vector`, as numbers rather than as a
re-derivation of the layout, because the layout is what was misread.

## Performance, same harness

`qref.c` also times (`qref <type> <n> <reps>` prints ms to stderr). Comparing scalar C against this
crate on 20 x 262144 elements, before any change:

    q4_k  llama.cpp 2.28 ms   ours 20.6 ms   9.0x
    q5_k            1.68            20.5    12.2x
    q2_k            7.55            20.6     2.7x
    q6_k            6.01             6.00    1.0x   <- already at parity

Exactly the three formats that use `f32::mul_add`, and only those. On baseline
`x86_64-unknown-linux-gnu` FMA is not in the target feature set, so `mul_add` does not lower to an
instruction — it lowers to an indirect call into `compiler_builtins`' `fmaf`, per element, which
also makes the loop unvectorisable. Isolated: 25.38 ms vs 3.13 ms for the same loop written with
`d * q - m`.

The `mul_add` is deliberate and had to stay (the shader contracts to an FMA and the CPU reference
must round once to agree with it — see the note in the Q4_K arm). The fix is `src/fmapath.rs`:
detect FMA once, run the block loop inside a `#[target_feature(enable = "fma")]` function. Same
arithmetic, same bits, one instruction.

    q4_k 20.6 -> 3.06 ms   q5_k 20.5 -> 3.36   q2_k 20.6 -> 5.49

aarch64 was never affected: FMA is baseline in ARMv8, so `mul_add` already lowers to `fmla`. This
was a desktop-x86 tax on a project that benchmarks on ARM.
