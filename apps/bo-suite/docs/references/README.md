# External references used while building bo-suite

## voiceterm/

The user uploaded `voiceterm.zip` as a reference for the whisper
transcription pipeline pattern. We extracted the relevant excerpt
(`voice-whisper-pipeline.rs.excerpt`, ~51 lines around
voiceterm/src/voice.rs:4350-4400) which shows the canonical
shell-out-to-whisper-cli pattern:

  • detect binary (`whisper-cli`, `whisper-cpp`, `nerd-dictation`
    fallbacks)
  • run with `-m <model> -f <audio> --no-prints`
  • parse stdout (one-line text mode) or written file (--output-srt
    mode for our timed-lyrics use case)

bo-controller's `lyrics_transcriber.rs` follows this pattern with
two main differences:

  1. We use `--output-srt --output-file <prefix>` to get a
     time-stamped file rather than a single line of stdout text.
     The SRT lets us render templates (LRC, Markdown with section
     breaks, plain).

  2. We added a content-addressed cache under
     `~/.local/share/bo-controller/lyrics/<sha256-of-path>.lrc` so
     repeat requests skip whisper entirely and re-render from the
     cached LRC.

The full voiceterm source isn't shipped — only the excerpt that
documents the pattern. The rest is unrelated to bo-suite.
