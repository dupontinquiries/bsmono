# Fan-Out Architecture: Realtime Dictionary + Subtle Gesture Manipulation

Status: **AdaptiveDictionary landed, ProgressiveRecognizer + HitboxNudger
sketched, integration deferred.**

## The vision

Instead of one big LLM, a swarm of small specialized components.
Each contributes statistical signal to a consensus prediction.
Predictions update CONTINUOUSLY during the swipe (not just at end)
and subtly bias the path-to-key mapping as the user draws.

The four pieces:

```
              ┌──────────────────────────────────────┐
              │  PROGRESSIVE RECOGNIZER (every       │
              │  path-update: 5-10 ticks per swipe)  │
              └────────────────────┬─────────────────┘
                                   │
                  ┌────────────────┼────────────────┐
                  ▼                ▼                ▼
        ┌─────────────────┐  ┌────────────┐  ┌────────────────┐
        │ Shape recognizer│  │ Adaptive   │  │ Context n-gram │
        │ (current)       │  │ dictionary │  │ (next-word LM) │
        │ <1ms / call     │  │ <0.5ms     │  │ <2ms           │
        └────────┬────────┘  └─────┬──────┘  └────────┬───────┘
                 │                 │                  │
                 └─────────────────┼──────────────────┘
                                   │
                         ┌─────────▼─────────┐
                         │  CONSENSUS SCORER │
                         │  weighted blend   │
                         └─────────┬─────────┘
                                   │
                  ┌────────────────┼────────────────┐
                  ▼                                 ▼
        ┌─────────────────┐                  ┌──────────────┐
        │  HITBOX NUDGER  │                  │  CANDIDATE   │
        │  bias next-key  │                  │  RIBBON      │
        │  hitboxes by    │                  │  show top-3  │
        │  predicted next │                  │  during swipe│
        └─────────────────┘                  └──────────────┘
```

## What landed

### `gestures/adaptive_dictionary.rs`

Per-user word-frequency tracker. Every swipe commit feeds it.
Persists to `~/.local/share/rusty-board/adaptive_dictionary.toml`
via atomic_write. Exposes `boost(word) -> f64` for the recognizer
to consult.

- Exponential decay (30-day half-life) so old vocab fades
- Capped at 1500 entries; lowest effective-count evicted at cap
- Case-insensitive
- Whitespace/punctuation-only commits ignored
- `forget_all()` for privacy-conscious users
- `top_n()` for the settings panel to show "your most-used swipe words"

13 tests including the canonical "Sue vs use" disambiguation case.

### `gestures/hitbox_nudger.rs`

The "subtle gesture manipulation" piece. Given the current top
candidates and the chars decoded so far, predicts which keys
the user is most likely to pass through next and biases the path-
to-key snapping toward them by up to 20%.

- `update_from_candidates(&[WordCandidate], chars_so_far)` — recomputes
  per-key nudge multipliers from the prediction
- `snap_with_nudge(x, y, keys, nudger)` — drop-in replacement for the
  existing distance-based snap; nudge-weighted
- `decode_with_nudge(path, keys, nudger)` — drop-in replacement for
  `decode_swipe_path`; returns the raw decoded string with nudges
  applied at pivot snap
- Empty nudger → byte-for-byte equivalent to the existing unnudged
  behavior (safety fallback)
- 13 tests covering nudge math, cap enforcement, threshold flipping,
  case-insensitivity, clone independence, and pass-through
  equivalence

**Risk mitigations (per design):**
- Nudges capped at 1.2× (max 20% boost). Strong actual finger
  signal still wins.
- The candidates that DRIVE the nudges should come from the
  UNNUDGED raw decode of the path-so-far. The swipe handler must
  enforce this to prevent feedback loops.
- Maximum 5 candidates considered per tick.

### `gestures/ngram.rs`

User's personal bigram + trigram next-word model. Tracks how the
user's typed words actually follow each other; provides a score
bonus the recognizer can blend into candidate ranking to break
ties using conversational context.

- `record(prev_prev, prev, word)` — call after every commit. None
  for prev/prev_prev at sentence start or after `.`/`!`/`?`.
- `score(prev_prev, prev, candidate) -> f64` — returns a bonus in
  `[0.0, 0.5]` to subtract from the recognizer's score. Trigram
  preferred; falls back to bigram with 0.75× weight; 0.0 if no
  data.
