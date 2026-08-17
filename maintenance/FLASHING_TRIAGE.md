# Flashing on Asahi: which layer is actually broken

Three problems are getting bundled into one, and only one of them is on the
critical path to an OTA flasher.

1. **The web flasher fails on Linux, works on macOS.**
2. **Asahi's USB stack may have quirks** that fastboot and EDL hit.
3. **pmbootstrap is Python** and a Rust rewrite would be nicer.

They are not the same problem, and (3) does not fix either of the others —
pmbootstrap *builds images* and shells out to fastboot to flash them. Rewriting
it in Rust touches no USB code at all. Doing it first would be months of work
that leaves the flashing failure exactly where it is.

## (1) is almost certainly udev, not Asahi

WebUSB on Linux cannot touch a device unless a udev rule has granted the
logged-in user access to the `/dev/bus/usb/**` node — the kernel refuses the
browser otherwise. macOS has no equivalent gate. That asymmetry alone predicts
"works on macOS, silently fails on Linux" without invoking the USB stack at
all.

This is a known, filed problem with the postmarketOS web flasher specifically:
its issue tracker carries "Inform the user that they might need to run the
flasher as the superuser to access the device", and the diagnosis there was a
distro not shipping the `android-tools` udev rules that other distros do.
Note also that the project is **archived and read-only**, so it is not
something to build a workflow on.

Two mechanisms, distinguishable:

* **Missing udev rule** — no ACL on the device node. Affects the CLI too.
* **Browser confinement** — a snap or flatpak Chromium cannot reach raw USB
  even when the rule exists and the CLI works. Different fix (`snap connect
  chromium:raw-usb`, or a non-confined build).

And for EDL specifically there is a third, unrelated to permissions: the
`qcserial` kernel driver binds Qualcomm 9008 and holds the interface, so
userspace cannot claim it. That one needs a driver unbind, not an ACL.

## The decisive test, in order

Run these on the Asahi box with a device in fastboot mode. Each step
eliminates a layer; stop at the first one that changes the answer.

```sh
lsusb                      # is the device enumerating at all?
fastboot devices           # as your user
sudo fastboot devices      # as root
```

* **`lsusb` does not show it** → the only result that implicates the USB stack
  or the cable. Everything below assumes it enumerates.
* **Root works, user does not** → udev. Install the android-tools rules,
  `udevadm control --reload-rules && udevadm trigger`, replug. Nothing to do
  with Asahi or with browsers.
* **Both work, browser still fails** → browser confinement. Check
  `chrome://device-log` for the actual error; "access denied" there with a
  working CLI is the snap/flatpak signature.
* **Root fails too, but `lsusb` sees it** → now it is worth suspecting the
  driver or the stack. Check `dmesg` for a driver having claimed the
  interface, and `ls /sys/bus/usb/drivers/*/` for the binding.

For EDL, add: `lsmod | grep qcserial` and check whether the interface is bound
before deciding anything about the controller.

The reason to run this before writing code: the "deeper USB stack" hypothesis
is the expensive one and the least likely, and thirty seconds of `sudo
fastboot devices` either falsifies it or promotes it to the real problem.

## What reading pmbootstrap changed

Two facts from the source that the triage above did not account for.

**pmbootstrap already runs fastboot as root, inside a chroot.** `pmb/flasher/run.py`
ends every action with `pmb.chroot.root(command, ...)`. So the udev ACL is
irrelevant to pmbootstrap's own flashing — it is root, the permission gate does
not apply. That sharpens the diagnosis rather than muddying it:

* If `pmbootstrap flasher list_devices` works while the web flasher does not,
  the udev/browser hypothesis is **confirmed**, because the only difference
  between those two paths is the permission gate.
* If `pmbootstrap flasher list_devices` ALSO fails, the problem is below
  permissions, and the chroot is the next suspect before the controller — see
  below.

So add one line to the triage, run before the others:

```sh
pmbootstrap flasher list_devices     # root, inside the chroot
```

**The chroot needs USB bind-mounted in, and that is a separate failure mode.**
`pmb/config/__init__.py` carries `flash_mount_bind`: `/sys/bus/usb/devices/`,
`/sys/dev/`, `/sys/devices/`, `/dev/bus/usb/`. Those four get bind-mounted into
the native chroot before a flash program starts. If any is missing or empty
inside the chroot, fastboot sees no device and reports exactly what a dead USB
stack reports — nothing. Checking `ls /dev/bus/usb` from inside the chroot
distinguishes "the host cannot see the phone" from "the chroot cannot see the
host's phone", and those have completely different fixes.

This is the failure most likely to be mistaken for an Asahi quirk, because it
produces a hardware-shaped symptom from a mount-namespace cause.

## What to build, if the answer is "udev"

Then the browser was never the right path anyway, and the useful piece is
narrow: **a Rust fastboot client**, not a pmbootstrap rewrite.

* The fastboot protocol is small — a handful of ASCII commands over bulk
  transfers (`getvar`, `download:%08x`, `flash:%s`, `reboot`), with four
  response prefixes (`OKAY`, `FAIL`, `DATA`, `INFO`).
* **pmbootstrap's flasher is 552 lines of 26,405** — about 2% of the codebase,
  and most of it is a data table. `pmb/config/__init__.py` holds `flashers` as
  a dict of method → action → argv templates with `$PARTITION_ROOTFS`,
  `$IMAGE`, `$BOOT` substituted in; `run.py` does the substitution, checks a
  partition blacklist, and shells out. That is the whole design. Porting *it*
  is tractable in a way that porting pmbootstrap is not, and the table
  translates to Rust almost literally.
* `nusb` (pure Rust, no libusb) or `rusb` handles the transport.
* Shipping the udev rule alongside it makes the permission problem a packaging
  concern rather than a thing the operator rediscovers per machine.
* It slots straight into the fleet layer: `Step::Flash` beside `Step::Enroll`,
  and the OTA flasher is then the same planner and the same progress view as
  everything else.

That is a bounded piece of work with an obvious test seam — the protocol
encoder and response parser are pure and testable; only the transport needs
hardware.

## What not to do yet

Do not rewrite pmbootstrap. If the Rust flasher exists and works, the parts of
pmbootstrap that matter for this workflow are image *building*, which is slow,
correct, and not the thing causing pain. Revisit only if image building
becomes the bottleneck.

## Status

Unverified from here — this sandbox has no USB device, so the triage above is
a procedure to run, not a result. What IS established: the pmOS web flasher's
Linux failure is a documented permissions issue with a documented workaround,
and the project hosting it is archived.
