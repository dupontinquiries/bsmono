# dialer — phone calls (egui, ModemManager)

The fleet's phone app: dialpad, in-call screen with DTMF, call recents.
egui-first per `UI_STRATEGY.md`; there is no Qt/GTK surface to archive because
this app was born after the strategy.

```
crates/dialer-core   headless: session model, Command/Event protocol,
                     mock + ModemManager backends, JSONL history
crates/dialer-egui   the surface on uikit-egui (glass keypad, Nav, Adaptive)
```

## Layering

The UI and the modem meet at one seam: a `std::mpsc` channel pair carrying
`Command` in and `Event` out (`dialer-core/src/link.rs`). Both backends —
`mock.rs` (simulated modem: the desktop/demo story) and `mm.rs`
(ModemManager over zbus blocking, feature `mm`) — are spawn functions
returning that pair, so the shell never knows which is running. All call
judgement (missed vs rejected vs no-answer, talk time = active→end) lives in
`session.rs`, pure and unit-tested; backends only report facts.

The mm loop **polls** (500 ms tick: `ListCalls` + `State` property diff)
instead of consuming MM signals — one readable function, no per-call signal
threads, latency invisible next to a cellular ring. Call proxies are built
with `CacheProperties::No`; without that, zbus's property cache is never
updated by a loop that doesn't pump `PropertiesChanged`, and every poll
reads the first value forever.

## Running

```
cargo run -p dialer-egui              # ModemManager if present, else mock (toast says so)
cargo run -p dialer-egui -- --mock    # force the simulated modem
cargo run -p dialer-egui -- --mm      # force ModemManager, exit 2 if absent
```

The mock scripts: outgoing calls connect after ~3 s (numbers ending in `0`
never answer, so NoAnswer is reachable); "simulate incoming" in the status
bar rings for 15 s. History lands in `$XDG_DATA_HOME/dialer/history.jsonl`.

## Setup wizard

First launch (or the status-bar `setup` button) opens a two-step wizard:

1. **Carrier / APN** — data from **mobile-broadband-provider-info** (the
   FOSS dataset NetworkManager itself uses): the distro XML at
   `/usr/share/mobile-broadband-provider-info/serviceproviders.xml` when
   present, a user-downloaded copy at `$XDG_DATA_HOME/dialer/` second
   (refresh = `curl -o` from the project's raw URL; the wizard prints the
   command), and a curated builtin snapshot (AT&T, T-Mobile, Verizon,
   Rogers, Bell, Telus, EU carriers) last. The modem's registered MCC+MNC
   (MM `OperatorCode`) preselects the matching carrier; fields stay
   editable. **Apply** writes a NetworkManager GSM connection over D-Bus
   (`Settings.AddConnection`, autoconnect) — the place APNs actually live
   on an MM system. The mock backend simulates the apply.

2. **Matrix** — homeserver discovery (`.well-known`) + password login on a
   worker thread; the session persists in `$XDG_DATA_HOME/dialer/matrix.json`
   and comes up on every launch as a second backend on the same Link
   protocol. Incoming `m.call.invite`s ring the normal in-call screen
   (caller = mxid) and **Decline** sends `m.call.hangup` — which also stops
   the ring on your other devices. **Signalling only in v1**: answering and
   placing Matrix calls need a WebRTC media engine that does not exist here
   yet, and the UI says so instead of pretending. matrix-rust-sdk was
   deliberately not used — four HTTP endpoints via `ureq` in the house
   visible-loop style, with every JSON parser a pure tested function.

## iOS-style interaction model

The gesture/interaction feel targets modern iOS, not Material. The *physics
and commit decisions* are headless in `dialer-core/src/gesture.rs` (pure,
tested); the shell only paints the offsets it returns.

