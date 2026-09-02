# C14_TODO — session c15unified, 2026-09-02

Next in the file series after `C13_TODO.md`. The numbering is the document's,
not the session's: c14 appended its notes to C13_TODO (§60–92) rather than
starting a file, so this is the first new one since c13. Read C13_TODO first
for anything still open there — in particular §19/§27, which §1 below corrects.

Sections are numbered fresh.

---

## 1. §19/§27 was wrong about the cause: it is not deferred wrap

C13_TODO §19 traced voiceterm's garbled panes to the grid mis-tracking the
cursor through a themed zsh prompt redraw, named the deferred-wrap ("phantom
column") model as the likely defect, and §27 directed the next session to read
konsole's `Screen.cpp` to settle it. No konsole checkout is in the c14unified-025
bundle, so this session built the corpus §27 asked for and ran the existing
parser against it instead.

**The hypothesis does not survive contact.** Eight constructed fixtures put the
cursor at the exact right margin and then do something to it — LF, CR, BS, CUF,
EL, ED, a double-width glyph, and a wrap that scrolls the screen. All eight
agree with tmux, unchanged, against the parser as it was. The eager-wrap model
already in `Grid::put` (park at `cur_col == cols`, wrap on the next write) is
observably equivalent to a pending-wrap flag for every case that could be
constructed.

So `§19` should not be worked as a wrap bug, and §27's konsole reading list is
answering a question that is not the one being asked. What was actually wrong
is a list of missing sequences, in `KNOWN_GAPS` in
`shared/uikit/crates/uikit-term/tests/vt_corpus.rs`, each with what it is for.

This is the third time in this repo's notes that a confidently-stated cause in
a design document turned out to be a hypothesis (CLAUDE.md §5). The note in §19
did say "very likely" — the loss was that §27 then built a reading list on top
of it as though it were settled.

**Marked as: HYPOTHESIS DISPROVED by measurement, 2026-09-02.**

## 2. What closed, and what the corpus still lists

Closed this session (CHA/HPA, VPA, CNL/CPL, ECH, ICH, DCH — commit
`31039857`). CHA is the one that most plausibly explains the field report:
readline redraws an edited line by jumping to an absolute column rather than
counting backspaces, so with it dropped the redraw landed wherever the cursor
was and the line interleaved with itself, which is what §19 describes.

Still listed, roughly in the order I would take them:

1. **SGR 38/48 with `5;n` or `2;r;g;b`.** Not ignored — actively corrupting.
   The sub-parameters fall through to the plain SGR table, so `38;5;33` sets
   foreground 3 and `38;2;255;128;0` ends on a `0` that RESETS the pen. Needs a
   `TermColor::Rgb` variant and `Ansi` widened to mean palette index 0–255;
   `src/snapshot.rs` already has the `rgb:rrggbb` spelling waiting for it.
2. **IL / DL / DECSTBM.** No scrolling region exists at all, so a scroll meant
   for part of the screen moves all of it. Every full-screen program sets one.
3. **DECSC/DECRC (`ESC 7`/`ESC 8`) and SCOSC/SCORC (`CSI s`/`CSI u`).**
4. **RI (`ESC M`).**
5. **The ESC-intermediate leak.** `ESC ( B` prints a `B`, `ESC # 8` prints an
   `8`. Half-implemented rather than absent, which is the worse state.
6. **Double-width filler cell** reads back as a space, so `row_text` returns one
   more column than the screen shows.

## 3. Not done and probably the bigger half of §20: the alternate screen

`CSI ?1049h` / `?1047` / `?47` are unhandled, so there is no alternate screen
buffer. vim, less, htop and anything else full-screen draw over the primary
screen and never restore it. Nothing in the corpus covers this yet because no
fixture spawns such a program.

This is likely a larger part of "voiceterm-egui panes still unusable as a real
terminal" (§20) than everything in §2 combined, and it is its own session: it
wants a second cell buffer, saved cursor/pen across the switch, and a decision
about whether the alternate screen contributes to scrollback (it must not).

## 4. The corpus is bash-only; the reported shell was zsh

`tools/capture_vt.py` records from a real pty, but zsh is not installed in this
sandbox, so the eight recorded scenarios are bash/readline. The original report
was a themed zsh, whose zle emits a different redraw shape (reverse-video
placeholder, run of spaces to the terminal width, `\r`-and-redraw). Capturing
those on a machine with zsh is cheap — add scenarios to `SCENARIOS` and run the
two tools — and is the single highest-value addition to the corpus.

## 5. `lock_check.py` cannot see a lock made stale by a transitive path dep

It reported `stale: 0` on a tree cargo was actively rewriting. It compares a
crate's own `[dependencies]` names against the names in its lock, so when
`uikit-egui` — a path dependency of `uikit-term` — gained `math-kit`,
uikit-term's committed lock went stale in a way the checker cannot express.
Found by tripping over it, which is the same way the original three stale locks
were found (CLAUDE.md §7).

Fix shape: walk `path = ` dependencies transitively from each lockfile's crate
and union their declared names into `want`, rather than reading one manifest.
Cycles are possible in principle and the walk needs a visited set.

Not fixed here; the lock itself was refreshed in `8b483653`.

## 6. `eframe_backend_check` fails on unendlich-dash, and the fix is not obvious

`games/unendlich/crates/unendlich-dash/Cargo.toml` declares
`eframe = { version = "0.34", features = ["glow"] }` with defaults left on.
Confirmed against the published manifest (fetched from static.crates.io):
eframe 0.34's `default` includes `wgpu` and does NOT include `glow`. So the
crate links BOTH backends, one stated and one inherited, which is exactly what
the checker is named for.

**But the source wants both.** `src/main.rs` runs `run(eframe::Renderer::Wgpu)`
and falls back to `run(eframe::Renderer::Glow)`. So this is not over-linking by
accident, and the naive fix — adding `default-features = false` — would leave
`Renderer::Wgpu` with no backend compiled.

Restating the defaults explicitly is the right shape, but it is not a
mechanical edit: eframe's `default` also contains `accesskit`, `default_fonts`,
`wayland`, `x11`, `web_screen_reader` and **`winit/default`**, and that last one
cannot be re-enabled from a dependent crate's feature list. Turning defaults off
would silently drop winit's own defaults with no way to restore them from here.

Pre-existing, not introduced by this session's merges (the file is byte-identical
before and after). Left alone deliberately: `unendlich-dash` is recorded as
TIMEOUT-on-build in C10_TODO §77 and unverified in C11-4c, so there is no way to
test either branch of this in the sandbox, and an unverifiable edit to a feature
set is how a working GUI stops starting.

