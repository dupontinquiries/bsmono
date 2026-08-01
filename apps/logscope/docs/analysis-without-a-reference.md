# Analysing something with no reference to check against

How people do this in fields where being confidently wrong is expensive. Written because a logscope
operator asking "make this faster" is asking for exactly this discipline, and because this session
repeatedly produced confident wrong answers that a named technique would have caught.

## The core move: refute, don't confirm

Intelligence analysis converged on this and named it. From the *Analysis of Competing Hypotheses* (ACH)
literature:

> The idea is to refute hypotheses rather than confirm them. **The most likely hypothesis is the one with
> the least inconsistent information that would argue against it, not the one with the most relevant
> information that supports it.**

That inversion is the whole technique. Looking for support finds it — for every hypothesis, including
the wrong ones — because evidence consistent with a true explanation is usually also consistent with
several false ones. Looking for *inconsistency* discriminates.

Practically: list the plausible explanations first, then take each piece of evidence and ask which
explanations it **rules out**. Rank by fewest surviving contradictions.

Our own session, repeatedly: the 24 GiB budget survived because everything was consistent with it. It
died the moment one piece of evidence was inconsistent — llama.cpp running a 70B on the same driver.

## Key Assumptions Check

List what you are treating as given, then ask of each: *what would have to be true for this to be
false, and would I have noticed?*

The value is in the second half. Most wrong assumptions are not unexamined — they are examined and then
found consistent with observation, because the observation was never capable of contradicting them. The
useful question is not "is this true" but "what would have shown me it was not".

Session examples, all of which held for many rounds:

* "The BO-VMA ceiling is a hard driver limit" — nothing in the code could have distinguished a driver
  limit from a limit of our own upload path.
* "`prechecks` catches these errors" — true, and it had zero callers, so nothing ever ran it.
* "Scout's active set is 10.5 GiB so it fits" — the active set is per-token; residency needs all experts.

## Devil's advocacy and Team A/Team B

Assign someone to argue the opposite case, in earnest, with the same evidence. The point is not
balance — it is that a case nobody is trying to make will not be made well by the person who already
believes the alternative.

The cheap version for one person: **write the strongest version of the position you reject**, and see
whether writing it changes what you believe. If it does not, you at least know where the disagreement
actually is.

## Falsification by substitution

For any universal claim, the asymmetry is total: no amount of checking confirms it, one counter-example
destroys it. So probe the edges first — 0, 1, −1, empty, maximum — because a claim that fails anywhere
usually fails there.

This is what property-based testing automates, and it is why `refute::Verdict` distinguishes `Survived`
from anything named `Proved`.

## Hardware: the roofline, and the Speed-of-Light baseline

For "make this faster" with no reference implementation, the discipline is:

**1. Establish the ceiling by microbenchmark, not from the spec sheet.** Measure peak bandwidth (a
STREAM-like copy) and peak compute (a dense GEMM) on the actual machine. Google's TPU team calls this the
*Speed-of-Light* baseline, and the point is that it turns optimization "from a trial-and-error process
into an empirical engineering discipline" — you now know what perfect looks like.

**2. Compute the operational intensity of the kernel**: FLOPs per byte of DRAM traffic. This is
arithmetic from the algorithm, not a measurement.

**3. Place the kernel on the roofline.** `P = min(P_max, I × bandwidth)`. Which term binds tells you
which optimizations can possibly help:

* **memory-bound** — tuning arithmetic yields nothing. Reduce bytes moved, improve locality.
* **compute-bound** — tuning memory yields nothing. Reduce FLOPs or improve kernel efficiency.

Getting this backwards is the most common way optimization effort is wasted, and it is invisible without
the measurement. LLM decode is emphatically memory-bound, which is why every arithmetic-level idea in
this session (a JIT, low-rank approximation) lost to ideas that moved fewer bytes.

**4. Beware what the simple roofline cannot see.** It models throughput only. Bottlenecks in *latency*,
*cache capacity*, and *out-of-order buffers* are invisible to it — a kernel can sit far under both
rooflines because of a dependency chain. Our float-reduction finding was exactly this: 8× available, and
neither roofline term explained it, because the limit was a serialised chain of adds.

### Reverse-engineering an undocumented device

When the vendor's numbers do not exist or cannot be trusted:

* **Probe until it fails, then bisect.** Allocation ceilings, queue depths and buffer limits are all
  found this way. Critically, *verify* — write and read back — because an allocator that accepts a
  request it cannot honour is common, and an unverified ceiling is worse than no number.
* **Vary one dimension at a time and look for cliffs.** A discontinuity names a mechanism. Our Q6_K
  kernel was exact through 64 blocks and 100% wrong at 65: the cliff *is* the workgroup size, and the
  size sweep found in one run what two commits of reasoning had missed.
* **Compare against a peer implementation on the same hardware.** Not for its code, for its behaviour.
  llama.cpp running a 70B settled a question that three of our own measurements had left open.
* **Distinguish "the device refused" from "we asked wrongly".** A 4096 MiB allocation failing looks like
  a small ceiling and was a per-buffer limit. The discriminator is varying the *shape* of the request,
  not its size.
* **Check what the OS already publishes.** `cpu_capacity` names the efficiency cores without a table of
  SoC names; `VkPhysicalDeviceMemoryProperties` reports heap sizes. Reaching for a probe before checking
  for an interface is a common waste — though note the converse trap: the driver reported 95 GiB where
  63 was verifiable, so a published number is a claim, not a measurement.

## The habit underneath all of it

Every technique above is the same shape: **make the belief specific enough to be wrong, then look for
the thing that would show it.** A belief that cannot be stated in a form which observation could
contradict is not yet a finding, however well supported it feels.
