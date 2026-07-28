# The Asahi 24 GiB ceiling — where it comes from, and whether it still applies

Prompted by asking whether libkrun exposes a way around it. It does not, and the reason is worth stating
before the interesting part.

## libkrun is not relevant

Its GPU path is virtio-gpu through `rutabaga_gfx`, targeting VM guests. It carries
`RUTABAGA_MEM_HANDLE_TYPE_APPLE` for **macOS** hosts and sizes its own host-visible window at
`1 << 33` (8 GiB, settable via `set_gpu_shm_size`). There is no Asahi, Honeykrisp or AGX code in the
tree. Its window is a VM shared-memory region and has nothing to do with a Vulkan driver's BO VMA
budget. Nothing to learn, and no backdoor to find — the ceiling is a userspace allocator policy, not a
hidden hardware gate.

## Where our 24 GiB actually comes from

From `memplan::gpu_buffer_budget_uma`, and its own comment is the finding:

> the conservative_vma (Asahi/Honeykrisp) cap of 24 GiB is an **EMPIRICAL fact about the *staging*
> upload path**: `queue.write_buffer` double-allocates (a transient staging buffer as big as the data)
> and that peak overran the driver's BO VMA ceiling on 40–70 GiB models. **BUT the map-at-creation arena
> does NOT stage** — it writes straight into the buffer's device-visible backing.

So the number measures a **2× peak during upload**, not a limit on resident weights. It was measured on
the staging path, and the arena removes the staging.

## Why this matters now

Every Asahi load log says:

```
arena ON (auto: BO-VMA-constrained driver) — packing resident weights into ≤2047 MiB slabs
```

The arena is already enabled, precisely *because* the driver is BO-VMA-constrained. If the comment is
right, the condition that produced 24 GiB is not present in the runs that are being capped by it. The
budget function does not take the arena into account — it sees only `conservative_vma`.

**This is a hypothesis, not a finding.** It rests on a comment, and the alternative is equally
plausible: the BO VMA wall may be about total mapped VA regardless of how it was written, in which case
the arena changes nothing and 24 GiB is correct. Nothing in the code distinguishes these, and no
measurement has been taken.

## The experiment that would settle it

The loader already prints the way to run it:

```
Options: ... or raise LOGSCOPE_MAX_GPU_GB to probe the real ceiling.
```

With the arena on, load a model that needs more than 24 GiB resident:

```bash
LOGSCOPE_MAX_GPU_GB=32 logscope ...    # Qwen3-30B int8, or Scout at q4 resident
LOGSCOPE_MAX_GPU_GB=48 logscope ...
```

Three outcomes, all informative:

* **Loads and generates correctly** — the cap is measuring the staging path, the arena removed it, and
  `gpu_buffer_budget_uma` should take the arena into account. This is the case worth having: Scout at
  ~34 GiB resident would stop streaming entirely, which removes the fills, the staging fence and most of
  the GPU timeouts in one step.
* **Allocation fails cleanly** — the wall is real and VA-total, the cap is right, and the comment should
  be corrected so nobody re-runs this experiment.
* **Loads and produces wrong output** — the worst case and the reason to check output rather than just
  exit status: an allocation that silently overcommits is how a model comes out quietly degraded.

Until one of those is observed, the conservative cap stays. Raising it on the strength of a comment is
exactly the reasoning that this project has been repeatedly bitten by.


## MEASURED: 63 GiB, against a 24 GiB assumption

```
gate-vram: adapter = Apple M2 Ultra (G14D B1) [Vulkan]
  probing in 1024 MiB slabs up to 64 GiB (writes and reads back each one)
Failed to allocate BO VMA
  VERIFIED CEILING (allocation failed above this): 63.0 GiB in 4.5s
  engine currently budgets: 24.0 GiB
```

The hypothesis held. The cap was measuring the **staging** upload path — the transient double-allocation
that `queue.write_buffer` makes — and the map-at-creation arena removed it. The device honours **2.6x**
what the engine was budgeting, and models that fitted were being streamed for no reason.

The probe popped cleanly ("Failed to allocate BO VMA", caught by the OOM guard) rather than hanging or
corrupting, which is the outcome that makes the number trustworthy.

## And it did NOT fix the gibberish

`LOGSCOPE_MAX_GPU_GB=32` loaded Scout fully resident, **with no GPU timeouts**, and still produced
`2, of`. Qwen3-30B produced word soup.

That is worth as much as the ceiling finding. It rules out, in one run:

* memory pressure
* expert streaming and the fills
* the staging fence
* the GPU timeouts

None of them are the cause, because all of them are absent and the output is unchanged. Whatever
produces `2, of` survives a completely different memory regime — which is the same conclusion the
cross-platform run pointed at before the frame lifecycle bug muddied it.

## On `/sys` — there is nothing to read

There is no standard sysfs node for "largest GPU allocation this driver will honour". `/sys/class/drm/card*/`
exposes device identity and connector state; DRM drivers that manage discrete VRAM may expose
`mem_info_vram_total` under their own vendor path, but that is amdgpu's convention rather than a
kernel-wide interface, and it reports physical VRAM rather than a VA-space policy.

The Asahi ceiling is not a hardware quantity at all — it is where the driver's BO VMA allocator gives up,
which depends on fragmentation, what else is mapped, and the allocation pattern. That is why probing
finds 63 GiB on a 192 GiB machine and why the number would not be constant across runs. A sysfs file
could not report it honestly even if one existed.

So reaching in the dark is, unusually, the correct approach here — but it should be done once and
cached, not left to a person with an env var.
