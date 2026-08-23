//! diagfeed.rs — deciding which engine diagnostics are worth a person's attention.
//!
//! The engine emits 95 distinct `eprintln!` lines from backend.rs alone, and NONE of them reach the
//! UI. Everything the operator needs to answer "why is this slow" or "why did that model refuse" —
//! the roofline fraction, the recurrent-layer census, the sliced-weights percentage, the template
//! mismatch alarm, the prefix-tree hit rate — exists only on stderr. Which is why debugging this
//! engine currently means running it from a terminal and pasting the output somewhere.
//!
//! The UI half of that is rendering, which is not testable here. The half that decides WHAT to show
//! is pure text, so it lives here with the awkward cases pinned.
//!
//! Two things make a raw stderr tail useless as a panel, and both are handled:
//!
//!   * VOLUME. A single load emits twenty near-identical "blk.N ships a FUSED attn_qkv" lines, each
//!     several hundred characters. Shown verbatim they bury the one line that mattered.
//!   * FLATNESS. "adapter = Apple M2 Ultra" and "load failed: 30 of 40 blocks carry ssm_*" arrive
//!     with equal weight. A feed that does not rank is a log with extra steps.

/// What a line is FOR, which is what decides whether it interrupts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Provenance and configuration: adapter, build, rope, tokenizer. Read once, never urgent.
    Context,
    /// A measurement worth acting on: roofline, phase balance, dispatch profile.
    Performance,
    /// Something is wrong but the run continues: template mismatch, marker leak, device error.
    Warning,
    /// The run stopped: load failed, refusals, panics.
    Failure,
}

impl Kind {
    /// Does this deserve to pull the eye on its own?
    pub fn interrupts(self) -> bool { matches!(self, Kind::Warning | Kind::Failure) }
}

/// Classify one engine line.
///
/// Ordered most-severe first: a line saying "load failed: ... device error" is a failure even though
/// it also contains a warning word, and checking warnings first would demote it.
pub fn classify(line: &str) -> Kind {
    let l = line.to_ascii_lowercase();
    if l.contains("load failed") || l.contains("refusing")
        || l.contains("panicked") || l.contains("is not supported")
        || l.contains("not implemented") {
        return Kind::Failure;
    }
    // DEGRADATION, which the first version of this missed almost entirely. Checked against all 92
    // emit sites in backend.rs, 77 of which were landing in Context — including three that describe
    // the engine quietly doing less than asked:
    //
    //   * "context 13412 tokens > 3840 window — kept 1024 head" — nine thousand tokens of the
    //     operator's paste dropped, rendered as dimmed provenance.
    //   * "diffuse_generate refused (...) — falling back to the AR window, which cannot produce
    //     coherent output" — missed by one character, because the keyword was "refusing".
    //   * "windowed decode hit the device buffer ceiling — windowing disabled" — a capability lost
    //     mid-run.
    //
    // "skipped" is deliberately NOT here. The engine skips things for good reasons and says so
    // ("int8 cache SKIPPED — converting would double the bytes without adding precision"), and
    // flagging those would make the badge cry wolf on every load. What earns a warning is being
    // FORCED to do less: refused, disabled, a ceiling hit, a fallback, a truncation.
    if l.contains("mismatch") || l.contains("marker leak") || l.contains("device error")
        || l.contains("unverified") || l.contains("carry ssm_") || l.contains("oom")
        || l.contains("refused") || l.contains("disabled") || l.contains("falling back")
        || l.contains("buffer ceiling") || l.contains("cannot produce")
        || (l.contains("window —") && l.contains("kept")) {
        return Kind::Warning;
    }
    if l.contains("roofline") || l.contains("phase balance") || l.contains("bottleneck")
        || l.contains("tok/s") || l.contains("profile —") || l.contains("submits")
        || l.contains("load path") || l.contains("cache —") {
        return Kind::Performance;
    }
    Kind::Context
}

