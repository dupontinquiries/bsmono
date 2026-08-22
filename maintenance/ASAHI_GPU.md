# Asahi / Honeykrisp GPU: what is measured, and what is only believed

One page for the whole tree, because two projects now run compute on this
hardware and were about to learn the same things twice. `logscope`'s
`gpu-gemm` has been running transformer inference on it for months;
`beatspace` is building a training path on it now.

Everything below is marked **MEASURED** (a run on real hardware produced it) or
**ASSUMED** (taken from documentation, or from one machine and generalised).
The distinction matters more here than usual — one of the constants in this
tree is a single measurement from one M2 Ultra applied to every Asahi machine,
and it says so in its own error message.

**Not a replacement for `ASAHI_VULKAN_LESSONS.md`, which sits beside it.** That
page is operational — how Asahi is detected (`/proc/device-tree/compatible`),
what the Mesa/Vulkan stack needs, how a container reaches the GPU. This page is
about writing a compute kernel once the device is open. Neither is complete
alone and they are cross-linked for that reason.

---

## MEASURED — the training GEMM works

```
adapter: Apple M2 Ultra (G14D B1) (Vulkan)
gemm 37x53x41 on a 3x3 tile grid
worst |gpu-cpu| = 1.669e-6 at (7, 24)
PASS — all 1517 elements within 1e-4
```

`beatspace gpu-selftest`, 2026-08-19, against the CPU reference in
`wgpu_train.rs` whose gradients are finite-difference checked.

**1.669e-6 is the right answer, not a near miss.** f32 has ~7 decimal digits,
the kernel accumulates 53 terms in tile order and the reference accumulates
them in index order, and a deviation at that scale is the reordering. A kernel
that was *exactly* equal would be suspicious — it would mean the tile loop was
not actually reordering anything.

The dimensions are deliberately ragged (37×53×41, none a multiple of the
16-wide tile), so this result covers the bounds handling in `a_at`/`b_at` and
not just the aligned interior.

## MEASURED — the parameters this GEMM uses

Validated by the run above, on `Apple M2 Ultra (G14D B1) [Vulkan]`:

| parameter | value | why |
|---|---|---|
| tile | 16×16 | two f32 tiles = 2 KB threadgroup memory |
| workgroup | 256 threads | eight whole 32-wide AGX SIMD groups |
| subgroup ops | **not used** | see below |
| barriers | two per tile iteration | one after load, one after the inner loop |

The second barrier is not optional. Without it a fast thread overwrites the
tile while a slow one is still reading the previous iteration out of it — a
race that produces plausible wrong numbers rather than a crash, which for a
training kernel is the worst available failure mode.

## MEASURED — the 65535 dispatch limit, the hard way

A 220,960-workgroup grid was accepted by the API and produced wrong results in
part of its output. It read as a training bug rather than a dispatch bug and
cost a day. Both projects now spill into `y`:

* `gpu-gemm`: `lib.rs` panics with the grid named, and `wg1()` spills.
* `beatspace`: `wgpu_run::Grid::for_items` spills by construction, so no
  caller can build the bad grid.

A kernel dispatched this way **must** derive its index from `num_workgroups`.
One that reads `gid.x` alone silently processes the first 65535 groups and
ignores the rest.

## ASSUMED — the AGX architecture numbers

From Apple-GPU compute documentation and ggml-metal practice, recorded in
`gpu-gemm/src/lib.rs`. Not measured by us:

* The SIMD group is 32 threads. Think in subgroups, not threads.
* Each core has ~32 KB of threadgroup memory, and a workgroup reserving the
  full amount pins occupancy to exactly one workgroup per core — which on a
  memory-latency-bound kernel is the dominant cost.

The practical rule both projects follow: keep any threadgroup reduction buffer
as small as possible, and prefer a shape that needs none.

## MEASURED — subgroup ops can hang this driver

`gpu-gemm` ships with subgroup matvec **disabled** on Honeykrisp:

> subgroup decode kernel DISABLED on Apple M2 Ultra (G14D B1) [Vulkan]
> (Asahi/Honeykrisp subgroup support can hang large matmuls;
> `LOGSCOPE_MATVEC_SG=1` re-enables)

This is the one place where the architecture argues one way and the hardware
argues the other. `subgroupAdd` *should* out-occupy a barrier tree here — no
threadgroup memory, native reduction — and it hangs. beatspace's GEMM
accumulates in registers and needs no reduction at all, which sidesteps it
rather than resolving it.

If someone re-tests this and it is fixed, the matvec path is where the win is,
not the tiled GEMM.

## ASSUMED, and loudly — the 31 GiB buffer ceiling

From `gpu-gemm`'s loader, quoted because the caveat is the important part:

> GPU budget 48.0 GiB clamped to 31.0 GiB — this driver accepts a large single
> allocation but will not hold more than that in buffers at once. **THIS FIGURE
> IS A COMPILED-IN CONSTANT, not a measurement of this device**: it was
> measured once on an M2 Ultra at 1 and 2 GiB slabs, mapped and unmapped, and
> every Asahi machine now receives that number.

