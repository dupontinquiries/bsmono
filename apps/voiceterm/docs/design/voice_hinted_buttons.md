# Voice-hinted on-screen buttons

Status: **DESIGN ONLY (filed post-v0.1.519).** F9: on-screen buttons
(accept/cancel, browse-tab controls) should take voice hints — when
the live voice capture matches a button's context, the button ANIMATES
to show it's the recognized target.

## The idea (user)

Buttons like Accept / Cancel that are on screen carry a voice CONTEXT.
When the voice pipeline detects a phrase matching that context, the
button animates (pulse/glow) to signal "I heard you mean this." Covers
confirm/deny dialogs (incl. F7's new-tab prompt) and the browse-tab
buttons.

## Why this is a good fit here

The app already has voice intent parsing (voice_parse_intent, the
grammar labels, editor/repl verb tables) and a visible-button layer.
This is the bridge: a button registers a small set of phrases it
responds to; the voice layer, when it matches one, signals that
button to animate AND (optionally) to fire.

## Design

### Button voice-context registry (pure, testable)
A registry mapping a button id → its voice context (a set of
phrases/intents + a confidence threshold). Pure matching function:
  match_button(transcript_or_intent, &registry) -> Option<(button_id,
  confidence)>
Reuse the existing intent/verb matching (editor_voice / repl_voice
already match phrases → actions with confidence; the confusion_table /
phonetic layer already does fuzzy phrase matching). So this is mostly
a thin mapping over existing matchers, returning WHICH on-screen
button matched. Testable: transcript + registry → expected button.

### The animation signal (live surface)
On a match, the backend emits a signal (button_id, confidence); QML
binds each button's "hinted" state to it and runs a Behavior/
animation (pulse). Reuses the native QML animation approach (the
cinematic-camera lesson: animation in QML's scene graph, logic in
Rust). No per-frame JS.

### Fire vs hint (policy)
Two modes per button: HINT-ONLY (animate, user still taps) vs
AUTO-FIRE (high confidence → act, like saying "yes"). Confirm/deny
dialogs likely want auto-fire on a clear "yes"/"no"; ambiguous
contexts hint-only. The threshold gate is pure + testable.

## What's pure/testable vs live
- TESTABLE: the registry, match_button(), the fire-vs-hint threshold
  policy. Build on editor_voice/repl_voice/phonetic matchers.
- LIVE: the animation, the signal wiring, the feel of pulse timing —
  needs the running build.

## Reuse / alignment
- editor_voice (15/15) + repl_voice (17/17) already match phrases →
  actions with confidence + margin guards — match_button is the same
  pattern, different output target.
- The confusion_table / phonetic fuzzy matching handles mishearings.
- Native QML Behavior animations (cinematic-camera principle) for the
  pulse — no JS.
- First consumer: F7's new-tab confirm/deny dialog. Second: the
  browse-tab buttons.

## Open questions
- Where do button voice-contexts get DEFINED — per-button in QML (a
  `voiceHints: ["yes","open it","confirm"]` property), or a central
  Rust registry keyed by a screen/context id? Central is more testable
  and lets the same matcher serve all buttons; QML just declares the
  id.
- Auto-fire safety: a misheard "yes" auto-confirming a destructive
  action is risky — auto-fire probably ONLY for non-destructive /
  explicitly-opted dialogs; everything else hint-only. Confirm the
  policy with the user.