/// A line reduced to what makes it distinct, so repeats can be folded.
///
/// Per-block chatter differs only in its index, so `blk.0 ships a FUSED attn_qkv` and `blk.19 ...`
/// collapse to one entry with a count. Digits are the only thing normalised — folding on anything
/// looser would merge lines that genuinely differ, and a feed that hides a real second problem
/// because it resembles the first is worse than a noisy one.
pub fn dedup_key(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_num = false;
    for c in line.chars() {
        if c.is_ascii_digit() {
            if !in_num { out.push('#'); in_num = true; }
        } else {
            in_num = false;
            out.push(c);
        }
    }
    out
}

/// One entry in the feed.
#[derive(Debug, Clone)]
pub struct Entry { pub kind: Kind, pub text: String, pub count: usize }

/// A bounded, folded, ranked view of the engine's output.
pub struct Feed { entries: Vec<Entry>, cap: usize }

impl Feed {
    pub fn new(cap: usize) -> Self { Feed { entries: Vec::new(), cap: cap.max(1) } }

    /// Add a line, folding it into an existing entry when it differs only in numbers.
    pub fn push(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() { return; }
        let key = dedup_key(line);
        if let Some(e) = self.entries.iter_mut().find(|e| dedup_key(&e.text) == key) {
            e.count += 1;
            // Keep the LATEST text: for a repeated measurement the newest numbers are the live ones,
            // and showing the first forever would make a changing value look frozen.
            e.text = line.to_string();
            return;
        }
        self.entries.push(Entry { kind: classify(line), text: line.to_string(), count: 1 });
        // Evict the least interesting, not the oldest: dropping a failure to make room for another
        // adapter banner is the one thing a bounded feed must not do.
        while self.entries.len() > self.cap {
            let victim = self.entries.iter().enumerate()
                .min_by_key(|(i, e)| (e.kind, std::cmp::Reverse(*i)))
                .map(|(i, _)| i);
            match victim { Some(i) => { self.entries.remove(i); } None => break }
        }
    }

    /// Most severe first, and within a severity the most recent first.
    pub fn ranked(&self) -> Vec<&Entry> {
        let mut v: Vec<(usize, &Entry)> = self.entries.iter().enumerate().collect();
        v.sort_by_key(|(i, e)| (std::cmp::Reverse(e.kind), std::cmp::Reverse(*i)));
        v.into_iter().map(|(_, e)| e).collect()
    }

