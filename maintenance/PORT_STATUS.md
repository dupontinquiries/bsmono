# egui port status — measured, not claimed

Run `python3 maintenance/tools/parity_audit.py` for the current numbers. The
hand-written parity ledgers drift the moment a widget lands without someone
editing a table; this derives the state from the source instead.

## What it checks

Three things, because a port can be "done" and still reach nobody:

* **Surface coverage** — UI surfaces the original exposes (Qt classes, QML
  files, Rust view structs) that the port names anywhere in its source.
* **Deployability** — a `[[bin]]` and `default = ["app"]`, so
  `cargo build --release` produces something runnable.
* **Catalogued** — present in `manifest/projects.toml`, so gitmind can see it.

## Current state

Snapshot of the last `parity_audit.py` run. It is a snapshot and it will drift —
the tool is the truth, and this table exists so a reader who has not run it can
see roughly where things stand.

The table now reports TWO numbers. `named` is the upper bound — somebody at
least knew the surface existed. `code` is the lower bound — something outside a
comment references it. Neither is the truth, and the gap between them is where
to go looking: WayfinderPage and GroupVoiceBar were both sitting at the top of
that gap, counted as covered while one page drew a different feature and the
other drew nothing.

Read the percentage as a worklist, not a score. The detector matches a
surface's NAME anywhere in the port, so it cannot tell a ported widget from a
doc comment that mentions one; `parity_audit.py --why <Surface>` prints every
match with its location and marks which are comments, so any single claim can
be checked in about ten seconds. snapclone's denominator jumped 38 → 58 when
the detector learned to find surfaces by inheritance rather than by name
suffix, which dropped its number from an inflated 39% to 34%. Expect the same
correction wherever a Qt app names things in families the old suffix list could
not see.

| port | surfaces | covered | bin | catalogued | primary |
|---|---|---|---|---|---|
| bo-suite | 47 | 94% named / 36% code | yes | yes | **yes** |
| bot-arena | 2 | 100% named / 50% code | yes | yes | — |
| fao-codec | 3 | 33% | yes | yes | — |
| rusty-board | 8 | 50% | yes | yes | **yes** |
| snapclone | 51 | 86% named / 24% code | yes | yes | **yes** |
| voiceterm | 22 | 91% named / 73% code | yes | yes | **yes** |
| krapper | 1 | 100% | yes | yes | — |
| settings | — | lib | no | **now yes** | n/a |

**Every port compiles.** That was worth establishing before anything else,
because "some don't build" turned out to be false — all seven plus `studio`
check clean. What was actually broken was narrower and more interesting.

## Two real defects found and fixed

**`settings-egui` was missing from the manifest entirely**, so it could never
appear in gitmind's catalogue — the "doesn't show up" symptom. It is a widget
library embedded by other frontends, with no `[[bin]]` and no `app` feature, so
it is catalogued as `role = "lib"` rather than given a binary it does not need.

**gitmind ignored `role` altogether**, which meant it would have offered that
library as a buildable application: building it produces nothing runnable and
leaves the user wondering why. The catalogue now parses `role` and
`is_runnable()` distinguishes an app from a library. An entry with no role
stated is assumed runnable, which is the historical behaviour and safe, since
most entries are apps.

## Reading the numbers honestly

Coverage is name-based: it asks whether the port *mentions* a surface, which is
evidence it was addressed, not proof it was faithfully ported. It over-counts
where a name is coincidental and under-counts where a port renamed something.
Treat it as a worklist, not a score — the value is the **next surfaces** list at
the bottom of the run, which is concrete.

Two detector bugs were fixed on the way, both worth noting because they made the
first run's numbers untrustworthy: restricting Rust scanning to `src/ui/` found
**zero** surfaces in bot-arena and fao-codec (they keep views elsewhere) and
reported 0%, which looks identical to a port with nothing done; and voiceterm's
HTML test fixtures were being counted as UI surfaces, so its "missing" list was
nonsense. A detector that silently sees nothing is worse than one that
over-counts.

## Where the work is

`snapclone` at 43% of 51 has the largest untouched surface area of any real Qt
app. `voiceterm` is at 50% of 22 — this file previously said 34% of 32 and
"still has no text input", and BOTH halves were wrong. Eleven of those 32
"surfaces" were golden-output fixtures named `article.html.qml` under
`tests/golden/expected/`, and the input path existed the whole time in
`uikit_term::keys`. What it actually lacked was mode-awareness; that is now
fixed in the shared kit rather than in voiceterm, since every egui app
embedding the terminal had the same gap. `bo-suite`, which was the biggest
gap at 2%, is at 51% after the transport / spaces / mixer / soundscape /
settings / safety / scroll-feel passes and the browse pages (albums and artists,
with their detail drill-downs). Each has a concrete next-surface list at the
bottom of an audit run.

`studio` (the kdenlive/Audacity work) is not in this table because it is not a
*port* of an existing frontend — it is a new application with seven crates and
164 tests, and its own README tracks what remains. Measuring it against a
surface list would be measuring the wrong thing.

## What the audit does NOT measure, learned the hard way

The name-based audit says a surface is "covered" if the port mentions it. That is
evidence it was addressed and nothing more, and a review of the actual ports
found gaps the audit could not see:

* **studio had no binary at all** — seven crates, 164 tests, and no `main.rs`
  anywhere. Every crate's tests passed, so nothing looked wrong from inside. A
  pile of well-tested libraries is not an application, and the audit checked for
  a `[[bin]]` in `egui-frontend/` while studio's crates live elsewhere. **Fixed**:
  `crates/studio-app` is the shell, `cargo build --release` produces `studio`.
* **snapclone's port is a nav bar.** `src/` is `lib.rs` and `unified_nav.rs` —
  no camera preview, no message list, no composer. The audit scored it 13%
  because names appear in comments and docs, which is exactly the over-count the
  method admits to.
* **voiceterm has no text input.** Menus work; typing does not.

So the audit is a floor, not a measure: a port scoring 13% is *at most* 13% done,
and may be far less. The next revision should check for **rendered surfaces** —
a function that takes a `&mut Ui` and is called from the app's draw path — rather
than a name appearing anywhere in the tree. That distinguishes a widget that
exists from a widget that is mentioned.

## web2qml is not a port, it is a rewrite

