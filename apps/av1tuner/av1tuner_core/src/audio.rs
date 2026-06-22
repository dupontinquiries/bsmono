//! # Audio — track handling for the final mux
//!
//! Audio doesn't get chunked alongside video in M1. Reason: Opus encodes 
//! ~20× faster than real-time, audio data is tiny vs video, and not 
//! chunking keeps the chunk state machine simple. The audio is encoded 
//! (or passed through) once during the final concat step.
//!
//! Three modes:
//! - **Passthrough**: copy every audio + subtitle + chapter stream from 
//!   source to output unchanged. Fast, zero quality loss, larger output 
//!   if source had lossless or high-bitrate audio.
//! - **Encode**: re-encode audio to Opus. Currently first audio track 
//!   only. Subtitles + chapters still pass through.
//! - **Drop**: video-only output. No audio in the final file.
//!
//! Future (M2+):
//! - Per-track selection (which language to keep, which to encode)
//! - Other audio codecs (AAC for compatibility, FLAC for archival)
//! - Audio normalization, loudness targeting (loudnorm filter)

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AudioMode {
    /// Copy all audio + subtitle + chapter streams from source unchanged.
    Passthrough,
    /// Re-encode the first audio track to Opus. Other audio tracks 
    /// (and subtitles, chapters) are still copied.
    EncodeOpus(OpusSettings),
    /// Strip audio entirely. Video-only output. Still preserves 
    /// subtitles + chapters in the container.
    Drop,
}

impl Default for AudioMode {
    fn default() -> Self {
        // Passthrough is the safe default. The user can switch to 
        // EncodeOpus when they want compression on the audio side.
        AudioMode::Passthrough
    }
}

/// Opus-specific tuning. Fields match libopus's command-line flags.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpusSettings {
    /// Target bitrate in kbps. Opus is excellent at low bitrates:
    /// - 32-64: voice / podcast (consider Application::Voip)
    /// - 96-128: transparent for most content
    /// - 160-256: archive-quality, near-lossless for stereo
    pub bitrate_kbps: u32,
    
    /// libopus application hint. Affects internal psymodel tuning.
    /// - Audio: general music, default for our tool
    /// - Voip: speech, narrow-band optimized
    /// - LowDelay: minimal algorithmic delay, suboptimal quality
    pub application: OpusApplication,
    
    /// VBR (variable bitrate). On by default — gives better quality 
    /// per byte. CBR is rare-niche (streaming with strict bandwidth caps).
    pub vbr: bool,
    
    /// Compression level (0-10). 10 = best quality, slower. Default 10 
    /// since Opus is fast even at level 10.
    pub compression_level: u32,
}

impl Default for OpusSettings {
    fn default() -> Self {
        Self {
            bitrate_kbps: 128,
            application: OpusApplication::Audio,
            vbr: true,
            compression_level: 10,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OpusApplication {
    /// General music / mixed content. Default.
    Audio,
    /// Speech / VoIP — narrow-band, prioritizes intelligibility.
    Voip,
    /// Minimal algorithmic delay. For live streaming. Worse quality 
    /// than Audio at the same bitrate.
    LowDelay,
}

impl OpusApplication {
    /// String for ffmpeg's `-application` flag.
    pub fn as_ffmpeg(&self) -> &'static str {
        match self {
            OpusApplication::Audio => "audio",
            OpusApplication::Voip => "voip",
            OpusApplication::LowDelay => "lowdelay",
        }
    }
}

/// All audio handling decisions for a job. One field on `JobState`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AudioSettings {
    pub mode: AudioMode,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self { mode: AudioMode::default() }
    }
}

