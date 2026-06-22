# Dynamic prompt + multi-stage LLM reasoning

User's ideas from v0.1.62 turn that warrant their own focused session.

## 1. Model-size-modulated prompts

Instead of a single hard-coded prompt, parameterize prompt complexity by
the model's capacity:

- 1B-class (tinyllama, gemma:2b, phi3:mini): few-shot patterns. ≤4 examples.
  Strict schema. Single-action output. They follow patterns reliably but
  reason poorly — give them shape, not freedom.

- 3-7B (llama3.1:8b, qwen2.5:7b, mistral): more directions, more examples
  (8-12), some reasoning prelude permitted. Can handle multi-action
  sequences in a single response.

- 13B+ (llama3:70b, command-r, gpt-class): treat as collaborator. Free-form
  task description + constraints. Can chain actions, ask clarifying
  questions, refuse ambiguous requests.

Implementation sketch:
- Detect param count from model name (parse `2b`/`8b`/`70b`/`q4`/etc.) or
  query `ollama show <model>` for parameters.
- Per-tier prompt template with placeholder slots for examples + task.
- Cache the parameter-count lookup since it doesn't change per session.

## 2. Dynamic example selection

Static prompts ship the same ~4 examples for every utterance. Dynamic
would be: index a corpus of (utterance → action) pairs, retrieve top-K
most similar to the current utterance via embedding similarity, plug
those K into the prompt.

Effect: model sees examples that ACTUALLY look like the current input,
not generic ones. Big quality win for 1B models that can't generalize
well.

Implementation:
- Build the corpus: (utterance, intent_kind, params, was_accepted) tuples
  from past confirmations + manual labels
- For embeddings: ollama's `nomic-embed-text` is fast (~50ms/query) and
  small (~70M params). Or stay zero-deps: simple character n-gram
  Jaccard similarity which is 100x faster and good enough at this scale.
- Top-K retrieval on each LLM call, K=3..6 based on tier
- Cache embeddings; only re-compute on corpus update

## 3. Multi-stage "thinking longer" chain

User insight: instead of one giant prompt with 100 options, do K small
1-vs-1 (or 1-vs-few) ranking calls. Each call is cheap; total budget
under what a single big-context call would cost.

Pattern:
- "Choose between 100 candidates" → tournament: split into 25 groups of
  4, pick best from each (25 calls); split 25 into 5 groups of 5, pick
  best (5 calls); final pick from 5 (1 call). 31 calls total but each
  is tiny.
- Smaller models can run in parallel (gemma:2b can do ~3-5 simultaneous
  on typical desktop) — so wall-clock time can be 5-10x faster than
  sequential big-context.

Implementation:
- Async dispatch of N sub-prompts via `ollama_query_model` already exists
- Need a coordinator that holds intermediate results
- "Thinking longer" UX: after the small model returns, show that result
  immediately; then run the big-context refinement in the background;
  if its answer differs, update with a small fade transition.

## 4. Desktop-only progressive enhancement

Tying multi-stage to "fastest model finishes quickly" gives a free UX
upgrade on desktop without slowing mobile. Test: time gemma:2b on a
short prompt; if it returns under 500ms, this device has headroom for
multi-stage chaining. Below that threshold, fall back to single-call.

State to track per session:
- Per-model wall-clock latency (rolling average of last 5 calls)
- Per-model success rate (parses to valid JSON)
- Choose strategy from these stats

## 5. Investigating ollama intent prompt repetition

User reported the model "might be getting stuck repeating". Need to
gather data:
- Log full LLM responses (currently we just log "succeeded with action=X")
- Look for: same response across different utterances, runaway repetition
  inside a single response, identical action+arg combos in close
  succession
- If confirmed, fix could be: setting `temperature: 0.1` to be sure not
  fully greedy (some repetition is actually frequency-induced); shuffling
  the example order each call so the model doesn't lock onto a single
  pattern; adding `frequency_penalty` if ollama supports it.

## Estimated sizes

- Model-size modulation alone: ~150 lines, half session
- Dynamic example selection (with n-gram, no embeddings): ~250 lines, one session
- Dynamic example selection (with embeddings): ~400 lines, two sessions
- Multi-stage chaining: ~300 lines + UX design, two sessions
- Repetition investigation: ~50 lines of logging + analysis, half session

Suggested order: investigation first (cheap, informs the rest), then
model-size modulation (foundational), then dynamic examples, then
multi-stage. The latter two are independent and can swap order.

## 6. AI binary classifier for ambiguous fragments

User idea v0.1.64: when our local heuristic marks an utterance as
AMBIGUOUS (could be a fragment or could be complete), fire an ollama
binary prompt in a background thread asking "is this a partial thought?
yes/no". Short timeout (~200ms). Cancellable.

The cancellation is the key: if the user replies (yes/no/anything) before
the AI returns, kill the AI subprocess. So the AI never delays user
decisions; it only fills in when the user is silent and we'd otherwise
have to commit blind.

Architecture:
- Backend: `fragment_check_async(text) -> handle` returns a token, spawns
  ollama subprocess with 200ms timeout
- Backend: `fragment_check_cancel(handle)` kills the subprocess
- Backend: `fragment_check_poll(handle)` returns "" / "fragment" / "complete"
- QML: on AMBIGUOUS, call _async, store handle, start poll timer
- QML: if user yes/no/etc. arrives, call _cancel
- QML: if poll returns "fragment" before user reply, retroactively
  switch to fragment-completion mode

Prompt design (binary, terse, 1B-friendly):
```
Is this an incomplete sentence cut off mid-thought?
Reply with one word: yes or no.

"open the file"
no

"go to the"
yes

"i want to"
yes

"this is fine"
no

"<user_text>"
```

Estimated size: ~100 lines QML + 80 lines Rust. Half session.
