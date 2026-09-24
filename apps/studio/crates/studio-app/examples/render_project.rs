//! Render a Studio project's full mix to raw 48 kHz stereo f32 (little-endian,
//! interleaved), the way Studio hears it: file clips, pattern clips from the
//! rack, the busses. Encode it with ffmpeg:
//!     cargo run -p studio-app --example render_project -- song.studio out.f32
//!     ffmpeg -f f32le -ar 48000 -ac 2 -i out.f32 -c:a libopus -b:a 128k out.opus
//! Rendered in 5 s chunks through one effect cache (the export's way). Prints
//! the peak so a render over full scale is said, not heard first.
//!
//! For bisecting a fault (a crackle, say), after the two paths:
//!   --from S --to S          render only that window
//!   --bypass kind,kind       remove those effect kinds from every bus and clip
//!   --mute-synths            mute the rack's stand-in synths (Sine / Sub)
use std::collections::BTreeMap;
use std::io::Write;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = std::path::PathBuf::from(&a[1]);
    let opt = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    let bypass: Vec<String> = opt("--bypass").map(|v| v.split(',').map(str::to_string).collect()).unwrap_or_default();
    let save = studio_collab::file::SaveFile::decode(&std::fs::read(&path).unwrap()).expect("project");
    let mut rack = save.rack.clone().unwrap_or_else(studio_live::Rack::trap_kit);
    let mut project = save.to_project();
    for b in project.busses.values_mut() { b.chain.retain(|e| !bypass.contains(&e.kind)); }
    for c in project.clips.values_mut() { c.effects.retain(|e| !bypass.contains(&e.kind)); }
    if a.iter().any(|x| x == "--mute-synths") {
        for ch in rack.channels.iter_mut() {
            if matches!(ch.kind, studio_live::SynthKind::Sine { .. } | studio_live::SynthKind::Sub { .. }) { ch.muted = true; }
        }
    }
    let base = path.parent().unwrap();
    let mut audio: BTreeMap<studio_core::SourceId, studio_audio::SourceAudio> = BTreeMap::new();
    for s in project.sources.values().filter(|s| !s.is_generator()) {
        if let Ok(x) = studio_media::decode_file(&base.join(&s.path)) { audio.insert(s.id, x); }
        else { eprintln!("cannot decode {}", s.path); }
    }
    let mut stamps = BTreeMap::new();
    studio_app::pattern_clips::refresh(&project, &rack, &mut audio, &mut stamps);
    let fades: BTreeMap<studio_core::ClipId, studio_audio::Fades> = project.fades.iter()
        .map(|(id, (i, o))| (*id, studio_audio::Fades { fade_in: *i, fade_out: *o })).collect();
    let rate = 48_000;
    let end = opt("--to").and_then(|v| v.parse().ok()).map(studio_core::Ticks::from_seconds).unwrap_or(project.duration());
    let mut out = std::io::BufWriter::new(std::fs::File::create(&a[2]).unwrap());
    let mut cache = studio_audio::EffectCache::new();
    let start = opt("--from").and_then(|v| v.parse().ok()).map(studio_core::Ticks::from_seconds).unwrap_or(studio_core::Ticks::ZERO);
    let (mut t, step, mut peak) = (start, studio_core::Ticks::from_seconds(5.0), 0.0f32);
    while t < end {
        let to = t.saturating_add(step).min(end);
        let mix = studio_audio::mixdown_with_cache(&project, &audio, &fades, t, to, rate, &[], &[], &mut cache);
        for x in &mix { peak = peak.max(x.abs()); out.write_all(&x.to_le_bytes()).unwrap(); }
        t = to;
        eprint!("\r{:.0} / {:.0} s", t.to_seconds(), end.to_seconds());
    }
    eprintln!("\npeak {:.2} ({:+.1} dBFS)", peak, 20.0 * peak.max(1e-9).log10());
}