    /// How many entries would interrupt — the number a badge shows.
    pub fn attention_count(&self) -> usize {
        self.entries.iter().filter(|e| e.kind.interrupts()).count()
    }

    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn len(&self) -> usize { self.entries.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real lines from the operator's logs, not invented ones.
    const ADAPTER: &str = "wgpu-engine: adapter = Apple M2 Ultra (G14D B1) [Vulkan] | engine build 923f8b51";
    const ROOFLINE: &str = "wgpu-engine: [m.gguf] roofline — 25 GB/s of 800 spec = 3% of the decode ceiling; NOT bandwidth-bound";
    const CENSUS: &str = "load failed: 30 of 40 blocks carry ssm_* tensors — this is a HYBRID RECURRENT model";
    const TEMPLATE: &str = "wgpu-engine: [d.gguf] TEMPLATE MISMATCH — the prompt carries marker TEXT that this tokenizer does not know";
    const DEVERR: &str = "wgpu-engine: device error (non-fatal, logged): Validation Error";
    fn blk(i: usize) -> String {
        format!("wgpu-engine: blk.{i} ships a FUSED attn_qkv (3072 rows, as expected) — using it directly.")
    }

    #[test]
    fn severity_is_checked_most_severe_first() {
        assert_eq!(classify(CENSUS), Kind::Failure, "a load failure outranks the warning words in it");
        assert_eq!(classify(TEMPLATE), Kind::Warning);
        assert_eq!(classify(DEVERR), Kind::Warning);
        assert_eq!(classify(ROOFLINE), Kind::Performance);
        assert_eq!(classify(ADAPTER), Kind::Context);
        assert!(Kind::Failure.interrupts() && Kind::Warning.interrupts());
        assert!(!Kind::Performance.interrupts() && !Kind::Context.interrupts());
    }

    /// The twenty near-identical per-block lines are what make a raw tail unusable.
    #[test]
    fn per_block_chatter_folds_into_one_entry() {
        let mut f = Feed::new(20);
        for i in 0..20 { f.push(&blk(i)); }
        assert_eq!(f.len(), 1, "twenty blk lines are one fact");
        assert_eq!(f.ranked()[0].count, 20, "and the count says how many");
        assert!(f.ranked()[0].text.contains("blk.19"), "showing the latest, not the first");
    }

    /// Folding must not merge genuinely different lines. Two DIFFERENT warnings that happen to share
    /// a shape would be the worst possible thing to hide.
    #[test]
    fn different_lines_do_not_fold_together() {
        let mut f = Feed::new(20);
        f.push(TEMPLATE);
        f.push(DEVERR);
        f.push(ROOFLINE);
        assert_eq!(f.len(), 3);
    }

    /// A bounded feed must evict the boring, not the old. Dropping a failure to make room for an
    /// adapter banner is the one behaviour that would make this worse than the log.
    #[test]
    fn eviction_drops_context_before_failures() {
        // Ten DISTINCT context lines, not ten numbered variants — the first version of this test
        // used "shape n={i} k=2048", which all fold to one entry, so eviction never ran and the
        // test asserted a path it did not reach. Folding working correctly made the eviction test
        // vacuous.
        let mut f = Feed::new(3);
        f.push(CENSUS);                       // Failure, first in
        for w in ["arena ON packing resident weights", "prewarmed pipelines at load",
                  "submit batch tuned for this GPU", "KV prefix tree ON pool cap",
                  "int8 cache skipped for this source", "advice about the GPU budget constant",
                  "tile overrides available", "fast-decode levers cached verdict",
                  "resident memory total anonymous", "released page cache from mapped files"] {
            f.push(&format!("wgpu-engine: {w}"));
        }
        let kinds: Vec<Kind> = f.ranked().iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&Kind::Failure), "the failure survived ten context lines: {kinds:?}");
        assert_eq!(f.len(), 3, "and the cap held");
    }

    #[test]
    fn ranking_puts_failures_first_and_context_last() {
        let mut f = Feed::new(20);
        f.push(ADAPTER); f.push(ROOFLINE); f.push(TEMPLATE); f.push(CENSUS);
        let order: Vec<Kind> = f.ranked().iter().map(|e| e.kind).collect();
        assert_eq!(order, vec![Kind::Failure, Kind::Warning, Kind::Performance, Kind::Context]);
    }

    /// The badge counts what should interrupt — not everything, or it always reads alarming.
    #[test]
    fn the_badge_counts_only_what_interrupts() {
        let mut f = Feed::new(20);
        f.push(ADAPTER); f.push(ROOFLINE);
        assert_eq!(f.attention_count(), 0, "provenance and measurements are not alarms");
        f.push(TEMPLATE);
        assert_eq!(f.attention_count(), 1);
        f.push(CENSUS);
        assert_eq!(f.attention_count(), 2);
    }

    /// A repeated measurement should show its CURRENT value, or a live number looks stuck.
    #[test]
    fn a_repeated_measurement_shows_its_latest_value() {
        let mut f = Feed::new(10);
        f.push("wgpu-engine: [m.gguf] produced 71 tokens in 4.4s = 16.31 tok/s");
        f.push("wgpu-engine: [m.gguf] produced 538 tokens in 33.1s = 16.24 tok/s");
        assert_eq!(f.len(), 1);
        assert!(f.ranked()[0].text.contains("16.24"), "the newest reading wins");
    }

    #[test]
    fn blank_lines_are_ignored() {
        let mut f = Feed::new(5);
        f.push("   "); f.push("");
        assert!(f.is_empty());
    }
}

// ---------------------------------------------------------------- the live sink

/// The process-wide feed the UI reads.
///
/// A sink rather than stderr interception. Capturing fd 2 with dup2 would pick up every line
/// including panics and wgpu's own logging, which sounds like a feature until a validation error
/// arrives mid-frame and the reader is the render thread. Writing to both places at the point of
/// emission keeps the terminal behaviour identical — the operator's existing workflow of piping to
/// a file still works, byte for byte — and gives the UI a structured copy.
static FEED: std::sync::OnceLock<std::sync::Mutex<Feed>> = std::sync::OnceLock::new();

fn feed() -> &'static std::sync::Mutex<Feed> {
    FEED.get_or_init(|| std::sync::Mutex::new(Feed::new(64)))
}

