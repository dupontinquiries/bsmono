# GPU benchmark findings

Everything measured while making this app fast, kept because the numbers cost
hardware time to get and the conclusions are not obvious from the code. The
transferable subset is written up for the other GPU app in
[`FOR_LOGSCOPE.md`](FOR_LOGSCOPE.md); this is the full record.

**Rig for every number below:** Apple M2 Ultra (G14D B1), Honeykrisp, Mesa
25.3.6, Fedora asahi 7.1.6 aarch64, 24 cores, 190 GB unified memory, rustc
1.97.1. f32 peak ≈ 13.6 TFLOPS (the 27 TFLOPS figure quoted for this part is
fp16/dual-issue and is *not* the ceiling these kernels run against).
Measured with `dandere2x bench`, which times full tiles. Every kernel is
bit-exact against the CPU reference before it is timed, so none of these are
speed-for-accuracy trades.

## 1. Model cost

| model | arch | MMAC / input px | 1080p full frame |
|---|---|---|---|
| RealESRGAN_x2 | RRDBNet, 398 ops, 16.7 M params | 4.48 | 9.30 TMAC |
| realesr-animevideov3 | SRVGG, 18 ops, 0.6 M params | 0.62 | 1.28 TMAC |
| upconv_7 (waifu2x) | 7 ops, 0.6 M params | 0.55 | 1.14 TMAC |

The photo model is 7–8x the compact one. That ratio is what the cascade
trades against.

## 2. Conv kernel variants (RealESRGAN x2, tile 688)

| kernel | GMAC/s | TFLOPS | % of peak | shape per thread |
|---|---|---|---|---|
| rt1 | 661 | 1.32 | 10% | 1 px × 4 ch |
| wino | 677 | 1.35 | 10% | Winograd F(2,3), 2.25× fewer multiplies |
| rt4 | 683 | 1.37 | 10% | 4×4 px × 4 ch, ~130 registers |
| rt2c8f | 803 | 1.61 | 12% | rt2c8 + duplicated fast-path branch |
| rt2 | 896 | 1.79 | 13% | 2×2 px × 4 ch |
| rt2c8 | 977 | 1.96 | 14% | 2×2 px × 8 ch |
| **rt2f** | **1022** | **2.04** | **15%** | rt2 + interior fast path |
| rt2c16f | 496 | 0.99 | 7% | 2×2 px × 16 ch + fast path |

On the compact model at tile 1024 the same kernels run faster in absolute
terms — rt2f 1403 GMAC/s (2.81 TFLOPS, ~21% of peak), rt2c8f 1298, rt2c8 1122,
rt2 1016, rt2c16f 921 — which fits the register story: SRVGG's layers are 64
channels wide against RRDB's 256.

**What the numbers mean, in the order we learned it:**

1. *Not multiply-bound.* Winograd cuts multiplies by 2.25× and loses 24% to
   plain rt2. It needs 16 accumulators; AGX occupancy falls off a cliff past
   roughly 64 registers. Arithmetic is not the scarce resource — registers are.
2. *The pixel axis has a small interior optimum.* rt1 → rt2 → rt4 is an
   inverted U peaking at 2×2, losing ~25% in both directions. "More work per
   thread raises arithmetic intensity" inverts once the working set stops
   fitting.
3. *The channel axis pays where the pixel axis does not.* rt2c8 reuses one
   loaded 4×4 input patch across two weight groups: double the arithmetic per
   load with the input-register footprint unchanged. +9%.
4. *Duplicated branch bodies spill.* rt2f's interior fast path is worth +14%
   alone, but composing it with channel blocking as a duplicated branch put
   four (rt2c8f) then eight (rt2c16f) copies of the accumulator block in one
   shader, and both lost — rt2c16f catastrophically. The structural fix is to
   hoist the address math instead of branching (`rt2h`, `rt2c8h`); generated
   and bit-exact but not yet timed on hardware.
5. *Addressing was a real cost.* The generic load did four comparisons, two
   selects and an `in_scale` divide **per scalar**, recomputed for every one
   of up to 256 input channels, for values constant across the whole loop.
   That is what rt2f's +14% removes.

## 3. Tile size and in-flight sets

Throughput is flat from tile 400 to 688 (875–898 GMAC/s for rt2) and identical
at 1, 2 and 4 activation sets in flight once the tile matches. Two conclusions:

- **Flat across problem size *and* across network shape ⇒ a per-thread issue
  ceiling**, not an occupancy tail, launch overhead or DRAM limit. That is what
  sent the work into kernel blocking rather than into fusion.
- **In-flight overlap is a no-op on unified memory, and harmful when memory is
  tight.** There is no PCIe transfer to hide; each extra set only steals VRAM
  from tile size. At a 2 GB budget, 4 sets shrink the tile to 240 and cost 27%
  (637 vs 878 GMAC/s). Default is now one set.