impl AudioSettings {
    /// Build the `-c:a ...` and related arguments for the final concat 
    /// ffmpeg invocation. Returns a vec of strings ready to extend onto 
    /// the existing ffmpeg argv. Includes the `-map` selectors.
    ///
    /// The caller's existing argv already has:
    /// - `-f concat -i concat_list.txt`   (video stream, index 0)
    /// - `-i <source>`                    (source for audio, index 1)
    ///
    /// We add the mapping and the codec selection on top.
    pub fn build_ffmpeg_args(&self) -> Vec<String> {
        match &self.mode {
            AudioMode::Passthrough => vec![
                // Video from the concat (input 0). All audio + subtitle 
                // + chapter streams from source (input 1). The `?` makes 
                // them optional so a source-without-audio doesn't error.
                "-map".into(), "0:v".into(),
                "-map".into(), "1:a?".into(),
                "-map".into(), "1:s?".into(),
                "-map".into(), "1:t?".into(),  // attachments (fonts in MKV)
                "-c:v".into(), "copy".into(),
                "-c:a".into(), "copy".into(),
                "-c:s".into(), "copy".into(),
            ],
            AudioMode::EncodeOpus(opus) => {
                let mut args = vec![
                    "-map".into(), "0:v".into(),
                    // Just the first audio track — multi-track 
                    // selection is M2+.
                    "-map".into(), "1:a:0?".into(),
                    "-map".into(), "1:s?".into(),
                    "-c:v".into(), "copy".into(),
                    "-c:a".into(), "libopus".into(),
                    "-b:a".into(), format!("{}k", opus.bitrate_kbps),
                    "-application".into(), opus.application.as_ffmpeg().into(),
                    "-compression_level".into(), opus.compression_level.to_string(),
                    "-c:s".into(), "copy".into(),
                ];
                args.push("-vbr".into());
                args.push(if opus.vbr { "on".into() } else { "off".into() });
                args
            }
            AudioMode::Drop => vec![
                // Video only. Subtitles + chapters still pass through if 
                // present — dropping audio doesn't imply dropping text.
                "-map".into(), "0:v".into(),
                "-map".into(), "1:s?".into(),
                "-c:v".into(), "copy".into(),
                "-c:s".into(), "copy".into(),
                "-an".into(),  // no audio, explicit
            ],
        }
    }
    
    /// True if the mode requires the source file to be available at 
    /// concat time. All current modes do — Passthrough copies all audio 
    /// + subs, EncodeOpus reads audio + copies subs, Drop copies subs.
    pub fn needs_source_at_concat(&self) -> bool {
        true
    }
    
    /// True if we'll be calling libopus. Used by preflight to check 
    /// the ffmpeg build has libopus support.
    pub fn needs_libopus(&self) -> bool {
        matches!(self.mode, AudioMode::EncodeOpus(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn default_is_passthrough() {
        let s = AudioSettings::default();
        assert_eq!(s.mode, AudioMode::Passthrough);
    }
    
    #[test]
    fn passthrough_args_include_all_streams() {
        let args = AudioSettings { mode: AudioMode::Passthrough }
            .build_ffmpeg_args();
        let joined = args.join(" ");
        assert!(joined.contains("-c:a copy"));
        assert!(joined.contains("0:v"));
        assert!(joined.contains("1:a?"));
        assert!(joined.contains("1:s?"));
    }
    
    #[test]
    fn opus_args_include_bitrate_and_application() {
        let opus = OpusSettings {
            bitrate_kbps: 96,
            application: OpusApplication::Voip,
            vbr: true,
            compression_level: 10,
        };
        let args = AudioSettings { mode: AudioMode::EncodeOpus(opus) }
            .build_ffmpeg_args();
        let joined = args.join(" ");
        assert!(joined.contains("libopus"));
        assert!(joined.contains("96k"));
        assert!(joined.contains("voip"));
        assert!(joined.contains("-vbr on"));
    }
    
    #[test]
    fn drop_args_have_no_audio_mapping() {
        let args = AudioSettings { mode: AudioMode::Drop }
            .build_ffmpeg_args();
        let joined = args.join(" ");
        assert!(joined.contains("-an"));
        assert!(!joined.contains("1:a"));
    }
    
    #[test]
    fn needs_libopus_only_for_encode() {
        assert!(!AudioSettings { mode: AudioMode::Passthrough }.needs_libopus());
        assert!(!AudioSettings { mode: AudioMode::Drop }.needs_libopus());
        assert!(AudioSettings { 
            mode: AudioMode::EncodeOpus(OpusSettings::default())
        }.needs_libopus());
    }
    
    #[test]
    fn settings_roundtrip_serde() {
        let s = AudioSettings {
            mode: AudioMode::EncodeOpus(OpusSettings {
                bitrate_kbps: 192, application: OpusApplication::Audio,
                vbr: true, compression_level: 8,
            }),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: AudioSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
