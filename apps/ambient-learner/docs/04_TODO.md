# TODO

Open work, ranked by tier. Tier 1 is "block other work or are user-
facing bugs"; Tier 4 is "would be nice eventually". Items move
upward as priorities shift.

When closing an item, delete it; don't leave "DONE" markers. The
git log is the change history.

## Tier 1 — Active blockers / user-facing

- [ ] **Test on actual mobile hardware.** The 900 °C / 300 °C bug
      was found that way; there are probably more. Likely culprits:
      thermal zone count, sensor name patterns in screen layout,
      QML font scaling on 360 dpi displays.

## Tier 2 — Quality / cleanliness

- [ ] **Split `src/main.rs`** (6000+ lines). Extract:
      - `Database` impl → `src/database.rs`
      - `Model` impl → `src/model.rs`
      - `ThermalPoller` → `src/thermal_poller.rs`
      - GTK `build_ui()` → `src/ui/gtk4/build.rs`
      Mechanical move; estimated 1-2 hours.

- [ ] **Per-device screen-temp layout config.** Default is
      `ScreenLayout::generic_phone()`. A real device-specific
      config (PinePhone, Librem 5, etc.) would place sensors at
      their actual PCB coordinates. Probably a TOML file in
      `/etc/ambient-learner/devices/` or similar.

## Tier 3 — Features

- [ ] **First-contact approval prompt** for D-Bus callers. Today
      every submission is accepted; audit trail exists but no
      moderation UI.

- [ ] **Database list / switch UI.** `list_all_databases()` exists.
      A QML page that shows them and lets the user pick which one
      is "latest" would help with the import workflow.

- [ ] **EMA smoothing in screen-temp publisher.** Discussed,
      decided yes for v1; not yet implemented. ~60 lines.

- [ ] **Raw side-channel** for screen-temps: publish both the EMA-
      smoothed default file and an unsmoothed `screen-temps.raw.bin`
      for consumers that want full control.

- [ ] **Permission grants for snapclone-bus inbound pins.**
      Currently the bus client trusts anything coming in. A per-
      source allow-list (or first-contact prompt) would be
      consistent with the ADR-006 plan.

## Tier 4 — Long-term / nice to have

- [ ] **Migrate to zbus 5.0.** Rename `SignalContext` → `SignalEmitter`;
      check for other API changes.

- [ ] **Iced backend: continue developing as a COSMIC experiment.**
      Iced is the toolkit COSMIC ships, so the Iced backend here is
      a stepping stone toward a future COSMIC-native version of
      this app. Play to Iced's strengths (Canvas viz, pure-function
      views, keyboard shortcuts) rather than chasing Qt parity.
      See ADR-015 for the shape.

- [ ] **Replace `qmetaobject` with `cxx-qt` or similar.** The
      macro-derived Qt properties are slowing compile time and the
      ergonomics for adding properties are deteriorating.

- [ ] **Spatial reasoning per location.** Today `GetTemperatureAt(lat,
      lon)` returns the global prediction with `in_range = true` if
      coordinates are near a saved location. Real per-location
      interpolation needs more data and a different model.

- [ ] **Hot-plug sensor support.** Today sensor count is fixed at
      startup; resizing models and history buffers at runtime is
      not trivial.

- [ ] **Hint file expiry sweep.** Stale hints are filtered on read
      but never deleted from disk. A cleanup pass once an hour
      would tidy up.

## Tier 5 — Investigation / unsure if worth it

- [ ] **Federated sensor calibration.** If two devices in the same
      room broadcast over snapclone-bus, can we cross-calibrate?
      Maybe. Probably a research project, not a product feature.

- [ ] **Wayland clipboard integration** for the bitfield. The screen-
      temp data could in principle be offered as a clipboard MIME
      type. Use case is thin.

- [ ] **Embedded screen-temp visualizer widget.** A drop-in QML/Qt
      widget other apps could embed to show the heat map. Probably
      the wrong abstraction — they should just read the file.

## Items in progress (don't add to Tier 1; track here)

(nothing currently)