Worth considering instead: the checker gaining a case for "both backends, chosen
at runtime with a fallback", which is a legitimate pattern it currently has no
way to express.

## 7. Merges taken this session

Four bundles offered, three merged, all with zero file overlap between them and
zero conflicts — four genuinely disjoint work streams.

| bundle | branch | result |
|---|---|---|
| `c14unified-001-rusty-board-v2-flick-swipes` | `inc-rustyboard` | merged, 8 commits |
| `c14unified-009-fao-weighted-pred` | `inc-fao` | merged, 14 commits |
| `unendlich-perf-work--c14unified-022-delta` | `inc-unendlich` | merged, 8 commits |
| `blackstar-decision-to-change-engines--c14unified-010-delta` | `inc-blackstar` | **already contained in main** — nothing to merge |

The blackstar delta's tip `2f97001c` is an ancestor of main, so its sixteen
commits were already here. The accompanying `engine-change.txt` is that
session's write-up and covers blackstar-rs gameplay, the game/engine split, and
the unendlich work that arrived separately as `-022`.

**Verified for the merges: they apply clean and the checkers above run.** Not
verified: none of the merged code was built. `games/unendlich` is a wgpu target
with no `/dev/dri` here, `apps/fao-codec`'s numbers were not reproduced and its
suite (reported 102/0) was not re-run, and rusty-board's wayland/GTK and KWin
script paths are not buildable in this sandbox. Each merge commit says so in
its own message.
