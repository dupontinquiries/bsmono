//! tts_cli.rs — build the exact Piper text-to-speech command line for VOICE OUTPUT.
//!
//! The output half of the voice pipeline: whisper (`voice.rs`) turns speech into text; this turns the
//! model's reply back into speech. Piper reads the text on STDIN and writes a wav, so the argv here does
//! NOT contain the text — `stdin_text()` returns what to pipe. Pure builder like `mediagen`/`vision_cli`:
//! the flags the app runs are the flags the tests pin; the `Command::spawn` + stdin write is a thin layer.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// A single speech-synthesis invocation. `length_scale` < 1 speeds speech up, > 1 slows it; `speaker` picks
/// a voice in a multi-speaker model.
#[derive(Clone, Debug)]
pub struct Tts {
    pub bin: PathBuf,       // e.g. build/bin/piper
    pub model: PathBuf,     // voice model, e.g. ~/models/voices/en_US-amy-medium.onnx
    pub out: PathBuf,       // wav to write
    pub text: String,       // spoken text (piped to stdin, not argv)
    pub speaker: Option<u32>,
    pub length_scale: f32,  // 1.0 = normal cadence
    pub extra: Vec<String>,
}

impl Tts {
    /// Speak `text` with `voice_file` under `models/voices/`. Piper expects the matching `.onnx.json`
    /// config beside the `.onnx`, which it finds automatically — so only the `.onnx` is named here.
    pub fn speak(bin: PathBuf, models: &Path, voice_file: &str, text: impl Into<String>, out: PathBuf) -> Self {
        Tts {
            bin,
            model: models.join("voices").join(voice_file),
            out,
            text: text.into(),
            speaker: None,
            length_scale: 1.0,
            extra: Vec::new(),
        }
    }

    pub fn with_speaker(mut self, s: u32) -> Self { self.speaker = Some(s); self }
    pub fn with_rate(mut self, length_scale: f32) -> Self { self.length_scale = length_scale; self }

    /// The command line (text is NOT here — it goes to stdin):
    ///   piper -m MODEL -f OUT [--length_scale L] [--speaker S]
    pub fn argv(&self) -> Vec<String> {
        let p = |pb: &Path| pb.to_string_lossy().into_owned();
        let mut v = vec![
            p(&self.bin),
            "-m".into(), p(&self.model),
            "-f".into(), p(&self.out),
        ];
        if (self.length_scale - 1.0).abs() > f32::EPSILON {
            v.push("--length_scale".into());
            v.push(fmt_num(self.length_scale));
        }
        if let Some(s) = self.speaker {
            v.push("--speaker".into());
            v.push(s.to_string());
        }
        v.extend(self.extra.iter().cloned());
        v
    }

    /// What to write to the process's stdin. Piper synthesizes one utterance per line, so newlines in the
    /// text become sentence breaks; we normalize CRLF and trim to avoid a trailing empty utterance.
    pub fn stdin_text(&self) -> String {
        self.text.replace("\r\n", "\n").trim().to_string()
    }
}

fn fmt_num(x: f32) -> String {
    if x.fract() == 0.0 { format!("{}", x as i64) } else { format!("{x}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tts() -> Tts {
        Tts::speak(PathBuf::from("build/bin/piper"), Path::new("/models"), "en_US-amy-medium.onnx",
                   "Hello there.", PathBuf::from("/tmp/reply.wav"))
    }

    #[test]
    fn argv_pins_piper_and_excludes_text() {
        let v = tts().argv();
        let j = v.join(" ");
        assert_eq!(v[0], "build/bin/piper");
        assert!(j.contains("-m /models/voices/en_US-amy-medium.onnx"));
        assert!(j.contains("-f /tmp/reply.wav"));
        assert!(!j.contains("Hello there"), "text goes to stdin, never argv");
        // default cadence + no speaker ⇒ neither flag present
        assert!(!j.contains("--length_scale") && !j.contains("--speaker"));
    }

    #[test]
    fn rate_and_speaker_flags() {
        let v = tts().with_rate(0.9).with_speaker(3).argv();
        let j = v.join(" ");
        assert!(j.contains("--length_scale 0.9"));
        assert!(j.contains("--speaker 3"));
    }

    #[test]
    fn stdin_text_is_normalized() {
        let t = Tts::speak(PathBuf::from("piper"), Path::new("/m"), "v.onnx", "  line one\r\nline two  \n", PathBuf::from("o.wav"));
        assert_eq!(t.stdin_text(), "line one\nline two");
    }

    #[test]
    fn integral_rate_formats_without_decimal() {
        let v = tts().with_rate(2.0).argv();
        assert!(v.join(" ").contains("--length_scale 2"));
    }
}
