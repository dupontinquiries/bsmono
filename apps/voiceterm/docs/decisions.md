# Decisions

Architectural decisions in compact ADR-style: title, context,
decision, alternatives, trade-offs. Newest first.

## ADR-12: Per-utterance speaker identification before per-chunk

**v0.1.117.** Speaker identification currently runs after full
utterance transcription, not on per-chunk audio.

**Context.** True per-chunk routing requires Separator hysteresis,
per-speaker audio buffers, independent VAD per buffer, and parallel
whisper jobs. That's a lot of moving parts.

**Decision.** Land the per-utterance identification first; build
Separator as a library; wire it incrementally. UI multicolor view
works against per-utterance attribution for now.

**Alternatives considered.**
- Full per-chunk pipeline in one turn. Too much risk; high chance of
  breaking working preview path.
- Skip speaker UI until per-chunk is ready. Loses visible feedback
  for users who are happy with whole-utterance attribution.

**Trade-offs accepted.** Two speakers in one utterance get
mis-attributed (averaged profile). Less accurate but visibly working.

---

## ADR-11: Speaker registry uses TonalProfile, not MFCC embeddings

**v0.1.116.** Speaker identification uses the existing TonalProfile
(8 floats) rather than MFCC-based speaker embeddings (~256 floats).

**Context.** TonalProfile is already computed for every utterance for
pitch-hint matching. Repurposing it for speaker ID is essentially free.

**Decision.** Start with TonalProfile. Threshold 0.65.

**Alternatives considered.**
- ECAPA-TDNN style embeddings. Higher quality, requires bundling a
  separate model file (~50MB) and inference call per utterance.
- pyannote-style end-to-end diarization. Heavy dependency, Python
  ecosystem.

**Trade-offs accepted.** Lower discrimination — speakers with similar
pitch confused. Upgrade path: swap profile type in `speakers::Speaker`
struct; matcher logic unchanged.

---

## ADR-10: Workflow recorder uses hand-rolled TOML

**v0.1.114.** Workflow persistence (`~/.config/web2qml/workflows/<name>.toml`)
hand-parsed instead of using the `toml` crate.

**Context.** Format is simple (name, created_at, commands array of
strings). Adding `toml` crate as dependency adds compile time and
binary size for one use.

**Decision.** Hand-rolled reader/writer. ~30 lines.

**Trade-offs accepted.** Doesn't handle TOML edge cases (multi-line
strings, comments inside arrays, etc). Users can hand-edit only
within the format we emit.

---

## ADR-9: Multi-style fingerprints with style_id u8

**v0.1.109.** Each fingerprint carries a style classification
(0=unclassified, 1=normal, 2=whispered, 3=energetic, 4=noisy,
5=fragmented).

**Context.** Same person voices differently across contexts. A single
"normal speech" fingerprint matches poorly when the user whispers.

**Decision.** Auto-classify each fingerprint at record time using
rule-based thresholds on TonalProfile. Disk format FP04 appends 1
byte per fingerprint. MAX_PER_LABEL raised 3 → 6; per-style cap of 2.
Matcher gives 5% score reduction (better) for same-style matches.

**Alternatives considered.**
- Don't classify; let the matcher figure it out from MFCC. Misses
  the chance to surface "you whispered this before, here it is".
- ML-based style classifier. Too heavy.

**Trade-offs accepted.** Rule-based thresholds are brittle. Most
utterances classify as 1 (normal); only outliers get tagged. The 5%
bonus is small to avoid degrading match quality on misclassified
utterances.

---

## ADR-8: Per-row terminal rendering instead of TextEdit RichText

**v0.1.98.** Switched from a single TextEdit with RichText document
to per-row Repeater of Text elements.

**Context.** TextEdit RichText had unpredictable line layout; cursor
positioning was approximate; wide chars, ANSI colors, and TUI cursor
moves caused drift.

**Decision.** Each row is its own Text element, height locked to
`charH`. Cursor at row N positions at `y = N * charH` with no
measurement surprises.

