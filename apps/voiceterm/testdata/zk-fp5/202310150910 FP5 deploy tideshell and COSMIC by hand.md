# FP5 deploy tideshell and COSMIC by hand

The manual path from a Linux machine to tideshell running on the phone -- the
same build gitmind's push now runs for you (see *What gitmind does and does not
do* below), spelled out for when it is not available or something needs
checking by hand. Every
command here is lifted from `apps/tideshell/deploy/DEPLOY-FP5.md` unless it
is marked **unverified**; that file is the authority and has the per-row
checks this note leaves out.

Phone access throughout is the reverse tunnel on `127.0.0.1:8022`; scp takes
the port as a flag: `scp -P 8022 FILE user@127.0.0.1:~/`.

## 1. Preflight (read-only)

```sh
scp -P 8022 apps/tideshell/deploy/preflight.sh user@127.0.0.1:~/
ssh -p 8022 user@127.0.0.1 'sh preflight.sh' | tee preflight.txt
```

It names what is missing -- a session bus (no trackpad), rusty-board (Keyboard
mode fails), cosmic-panel (possible overlap). It must be run from inside the
COSMIC session on the phone, so step 5 comes first on a phone still on Plasma
Mobile.

## 2. Build

On an **aarch64 host** (an M-series Mac on Asahi Linux) the Alpine container
runs natively -- the same musl, libwayland and libEGL as postmarketOS, no
emulation:

```sh
sh apps/tideshell/deploy/pack-src.sh HEAD             # -> tideshell-src.tar.gz; refuses if a needed crate is missing
cd apps/tideshell/deploy
sh build-in-podman.sh ../../../tideshell-src.tar.gz   # -> out/tideshell, out/tideshell-settings, out/comfortgov
```

On an x86 host the same script runs under qemu (slow). The other way is to
build ON the phone: copy the tarball over and follow DEPLOY-FP5 stage 4
(`sudo apk add wayland-dev mesa-dev pkgconf` -- names **unverified**; add what
the linker asks for).

Not gitmind's generic musl image: it is a cross toolchain without musl
libwayland, and tideshell's `wayland-sys` needs it at build time. (gitmind now
picks tideshell's own Containerfile instead -- the same image as this script.)

## 3. Install

```sh
scp -P 8022 out/tideshell out/tideshell-settings user@127.0.0.1:~/
ssh -p 8022 user@127.0.0.1 'install -Dm755 tideshell ~/.local/bin/tideshell && install -Dm755 tideshell-settings ~/.local/bin/tideshell-settings'
scp -P 8022 apps/tideshell/deploy/tideshell-settings.desktop user@127.0.0.1:~/
ssh -p 8022 user@127.0.0.1 'install -Dm644 tideshell-settings.desktop ~/.local/share/applications/'
```

## 4. Run inside the session (the real test)

```sh
scp -P 8022 apps/tideshell/deploy/run-in-session.sh user@127.0.0.1:~/
ssh -p 8022 user@127.0.0.1 'sh run-in-session.sh --show-strips'
```

Then walk DEPLOY-FP5 stage 5's table row by row; autostart (stage 6) only
after it passes.

## 5. Getting onto the COSMIC session (from Plasma Mobile)

**Verified:** COSMIC is packaged in Alpine and postmarketOS (announced in the
postmarketOS 2025-03 update), and the UI metapackage is
`postmarketos-ui-cosmic` -- built for aarch64 (`main/aarch64/
postmarketos-ui-cosmic-2-r4` on build.postmarketos.org); it pulls in
`cosmic-session`. The postmarketOS wiki has a COSMIC page (wiki.postmarketos.org,
"COSMIC") -- read it for the current details before switching.

**Still unverified here:** how the installed login manager picks the session
on the FP5 image. Keep Plasma Mobile installed and an ssh session open:

```sh
ssh -p 8022 user@127.0.0.1
sudo apk add postmarketos-ui-cosmic   # alongside Plasma Mobile, not instead
ls /usr/share/wayland-sessions/       # a cosmic entry should now be listed
```

Then choose the COSMIC session at the login screen (or set it as the default
the way the wiki page describes), log in, and run step 1 from inside it.

If COSMIC does not come up: from the open ssh session, choose Plasma Mobile
again, or `sudo apk del postmarketos-ui-cosmic`, and restart the display
manager. Plasma stays installed until COSMIC has been seen working.

## What gitmind does and does not do

- It **pins and patches** `cosmic-comp` (touch focus; the EI allowlist for
  tideshell's trackpad) from its own `@fleet` apk repository, with a timed
  rollback -- once COSMIC is installed (`cockpit::patchset`).
- Its push page has a "COSMIC + tideshell (wayland + layer)" feature choice
  and installs launchers.
- It does **not** install COSMIC or switch the session from Plasma Mobile.
- Its push builds tideshell in **tideshell's own container**
  (`deploy/Containerfile.aarch64`, built once as
  `localhost/tideshell-build-aarch64`), natively on an aarch64 host, with the
  crate registry cached in `~/.cache/gitmind/`. It still tries a local build
  first and falls back to the container when that fails (for tideshell, early,
  at `wayland-sys`).

Up: [[202310150900 Fairphone 5]]. Related: [[202310150907 FP5 mainline Linux gaps]] (why stock and
postmarketOS differ), [[202310150909 FP5 camera pipeline]] (the camera works
under Plasma but not yet under COSMIC -- `fp5-camera-diag.sh`).

Source: `apps/tideshell/deploy/DEPLOY-FP5.md`, `apps/gitmind` (c20unified).

#fairphone #deploy #tideshell #cosmic #postmarketos
