# Scout UD-IQ1_S: the checkpoint fits, the engine cannot hold it

```
Llama-4-Scout-17B-16E-Instruct-UD-IQ1_S.gguf     32.5 GB = 30.3 GiB
measured concurrent-buffer ceiling                          31.0 GiB
```

**0.7 GiB of headroom.** Tight — a KV cache of any size pushes it over — but the first Scout
checkpoint that is even arguably resident on this hardware.

And UD-IQ1_S is *already* the mixed-width layout the sensitivity argument asks for: Unsloth's dynamic
quant keeps attention and sensitive tensors at higher precision and pushes the experts to ~1.6 bpw.
The work of choosing which tensors get which width is done, in the file.

## Why none of the three existing paths can load it

**Resident, converted:** the engine requantizes everything to q4 or int8. 30.3 GiB at ~1.56 bpw
becomes **87.2 GiB** at 4.5 — over the ceiling by 56.

**Streamed:** `file_views_plausible` allows ggml types 12 (Q4_K) and 14 (Q6_K). IQ1_S is type 19, so
there is no file view, so it falls to the host-copy path and requantizes — the same 87 GiB, now in
host RAM. Refused before it starts.

**Resident, native:** there is no `LinKind` for it. The enum is `F32 | Q8 | Q4`.

## What exists already

* `dot_iq1_s` — a CPU reference that reads IQ1_S source bytes directly, bit-exact, no dequantize pass.
* `MATVEC_IQ1_S` — a GPU kernel, and its lane-varying shifts were fixed in the extractBits sweep.
* `--gate-iq1s` and `--gate-iq1s-kernel` — both already in the gate list.

So the arithmetic is written and validated. What is missing is **residency**: a way to hold a tensor
as IQ1_S rather than converting it.

## The change, in order of size

1. **`file_views_plausible` accepts type 19.** One line. On its own it does not help — the streaming
   path still converts to q4 per expert via `raw_expert_to_q4_resident`, so the pinned set is
   q4-sized and nothing is saved.
2. **`LinKind::Iq1(Iq1Weight)`.** The real work: a fourth variant, every match arm the compiler finds,
   the loader path that keeps the bytes rather than converting, and memory accounting that knows
   1.5625 bpw. The compiler makes it tractable — a missing arm is an error, not a bug.
3. **Dispatch `MATVEC_IQ1_S` for that variant.** The kernel exists; this is wiring.

## Why it is worth it, beyond Scout

The same three steps admit GLM-5.2 at IQ1_S (24.7 GiB, 6 GiB spare) and every future model too large
for q4. At a 31 GiB ceiling, **1.56 bpw is the widest format a 109B model can use** — so this is not a
Scout feature, it is the floor under everything above ~60B.

## The honest caveat

0.7 GiB of headroom is not much. If it does not fit once the KV cache is allocated, the fallback is
the same streaming path — but with **type 19 file views**, streaming IQ1_S experts moves a third the
bytes of Q4_K ones, which is a large win on the path that already works.

That makes step 1 worth doing even if step 2 never happens.
