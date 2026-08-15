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

## Deliberate v1 boundaries

- No contacts, no SMS/USSD, no call audio routing (on ModemManager devices
  the audio path is the modem's/UCM's problem, as with plasma-dialer's
  modem-daemon split; if a device needs explicit routing, that is a new
  `Command`, not a UI change).
- No multi-call juggling UI (hold/swap): the session tracks multiple calls
  and a `Waiting` call takes the foreground, but the only verbs offered are
  accept/decline/hang-up.
- Number handling is region-blind by design (`number.rs` docs).

## Verification status (authoring pass)

`dialer-core` compiled and its 10 unit tests ran green on rustc 1.75
**without** the `mm` feature. Everything touching zbus (`mm.rs`) and the
whole egui surface is static-checked only (suspenders) — the sandbox
toolchain predates egui 0.34's MSRV — and needs a real `cargo build` plus an
on-device pass (FP5/PinePhone with ModemManager) before being called working.
