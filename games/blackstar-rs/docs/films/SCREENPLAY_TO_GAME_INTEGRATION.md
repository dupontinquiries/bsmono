> **STATUS: A PLAN, NOT A RECORD.** Two of its claims are not true of this
> repository, and both matter before anyone follows it.
>
> * **`src/saga.rs` does not exist here.** The document says "Already created.
>   No changes needed." It is not in the tree. Everything below that reads
>   "from saga.rs" is therefore describing a file nobody here can open.
> * **Its Film 5 worked example uses the abandoned plot** — the Matter
>   Transformation Planet and Billy guarding it — which
>   `FILM_05_SOLAR_SCREENPLAY.md` replaced. The *method* in that example is
>   sound; the content is superseded.
>
> Two smaller mismatches: the title says a 26-film saga while the body
> describes 9, and it proposes adding `Stratum::Ascension` and
> `Stratum::Convergence` to `archive.rs`, which today ships exactly the six
> strata Layer 0–5. Adding them is a real proposal, not a description.
>
> **Step 1 is wrong and would break the archaeology layer.** It says to add
> `Stratum::Ascension = 6` and `Stratum::Convergence = 7`. `Stratum` is
> ARCHAEOLOGICAL DEPTH, not narrative time — its own doc says it "numbers them
> from the present downward", it derives `Ord`, and `from_u8` clamps to 5. The
> films run forward, so Film 9 is the *latest* event in the saga; filing it as
> Layer 7 would sort the newest thing in the game as older than the Machine
> layer, and every comparison on that enum would be wrong in a way no type
> catches. The relationship is the other way round: the films are how the
> player digs, and the deep strata are what they UNCOVER.
> `saga::evidence_stratum` does that mapping, and
> `saga::stratum_tests::the_strata_enum_still_has_exactly_six_layers` fails if
> anyone follows Step 1.
>
> What it is good for, and it is genuinely good: the pipeline it lays out —
> screenplay beat → Claim → Findings → codex link, with tests catching
> orphans — is the right shape and matches how `archive.rs` already works.

# TRANSFORMERS BLACKSTAR 26-FILM SAGA: SCREENPLAY → GAME INTEGRATION

## Overview

The 26-film saga has three layers:

1. **Screenplays** — `/archive/transformers_blackstar/` (external, HTML + Markdown)
2. **Game narrative structure** — `src/saga.rs` (new module, in Rust)
3. **Game archaeology layer** — `src/archive.rs` and `src/codex.rs` (existing systems)

This document explains how each screenplay film maps to the game's evidence system, allowing players to discover the saga's story through in-game archaeology rather than just reading external documents.

---

## The Screenplay Archive

Your screenplay content is organized as:

```
transformers_blackstar/
├── 🎬 TRANSFORMERS BLACKSTAR.md          [Opening storyboard]
├── TRANSFORMERS_DOOMSDAY+DOMINION+DESTINY.md  [Films 1–3 outline + treatments]
├── TF_TRILOGY_TWO_FRAMEWORK.md           [Films 4–6 framework + character arcs]
├── Jhiaxus_Horror.md                     [Character study: Jhiaxus]
├── TF_JHIAXUS_AND_THE_CORRUPTED.md       [Corrupted faction lore]
├── TF_ADDITIONAL_SCENE_AIDS.md           [Optional scenes, deleted material]
├── TF_QUANTUM_GRAVITY_LEAP.md            [Physics/lore engine]
├── TF_DOOMSDAY_FILMMAKER_CHARTS.md       [Visual design, character sheets]
├── TF_DOMINION_SHIELD_HINTS.md           [Shield mechanics lore]
├── Clones.md                             [Clone coalition design doc]
└── addendum/                             [Additions and variants]
```

---

## How `saga.rs` Works

`saga.rs` is the **single source of truth** for:

- **9 films** (Films 1–3: Doomsday, Films 4–6: Ascension, Films 7–9: Convergence)
- Each film's **three acts**
- **Emotional cores** and **thematic centers**
- **Character arcs** (spanning the full saga)
- **Unresolved questions** (carrying across trilogies)
- **Roster changes** (new factions, departures, transformations)