/// Record a diagnostic line. Never panics and never blocks on a poisoned lock: a diagnostics
/// channel that can take down the thread it observes is worse than no channel.
pub fn record(line: &str) {
    if let Ok(mut f) = feed().lock() { f.push(line); }
}

/// Snapshot for rendering: ranked, owned, and cheap to hold across a frame.
pub fn snapshot() -> Vec<Entry> {
    feed().lock().map(|f| f.ranked().into_iter().cloned().collect()).unwrap_or_default()
}

/// How many entries want attention — what a badge shows without opening the panel.
pub fn attention() -> usize {
    feed().lock().map(|f| f.attention_count()).unwrap_or(0)
}

pub fn clear() {
    if let Ok(mut f) = feed().lock() { *f = Feed::new(64); DISMISSED.store(0, O); }
}

/// How many entries had been seen when the banner was last dismissed.
///
/// Dismissing a banner must not DELETE the diagnostics — the button sits next to one that opens the
/// page showing them, and wiping the evidence on the way to reading it is the worst possible
/// pairing. It suppresses the banner until something new arrives instead.
/// Serialises the tests that drive the process-global [`FEED`].
///
/// `clear()` / `record()` / `banner()` all operate on one static, so any two
/// tests using them are the same test running twice with interleaved steps.
/// Reported from a fleet build (2026-08-23): `a_failure_outranks_a_later_warning`
/// unwrapped `None`, and `the_banner_shows_what_the_panel_ranks_first` asserted
/// against a line another test had recorded —
///
///     left:  Some("load failed: 30 of 40 blocks carry ssm_* tensors")
///     right: Some("load failed: something structural")
///
/// — which is one test's fixture appearing in another's assertion, the
/// signature of a shared mutable global rather than of a logic error.
///
/// It passed everywhere it had been run before because those machines had ONE
/// core, so libtest's default thread pool was one thread and the tests were
/// serial by accident. It is a real defect that a 1-CPU sandbox structurally
/// cannot surface, which is worth remembering the next time a suite is green
/// here and not there.
///
/// A lock rather than `--test-threads=1`: the flag would serialise the whole
/// crate's 981 tests to fix ten of them, and a future test that touches the
/// feed would silently rejoin the race.
///
/// `.unwrap_or_else(|e| e.into_inner())` because a panicking test poisons the
/// mutex, and the next test should report its own failure rather than a
/// poisoning error that hides it.
#[cfg(test)]
fn feed_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

static DISMISSED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
use std::sync::atomic::Ordering::Relaxed as O;

/// Suppress the banner until a new entry appears. The Engine page keeps everything.
pub fn dismiss_banner() {
    if let Ok(f) = feed().lock() { DISMISSED.store(f.entries.len(), O); }
}

/// `eprintln!` that also reaches the UI.
///
/// Deliberately a drop-in: the format arguments are evaluated once, written to stderr exactly as
/// before, and the same string is recorded. Migrating a call site is a one-word edit, which is what
/// makes it plausible to move all 95 rather than the handful someone judged important today.
#[macro_export]
macro_rules! diag {
    ($($arg:tt)*) => {{
        let s = format!($($arg)*);
        eprintln!("{}", s);
        $crate::diagfeed::record(&s);
    }};
}

#[cfg(test)]
mod sink {
    use super::*;

    /// The sink is global, so these run in one test — separate ones would race on it and the
    /// flakiness would be blamed on the feed rather than on the test design (c4-816's PID bug was
    /// exactly that mistake).
    #[test]
    fn the_sink_records_ranks_and_counts() {
        let _feed = crate::diagfeed::feed_guard();
        clear();
        assert_eq!(attention(), 0);
        record("wgpu-engine: adapter = Apple M2 Ultra [Vulkan]");
        record("wgpu-engine: [m.gguf] roofline — 25 GB/s of 800 spec = 3% of the decode ceiling");
        assert_eq!(attention(), 0, "provenance and measurements do not interrupt");

        record("wgpu-engine: [d.gguf] TEMPLATE MISMATCH — marker TEXT this tokenizer does not know");
        record("load failed: 30 of 40 blocks carry ssm_* tensors — HYBRID RECURRENT model");
        assert_eq!(attention(), 2, "a warning and a failure do");

        let snap = snapshot();
        assert_eq!(snap[0].kind, Kind::Failure, "most severe first: {:?}", snap[0].kind);
        assert_eq!(snap.last().unwrap().kind, Kind::Context);

        // Folding still applies through the sink, not just the bare Feed.
        for i in 0..20 { record(&format!("wgpu-engine: blk.{i} ships a FUSED attn_qkv (3072 rows)")); }
        let folded = snapshot();
        let blks: Vec<&Entry> = folded.iter().filter(|e| e.text.contains("FUSED attn_qkv")).collect();
        assert_eq!(blks.len(), 1, "twenty lines, one entry");
        assert_eq!(blks[0].count, 20);
        clear();
    }
}

