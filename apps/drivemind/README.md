# drivemind

Disk & data health: SMART for external SSDs behind USB-NVMe bridges,
BLAKE3 integrity manifests, reclaimable-space suggestions, delta-hostile
git scans (feeds `gin migrate`), video re-encode triage (feeds av1tuner),
and safety-checked clone plans — with a cross-signal advisor on top.

Shape matches gitmind: headless `drivemind-core`, scriptable `dvm` CLI,
opt-in egui shell on `shared/uikit`. Default-members build core+cli only.

```
dvm devices                # lsblk + attach classification (NVMe/SATA/USB bridge/virtual)
dvm smart /dev/sdX         # -d auto→sat→sntjmicron→sntasmedia→sntrealtek ladder, verdicts
dvm health                 # every SMART-capable device, one line each; exit 1 if ≥ Warning
dvm verify create ROOT M   # BLAKE3 manifest; `verify check ROOT M` classifies drift vs read errors
dvm cleanup PATH           # rebuildable dirs / big files / duplicates — suggests, never deletes
dvm videos PATH --batch F  # legacy/hot-bitrate codecs → av1tuner batch file
dvm gitscan REPO           # tracked delta-hostile files → `gin migrate` command
dvm clone SRC DST [--run]  # dd (healthy) or ddrescue (unhealthy) plan; blockers unforceable
dvm advise PATH            # everything above, one prioritized list
```

Design invariants:

- Every system command goes through the `SysRunner` seam (dyn-compatible);
  `FixtureRunner` replays canned outputs, so the whole SMART/lsblk surface
  is tested without root or real disks.
- Every operation reports through one `Event` sink; the CLI prints it to
  stderr, the egui shell renders it in a log pane. Same loop, two windows.
- Advice is commands, never actions. The only thing that writes to a
  device is `clone --run`, and blockers cannot be forced past.