| VRAM budget | sets | tile | rt2 GMAC/s |
|---|---|---|---|
| 2 GB | 1 | 512 | 878 |
| 2 GB | 2 | 352 | 823 |
| 2 GB | 4 | 240 | 637 |
| 8 GB | 1 | 688 | 897 |
| 24 GB | 4 | 688 | 894 |

Tile 688 is where the adapter's 2047 MiB storage-binding limit stops growth,
not where returns end — though 512 → 688 is already only +2%.

## 4. End to end (1080p → 4K, 24 frames, synthetic clips)

| run | fps | note |
|---|---|---|
| RRDB, cascade + flat bypass, rt2 | 1.50 | |
| same, rt4 | 1.00 | matches the kernel bench ordering |
| RRDB, **no cascade** | 0.13 | 11x slower |
| RRDB, cascade, full-motion clip | 0.38 | little reuse available |
| compact model, no cascade | 1.15 | |
| RRDB, cascade, `-j 2` | 1.94 | segment parallelism on top |

The cascade is the largest single lever measured — but that clip is synthetic
gradients, where ~96% of changed pixels route to bicubic. The full-motion clip
at 0.38 fps is the other end. Real footage lands between, which is why
`analyze` exists.

## 5. Reuse, measured by `analyze`

| clip | quality | through main network | through any tier |
|---|---|---|---|
| partial motion | 97 / 93 / 90 | 2.1% | 17.4 / 15.1 / 13.4% |
| full motion | 97 / 93 / 90 | 14.7 / 14.5 / 14.4% | 17.5 / 17.2 / 17.0% |

Quality has surprisingly little effect on the *main network* share on these
clips — the cascade is doing the routing, not the gate. On compressed sources
the gate matters more, because artifacts change frame to frame.

## 6. CPU hot paths (one core, per call)

| path | ms |
|---|---|
| `hash_cell` ×2304 (block cache, 1080p grid) | 1.73 |
| `write_output_rgba8`, 400-tile ×4 | 1.13 |
| `add_uniform_noise`, 1080p | 1.12 |
| `block_sse` ×2304 | 0.45 |
| `extract_input`, 400-tile u=2 | 0.41 |
| `padded_to` replicate, 1080p +16 | 0.39 |
| `psnr`, 1080p | 0.21 |
| `extract_input`, 400-tile u=1 | 0.06 |

All small against a 660 ms (compact) or 2.3 s (RRDB) tile, so the CPU is not
the bottleneck — but `hash_cell` is now the largest and would matter if the
GPU ever got several times faster. Single-core timings on this box vary run to
run by up to 2x (`add_uniform_noise` read 1.12 and 2.33 in consecutive runs),
so small deltas here mean nothing.

## 7. Things that were tried and did not work

- **Winograd F(2×2,3×3)** — see §2.1. Correct, bit-exact, 24% slower.
- **4×4 pixel register tiling** — 2.9× the arithmetic per load, 24% slower.
- **Composing the fast path with channel blocking by branch duplication** —
  both wins are real alone, the combination lost 18% (and 45% at 16 channels).
- **In-flight activation sets** — no effect on UMA, harmful under a tight
  budget.
- **f16 weights** — weights are 82% of this kernel shape's load bytes, so this
  looked like the obvious next lever, but the evidence above says the kernel is
  not bandwidth-bound; unpacking costs ALU to save bytes, trading the resource
  that is not scarce for the one that is. Also blocked mechanically: naga 29
  gates `unpack2x16float` behind `SHADER_FLOAT16_IN_FLOAT32`, so it needs a
  device feature and a probe, not a shader edit.
- **DCT-coded upload** — compresses a transfer that is nearly free on unified
  memory, and cannot touch the traffic that matters (activations re-read
  between 398 layers, which need exact spatial values every layer).

## 8. Traps in the wgpu submission model (found the hard way, three times)

`queue.write_buffer` writes **all land before any command in the same submit**,
not at their call site. Batching N tiles into one command buffer therefore
breaks every per-tile uniform written that way: all N tiles read the last
tile's values. It bit three times in one refactor —

1. the extract pass's tile rectangle (53% of output bytes correct),
2. the pack pass's destination (66% correct),

— and each time the symptom was a plausible-looking picture, never an error.
Anything per-unit that varies within a submit must be **baked** with
`create_buffer_init` (contents at creation, no queued write) or indexed out of
a storage array. The general rule for batching: *N units in one submit needs N
copies of every per-unit uniform, or units that share one.*

The third form is subtler: uniforms held in a shared activation set constrain
what may share a submit at all. The conv params live there and are reused by
every tile in the encoder, so only same-shaped tiles can be batched; tiles are
grouped by region size, which balanced tiling keeps to one or two shapes.

All three were caught by the bit-exact selftest, not by reading the code.

## 9. On-device downscale for the cascade (done)