voiceterm's web view is a native browser: `qml_gen.rs` (5880 lines),
`web_islands.rs` (553), `ad_filter.rs` (404), `web_interact.rs` (334). It does
not build a document and render it — it walks the scraper DOM and **writes QML
source text**, which Qt parses, retains in a scene graph, and lays out. There is
no intermediate representation, because QML *was* the intermediate
representation.

That is why this one cannot be translated the way the widget surfaces were. Two
things Qt was silently carrying have to become explicit code:

* **Identity.** `objectName: "_field_0"` names a widget that persists between
  frames, and `submit_form_get` reads it back later. Immediate mode has no
  widget to name; if what the user typed is not held outside the frame it does
  not exist, and a form whose input clears on every repaint cannot be used at
  all.
* **Culling.** `opacity: (y + height >= _vpTop && y <= _vpBot)` is Qt
  virtualising a long page declaratively. In egui that is hand-rolled, or a long
  article pays a full layout pass every frame forever.

`uikit-web` had `RichDoc` — a flat list of styled blocks, right for command
output, docs and HTML mail, and structurally unable to express a table, a form
or an image. `uikit_web::doc` is the other half: a node tree with document-order
`NodeId`s, `FormState` living outside the frame, and `visible_rows` doing by
hand what the opacity binding did declaratively.

Still to port, named rather than left as a TODO in a file nobody opens: ad and
tracker filtering, island placeholders for canvas/video/iframe (the node kind
exists, the placeholder UI does not), hydration extraction (JSON-LD,
`__NEXT_DATA__`), SPA and challenge detection, and a real box-model layout pass.

## Blocked: sharing manpages.rs needs a Qt host

`manpages.rs` is 4,104 lines and splits cleanly in principle — `get`,
`list_names` and `to_html` are Qt-free, while `to_qml` pulls in `qml_gen` (5,880
lines) and `scraper`. A module cannot be half-included with `#[path]`, so
sharing it needs the Qt-free half lifted into its own file in the app crate.

That refactor is NOT done, and deliberately: **the app crate cannot be built in
this container.** `cargo check --bin web2qml` fails in the build script, which
shells out to `qmake`, and there is no Qt here. Refactoring a 4,000-line module
that cannot be compiled is a guess, not a refactor.

The egui reader (`manpage_view.rs`) therefore takes documents from its host
rather than fetching them — no registry duplicated, no converter duplicated. The
split is the next real step for whoever has a Qt host, and it would let the
frontend read the registry directly.

## What is actually left

The named-surface work is close to done, and the remainder is mostly not
product code. Recorded so the next session does not re-derive it:

* **rusty-board** reports 50%, but `MicrotermView` is already ported as
  `microterm.rs` — the audit only misses the literal name. Its other three are
  `Dbus` (an interface, not a surface) and two test harnesses
  (`KeyboardBehaviorsTests`, `RustyBoardTestHarness`).
* **voiceterm**'s `WorkspacesPage` is a phantom: nothing by that name exists in
  the tree. It comes from the audit's filename rule. `MessageView` is covered by
  `mail_view.rs`. `Manpages` is blocked on a Qt host — see above.
* **snapclone**'s remainder includes `ComponentTestWindow` and
  `ModalLayerTestPanel`, which are Qt-side harnesses rather than product.
* **fao-codec**'s `V2Encoder` and `V8GpuRasterizer` are codec/render backends,
  not UI.

So the honest reading of the table is that the ports are further along than the
percentages suggest, and what is left divides into: blocked (manpages), not-UI
(Dbus, codecs, harnesses), and phantom (WorkspacesPage).

## Text input: what works, and the one thing that does not

Audited after a report that voiceterm "was not taking input". The answer is
partly yes and partly no, and the no is the interesting half.

**Terminal (voiceterm).** Typing, Backspace, Enter and paste all reach
`uikit_term::App::handle_input`, which the main binary calls each frame; split
panes work because the focused pane's `App` is SWAPPED into the drawn one, so
there is only ever one input path. `terminal_standalone` calls it too.

But control keys are a different story. Arrows, Ctrl-C, Tab and the rest are
encoded correctly into `App::pending_bytes` — and **nothing in the tree drains
them**. `take_pending_bytes()` has no caller. The bytes are computed every frame
and thrown away, which is exactly why plain typing works while Ctrl-C appears
dead. The demo has no PTY to write them to; a host that grows one must call
`take_pending_bytes` after `handle_input`, and that is the single remaining
wire.

Two links in that chain were also broken and are now fixed: `handle_input` was
calling the mode-UNAWARE `keys::encode`, so the application-cursor branch in
`keys.rs` and the DECCKM tracking in the parser — landed in separate sessions,
each tested in isolation — never met. Paste now also emits bracketed bytes when
the program asked for DEC 2004.

**Search and text fields elsewhere.** All present and wired: snapclone's contact
filter, add-peer host/port, landmark name, QR manual entry, login; bo-suite's
tag editor and playlist create; voiceterm's manual-page filter and table command
line; fao-codec's paths; krapper's lyrics box.

**bo-suite's playlist search did not exist** and now does — matched against name
AND description, because "long roads" is how someone thinks of the playlist they
called "Drive". A filter that hides everything says so distinctly from an empty
collection, or the user thinks their playlists vanished.

**eguikit has no shared text-field widget**, and after this audit that looks
right rather than like a gap: every field above is a bare `egui::TextEdit` with
a hint and a width. There is no repeated behaviour to promote — no debounce, no
validation, no shared focus rule. If one appears, this is the place to check
before writing it twice.

## Context menus, arrow keys, clipboard — and the one I did not build

A second input audit, prompted by "there is no right-click action menu or tap
and hold anywhere across egui". That was exactly right: **zero** matches for
`secondary_clicked` or `context_menu` in any app or kit, and no arrow-key list
navigation outside the terminal. Every action had to be a visible button, which
is why several pages grew rows of them.

`uikit_egui::menu` now carries all three:

* **One menu, two ways in.** Right-click on a desktop, press-and-hold on a
  phone. Those are the same intent on different hardware, and treating them as
  two features grows two menus that drift — one gets a new action and the other
  does not. `invoked()` asks "was I invoked?", not "was I right-clicked?".
