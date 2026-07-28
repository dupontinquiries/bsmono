# What to run, and what each answer would mean

Ordered by how much the answer changes what to do next.

## 1. The VRAM ceiling is measured, not guessed — confirm the budget uses it

```bash
LOGSCOPE_PROBE_MAX_GB=64 LOGSCOPE_PROBE_SLAB_MB=1024 \
  kernel-validate --gate-vram
```

Last run: **63.0 GiB verified** against a 24.0 GiB assumption. The cap was measuring the *staging*
upload path, which the arena removed.

If it pops again near 63, the number is stable and `LOGSCOPE_MAX_GPU_GB=45` is a reasonable working
value — enough for Scout resident with room for KV. Watch for **loads but produces wrong output**: a
silent overcommit is the dangerous outcome, and it looks like a bad model rather than a failed
allocation.

## 2. The CPU path — the one thing this container could not measure at all

```bash
cargo run --release --example cpu_bench      # in crates/gpu-gemm
cargo run --release --example spawn_cost
```

Two numbers matter, and they answer different questions:

* **single-core GiB/s.** Here it is 1.9 (Q8_0) and 1.1 (Q4_K) on one x86 core. If the M2 comes back
  near 10+, the inner loop is vectorizing on NEON and threading is the remaining story. If it is still
  ~2, the loop is *not* vectorizing and that is where effort should go before anything else.
* **scaling to 24 threads.** This box reports `available_parallelism = 1`, so every threading number so
  far is meaningless. `spawn_cost` also decides whether the persistent pool earns its place — **if the
  pool does not beat `thread::scope` there, delete it**; it is committed on llama.cpp's evidence, not
  on a measurement I could take.

## 3. Scout, on a run with the current fixes

```bash
LOGSCOPE_DIAG=1 LOGSCOPE_MAX_GPU_GB=45 \
  cargo r --release --features wgpu-engine --bin logscope-egui
```

Send the diag zip. The lines worth reading:

* `submit causes` — DeepSeek was 96% batched, Scout 80% *no-frame*. The gap is the staging fence, and
  Q6_K conversion is what removes it rather than making it cheaper.
* `tokenizer: N byte(s) dropped` — should be **absent**. If it appears, the prompt is not the text you
  typed and nothing downstream matters.
* the first assistant reply to a plain "hi there" — it should no longer discuss `GET /health`. That was
  the built-in `sample.log` being attached with "use only this data", now fixed.

## 4. Consensus mode

Nothing drives it yet, so this is a read rather than a run. What it will do when wired:

* `ROUND_ONE` does not tell a panellist it is on a panel — priming for agreement would defeat the
  independence measurement before it could take one;
* agreement is only credited once the panel has **demonstrated it can disagree**. A panel that never
  disagreed is one model queried repeatedly, and that escalates rather than settling;
* the bandwidth case is the justification: 4x3B reads ~6.7 GiB/token against one 30B's ~17, so the
  panel gets about two and a half rounds before it costs what the single model costs.

## Still open, honestly

* **The `2, of` gibberish.** Ruled out: the GPU path (two drivers agree byte-for-byte), memory (32 GiB
  resident, no timeouts, same output), the MoE forward (now verified against a CPU reference), and
  dense models (DeepSeek is coherent). What remains is the prompt going in and the sampler coming out.
* **~4000 unattributed submits** per run — a path still untagged.
* **Qwen3-30B at 71% no-frame** where it was 12%; the raised budget moved it onto a different path.
* **The CPU forward is unassembled.** Every primitive exists and is cross-checked against the shaders;
  nothing wires them into a decode loop yet.