A x4 network serving a x2 job used to go through `Resampled`, a host-side
Lanczos pass over the network's 4x output, per tile, on one core. At tile 504
that is a 2016² → 1008² resize against roughly 40 ms of GPU work for the same
tile — plausibly the largest CPU cost in cascade mode, since the compact model
is x4 and jobs typically run x2. The pack pass now box-averages `d x d` native
pixels per output pixel instead, and the wrapper is gone: the upscaler reports
`x2` directly rather than `x4 -> x2 (lanczos)`.

Two bugs on the way, both of which produced a picture rather than an error:

1. **The pack params' scale field feeds the input-skip lookup**, which lives in
   the network's own output space, so it must carry the NATIVE scale even when
   the pass delivers a smaller one. Setting it to the delivered scale cost
   240 LSB.
2. **Each sample must be clamped to [0,1] before averaging, not after.**
   Network outputs overshoot; averaging raw floats lets an out-of-range sample
   drag its neighbours down or up. Worth 22 LSB, and it survived every
   geometric explanation — a single-tile run showed the same error as a
   multi-tile one, which is what ruled out seams and pointed at per-sample
   arithmetic.

The second is the more general lesson: **whenever a reference quantises before
combining, a GPU version that combines before quantising is not the same
function.** Box-averaging is otherwise a slightly different filter from
Lanczos, which shows as 45.8 dB / 0.995 SSIM between the two renders —
visually indistinguishable, and softer rather than sharper.

## 10. Host work on the critical path

Per frame, between GPU submits, the host was doing more than it needed to.
Measured on the sandbox box (one core, x86, contended — read the *ratios*, not
the absolutes):

| step | ms |
|---|---|
| pad frame + convert to RGBA8 (old) | 2.63 |
| convert raw frame to RGBA8 (new) | 1.71 |
| RGBA→RGB readback pass, 4K | 5.67 |
| `write_output_rgba8`, 400-tile ×4 (now unused on the GPU path) | 1.60 |

Two changes:

- **No padded copy.** `pad_frame` allocated a 1952×1112 frame, replicate-filled
  it, and then ~10% more pixels were converted and uploaded — all to emulate
  coordinate clamping the extract shader can do for free. It now clamps its own
  reads. The output is *bit-identical* to the padded path (18/18 frames,
  infinite PSNR), which is the proof that clamping ≡ replicate padding here.
- **Both frame conversions are row-parallel** (`Network::frame_rgba8_into`,
  `Network::rgb_from_rgba8`). The readback shuffle is 8.3 M pixels at 4K and
  was the largest single host cost left. **Unmeasurable in this sandbox — it
  has one core** — so this is a change justified by shape, not by a number:
  both loops are independent per row and should scale with cores.

What remains on the host per frame: one RGBA8 conversion up, one RGB
conversion back, and the analysis stage. The readback could be cut a further
25% by packing RGB24 on device instead of RGBA8, but that needs each thread to
own 4 output pixels (3 u32 words) to avoid two threads writing the same word,
and the row stride stops being word-aligned when the width is not a multiple
of 4 — not obviously worth the edge cases.

## 11. The CPU upscaler (the compatibility path)

`--backend cpu` is what runs on a machine with no usable Vulkan driver — a
PinePhone, a Fairphone, anything where Honeykrisp is absent — so its speed is a
device-support question, not just a fallback nicety. Two loop changes, measured
on the sandbox box with SRVGG on a 64x64 tile:

| version | ms |
|---|---|
| original (`oy` innermost) | 830 |
| row hoisted out of the tap loops | 672 |
| + four output channels per task | 659 |

**Row order was the big one (1.23x).** The tap loops had `oy` innermost, so
each of the 9 x in_ch taps streamed the whole output plane — for an RRDB layer
about 1700 passes over 520 KB, which is past L2 on exactly the small machines
this path serves, so the output was re-fetched from memory every tap. Hoisting
the row keeps one output row in L1 across all of them. Expect a larger win on
a phone than the 1.23x seen here, since the sandbox has more cache than the
targets do.

**Channel blocking added only 2%** — four output channels per task so each
input row is read once and used twelve times. The same trade was worth 9% on
the GPU; here the input rows were evidently already resident, so the ceiling is
elsewhere.

Both are bit-identical, not approximations: the per-element accumulation order
is still ic, then ky, then kx, which the exact numpy-reference tests confirm.

## 12. Open questions the next hardware run should answer

- Do `rt2h` / `rt2c8h` (hoisted addressing, single code path) clear rt2f's
  1022, and does channel blocking finally compose?
- What do the one-submit-per-frame and buffer-pooling changes do to the
  end-to-end numbers in §4? `bench` times a single tile and will not show them.
- On real footage: the main-network share from `analyze`, and the dB cost of
  each speed setting from `compare`. Both are content-dependent and neither
  can be inferred from synthetic clips.