// ---------------------------------------------------------------- display helpers

/// Label for the tab strip, carrying the count only when there is something to act on.
///
/// A badge that always shows a number trains people to ignore it. Provenance and measurements are
/// not alarms — the engine emits dozens of both every load — so the count reflects
/// `attention_count`, and a quiet engine gets a bare label.
pub fn tab_label(attention: usize) -> String {
    match attention {
        0 => "⚡ Engine".to_string(),
        n => format!("⚡ Engine ({n})"),
    }
}

/// One line, shortened for a 330px panel without losing which line it is.
///
/// Engine diagnostics are long by design — they explain themselves, and several run past 400
/// characters. Truncating the TAIL keeps the identifying half: every line starts with
/// "wgpu-engine: [model]" or "loader:", and the distinguishing words come early. The full text stays
/// available on hover.
pub fn shorten(text: &str, max: usize) -> String {
    let t = text.strip_prefix("wgpu-engine: ").or_else(|| text.strip_prefix("loader: ")).unwrap_or(text);
    if t.chars().count() <= max { return t.to_string(); }
    let cut: String = t.chars().take(max.saturating_sub(1)).collect();
    // Break at a word boundary when one is near, so the visible half stays readable.
    match cut.rfind(' ') {
        Some(i) if i > max * 2 / 3 => format!("{}…", &cut[..i]),
        _ => format!("{cut}…"),
    }
}

/// Count suffix for a folded entry. Silent at one, because "(x1)" on every line is noise.
pub fn count_suffix(count: usize) -> String {
    if count > 1 { format!("  ×{count}") } else { String::new() }
}

#[cfg(test)]
mod display {
    use super::*;

    #[test]
    fn the_badge_is_bare_when_nothing_needs_attention() {
        assert_eq!(tab_label(0), "⚡ Engine");
        assert_eq!(tab_label(3), "⚡ Engine (3)");
    }

    /// The prefix every line shares carries no information in a panel headed "Engine".
    #[test]
    fn the_shared_prefix_is_dropped() {
        assert_eq!(shorten("wgpu-engine: adapter = M2", 40), "adapter = M2");
        assert_eq!(shorten("loader: NEOX pairing", 40), "NEOX pairing");
        assert_eq!(shorten("something else", 40), "something else");
    }

    /// Truncating the tail keeps the half that says WHICH line this is.
    #[test]
    fn shortening_keeps_the_identifying_start() {
        let long = "wgpu-engine: [m.gguf] roofline — 25 GB/s of 800 spec = 3% of the decode ceiling; \
NOT bandwidth-bound — the hardware is mostly idle and the limit is dispatch overhead";
        let s = shorten(long, 50);
        assert!(s.starts_with("[m.gguf] roofline"), "the identity survives: {s}");
        assert!(s.chars().count() <= 50, "and it fits: {} chars", s.chars().count());
        assert!(s.ends_with('…'), "and says it was cut: {s}");
    }

    /// Multi-byte characters must not be sliced mid-character — these lines are full of — and ×.
    #[test]
    fn multibyte_text_is_not_split_mid_character() {
        for max in 3..40 {
            let s = shorten("wgpu-engine: roofline — 25 GB/s — NOT bandwidth-bound ✓", max);
            assert!(s.chars().count() <= max, "max={max}: {} chars", s.chars().count());
        }
    }

    #[test]
    fn a_single_occurrence_shows_no_multiplier() {
        assert_eq!(count_suffix(1), "");
        assert_eq!(count_suffix(20), "  ×20");
    }
}