* **Long-press has a rule right-click does not: movement cancels it.** A finger
  that drifts past 12px is scrolling, and a menu opening under a scroll makes a
  list feel like it is grabbing at you. snapclone's snap tiles had the only
  tuned hold in the fleet (450ms) and no slop at all; the kit took the timing
  from them and gave the slop back.
* **Keyboard.** Up/Down move, Enter chooses, Escape dismisses. Disabled entries
  and separators are SKIPPED, because landing on something unpickable reads as
  the keyboard being broken. Nothing is selected on open — a highlighted first
  entry invites Enter, and the first entry is often the destructive one.
* **Clipboard.** `copy_to_clipboard`, because nothing in the fleet could:
  voiceterm had a `TermAction::Copy` that named the intent and never performed
  it.

Wired to snapclone's snap tiles rather than left as a kit feature nobody calls —
the mistake this programme has now made three times. Entries are derived from
the snap's state: "Save" is disabled on an expired snap, because the bytes are
gone and offering it would fail after the tap.

### Not built: phone-to-desktop field entry for TV mode

Asked for and NOT attempted, deliberately. It is not a widget — it needs a
transport between two devices, a pairing handshake, a focus protocol so the
phone knows which field it is filling, and an echo so the TV shows the text as
it is typed. snapclone already has DHT peering and voiceterm has a cross-app
action protocol, so the pieces exist, but wiring them is a feature with a
security surface (anyone on the network typing into your TV) rather than a
refactor. Naming it here rather than half-building it.

## Sensors: what exists, what is ported, what is not

Asked about gyroscope integration. The ports had none; the ORIGINALS have two
independent sensor users, and neither had been carried across.

**snapclone's posture subsystem — now ported.** Proximity from
`iio-sensor-proxy` over the system bus, motion from raw IIO accelerometer counts
in sysfs. Two uses: a screensaver that dims when the phone is at your ear, and a
flip from push-to-talk to gated talk mode when it gets there. `PostureEngine`,
`PostureSource` and `DimOverlay` are in `posture.rs`; the D-Bus and sysfs source
stays with the host, as it must.

The Qt header is blunt that it is **unverified on hardware** — whether the FP5
exposes proximity through iio-sensor-proxy, the exact IIO device path and the
motion threshold all need on-device validation. What makes it shippable anyway
is the degradation contract, ported exactly: **no sensor means far and still**,
so the app behaves as though it had none. That inverts the usual default and is
the right way round — a screensaver that dimmed when unsure would black out a
phone with no proximity sensor at all.

**`ambient-learner` was partly ported after all** — see the correction below.
The original note read:

> **`ambient-learner` has NOT been ported at all.** It is a whole app —
`iio_proxy.rs`, `environment.rs`, `world_state.rs`, an animated background
driven by real sensor readings — and it has no egui frontend and no row in the
parity table. It is the largest genuinely unported thing found so far, and it is
invisible to the audit because the audit only walks apps that already have a
port directory.

**There is still no gyroscope proper.** What exists is proximity plus an
accelerometer-delta "moving / still" bit. Nothing reads rotation rate, and no
surface would currently use one: the wayfinder takes its bearing from GPS
positions rather than from a compass, and the dial reads touch, not tilt. A real
gyroscope integration would be a new feature rather than a port — worth saying
plainly, because "integrate the gyroscope" sounds like wiring up something that
is already half there, and it is not.

## Glove mode: built, and the three things it needs from outside

`uikit_egui::glove` is a tilt-rolled cursor for cold weather, when gloves make
capacitive glass useless and the real alternative is not using the phone at all.
The physics, snapping, vocabulary and gate are here and pure. Three inputs are
seams, each because a daemon already owns it:

1. **Temperature** — `ambient-learner`'s `EnvSnapshot::aux_temp_c`, a genuine
   ambient reading rather than a CPU thermal zone. Unknown temperature is NOT
   cold, deliberately: a phone with no ambient sensor would otherwise roll a
   ball across the screen indoors in July.
2. **Orientation** — `AccelerometerOrientation`, the descriptor strings, and
   nothing else. **An earlier version of this section said the daemon could not
   drive the cursor because it exposes no vector. That was wrong**, and it was
   wrong because I had assumed the wrong motion model. A RATCHET needs only
   which way the phone turned and when it came back, and "left-up" / "normal"
   carries both. No sysfs reader is required after all.
3. **Speech** — the recogniser. `Vocab` maps a transcript to an action; it does
   not listen.

**`ballgame.py` is not in this repository** — the only Python is in
`shared/webhost-kit` and `tools/` — so the mechanic comes from the description
of it, and the description corrected my first attempt. That attempt rolled a
ball under continuous acceleration. The real thing ratchets: each shake moves a
fixed step, returning to face-up or face-down does nothing but re-arm, and you
traverse by repeating.

Ratcheting is better here for a reason worth writing down: **it can be gated on
flick timing.** The step fires on the RETURN to rest and only if the phone was
away for under 700ms. On a bus or while walking, the orientation descriptor
drifts through exactly the values a flick passes through — but slowly, and it
stays there. A lean is not a flick, and a lean that is held is not a flick that
was slow. Continuous physics has no equivalent defence: ambient sway is
indistinguishable from gentle steering.

Firing on the return costs one return-stroke of latency, taken on purpose. A
step that fires on the way out cannot be un-fired when the phone turns out to
have been leaning, and an accidental step is far worse than a late one for
someone in gloves who cannot easily correct it.

### What was built

* Snapping is generous (72px) and the ball latches to the nearest target rather
  than requiring containment. A tilt-driven cursor is least precise exactly when
  it is needed, because a cold hand shakes.
* **Selecting a field arms VOICE, never the keyboard.** rusty-board raising
  itself would cover the screen with keys the gloved hand cannot press — the
  problem restated. `ArmField` is the whole signal, and its cue to rusty-board
  is negative: no key event is ever synthesised.
* A vocabulary that is small and literal, because the user is speaking through a
  scarf. Sliders take coarse and fine nudges ("louder", "a bit less", "mute")
  because that is how people talk about volume. Words that mean nothing to a
  kind are REFUSED rather than guessed — acting on a misheard word is worse than
  doing nothing when you cannot easily undo.
* "next" and "back" step the snap through a list, and move the ball with it, so
  the highlight is never somewhere the ball is not.

