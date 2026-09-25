// Hear the spaces: one ability sound through the real mixer in each place
// (dry, jungle, a tight gorge, the open canyon, a cave), one after another.
//   cargo run --example space_demo [out.wav]
use clawbound::mixer::{self, Bus, Cmd, Play};
use clawbound::reverb::Space;

fn main() {
    let out_path = std::env::args().nth(1).unwrap_or_else(|| "space_demo.wav".into());
    let def = clawbound::gods::ROSTER.iter().find(|d| d.name == "Goldenmane").unwrap();
    let roar = clawbound::sfxgen::ability(def, 3);
    let pcm: std::sync::Arc<[f32]> = roar.into();
    let spaces = [
        ("dry", Space::DRY),
        ("jungle", Space { room: 0.45, damp: 0.85, wet: 0.16, echo_time: 0.0, echo_fb: 0.0, echo_wet: 0.0 }),
        ("tight gorge", Space { room: 0.88, damp: 0.25, wet: 0.33, echo_time: 0.06, echo_fb: 0.29, echo_wet: 0.31 }),
        ("open canyon", Space { room: 0.72, damp: 0.25, wet: 0.25, echo_time: 0.36, echo_fb: 0.47, echo_wet: 0.19 }),
        ("cave", Space { room: 0.55, damp: 0.2, wet: 0.5, echo_time: 0.05, echo_fb: 0.25, echo_wet: 0.1 }),
    ];
    let mut reel: Vec<f32> = Vec::new();
    for (name, sp) in spaces {
        let (mut h, mut m) = mixer::new_at(48_000);
        h.send(Cmd::Load { id: 0, pcm: pcm.clone(), channels: 1 });
        h.send(Cmd::BusGain(Bus::Sfx, 1.0));
        h.send(Cmd::Space(sp));
        let mut block = vec![0.0f32; 2 * 512];
        for _ in 0..20 {
            block.iter_mut().for_each(|x| *x = 0.0);
            m.render_add(&mut block, 2); // let the space settle
        }
        h.play(Play { clip: 0, bus: Bus::Sfx, gain: 0.8, pan: 0.0, priority: 5, looped: false, speaker: None });
        for _ in 0..(2.6 * 48_000.0 / 512.0) as usize {
            block.iter_mut().for_each(|x| *x = 0.0);
            m.render_add(&mut block, 2);
            // mono for the WAV writer: average the ears
            reel.extend(block.chunks(2).map(|c| (c[0] + c[1]) * 0.5));
        }
        println!("{name}");
    }
    let peak = reel.iter().fold(0.0f32, |a, x| a.max(x.abs())).max(1e-6);
    let reel: Vec<f32> = reel.iter().map(|x| x / peak * 0.9).collect();
    std::fs::write(&out_path, clawbound::brass::wav_bytes(&reel, 48_000)).expect("write");
    println!("-> {out_path} ({:.1} s)", reel.len() as f32 / 48_000.0);
}
