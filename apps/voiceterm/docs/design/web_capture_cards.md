# Web capture → Helix-editable note cards → dual-save

Status: **DESIGN + pure card-split core (v0.1.503).** A large
multi-part feature; designed first, with the safe pure core built and
the live surface / persistence deliberately staged.

## The feature (user)

A Helix-style modal editor overlaid on the web browsing page. While
reading a page, the user can:
  - capture content (selection or whole paragraphs),
  - take notes on it with Helix muscle-memory (reusing the
    editor_voice verb work),
  - "split cards out of it" — carve the captured content into atomic
    note-cards,
  - have those cards auto-save in an organized way into BOTH the live
    session AND the local filesystem.
Paired with live voicing as a lightweight always-available "bit mode"
capture.

Explicit reuse the user named: the Helix parsing work
(editor_voice.rs) as the INTERMEDIATE command layer — same modal
grammar drives note capture as drives the terminal editor.
("camera following" read as VIEWPORT-FOLLOW: the editor view tracks
the cursor/selection as it moves — the camera metaphor for editor
scrolling. To confirm with the user.)

## What already exists (ground truth, v0.1.502)

- `cardsCanvas` (qml/main.qml): a cards SURFACE that opens/closes, has
  a `_sourceMode` (mermaid / niftyplot), tracks block IDs + source
  files, and holds `cardData: []`. The DESTINATION exists.
- `editor_voice.rs`: the reusable intermediate layer —
  `Editor` enum, `EditorAction` struct, `translate(editor, phrase)
  -> Option<EditorAction>`. The Helix grammar is here.
- `paragraph_selected(id, text, content_type, raw_content)`
  (main.rs): the browse page ALREADY notifies the backend when a
  paragraph is selected, carrying both rendered + raw forms — the
  natural CAPTURE hook (its own comment anticipates
  "highlighting in 2.2, capture recording in 2.3+").

So the capture hook and the cards destination both exist; the VOID is
the pipeline between them: split → edit → organize → dual-save.

## Architecture

### Layer 1 — Capture (hook exists)
paragraph_selected is the seam. A capture appends the (raw + rendered)
paragraph to a provisional capture buffer for the current page/session
— NOT yet a card. Mirrors the v0.1.497 provisional/temp pattern:
loose to capture, structured on promotion to a saved card.

### Layer 2 — Card split (THIS ship: pure core)
Carve captured content into atomic cards by STRUCTURE. A card
boundary is created at: a heading (markdown # / ## / setext), a blank
line between paragraphs, a list-item run (grouped), a fenced code
block (whole block = one card), and an explicit user split (Helix
selection → "split here"). Each card gets: a title (first heading or
first ~N chars), a body, a source ref (url + paragraph id), and a
kind (prose / list / code / heading). Pure, testable. This is the
"split cards out of it" operation, with the Helix selection as the
intermediate: a card is a span; the verb grammar carves/merges spans.

### Layer 3 — Modal Helix editing (staged)
The captured/split content is editable with the editor_voice grammar.
Reuse translate(Editor::Helix, phrase) so the SAME verbs (j/k/w/b/
visual/etc.) move and edit within the card buffer. "split here" /
"merge up" become card-level verbs layered on the selection model.
Viewport-follow keeps the active card centered. Needs the live QML
surface — staged after the core + the verb extension.

### Layer 4 — Dual-save + auto-organize (staged; touches filesystem)
Save each card into BOTH:
  - the live SESSION (in-memory + the session model the app already
    keeps), and
  - the local FILESYSTEM, auto-organized. Proposed layout:
    notes/<domain-or-topic>/<page-slug>/<card-id>.md with frontmatter
    (source url, captured-at, kind, title). Auto-organization keys off
    the source (domain → topic folder) + capture time. This LANDS ON
    DISK so it calcifies a layout — defer until the on-disk scheme is
    agreed (same caution as the FP0x format + the corpus owner field).

### Layer 5 — Voice "bit mode" capture (staged)
A lightweight always-available capture: a short voice phrase ("note
this", "capture", "split card") routes through the existing voice
intent layer to Layer 1/2 without opening the full editor. Reuses the
repl_voice/editor_voice routing. Staged after the core proves out.

## What v0.1.503 ships (pure core only)

`src/note_cards.rs`: `split_into_cards(captured: &str, source: &CardSource)
-> Vec<Card>` + the Card/CardKind/CardSource types. Pure structural
splitter (headings, blank-line paragraph breaks, list grouping, fenced
code blocks). No IO, no QML, no format-on-disk. Fully tested.

NOT yet: the QML overlay, the editor_voice card-verb extension, the
filesystem layout, the voice bit-mode routing. Those are staged
(surface needs the live build; disk layout calcifies; verb extension
should follow the user's real card-verb muscle memory like the
editor verbs did).

## Helix-edit → re-render pipeline (user's v0.1.504 instruction)

User: "wire helix to access the tmp file or AST content, then
re-render QML sections." Plumbing exists at both ends:
  - temp QML file: backend writes /tmp/web2qml_pv_*.qml, tracks it in
    self.temp_qml_file, the qml_preview Loader renders it (~3105);
  - re_render_current_page() (6072) — but WHOLE-page only;
  - content blocks keyed by block_id.

Pipeline: Helix edits a scratch file → backend reads it → re-splits
via split_into_cards → diff_cards(old,new) → sections_to_rerender()
gives the changed block indices → re-render ONLY those sections (the
"camera follows the edit" behaviour).

### v0.1.504 ships the SCOPED-RE-RENDER CORE (pure, tested)
diff_cards + sections_to_rerender: positional diff of old vs re-split
cards → which sections changed/added/removed. An edit to one
paragraph flags exactly one section; an idle buffer flags none. This
is what makes "re-render QML SECTIONS" (scoped, not whole page)
correct.

### Deliberately NOT wired yet (consumer doesn't exist)
  - scratch-file CONVENTION (where Helix writes; QML editor + backend
    agreement) — calcifies a path; define with the live surface;
  - a SCOPED re-render method QML-side (re_render_current_page is
    whole-page) — needs QML co-design, like dirty-rect stage c;
  - the backend ingest method (read tmp → split → diff → scoped
    re-render) — infra-on-spec before the QML scoped-patch consumer
    exists. The diff core is the safe seam.

## Open questions for the user

- Pulsar "camera following" / impakt-cam: RESOLVED by reading the
  uploaded source (pulsar-stuff.zip → impakt-cam). It's a neovim-style
  smooth cursor-follow: animates editor scroll so the cursor stays
  vertically centered. Ported the ALGORITHM (time-windowed cursor
  averaging for smooth glide, center target, big-jump snap cutoff) as
  native Rust in src/follow_camera.rs (v0.1.505), DISCARDING its heavy
  parts (GSAP tweens, DOM getBoundingClientRect, per-frame JS
  onUpdate). Plan: Rust computes one target scroll; QML eases via a
  native `Behavior on contentY { NumberAnimation { easing.type:
  Easing.OutCubic } }` — no per-frame JS. Pairs with the
  helix-edit→re-render pipeline (the camera follows the edit/cursor).
- Dual-save filesystem layout: notes/<domain>/<page-slug>/<card>.md
  acceptable, or a different organization (by topic? by date? by
  session)?
- Card-split granularity default: one card per paragraph, per
  heading-section, or user-driven only?
- "bit mode" trigger phrases: what should they be (matching your real
  muscle memory, like the editor verbs)?
