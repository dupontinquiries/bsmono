# What the llama.cpp run tells us about our wgpu backend

Read from a real run on the M2 Ultra (`log.log`, 1098 lines). Four findings,
and the first two invert the usual assumption.

## 1. We support an architecture llama.cpp does not

```
llama_model_load: error loading model: unknown model architecture: 'llada2'
worker: load failed: failed to load GGUF 'LLaDA2.0-mini-preview-Q4_0.gguf'
```

Our wgpu backend has `llada2.rs` and an `archcfg` entry for it. llama.cpp
refuses the file outright.

**Do not "fix" this by routing llada2 to llama.cpp as a fallback.** The
fallback does not exist. Any code that treats llama.cpp as the safe option for
an unfamiliar architecture will fail on exactly the models where our backend is
the only one that works. Worth an explicit check at routing time rather than a
runtime discovery.

## 2. The Vulkan backend did not load at all

```
worker: default backend = llama.cpp · Vulkan (llama.cpp)
llama_context: backend_ptrs.size() = 1
load_tensors: layer 0..32 assigned to device CPU
load_tensors:   CPU_Mapped model buffer size =  4653.80 MiB
```

**33 layers on CPU, 0 on GPU**, on a build labelled Vulkan. `backend_ptrs.size()
= 1` means only the CPU backend registered — the Vulkan one was never
initialised, so this is not "offload was disabled", it is "the GPU backend is
absent".

Two things follow:

* **Any timing comparison against this log is meaningless.** It is a CPU run
  wearing a Vulkan label, and comparing our GPU numbers against it would flatter
  us enormously.
* **The label lies before the failure does.** Our own backend banner prints the
  adapter name (`adapter = Apple M2 Ultra (G14D B1) [Vulkan]`) which at least
  proves a device was found. Worth keeping — and worth checking that we would
  not print a backend name we then failed to initialise.

## 3. `CPU_REPACK` is a real technique we do not have

```
repack: repack tensor blk.0.attn_q.weight with q4_K_8x8
load_tensors:   CPU_REPACK model buffer size =  3204.00 MiB
```

llama.cpp rewrites q4_K weights into a `q4_K_8x8` interleaved layout at load
time so the CPU kernel can read 8 rows at once. That is the same *shape* of idea
as our arena packing, applied to the value layout rather than the allocation.

Relevant to us because the Asahi GPU reads weights through a similar
coalescing constraint. If a resident quantised layout were interleaved for the
workgroup width, the decode kernel would fetch fewer, wider blocks. This is a
hypothesis, not a measurement — but it is the first concrete thing in this log
worth trying.

Note also:

```
tensor 'token_embd.weight' (q4_K) (and 98 others) cannot be used with preferred
buffer type CPU_REPACK, using CPU instead
```

99 tensors fall out of the fast path and it says so per-buffer-type. Our
equivalent — `int8 cache SKIPPED — source is ggml type 12` — already does this;
worth keeping the habit.

## 4. Context is capped well below the model's training length

```
print_info: n_ctx_train = 8192
llama_context: n_ctx     = 4096
llama_context: n_batch   = 4096
```

Halved, silently. We print the model's own dimensions at load; we do not
currently say when the *runtime* context is below what the model was trained
for. That is a quality difference a user would attribute to the model.

## What not to conclude

The log contains **no successful GPU generation**, so it says nothing about
llama.cpp's Vulkan speed, kernel quality, or memory ceiling on this hardware. It
is evidence about packaging and architecture coverage, not about performance.
