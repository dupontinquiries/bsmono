# What a read of the Asahi driver changes for logscope

From an external audit of drm/asahi against the 7.1.5 UAPI. Recorded because it closes two questions
this project has spent real effort on, and reorders the rest.

## Closed: zerocopy is impossible, not merely unavailable

`drm_asahi_gem_create` takes size, flags and vm_id; the flags are WRITEBACK and VM_PRIVATE. **There is
no userptr and no host-pointer import in the UAPI at all.** `VK_EXT_external_memory_host` therefore
cannot be implemented by Mesa — it would need a kernel ABI change, and from then on a matched
kernel/Mesa pair forever.

`--gate-zerocopy` had already measured the extension as absent. The difference this makes is that
absent is now known to be **permanent**, so the lever is PARKED with the reason attached rather than
left looking like something a driver update might fix. On Metal the arena's map-at-creation path
already achieves the same end without the extension, which is why that platform never needed it.

## Reframed: the 63 GiB ceiling is not address-space exhaustion

The GPU virtual address space is **512 GiB** (`UAT_IAS = 39`), and a run mapping ~110 GiB across three
models sits at **21%** of it. So the "Failed to allocate BO VMA" that `--gate-vram` hits at 63 GiB is
*not* VA exhaustion, and the mental model behind the 24 GiB constant — "the BO-VMA wall is real and
independent of RAM" — is describing something whose mechanism we do not actually know.

That strengthens the case for measuring rather than weakens it: **no constant can encode a limit whose
mechanism is unidentified.** `gpu_budget_preferring_measurement` now prefers what `--gate-vram` found,
falls back to the constant when unprobed, and is still bounded by RAM — honouring an allocation is not
the same as it being wise to make.

## Explains: display stutter and watchdog pressure

**AGX has one hardware queue, shared with the compositor**, and `DRM_ASAHI_PRIORITY_HIGH` exists while
we run at default. Every "the desktop stutters during inference" observation in this project has that
as a sufficient explanation, and the submit-pacing work in _270 was treating a symptom of it — badly
enough that it ratcheted the autotuner's value and had to be reverted in _308.

wgpu does not expose DRM queue priority, so this is not a logscope change. It is worth knowing that the
lever exists one layer down.

## Validates: mmap residency management

**There is no shrinker in the driver** — no purge, no evict, no madvise. GPU buffers are pinned for
life. Good for us (no eviction stall mid-inference) and bad (a resident model fights page cache, and the
OOM killer cannot reclaim it).

That is the direct justification for `release_stale_map_residency` (_288) and the live-view guard
(_287): if the kernel will not reclaim, handing back page cache ourselves is the only mechanism there
is.

## Closes: no matrix units to reach for

M1/M2 AGX has none. `VK_KHR_cooperative_matrix` would lower to the same FMAs already being issued, so
there is no gain to chase. The matrix hardware on this SoC is the ANE, behind a driver that has no
prime, no dma_buf, and no import path — every GPU→NPU tensor is a CPU copy, unavoidably, today.

Combined with the measured CPU-inference opportunity in `cpu-inference-lessons.md`, the practical
conclusion is unchanged: the CPU is the second processor worth using here, not the NPU.