#[cfg(test)]
mod against_real_emit_sites {
    use super::*;

    /// Lines taken verbatim from backend.rs emit sites, with the format holes filled in as they
    /// would appear at runtime. Hand-picked examples let the first version of this classifier put
    /// 77 of 92 real lines into Context; a corpus is what caught that.
    #[test]
    fn degradation_is_not_filed_as_provenance() {
        let must_warn = [
            "wgpu-engine: context 13412 tokens > 3840 window — kept 1024 head + 2800 tail, 9588 elided",
            "wgpu-engine: [m.gguf] diffuse_generate refused (short readout) — falling back to the AR \
window, which cannot produce coherent output for a diffusion model",
            "wgpu-engine: [m.gguf] windowed decode hit the device buffer ceiling — windowing disabled",
            "wgpu-engine: KV reuse DISABLED (LOGSCOPE_KV_OFF) — full prefill every turn",
        ];
        for l in must_warn {
            assert_eq!(classify(l), Kind::Warning, "should warn: {}", &l[..60.min(l.len())]);
        }
    }

    /// And the ones that must NOT warn, which is the harder half. The engine skips work for good
    /// reasons constantly; flagging those would make the badge cry wolf on every load and train the
    /// operator to ignore it — the exact failure the badge design was meant to avoid.
    #[test]
    fn deliberate_choices_stay_quiet() {
        let must_stay_calm = [
            "wgpu-engine: int8 cache SKIPPED — source is ggml type 12, which already carries per-32 \
scales. Converting would double the bytes without adding precision",
            "wgpu-engine: fast-decode levers ON (LOGSCOPE_FAST_DECODE=1) — A/B skipped",
            "wgpu-engine: released page cache from 1 previously-loaded model file(s)",
            "wgpu-engine: adapter = Apple M2 Ultra (G14D B1) [Vulkan] | engine build bd53807d",
            "wgpu-engine: prewarmed 14 pipelines (prefill+decode kernels compiled at load)",
        ];
        for l in must_stay_calm {
            assert!(!classify(l).interrupts(), "should not interrupt: {}", &l[..60.min(l.len())]);
        }
    }

    /// A failure still outranks the degradation words it contains.
    #[test]
    fn failures_still_win_over_their_own_warning_words() {
        let l = "worker: load failed: blk.0.attn_qkv has 8192 rows but this model needs 5120";
        assert_eq!(classify(l), Kind::Failure);
        let l2 = "load failed: 30 of 40 blocks carry ssm_* tensors — LOGSCOPE_TRY_UNSUPPORTED_ARCH cannot help";
        assert_eq!(classify(l2), Kind::Failure, "contains 'cannot' and 'unverified'-adjacent words");
    }
}

// ---------------------------------------------------------------- the one-line banner

/// The single thing the operator most needs to know right now, or nothing.
///
/// The Engine page is somewhere you go; a banner is something you see. Silent data loss is the case
/// that needs the second kind: a 13412-token paste truncated to 3840 answers a question about code
/// the model never received, and the reply looks entirely confident. That belongs in the chat, not
/// three clicks away.
///
/// Exactly ONE line, chosen by severity then recency, and None when nothing interrupts. A banner
/// that is usually present is furniture — people stop reading it, which costs more than it saves.
pub fn banner() -> Option<String> {
    // Picks the winner UNDER THE LOCK and formats one string. The first version called snapshot(),
    // which clones every entry and sorts them, then threw all but one away — 64 String clones and a
    // sort per frame, 3840 allocations a second at 60fps, to render a single line that is usually
    // absent. An egui panel body runs every frame; anything called from one has to be priced that
    // way, and I did not price it.
    let f = feed().lock().ok()?;
    // Nothing new since the operator dismissed: stay quiet. Comparing COUNTS rather than storing a
    // flag means a genuinely new notice re-raises the banner on its own, which a boolean would not.
    if f.entries.len() <= DISMISSED.load(O) { return None; }
    let best = f.entries.iter().enumerate()
        .filter(|(_, e)| e.kind.interrupts())
        // Same ordering as `ranked`: severity first, then most recent. Duplicating the comparison is
        // the risk here, so a test asserts the two agree on which entry wins.
        .max_by_key(|(i, e)| (e.kind, *i))?;
    Some(summarise(&best.1.text))
}

