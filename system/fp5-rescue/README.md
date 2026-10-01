# fp5-rescue

The last resort for a phone whose shell, compositor and network are all gone.

* **Chord:** hold the power key and tap volume-down three times (within 5 s of pressing power,
  each tap within 1.2 s of the last). `fp5-rescued` (root, std only) sees it and starts
  `fp5-rescue-console.service`.
* **Console:** switches to tty3, starts `buffyboard` (an on-screen keyboard for the console) and a
  normal `agetty` login. Log in, fix things, log out: the screen goes back to the VT it came from.
* **Over the cable:** the separate `emergency_access_plan` in gitmind's `fp5_setup` gives sshd a
  password login on the USB gadget address (172.16.42.1) only.

Modelled on postmarketOS's initramfs debug shell (`/usr/share/initramfs/init_functions.sh`:
`iskey KEY_LEFTCTRL KEY_VOLUMEDOWN` at boot, then buffyboard + getty + telnetd on the USB net). That
shell exists only during boot; this one is for a running system.

Not verified on a phone yet: the VT switch away from cosmic-comp (logind should pause its session)
and buffyboard under it. `fp5-rescued --probe` and `--dry-run` are safe to run first.
