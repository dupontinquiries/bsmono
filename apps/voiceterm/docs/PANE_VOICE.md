# PANE_VOICE — gated multi-pane voice (phases 1 + 2)

Status as of v0.1.845. **Gated, off by default** (`WEB2QML_PANE_VOICE` unset →
zero behavior change). Turn on with `WEB2QML_PANE_VOICE=1`.

## The idea
Panes are already addressable channels (`pane_ptys`, keyed by pane id) and we
already have a routing key (`speakers::current_active_id()`). PANE_VOICE wires
those together so a spoken command can be aimed at a chosen pane, or fanned out
to a set of panes ("one voice → several ssh sessions", i.e. tmux
synchronize-panes but voice-driven).

## What's implemented and verified (backend, all `cargo check` + ship-gate clean)
Nine `qt_method`s on the backend (declared near `open_note_at`, implemented near
`pty_write`):

- `pane_voice_enabled() -> bool` — reads the gate.
- `pane_pty_write(pane_id, text) -> bool` — phase-1 primitive: write to a
  specific pane's pty with the same `\n`/`\r\n` → `\r` canonicalization as
  `pty_write` (extracted into the shared, unit-tested `canonical_pty_bytes`).
  Trailing `\n` submits. False if the id doesn't resolve.
- `pane_context(pane_id) -> "id\tfg\tremote\tpassword\tpager\talive"` — phase-1
  context read straight off the pane's own `PtyTerminal` (so it works for
  background panes, unlike the singular `block_tracker`): foreground command,
  remote/ssh detection, password-prompt and pager state.
- `set_voice_target_pane(pane_id)` / `voice_target_pane_id() -> f64` — phase-1
  routing target (negative id clears → focused pane).
- `voice_broadcast_toggle(pane_id) -> bool` / `voice_broadcast_clear()` /
  `voice_broadcast_list() -> "id,id,…"` — phase-2 broadcast set.
- `pane_voice_dispatch(text) -> "mode\tdetail\tspeaker"` — the routing heart.
  Gate off → exactly `pty_write` (legacy). Gate on → broadcast to the set if
  non-empty, else the target pane, else the focused pane. Modes returned:
  `active` / `target` / `target_gone` / `broadcast`.

### Wiring
The voice-confirmed `term_cmd` path (`executeIntent`, qml/main.qml ~20706) now
calls `pane_voice_dispatch(cmd + "\r")` instead of `pty_write` directly. Because
the dispatcher falls back to `pty_write` when the gate is off, this is identical
to prior behavior unless `WEB2QML_PANE_VOICE=1`. The active-pane type-overlay is
suppressed when a command is routed elsewhere (so it doesn't animate the wrong
pane).

### Spoken routing commands (v0.1.849)
Routing no longer needs a console/JS hook — the dispatcher recognizes spoken
commands (pure parser in `src/pane_voice.rs`, applied only when the gate is on;
any unrecognized phrase falls through and is typed as a normal command):
- **"send this to pane 2"** / "target pane two" / "route to pane 3" → set the
  voice target pane (number words and the common mishears "to"→2, "for"→4 are
  handled).
- **"send here"** / "clear target" → route back to the focused pane.
- **"broadcast to all"** / "sync panes" → broadcast to every pane in the tab.
- **"broadcast to pane 2"** / "add pane 3" → add one pane to the broadcast set.
- **"stop broadcasting"** / "broadcast off" → clear the broadcast set.

A recognized command is applied (nothing is typed) and a toast confirms it
("Voice: target pane 2", "Voice: broadcast to all (3)", …).

## How to test (on hardware)
1. `WEB2QML_PANE_VOICE=1 ./web2qml`
2. Split into 2–3 panes (e.g. a couple of ssh sessions).
3. Aim a single pane: from a console/JS hook call
   `backend.set_voice_target_pane(<id>)` (ids come from `ws_pane_rects` /
   `pane_context`), then voice a command — it lands in that pane, not the focused
   one. `pane_context(<id>)` should report `remote=true` for an ssh pane.
4. Broadcast: `backend.voice_broadcast_toggle(<idA>)`,
   `…toggle(<idB>)`, then voice one command — it fans out to both ptys. Clear
   with `voice_broadcast_clear()`.
5. Gate off (unset the env) → everything behaves exactly as before.

## Per-speaker routing (v0.1.854 — multi-user)
Routing is now **per speaker**, not global. Each speaker (keyed by
`speakers::current_active_id()`) has their own target pane and broadcast set, so
two enrolled people can drive different panes at the same time: Alice says "send
to pane 1", Bob says "send to pane 2", and each one's later commands go to their
own pane. A solo or unidentified speaker uses the empty-string key — a single
entry — so nothing changes for single-user use.

The core is the pure, unit-tested `pane_voice::VoiceRouting` (per-speaker
`target` + `broadcast`, with `resolve()` deciding broadcast > target > focused).
`pane_voice_dispatch` resolves by the *current* speaker; the `set_voice_*` /
`voice_broadcast_*` methods operate on the current speaker; and a pane closing is
dropped from every speaker's routing (`forget_pane`). `voice_routing_status()`
returns one record per speaker (`speaker\u{1f}target\u{1f}b1,b2`, records split by
`\u{1e}`) for a multi-user routing panel.

## Per-mic routing (v0.1.856 — multi-mic → panes)
With one mic per person, you can bind each **mic source** to a pane and skip
diarization entirely — the hardware says who's talking. `MicBindings` (pure,
tested) maps a mic device name → pane. Methods:
- `audio_input_devices()` — newline-separated capture device names for a picker.
- `bind_mic_to_pane(mic, pane)` / `unbind_mic(mic)` — set/clear a binding
  (pane < 0 also unbinds).
- `mic_binding_status()` — `mic\u{1f}pane` records (split by `\u{1e}`) for a panel.
- `pane_voice_dispatch_from(mic_source, text)` — the routing entry point: a bound
  mic writes straight to its pane (spoken routing commands like "send to pane 5"
  still win first); an unbound mic, or a bound pane that has closed, falls through
  to the per-speaker dispatch. Gate off → plain behavior.

**Still needed (hardware):** multi-stream capture. Today one cpal stream fills the
ring buffer; routing-by-mic only does anything once the capture side opens one
stream per device and tags each utterance with its source, then calls
`pane_voice_dispatch_from(source, …)` instead of `pane_voice_dispatch(…)`. The
routing layer + enumeration are in place and tested; the per-device capture loop
needs real multi-mic hardware + the audio pipeline to validate, so it's the next
gated step.

## Not done yet (next steps)
- **QML UI** to pick the target / toggle broadcast panes (chips on the pane
  borders, or a marking-menu entry), and a routing panel fed by
  `voice_routing_status()`. The plumbing and `pane_context` are ready for it.
- **Phase 3 — true parallel attribution.** Per-speaker routing now exists, but it
  hinges on `current_active_id` tracking a single active speaker, not "who said
  *this* utterance." Simultaneous A-drives-1-while-B-drives-2 still needs
  per-utterance `speaker_embed` + parallel capture + diarization. The routing
  layer it lands on is in place; diarization is its own project.
- Per-pane cwd history (beyond the live `foreground_command`) if richer
  "context in the pane" framing is wanted.
