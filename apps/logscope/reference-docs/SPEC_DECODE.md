# Speculative decoding — measuring tok/sec

Speculative decoding raises tokens/sec by letting a small **draft** model propose a block of `K`
tokens that the big **target** model verifies in **one** forward pass. Every accepted guess is a
token the target didn't have to generate serially, so a single target pass can emit up to `K+1`
tokens. It is **lossless for greedy decode**: the emitted text is bit-exact to what the target
produces on its own — the speedup has no quality cost. The whole win is set by *acceptance*: how
often the draft's guesses match the target. Predictable spans (code, boilerplate, repetition)
accept many tokens; hard/creative tokens fall back to ~1 (i.e. normal speed), never worse.

## Requirement: draft and target must share a tokenizer/vocab
Pick a smaller model from the **same family** as the draft, e.g.
- Llama-3.2-1B-Instruct (draft) for Llama-3.1-8B-Instruct (target)
- Qwen2.5-0.5B/1.5B (draft) for Qwen2.5-7B (target)

If the vocabs differ, spec-decode is refused (the bench prints `VOCAB MISMATCH`; the app logs
`spec draft vocab … != target …` and stays on normal decode).

## 1) Precise bench — `kernel-validate --spec`
The ground-truth measurement: it runs the target's **serial greedy** baseline and the
**draft+target speculative** path back to back, on the same tokenized prompt, and reports both
wall-clock tok/s, the speedup, whether the outputs are identical, and the mean accepted draft
length.

```
cargo run --release --bin kernel-validate -- \
  --spec  /path/target.gguf  /path/draft.gguf  [n_new=64]  [K=4]  ["your prompt"]
```

Example output:
```
spec: baseline 31.2 tok/s (…ms) | speculative 58.7 tok/s (…ms) | speedup 1.88x | identical-output true
  mean accepted 3.1 of K=4 per round (… rounds, … tokens) → ~4.1 tokens per target verify-forward
```
`identical-output true` is the losslessness check. Sweep `K` (try 3–6) to find the best block size
for a given draft/target pair — too large wastes target compute on rejected tails, too small caps
the win.

## 2) In-app — `LOGSCOPE_SPEC_DRAFT`
To use spec-decode in the actual chat (wgpu engine), point the env var at a draft GGUF before
launching; load your **target** model as usual. Every reply then decodes speculatively and logs its
rate.

```
LOGSCOPE_SPEC_DRAFT=/path/draft.gguf  LOGSCOPE_SPEC_K=4  <run the app, load the target model>
```

Per-reply log line:
```
wgpu-engine: [target.gguf] SPECULATIVE k=4 — 128 tokens in 2.2s = 58.1 tok/s | 3.1 accepted/round × 41 rounds (~4.1 tok per target verify-pass)
```

Knobs: `LOGSCOPE_SPEC_K` sets the proposed block size (default 4). Unset `LOGSCOPE_SPEC_DRAFT` to
return to normal streaming decode.

## Notes / current limits
- **Greedy only.** The spec path decodes greedily (the lossless-greedy rule); the temperature slider
  is ignored while a draft is loaded. Sampled speculative decode (the lossless `verify_sampled`
  rule, already in `logscope::specdecode`) is not wired into the engine loop yet.
- **In-app spec bypasses KV-tree reuse.** The spec path manages its own target+draft caches, so it
  doesn't share the branch/fan-out prefix-reuse pool. For a single linear chat that's fine; the
  bench is the cleanest place to read the raw speedup.
- The speedup is **GPU- and pair-dependent** — it scales with acceptance, which depends on how well
  your draft tracks your target and on the content. Measure on your box; the bench's wall-clock
  numbers are the truth.
