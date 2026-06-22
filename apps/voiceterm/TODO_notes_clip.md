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

## Next pure slice (optional, before wiring)
A clip-kind detector: given a selection + a little context (is it inside a
fenced block? does it look like code — braces/indentation/known-keywords?)
→ ClipKind::Code{lang} vs Text. Pure + testable, removes a guess from the
clip path. Mirrors notebook::classify's conservative style.
