# Corpus-driven findings (v0.1.706)

Insights from running the real recorded corpus + its transcripts through the
actual code paths (not guesses).

## 1. The corpus is a dictation/WER set, not an intent set
Most transcripts are paragraphs read aloud for voice-profile registration
(pangrams, product copy, CAD specs, shell commands). That's the right shape
for WER + speaker-ID testing — NOT for intent-routing regression (the parse
regression layer uses tests/voice_corpus.txt for that). Good to know before
trying to assert intent accuracy from it.

## 2. Mojibake in reference transcripts inflated WER (FIXED)
5 transcripts contain "â" where a "→" arrow was double-encoded upstream
(bytes c3 a2 c2… = "→" UTF-8 re-interpreted as Latin-1 and re-encoded). "â" is
a Latin letter, so the WER normalizer counted it as a word — a guaranteed
phantom error vs whisper on every affected reference. Fix: normalize_words now
counts only ASCII alphanumerics (src/wer.rs + scripts/wer_score.rs), dropping
mojibake, ·↓ separators, and emoji. Upstream: the paragraph-scrape path that
writes the .txt sidecars mangles multi-byte arrows — worth a look in the
HTML-extraction step (not fixed here; the corpus was already captured).

## 3. Multiplexer voice only matched terse canonical forms (FIXED for tmux)
Testing natural phrasings a user would actually dictate — "open a vertical
pane", "split the window", "make a new pane", "give me a pane on the right",
"go to the next window" — ALL fell through to web search, because the matchers
were exact-string only. Fix: tmux_voice now normalizes fillers (a/the/me/my/
please/give/to…) and matches by intent words (contains), so all of those route
correctly while non-commands ("the quick brown fox") still return None. zellij_
voice got the filler-strip too (partial: its matchers are still mostly exact;
a few multi-word natural forms remain unmatched — candidate follow-up to give
zellij the same contains-based treatment as tmux).

## 4. The → mojibake mechanism, and a repair (v0.1.708)
Traced the corpus "â" to classic Latin-1↔UTF-8 double-encoding: "→" (e2 86 92)
was decoded as Latin-1 and re-encoded, becoming "â…". The Rust write path
(corpus_recorder) is clean UTF-8 — the corruption is upstream (web scrape / QML
text boundary). Rather than chase every source, added src/demojibake.rs: detect
double-encoding (all chars ≤0xFF, byte-reinterpretation yields valid non-ASCII
UTF-8 that differs) and repair it, applied at the paragraph_selected capture
boundary so future corpus references are clean. No-op on clean text and genuine
non-Latin scripts (日本語 passes through).

## 5. Main parser already handled prefixes; mid-phrase fillers were the gap
voice_intent already strips polite prefixes ("could you please", "I want to")
and learned fillers. Testing real phrasings found the remaining gaps were
MID-phrase: "open up the editor"→search, "read me this section"→freeform,
"take me to the very top"→search. Fixed with conservative word-boundary
reductions in normalize_natural_language (open up→open, read me→read, take me
to→go to, the very→the, pull up/bring up→open, show me→show). 36→41 phrase
intent corpus; no regressions.