- Persists to `~/.local/share/rusty-board/ngram.toml`, same atomic-
  write pattern as the adaptive dictionary.
- Capped at 5000 bigrams / 10000 trigrams; same 30-day decay.
- 11 tests including the realistic "how are you" → boost "you" over
  "your" scenario.

**No pre-trained corpus**: the model starts empty per user and
grows from their actual typing. ~50KB after a few weeks of use,
not the 100MB a Brown-corpus-pretrained trigram would be. Tied
in well with the privacy-preserving design.

**Integration point** (not yet wired): in `keyboard_view.rs`, the
swipe-commit handler needs to:
1. Take the last 2 committed words (from engine state OR a separate
   recent-commits buffer).
2. Pass them to `ngram.record(prev_prev, prev, &committed_word)`
   after the commit fires.
3. Have the recognizer take an `&NgramModel` reference and call
   `score()` on each candidate, blending into the final ranking.

**Integration point** (not yet wired):

```rust
// In keyboard_view.rs, after a successful swipe commit:
let dict = self.adaptive_dictionary.clone();  // Rc, cheap
dict.record_commit(&word);
// Later, every ~5 sec or on focus change:
let _ = dict.flush_if_dirty();  // best-effort save

// In recognizer.rs, when computing candidate score:
let user_boost = dict.boost(&candidate.word);
candidate.score -= user_boost;  // lower-is-better
```

The recognizer doesn't currently take an AdaptiveDictionary — that's
the next step. The interface is designed to be a clean addition
that defaults to no-op (empty dictionary → all boosts are 0).

## What's sketched but not built yet

### `ProgressiveRecognizer`

Re-runs the recognizer on every drag_update tick (currently it
only runs at drag_end). Maintains a small ring buffer of recent
candidate sets so we can show a "stable" ribbon that doesn't
flicker every 16ms.

```rust
pub struct ProgressiveRecognizer {
    recognizer: Arc<GestureRecognizer>,
    history: VecDeque<Vec<WordCandidate>>,
    last_path_len: usize,
}

impl ProgressiveRecognizer {
    /// Called from drag_update. Returns Some(candidates) if the
    /// path has changed enough since last call to be worth
    /// recomputing; None if we should reuse the previous result.
    pub fn update(
        &mut self,
        path: &[SwipePoint],
        hitboxes: &[KeyHitbox],
    ) -> Option<Vec<WordCandidate>> {
        // Only recompute every ~3 path points, not every one.
        if path.len() < self.last_path_len + 3 {
            return None;
        }
        self.last_path_len = path.len();
        let cands = self.recognizer.recognize(path, hitboxes);
        self.history.push_back(cands.clone());
        if self.history.len() > 5 {
            self.history.pop_front();
        }
        Some(self.smooth_with_history(cands))
    }

    /// Take consensus across recent recognitions. A candidate that
    /// keeps appearing as the user extends the path is high-confidence.
    fn smooth_with_history(&self, current: Vec<WordCandidate>) -> Vec<WordCandidate> {
        // Re-rank: each candidate's score is adjusted by how
        // often it appeared in recent history. Anti-flicker.
        // ... O(N²) but N ≤ 5 ≤ 25 comparisons. Fast.
    }
}
```

**UI implication**: the keyboard view can render a small ribbon
above the keys showing the top 1-3 current predictions. User can
tap one to commit early (short-circuit swipe).

### `HitboxNudger` — the "subtle gesture manipulation" piece

For each candidate word currently being tracked, predict which key
the user is most likely to pass through NEXT. Slightly widen those
keys' hitboxes (by ~10-15%, not enough to be visually noticeable)
so the user's path is more likely to register through the right
key when it strays slightly.

