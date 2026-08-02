# Bug classes found by hunting, and where to look next

Written after a run of ten consecutive real bugs, because the *taxonomy* transferred better than any
individual fix. Two classes recurred; both survive code review for the same reason.

## Class 1 — a guard keyed on the wrong condition

The code is present, correct, and wired to a signal that does not arrive.

| instance | keyed on | should have been |
|---|---|---|
| subgroup matvec guard | Vulkan-on-Apple | any device whose driver can't schedule the subgroups |
| Q6_K file-view gate | `ggml_type != 12` | the types the fill path can convert (gate went green, check never learned) |
| governor pressure latch | `dims_alloc_failed()` only | any device pressure, incl. the VMA OOM this hardware actually reports |
| unified-memory budget | `conservative_vma \|\| macos` | `self.unified`, which was in scope |
| watchdog-bounded decode window | `conservative_vma` | `watchdog_prone()` — mobile drivers have the same watchdog |
| `tuneprofile` device defaults | Apple + Vulkan | *duplicate of the first fix, corrected in one place only* |

**Why these survive review.** In every case the real predicate existed and a plausible proxy was reached
for instead. `conservative_vma` *reads* as "this is a constrained unified device" and *means* "Vulkan on
Apple". The name describes a category; the value encodes one vendor.

**The fix is not vigilance.** It is naming the predicate after the question it answers — `watchdog_prone`,
`unified`, `mobile_gpu` — so the wrong one stops looking right.

**The duplicate variant is worse and needs a different tool.** A policy expressed in two places gets
corrected in one, and the survivor looks correct because it matches the code around it. Neither grep nor
review finds that; only asking *"where else is this decided?"* does. Where two crates must agree and
cannot share code, put a test in a third that depends on both — `watchdog_agreement` in `logscope-egui`
is the pattern.

## Class 2 — a label asserting more than its evidence

| instance | claimed | actually tested |
|---|---|---|
| `gate-q4k` | "the loader can stop requantizing" | exact Q4_K → f32 dequantization, not a native matvec |
| `gate-vram` | "budget is consistent with the device" | only checked the *too-low* direction; 48 GiB budget vs 31 GiB ceiling passed |
| profile line | "GPU-execution-bound" | setup+sync under 15% — i.e. 99.9% *unaccounted* |
| agent badge | "✓ verified" | `verified` starts **true** when no verifier is configured |

**The predictable shape:** a boolean that is *permissive-by-default* gets read as *affirmative-by-default*
the moment it reaches a display. `= x.is_empty()`, `unwrap_or(true)`, `!has_foo` — grep for these wherever
the result also renders.

**Why this class is the more damaging one.** A green light nobody re-derives is worse than no light. I
acted on `gate-q4k`'s message for several commits while scoping unrelated work.

## Class 3 — noticed and not acted on

`loader: hparam prefix is 'qwen35.' (registry expected 'qwen3_5.') — using the file's prefix`

It was **not** using the file's prefix. The sniff wrote a local and `acfg.prefix` kept the registry
value, so every later metadata read returned `None` — rope base, norm epsilon, expert weight scale, all
defaulting silently. A model with the wrong rope base emits near-random text.

**Noticing and not acting is worse than not noticing**, because the one message that could have pointed
at the cause said the opposite.

## Where the container's judgement runs out

Every kernel claim made from a dev container was validated on **lavapipe**, and hardware disproved two of
them. Lavapipe is a *reference* rasterizer: more permissive and more nearly correct than a driver, so it
validates arithmetic and conceals codegen, scheduling and memory-visibility faults. Four Q6_K faults were
found and fixed against it, all four real, and the kernel was still wrong on the only hardware that
matters.

`gateclass` now prints `ARITHMETIC ONLY` rather than a tick on a software adapter. The constraint stands
regardless: **container-green is a precondition, never a verdict.**

## Class 4 — written, benchmarked, unreachable

`MATVEC_Q8_0_TILED_V3` did 128-bit `vec4<u32>` loads and could be selected only via
`LOGSCOPE_MATVEC_V3`. The per-shape autotune searched every *other* kernel and picked `V2Wg128` — and I
read that result as "kernel choice is exhausted". **It wins the search it was entered in.**

This is worse than the ordinary no-caller case, because the code *appears* to have been evaluated. An
autotune result reads as evidence; it is only evidence about its candidate list.

**How to sweep for it, and how not to.** Grepping for `dispatch(SHADER` misses every shader selected
through a `match` arm — `MOE_MATVEC_GLU_Q8` and friends are reached that way and show as false
positives. Count *all* references and read the non-declaration ones. On this tree that sweep found
exactly one real instance (V3, now wired) and four false ones.

**The durable form** is not a sweep at all: assert the candidate list contains every variant of the
enum it searches, so adding a kernel without entering it fails a test.

## Next hunt

* The remaining `SUBMIT_BY_*` shortfall — the profile still reports an untagged submit path.
* `prechecks` catches syntax, not types; widening to `cargo check` costs seconds where prechecks costs
  microseconds, so the question is *when* rather than *whether*.
* Model-layer modules with no caller are a known and separate class — building is not the bottleneck,
  wiring is.
