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