```rust
pub struct HitboxNudger {
    base_hitboxes: Vec<KeyHitbox>,
}

impl HitboxNudger {
    /// Given current candidates and current path, produce a
    /// nudged hitbox map. The next-likely keys per candidate
    /// each get a small radius bonus, weighted by candidate
    /// confidence.
    pub fn nudged(
        &self,
        candidates: &[WordCandidate],
        path_chars_so_far: &str,
    ) -> Vec<KeyHitbox> {
        let mut bonus: HashMap<char, f64> = HashMap::new();
        for c in candidates.iter().take(5) {
            // Next char in this candidate, if any.
            let next = c.word.chars().nth(path_chars_so_far.chars().count());
            if let Some(ch) = next {
                // Confidence-weighted bonus: top candidate's next
                // char gets a bigger nudge than #5's.
                let weight = 1.0 - (c.score / 5.0).min(0.8);
                *bonus.entry(ch).or_insert(0.0) += weight;
            }
        }
        // Normalize so total bonus across keys is bounded.
        // Then return hitboxes with adjusted radii.
        self.base_hitboxes
            .iter()
            .map(|hb| {
                let label = hb.label.chars().next().unwrap_or('\0').to_ascii_lowercase();
                let nudge = bonus.get(&label).copied().unwrap_or(0.0);
                let scale = 1.0 + (nudge * 0.15).min(0.15);  // cap at 15%
                hb.scaled(scale)
            })
            .collect()
    }
}
```

**Caveat**: this is the most invasive change. It requires:
- The swipe handler runs the progressive recognizer
- `decode_swipe_path` takes nudged hitboxes instead of static ones
- Hitboxes are recomputed every ~3 path points (cost: ~50 KeyHitbox
  clones per swipe — measurable but not crippling)

Risk: nudging changes the path-decoding output, which could create
feedback loops (nudged hitboxes → different decoded raw → different
candidates → different nudged hitboxes). Needs care to keep stable.

## Where the LLM fits

The `llm_swipe.rs` re-ranker from last session is now the FOURTH
component, not the primary signal. Order of operations:

1. Shape recognizer produces top-5 candidates from the raw decoded path
2. Adaptive dictionary boosts user-recent candidates
3. (Future) n-gram LM rescores based on previous word(s) in surrounding text
4. (Future) LLM re-ranker breaks ties when top candidates are still close

Each stage is cheap on its own; together they cost <10ms even on phone.
The LLM only fires in the rare deeply-ambiguous case.

## Why this is better than one big model

- **Composable**: each piece can be toggled on/off independently. User can
  opt out of LLM but keep adaptive dictionary, or vice versa.
- **Debuggable**: each component's contribution to the final ranking is
  inspectable. With a single LLM, you get one opaque "this is the answer."
- **Latency-bounded**: each piece has a known, small budget. No surprise
  300ms inferences.
- **Offline-friendly**: only the LLM piece needs the heavy `llama-cpp-2`
  dependency. The adaptive dictionary + progressive recognizer + nudger
  ship with zero new dependencies.
- **Privacy-preserving**: adaptive dictionary stays local; nothing leaves
  the device.

## "Realtime dictionary" interpretation

That phrase from your prompt I'm reading two ways, both buildable:

1. **Adaptive: dictionary itself grows from user's commits** ✅ landed
2. **Live: candidates refresh DURING the swipe, not just at end** ⏳ sketched
   as `ProgressiveRecognizer`

Both are practical without a big model.

## "Subtle gesture manipulation" interpretation

Three concrete things this could mean, ranked by impact:

1. **HitboxNudger** — adjust hitbox radii toward predicted-next keys.
   Highest impact: better accuracy on imprecise swipes. ⏳ sketched.
2. **Live candidate ribbon** — UI feedback, lets users commit mid-swipe.
   ⏳ sketched as part of `ProgressiveRecognizer` integration.
3. **Velocity-modulated confidence** — slow movement = "user is being
   precise"; fast movement = "trust the LM more." Already partially
   in `recognize_with_speed`; could go deeper. Already shipping.

## What to build next

If you want to keep the foothold-style progression:

1. **Wire AdaptiveDictionary** into the recognizer + the swipe-commit
   path. ~50 lines, no UI changes. Adds the boost signal everywhere.
2. **Settings panel toggle** + "your top words" diagnostic. ~30 lines.
3. **ProgressiveRecognizer** prototype, wired to log candidates per
   path-tick without UI yet. Measure: how often do the candidates
   stabilize before drag_end? If they stabilize early, the ribbon
   has real value.
4. **Candidate ribbon** UI based on (3).
5. **HitboxNudger** — gated behind a feature flag; benchmark accuracy
   on a small dataset of swipe paths before enabling by default.

Each of these is its own 1-2 turn project. Adaptive dictionary already
provides measurable value before any of the others land.