- **Interactive edge-pop** — drag from the left edge of an open thread and the
  whole conversation tracks your finger 1:1, revealing "‹ Messages" behind it;
  release past half-width *or* a flick commits the back, otherwise it springs
  home (UIKit's interactive `popViewController`). A drag that starts mid-screen
  scrolls instead of popping.
- **Rubber-banding** — the UIScrollView elastic curve (`1 − 1/(x·0.55/d + 1)`)
  on overscroll and on a pop dragged the wrong way, rather than Android's
  stretch/glow.
- **Swipe-to-peek timestamps** — drag the conversation left to slide each
  bubble aside and reveal its time (iMessage), rubber-banding past the reveal.
- **Long-press Tapback + menu** — press-and-hold a bubble (or right-click on
  desktop) for the six-reaction bar (❤ 👍 👎 😂 ‼ ❓) and a Copy / Reply / Close
  menu. A reaction shows on the bubble corner; the same tapback again clears it,
  a different one replaces it — iMessage semantics.

Reactions persist without rewriting the append-only log: a Tapback re-appends
the message line, and `Conversations::open` keeps the last record per
`(peer, dir, ts)` key. (That key can collide only for two messages to the same
peer, same direction, in the same second — rare, and it only affects which one
a late reaction lands on.)

## Messaging & page nav

**Gestures:** swipe left/right pages across Keypad ↔ Recents ↔ Messages using
the shared `uikit_egui::GestureLayer` (the FP5-tuned `TwoTierGesture`), so the
feel matches the rest of the fleet. Swipe is disarmed inside an open thread so
it doesn't fight the message list's vertical scroll.

**SMS** over ModemManager's `Modem.Messaging`/`Sms` interfaces (feature `mm`):
send creates+sends an SMS object; inbound is polled (deliver PDU + received
state), delivered as an event, then deleted from the modem store. Threads are
per-peer, persisted as JSONL (`messages.jsonl`), unread-tracked.

**Linux handoff protocol** (`message.rs`) — a small structured payload carried
two ways: lossless JSON as a Matrix custom event
(`org.ecosystem.dialer.payload`, with an `m.room.message` text fallback so
plain clients see something), and a compact `run:` / `file:` / `view:` text
encoding for SMS that a non-app recipient can still read. Payloads:

- `Text`
- `CommandSuggestion { command, cwd?, note? }` — a *suggestion*, see below
- `FileOffer { name, mime?, size?, uri }` — a pointer, no bytes in the message
- `FileView { path }` — "open this local path", confined by the guard

The key safety property: a normal message is never misread as a command — only
the explicit `run:` prefix (or the JSON kind) upgrades it.

## Command execution: the threat model

Received command text is attacker-controlled. `command_guard.rs` is the single
gate:

- **Running is OFF by default** (`allow_run = false`); inbound commands are
  display + copy only. The opt-in lives in the setup wizard's Commands step
  with the warning attached.
- **Even enabled, every run is confirmed** in a modal showing the exact
  command, unless the sender is on an explicit allowlist AND the command
  matches an allowed prefix.
- **No shell.** An approved command is tokenised (quote-aware, control-char
  rejecting) and spawned as argv via `std::process::Command` — `;`, `|`, `$()`
  are literal argument bytes, inert. A newline in the payload is rejected
  outright so a second command can't be smuggled past the confirmation.
- **File-view is path-confined**: a `view:` path must canonicalise under a
  configured root or it's denied (`../` escapes and `/etc/shadow` go nowhere).

A corrupt policy file fails *closed* (running off). None of this trusts the
transport — E2EE tells you who sent a thing, not that it's safe to run.

## Real cellular MMS

Not implemented. Cellular MMS (multimedia over the carrier MMSC/WAP) needs
**mmsd-tng**; ModemManager doesn't carry MMS content. The handoff protocol
above is the richer-message story over Matrix/SMS-text instead. mmsd-tng is
the integration point if picture-MMS is wanted later — it would be a fourth
backend on the same Link, emitting the same `MessageIn`/`MessageSent`.

## Deliberate v1 boundaries

- No contacts, no SMS/USSD, no call audio routing (on ModemManager devices
  the audio path is the modem's/UCM's problem, as with plasma-dialer's
  modem-daemon split; if a device needs explicit routing, that is a new
  `Command`, not a UI change).
- No multi-call juggling UI (hold/swap): the session tracks multiple calls
  and a `Waiting` call takes the foreground, but the only verbs offered are
  accept/decline/hang-up.
- Number handling is region-blind by design (`number.rs` docs).
- Matrix: no media engine → incoming ring + decline only; a decline can lag
  up to ~5 s (single-thread sync loop with a 5 s server timeout — split into
  a feeder thread if that ever matters).

## Verification status (authoring pass)

`dialer-core`: **35 unit tests green on rustc 1.75**, both default-features and
`--features matrix`. New coverage includes the payload SMS round-trip and the
"plain text is never a command" property, the guard (running-off default,
trusted+prefixed bypass, shell-metacharacter inertness, newline rejection,
view-root confinement), the conversation store, and Matrix inbound
message/handoff parsing. (`--features matrix` needs ureq pinned to 2.9.7 + url
2.5.0/zeroize 1.7.0 downgrades ONLY because the sandbox cargo predates
`edition2024`; a modern toolchain resolves `ureq = "2"` freely.)

The egui surface, the mm.rs SMS additions (zbus), and the Matrix send path are
**not run here** (sandbox toolchain predates egui 0.34's MSRV). They pass
rustcheck Tier 0, and every egui-0.34 API the shell uses was checked by name
and signature against the egui 0.34 source (`corner_radius`, `copy_text`,
`open_url`/`OpenUrl::new_tab`, `stick_to_bottom(bool)`, `Frame::new`,
`long_touched`). A hardware `cargo build` is still required before calling any
of it working. Everything touching zbus (`mm.rs`) and the
whole egui surface is static-checked only (suspenders) — the sandbox
toolchain predates egui 0.34's MSRV — and needs a real `cargo build` plus an
on-device pass (FP5/PinePhone with ModemManager) before being called working.
