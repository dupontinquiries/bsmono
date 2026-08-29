//! aperture-complete — ghost-text completion as a small, visible machine.
//!
//! The design splits along what actually varies:
//! * **who completes** — [`Provider`], a blocking trait object. Three ship:
//!   [`providers::OllamaProvider`] (local models, FIM), [`providers::CommandProvider`]
//!   (any subprocess — your script, your API relay, your `llm` CLI), and
//!   [`providers::StaticProvider`] (deterministic, for tests and demos).
//! * **when to ask** — [`Debounce`], a pure decision (`now` is a parameter).
//! * **what to do with late answers** — [`engine::Engine`]'s generation gate:
//!   every buffer mutation bumps the generation; the worker coalesces queued
//!   requests to the newest and the UI drops any outcome whose generation is
//!   stale. No cancellation protocol, no async runtime — just a monotone
//!   counter and the observation that an obsolete completion is worthless.
//!
//! Every outcome carries its latency and provider name, and the engine keeps
//! nothing secret: the shell's "loop pane" renders exactly what came through
//! this API. Steerable beats magic.

pub mod edit;
pub mod engine;
/// Reuse of completions across caret motion and typing — the decision
/// layer that keeps identical requests from reaching the model at all.
/// Keeping the prompt's PREFIX stable so a KV-cached engine can skip
/// re-prefilling context that did not change — where the latency is.
pub mod kvplan;
pub mod prefixcache;
pub mod providers;
/// How much of the file a completion sees — several scopes, planned and
/// batched together rather than one global budget.
pub mod scope;
pub mod route_ai;

/// What the provider sees. `prefix`/`suffix` are already budget-trimmed
/// (see [`make_request`]); `lang` is a lowercase tag like "rust".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionRequest {
    pub generation: u64,
    pub prefix: String,
    pub suffix: String,
    pub lang: String,
    /// The file being edited, when it has one — providers that read
    /// sibling files or honor `@read` tool calls resolve relative to its
    /// directory; None disables both, structurally.
    pub path: Option<std::path::PathBuf>,
}

/// What comes back. `generation` echoes the request; the UI compares it to
/// the editor's current version and silently drops mismatches.
#[derive(Clone, Debug)]
pub struct CompletionOutcome {
    pub generation: u64,
    pub provider: String,
    pub latency_ms: u64,
    pub result: Result<String, String>,
    /// `false` for a streaming provider's in-progress deltas (ghost text
    /// should still show them — the text is real, just not the end of the
    /// answer yet); `true` for the one outcome that closes out a
    /// generation. History views (the loop pane) should record only the
    /// final one per generation, not every intermediate fragment.
    pub is_final: bool,
}

/// A completion backend. Blocking on purpose: it runs on the engine's worker
/// thread, and a blocking `fn` keeps providers writable as plain code (a
/// socket read, a `Command::output`) with no runtime in the dependency tree.
pub trait Provider: Send {
    fn name(&self) -> &str;
    fn complete(&mut self, req: &CompletionRequest) -> Result<String, String>;

    /// Streaming form: call `on_delta` as fragments arrive, checking
    /// `should_cancel` between them — the worker uses this as the
    /// cancellation path (drop the connection the moment a newer request
    /// supersedes this one), no async runtime required.
    ///
    /// Default: call `complete` once and return it, WITHOUT calling
    /// `on_delta` — a single-shot provider (`CommandProvider`,
    /// `StaticProvider`) has no progressive text to show, so this emits
    /// exactly the one outcome the worker sends after the call returns,
    /// matching pre-streaming behavior byte for byte. `should_cancel` is
    /// ignored: a subprocess or a lookup table has no meaningful mid-call
    /// cancellation point to check anyway.
    fn complete_streaming(
        &mut self,
        req: &CompletionRequest,
        on_delta: &mut dyn FnMut(&str),
        should_cancel: &mut dyn FnMut() -> bool,
    ) -> Result<String, String> {
        let _ = (on_delta, should_cancel);
        self.complete(req)
    }
}

