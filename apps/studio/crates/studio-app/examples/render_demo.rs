//! Render the built-in trap kit's demo pattern to a WAV, no window needed.
//!
//!     cargo run -p studio-app --example render_demo --no-default-features -- out.wav [bars]
//!
//! Two jobs. For a person: hear the kit on a machine where the GUI will not
//! open yet. For this repo: the end-to-end proof — pattern → swing placement
//! → drum voices → 808 slides → dithered WAV — that a screenshot of a grid
//! cannot give, produced by exactly the same `render_loop` the audition sink
//! plays and the bounce stores.

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "trap_demo.wav".into());
    let bars: u32 = args.next().and_then(|b| b.parse().ok()).unwrap_or(4);

    let rack = studio_live::Rack::trap_kit();
    let pattern = &rack.patterns[0];
    let tempo = rack.tempo();
    let rate = 48_000;

    // Render each pass separately: `pass` is what makes StepMod::prob and
    // ::every meaningful across bars, so a 4-bar render is 4 passes, not one
    // pass copied 4 times.
    let mut out = Vec::new();
    let mut synth = studio_live::BuiltinSynth { sources: &std::collections::BTreeMap::new() };
    for pass in 0..bars {
        out.extend(studio_live::render_pattern(pattern, &rack, &mut synth, &tempo, rate, pass));
    }
    // Leave headroom for the dither/quantize stage rather than clipping.
    let peak = out.iter().fold(0.0f32, |a, s| a.max(s.abs())).max(1e-9);
    let g = (0.891 / peak).min(1.0); // -1 dBFS ceiling
    for s in &mut out { *s *= g; }

    studio_audio::export::export_wav_dithered(&out, 1, rate, 0xBEA7, std::path::Path::new(&path))
        .expect("wav write");
    eprintln!(
        "wrote {path}: {bars} bars of '{}' at {} bpm, swing {:.0}%, {:.2}s, peak {:.1} dBFS",
        pattern.name, tempo.bpm, tempo.swing * 100.0,
        out.len() as f32 / rate as f32,
        20.0 * (peak * g).log10()
    );
}
