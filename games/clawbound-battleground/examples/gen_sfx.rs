// Render the designed sound effects (src/sfxgen.rs) to .opus under the audio
// root, in the keys the cues play, plus one preview WAV of them all.
//   cargo run --example gen_sfx [audio root] [preview.wav]
use opus_pure::{Application, OggOpusWriter, OpusEncoder, OpusHead, MAX_PACKET_BYTES};

fn opus(pcm: &[f32]) -> Vec<u8> {
    let frame = 960;
    let mut enc = OpusEncoder::new(48_000, 1, Application::Audio).expect("encoder");
    enc.bitrate_bps = 64_000;
    let head = OpusHead::for_encoder(&enc, 48_000);
    let mut out = Vec::new();
    {
        let mut w = OggOpusWriter::new(&mut out, head).expect("writer");
        let mut pk = vec![0u8; MAX_PACKET_BYTES];
        let mut padded = pcm.to_vec();
        padded.resize(pcm.len().div_ceil(frame) * frame + frame, 0.0);
        for block in padded.chunks_exact(frame) {
            let n = enc.encode(block, frame, &mut pk).expect("encode");
            w.write_packet(&pk[..n]).expect("write");
        }
        w.finish().expect("finish");
    }
    out
}

fn main() {
    let root = std::env::args().nth(1).map(std::path::PathBuf::from).unwrap_or_else(|| "assets/audio".into());
    let preview = std::env::args().nth(2);
    let sounds = clawbound::sfxgen::all();
    let mut total = 0usize;
    let mut reel: Vec<f32> = Vec::new();
    let mut last_who = String::new();
    for (key, pcm) in &sounds {
        let path = root.join(format!("{key}.opus"));
        std::fs::create_dir_all(path.parent().unwrap()).expect("dir");
        let bytes = opus(pcm);
        total += bytes.len();
        std::fs::write(&path, bytes).expect("write");
        // the preview: a longer gap between ancestors than between abilities
        let who = key.rsplit_once('/').map(|x| x.0.to_string()).unwrap_or_default();
        let gap = if who != last_who { 0.9 } else { 0.35 };
        last_who = who;
        reel.extend(std::iter::repeat(0.0).take((gap * 48_000.0) as usize));
        reel.extend_from_slice(pcm);
    }
    println!("{} sounds, {:.1} KB of opus under {}", sounds.len(), total as f32 / 1024.0, root.display());
    if let Some(p) = preview {
        std::fs::write(&p, clawbound::brass::wav_bytes(&reel, 48_000)).expect("preview");
        println!("preview: {p} ({:.0} s, in order: {})", reel.len() as f32 / 48_000.0, "each ancestor's abilities 1-4, then the shared sounds");
    }
}