/// Trim caret context to char-budgeted windows. Pure; boundary-safe.
pub fn make_request(
    context: &str,
    before: &str,
    after: &str,
    lang: &str,
    generation: u64,
    prefix_budget: usize,
    suffix_budget: usize,
) -> CompletionRequest {
    // Context RIDES ABOVE the tail-trim: a long file prefix must never evict
    // the outline (trimming after concatenation would cut exactly the part
    // that tells the model what the file contains). Context gets at most
    // half the budget; the live prefix keeps the rest.
    let ctx: String = if context.is_empty() {
        String::new()
    } else {
        context.chars().take(prefix_budget / 2).collect()
    };
    let prefix: String = format!("{ctx}{}", tail_chars(before, prefix_budget.saturating_sub(ctx.chars().count())));
    let suffix: String = after.chars().take(suffix_budget).collect();
    CompletionRequest { generation, prefix, suffix, lang: lang.to_string(), path: None }
}

fn tail_chars(s: &str, n: usize) -> String {
    let total = s.chars().count();
    s.chars().skip(total.saturating_sub(n)).collect()
}

/// The "when to ask" rule as a value: fire once the buffer has been idle for
/// `idle_ms` since the last edit. `now_ms` is always a parameter — the tests
/// drive this with fake time, and the shell drives it with egui's clock.
#[derive(Clone, Copy, Debug)]
pub struct Debounce {
    pub idle_ms: u64,
    last_edit_ms: Option<u64>,
}

impl Debounce {
    pub fn new(idle_ms: u64) -> Self {
        Debounce { idle_ms, last_edit_ms: None }
    }

    /// An edit happened; (re)arm the timer.
    pub fn note_edit(&mut self, now_ms: u64) {
        self.last_edit_ms = Some(now_ms);
    }

    /// Disarm without firing (caret moved away, ghost dismissed, …).
    pub fn cancel(&mut self) {
        self.last_edit_ms = None;
    }

    /// True exactly once per armed edit, after the idle window passes.
    pub fn fire(&mut self, now_ms: u64) -> bool {
        match self.last_edit_ms {
            Some(t) if now_ms.saturating_sub(t) >= self.idle_ms => {
                self.last_edit_ms = None;
                true
            }
            _ => false,
        }
    }

    pub fn armed(&self) -> bool {
        self.last_edit_ms.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_fires_once_after_idle_and_rearms_on_edit() {
        let mut d = Debounce::new(250);
        d.note_edit(1000);
        assert!(!d.fire(1100), "still typing");
        d.note_edit(1200); // keystroke resets the window
        assert!(!d.fire(1400));
        assert!(d.fire(1450));
        assert!(!d.fire(1500), "one-shot until re-armed");
    }

    #[test]
    fn make_request_trims_on_char_boundaries() {
        let before = "aé🙂xyz"; // multibyte on purpose
        let req = make_request("", before, "après", "rust", 7, 4, 3);
        assert_eq!(req.prefix, "🙂xyz");
        assert_eq!(req.suffix, "apr");
        assert_eq!(req.generation, 7);
    }
    #[test]
    fn context_rides_above_the_tail_trim() {
        // A long prefix would evict naive prepended context; ours can't be.
        let long_prefix: String = "x".repeat(500);
        let req = make_request("// ctx\n", &long_prefix, "", "rust", 1, 100, 10);
        assert!(req.prefix.starts_with("// ctx\n"), "context survives any prefix length");
        assert!(req.prefix.chars().count() <= 100, "total budget still holds");
        // And it never hogs more than half the budget.
        let big_ctx: String = "c".repeat(500);
        let req = make_request(&big_ctx, &long_prefix, "", "rust", 1, 100, 10);
        assert_eq!(req.prefix.chars().filter(|c| *c == 'c').count(), 50);
        assert_eq!(req.prefix.chars().filter(|c| *c == 'x').count(), 50);
    }

}
