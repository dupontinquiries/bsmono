# TODO: notes & clippings — jot, clip-with-provenance, voice-dictate

## Vision (user, v0.1.640 session)
A way to jot notes and add them to the web (we render markdown). Be able
to: click a word OR voice-command + highlight to create a card for a
section / pull up suggestions; a voice-typing entry mode for free jotting;
and clip SNIPPETS that remember where they came from (to reference back) —
text, code segments, or image crops.

This is a notes-and-clippings system. The pieces:
1. CLIP a selection from anywhere → a note card with PROVENANCE.
2. SUGGESTIONS when you clip (related cards, actions).
3. VOICE-DICTATION entry mode for free-text jotting.
4. Multi-modal clips: text, code, image crops.
5. Notes are MARKDOWN → viewable in the web view, referenceable back.

## Done (pure, tested — v0.1.641)
src/note_clip.rs — the data model + markdown serialization (the spine):
- Source { Web{url,title}, File{path,anchor}, Terminal{command,at},
  Manual } with .citation() → a markdown citation per source. THIS is
  "reference back": every clip knows its origin.
- ClipKind { Text (→ blockquote), Code{lang} (→ fenced block),
  Image{path,alt} (→ markdown image) }.
- Clip { kind, content, source, captured_at }.to_markdown() → content +
  "— <citation>".
- Note { title, blocks: [Prose | Clip] } interleaves dictated prose with
  clips; .to_markdown() → a full .md doc; .sources() → dedup'd jump-back
  list.
9 tests (each source citation, each clip kind, interleaved note, dedup).

## Wiring (build-gated — by piece)

### Clip from a selection (the "highlight → card" core)
- WEB: the page already exposes paragraph_selected (CardSource has
  url+paragraph_id). On a voice "clip this" / "make a card" or a clip
  gesture over a selection → build a Clip{ Text or Code, content=selection,
  source=Web{url,title} } and push to the active Note. Reuse the existing
  selection plumbing.
- TERMINAL: selection already works (selection.rs extract). "clip this"
  over a terminal selection → Clip{ source=Terminal{command of the block,
  block.started_at} }. Code-detect via the block (notebook::classify /
  fenced) to set ClipKind::Code.
- MARKDOWN FILE view: selection → Clip{ source=File{path, anchor=heading
  or Lx-Ly} }.
- Detect code vs prose for the clip kind: a selection inside a fenced
  block or matching a language pattern → Code{lang}; else Text.

### Click a WORD → suggestions (reuse landmark/recall scorers)
- Tapping/saying a word in rendered content → run the existing
  ui_landmarks/recall-style scorer over the note corpus + page → "related
  clips / create card / search" suggestions. The click-search planner
  (plan_click, v626) is the bucketing brain; point it at the note corpus.

### Voice-dictation entry mode
- A toggled "jot mode": route transcription straight into the active
  Note as Block::Prose (append), instead of through the command intent
  parser. End on a wake-word / "done jotting". The two-tier whisper path
  already produces the text; this just changes the SINK.

### Image crops
- A crop gesture over the web/preview → save the crop to clips/<id>.png,
  push Clip{ ClipKind::Image{path, alt} }. alt from a voiced caption or
  the surrounding text.

### Persist + view
- Notes saved as .md under a notes dir; the web view already renders
  markdown → "add to the web" = open the note's .md. A qt_method
  note_to_markdown()/save_note(); list notes for a picker.

## Suggested build order
1. Web "clip this" → Text/Code clip with Web source (smallest, proves the
   spine end-to-end + immediately useful).
2. Voice jot mode (just re-routes transcription → Prose).
3. Terminal + file clips (more Source variants, same Clip path).
4. Suggestions on word-tap (reuse plan_click/recall).
5. Image crops (needs the crop gesture + save).

## Next pure slice (optional, before wiring) — DONE

`egui-frontend/src/clip_kind.rs`. `detect(text, ctx) -> ClipKind`, three
ranked kinds of evidence, biased toward Text because a paragraph in a
fence is ugly and lossless while a diff in a blockquote is reflowed,
de-indented and part-italicised:

1. a fence in the selection (```rust) — the author said so, and the
   language comes free; outranks everything, including a contradicting
   command;
2. the command that produced it — `cat main.rs` / `bat` / `head` name a
   file and its extension names the language; `git diff|show|log|blame`
   and `diff` are code whatever the file was; `ls`, `git status` and an
   `echo` containing a brace imply nothing. A file view passes a path
   instead;
3. shape, needing TWO signals of five, since one is a coincidence:
   statement punctuation at line ends, a declaration opener, sigils prose
   does not use (`=>`, `::`, `->`), shell block words, and structural
   indentation (a uniformly indented quote is not code — it has one
   depth, not several).

The shell needed its own signal: `if [ -f "$f" ]; then / cp … / fi` has
no semicolon at a line end, no `if (` and no sigil, so it scored zero
until block words counted. Found by the test for that snippet.

Wired at the one clip site: `ClipSelection` was hardcoded to
`ClipKind::Text`. 5 tests.

Unknown extensions and files without one fence with NO language rather
than guessing a grammar — `Code { lang: "" }` is a plain fence, which is
right for a shell transcript.


## DONE — persist (2026-09-07)

"Notes saved as .md under a notes dir; the web view already renders markdown → 'add to the
web' = open the note's .md". The data model and `Note::to_markdown` have been pure and tested
since v0.1.641 and nothing ever wrote the result anywhere, so every clip died with the
process — the one failure a clipping tool cannot have.

`note_store` writes them to `$XDG_DATA_HOME/voiceterm-egui/notes/`. DATA, not config: a
clipped note is something the operator made, and config directories get hand-edited, synced
and reset in ways nobody expects their notes to follow.

Markdown for the doc's own reason — the web view already renders it, so a saved note is
readable by the thing that would show it, by any editor, and by `grep`. The cost is that a
note read back is markdown rather than the `Note` tree, so `load_markdown` returns a String
rather than pretending to parse one back: a round trip that is nearly right is worse than one
that is honestly one-way.

Saved ON EVERY CLIP, not on quit. A terminal is closed by closing its window, and a note that
only survives a graceful exit does not survive the way this app is actually ended.

The path is chosen ONCE and rewritten after. A note grows — each clip appends a block and the
whole thing is re-rendered — and saving afresh each time would scatter one note across
`build-notes.md`, `-2`, `-3`: one file per clip, each a prefix of the next. That has its own
test.

Two notes with the same title get a suffix rather than one overwriting the other, and an
unusable title ("???") becomes `note.md` rather than `.md`, which is hidden on every unix and
looks like the save failed.

A write failure is said out loud ("clipped, but not saved: …"). A silent one leaves someone
clipping into a void for an afternoon.

### And the picker / "add to the web" — DONE the same day

Saved notes join the SHIPPED DOCS in `manpage_view`, so the picker is the reader's own list
and "add to the web" is opening the note in the thing that already renders markdown, rather
than a second viewer that renders it slightly differently.

Refreshed when the docs pane draws, not at startup: a note written this session would
otherwise not appear until the app was restarted. Only when the count has CHANGED, because
`set_docs` prunes the reader's navigation stack against the new list and replacing it every
frame would drop the page being read the moment a note was saved.

Names are prefixed `note:` so a note called "readme" cannot displace the shipped README in a
view keyed by name. Titles come from the note's own first heading — the file name is a slug,
and showing "build-notes" when the note says "Build notes" is a worse label than the one it
carries. A note that cannot be read is skipped rather than listed as an entry that fails when
opened.


## DONE — step 2, voice jot mode (2026-09-07)

"Route transcription straight into the active Note as `Block::Prose` (append), instead of
through the command intent parser… this just changes the SINK." So the whole feature is a
decision about where a transcript goes, and `jot::route` is that decision — nothing in it
speaks, listens or writes a file.

THE EXIT PHRASE IS THE HARD PART. A person finishing a thought says "…and that fixed it,
done jotting" in one breath. If the phrase only counted as a whole utterance, that transcript
would be filed as prose ending in "done jotting" and the mode would stay on — so the NEXT
sentence, meant as a command, gets filed too. Handling it only at the end of an utterance is
the difference between a mode you can leave and one you have to remember to leave twice.

So the phrase is matched at the END: the text before it is kept, the mode ends, and an
utterance that is only the phrase files nothing. NOT in the middle — "I was done jotting
notes about the parser" is prose about jotting, and ending from mid-sentence would truncate
the note at a word the speaker used as content.

Several phrasings, because a speaker does not remember one exact form and a mode that will
not exit on "stop jotting" is one you have to kill the app to leave. Punctuation-insensitive,
because whisper punctuates and a mode that stayed on because the transcriber added a full
stop would be maddening in a way nobody would think to report.

The kept prose comes from the ORIGINAL text, so capitals, punctuation and accents survive and
nothing is cut mid-codepoint; `normalise` is for matching only.

Wired at the spoken-phrase box, ahead of `voice_prefix::normalise` — jot mode takes the
transcript before the command parser sees it, which is the sink change the doc describes.
Jotted prose is saved with the same path rule as a clip: the note grows and rewrites its own
file, because a jotted thought lost to a window close is the same failure a clip would be.

`Command::ToggleJotMode` enters it. A mode you could only ENTER by voice is unreachable the
moment the microphone mishears its own trigger. Both edges are announced: a mode that
silently swallows your next sentence into a note is worse than one that never existed.

Not yet covered: the other surfaces a transcript can arrive at. This build routes one — the
spoken-phrase box — and the rest still go to the command parser.


## DONE — step 3's real gap: the clip's SOURCE (2026-09-07)

`Source` has had four variants since v0.1.641 — Web, File, Terminal, Manual — and the app
produced exactly one. Every clip was `Terminal { command, at }` with whatever had run last,
so a paragraph clipped from a documentation page cited itself as `cargo build`.

A citation naming the wrong source is worse than none: the note LOOKS sourced, and the
provenance is the reason to clip rather than retype.

`clip_source::of` decides from what the focused pane is: a web pane cites its url and title,
a file cites its path with an `L12` anchor, the terminal cites its command. Pure, so the
decision is testable without a pane.

Three details that would each be a confident wrong answer:

* A url with NO TITLE falls back to the url. It still cites better than a command that had
  nothing to do with it.
* An anchor of `L0` is dropped. Editors count from 1, and L0 sends someone to the wrong place
  with confidence.
* An unnameable origin is `Manual`, not a Terminal clip with an empty command. `Manual` cites
  as "(typed)", which is true; an empty command cites as an empty backtick pair, which reads
  as a bug.

The web half is wired through `clip_focus` — the file half needs a pane that reports a path
and a selection line, which this build does not yet have, so `Focus::File` is built and
tested and not yet produced.


## PARTLY DONE — step 4, related notes (2026-09-07)

The step names `plan_click` as "the bucketing brain; point it at the note corpus". I did not.
That planner scores UI ELEMENTS — it takes a zone, a colour and a shape — and a note has none
of those, so pointing it at the corpus means filling four fields with blanks and reading the
result as if it meant something. `note_search` scores what a note actually has: a title and a
body.

WHOLE WORDS, not `contains`. Substring matching finds "cargo" inside "supercargo" and "arg"
in every note that mentions a flag; a suggestion list built from substring hits is mostly
noise, and noise reads as "this feature does not work" where an empty list reads as "nothing
related".

The TITLE counts triple. A note called "Linker notes" is about linkers in a way one that
mentions the word once is not — weighted rather than filtered, so a body-only match still
surfaces when nothing better exists.

COVERAGE OVER REPETITION: a note hitting two of your words beats one hitting the same word
four times, because repetition is often just a long note.

Stop words and two-character fragments are not query terms — "the" and "and" appear in every
note and would rank the corpus by length.

Wired at the moment a clip lands, which is when the question answers itself: the text was
just selected and the corpus is on disk. The note being written is excluded — it now CONTAINS
the clip, so it would always be the top hit and always be useless. The toast NAMES the
related note rather than counting them: "3 related notes" is a number you cannot act on, a
title is one you can go and open.

### The word-tap — DONE the same day

A tap on a notebook cell's plain output now asks the corpus about the word under it. The
pointer gives a position, egui's galley turns that into a character index, and
`note_search::word_at` turns that into a word.

Boundaries match `has_word`'s DELIBERATELY. A tap that picked "cargo-build" as one word and
searched with a matcher that splits on the hyphen would find nothing, and that reads as an
empty corpus rather than as two halves of the same feature disagreeing about what a word is.
Underscores do not split — an identifier is one word to someone pointing at it.

A tap on whitespace or punctuation picks NOTHING. Tapping the gap between two words is not a
request to search for either, and choosing the nearer one guesses at something the tap did
not say. Text with accents is handled by byte-aware slicing rather than by character
arithmetic that would split a codepoint.

"No notes about X" is a real answer and is said: a silent tap is indistinguishable from one
that missed.

### The answer leads somewhere — DONE the same day

The tap's result is HELD in a strip above the notebook rather than toasted. A toast names the
related notes and then takes them away; the answer to "what does this belong with" is only
useful if you can go there, and three seconds to read a title and act on it is not an action.

Each hit is a button that opens that note in the docs reader. "No notes about this" is said
plainly, because nothing is a real answer and silence is indistinguishable from a tap that
missed. A × dismisses.

ONE ORDERING BUG WORTH RECORDING: `manpage_view::open` refuses a document the reader does not
hold, and the reader only learned about saved notes when the DOCS pane drew. So a hit clicked
from the notebook did nothing while the identical click from the docs pane worked — a
difference nobody reports as a bug, they just stop using the feature. `refresh_note_docs` is
now called before either.

STILL OPEN: "create card" — searching answers, and there is still no action that makes a new
card from the word.
