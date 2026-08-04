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
| bo-suite | 47 | 94% named / 34% code | yes | yes | — |
| bot-arena | 2 | 100% named / 50% code | yes | yes | — |
| fao-codec | 3 | 33% | yes | yes | — |
| rusty-board | 8 | 50% | yes | yes | **yes** |
| snapclone | 51 | 82% named / 20% code | yes | yes | — |
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

**`ambient-learner` has NOT been ported at all.** It is a whole app —
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