/// Compress one diagnostic to banner length, keeping the number that makes it matter.
///
/// Generic openers are dropped; the engine's lines lead with a model name and a phase, and the
/// operator already knows which model they loaded. What survives is what changed and by how much.
pub fn summarise(text: &str) -> String {
    let t = text.trim();
    // Strip the emitter and any [model.gguf] tag: the banner sits in that model's own chat.
    let t = t.strip_prefix("wgpu-engine: ").or_else(|| t.strip_prefix("worker: "))
        .or_else(|| t.strip_prefix("loader: ")).unwrap_or(t);
    let t = match (t.find('['), t.find("] ")) {
        (Some(0), Some(j)) => &t[j + 2..],
        _ => t,
    };
    // Prefer the clause before the first em-dash: these lines put the finding first and the
    // explanation after, and the explanation is what the Engine page is for.
    let head = t.split(" — ").next().unwrap_or(t).trim();
    let head = if head.len() < 12 { t } else { head };
    shorten(head, 96)
}

#[cfg(test)]
mod banner_tests {
    use super::*;


    /// Every test that touches the global feed takes `feed_guard()`.
    ///
    /// The lock only works if it is taken. A new test calling `record()`
    /// without it rejoins the race silently — and, on a 1-CPU machine, passes
    /// while doing so. That is exactly how the two failures this guard exists
    /// for reached a fleet build in the first place.
    ///
    /// Structural, because the alternative is trusting nine call sites to stay
    /// correct and a tenth to be written correctly.
    #[test]
    fn every_test_touching_the_global_feed_takes_the_guard() {
        let src = include_str!("diagfeed.rs");
        // Split into test functions, crudely but sufficiently: each `#[test]`
        // starts one and the next `#[test]` (or EOF) ends it.
        let mut offenders = Vec::new();
        let parts: Vec<&str> = src.split("#[test]").skip(1).collect();
        for body in parts {
            let name = body
                .split("fn ").nth(1).unwrap_or("")
                .split('(').next().unwrap_or("").trim().to_string();
            if name == "every_test_touching_the_global_feed_takes_the_guard" { continue }
            let touches = body.contains("clear()")
                || body.contains("record(")
                || body.contains("banner()")
                || body.contains("dismiss_banner()");
            if touches && !body.contains("feed_guard()") {
                offenders.push(name);
            }
        }
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "these tests drive the process-global FEED without taking feed_guard(), so they race each other under parallel libtest: {offenders:?}. Add `let _feed = crate::diagfeed::feed_guard();` as the first line. A 1-CPU machine will not catch this for you."
        );
    }

    #[test]
    fn nothing_worth_saying_shows_nothing() {
        let _feed = crate::diagfeed::feed_guard();
        clear();
        record("wgpu-engine: adapter = Apple M2 Ultra [Vulkan]");
        record("wgpu-engine: [m.gguf] roofline — 25 GB/s of 800 spec = 3% of the decode ceiling");
        assert!(banner().is_none(), "provenance and measurements are not banners");
        clear();
    }

    /// The case this exists for: the paste that lost its middle.
    #[test]
    fn a_truncated_paste_says_so_with_the_number() {
        let _feed = crate::diagfeed::feed_guard();
        clear();
        record("wgpu-engine: context 13412 tokens > 3840 window — kept 1024 head + 2800 tail, \
9588 elided from the middle and MARKED as such (raise n_ctx for more)");
        let b = banner().expect("truncation must reach the banner");
        assert!(b.contains("13412") && b.contains("3840"), "keep the numbers that matter: {b}");
        assert!(!b.contains("wgpu-engine"), "drop the emitter: {b}");
        assert!(b.chars().count() <= 96, "fits a strip: {} chars", b.chars().count());
        clear();
    }

    /// A failure outranks a warning even when the warning arrived later — severity first, then
    /// recency, matching the panel's own ranking so the two never disagree.
    #[test]
    fn a_failure_outranks_a_later_warning() {
        let _feed = crate::diagfeed::feed_guard();
        clear();
        record("worker: load failed: 30 of 40 blocks carry ssm_* tensors — HYBRID RECURRENT model");
        record("wgpu-engine: [m.gguf] TEMPLATE MISMATCH — the prompt carries marker TEXT");
        let b = banner().unwrap();
        assert!(b.contains("load failed"), "the failure wins: {b}");
        clear();
    }

    /// The model tag is noise in that model's own chat window.
    #[test]
    fn the_model_tag_is_dropped() {
        assert_eq!(summarise("wgpu-engine: [Llama-3.2-1B.gguf] TEMPLATE MISMATCH — the prompt carries \
marker TEXT that this tokenizer does not know"), "TEMPLATE MISMATCH");
    }

    /// A line with no em-dash must not be reduced to nothing.
    #[test]
    fn a_short_line_survives_intact() {
        let _feed = crate::diagfeed::feed_guard();
        let s = summarise("worker: load failed: missing feed_forward_length");
        assert!(s.contains("missing feed_forward_length"), "{s}");
    }
}