### Not built

The host wiring: nothing yet reads `aux_temp_c` into `glove.ambient_c`, polls a
tilt vector, or feeds transcripts in. snapclone holds the cursor and applies its
actions, but the three seams are unconnected — this is a working engine behind
an unconnected socket, and saying so is the point of this section.

## Reading the accelerometer in Rust: not kernel work

Answered by reading iio-sensor-proxy's own source rather than guessing.

**It is userspace.** `drv-iio-poll-accel.c` does exactly three things: read
`in_accel_{x,y,z}_raw` from a sysfs directory and multiply by a scale. Plain
file I/O against an interface the kernel's IIO core already exports — no module,
no ioctl, no privilege. `iio_accel.rs` is that, in Rust.

**Sharing depends on which driver the daemon is using, and that is knowable.**

* `drv-iio-poll-accel` — sysfs on a timer, nothing claimed. A second reader
  coexists; both processes read the same files.
* `drv-iio-buffer-accel` — opens `/dev/iio:deviceN` and ENABLES THE RING BUFFER.
  While that is enabled the IIO core refuses `*_raw` sysfs reads with `EBUSY`.

So `EBUSY` is not a transient to retry through — it is the daemon saying it owns
the device in buffered mode. `Reading::Contended` names that case so a caller
falls back to the D-Bus strings instead of spinning.

**Passing data INTO iio-sensor-proxy is not possible**, and it is better to say
so than leave it open. The daemon is a reader of kernel devices that exports
over D-Bus: no ingest path, no socket, no shared buffer, drivers compiled in and
discovered through udev. Handing it a vector would mean writing a new driver
into the daemon (C, upstream) or presenting a fake IIO device to the kernel —
which IS the kernel-adjacent work the question was trying to avoid.

The direction that works is the opposite one, and it is what this takes: read
the same sysfs the daemon reads, and **classify identically**. `orientation.c`'s
thresholds — 35° portrait, 35° landscape, 70° face-down, 20° tilt, and the
`SAME_AXIS_LIMIT` hysteresis — are copied rather than invented, because a Rust
reader with its own numbers would make the glove ratchet fire at different
angles depending on which backend happened to be available. That is the worst
kind of bug: it works on the developer's phone.

The hysteresis is the part that matters most for gestures. Two branches return
the previous orientation unchanged — corner-up, and flat — and dropping them
would let the reading flap at a border, which the ratchet would see as spurious
flicks from a hand that is merely holding the phone at an angle. A contended or
absent read also keeps the last classification rather than resetting to
Undefined, since Undefined would re-arm the ratchet.

Still not wired: nothing polls this yet. It is a reader with no caller, which is
the same honest gap as the temperature seam.

## Correction: ambient-learner was partly ported

An earlier section here said ambient-learner "has NOT been ported at all". That
was wrong, and grepping before writing caught it: `src/ui/egui/mod.rs` already
holds a 214-line port of the **Live page**, with eased temperature, an animated
confidence tint and a weather-share row. What is missing is the rest —
`HistoryPage`, `LocationsPage`, `MapPage`, `ModelPage`, `SyncPage`, and the
small pieces.

It is still invisible to the parity audit, which only walks apps with an
`egui-frontend` directory. ambient-learner puts its port at `src/ui/egui`
instead, so the tool has never seen it. That is a tooling gap rather than a
porting one, and it is the reason the app looked untouched.

**`PressureGauge` is now ported** into that module and drawn on the Live page.
The interesting rule is that an unknown trend is not a steady one:
`environment::pressure_trend_hpa` returns `None` until it has four samples over
five minutes, because a barometer wobbles and a short window extrapolates
nonsense. Rendering that as "steady" would claim settled weather right before a
storm, so `Trend::Unknown` is its own state and reads "collecting readings".

The needle saturates outside 980–1040 hPa rather than rescaling the dial — an
extreme value means "weather event" whatever the number, and a dial that
stretched to fit one outlier would flatten every ordinary day into the middle.
Saturation shows in the needle's colour, because a needle pinned at the edge
otherwise looks like a stuck one. `LiveSnapshot::pressure` is an `Option`, and
`None` draws nothing: a dial at its minimum is indistinguishable from a real
low-pressure reading, which is the worst possible way to say "no barometer".

**Building it needs `--no-default-features --features egui`.** The default
feature is `gtk`, which pulls pango and libadwaita and does not build in this
container. Same shape as logscope's lean build: the flag exists, and nothing was
exercising it.

## ambient-learner joins the table, and the table was the problem

I reported this app as "not ported at all". That was wrong in one direction and
right in another, and both halves matter.

**Wrong:** it has an egui port. `src/ui/egui/` holds the pressure gauge and now
the live page, behind an `egui` feature, and 34 tests pass.

**Right:** the audit could not see it, so it had no row — and an app with no row
reads as an app with nothing to do rather than an app nobody has measured. Two
separate causes, both fixed:

1. It was not in `PORTS` at all.
2. `port_text` walked only `egui-frontend/`. This app's port is a FEATURE of the
   existing crate rather than a separate one, so it lives inline. The discovery
   now accepts either shape.

With both fixed it lands at **39% named / 11% code across 18 surfaces**, and it
is the only app in the table with **no egui binary and no app-def** — it cannot
be launched as an egui app at all yet. That is a more useful thing to know than
the "0%" I would have guessed.

Its GTK dependencies also do not build in this container (`pango-sys` fails), so
everything here is `--no-default-features --features egui`. That is the same
lean-build shape as logscope and worth remembering before believing a build
error from this app.

## ambient-learner now has a row, and it says something uncomfortable

The audit reports it at **18 surfaces, 44% named, 11% code — and `no` for
binary, `no` for app-definition, `NO` for manifest.** Those three columns are
the real finding. Every other port in the table produces a runnable binary and
is registered; this one is a library module that nothing launches.

That is a different kind of incomplete from a missing page. `HistoryPage` and
`PressureGauge` being ported does not mean anyone can run the egui
ambient-learner, because there is no egui binary target and no manifest entry.
Worth fixing before porting more pages: three more surfaces behind an
unrunnable front end is less useful than one behind a runnable one.

**`HistoryPage` is ported.** The rules worth naming:

* **A flat series must not render as a storm.** Indoors with the heating on,
  every reading can be identical to a tenth of a degree, and a chart that scaled
  to its own range would fill itself with sensor quantisation. A minimum span of
  0.5°C is enforced and a genuinely flat series draws through the middle.
* **Fewer than two points is not a chart** — the QML refuses to draw rather than
  plotting a dot, because one mark invites the eye to read a trend that is not
  there. The empty states say which case it is, so a user can tell waiting from
  broken.
* **The point count is shown beside the statistics.** "Min 18.2, Max 24.6" reads
  as a fact about the week; over four samples it is a fact about four moments.
* Normalised `y` is 0 at the BOTTOM and the caller flips once. Every call site
  remembering to invert is how half a chart ends up upside down.

## Reported defects, and what I could confirm

A list arrived of things wrong with the ports. Recording what is verified, what
is reproduced, and what I have not touched — so none of it depends on my memory.

### 1. Input fields accept one character — DID NOT REPRODUCE (corrected)

Three probes, in order:

**An earlier version of this section said the bug was reproduced and lived in
the shell. That was wrong — twice over — and the corrections are the useful
part.**

| level | result |
|---|---|
| bare `TextEdit`, synthetic keystrokes | accepts every character |
| `qr::qr_ui`, a real ported page, focused by Tab | accepts every character |
| `Snapclone::ui`, the whole shell, Tab-navigated | **accepts every character** |

Two of my own probes invented the bug before the third measured it:

1. Focusing `ui.next_auto_id()` captured *before* the page call targets the
   wrong widget — several widgets precede the field — and reported ZERO
   characters for a page that works perfectly.
2. Watching only `add_form.host` reported zero again, because that field is not
   focusable in the default state. The text had been landing in `picker.query`
   the whole time. The fifth Tab reaches it and all three characters arrive.

I also named `GestureLayer::update` as the prime suspect. It only READS input —
`ui.input(...)` — and never allocates a widget or interacts, so it cannot take
focus. That was a guess stated with more confidence than it had earned.

`typing_through_the_shell_keeps_every_character` is now a PASSING regression
test rather than an ignored reproduction.

**This does not mean the report is wrong.** It means the fault is not reachable
from a synthetic-event harness. What this cannot see is the windowing layer:
real keystrokes arrive through winit and may come as `Event::Ime` composition
rather than `Event::Text`, and an on-screen keyboard adds a third path. That
needs a running binary on a device, not a test.

### 2. Not yet addressed, and named rather than implied

* **Daemons cannot be built in gitmind.** Untouched.
* **The app list is incomplete and does not mark egui ports as primary.**
  FIXED. Only 2 of 10 egui entries carried `primary = true` (voiceterm-egui,
  rusty-board-egui); all 11 do now, including `gitmind-egui`, whose own notes
  already described it as "the egui-primary port" while the flag was simply
  missing. `ambient-learner-egui` is registered, and its row now reads
  `yes / yes / yes / yes`.

  One flag carries a caveat rather than a claim: `snapclone-egui` is marked
  primary with the note that camera and call wiring are still absent. Marking it
  primary is what was asked for; pretending it is complete would not be.
* **voiceterm has no text input to the terminal.** Partly diagnosed already:
  control keys are encoded into `App::pending_bytes` and **nothing drains
  them** — `take_pending_bytes()` has no caller. That is a wire, not a rewrite.
* **The Qt functionality is missing from the egui voiceterm.** Broad; the parity
  table's `code` column (55%) is the honest measure of it.
* **snapclone egui needs camera and voice calls to actually work.** The views
  are ported — viewfinder, ring, voice bar — but nothing is connected to a
  capture device or a call session. This is the same class as the glove-mode
  seams: engines and views exist, the host wiring does not.

## ambient-learner is runnable, and why it needed a separate crate

The parity row now reads `yes / yes / yes / yes`. Getting there was not just
adding a `[[bin]]`.

The first attempt put an eframe binary in the parent crate and it **would not
resolve**: iced 0.12 caps `web-sys` at 0.3.67 and eframe 0.34 needs >= 0.3.77.
Cargo resolves optional dependencies whether or not their feature is enabled, so
`--features egui-app` failed with iced *switched off*. The parent's own comment
says the two "cannot be enabled together" — the truth is stronger: they cannot
coexist in one manifest at all.

So the frontend is its own crate at `egui-frontend/`, matching every other port
in the fleet and giving the audit the directory it looks for. **The view code is
not copied**: one `#[path]` include pulls in `src/egui_lib.rs`, and because a
path-included module resolves ITS nested modules relative to its own directory,
the `pub mod ui { pub mod egui; }` inside finds the page tree without further
help. One include, one copy of the pages.

The binary sits behind a default-on `app` feature so `--no-default-features`
builds a headless library. That matters more here than elsewhere: eframe pulls
glutin and winit, and this container cannot even LINK them, so without the split
the pages would be untestable in CI.

## Gaze: where it fits, and what it should NOT be

`gaze-core` is a layered headless engine — geometry / pipeline / roi / tracker /
engine / sink / synth — that emits a **unit gaze vector and a confidence** per
frame, owning no camera and no thread. It deliberately does not emit a screen
coordinate, because that depends on where the screen is relative to the eye,
which is a per-session calibration rather than a property of the tracker.

**It must not become a second cursor.** Glove mode already has targets, a snap
radius, a spoken vocabulary and a confirmation type that refuses "yes". A
parallel gaze cursor would duplicate all of it and the two would drift — the
exact failure this programme keeps finding in the Qt code. So gaze produces a
POINT and `glove`'s target machinery does the rest, using the same
`SNAP_RADIUS_PX` so the two pointing methods cannot disagree about what "on a
button" means.

### The three things between a gaze vector and a usable pointer

1. **Saccades.** The eye jumps several times a second and never truly stops
   between jumps. This is structure to recognise, not jitter to smooth —
   smoothing a saccade gives a cursor that lags then overshoots. Dispersion
   detection (I-DT) reports a point only while the samples stay inside a box.
2. **Blinks and glare.** A blink is the ABSENCE of a measurement, reported as
   low confidence. Passing it through as a position throws the cursor across the
   screen every few seconds, so low confidence FREEZES the pointer and stops the
   dwell — you cannot dwell with your eyes shut.
