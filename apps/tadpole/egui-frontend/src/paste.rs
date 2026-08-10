//! paste — what arrived, and what to do with it.
//!
//! Drag-and-drop, Ctrl+V and a screenshot all deliver the same three things in
//! practice: **a path, some bytes, or some text**. Routing them is one decision,
//! so it lives in one place rather than three copies drifting apart in the drop
//! handler, the paste handler and the file dialog.
//!
//! ## Bytes are sniffed; the path is only a hint
//!
//! A screenshot pasted from a clipboard has no filename at all, and a file
//! dragged from a browser cache is often called `download`. Meanwhile a `.png`
//! that is really a JPEG is routine. So the bytes decide, and the extension is
//! consulted **only when the bytes are inconclusive** — which happens for real
//! reasons, not just corruption: some clipboards hand over a path-shaped string
//! before the file exists.
//!
//! ## An unsupported drop is a message, not a shrug
//!
//! Dropping a PDF on an OCR app is a reasonable thing to try. Saying "PDF isn't
//! supported yet — export a page as PNG" is a different experience from nothing
//! happening, and the difference costs one enum variant.

use crate::audio::{self, Format as AudioFormat};

/// An image container tadpole can hand to OCR.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageFormat { Png, Jpeg, Webp, Bmp }

impl ImageFormat {
    pub fn label(self) -> &'static str {
        match self {
            ImageFormat::Png => "PNG", ImageFormat::Jpeg => "JPEG",
            ImageFormat::Webp => "WebP", ImageFormat::Bmp => "BMP",
        }
    }
}

/// Sniff an image container.
pub fn sniff_image(b: &[u8]) -> Option<ImageFormat> {
    if b.len() < 12 { return None; }
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some(ImageFormat::Png);
    }
    // JPEG's magic is only two bytes, so the third is checked too: 0xFFD8 alone
    // matches often enough in arbitrary binary to matter.
    if b[0] == 0xFF && b[1] == 0xD8 && b[2] == 0xFF { return Some(ImageFormat::Jpeg); }
    if &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" { return Some(ImageFormat::Webp); }
    if &b[..2] == b"BM" { return Some(ImageFormat::Bmp); }
    None
}

/// What a dropped or pasted thing turned out to be.
#[derive(Clone, PartialEq, Debug)]
pub enum Incoming {
    Image(ImageFormat),
    Audio(AudioFormat),
    /// Plain text — pasted transcript or OCR output coming back in. Useful
    /// rather than an error: the overview pane works on text alone, so a pasted
    /// wall of notes still gets a mindmap.
    Text(String),
    /// Recognised, and explicitly not handled yet. Named so the message can be
    /// specific.
    Unsupported { what: &'static str, hint: &'static str },
    /// Nothing usable.
    Empty,
}

/// Formats worth naming when refused, because a user dropping one has a
/// reasonable expectation and deserves the next step rather than silence.
fn known_unsupported(b: &[u8]) -> Option<(&'static str, &'static str)> {
    if b.len() >= 5 && &b[..5] == b"%PDF-" {
        return Some(("PDF", "export a page as PNG and drop that"));
    }
    if b.len() >= 4 && &b[..4] == b"\x1aE\xdf\xa3" {
        return Some(("Matroska/WebM", "extract the audio to Opus or WAV first"));
    }
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        return Some(("MP4/M4A", "extract the audio to Opus or WAV first"));
    }
    if b.len() >= 3 && (&b[..3] == b"ID3" || (b[0] == 0xFF && (b[1] & 0xE0) == 0xE0)) {
        return Some(("MP3", "convert to Opus, FLAC or WAV"));
    }
    None
}

/// Classify a blob, with an optional filename as a tiebreak.
pub fn classify(bytes: &[u8], name: Option<&str>) -> Incoming {
    if bytes.is_empty() { return Incoming::Empty; }
    if let Some(f) = sniff_image(bytes) { return Incoming::Image(f); }
    if let Some(f) = audio::sniff(bytes) { return Incoming::Audio(f); }
    if let Some((what, hint)) = known_unsupported(bytes) {
        return Incoming::Unsupported { what, hint };
    }
    // Valid UTF-8 with no NULs is text. The NUL check matters: plenty of binary
    // formats are accidentally valid UTF-8 for their first few hundred bytes,
    // and showing a user a mindmap of a truncated ELF header helps nobody.
    if !bytes.contains(&0) {
        if let Ok(s) = std::str::from_utf8(bytes) {
            if !s.trim().is_empty() { return Incoming::Text(s.to_string()); }
        }
    }
    // Only now does the name get a vote, and only to produce a better message.
    if let Some(n) = name {
        let lower = n.to_lowercase();
        for (ext, what) in [(".png", "PNG"), (".jpg", "JPEG"), (".jpeg", "JPEG"),
                            (".wav", "WAV"), (".opus", "Opus"), (".flac", "FLAC")] {
            if lower.ends_with(ext) {
                return Incoming::Unsupported {
                    what,
                    hint: "the file name says one thing and its contents another — it may be \
                           truncated or still downloading",
                };
            }
        }
    }
    Incoming::Unsupported { what: "this file", hint: "try a PNG, JPEG, WAV, Opus or FLAC" }
}

/// A screenshot from the clipboard: bytes with no name at all.
pub fn classify_screenshot(bytes: &[u8]) -> Incoming { classify(bytes, None) }

impl Incoming {
    /// The line shown when something cannot be used.
    pub fn message(&self) -> Option<String> {
        match self {
            Incoming::Unsupported { what, hint } => Some(format!("{what} isn't supported — {hint}")),
            Incoming::Empty => Some("Nothing to paste.".into()),
            _ => None,
        }
    }

