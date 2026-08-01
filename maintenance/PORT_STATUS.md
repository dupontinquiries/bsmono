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

| port | surfaces | covered | bin | catalogued | primary |
|---|---|---|---|---|---|
| bo-suite | 46 | 2% | yes | yes | — |
| bot-arena | 2 | 50% | yes | yes | — |
| fao-codec | 3 | 33% | yes | yes | — |
| rusty-board | 8 | 50% | yes | yes | **yes** |
| snapclone | 38 | 13% | yes | yes | — |
| voiceterm | 32 | 25% | yes | yes | **yes** |
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

`bo-suite` at 2% of 46 surfaces is the least-started port and the biggest single
gap. `snapclone` at 13% of 38 has the most surfaces of any real Qt app. Both
have concrete next-surface lists.

`studio` (the kdenlive/Audacity work) is not in this table because it is not a
*port* of an existing frontend — it is a new application with seven crates and
164 tests, and its own README tracks what remains. Measuring it against a
surface list would be measuring the wrong thing.
