// Write the intro fanfare to a WAV, to listen to it outside the game.
//   cargo run --example fanfare_wav [out.wav]
fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "fanfare.wav".into());
    let sr = 44100u32;
    let s = clawbound::brass::Fanfare::render_all(sr as f32);
    std::fs::write(&path, clawbound::brass::wav_bytes(&s, sr)).expect("write");
    println!("{path}: {:.2} s, peak {:.2}", s.len() as f32 / sr as f32, s.iter().fold(0.0f32, |m, x| m.max(x.abs())));
}
