# Patchsets: frozen bases with our fixes, bumped on purpose

A patchset is a package the fleet depends on, pinned to one upstream version,
with the patches we need on top, shipped as an apk from gitmind's own repo.
`system/patchsets/<pkg>/patchset.txt` describes it; the patches sit beside it.

## Freeze
The phone's `/etc/apk/repositories` gets `@fleet http://gitmind.lan/apk`, and
its world entry becomes `cosmic-comp@fleet`. From then on only that repo can
supply the package: upstream moving to 1.9 changes nothing on the phone. Our
package is `1.8.0-r1` (the distro's `-r0` plus one), built from the upstream
tag with `abuild` (`default_prepare` applies the patches listed in `source=`).

## Stable bump
`gitmind patchset bump patchset.txt 1.9.0` rewrites the base and clears the
tested-on list. A rollout to the fleet is refused until one device is recorded
as tested (`rollout --untested` for that first device, then add a `tested =`
line). The rollout builds first, checks that every patch still applies, and
only then touches a device: arm a dead-man revert timer, install, restart the
compositor, confirm it is up, disarm. Any failed gate runs that step's rollback.

## Commands
```
gitmind patchset show     system/patchsets/cosmic-comp/patchset.txt
gitmind patchset apkbuild system/patchsets/cosmic-comp/patchset.txt aarch64 > APKBUILD
gitmind patchset freeze   system/patchsets/cosmic-comp/patchset.txt http://gitmind.lan/apk tideshell
gitmind patchset rollout  system/patchsets/cosmic-comp/patchset.txt http://gitmind.lan/apk /srv/aports
gitmind patchset bump     system/patchsets/cosmic-comp/patchset.txt 1.9.0
```

## For gitmind-egui (not built yet)
A Patchsets screen: each patchset with its version, patches and tested-on
list, and per device the installed version from `apk info -v` (the runner's
ssh), coloured by drift: matches the patchset, older, or upstream-untagged.
Buttons: build, freeze, rollout (with the gate log), bump. The cockpit module
(`crates/cockpit/src/patchset.rs`) is the whole logic; the screen only shows
it and calls the runner.

## Not in a patchset yet
The FP5 DPU brightness fix (pmaports #4274) is a kernel patch; the same
mechanism applies to `linux-postmarketos-qcom-sc7280` once the patch file is
in hand, with kerneltry's slots for the reboot.
