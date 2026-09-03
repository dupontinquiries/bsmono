# Running and seeing a GUI app from an agent sandbox

An egui app in this repo can be **built, launched, driven with synthetic
keystrokes, and screenshotted** from a Claude sandbox. Nothing exotic is
needed — four apt packages and a habit of splitting long waits across
tool calls.

This is written because I spent sixteen commits on `voiceterm-egui`
reasoning about a UI I had assumed I could not run, and the first
screenshot overturned a premise four of those commits were built on. The
compiler had been green the whole time. **A compiler cannot tell you
which of two grids reaches the screen.**

## The claim this corrects

`CLAUDE.md` §8 says "GPU, Qt and GTK paths cannot be built in this
sandbox". For **Qt** that is still true (`qmetaobject` needs Qt headers,
which are not installable here). For **egui/eframe/glow it is false** —
they build, link, and run against software GL. The claim was written for
Qt and quietly generalised; I inherited it and did not check for nine
commits.

Check before you assume. It costs one `cargo build`.

## Setup

```sh
apt-get install -y --no-install-recommends xvfb openbox xdotool imagemagick
```

Roughly 150 MB. `x11-apps` is not needed — ImageMagick's `import` does
the capture.

**`openbox` is not optional.** Under bare `Xvfb`, X input focus never
settles on the window: `xdotool windowactivate` reports *"Your
windowmanager claims not to support _NET_ACTIVE_WINDOW"* and every
synthetic keystroke lands nowhere. With a window manager running, the
same commands work. I lost two rounds to this before installing one.

## Running it

```sh
export DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1
nohup setsid Xvfb :99 -screen 0 1280x800x24 >/tmp/xvfb.log 2>&1 </dev/null &
sleep 3
nohup setsid openbox >/tmp/ob.log 2>&1 </dev/null &
sleep 2
nohup setsid ./target/debug/the-app >/tmp/app.log 2>&1 </dev/null &
sleep 9
```

`LIBGL_ALWAYS_SOFTWARE=1` because there is no GPU; llvmpipe is enough
for an egui app at 1280x800, though frames are slow enough to matter
(see typing delay below).

### Background processes die when a bash call times out

This is the single most disruptive gotcha. If a `bash_tool` call exceeds
its limit and returns an error, **everything it started is killed**,
`nohup setsid` notwithstanding. A command with `sleep 20` in it will
often take the app down with it.

So: **keep each call under ~25 s** and split the work.

```
call 1:  start Xvfb + openbox + app, sleep 9, print pgrep count
call 2:  activate window, type, press Return, sleep 3, capture
call 3:  view the screenshot / read the side effect
```

Verify the app is alive at the start of each call (`pgrep -c the-app`)
rather than assuming it survived.

## Driving it

```sh
W=$(xdotool search --onlyvisible --name "AppTitle" | head -1)
xdotool windowactivate --sync $W
xdotool type --clearmodifiers --delay 250 '  echo HELLO'
sleep 2
xdotool key --clearmodifiers Return
```

Three things that are not obvious:

* **`--delay 250`, not the default.** Software GL makes frames slow, and
  the app only drains events when a frame runs. At `--delay 40` I got 2
  of ~26 characters. At 250 I got all of them.
* **Lead with two spaces.** The first character is routinely lost while
  focus settles, so `echo X` arrives as `cho X`. A throwaway prefix
  costs nothing and saves a confusing rerun.
* **`xdotool type --window $W` is unreliable here**; activate the window
  and type to the focused target instead.

## Seeing it

```sh
import -window root /tmp/shot.png
```

Then open it with the `view` tool. This is the part worth building the
rest for: **you can look at the app.** In one screenshot I established
that the terminal rendered the demo seed and not the shell, which no
amount of reading had told me.

## Observing without pixels

Screenshots answer "does it look right". For "did it actually happen",
prefer a side effect you can check from bash:

* **A process**: type `sleep 41`, then `ps -o pid,ppid,cmd -e | grep`.
  Exactly one match means the command ran exactly once — this is how the
  double-execution bug was finally settled.
* **A file**: `echo X >> /tmp/proof`, then `wc -l`. Beware: `>>` is a
  redirect, and voiceterm's `cmd_risk` classifies redirects as
  *Mutating*, so its confirmation gate withholds the Enter and nothing
  runs. That is the gate working correctly and it looks exactly like a
  failure.

### Two ways the instrument lied to me

Both produced an alarming number that was not a bug:

* **`pgrep -c -f 'sleep 53'` returned 2.** It counts its own command
  line. `ps -o pid,ppid,cmd -e | grep` showed one process. Never use
  `pgrep -c -f` to count occurrences of a string you just typed.
* **Two `/bin/sh -l` children looked like a double spawn.** The default
  workspace has two panes and spawns one shell each. I "fixed" it before
  checking. An `eprintln!` probe at each spawn site settled it in one
  run: `panes=[0, 1]`.

When a measurement looks like a bug, **probe before patching**. A
temporary `eprintln!` at the two candidate sites, rebuilt and rerun, is
about three minutes and it has been right every time I have used it —
where guessing was wrong twice.

## What this cannot do

* **Read text out of the window.** No OCR here; you can look at a
  screenshot yourself, but you cannot assert on it in a test.
* **Judge ergonomics.** Whether a tap lands on the word you meant on a
  phone, whether a control is reachable one-handed, whether anything is
  legible at real DPI — none of that survives a 1280x800 virtual screen.
* **Anything Qt.** `qmetaobject` still needs Qt headers. The pure
  modules of `apps/voiceterm` compile standalone with
  `rustc --test src/module.rs`; the app crate does not build.

## Disk

The sandbox filled to 90% during this. `target/` for an eframe app is
several GB. Clear other crates' `target/` directories before starting,
and expect a full debug build of a large egui binary to be ~100 MB on
its own.
