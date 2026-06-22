# Vision

web2qml is a voice-first, native, low-distraction environment for using
the terminal and the web together. The long-term goal is that you can
sit at the system and steer it primarily by speaking — with the
terminal and browser as peers, not separate apps — and that the system
learns your voice well enough to feel like a familiar tool, not a
generic assistant.

## Core beliefs

1. **Voice is the right input modality for many tasks** once recognition
   is reliable. People can speak faster than they type, and the
   cognitive cost of speaking a known phrase is lower than typing it
   accurately on a small keyboard or a phone.

2. **Local-first, no cloud round-trip.** Whisper runs on-device.
   Phonetic fingerprints, decisions, conversation history are all on
   disk and never transmitted. The user owns the data.

3. **Recognition should adapt to the user.** Generic models do generic
   things; our phonetic fingerprint DB, confusion table, token stats,
   and audio profiles all learn the user's voice and corrections. A
   command voiced once and confirmed is matched faster and more
   reliably the next time.

4. **The terminal is part of the system, not a foreign tool.** The
   browser and terminal share a single application, share context
   (URL → man page lookup, terminal output → AI summary), and share
   the voice intent pipeline. They are tabs in the same surface.

5. **Doors should be open.** Settings should be discoverable, default
   behaviors should be reasonable, and power users should find the
   knobs they need (env vars, hand-editable config, plain-text TOML).

6. **Small, dense, fast.** Native Qt6/QML + Rust. No Electron. No
   embedded browser engines beyond what we need. Aarch64 is a
   first-class target — if it doesn't run well on a phone-class CPU,
   it's wrong.

## Where we are now (v0.1.637)

Voice + recognition:
- Voice intent parser with broad natural-language coverage (English),
  Spanish and French per-language parsers.
- Phonetic fingerprint DB (`~/.config/web2qml/voice_fingerprints.bin`)
  with multi-style support — same phrase captured across whispered,
  energetic, noisy, fragmented variants and matched per-style.
- Two-tier voice preview: phonetic dumb-DB match every 250ms +
  real whisper-streamed transcription every 1500ms during recording.
- Wake-gate decision (whisper-vs-skip) extracted to a pure tested
  module so the expensive-step gating is tunable (v0.1.632).
- shell-dialect detection (bash-family / nu / fish / python) as the
  foundation for per-shell NLP routing (v0.1.636).

Multi-speaker / collaboration:
- Speaker registry with whitelist (`~/.config/web2qml/speakers.toml`),
  per-speaker auto-color from voice, live multicolor transcript.
- New-speaker enroll detection — a brand-new voice offers the owner an
  "add them?" prompt (owner confirms first), v0.1.624.
- Per-command audit attribution (who asked) + a hash-chained audit log;
  pure tier-gate decision (Owner/Trusted/Guest/Observer) for a future
  privileged-action shim — attribution, never OS impersonation (v0.1.633).

Terminal / thinking surface:
- Smart terminal blocks with AI summary, missing-package detection
  (bash/zsh/fish AND nushell), smart-cd undo.
- Per-row terminal rendering with wide-char support, ANSI colors,
  region selection, copy menu with chevron expansion.
- Render path measured + optimized: RichText re-parse de-duped on
  unchanged HTML and burst-coalesced on load (v0.1.628-630).
- Workflow recorder; cards/graph knowledge view (pages/files/chats/
  workflows as voice-navigable nodes).
- zellij voice/assisted control foundation — drive zellij via its
  `zellij action` CLI, with structured `list-panes` reads for an
  assisted layer keys can't replicate (v0.1.634).

See docs/power-user-brainstorm.md for the active power-user roadmap
candidates and docs/power-features.md for the standing gap audit.

## Where we're going

The big themes ahead:

### Multi-user collaboration
The architectural pivot of late 2025/early 2026. Multiple people share
one screen, one voice channel, one terminal session. The system knows
who said what, color-codes the transcript, lets people hand off control
with a touch. Expert "shows" by driving; learner "tells" by typing or
speaking; system supports both modes without forcing a choice.

See [multi-user.md](multi-user.md) for the current state and
[pipeline.md](pipeline.md) for the audio flow.

### Real-time speaker separation
We classify whole utterances by speaker today. The next step is
per-chunk classification (~500ms windows) so two people talking in
overlapping voice activity get routed to separate transcript streams.
This also gives single-user mode "noise rejection" — chunks that
don't match the registered profile get dropped.

### Editor integration
Helix LSP integration so the system understands what file you're
editing, what symbol your cursor is on, what diagnostics exist. Voice
commands like "fix the warning on line 47" or "rename this symbol to
Foo" become natural.

### Language coverage
English, Spanish, French today. German, Japanese, Mandarin, Italian,
Portuguese on the roadmap. Each new language is ~200-400 lines of
keyword mapping and politeness phrase handling.

### Codemap
Per-project symbol/file structure cached locally so voice can
navigate. "Open the file where authentication is handled" should
resolve without a fuzzy search.

### Browser ↔ terminal context bridge
"That URL I was just on, look it up with `man` if it's a system
topic." "The error in the terminal, search for it on Stack Overflow."
Cross-tab actions that feel natural.

## What we explicitly don't do

- **Cloud-only features.** Anything that requires uploading user data
  to a remote service is opt-in and isolated. Web search yes, send-your-
  audio-to-anyone-else no.
- **Telemetry, analytics, "improvement" data collection.** None of it.
  No data leaves the device unless the user explicitly initiates an
  action that involves the network.
- **Forced AI features.** AI summarization is opt-in (with off-toggle).
  The system works without ollama installed; AI augments but isn't
  required.
- **Mandatory accounts, mandatory updates, mandatory anything.**
  Single binary, runs from a directory, configuration in plain-text
  files the user can edit and version-control.

## Trade-offs we accept

- **Recognition is imperfect.** Whisper on CPU at Tiny/Base tier is
  ~90% accurate on clear speech, less in noise. We mitigate with
  phonetic fingerprinting, confusion correction, and visible confidence
  signals — but we don't pretend it's perfect.
- **First-call latency is noticeable.** Ollama cold-start, whisper
  state init, etc. We pay these costs once per session; subsequent
  calls are warm and fast.
- **The codebase is dense.** Single repo, lots of modules, complex
  interaction between voice/terminal/browser. We use detailed module
  comments and this docs folder to make it navigable rather than
  aiming for a clean facade that hides complexity.

## Anti-goals

- "AI agent" framing — we're a voice-driven UI, not an autonomous
  agent. The system does what the user voices; it doesn't take
  independent action.
- Wide cross-platform parity at the cost of native quality — we
  prioritize Linux/aarch64 first.
- Marketing-driven feature checklist — features ship when they
  actually work, not to fill a comparison table.

## Exploratory direction: structured documents + 2D shape rendering

(Added after the CAD/block-document discussion.) A possible expansion:
the niftyplot graph format (already in the tree) gains a 2D shape
RENDERER, so a diagram can also describe a flat physical part — a panel
cutout, a vented phone back cover — in real millimetres, exported as
SVG. The voice pipeline then becomes a way to MODEL ("set plate width to
160", "add a hex pattern"), not just to navigate.

This is deliberately scoped SMALL and SEPARATED: a basic SVG renderer
behind a swappable trait (`ShapeRenderer`), with a heavier renderer
(GPU/3D) only ever slotting in behind that same trait later, IF earned.
It does not displace the voice-first terminal+web core — it's an
additional surface that reuses the existing parser (niftyplot), layout,
and voice-intent machinery. Full plan: docs/design/shape_renderer.md.
Explicitly NOT committing to a geometry kernel, 3D, or plugins.

