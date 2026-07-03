# kRapper

> hum it. rap it.

A Rust + [iced](https://iced.rs) workflow for writing rap to a hummed vibe.

The full workflow has four stages (see [`docs/path-a-plan.md`](docs/path-a-plan.md)):

1. **Beatbox** — record a bed, vocally or by loading a `.opus` track
2. **Calibrate** — one-shot probe deciding whether echo cancellation needs to run
3. **Vibe capture** — loop the bed, mic armed, hum over it
4. **Scratchpad** — write lyrics with a hot mic, against the captured vibe

## Ship plan

This repo ships in two paths:

- **Ship B — Scratchpad-first.** *(this is what's wired today)* The Scratchpad
  stage with manually-typed vibe inputs (BPM, syllables/bar, stress pattern,
  rhyme seed). Lets us validate "writing-with-suggestions" without first
  solving the audio engineering. **Stress-matched blurbs**, not full-line word
  soup.
- **Ship A — Beatbox-first.** *(in progress)* The audio half: the beatbox
  mixer, the `.opus` folder loader with BPM detection and in/out bounds, the
  calibration probe, and the AEC-wrapped Vibe-capture stage feeding a real
  `VibeSummary` into the Scratchpad. **Echo cancellation:**
  `webrtc-audio-processing` (gated behind a Cargo feature).

The contract between A and B is `src/vibe.rs::VibeSummary`. Whether you
*typed* the BPM and stress pattern (B) or *hummed* them and let the analysis
extract them (A), the Scratchpad consumes the same struct.

## Architecture

```
┌──────────────┐   audio thread (cpal)
│   mic input  ├──► 25ms RMS windows ─► hysteresis onsets ─► hot-mic indicator
└──────┬───────┘
       └────► rolling level bars ──────────────────────────────┐
                                                               ▼
                            ┌─────────────────────────────────────────┐
                            │  Scratchpad (Ship B)                    │
                            │  ┌───────────────┐  ┌───────────────┐   │
                            │  │ Vibe inputs   │  │  Blurb panel  │   │
                            │  │ BPM / syll /  │  │  stress-      │   │
                            │  │ stress / seed │  │  matched      │   │
                            │  │ / vocab       │  │  fragments    │   │
                            │  └───────┬───────┘  └───────▲───────┘   │
                            │          │ regen             │          │
                            │          ▼                   │          │
                            │  ┌───────────────────────────┴───────┐  │
                            │  │   Lyrics editor                   │  │
                            │  │   live syllable count vs target   │  │
                            │  │   click a blurb to insert at cur. │  │
                            │  └───────────────────────────────────┘  │
                            └─────────────────────────────────────────┘
                                          ▲
                                          │ feeds VibeSummary
                            ┌─────────────────────────────────────────┐
                            │  Path A (planned)                       │
                            │  Beatbox → Calibrate → Vibe capture     │
                            └─────────────────────────────────────────┘
```

Word matching is dict-first via CMUdict — phonetic rhyme, phoneme stress
patterns, authoritative syllable counts. See [`docs/learning-example.md`](docs/learning-example.md)
for the difference between this and the original string-edit approach (also
preserved as `cargo run --example learning_string_matching`).

## Project layout

```
kRapper/
├── Cargo.toml
├── README.md
├── data/
│   ├── README.md                  # how to upgrade to full cmudict-0.7b
│   └── cmudict_starter.txt        # curated starter set
├── docs/
│   ├── learning-example.md        # string-approach pedagogy
│   └── path-a-plan.md             # beatbox / opus / webrtc / arranger plan
├── examples/
│   └── learning_string_matching.rs
├── src/
│   ├── main.rs
│   ├── app.rs                     # Scratchpad UI (Ship B)
│   ├── audio.rs                   # cpal capture + onset detection
│   ├── dict.rs                    # CMUdict + phoneme operations
│   ├── vibe.rs                    # VibeSummary contract
│   └── vocab.rs                   # Vocab + Blurb + suggest_blurbs
└── vocab/
    ├── words_wrld.txt
    ├── words_theweeknd.txt
    └── words_common.txt
```

## Build & run

```bash
cargo run --release
```

The runnable learning example (string-edit fit-to-vocab):

```bash
cargo run --example learning_string_matching
```

### Linux audio deps

```bash
sudo apt install libasound2-dev pkg-config       # Debian/Ubuntu
sudo dnf install alsa-lib-devel                  # Fedora
```

### Upgrading to the full CMUdict

```bash
cd data/
curl -LO https://raw.githubusercontent.com/cmusphinx/cmudict/master/cmudict-0.7b
mv cmudict-0.7b cmudict-0.7b.txt
```

kRapper auto-detects and prefers the full file when present.

### Version pins

- `iced = "0.13"` (`canvas`, `advanced`, `tokio` features)
- `cpal = "0.15"`
- `rand = "0.8"`

For Path A (planned): `symphonia` for opus decoding,
`webrtc-audio-processing` (gated) for AEC.

## Using Scratchpad (Ship B)

1. **Vibe inputs.** Set BPM, syllables-per-bar, stress pattern (`x.x.x.x.`),
   and an optional rhyme seed. Pick a vocab.
2. **Vocab management.** Next to the vocab picker, two buttons:
   - **+ Append** — opens a file picker; the chosen `.txt`'s words are merged
     into the currently selected artist profile and saved back to
     `./vocabs/<name>.txt`.
   - **+ New** — opens a file picker; the file's stem becomes the new artist
     name; writes `./vocabs/<stem>.txt` and switches to it. Useful if you're
     engineering / coaching for another writer and want a separate profile
     for their voice.

   Embedded WRLD/Weeknd/Common are written to `./vocabs/` on first launch and
   from then on are just files on disk like any other vocab.
3. **Write lyrics** in the editor. The strip below shows:
   - **Scansion chips** — each word in the current line, with its phoneme
     stress as dots. Green = aligned with the target bar pattern, orange =
     misaligned, gray = past the bar. Trailing `?` = not in dict (heuristic).
   - **Counts line** — `▌ L·C` cursor indicator (where blurbs will paste),
     current-line syllables vs target, total verse syllables.
4. **Blurbs adapt to where the cursor is.** Partial line → fragments that
   complete it. Empty seed → auto-rhyme with the line above. Manual seed
   overrides. The blurb header tells you which target and which rhyme.
5. **Click any blurb** to paste it at the cursor.
6. **Hot mic.** Arm the mic to pulse the level bars + count onsets.
7. **Pitch meter (new).** Below the mic strip:
   - `♪ A4 +3¢ · 440 Hz` — the note you're currently humming, with cents
     offset from the nearest semitone.
   - `scale: A minor pent (strong, score 0.78)` — best-fit guess across 12
     roots × 5 scales (major / natural minor / pentatonic major / pentatonic
     minor / blues). "Strong" / "ok" / "fuzzy" reflect the margin over the
     runner-up.
   - 12-cell pitch class strip — each cell is a note name (`C C# D...`).
     The red cell is what you're hitting right now; brighter green cells
     are in the guessed scale, weighted by how often you've sung them.
     **Not autotune** — purely analysis.

   *Reset* clears the pitch history alongside the onset counter.
8. **Sessions persist** to `./session.json` (debounced 300ms idle, atomic
   write). On launch, kRapper restores the last session.
9. **Debug** (toggle, or `KRAPPER_DEBUG=1`) — surfaces audio thread state,
   dict source/size, vibe values, vocab coverage.

### Stress pattern notation

`x` or `1` = stressed syllable; `.` or `0` = unstressed. Spaces ignored.

```
x.x.x.x.    →  classic 8-syllable alternating
xx..xx..    →  bursts of two stressed
x...x...    →  back-beat heavy, syncopated
```

If the pattern is shorter than `Syll/bar`, it tiles. Longer, it truncates.

## Scope, honestly

The word side is properly at the phoneme layer — rhyme, stress, syllable
counts all via CMUdict where available. The blurb generator searches the
real space of stress-matched word combinations. None of it is "guessing
based on spelling."

The audio side is still RMS onset detection — useful as a hot-mic indicator,
not yet useful as a "this is the vibe you hummed" extractor. That's Path A's
job; see `docs/path-a-plan.md`.