    pub fn is_usable(&self) -> bool {
        matches!(self, Incoming::Image(_) | Incoming::Audio(_) | Incoming::Text(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(&[0; 16]);
        v
    }
    fn jpeg() -> Vec<u8> { let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0]; v.extend_from_slice(&[0; 16]); v }
    fn wav() -> Vec<u8> {
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&[0; 4]);
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(&[0; 8]);
        v
    }
    fn webp() -> Vec<u8> {
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&[0; 4]);
        v.extend_from_slice(b"WEBP");
        v.extend_from_slice(&[0; 8]);
        v
    }

    #[test]
    fn a_screenshot_with_no_filename_still_routes_to_ocr() {
        // The case a name-based router gets wrong every time: a clipboard
        // screenshot has no filename at all.
        assert_eq!(classify_screenshot(&png()), Incoming::Image(ImageFormat::Png));
        assert_eq!(classify_screenshot(&jpeg()), Incoming::Image(ImageFormat::Jpeg));
    }

    #[test]
    fn the_bytes_win_when_the_extension_disagrees() {
        // A .png that is really a JPEG is routine, and trusting the name hands
        // a decoder bytes it cannot read.
        assert_eq!(classify(&jpeg(), Some("screenshot.png")),
                   Incoming::Image(ImageFormat::Jpeg));
        assert_eq!(classify(&wav(), Some("recording.flac")),
                   Incoming::Audio(AudioFormat::Wav));
    }

    #[test]
    fn riff_is_disambiguated_between_wav_and_webp() {
        // Both are RIFF and share the first four bytes exactly; only the form
        // at offset 8 separates them, and getting it wrong sends an image to
        // the transcriber.
        assert_eq!(classify(&wav(), None), Incoming::Audio(AudioFormat::Wav));
        assert_eq!(classify(&webp(), None), Incoming::Image(ImageFormat::Webp));
    }

    #[test]
    fn every_supported_audio_container_is_accepted() {
        let mut ogg = b"OggS".to_vec(); ogg.extend_from_slice(&[0; 16]);
        let mut flac = b"fLaC".to_vec(); flac.extend_from_slice(&[0; 16]);
        assert_eq!(classify(&ogg, Some("voice.opus")), Incoming::Audio(AudioFormat::Ogg));
        assert_eq!(classify(&flac, Some("song.flac")), Incoming::Audio(AudioFormat::Flac));
    }

    #[test]
    fn a_pdf_gets_a_next_step_rather_than_silence() {
        // Dropping a PDF on an OCR app is a reasonable thing to try, and the
        // difference between a hint and nothing happening costs one variant.
        let mut pdf = b"%PDF-1.7".to_vec(); pdf.extend_from_slice(&[0; 16]);
        let got = classify(&pdf, Some("paper.pdf"));
        assert_eq!(got, Incoming::Unsupported { what: "PDF",
                   hint: "export a page as PNG and drop that" });
        assert!(got.message().unwrap().contains("export a page as PNG"));
        assert!(!got.is_usable());
    }

    #[test]
    fn mp3_and_mp4_are_named_specifically() {
        // The two most likely audio drops we cannot decode. Naming them beats
        // "unsupported file".
        let mut mp3 = b"ID3".to_vec(); mp3.extend_from_slice(&[0; 20]);
        assert!(matches!(classify(&mp3, None), Incoming::Unsupported { what: "MP3", .. }));
        let mut mp4 = vec![0u8; 4];
        mp4.extend_from_slice(b"ftyp"); mp4.extend_from_slice(&[0; 12]);
        assert!(matches!(classify(&mp4, None), Incoming::Unsupported { what: "MP4/M4A", .. }));
    }

    #[test]
    fn pasted_text_is_usable_rather_than_an_error() {
        // The overview pane works on text alone, so a pasted wall of notes
        // still earns a mindmap.
        match classify(b"some notes about routing tables", None) {
            Incoming::Text(s) => assert!(s.contains("routing")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn binary_that_happens_to_be_valid_utf8_is_not_treated_as_text() {
        // Plenty of binary is accidentally valid UTF-8 for its first few
        // hundred bytes, and a mindmap of a truncated header helps nobody.
        let mut elf = vec![0x7F, b'E', b'L', b'F', 0x02, 0x01, 0x01, 0x00];
        elf.extend_from_slice(&[0u8; 24]);
        assert!(!matches!(classify(&elf, None), Incoming::Text(_)),
                "NULs mark it as binary");
    }

    #[test]
    fn a_truncated_download_says_so_rather_than_blaming_the_format() {
        // The name says PNG and the bytes are not yet a PNG — which usually
        // means the file is still arriving, not that PNG is unsupported.
        let got = classify(&[0u8; 20], Some("shot.png"));
        match got {
            Incoming::Unsupported { what, hint } => {
                assert_eq!(what, "PNG");
                assert!(hint.contains("still downloading"), "hint was {hint}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_empty_paste_says_nothing_to_paste() {
        let got = classify(&[], None);
        assert_eq!(got, Incoming::Empty);
        assert_eq!(got.message().unwrap(), "Nothing to paste.");
    }

    #[test]
    fn a_jpeg_magic_needs_three_bytes_not_two() {
        // 0xFFD8 alone matches often enough in arbitrary binary to matter.
        let mut nearly = vec![0xFF, 0xD8, 0x00];
        nearly.extend_from_slice(&[0u8; 16]);
        assert_eq!(sniff_image(&nearly), None);
    }

    #[test]
    fn a_usable_drop_has_no_message_to_show() {
        assert!(classify(&png(), None).message().is_none());
        assert!(classify(&png(), None).is_usable());
    }
}