3. **The Midas touch.** If looking activates, you cannot look. There is no
   clever fix, only an explicit one: dwell activation is ARMED separately, and
   while disarmed the pointer still tracks and highlights so the user can see
   where the system thinks they are looking without pressing anything.

### The rule that decides whether it works

**Dwell belongs to the TARGET, not the pixel.** Human fixation drifts by about a
degree even when "still", so a timer keyed to a screen position restarts
constantly and never completes. Keyed to the snapped target, the drift is
irrelevant — the timer runs as long as the eye stays on the same button.

Calibration is an affine fit rather than anything cleverer: with five to nine
points a projective model overfits the noise and behaves worse away from them.
It **reports its residual** and refuses to point above half a snap radius, and
it returns `None` for collinear points rather than a near-singular solve, whose
enormous coefficients look like a working calibration until the eye moves. An
uncalibrated pointer is deliberately useless rather than plausible: one that
lands plausibly-but-wrongly makes the user try to compensate and blame
themselves.

### Not built

The bridge from `gaze-core` to this layer. gaze-core needs OpenCV, so it is a
host-side poller feeding `GazeSample`s in — the same seam shape as the tilt and
temperature readers, and honest for the same reason.

## Where gaze would actually get used

The dwell pointer is the RARE use. Dwell-to-click is slower than touch, tiring
to sustain, and wrong often enough that people abandon it after the demo. It
earns its place for hands-free operation and nothing else.

The distinction that decides everything is **pointing versus attention**:

| | pointing | attention |
|---|---|---|
| calibration | per session | **none** |
| accuracy needed | ~1° | "a face oriented at the screen" |
| frame rate | 30–60fps | 2–3fps |
| cost of being wrong | activates the wrong thing | a few seconds of backlight |

Attention is cheap in every dimension pointing is expensive, and that is what
makes it deployable on a phone rather than a demo.

### Ranked by how often it would actually fire

1. **Screen dimming (BUILT).** `PostureEngine` dims and blanks from proximity
   and stillness, which cannot tell reading from absence — sitting still with a
   long message open looks exactly like an unused phone, so the screensaver
   fires while you are reading it. Every phone has this bug and everyone works
   around it by jabbing the screen. Attention is a VETO here, never a trigger:
   the errors are not symmetric, and a false "not looking" blanks the screen in
   someone's face.

2. **Which app is being addressed.** `voiced` routes intents by focus scope and
   takes focus from the compositor. With several windows visible, gaze answers
   "which one did you just talk to" better than which window was last clicked —
   and being wrong is cheap, because the compositor's answer is still there as a
   fallback.

3. **Which pane a voice command means.** voiceterm has split panes and a spoken
   vocabulary. "Clear" is ambiguous across four panes; the pane being looked at
   is the referent. No dwell, no calibration precision needed — pane-sized
   targets are enormous — and the user sees the highlight before speaking.

4. **Notification placement.** Put the banner where the user is not looking. No
   interaction at all, and a wrong guess costs nothing.

5. **"Seen" that means seen.** A snap marked viewed when it was actually looked
   at rather than when it rendered. Correct, but privacy-loaded enough that it
   should be opt-in and is not worth leading with.

### The constraint that shapes all of it

A selfie camera at 30fps to keep a screen awake would cost more battery than the
screen. Attention needs 2–3fps, no calibration, and tolerates a blurry
half-turned face — so it is the only variant that can run continuously. Any
design that needs the pointing pipeline running all day is not deployable, which
is the real argument for treating the two halves as separate features rather
than one feature with a quality setting.

## ambient-learner is done bar an artifact

94% named, 56% code, and the one remaining "surface" is `AmbientLearnerApp` —
the audit's CamelCase of the app itself, not a thing that can be ported. Every
real page is across: Live, History, Places, Map, Model, Sync, the debug
screen-temperature viewer, the ambient layer and the section chrome.

It arrived at this from an odd starting point. Three sessions ago it was
recorded here as "NOT ported at all", which was wrong — a Live page already
existed. Then it had no binary, no manifest entry and no audit row, so the pages
that did exist could not be run or counted. Both of those were fixed before any
more porting, and the order mattered: the run column is what turned "some
modules compile" into "an app you can open".

## I have been misreading the `code` column

Worth recording, because it changed what I thought the remaining work was.

Chasing bo-suite's low `code` percentage, I went to port `SafetyToast` — and
found `safety.rs` already there, 229 lines covering the bands, the eight-second
toast, the clamp, and unlock-with-reapply. Writing it again would have been the
"two lists of the same thing" failure this programme keeps finding in the Qt
code, committed by me, for the fourth time. **Grepping before writing is what
stopped it.**

The same check on the rest of bo-suite's "named but not in code" list found
`BandSlider` in `device.rs`, `BoRebound` in `scrollfeel.rs`,
`AddToPlaylistDialog` and `ScanPickerDialog` in `dialogs.rs` — all ported, all
under Rust names.

**The tool is not wrong; its docstring says exactly this.** `covered_in_code`
records that code-only matching drops bo-suite from 51% to 11% "because the
house style names the Qt class in the MODULE DOC and then uses idiomatic Rust
names in the code", which is why both metrics are shown. I have been quoting the
lower number as though it were a coverage figure. It is a *naming-convention*
figure, and a port that renames correctly scores badly on it by design.

A third reading, counting surfaces whose snake_case stem appears in port code:

| app | surfaces | named | code | named-only | of those, present under a Rust name |
|---|---:|---:|---:|---:|---:|
| bo-suite | 47 | 44 | 17 | 27 | 14 |
| snapclone | 51 | 44 | 12 | 32 | 6 |
| voiceterm | 22 | 20 | 16 | 4 | 1 |

So bo-suite's real coverage is around 31 of 47 rather than 17, and the honest
summary is that `named` overstates and `code` understates, with the truth
between them and closer to `named` for the apps whose house style puts the Qt
name in a module doc.

That does not make `code` useless — it caught WayfinderPage and GroupVoiceBar,
which is why it exists. It makes it a *lower bound* rather than a score, and the
gap between the columns is a work queue to check, not a work queue to build.

## Correction: snapclone's named-only list is now current, and it is empty

