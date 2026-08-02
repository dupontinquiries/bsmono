# The expert host copy holds a requantized layout the same size as the source

Two lines from the same engine, on two models:

```
Qwen3-30B:  Q4_K → q4 on GPU — 168 tensors converted in batch
            (no host copy, no f32 round-trip)

Scout:      MoE streaming will hold ~50.6 GiB of expert weights in HOST RAM
            The engine requantizes every expert to its own q4 layout on load,
            which is the cost here.
```

**The dense path converts Q4_K to q4 on the GPU with no host copy. The expert path does the same
conversion through 50.6 GiB of host RAM.**

And from `memplan::conversion_cost`: Q4_K is 4.5 bpw and q4-resident is 4.5 bpw — **the conversion is
exactly size-neutral**. So the host copy holds a rearrangement of bytes that occupies the same space as
the bytes it came from, for a transformation that already has a GPU implementation in this codebase.

That single fact is what refuses Scout on a 192 GiB machine: 50.6 host + 48.0 resident + 60.9 page
cache against 189.7.

## What Colibri does instead

`olmoe.c`'s header: *"Expert letti dal disco on-demand via `pread`+`fadvise(DONTNEED)`, cache LRU
per-layer."* Experts are read from disk when routed, and the pages are handed back immediately. There
is no host-resident copy of the expert set at all — the file is the store, and the LRU bounds what is
live.

That works because Colibri reads the file's own layout. It never needs a second copy because it never
produces a second format.

## The two ways to close it

**Route experts through the existing GPU conversion.** `Q4_K → q4 on GPU` already runs in batch for
dense tensors. If the streaming path fed expert slabs to the same conversion, the host copy disappears
and the cost becomes GPU-side staging, which the arena already manages.

**Or read the source layout directly and skip conversion entirely.** For Q4_K this is the better answer
because the formats are the same size — the conversion is a rearrangement, not a compression, and a
kernel that reads Q4_K blocks natively makes it unnecessary. That is also the path GLM-5.2 needs for
IQ1_S, where the conversion is a 2.88× *expansion* rather than neutral.

The second subsumes the first, and both are the same work the `docs/prefill-q4-gap.md` q4 GEMM and
`dot_iq1_s` point at: **read the checkpoint's format, stop converting to a compute format.**

## What the engine already tells the user

The Scout refusal says: *"use the llama.cpp backend, which reads the file's native Q4_K blocks directly
and needs no host copy."* That is the correct advice, and it is also a description of what this engine
does not do yet — the sentence names the missing feature while recommending someone else's
implementation of it.

**Read from Colibri's source and this engine's own log lines. Not measured** — no toolchain in the
container after a reset.

---

# Colibri makes weights GPU-resident lazily, per tensor, on first use

`backend_metal.h`'s contract:

> The first successful call wraps `W` and its row scales in GPU-visible buffers; later calls reuse them
> (weights are assumed stable at the same address).

And the signature takes the format as a parameter rather than a precondition:

```c
int coli_metal_matmul(ColiMetalTensor **tensor, float *y, const float *x,
                      const void *weights, const float *scales,
                      int fmt, int S, int I, int O);
/* fmt: 0=f32, 1=int8, 2=int4(packed), 3=int2(packed) */
```

**Three differences from this engine, each with a measured cost here.**

**Residency is lazy and per tensor.** Nothing is uploaded until a matmul needs it. This engine converts
and uploads the whole model at load — 23.4 s for Qwen3-30B before a token — and for MoE that includes
experts a short session may never route to. With 8 of 128 active per token, most of a 128-expert layer
can go untouched for an entire conversation.

**The format is an argument, not a conversion.** `fmt` selects the unpack path inside the kernel; the
weights are passed as they are. This engine converts to a single compute format at load, which is what
`docs/prefill-q4-gap.md` (q4 has no GEMM), `dot_iq1_s` (IQ1_S inflates 2.88×) and the 50.6 GiB expert
host copy are all consequences of. **One decision, three symptoms.**

**And `S` is in the signature.** The Metal matmul is batched by construction — a caller with one row
passes `S=1` and one with a fan passes `S=n`, through the same entry point. Nothing here has to be
routed to a different kernel to get batching, which is exactly what the fan-out round-robin and
`MATVEC_SMALL_Q4` currently need arranging by hand.

## What it costs Colibri

Lazy residency assumes the weights stay at a stable host address — the comment says so plainly. That
rules out an arena that repacks, and it means the host copy is the source of truth rather than a
staging buffer. This engine's arena exists precisely because the driver is BO-constrained and packing
weights into ≤2047 MiB slabs is what makes them fit at all.

So this is a trade, not a strict improvement: Colibri buys lazy upload and format flexibility by
keeping the host mapping authoritative. On a driver with a two-buffer VMA ceiling, that choice would
have to be reconciled with the slab packing rather than adopted.

**The transferable part is the format argument.** Converting at load is what forces the host copy, the
q4 GEMM gap, and the IQ1_S expansion — and it is a decision this engine makes once, at the top, for
reasons that were about kernel simplicity rather than memory.
