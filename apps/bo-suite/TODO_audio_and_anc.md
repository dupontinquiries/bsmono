# TODO: bo-suite-egui — the audio player, the UI, and ANC

Asked 2026-09-08: *"finish the audio player, keep working on the UIUX, keep working on the
active anc features"*. None of it is started in this session; this file is the shape of the
work rather than a claim about it.

## Audio player

- [ ] What "finish" means, written down before anything is built. A player that can start a
      file is not the same as one that can be trusted with a queue, and the difference is
      where all the remaining work is.
- [ ] Transport state as a machine, not booleans. Play/pause/stop/seek/next interact, and the
      combinations that get shipped broken are the ones nobody enumerated — seek while
      paused, next at the end of a queue, stop during a seek.
- [ ] The decode thread's relationship to the UI thread. A player that blocks a frame on a
      decode drops audio AND drops frames, and the fix afterwards is a rewrite.
- [ ] Position reporting: from the audio clock, not from a UI timer. A progress bar that
      drifts from the sound is the complaint every player gets.

## UI/UX

- [ ] Run `uikit_egui::uxaudit` over each surface first (see `logscope-egui::uxloop` for the
      loop shape). It finds blank panes, missing backdrops, overflow and unreadable contrast
      mechanically, which is a better starting list than an opinion.
- [ ] The motion primitives are in `uikit_egui::motion` and honour `UIKIT_REDUCE_MOTION`;
      use `dur` for every duration so the switch reaches this app too.

## Active ANC

- [x] Say what the app CONTROLS versus what it only displays. The answer was already in
      `anc.rs`: `process` has no caller here BY DESIGN — cpal lives in `bo-controller`, which
      owns the engine and feeds it, and this app is the control surface. What was wrong is
      that the panel did not say so. In Auto with no snapshot, `phase()` returns `Quiet` for
      `None`, so the readout claimed "quiet — nothing to cancel" about a room nothing had
      listened to. It now says it is waiting for the engine, and the dot is HOLLOW rather
      than grey-filled — grey-filled and grey-hollow both read as "not emitting", but only
      one also means "and I do not know what the room sounds like".
- [ ] The other half: nothing carries a snapshot FROM `bo-controller` to this app. Until that
      link exists the panel is permanently in the waiting state — which is now at least
      honest, but it is not the feature.
- [ ] Whatever the transport is (BLE, USB HID, a vendor protocol), the argv/packet builder
      should be pure and tested the way `mediagen` and `visionpair` are in logscope — the
      command is what the tests pin, and the spawning is a thin layer.