Example — Film 1:

```rust
pub static FILM_01_DOOMSDAY: FilmEntry = FilmEntry {
    number: 1,
    trilogy: 1,
    position: 1,
    title: "TRANSFORMERS: DOOMSDAY",
    premise: "Unicron awakens beneath Earth; Hot Rod becomes Rodimus Prime...",
    acts: [...],
    emotional_core: "...",
    introduces: vec!["Rodimus Prime", "Unicron (dormant)", "Aiden", ...],
    resolves: vec!["Earth's immediate extinction risk"],
    unresolved: vec![...],
};
```

This is **not** a screenplay. It's a **story skeleton** that tells the game's archaeology layer what to look for and what it means.

---

## How to Populate the Game's Archive Layer

The game's `archive.rs` already has:

- **Strata** (layers of history): Present, Recent, Blackstar, BlackArchive, Asteri, Machine
- **Claims** (propositions about the universe's past)
- **Findings** (physical evidence, documents, sensor logs)
- **Links** (how evidence supports or contradicts claims)

**To wire in the screenplay:** Add new **Strata** and **Claims** corresponding to each film's story beats.

### Step 1: Extend the Strata Enum

Add new strata for the post-Blackstar era:

```rust
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Stratum {
    // Existing layers
    Present = 0,
    Recent = 1,
    Blackstar = 2,
    BlackArchive = 3,
    Asteri = 4,
    Machine = 5,

    // NEW: Ascension trilogy
    Ascension = 6,
    // NEW: Convergence trilogy
    Convergence = 7,
}
```

### Step 2: Create Claims for Each Film's Major Beats

Each film becomes a **Claim** cluster. Example:

```rust
// Film 4: First Light
Claim {
    id: "film_4_shield_breach",
    text: "The shield was breached for the first time \
           to establish contact with the Clone coalition",
    stratum: Stratum::Ascension,
    prior: Prior::Probable,
},
Claim {
    id: "film_4_morpher_identity",
    text: "The Morpher is an Ice Clone guardian who has \
           existed longer than recorded Cybertronian history",
    stratum: Stratum::Ascension,
    prior: Prior::Probable,
},
Claim {
    id: "film_4_photon_man_exists",
    text: "The Watch contains a thirteenth form called \
           Photon Man that is activated under extreme conditions",
    stratum: Stratum::Ascension,
    prior: Prior::Speculative,
},
```

### Step 3: Create Findings (In-Game Discoveries)

Each finding is evidence for or against a claim. They come from:

- **Dialogue logs** (characters discussing events)
- **Sensor data** (Array readings, orbital observations)
- **Archive fragments** (discovered documents)
- **Archaeology** (examining ruins, observing artifacts)

Example findings for Film 4:

```rust
Finding {
    id: "array_log_shield_crossing",
    source: "blackstar_array_logs",
    text: "Signal loss: 11 seconds. Shield geometry shifted. \
           Instruments recalibrating. We're through.",
    supports: &["film_4_shield_breach"],
    opposes: &[],
    medium: Medium::DigitalLog,
    stratum: Stratum::Ascension,
},

Finding {
    id: "morpher_testimony_age",
    source: "first_contact_transcript",
    text: "\"I have guarded this boundary for longer than \
           Cybertron's entire recorded history.\"",
    supports: &["film_4_morpher_identity"],
    opposes: &[],
    medium: Medium::DirectTestimony,
    stratum: Stratum::Ascension,
},

Finding {
    id: "watch_spectral_analysis",
    source: "technical_analysis",
    text: "The Watch contains a dormant resonance pattern \
           at 7.7 terahertz, never before detected in any \
           Clone device.",
    supports: &["film_4_photon_man_exists"],
    opposes: &[],
    medium: Medium::AnalysisReport,
    stratum: Stratum::Ascension,
},
```

### Step 4: Connect Characters to Archive Entries

The `codex.rs` now includes the 5 new characters. Wire them into findings:

```rust
Finding {
    id: "morpher_first_contact_video",
    source: "rodimus_expedition_log",
    text: "The Morpher emerged from the ice with the Watch \
           on a chain of condensed ice that never melted. \
           His first words: 'The shield has been compromised. \
           I will show you why.'",
    supports: &["film_4_morpher_identity"],
    opposes: &[],
    medium: Medium::VideoLog,
    stratum: Stratum::Ascension,
},
```

Players who read the codex entry for "the_morpher" and then find this log will see the corroboration.

---

## Where to Add These in the Codebase

### `src/archive.rs`

Add new claims and findings to the tables:

```rust
// Around line 100–200, after existing FINDINGS:

pub static FINDINGS: &[Finding] = &[
    // ... existing findings ...

    // FILM 4: FIRST LIGHT
    Finding { /* ... */ },
    Finding { /* ... */ },
    // ... more findings for films 4–9 ...
];

pub static CLAIMS: &[Claim] = &[
    // ... existing claims ...
    
    // FILM 4
    Claim { /* ... */ },
    Claim { /* ... */ },
    // ... more claims for films 4–9 ...
];
```

### `src/saga.rs`

**Already created.** No changes needed. Reference it from:

- Tutorial text that introduces the film structure
- Menu systems that might display "which film does this evidence come from?"
- Tests that verify `saga.rs` and `archive.rs` are synchronized

---

## Writing Findings from Screenplays

For each film, extract:

1. **Key story beats** → become Claims
2. **Dialogue, data, documents** → become Findings
3. **Contradictions or ambiguities** → become multiple Findings in conflict

### Example: Film 5 (Ember Deep)

**Screenplay beat:**
> "Billy is a Fire Clone of singular intensity who has been guarding this world alone for longer than the Clone coalition has existed."

**Becomes in the game:**

```rust
Claim {
    id: "film_5_billy_age",
    text: "Billy has guarded the Matter Transformation Planet \
           for longer than the Clone coalition's recorded existence",
    stratum: Stratum::Ascension,
    prior: Prior::Probable,
},

Finding {
    id: "morpher_testimony_billy_age",
    source: "ascension_expedition_logs",
    text: "\"There is one who fights further out than the rest of us. \
           He does not require coordination. He has fought alone for \
           longer than we have had coalition records.\"",
    supports: &["film_5_billy_age"],
    opposes: &[],
    medium: Medium::DirectTestimony,
    stratum: Stratum::Ascension,
},

Finding {
    id: "planetary_defense_logs_billy_arrival",
    source: "matter_transformation_planet_sensors",
    text: "Energy signature (Magma form) holding orbit against \
           Hijak fleet assault. Estimate endurance: unlimited. \
           Estimate consciousness: autonomous.",
    supports: &["film_5_billy_age"],
    opposes: &[],
    medium: Medium::SensorLog,
    stratum: Stratum::Ascension,
},
```

---

## Sync Check: Making Sure Everything Connects

Before adding findings to `archive.rs`, verify:

1. **Every claim is real**: Check `saga.rs` — does this story beat actually appear in that film's entry?
2. **Every finding is rooted in the screenplay**: Can you point to dialogue or a scene that justifies this evidence?
3. **No orphaned claims**: Use `archive.rs` tests to verify every claim has at least one supporting finding.
4. **Character consistency**: Cross-check codex entries against saga character arcs.

Test in the game:

```
cargo test archive -- --nocapture
cargo test codex -- --nocapture
```

The test suite will flag:
- Findings that cite non-existent claims
- Claims with zero evidence
- Codex entries with no corroborating findings

---

## Integration Checklist

- [ ] **Phase 1: Structure**
  - [x] `saga.rs` created with 9 films
  - [x] Character arcs defined
  - [ ] Add `Stratum::Ascension` and `Stratum::Convergence` to `archive.rs`

- [ ] **Phase 2: Claims**
  - [ ] Create 60–80 claims (roughly 7–9 per film)
  - [ ] Organize by stratum
  - [ ] Link to screenplay beats in comments

- [ ] **Phase 3: Findings**
  - [ ] Write 200–300 findings (2–4 per claim)
  - [ ] Use screenplay dialogue and descriptions as source text
  - [ ] Create supporting and opposing finding pairs for contested claims

- [ ] **Phase 4: Codex**
  - [x] 5 new character entries added
  - [ ] Link each entry to a claim via `claim: Some("...")`
  - [ ] Test annotation system

- [ ] **Phase 5: Testing**
  - [ ] `cargo test archive` passes
  - [ ] `cargo test codex` passes
  - [ ] No orphaned claims or findings
  - [ ] Stratum ordering is correct

- [ ] **Phase 6: Documentation**
  - [ ] Update `docs/STORY_BIBLE.md` with film structure
  - [ ] Add `docs/SAGA_ARCHAEOLOGY.md` explaining how to read the 26-film arc in-game
  - [ ] Create quick-reference: "which film does this evidence come from?"

---

## Example: Complete Film 4 Pipeline

### Screenplay (external)

From `TF_TRILOGY_TWO_FRAMEWORK.md`:

```
### FILM 4 — WORKING TITLE: *FIRST LIGHT*

[...]

### Where We Pick Up

Rodimus Prime leads a small joint fleet...
They cross the shield boundary for the first time.

The transition should be a cinematic event in itself — not a gate, 
not a door — more like breaching the surface of water. The geometry 
of space changes subtly on the other side.

Sam Witwicky, watching from inside the shield on relay: the signal 
cuts out for eleven seconds. Then comes back.

Rodimus: *"We're through."*
```

### `saga.rs` (game structure)

```rust
pub static FILM_04_FIRST_LIGHT: FilmEntry = FilmEntry {
    number: 4,
    title: "TRANSFORMERS: FIRST LIGHT",
    premise: "The heroes breach the shield for the first time...",
    acts: [
        "Rodimus leads a joint expedition across the shield boundary...",
        ...
    ],
    ...
};
```

### `archive.rs` (game evidence)

```rust
Claim {
    id: "film_4_shield_geometry_changed",
    text: "Crossing the shield boundary alters the geometry of space itself",
    stratum: Stratum::Ascension,
    prior: Prior::Probable,
},

Finding {
    id: "rodimus_log_shield_crossing",
    source: "first_light_expedition_logs",
    text: "Signal loss: 11 seconds during boundary transit. \
           Geometry shifted. Instruments recalibrating. Rodimus: \
           'We're through.'",
    supports: &["film_4_shield_geometry_changed"],
    opposes: &[],
    medium: Medium::CaptainLog,
    stratum: Stratum::Ascension,
},

Finding {
    id: "aiden_testimony_wonder",
    source: "first_light_expedition_logs",
    text: "Aiden, looking out a viewport at the outer cosmos for \
           the first time: says nothing. There are no words that fit.",
    supports: &["film_4_shield_geometry_changed"],
    opposes: &[],
    medium: Medium::Observation,
    stratum: Stratum::Ascension,
},
```

### `codex.rs` (game reference)

```rust
Entry {
    id: "the_morpher",
    title: "the morpher",
    claim: None,  // or Some("film_4_morpher_identity") if you want annotation
    lines: &[
        "ice clone of unrecorded age. guardian of the shield's perimeter...",
        ...
    ],
},
```

### Player Experience

1. Player finds the sensor log in a wreckage on the shield boundary
2. Game displays `Finding { id: "rodimus_log_shield_crossing" }`
3. Player checks codex: entry "the morpher" becomes available
4. In the codex, player sees the entry is annotated with claim "film_4_morpher_identity"
5. Player opens the archaeology map
6. Map shows the claim and other evidence supporting it
7. Player pieces together: "Film 4 introduced the Morpher. This evidence proves it."

---

## Next Steps

1. **Go screenplay by screenplay**, extracting the major plot beats
2. **Create claims** for each beat (60–80 total across 9 films)
3. **Write findings** that support or contradict those claims (200–300 total)
4. **Test** that everything connects without orphans
5. **Play** through the game and follow the archaeology layer to see the complete saga emerge

The game becomes a **discoverable archive** of the entire 26-film narrative. Players don't have to read the screenplay documents—the game becomes the storytelling medium.