**Trade-offs accepted.** More QML elements per frame; slightly higher
GPU cost. But predictable layout is worth it — eliminated an entire
class of alignment bugs.

---

## ADR-7: HTML whitespace preservation via `<span style='white-space:pre'>`

**v0.1.113.** Every span in per-row HTML output (including default-
styled runs) wraps in `<span style='white-space:pre'>`.

**Context.** When `colorMode=true`, per-row uses RichText. Qt's HTML
parser collapses consecutive spaces by default, destroying column-
aligned tree output (dust, fastfetch, etc).

**Decision.** Force `white-space:pre` on every span. Resolves the
dust/fastfetch alignment regression.

**Alternatives considered.**
- Use `&nbsp;` for spaces. Doubles HTML output size.
- Default to PlainText mode. Loses ANSI colors.

**Trade-offs accepted.** Trailing whitespace cells now technically
render, but they're clipped by per-row width anyway.

---

## ADR-6: ollama summarizer 30s timeout, 45s total budget

**v0.1.115.** Models with "summariz" in name get 30s timeout
individually; total budget across all attempts capped at 45s.

**Context.** Purpose-built 400M summarizer models (QyrouNnet/summarizer)
were timing out at 12s repeatedly, falling through to uninstalled
fallback models.

**Decision.** Adaptive timeout. Total budget prevents 2 × 30s = 60s
worst case.

**Trade-offs accepted.** Worst-case wait up to ~60s if summarizer is
truly stuck. Acceptable because the worker is background and the user
sees a "summary pending" indicator.

---

## ADR-5: Confusion table persistence opt-in, not default

**v0.0.x.** `WEB2QML_PERSIST_LEARNING=1` required to save voice
learning data to disk.

**Context.** Voice fingerprints and learned corrections are personal
data. Some users want the system to learn within a session but not
persist.

**Decision.** Default to session-only; opt-in to persistence.

**Trade-offs accepted.** Users who never set the env var don't get
the benefit of cross-session learning. Documented in setup-voice.sh
and the security log at startup.

---

## ADR-4: Two-tier streaming voice preview

**v0.1.108 + v0.1.116.** Preview thread runs phonetic match every
250ms AND real whisper streaming every 1500ms.

**Context.** Phonetic gives <10ms feedback but only for known
utterances. Whisper gives real text but takes 200-400ms even at Tiny.

**Decision.** Run both. Phonetic fills the gap before whisper lands;
whisper supersedes when available.

**Trade-offs accepted.** Memory cost (~210MB extra for dedicated
PREVIEW_STATE). Worth it for the dual feedback.

---

## ADR-3: Single binary, no services

**Project inception.** No daemon, no helper processes, no separate
voice service.

**Context.** Voice apps often spawn a "voice daemon" for always-on
listening. Adds complexity, IPC, lifecycle issues.

**Decision.** Everything in one process. Audio capture in a thread.
Wake-word detection in another thread. Whisper inference on-demand
or in pool workers.

**Trade-offs accepted.** Memory usage stays loaded while the app is
open. If the user wants 0-memory in idle, they close the app.

---

## ADR-2: Qt6/QML over Electron/Web

**Project inception.** Native Qt6 + QML for the UI; Rust backend.

**Context.** Electron-based alternatives have well-documented memory
and performance overhead. We target aarch64/phone-class hardware.

**Decision.** Native QML. Rust backend via qmetaobject-rs bridge.

**Trade-offs accepted.** Smaller community than Electron. Fewer
pre-built widgets. We write things by hand more often.

---

## ADR-1: Local-first, no cloud

**Project inception.** All voice/learning data stays local. Web
search via the user's choice of search engine; no opaque backends.

**Context.** Voice data is sensitive. Building a cloud architecture
that respects user privacy is hard and undermines trust.

**Decision.** No cloud at all. Models download once on setup; user
owns everything in `~/.config/web2qml/`.

**Trade-offs accepted.** Less context (no cross-device sync). The
user is responsible for backups.
