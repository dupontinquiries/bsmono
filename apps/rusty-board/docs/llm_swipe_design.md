# LLM-Assisted Swipe Disambiguation

Status: **scaffold landed, FFI deferred**. Module compiles in mock
mode; switch on `--features llm-swipe-ffi` to use a real local
Llama 3.2 GGUF.

## TL;DR

The current shape-based recognizer (`gestures/recognizer.rs`) returns
top-N word candidates in <1ms. It's already very good at "what
dictionary word fits this path geometrically." It's weak at
context-aware disambiguation: when top candidates have near-tied
scores, falling back to word frequency picks the wrong word.

This module adds a **re-ranker**: when the top candidates are
ambiguous, a small local LLM picks the right one given the
surrounding text. The LLM emits a single digit (the candidate
index), so it's a 1-token forward pass — not a full generative
round-trip.

## Why re-ranker, not generator

Hardware reality on a Fairphone 5 (Snapdragon 7+ Gen 1, 8 GB RAM,
no working GPU offload through Vulkan as of pmOS 24.06):

- Llama 3.2 1B Q4 GGUF: ~700 MB on disk, ~1.5 GB RAM at inference
- Decode speed: ~10-15 tok/s CPU-only on the 7+ Gen 1's big cores
- Pure-LLM generation ("here's the key sequence, predict the word"):
  prompt ~30 tokens + output ~5 tokens = **150-300 ms per swipe**
- Re-ranker ("pick which of these 5 is correct"): prompt ~80 tokens
  but only **1 output token** = **50-100 ms per swipe**

And the re-ranker only fires when needed:
- Single candidate → skip
- No surrounding text → skip
- Top candidate's score is >0.5 ahead of #2 → skip (confident win)

In practice that's about 15-20% of swipes. Amortized cost: **10-20 ms
per swipe**, vs <1 ms baseline. Acceptable.

## What's in this scaffold

`src/gestures/llm_swipe.rs`:

- `LlmSwipeReranker::load(model_path)` — loads once at startup,
  ~1-3 sec on phone for a 1B model
- `LlmSwipeReranker::rerank(candidates, surrounding_text)` — fast-
  path checks + LLM call when needed
- `build_prompt` — constructs a digit-only response prompt
- `parse_response` — tolerant of the LLM echoing/decorating the digit
- 15 unit tests covering fast paths, prompt building, response parsing

`src/config/mod.rs`:

- `GestureConfig::llm_swipe_enabled: bool` (default false)
- `GestureConfig::llm_swipe_model_path: String` (default empty →
  platform data dir + `rusty-board/models/llama-3.2-1b-instruct-q4_k_m.gguf`)

## What's deferred

1. **The llama-cpp-2 dependency**. Adding it to `Cargo.toml` requires
   bringing in `clang` + a usable compute backend (Vulkan/Metal/CPU)
   in the build environment. The dev sandbox here doesn't have it
   and cross-compile to ARM64 Alpine is its own project. Wire on
   your dev box, not in CI.

2. **Integration into the swipe-word commit path** in `window.rs`.
   The right place is between the `SwipeWord` decode in the gesture
   handler and the `engine.handle_input(EngineInput::SwipeWord(...))`
   call. Pseudo-code:

   ```rust
   // Currently:
   let candidates = recognizer.recognize_with_speed(path, hitboxes, vel);
   let word = candidates.first().map(|c| c.word.clone()).unwrap_or_default();
   engine.handle_input(EngineInput::SwipeWord(word));

   // After integration:
   let candidates = recognizer.recognize_with_speed(path, hitboxes, vel);
   let surrounding = engine.state_snapshot().surrounding_text;
   let word = match &llm_reranker {
       Some(r) => r.rerank(&candidates, &surrounding)
                   .unwrap_or_else(|_| candidates[0].word.clone()),
       None => candidates.first().map(|c| c.word.clone()).unwrap_or_default(),
   };
   engine.handle_input(EngineInput::SwipeWord(word));
   ```

3. **Settings panel toggle**. Add a row in `settings_panel.rs`
   under "Gestures":

   ```rust
   scroll_inner.append(&setting_row(
       "LLM swipe disambiguation",
       "Context-aware re-ranking of ambiguous swipes (~50ms cost)",
       &["Off", "On"],
       if cfg_state.borrow().gestures.llm_swipe_enabled { 1 } else { 0 },
       move |idx| {
           cfg_state.borrow_mut().gestures.llm_swipe_enabled = idx == 1;
           notify();
       },
   ));
   ```

## Bringing up the FFI

On your laptop:

```bash
# Add to Cargo.toml dependencies:
llama-cpp-2 = { version = "0.1", optional = true }
encoding_rs = "0.8"  # already in via your logscope-suite

# Add features to Cargo.toml:
[features]
llm-swipe-ffi = ["llama-cpp-2"]
default = ["..."]  # don't add llm-swipe-ffi to default

# In src/lib.rs and src/main.rs, behind cfg(feature = "llm-swipe-ffi"):
mod llm_backend;  // copy of your logscope llm_backend.rs

# Build:
cargo build --release --features llm-swipe-ffi
```

The `llm_swipe.rs` module is already set up to use `crate::llm_backend`
when the feature is on.

## Where it likely shines

- Proper names in context: "I met **Sam**" vs "I met **Sue**" — the
  recognizer can't tell from path alone if both are in the dictionary
- Homophone-ish dictionary words: "store" vs "stoke" vs "stove"
- Sentence-end disambiguation: "the **end**" vs "the **and**"
- Domain context: technical writing biases toward technical terms

## Where it doesn't help

- First word of a new utterance (no context yet)
- When the dictionary doesn't have the right word at all (you'd
  need a generator, not a re-ranker)
- When the recognizer's #1 candidate is just wrong by a wide margin
  (re-ranker is constrained to the top-5)

## Future: training-free improvements

Things to try without leaving the re-ranker architecture:

1. **Few-shot examples in the prompt** — include 2-3 examples of
   "context + candidates + correct pick" before the actual query.
   Costs ~30 more prompt tokens but can dramatically improve
   small-model accuracy.

2. **Sampling top-K from the recognizer based on entropy**, not a
   fixed 5. If the top 3 are near-tied and #4-5 are far behind,
   only show the LLM 3. Saves tokens.

3. **Reverse logits** — instead of having the LLM generate a digit,
   compute the conditional probability of each candidate word
   given the context (one logits-read per candidate, no sampling).
   This is what llama.cpp's `--logit-bias` is for. Faster and more
   reliable than digit-output.

4. **Layout-aware candidate generation** — the current recognizer
   only considers dictionary words. For proper names / out-of-
   dictionary, you could let the LLM generate candidates from a
   noisy key sequence ("user passed through k-a-t — what words?")
   as a FALLBACK when the recognizer's confidence is low across
   the board.

## Open question: model choice

Llama 3.2 1B is the sweet spot for FP5 right now. Alternatives:

- **Qwen 2.5 0.5B** — even faster (~25 tok/s), but accuracy drop
  is noticeable for non-English context
- **Phi-3.5 mini** (3.8B) — much better accuracy, but borderline
  for FP5 RAM and ~3x slower per token
- **TinyLlama 1.1B** — fastest but worst accuracy

For the re-ranker workload (pick a digit, very short context),
1B is probably more than enough. Worth benchmarking on actual
swipe ambiguity cases once the FFI is wired.
