# Voice editing + the unification principle (from the Pulsar review)

## The lineage: kss → reVos → web2qml (the spine)

The user's prior tools reveal a single 20-year instinct — **compress
the dead time out of a process so the human stays on the content,
not the semantics.**

- **kss** (Kitchen Silence Splitter): cleave dead air out of lecture
  video. ffmpeg/moviepy/pydub silence detection → shorter watch time.
- **reVos** ("re-vision videos", FOSS, no central server, any
  device): SS Protocol (speed-curve the quiet parts), Lecture
  Protocol (skip silence), uh-ware (strip filler words/sounds),
  Heartbeat (level out tempo). Stated value: "keep users focused on
  the CONTENT of the video and not the semantics." The user's own
  lecture note records "saved ~31 minutes using reVos."
- **web2qml**: the SAME move applied to CODING instead of WATCHING.
  Cleave the dead air out of the dev loop.

**This reframes every architectural demand the user made.** AI speed
over SOTA, fast local models, knowledge webs, cross-references,
shortcuts, saved sessions, collab, certs, multi-user — NOT a
grab-bag. All one thing: **remove the friction between intent and
result so a developer can work on the move, on a phone, sustainably.**
reVos removed friction from CONSUMING knowledge; web2qml removes it
from PRODUCING code.

## The mobility thesis (the user's words)

"Speed of getting things done while on the move on my phone is the
goal. Devs don't need a workstation in 2026, they need a smart
developer strategy and access to their files dynamically. Voice
editing on the go, and workflows and saved sessions for social and
traveling coders."

Everything below serves THIS: a phone-first, voice-first, low-latency
dev loop. Anything that only makes sense at a workstation is the
wrong direction.

## Surface rule (the user's tested preference, refined)

NOT "everything is a unified popup." The right surface depends on
whether the task is a MOMENT or a WORKSPACE:
- **Voice commands → ONE unified popup / one grammar.** (A moment.)
- **Find-and-replace → overlay / sidebar.** The user TESTED this: a
  spatial, scrub-against-it task wants a persistent surface, not a
  one-shot command. This is not a unification failure; it's matching
  surface to task-shape.
- **Prezi-style walkthrough** as connective tissue is worth
  exploring — it's literally reVos's "move the view to the user,
  keep focus on content" applied to navigating code.

The anti-pattern to avoid is the Pulsar config-nightmare: a dozen
features each with its own SETTINGS/keymaps. Multiple SURFACES are
fine (popup vs sidebar vs walkthrough) as long as they share one
config model and appear by CONTEXT, not by configuration.

---

Original Pulsar-review notes follow.

Design input captured 2026-05-24 from the user's `pulsar-stuff.zip`
(their prior Pulsar/Atom package ecosystem) and their framing:
**"web2qml's purpose is to prep one to become a plumber of their own
code."**

## What the Pulsar packages reveal about the intent

The user's own packages (the `impakt-*` family + voicecode +
teletype-diy) all pull the same direction:

- **impakt-inline-charts** — "small docs inside the code to improve
  comprehension." Docs live WHERE the code is.
- **impakt-cam** — cursor-follow focus; the view comes to you.
- **impakt-inline-images / inline-markdown** — rich content rendered
  inline, never a side panel.
- **impakt-quick-menu** — one fuzzy entry point to everything,
  including "hidden commands."
- **voicecode** — control the editor by voice (speech to code).
- **teletype-diy** — self-hosted realtime collaboration.

Their lecture notes (e.g. L09.md, a hand-built MIPS datapath lesson)
show the learning style: inline `flowchart TD` diagrams, `[[wikilinks]]`,
instruction-encoding tables woven through prose, and even
instrumented note-taking ("*saved ~31 minutes using reVos*"). They
learn by building the explanation in the same surface as the thing.

## The core principle (and the trap to avoid)

The Pulsar setup compounded into "a configuration nightmare" — a
dozen independent packages, each with its own settings, keymaps, and
failure modes. **web2qml is that same philosophy collapsed into ONE
coherent surface with ONE config model.** The terminal IS the
inline-doc surface; the voice layer IS voicecode; the model picker
IS the one quick-menu.

**Implication for how we add features:** the recent additions (REPL
voice, install card, model playground, recommendation) each risk
becoming their own surface with their own trigger — which is exactly
the compounding-config trap web2qml exists to escape. The fix is
UNIFICATION: features should be context-detected and routed through
ONE grammar / one entry point, not bolted on as separate modes the
user must learn and configure.

## Next build: voice editing as a CONTEXT, not a new surface

The user named the specific gap: "I have struggled to break out
something like active debugging or file editing via voice." REPL
voice (v0.1.477-482) handled debugging. File editing is the twin.

The pattern is already proven and unified-by-design:
`repl_voice::detect_repl(last_line)` + `translate(repl, phrase)`,
intercepted in `voice_parse_intent` before normal routing. Editor
voice should reuse the SAME shape:

- **Detection already exists:** `is_editor_active()` (returns true
  for hx/helix/vim/nvim/nano/micro/emacs) and
  `term_foreground_command()` (returns "helix"/"vim"). No new
  detection surface needed — reuse these.
- **New piece (pure, testable):** an `editor_voice` module mirroring
  `repl_voice` — a (editor, verb-table) translation. Helix first
  (the user lives in it; see helix-mode-plus in their Pulsar set).
  Examples for helix:
    - "save" → `:w<ret>`,  "save and quit" → `:wq<ret>`
    - "go to line 42" → `42gg` (or `:42<ret>`)
    - "next word" → `w`,  "delete line" → `dd`,  "undo" → `u`
    - "find foo" → `/foo<ret>`,  "open file X" → `:open X<ret>`
  vim variant follows. SAME safety rules as repl_voice: strict
  detection, symbol/path sanitization, never auto-issue destructive
  multi-step sequences without a clear verb.
- **Wiring:** extend `voice_parse_intent` — if `is_editor_active()`
  and `editor_voice::translate` maps the phrase, return a
  TerminalCommand carrying the keystrokes. ONE interception point,
  same as REPL. The user never configures a "mode"; it just works
  when an editor is focused.

### Why this serves the principle

It adds a capability (voice editing) WITHOUT adding a surface: no new
panel, no new settings, no new trigger to learn. Detection is
automatic (editor focused → editing grammar live), exactly like the
REPL loop. That's the anti-Pulsar move — one pipe-wrench that does
the right thing in context, not a twelfth package.

### Open question for the user (asked, not yet answered)

Whether to ALSO unify the existing separate surfaces (install card,
playground) into the one-grammar / context-only model, or leave them
and just stop adding new separate ones. Either way, editor-voice
should be built unified from the start.

## Note on the dogfooding gap

The user debugs/edits by voice rarely because the loop isn't
fast/reliable enough to trust mid-flow (their own diagnosis). Two
levers, both already in motion:
  1. Reachable triggers (REPL wiring v0.1.482; editor-voice next).
  2. Low per-utterance latency — a fast LOCAL model (the curated 3B +
     the setup pill v0.1.481), so each "step"/"save" is near-instant
     instead of a 5s Ollama round-trip.
Both are needed: a reachable loop that's slow still won't get used.
