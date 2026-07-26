# Why llama.cpp is faster, and whether it escapes our size ceiling

Read from the llama.cpp tree (`ggml/src/ggml-vulkan/ggml-vulkan.cpp`, `src/llama-model.cpp`,
`src/llama-mmap.cpp`). Findings, and what each one means for us.

## 1. The big one: weights are never uploaded at all

`src/llama-model.cpp:1574`

```cpp
ggml_backend_buffer_t buf = ggml_backend_dev_buffer_from_host_ptr(
        dev, (char *) addr + first, last - first, max_size);
```

`addr` is the **mmap'd GGUF**. The backend imports that host pointer *as a GPU buffer*. On Vulkan
(`ggml_vk_buffer_from_host_ptr`, ggml-vulkan.cpp:18139) this is `VK_EXT_external_memory_host`:

```cpp
if (!device->external_memory_host) return {};
if (uptr & (device->min_imported_host_pointer_alignment - 1)) return {};
if (size & (device->min_imported_host_pointer_alignment - 1)) return {};
const vk::MemoryPropertyFlags property_flags =
      eHostVisible | eHostCoherent | eHostCached;
buf = ggml_vk_create_buffer(device, size, { property_flags }, ptr);
```

So on a UMA machine the GPU reads the file's **page-cached pages directly**. There is no upload, no
staging, no host copy, and no device allocation for the weight bytes.

This is the same mechanism Colibri uses on Metal (`newBufferWithBytesNoCopy` over file-backed pages —
*"niente copia: la page cache del kernel È la cache"*). Two independent engines converged on it, which is
a strong signal it is the right shape.

**What it means for our ceiling.** Our "Failed to allocate BO VMA" comes from *allocating* device buffers
for weights. llama.cpp does not allocate them — it maps pages that already exist. That plausibly sidesteps
the budget we hit, though it is not proven: imported memory still needs GPU VA, and whether Honeykrisp
accounts for it against the same pool is unknown from here.

**What it means for us practically: wgpu cannot do this.** There is no safe wgpu API for importing host
memory as a buffer. This is a capability gap in our stack, not a design choice we made — and it is the
single biggest structural difference between our engine and llama.cpp on this hardware.

**MEASURED on the M2 Ultra (`--gate-zerocopy`, 2026-07):**

```
adapter = Apple M2 Ultra (G14D B1) [Vulkan]
VK_EXT_external_memory_host advertised: no
```

**Not available.** So on this hardware llama.cpp cannot import either — it allocates exactly as we do, and
the BO-VMA ceiling is a genuine driver limit that no API choice, ash, or unsafe code dodges. The
`zerocopy.rs` comment that predicted this was correct.

Two consequences worth stating plainly:

* **The streaming architecture was the right call.** It is not a workaround for a self-inflicted problem;
  it addresses a real hardware constraint that the reference implementation also faces.
* **llama.cpp cannot be holding Scout fully resident on this device either.** A 62 GB checkpoint against a
  ~24 GiB ceiling means it must be running partially on CPU (`-ngl` below full). So its speed advantage on
  a model this size is not "it fits and ours does not" — it is a different split of the same problem, and
  our fully-resident-plus-streaming approach is arguably the more ambitious one. Worth measuring
  `llama.cpp -ngl` behaviour on Scout before treating its throughput as a target.

Note llama.cpp explicitly *disables* this on MoltenVK (ggml-vulkan.cpp:6090), so on macOS/Metal it takes
the Metal backend's equivalent instead. The mechanism is not universally available.

## 2. Allocation sizing: they read the real limits, we assumed one

```cpp
device->max_memory_allocation_size = std::min(props3.maxMemoryAllocationSize, props4.maxBufferSize);
device->suballocation_block_size   = 1024*1024*1024;                       // 1 GiB default
device->suballocation_block_size   = std::min(suballocation_block_size, max_memory_allocation_size);
```

