# Spatial music mixer — playback wiring (compile-on-end)

The PURE mixer is `sim::spatial_audio` (verified): `mix_gains(listener, stems,
falloff) -> per-stem gains`, plus `menu_stem_layout()` placing music/bass/drums in
space. Walking/flying between the points crossfades the mix by position.

The stems are the track-separated opus files that came back in results.zip
(docs/incoming-r94/new-docs/track-separated/): `koYopper (music|bass|drums).opus`.

## Playback shell (NOT built here — needs an audio device + system libs)
Add behind an OPTIONAL `audio` feature so the default/headless build needs nothing:
```toml
# Cargo.toml
[features]
audio = ["dep:rodio"]
[dependencies]
rodio = { version = "0.19", optional = true }   # pulls cpal → needs libasound2-dev on Linux
```
```rust
// src/audio/playback.rs  (#[cfg(feature = "audio")])
// One rodio Sink per stem; decode the opus once, loop it, and every frame set each
// sink's volume from the spatial mixer:
let (_stream, handle) = rodio::OutputStream::try_default()?;
let stems = sim::spatial_audio::menu_stem_layout();
let mut sinks = Vec::new();
for s in stems { let sink = rodio::Sink::try_new(&handle)?; sink.append(decoded_loop(s.name)?); sinks.push(sink); }
// per frame (menu camera = listener):
let gains = sim::spatial_audio::mix_gains(listener_pos, &stems, 6.0);
for (sink, g) in sinks.iter().zip(gains) { sink.set_volume(g); }
```
NOTE: rodio decodes wav/flac/mp3/ogg-vorbis natively; for OPUS either transcode the
stems to ogg/flac at build/asset time, or add a `symphonia` (opus) decode source.
Transcoding to .ogg is the lowest-friction path.

VERIFY on-device: with `--features audio`, move the menu camera between the three
stem points — the bass/drums/theme balance should follow position. The gain math is
already unit-tested headless.
