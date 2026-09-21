# cartoon-fragile

A synthetic, shapes-only stand-in for the "A Fragile Thing" rooftop scene (N7), so
Studio's film tools can be exercised without the real footage. Under 600 KB.

- `A001_wide.mp4`, `A002_close_left.mp4`, `A003_close_right.mp4` — 320x180, 24 fps.
  Cameras roll at scene 0.0 / 0.4 / 0.9 s. The wide shakes at 7.5–8.5 s.
- `R001_recorder.flac` — rolled at scene −1.5 s; the clean dialogue.
- `*.srt` — each camera's transcript, in that file's own time.
- `fragile-cartoon.studio` — the angles as a camera group, the recorder (muted),
  a Mix bus feeding a silent Control channel, markers.

Slate clap at scene 1.0 s. Lines: "Max" 2.0–4.2, "Jude" 5.0–7.0 (with a hand gesture),
"Max" 8.0–9.2, a woman 10.5–12.5. Each close mic is louder for its own side.

Regenerate: `cargo run -p studio-app --example cartoon_fragile -- fixtures/cartoon-fragile`
Checked by `crates/studio-app/src/fixture_tests.rs`.