They query `maxMemoryAllocationSize` **and** `maxBufferSize` and take the minimum, then suballocate
tensors out of ~1 GiB blocks. `GGML_VK_SUBALLOCATION_BLOCK_SIZE` and
`GGML_VK_FORCE_MAX_ALLOCATION_SIZE` let a user override both.

Our arena does the same thing at 2047 MiB slabs, so we are not far off here — but we derived our cap from
an empirical crash, while they read it from the device. Worth adopting: query both limits rather than
hard-coding, and expose the override.

## 3. Where the rest of the speed comes from

Not memory at all:

* **Graph execution.** ggml builds the whole forward graph and `ggml-alloc` plans every intermediate
  buffer up front, so a token is a small number of command-buffer submissions rather than one per op.
  Our profile shows 1152 submits/token on Scout; this is the structural reason theirs is lower.
* **Async submission** (`device->support_async`, disabled per-vendor where broken) overlaps recording
  with execution.
* **Fused kernels** for the common op chains, so fewer dispatches per layer.

## 4. What this changes for us, in priority order

1. **Run the `vulkaninfo` check.** It decides whether the ceiling is an API limitation or a hardware one,
   and that answer redirects everything else.
2. **Submits per token is the real speed gap**, not upload bandwidth — our own profile already said
   GPU-sync-bound with 79.9% accounted. Graph-level batching is the fix, and it is a large piece of work.
3. **Query the device limits** instead of hard-coding the arena slab cap.
4. `src/models/<arch>.cpp` is one file per architecture and is the reference to check BEFORE assuming our
   implementation of a model is complete. It is what identified the missing Llama-4 `use_kq_norm`.

## 5. Non-blocking uploads: what llama.cpp has, and why it does not help here

Three mechanisms in `ggml-vulkan.cpp`, in order of how much they would matter:

**Dedicated transfer queue.** llama.cpp looks for a queue family with TRANSFER but *not* compute/graphics —
a real DMA engine that runs concurrently with compute:

```cpp
transfer_queue_family_index = ggml_vk_find_queue_family_index(
        queue_family_props, eTransfer, eCompute | graphics_flag, compute_queue_family_index, 1);
device->single_queue = compute_queue_family_index == transfer_queue_family_index
                       && queue_family_props[compute_queue_family_index].queueCount == 1;
...
} else {
    device->transfer_queue = ggml_vk_create_aliased_queue(device, device->compute_queue);
    device->async_use_transfer_queue = false;      // fall back to the compute queue
}
```

Apple GPUs expose a single queue family, so `single_queue` is true and llama.cpp **aliases the transfer
queue onto the compute queue and disables async transfer**. It gets no overlap here either — which
independently explains our own M2 probe result that slot fills do not overlap compute. That was measured
before we knew why; this is why.

wgpu exposes one queue regardless, so this is doubly moot for us.

**Pinned host memory** (`pinned_memory`, registered host allocations) and a **persistent staging buffer**
(`sync_staging`). We now have the second (one staging buffer reused for every expert fill, _180). The
first is not reachable through wgpu.

**Conclusion: upload mechanism is not our problem.** The M2 probe already measured staged `write_buffer` at
~32 GiB/s versus 4–14 GiB/s mapped, and no faster path is available on this hardware to either engine.

## 6. Where the time actually goes — the profile says it plainly

```
4610 submits (1152/tok), 193 polls | setup 7ms vs sync 3047ms of 3822ms (GPU-sync-bound, 79.9% accounted)
```

**193 polls / 4 tokens ≈ 48 per token = one per layer.** That is `moe_ffn_v1` downloading the router
logits every layer to decide routing on the CPU. Each one is a full GPU→CPU sync, and together they are
the 3047 ms.

llama.cpp never does this: top-k routing is part of the graph (`ggml_top_k`/argsort), so nothing leaves
the GPU mid-forward. Our v2 GPU-routed path does the same — but **streaming forces v1**, because the CPU
must know which expert to upload before it can upload it.