The table above says snapclone had 32 named-only surfaces with only 6 confirmed
under a Rust name — as of this session that is stale on both counts. A fresh
`parity_audit.py` run puts snapclone at 49 surfaces / 25 in code (the surface
count moved as the source moved), and re-deriving the named-only set directly
from `surfaces()` / `covered()` / `covered_in_code()` gives **24** names, not 32.

Checked each of the 24 by hand — not by filename pattern-matching, by opening
the file and reading the actual `pub struct`/`pub fn` — and all 24 are real,
tested, ported code under an idiomatic Rust name:

| Qt/QML name | lives in | as |
|---|---|---|
| ContactCard, ContactPickerDialog, ContactEditorDialog, ConversationSecurityBar | `contacts.rs` | `Contact`, `Picker`, `SecurityBar` |
| ExportReminderCard | `export_reminder.rs` | `ReminderState` |
| FocusReticle | `reticle.rs` | `Reticle` |
| FriendListWidget | `friends.rs` | `Roster` |
| GlassIconButton | `glass.rs` | glass-effect primitives (`Img`, `downscale`, `box_blur`) |
| GlassScrim | `modal.rs` | `ModalSurface` / `scrims_ui` |
| GroupVoiceBar | `voicebar.rs` | `VoiceBar` / `voicebar_ui` |
| GroupVoiceRoster | `roster.rs` | `Roster` (voice-call variant, distinct from friends') |
| CarouselAwareScrollArea, HorizontalScrollArea | `scroll_areas.rs` | `Wheel`, `HScroll` |
| MediaSendSummary | `send_summary.rs` | `SendSummary` |
| PeerPunchAddrView | `punch_addrs.rs` | `PunchAddrs` |
| SegmentedTabStrip, PagePeelHint | `nav_chrome.rs` | `TabStrip`/`strip_ui` (moved to `uikit_egui::segmented_tabs`), `PeelHint` |
| SnapComposerDialog, SnapViewerDialog | `composer.rs` | `Composer`, `Snap` |
| SnapTile | `tile_strip.rs` | `tile_strip_ui` over `VisibleTile` |
| SwipeNavigator | `swipe_physics.rs` | `Swipe` |
| UnifiedNavBar | `unified_nav.rs` | `UnifiedNav` |
| VideoSnapViewerDialog | `video_viewer.rs` | `VideoViewer` / `video_viewer_ui` |
| ConversationDetailDialog | `conversation.rs` | `Conversation` |

So the earlier "6 of 32" undercounted for the same reason bo-suite's did: the
detector cannot match a name the port renamed, and nobody had gone back to
check the renamed ones by hand since the source moved. Read snapclone's `code`
column (51%, 25/49) as the same kind of floor as bo-suite's — real coverage is
close to `named` (100%), not to `code`.

## Checked the small named-only lists on the other apps too

Same method as snapclone above — read the actual code, don't trust the name
match — applied to every app with a nonzero named-only list:

* **voiceterm** (`GestureHost`, `SlidePopup`, 2 of 15): both real. QML's
  `GestureHost.qml`/`SlidePopup.qml` is the press→drag→release popup-menu
  pattern for the toolbar's pipe/brace buttons; `action_compass.rs` says so
  directly in its own module doc — "the QML toolbar's GestureHost/SlidePopup
  pattern, egui-native" — built on `uikit_egui::slide_menu` and wired to real
  `App` actions (clear/copy/interrupt/search), with a pure unit-tested
  direction→action map. Ported, not missing.
* **gitmind** (`FileRow`, 1 of 22): confirmed ported.
  `crates/gui-egui/src/status_rows.rs` opens with "the changed-file list from
  `qml/FileRow.qml`, as a model" and defines `Row` (not `FileRow` — the
  false-negative pattern again), with `parse_porcelain`, `counts`, and a full
  test suite (rename handling, malformed-output skipping, quoted paths).
* **logscope** (`BlocTab`, `CargoTab`, `ChatTab`, `CodeTab`, `ModifyTab`,
  `RotationTab`, `SourcesTab`; 7 of 28): confirmed ported, all seven.
  `crates/logscope-egui/src/tabs/{bloc,cargo,chat,code,modify,rotate,sources}.rs`
  map 1:1; `bloc.rs`'s own header says outright "Ports `ui/bloc_tab.rs`", and
  `tabs/mod.rs` explains the shape — non-UI logic (engine, providers, bloc,
  conversation_db) reused as-is, each tab module carrying representative UI
  state so the shell is alive and faithful to the original `src/ui/*`.
* **ambient-learner** (`AnimatedBackground`, `DebugScreenTempsPage`, `EguiLib`,
  `HumiditySparkles`, `PressureGauge`; 5 of 16): all real except one phantom.
  `PressureGauge` is confirmed ported already, in the doc's own "ambient-learner
  joins the table" section above. `AnimatedBackground` and `HumiditySparkles`
  are BOTH in `src/ui/egui/ambience.rs`, whose own header states it outright:
  "`HumiditySparkles` and `AnimatedBackground`, ported" — `Sparkles` and
  `Ambience`/`ambience_ui`, with 13 tests covering the exact contract the doc
  above cares about (clamped humidity, session-stable droplet placement,
  storm-clamped parallax, dark-room dimming rather than blanking). `EguiLib`
  is the audit's CamelCase of the crate name itself, not a portable surface —
  same phantom-name class as `AmbientLearnerApp` noted a few sections up.
  `DebugScreenTempsPage` also confirmed: `screen_temps.rs`'s header says
  "`DebugScreenTempsPage` and the frame it reads", defines `ScreenTempsPage`/
  `screen_temps_ui`, and is tested. So all 4 non-phantom items in this app's
  named-only list are ported; `EguiLib` remains the one phantom name.
* **bo-suite** (14 of 46) is unchanged from an earlier pass and already
  confirmed ported under Rust names in the "misreading the code column"
  section above.

So across every app in the fleet, every named-only surface with a real
predecessor that was checked this session — 24 in snapclone, 2 in voiceterm, 1
in gitmind, 7 in logscope, 4 of 5 in ambient-learner — turned out to be ported,
just under an idiomatic Rust name the string-matching audit can't see. The
`code`/`wired` percentage column is a floor, not a measure, exactly as the
doc's own "misreading the code column" section already concluded for bo-suite;
this extends that conclusion to the rest of the fleet with actual evidence
rather than assuming it generalizes.

