# dev/ — preparation for the game's voices, music and lore

Everything here is for the people making the game's audio: what to record,
what to compose, and the world it belongs to. The cards and sheets are
generated from the game's own data, so they always say what the game does.

| | |
|---|---|
| `LORE.md` | The world: the clans, the Gathering, Frostfang and Emberclaw, the old grounds, every ancestor (kind, age, land, gender, story), and the music of each clan's culture. |
| `voice_script/01_goldenmane.svg` … `16_announcer.svg` | One cue card per voice, to read at the mic: every line in large type, the file to save it as, how to say it, an energy meter (1 murmur … 5 full shout), and when the game plays it. |
| `voice_script/recording_sheet.tsv` | The same, as a spreadsheet: one row per file. |
| `music/intro_cue_sheet.svg` | The intro's timeline for scoring: sections, the narrator's eight lines, numbered sync points, a 90 BPM bar grid, and instrumentation notes per section. |

## Recording voices

- 155 lines: 14 ancestors × 9 (a select line, one per ability, two kills,
  two deaths), the narrator's 8-line prologue, and 21 announcer calls.
- Record 48 kHz mono; leave about 0.2 s of silence before and after each
  line; export **Opus** (~96 kbps). WAV (16-bit or float) also works.
- Save each take as the path on its card, under the game's audio folder:
  `assets/audio/voice/goldenmane/cast4.opus`, `assets/audio/narrator/prologue1.opus`,
  `assets/audio/announcer/first_blood.opus`.
- `kill.1` / `kill.2` (and `death.1` / `death.2`) are two different lines;
  the game plays one at random, never the same twice running.
- A line not yet recorded is simply silent, so record in any order.

## Composing

| File (under `assets/audio/`) | Plays |
|---|---|
| `music/intro_full.opus` | the full intro (first match of a session): 61.4 s, prologue included |
| `music/intro.opus` | the short intro (every later match): 17.4 s |
| `music/fanfare.opus` | instead of the synthesized fanfare, when no intro track is present |
| `music/jungle.opus`, `music/pass.opus` | each map's loop, from when play begins |

An intro track replaces the synthesized fanfare; the narrator's lines play
over it (the music ducks under them); skipping the intro silences both. The
cue sheet has the exact times; they come from the game's intro, so if the
intro changes, regenerate and the sheet follows.

Sound effects are generated (`src/sfxgen.rs`, into `assets/audio/sfx/`); a
studio file at the same path replaces one. To hear them all:
`cargo run --example gen_sfx -- assets/audio sfx_preview.wav` (writes a
64 s preview reel; not kept in the repository).

## Regenerating

```sh
cargo run --example devdocs        # every card, the sheet and the cue sheet, into dev/
```

`cargo test` fails (`devdocs::the_committed_dev_docs_are_current`) if a
committed card or sheet no longer matches the game's data — change the data
(`src/voicescript.rs`, `src/lore.rs`, `src/intro.rs`), then regenerate.