So the sync is not incidental, it is the price of streaming as currently designed. The ways out, roughly
in order of payoff:

1. **Pipeline the readback.** Use layer L's routing to prefetch for the *next* token rather than blocking
   on it for this one, so the sync overlaps compute instead of stalling it. The coupling predictor
   (`expert_predict.rs`) already exists for exactly this and measured 72% accurate.
2. **Keep enough experts resident that the common case needs no fill**, and only sync when the ring
   misses. With a good pin set (repin measured 0%→62%) most layers would need no CPU decision at all.
3. **Batch the syncs**: read back several layers' routing in one poll where the dependency allows.

None of these is an upload optimization. The upload is already as fast as this hardware offers.

## 7. Where our ceiling actually comes from — it is a Mesa policy, not hardware

Read from `mesa-main/src/asahi/lib/agx_device.c`. The advertised limit is not the constraint:

```c
// hk_physical_device.c
.maxMemoryAllocationSize = (1ull << 37),      // 128 GiB advertised
```

The real cut happens when Honeykrisp sets up the user VA window:

```c
// agx_device.c, agx_open_device()
uint64_t kernel_size = MAX2(dev->params.vm_kernel_min_size, 32ull << 30);   // 32 GiB for the kernel
uint64_t user_size   = vm_create.kernel_start - user_start;

/* Round the user VA window to powers-of-two... */
user_start = util_next_power_of_two64(user_start);
user_size  = util_next_power_of_two64(user_size + 1) >> 1;

/* ...so when we cut user size in half to emulate sparse buffers... */
user_size /= 2;

/* ...or maybe in quarters if necessary to disambiguate */
if (user_size == user_start) {
    user_size /= 2;
}

dev->sparse_ro_offset = user_size;
util_vma_heap_init(&dev->main_heap, user_start, user_size);
```

Three reductions in a row, before a single byte is allocated:

1. **32 GiB** off the top for the kernel heap.
2. **Round DOWN to a power of two** — worst case loses almost half of what remains.
3. **Halve it** (and possibly halve again) so one address bit can distinguish a read-only sparse alias of
   every address. This is how Honeykrisp emulates sparse buffers without hardware support for them.

`util_vma_heap_init(&dev->main_heap, user_start, user_size)` is the heap that "Failed to allocate BO VMA"
comes from (agx_device.c:211). So our ~24 GiB ceiling is **address space Mesa reserved for a feature we do
not use**, not a limit of the M2 or of the kernel.

### What this means

* **The ceiling is soft.** A local Mesa build with the sparse halving removed would roughly double the
  usable VA window. The cost is that sparse buffer support breaks — which we do not use, and which is
  ironic given `gpu-gemm` has a `sparse` feature: on this driver, sparse residency is *paid for* out of
  exactly the address space it would then manage.
* **This is worth confirming before acting on.** The numbers should be checked against the real device:
  `dev->params.vm_end`, `vm_kernel_min_size`, and the resulting `main_heap` size. If the machine's user VA
  is, say, 64 GiB before the halving, the arithmetic above lands close to the ~24 GiB we observe, which
  would confirm the reading.
* **Shipping a patched driver is a real option but a heavy one** — it means a forked Mesa on the user's
  machine, rebuilt against each upstream release. Upstreaming a flag (`ASAHI_NO_SPARSE=1` skipping the
  halving) would be the durable version, and is a small, well-motivated patch: the code already comments
  exactly why the halving exists.
* **A kernel change is NOT needed.** Everything above is userspace Mesa. That is a much lower bar than the
  question assumed.

### Honest caveat

This is read from source, not measured. The next step is a small probe that prints the actual heap bounds
on the M2 (the values are in `dev->params`), which would turn "the arithmetic suggests ~24 GiB" into a
measured fact — the same discipline that settled the external-memory-host question.
