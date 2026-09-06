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

## The cheaper path: offscreen, no display at all

Everything above is for driving a real app. If all you need is a PICTURE
of some geometry, you do not need X, a window manager, or input
injection. wgpu will render to a texture with only an adapter, and
lavapipe provides one in software:

```sh
apt-get install -y --no-default-features mesa-vulkan-drivers   # lavapipe
export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json
env -u DISPLAY -u WAYLAND_DISPLAY ./target/debug/examples/botshot <asset> out.png
```

Verified with `DISPLAY` and `WAYLAND_DISPLAY` both unset. It prints
`XDG_RUNTIME_DIR is invalid or not set` and renders anyway — that warning
comes from a winit/env probe, not from the render path.

`games/blackstar-rs/examples/botshot.rs` is the worked example: it builds
a device, a pipeline and a depth buffer, draws, and reads the target back
to a PNG. Roughly 200 lines, no swapchain, no event loop.

**This is the one to reach for first.** Three of the findings in this
repo's asset work came from looking at such a render, and two of them
contradicted a metric that had looked convincing. A PNG costs seconds and
settles arguments that numbers do not.

**And know what it does NOT show.** `botshot` originally rendered the
authored mesh, while the game applies a stow transform before drawing.
The still was therefore of geometry the game never displays, and an
investigation ran a long way on that before anyone noticed. If a harness
renders assets, make it apply the same transforms the runtime does, or
say loudly in its `--help` that it does not.

## Clicking things

`xdotool getwindowgeometry --shell` reports the **client area**, and
under openbox that is not where the frame is — computing a click from it
lands outside the window. Read coordinates off a screenshot instead and
pass them to `xdotool mousemove X Y click 1` in root space.

`xdotool windowactivate` sometimes prints
`XGetWindowProperty[_NET_ACTIVE_WINDOW] failed (code=1)` and works
anyway. It is not a reason to stop and retry.

## Disk

**Two egui apps do not fit in this sandbox at once.** Measured:

| app | build | binary | target tree |
|---|---|---|---|
| `voiceterm-egui` | ~2 min | 96 MB | 2.3 GB |
| `aperture` (wgpu + tree-sitter) | ~9 min | 413 MB | 3.0 GB |

Build one, look at it, `rm -rf target`, then build the other. I hit 96%
full mid-link and had to free the first tree before the second would
finish.

Poll a long build from separate short tool calls
(`nohup cargo build > /tmp/build.log 2>&1 &` then `tail -1
/tmp/build.log`) — a single call that waits for a nine-minute build will
time out and take the build with it.

## Seeing a logscope SCENE (not the graph on top of it)

Every attempt to check scene geometry this session was defeated by the same
thing: `scene-view` draws the conversation graph over the world, and with
the shipped 180-build spine the dots and labels cover most of the frame. Two
sessions' worth of "not visually confirmed" came from that, not from any
difficulty with Xvfb.

`LOGSCOPE_CONVO` points at the `.logscope` script the preview loads. Point
it at a nearly-empty one and the graph goes away:

    printf 'user: hi\n' > /tmp/tiny.logscope
    LOGSCOPE_CONVO=/tmp/tiny.logscope \
    LOGSCOPE_THEME=Alley LOGSCOPE_CAM="0.9,0.35,26" \
    LD_LIBRARY_PATH=$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/lib \
    DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 ./scene-view &

then `xdotool search --name Logscope` + `xdotool windowmap <id>` (winit
leaves the window unmapped without a WM), wait 5-8 s for the first llvmpipe
frame, and `import -window root`.

`LOGSCOPE_CAM="yaw,pitch,dist"` is radians, radians, world units. Finding a
camera that frames a particular prop is still trial and error — pitch around
0.3-0.6 and dist 20-40 looks along a lane; small dist puts the eye inside
geometry.

Also needed on this binary: it is dynamically linked against the toolchain
libstd, hence the `LD_LIBRARY_PATH` above, or it dies before opening a
window.

### What the scene renders taught, one session later

`apps/logscope/tools/scene-shot.sh` wraps the recipe above in two halves
(`start`, then `grab` in the next call) and pins four things every earlier
render had wrong:

* **`LOGSCOPE_HOUR=14`.** The sandbox clock is UTC 04:00-ish; the sun model
  follows it, so an unpinned render is a night shot and every scene looks
  underlit. Three rounds of "brighten the albedo" went to this.
* **`LOGSCOPE_POWER=full`.** llvmpipe's `GL_RENDERER` trips the weak-GPU
  trim, which drops props by screen size.
* **`LOGSCOPE_CAM=yaw,pitch,dist,tx,ty,tz`** — the six-value form orbits a
  FIXED point. The three-value form can only orbit the graph centroid, which
  puts a prop outside the graph's footprint (the physical UI panel) at the
  frame edge or behind the camera. `LOGSCOPE_DEBUG_LABELS=1` dumps every
  mesh's centroid to `/tmp/scene_labels_<theme>.txt`; grep the prop's
  emissive or size there to get `tx,ty,tz`.
* **A/B the renderer switches before the geometry.** Floors black at every
  albedo were not dark: `LOGSCOPE_CULL=0` brought them back in one frame
  (every disc floor wound clockwise), and `LOGSCOPE_SHADOWS=0` ruled the
  shadow map out first. A far plane fitted to the graph's extent cut the
  world at forty units. All three are fixed at the source now, but the
  order of suspicion — camera range, culling, shadows, then colour — is the
  part to keep.
