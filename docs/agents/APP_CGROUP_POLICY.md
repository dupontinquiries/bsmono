# Per-app resource policy for Tideshell (design, not built)

Written 2026-10-03 for the fp5 (Fairphone 5, 8 cores, ~7.5 GB, COSMIC + Tideshell).
Goal: the shell and keyboard stay smooth when one app goes wrong (a browser with 50 tabs, a stuck Qt app), and
background apps cost less battery. **Nothing here is implemented yet.**

## What was checked on the phone (2026-10-03)

- **The mechanism needs no root.** The user's `app.slice` has the `cpu memory pids` controllers. In an app scope,
  `cgroup.freeze`, `cpu.weight`, `cpu.max`, `memory.high` and `memory.max` exist and are writable by the user.
- **Tideshell already launches apps into their own scope**, named `app-tideshell-<desktop-id>-<pid>.scope`
  (`egui-frontend/src/effects.rs`, `scope_unit_name`). Flatpak apps launched elsewhere get
  `app-flatpak-<id>-<n>.scope`.
- **Tideshell already knows the active app** (the Wayland toplevel list; `write_foreground_hint` in
  `bin/tideshell.rs` already writes it to `comfortgov.foreground`).
- **Anomaly to handle:** kweather's process sat in `app-tideshell-org.kde.kweather-9541.scope (deleted)`, a scope
  whose directory was gone while the process lived. So the policy must find an app's cgroup from
  `/proc/<pid>/cgroup` each time and tolerate a missing or deleted one, never assume the scope name.
- Today Tideshell has `CPUWeight=500` and rusty-board `400` (drop-ins in `~/.config/systemd/user/*.service.d/`).
  Apps are at the default 100.
- The CPU governor is `performance` on all cores (the owner's choice; left alone).

## Policy

Per app, a state from (is it the active toplevel, is it audible/playing, how long unfocused):

| state | cpu.weight | other | notes |
|---|---|---|---|
| foreground | 200 | none | above background apps, below the shell (500) |
| background | 20 | memory.high set to a share of RAM | runs, but loses every contention |
| idle background (unfocused > 5 min, not exempt) | — | **cgroup.freeze = 1** | zero CPU; thaw on focus |
| exempt | 100 | never frozen | music/calls/navigation/timers; user list |

- **Freeze is the strong lever and the risky one.** It breaks anything that must keep running (alarms, a download,
  a call, a sync). So: freeze only after the idle timeout, never when the app holds audio or a wake lock, and
  keep an exempt list the owner can edit. Unfreezing on focus must be instant and is the first thing tested.
- **Never freeze the shell, the keyboard, the compositor, the portal/dbus services, or anything not in an app
  scope.** The policy touches only scopes it can name.
- **Memory:** `memory.high` (throttles, does not kill) on background apps; leave `memory.max` alone. Add
  `systemd-oomd` only after measuring that the phone actually swaps under load.
- **Rejected:** a CPU hard cap (`cpu.max`) by default. It wastes cycles when the machine is idle, and weights
  already protect the shell under contention at no idle cost.

## Where the code goes

1. **Pure decision core** in `tideshell-core` (`appcg.rs`): inputs are the app list with focus, audio state and
   times; output is the list of `(app, Action)`. No filesystem, no clock of its own, so it can be tested.
   Tests to write first: the foreground app is never frozen; an exempt app is never frozen; an audible app is
   never frozen; unfreeze happens on focus the same tick; a state change never produces two conflicting
   actions; a missing cgroup is skipped, not an error.
2. **Thin applier** in the egui frontend: reads `/proc/<pid>/cgroup`, writes the files, logs every action
   (which app, what, why) to the shell trace so a surprise freeze can be explained afterwards.
3. **Settings:** one switch for the whole policy, an exempt list, and the idle timeout. Off by default until
   the owner has watched the log for a day.

## Open questions (answer before building)

- How to know an app is audible: PulseAudio/PipeWire stream owners by pid (pipewire is failed on this phone,
  see the user-unit listing), or skip audio detection and rely on the exempt list at first?
- Flatpak apps run inside a bwrap sandbox: confirm a freeze on the scope reaches every process of
  GoofCord-like apps (zygote helpers) and that thaw leaves the app working.
- The stall log (queue row to add) should be on before this is, so the effect can be measured, not guessed.