#[cfg(test)]
mod banner_agrees_with_the_panel {
    use super::*;

    /// banner() picks its winner independently of ranked() for cost reasons, so the two orderings
    /// are duplicated logic — which is a bug waiting to happen. This asserts they agree: whatever
    /// the panel shows FIRST among interrupting entries is what the banner summarises.
    #[test]
    fn the_banner_shows_what_the_panel_ranks_first() {
        let _feed = crate::diagfeed::feed_guard();
        for order in [
            vec!["wgpu-engine: adapter = M2",
                 "wgpu-engine: [a] TEMPLATE MISMATCH — marker text",
                 "worker: load failed: something structural",
                 "wgpu-engine: [b] device error (non-fatal, logged)"],
            vec!["worker: load failed: first",
                 "wgpu-engine: [a] TEMPLATE MISMATCH — later warning"],
            vec!["wgpu-engine: [a] device error (non-fatal)",
                 "wgpu-engine: [b] TEMPLATE MISMATCH — most recent warning"],
        ] {
            clear();
            for l in &order { record(l); }
            let panel_first = snapshot().into_iter().find(|e| e.kind.interrupts())
                .map(|e| summarise(&e.text));
            assert_eq!(banner(), panel_first, "banner and panel disagree for {order:?}");
        }
        clear();
    }

    /// And nothing interrupting still means no banner, via the cheap path too.
    #[test]
    fn the_cheap_path_stays_silent_when_it_should() {
        let _feed = crate::diagfeed::feed_guard();
        clear();
        record("wgpu-engine: adapter = M2 Ultra");
        record("wgpu-engine: [m] roofline — 3% of the decode ceiling");
        assert!(banner().is_none());
        clear();
    }
}

#[cfg(test)]
mod dismissal {
    use super::*;

    /// The bug: the dismiss button called clear(), destroying the very lines the neighbouring
    /// "details" button opens a page to show.
    #[test]
    fn dismissing_hides_the_banner_and_keeps_the_evidence() {
        let _feed = crate::diagfeed::feed_guard();
        clear();
        record("worker: load failed: 30 of 40 blocks carry ssm_* tensors");
        assert!(banner().is_some());
        let before = snapshot().len();

        dismiss_banner();
        assert!(banner().is_none(), "the banner goes");
        assert_eq!(snapshot().len(), before, "and the Engine page keeps everything");

        // Something NEW must raise it again — a dismissal is for what was read, not for the future.
        record("wgpu-engine: [m] TEMPLATE MISMATCH — marker text");
        assert!(banner().is_some(), "a new notice re-raises the banner");
        clear();
    }

    /// A repeat of an already-dismissed line folds into its entry and must NOT re-raise: the count
    /// changes but the population does not, and re-alarming on a repeat is how a banner becomes
    /// furniture.
    #[test]
    fn a_repeat_of_a_dismissed_line_stays_quiet() {
        let _feed = crate::diagfeed::feed_guard();
        clear();
        record("wgpu-engine: [m] device error (non-fatal, logged): Validation Error");
        dismiss_banner();
        for _ in 0..5 { record("wgpu-engine: [m] device error (non-fatal, logged): Validation Error"); }
        assert!(banner().is_none(), "the same problem repeating is not a new problem");
        clear();
    }
}
