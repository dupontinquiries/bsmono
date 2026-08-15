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

`dialer-core` compiled and its 19 unit tests ran green on rustc 1.75, both
default-features and `--features matrix` (the latter with ureq pinned to
2.9.7 plus url 2.5.0 / zeroize 1.7.0 downgrades, needed only because that
sandbox cargo predates `edition2024`; modern toolchains resolve `ureq = "2"`
freely and the APIs used are stable across 2.x). Everything touching zbus (`mm.rs`) and the
whole egui surface is static-checked only (suspenders) — the sandbox
toolchain predates egui 0.34's MSRV — and needs a real `cargo build` plus an
on-device pass (FP5/PinePhone with ModemManager) before being called working.
