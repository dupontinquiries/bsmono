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
| bot-arena | 2 | 50% | yes | yes | — |
| fao-codec | 3 | 33% | yes | yes | — |
| rusty-board | 8 | 50% | yes | yes | **yes** |
| snapclone | 51 | 80% named / 18% code | yes | yes | — |
| voiceterm | 22 | 77% named / 64% code | yes | yes | **yes** |
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
