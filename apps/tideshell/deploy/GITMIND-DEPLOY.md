# One button: deploying tideshell to a phone from gitmind, without an AI

What the local AI did by hand over ssh is a fixed sequence with gates. This
is that sequence as a plan gitmind can run and depot can hold the artifacts
of, so a person clicks "deploy tideshell to fp5" and reads a report.

## Roles
- **gitmind**: runs the plan against a fleet entry (host, port, user, the
  `via` relay), shows each stage's gate as pass/fail, keeps the last report.
- **depot**: stores the built artifacts per commit and target
  (`tideshell/aarch64/<commit>/tideshell`, `comfortgov`), so a phone can be
  updated from a build made once, on the Mac in podman or on another phone.
- **the phone**: only ever receives files and runs the scripts already in
  `apps/tideshell/deploy/`; nothing on it is improvised.

## The plan (stages = the scripts, gates = their exit codes / lines)
| stage | command on the phone | gate |
|---|---|---|
| preflight | `sh preflight.sh` | last line `PREFLIGHT OK` |
| touch | `sh fp5-touch-diag.sh` | `ID_INPUT_TOUCHSCREEN=1` present |
| keys | install `70-tideshell-keys.rules` (sudo) or `adduser user input` | next run's `keys:` line has no `unreadable` |
| build | either `sh build-tideshell.sh start` … `status` until `DONE ok`, or upload depot's `tideshell/aarch64/<commit>` | `ldd` resolves libwayland-client + libEGL |
| install | `install -Dm755 tideshell ~/.local/bin/tideshell`, unit + `systemctl --user restart tideshell` | `systemctl --user is-active tideshell` = active |
| verify | `journalctl --user -u tideshell -n 40` | `ei:` / `keys:` / `backlight write mode:` lines present; no `panicked` |
| optional | cosmic-comp patch 0002 (stage 7 of DEPLOY-FP5.md) | `ei: Ready` |

Rollback is one line per stage (table in DEPLOY-FP5.md); a failed gate stops
the plan and shows the raw output, the same rule the AI followed.

## What gitmind needs (not built yet)
- fleet entry with `via` (the fp5 is reachable only through fp5-blue);
- a "plan" type whose steps are `scp` + `ssh` commands with a gate regex,
  run in order, each step's output kept;
- a depot fetch step: `depot get tideshell/aarch64/<commit>` -> file;
- the report view.

## What depot needs
- `put`/`get` by `<project>/<target>/<commit>/<file>` with the size and a
  checksum; `latest` per project/target.

## Build routes
- on the phone (5 min, the proven route): `build-tideshell.sh`;
- in podman on the Mac (emulated aarch64 Alpine): `build-in-podman.sh`,
  then depot `put`; unverified here, and slower per build, but hands-off.