**What is still genuinely open** — not a naming artifact, confirmed by reading
the code rather than assumed from the table above — is exactly what the
"Reported defects" section already says: `camera_source.rs` is the *selection*
layer (`choose()` picks a `Device` + `VideoFormat` via `device-kit`, tested,
real) but capture itself — actually opening `/dev/videoN` and pumping frames —
stays host-side by design, since a UI crate that opens a device node cannot be
tested or run in this sandbox. The viewfinder, ring and voice bar views are
ported; nothing feeds them a live frame or a live call yet. That gap is
correctly described already and is not something a sandbox session can close.

## wiring_check.py: ambient-learner has a real dead-code collision, not a false positive

Ran `maintenance/tools/wiring_check.py`, which is stricter than either audit
above — it asks whether a page's actual `*_ui` render function is CALLED, the
class of bug that made snapclone draw a placeholder rectangle for months.

It reported 11 of ambient-learner's renderers as dead. Checked each against
the real running binary (`egui-frontend/src/main.rs`, the crate the parity
audit's own binary/app-def/manifest columns already confirm is the runnable
one) rather than trusting the tool:

**10 of 11 are a false positive**, and a new instance of the by-now-familiar
class: `main.rs` is a separate crate (`ambient-learner-egui`) that imports
`ambient_learner_egui::ui::egui::{ambience, chrome, history, live, locations,
map, model, screen_temps, sync}` and calls every one of `ambience_ui`,
`section_header`, `history_ui`, `live_ui`, `locations_ui`, `map_ui`,
`model_ui`, `screen_temps_ui`, `sync_ui` directly in its `update()` match —
read the file, they're all there. `wiring_check.py` only scans
`src/ui/egui/mod.rs`'s own module tree for callers, so it can't see the
sibling `egui-frontend` crate calling in, same blind spot as `reachability.py`
on gitmind/logscope/av1tuner above.

**The 11th, `router.rs`'s `tab_bar_ui`, is real** — and it's a specific and
useful kind of real: not a missing feature, a **duplicate one**. `router.rs`
was built by an earlier session specifically to fix "ten renderers and no way
to reach any of them" (its own header says so). But `main.rs` independently
grew its own dispatch — a hand-written `Page` enum, its own
`ui.horizontal_wrapped` tab row, its own `match self.page { ... }` — that does
the identical job without ever calling into `router.rs`. Grepped the whole app
tree: `Router::`, `router::`, and `tab_bar_ui` appear NOWHERE outside
`router.rs` itself. Two sessions solved the same missing-dispatch bug in two
different files, and only one ended up wired into the binary. `router.rs`'s 7
tests all pass in isolation and prove nothing about the app, for the same
reason the doc's own "misreading" sections keep finding: a test suite
exercises the module, not its caller.

**A second, more interesting duplicate, found while checking the first:**
`mod.rs` ALSO has its own complete `LiveView` struct with a `ui()` method —
different from, and more capable than, the `LiveState`/`live::live_ui`
combination `main.rs` actually wires up. `LiveView` has a working
weather-sharing row (`WeatherShare`, a contact picker, `pressure::gauge_ui`)
that `mod.rs`'s own module doc advertises as a headline feature: "a 'Share
with…' row that exposes the current reading to snapclone contacts through the
weather-share kit." Grepped: `LiveView` is referenced nowhere outside its own
file. **The weather-sharing feature this app's own top-level doc comment
describes does not exist in the app anyone can run.** `main.rs`'s actual Live
page (`egui_lib.rs`'s `LiveState` + `live.rs`'s sensor panel) has no sharing
UI at all.

**Not fixed this session, and why**: reconciling three competing
implementations of the same page (`LiveState`+`live_ui`, `LiveView`, and the
`Router`/`tab_bar_ui` dispatch layer) is a real design decision — which one is
the intended shape going forward — not a mechanical wire-up, and picking
wrong risks discarding the sharing feature or the sensor panel rather than
merging them. Recording it precisely instead so whoever does the merge knows
exactly what exists and where: `main.rs`'s dispatch is correct and running;
`router.rs` is safe to delete once confirmed nobody else is about to build on
it; `LiveView`'s weather-sharing logic is the one piece of real, tested,
user-visible functionality actually missing from the runnable app, and porting
IT (not rewriting the Live page around it) is the concrete next step.

## wiring_check.py: two more real gaps, in voiceterm

The same tool flagged 2 of voiceterm's 61 modules dead. Checked both against
`bin/voiceterm_egui.rs`, the actual running binary:

**`detect::picker_ui` is real.** `detect.rs`'s own header explains the design:
two detection engines (`entity_extract`, `dumb_summary::detect_progress_bars`)
had no surface at all before this file. Its sibling `progress_ui` IS wired —
called directly in the binary's draw loop — but `picker_ui`, the disambiguation
UI for when a spoken reference matches more than one on-screen entity equally
well ("connect to the headphones" when two Bluetooth devices both plausibly
match), is not. `EntityPicker` has zero references outside `detect.rs` itself.
The progress-bar half of this file's job works; the entity-disambiguation half
does not reach the user.

**`archived_view::bar_ui` is real.** The archive page's main renderer
(`archive_ui`) is wired and working — confirmed in `CentralView::Archive`'s
match arm. But its sibling `bar_ui` — a fan-out bottom action bar, with the
specific rule "a button with no options must not fan out into an empty popup"
documented and presumably tested — is a different surface for the same page
that `archive_ui` does not call and nothing else does either. `BottomBar` has
zero references in the binary.

Both are the same shape as the ambient-learner `LiveView` finding above: a
page got ported with two halves, one wired and one not, rather than one whole
page missing. Recorded as two more concrete next steps rather than guessed at
blind, for the same no-compiler reason as tomescope's orphans in `TODO.md`:

- [ ] voiceterm: wire `detect::picker_ui` into the ambiguous-reference path —
      wherever the engine currently falls back when a spoken entity reference
      doesn't resolve to one match, show the picker instead of failing silent.
- [ ] voiceterm: wire `archived_view::bar_ui` into the Archive view alongside
      `archive_ui` — likely a per-page action row (share/re-fetch/delete),
      matching the "fan out" pattern described in the module doc.
