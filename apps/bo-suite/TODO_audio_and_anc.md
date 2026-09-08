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

- [ ] Say what the app CONTROLS versus what it only displays. ANC on most hardware is
      negotiated by the device; a UI that appears to set a level it cannot set is worse than
      one that shows the level and says where it is set.
- [ ] Whatever the transport is (BLE, USB HID, a vendor protocol), the argv/packet builder
      should be pure and tested the way `mediagen` and `visionpair` are in logscope — the
      command is what the tests pin, and the spawning is a thin layer.
