# Voice test harness

Automated regression tests for voice → intent routing.

## Quick start

1. Install whisper model (one-time):
   ```sh
   ./setup-voice.sh   # downloads ggml-base.en.bin to ~/.config/web2qml/models/
   ```

2. Record a test case:
   ```sh
   cd tests/voice/cases
   arecord -f S16_LE -r 16000 -c 1 -d 4 connect_smokey.wav
   # (speak the phrase, ctrl-C to stop)
   ```

3. Write expectations alongside it:
   ```sh
   cat > connect_smokey.expect <<EOF
   transcript_contains: connect to smokey
   top_intent: term_cmd
   top_command_contains: bluetoothctl connect
   min_score: 0.7
   context_tab: terminal
   EOF
   ```

4. (Optional) Seed terminal context:
   ```sh
   cat > connect_smokey.context.txt <<EOF
   bluetoothctl devices
   Device 50:1E:2D:5B:E6:EF Smokey
   Device 00:09:A7:8A:94:64 Beoplay H95
   EOF
   ```

5. Run:
   ```sh
   cargo test --test voice_cases --features voice -- --nocapture
   ```

## Expectations file format

Each `<case>.expect` is plain key:value text. Empty fields are not checked.
Recognized keys:

- `transcript_contains` — case-insensitive substring of whisper output
- `top_intent` — kind of the top candidate (`term_cmd`, `search`, `nav`, etc.)
- `top_command_contains` — substring of the top candidate's command
- `top_command_equals` — exact string match
- `min_score` — float, top candidate's score must be >= this
- `not_intent` — top candidate must NOT be of this kind
- `context_tab` — `browser` or `terminal`, sets context before parsing

Lines starting with `#` are comments.

## What this catches

- Whisper transcription regressions (model swap, prompt-bias change)
- Voice → intent parsing regressions (rule changes)
- Composition / entity-extraction regressions (after changing extractors)

## What this does NOT catch

- GUI rendering
- Focus / keyboard shortcut routing
- PTY / terminal emulator behavior
- VAD / chime / mic device issues

For those, you need the live app.

## Recording tips

- Quiet environment helps but isn't required — VAD is in the live app, not
  the test harness; we transcribe the whole WAV
- 4 seconds is plenty for short phrases. Trim trailing silence with
  `sox in.wav out.wav silence -l 1 0.1 1% -1 1.0 1%` if VAD cliffs matter
- 16 kHz mono 16-bit PCM is required. Other formats will be rejected.