The failure it prevents is a plan that dies *part way through* a load rather
than at planning time. `kernel-validate --gate-vram` measures the
single-allocation ceiling on the device in front of you; a load that fails
part-way is evidence the concurrent one is lower than 31.

`beatspace::wgpu_run::BufferBudget` refuses up front and repeats the caveat in
its error, so nobody reads the number as a property of their machine.

## MEASURED — pass boundaries dominate submits

From a real logscope run:

> 15.62 tok/s at 7 submits/tok — 96% of submits were forced by PASS BOUNDARIES
> with fusion NOT engaging (7200 passes for 9921 dispatches)

Consecutive dispatches can share a pass, and `LOGSCOPE_PASS_FUSE` is
**RETRACTED**: it broke AR decode 6/6 and changes generated tokens on 3 of 6
diffusion configurations. Do not reach for it. A training step has the same
shape — many small dispatches — and will hit the same wall; the answer there is
fewer, larger dispatches, which is the reason beatspace batches every
convolution's output positions into one GEMM rather than looping.

## The wgpu 27 API details that cost a compile each

* `device.poll(wgpu::PollType::wait_indefinitely())` — `PollType::Wait` is a
  struct variant and is not a value.
* `request_adapter` / `request_device` return `Result`, not `Option`.
* `ShaderModuleDescriptor::source` takes `Cow`, so `src.into()`.

---

## Where the code is

| what | where |
|---|---|
| inference kernels, autotuner, quant | `apps/logscope/crates/gpu-gemm/src/` |
| WGSL validation without a GPU (naga) | `gpu-gemm/src/wgsl_validate.rs` |
| training kernels + CPU reference | `apps/beatspace/src/wgpu_train.rs` |
| dispatch, budget, self-test | `apps/beatspace/src/wgpu_run.rs` |
| graphics-side perf method (p50/p99 seeded rows) | `games/unendlich/crates/vk-harness/perf/` |

`vk-harness` is graphics rather than compute, but its methodology is worth
copying when the training path starts producing timings: seeded rows tagged
`source:"hardware"`, tracked at p50/p99/max with a run count, so a regression
is visible against a recorded baseline instead of a remembered one.

---

## Why speculative decoding is worth MORE here than on a datacentre card

Prompted by a vLLM fork (`oceanplexian`, Aug 2026) reporting **218 tok/s** on
Qwen3.8-27B across two RTX 3090s. The headline is speculative decoding, and the
numbers are worth reading precisely:

| | their run | what it means |
|---|---|---|
| draft tokens | 7 | proposed per target forward |
| acceptance | 47.8% | of individual draft tokens |
| **acceptance length** | **3.35** | **tokens committed per target forward** |
| speedup | ~4.6x | over non-speculative |

**Acceptance length is the metric, not acceptance rate.** 47.8% sounds like a
coin flip; 3.35 tokens per forward is the number that becomes the speedup.
logscope already reports it as `per_pass` in the spec-decode path — worth
keeping, because "48% acceptance" invites tuning toward a percentage that is not
the thing being bought.

**The argument for this hardware is stronger than theirs, and it is the
inversion worth noticing.** On a 3090 the target forward is compute-bound, so
verifying 8 tokens costs meaningfully more than verifying 1 and the speedup is
sublinear in acceptance length. Here it is not: the measured profile is 92% GPU
sync across ~35 submits per forward, and a verify pass over k+1 tokens costs
the SAME submits as one token — the batch dimension is nearly free when the
bottleneck is dispatch, not arithmetic. So acceptance length converts to
speedup more directly on Honeykrisp than on the card the technique was tuned
for.

**Their drafter costs 13.5 GB of VRAM. Ours must not.** DFlash2 is a second
model resident alongside the target, which is available on a 24 GB card and
impossible on a 4-8 GB phone — the fleet this targets. `spec::PromptLookup` is
the answer already in the tree: model-free, it finds the current suffix earlier
in the context and proposes what followed, so there is no second checkpoint to
download, load or keep resident. It should do well on repository reading, where
identifiers and paths repeat, and mostly whiff on prose — where a miss costs one
verification slot and nothing else.

Defaults today: `LOGSCOPE_SPEC_DRAFT=lookup`, `LOGSCOPE_SPEC_K=4`,
`LOGSCOPE_SPEC_NGRAM=2` (measured: ng=2 beats 3 and 4 on every corpus). Their
run used k=7. **Worth re-sweeping k on this hardware for exactly the reason
above** — a larger k costs almost nothing per forward when submits dominate, so
the optimum here should sit higher than on a compute-bound card. That is a
prediction, not a measurement.

**The same lever, in the other family.** Block diffusion's threshold commit
(`LOGSCOPE_DIFFUSION_TAU`) is the identical arithmetic: tokens committed per
forward. Speculative decoding does it for AR by guessing ahead and verifying;
diffusion does it by committing every position the model is already confident
about. One number, two families, and on this hardware it is the number that
matters in both — because the forward is the unit that costs.

---

# What a 218 tok/s vLLM number does and does not transfer (2026-08-19)

