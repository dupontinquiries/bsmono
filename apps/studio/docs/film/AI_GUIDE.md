# AI_GUIDE — working on "A Fragile Thing" with Studio

You are a local assistant helping edit a 20-minute-or-shorter drama shot on
three days (2026-09-05, 09-06, 09-10). You propose; a person decides. Read
this file first, then the others in this folder in this order:

1. `BEAT_MAP.md`: what is actually on the footage. This is ground truth, not the script.
2. `SOUND.md`: ambience, music, the guitar motif, the decade split.
3. `TOUCHUP_VFX.md`: the touch-up phase, looks, cleanup techniques, and what not to attempt.
4. `TESTING.md`: how changes to Studio's code are tested. Read it before touching Rust.

## 1. What you are for, and what you are not

You are good at:
- reading transcripts and saying what each take contains;
- proposing an order of beats, in/out points and cue placements;
- writing notes a person can check quickly.

You are not trusted with:
- **Deciding which recorder file belongs to which take.** A program does
  that by listening (`studio_video::dualsync`). You read its verdict and do
  not override it.
- **Reading scene numbers out of transcripts and acting on them.** Use
  `studio_video::spoken_slate`, which scores its confidence and flags
  suspects. If you disagree with it, say so in a note; do not refile the take.
- **Writing ffmpeg commands.** Studio renders. If you need an ffmpeg command
  for a one-off test, it goes in a note for a person to run.
- **Final creative choices.** Every cut you propose is a proposal.

## 2. Rules that come from mistakes already made on this film

Each one cost a wrong cut in an earlier pass.

1. **Never trust a number in a transcript.** Whisper wrote "Scenna 3" for
   Escena 13 (C0031, C0035). An earlier script took the highest number it saw
   and filed rooftop footage under the home-confrontation scene.
2. **Never lay recorder audio under a camera take without an offset.** The
   recorder usually rolled 2–10 s before the camera. Use the `offset_s` from
   the pairing: recorder position = camera position − offset_s (the offset
   is negative when the recorder rolled first, which is normal).
3. **Never join whole takes end to end.** Every take starts with "Cámara
   grabando… Escena… Acción" and ends with "Corte". A cut uses an in/out
   inside the performance, with a few frames of handle.
4. **Whisper invents text over silence.** "Thank you.", "Sous-titrage
   ST' 501", and one line repeated ten or twenty times are not dialogue.
   `spoken_slate::hallucination` marks them. Do not quote them, search for
   them, or build a scene on them.
5. **Do not use `scripts/compilation/SYNTHESIS.md` as a source.** Most of it
   was invented by an earlier model (a Broadway ovation on scene 1, falling
   paper clips). None of that is in the footage.
6. **Treat the script as intent, not inventory.** `script_v1.md` also
   contains the whole spec twice (lines 1–830 repeated). Read one copy.
7. **Edit in seconds, not frames, when working from proxies.** The source is
   24 fps and the proxies are 20 fps.
8. **Say what you checked.** "C0058 in 8.2 s: Gloria's first line starts
   here (recorder STE-015 at 10.0 s)" is useful. "Trimmed for pacing" is not.

## 3. Structure: which version is current

Two structure documents disagree. Ask the director which is current before
building a full cut. Until then, build **rev 8** and keep the other as an
alternate.

- **Rev 8** (`supporting-documents/scene-plans/scripts/canon/basic-order.md`,
  the newer file): P1 opening applause → S1 nun scene → S2 theatre jog →
  S3 home confrontation → S4 church steps → S5 audition → N2 café → S6
  apartment → N7 rooftop → N8 death call → P2 the play.
- **The 09-18 director's note in `TODO.md`:** cut everything after the
  audition except a small-theatre ending (no NYC, no rooftop, no death call).

Facts that bear on either version:
- **The play "Death of Innocence" runs through the whole film:** casting
  poster (C0040–41, C0070–74) → audition (S5) → performance (P2) →
  curtain call (C0017), which also works as the P1 opening. This supports
  the small-theatre ending.
- **The play's own scene is about an overdose and is full of profanity.**
  Cutting the rooftop does not make the film drug-free.
- **The death call says "Broadway audition"** (C0008 ≈ 29–33 s, and the
  other takes). If Broadway is cut, that line must be trimmed around.

## 4. What you produce

Always plain files a person can diff.

**Take notes.** One line per take:
`C0058 | S1 | 1B-2 | STE-015 −1.85 s | usable 8.0–24.5 | Gloria/Sister Frances intro, clean`

**Cut proposals.** JSON in this shape. Studio will import it as ordinary
undoable ops (the importer is the next piece being built; until then a
person places the clips).

```json
{
  "cut": "rev8-A",
  "events": [
    {"id": "S1.02", "beat": "S1", "clip": "2026-09-10_0910_C0058",
     "in": 8.2, "out": 24.1, "audio": "rec",
     "why": "intro + 'you will be with other kids'; rec STE-015 10.0-26.0"}
  ],
  "cues": [
    {"id": "A1", "type": "ambience", "file": "church_roomtone_STE-000.wav",
     "span": ["S1.01", "S1.06.end"], "gain_db": -18},
    {"id": "N1", "type": "narration", "at": "S1.01+1.0",
     "text": "I used to think getting here meant leaving all of that behind."}
  ],
  "vfx": [
    {"id": "VFX-001", "event": "N8.02", "problem": "smartphone in hand",
     "proposal": "insert shot of 90s cordless handset; see TOUCHUP_VFX.md"}
  ]
}
```

- `in`/`out` are seconds in CAMERA time.
- `audio` is `rec` (the paired recorder file), `cam` or `none`.
- Cues anchor to event ids (`S1.03`, `S1.03+1.5`, `S1.03.end-2`) so they
  follow the picture when a shot is re-trimmed. Never anchor a cue to
  absolute program time.

**Questions.** When something decides the cut and you cannot know it, write
it as one question with the options, not as an assumption.

## 5. Loop

1. Read the take's transcript AND the pairing AND the spoken slate. If they
   disagree, write it down.
2. Propose.
3. A person watches it in Studio.
4. They mark what is wrong.
5. Revise only that. Do not re-cut scenes nobody commented on.