A vLLM fork reports **218 tok/s single request, Qwen3.8-27B on 2×3090**, with
**~4.6× effective speedup over non-speculative**. Worth separating three
claims that arrive as one number.

## The hardware is not comparable and the technique is

Two 3090s are ~1.7 TB/s of aggregate bandwidth with mature CUDA kernels; the
Asahi M2 Ultra path here measures **3–9 GB/s of an 800 GB/s spec**, i.e. under
1% of its own ceiling. The gap in that number is not the algorithm.

The 4.6× **is** the algorithm: `--speculative-config` with
`num_speculative_tokens: 7` and a DFlash2 drafter. Draft several tokens
cheaply, verify them in one batched pass of the target model, keep the prefix
that matches. It converts a latency-bound decode into a throughput-bound one —
which is exactly the regime logscope is in, where the roofline says the
hardware is idle and the limit is dispatch and sync overhead.

## logscope already has it, and nothing calls it

Every piece is built and tested:

| piece | where | state |
|---|---|---|
| the control loop | `specloop.rs` — `propose()` / `settle()` | 6 tests, **no caller** |
| the accept rule | `specverify.rs` | present |
| a drafter needing no second model | `lookupdraft.rs` (n-gram from context) | present |
| the batched verify pass | `Gpu::decode_batched` | present |
| draft rollback | `Session::rewind(rows)` | present |

`specloop`'s own header explains the sequencing it was designed for:

```text
let d = spec.propose(&context);        // tokens to draft, possibly empty
// run the target over context + d, one batched pass -> d.len()+1 argmaxes
let out = spec.settle(&d, &argmaxes);  // tokens to emit
```

`lookupdraft` matters more here than DFlash2 does: vLLM's speedup needs a
second checkpoint sharing a tokenizer, and the n-gram drafter needs nothing —
which for a fleet that loads one GGUF at a time is the difference between a
feature and a deployment problem.

**This was NOT wired in that session, deliberately.** `specloop`'s header calls
the generation loop "the hardest code in the engine to change safely", the
rejection path has to `rewind` the KV cache by exactly the count that failed,
and there is no hardware here to check the output against. Wiring it blind is
what `LOGSCOPE_PASS_FUSE` did — it changed generated tokens on 3 of 6
configurations and is now retracted. The right shape is an env flag, off by
default, reporting `accept_rate()` — the same way every other risky lever in
this engine ships.

## It does not apply to the diffusion path at all

Speculation drafts a token and checks it against the model's next-token
distribution. Block diffusion has no next-token step to draft — it denoises a
whole block. The analogue is **tokens committed per forward**, which is the
confidence threshold `LOGSCOPE_DIFFUSION_TAU`, and as of c4-917 the measured
rate is reported at the end of a run.

So for DiffuCoder the lever is tau, and for an AR model on this engine it is
the speculative loop that already exists. Two different answers, and the vLLM
number is evidence for the second one only.

---

## Speculative decoding: what a real MTP release measured

From DavidAU's Qwen3.6-27B MTP quants (Aug 2026), which ship regular and
multi-token-prediction GGUFs of the same weights so the two can be compared
directly. Their numbers, and what transfers:

| | their measurement |
|---|---|
| regular GGUF, Q4_K_S, RTX 5090 | ~75 tok/s |
| MTP GGUF, 60% acceptance, 2 draft tokens | ~90 tok/s |
| break-even | **50% acceptance** — below it, the plain quant is faster |

**Acceptance is a function of the SAMPLER, not only the model.** Their guidance
is explicit: keep temperature at or below 1.0, and keep repetition penalty at
1.0 (off), because both degrade MTP performance. The mechanism is the same in
any speculative scheme — a draft is accepted when the target agrees with it, and
temperature and a per-position penalty both move the target's distribution away
from the greedy token the drafter guessed. **A repetition penalty is worse than
temperature here**: it reshapes the distribution per position based on history,
so the draft is scored against something the drafter never saw.

logscope now says this out loud. The spec path reports tokens per verification
pass (not acceptance rate — c4-829: a high rate on short drafts can be worth
less than a mediocre rate on long ones), and below the 1.00 break-even it names
the cause, checking the operator's own temperature and penalty first.

**They also observe acceptance RISING as the context fills and across multi-turn
chats.** That fits prompt-lookup drafting exactly: more context means more
n-grams to match. Worth knowing before judging a drafter on a cold first turn.

## Mixed precision by tensor ROLE, not one number for the file

The same release keeps the output tensor at f16 while the body is 4-bit, and
holds MTP tensors at Q8_0 regardless of the body quant. Both are role-based
choices: the output projection is 10-20% of the bytes and all of the vocabulary
resolution, and a draft head that is noisier than the target it feeds wastes
verification slots.

**And a Vulkan-specific note worth testing here.** They ship separate "AMD"
quants whose only difference is the output tensor in f16 rather than bf16,
because bf16 there provoked an "odd CPU offload of the OT" that hurt
throughput. This fleet is Vulkan-on-Asahi via Honeykrisp, which is exactly the
class of driver that would do that — if a model's output tensor is bf16 and
decode is slower than the roofline says it should be, that is a cheap thing to
check.
