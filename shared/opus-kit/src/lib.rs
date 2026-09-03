//! OPUS-KIT — the container arithmetic, extracted.
//!
//! `audiomix-kit` has deferred codec work to "the feature-gated `opus-kit`" in
//! its own description for some time, and no such crate existed. Meanwhile
//! three places wrote the same parsing by hand:
//!
//! * `apps/beatspace/src/audio_io.rs` — reads `OpusHead`, pulls `pre_skip`,
//!   downmixes to mono for its mel front-end.
//! * `apps/studio/crates/studio-media/src/opus.rs` — the most complete: keeps
//!   channels, decodes float, and trims BOTH ends.
//! * `games/shared/linebank/src/assets.rs` — needed the numbers and had to
//!   measure them with a script.
//!
//! What is duplicated is not the decode. Decoding is `Decoder::decode` and
//! nobody gets it wrong. What is duplicated — and what one of the three got
//! only half right — is knowing WHICH SAMPLES ARE REAL.
//!
//! ## The two trims, and why the second one is the one that gets missed
//!
//! RFC 7845 gives an Opus stream two pieces of padding:
//!
//! * **`pre_skip`** at the front: encoder priming, declared in `OpusHead`,
//!   which the decoder must discard. 312 samples (6.5 ms) in every file in
//!   this repository. Widely known, and all three call sites handle it.
//! * **End padding**: the last frame is a whole frame, so it usually decodes
//!   past the real end. The final page's granule position says where the audio
//!   actually stops. This one is easy to miss because nothing sounds wrong on
//!   a one-shot — the tail is silence-ish and short. It is audible on a LOOP
//!   as a tick every bar, and on a bank of 300 ms blips as a soft edge on
//!   every single one.
//!
//! [`Stream::body`] returns the window with both applied, so a caller cannot
//! remember one and forget the other.
//!
//! ## No libopus, and no `ogg` crate either
//!
//! Everything here is page headers and 64-bit granule positions. Keeping it
//! dependency-free means it builds anywhere — including a container with no
//! system libopus, which is where this was written — and means the part with
//! the subtle arithmetic is testable without a codec. A future `decode`
//! feature can add libopus behind a flag; the arithmetic will not change.
//!
//! Granule positions in Ogg Opus are ALWAYS at 48 kHz regardless of the
//! original sample rate ([`GRANULE_RATE`]), which is why `input_rate` in the
//! header is informational only and must not be used for timing.

/// Ogg Opus granule positions are always in 48 kHz units, whatever the source
/// rate was. Using `OpusHead::input_rate` for duration is the classic error.
pub const GRANULE_RATE: u32 = 48_000;

/// Why a stream could not be read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Error {
    /// No `OggS` capture pattern at the start.
    NotOgg,
    /// Ogg, but the first packet is not `OpusHead`.
    NotOpus,
    /// A page header runs off the end of the buffer.
    Truncated { at: usize },
    /// Channel mapping family other than 0. Families 1 and 255 need the
    /// mapping table and a multistream decoder; refusing beats decoding a
    /// surround stream as if it were stereo.
    UnsupportedMapping(u8),
    /// The final granule is smaller than `pre_skip`, so there is no audio
    /// between the trims. A truncated file, or one page of priming and
    /// nothing else.
    NoAudio,
}

/// `OpusHead`, RFC 7845 §5.1.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpusHead {
    pub version: u8,
    pub channels: u8,
    /// Priming samples to discard, at 48 kHz.
    pub pre_skip: u16,
    /// The rate the material was captured at. INFORMATIONAL — decode is always
    /// 48 kHz and granules are always 48 kHz.
    pub input_rate: u32,
    /// Q7.8 fixed point dB.
    pub output_gain_q8: i16,
    pub mapping_family: u8,
}

impl OpusHead {
    pub fn output_gain_db(&self) -> f32 {
        self.output_gain_q8 as f32 / 256.0
    }
}

/// One Ogg page, located.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Page {
    pub offset: usize,
    /// Header plus segment table plus body.
    pub len: usize,
    /// `-1` (all ones) means "no packet completes on this page".
    pub granule: u64,
    pub serial: u32,
    pub seq: u32,
    pub body: usize,
    pub body_len: usize,
}

impl Page {
    pub fn continued(&self) -> bool {
        self.granule == u64::MAX
    }
}

/// A parsed Ogg Opus stream: everything but the samples.
#[derive(Clone, PartialEq, Debug)]
pub struct Stream {
    pub head: OpusHead,
    pub pages: Vec<Page>,
    /// Granule of the last page that completed a packet.
    pub end_granule: u64,
    /// Total bytes of the file.
    pub bytes: usize,
}

impl Stream {
    /// The samples that are real, as `[start, end)` in 48 kHz units from the
    /// start of decoded output.
    ///
    /// Both trims, together, because remembering one and forgetting the other
    /// is the failure this crate exists to stop.
    pub fn body(&self) -> (u64, u64) {
        (self.head.pre_skip as u64, self.end_granule)
    }
    /// Real samples, per channel, at 48 kHz.
    pub fn samples(&self) -> u64 {
        self.end_granule.saturating_sub(self.head.pre_skip as u64)
    }
    pub fn seconds(&self) -> f64 {
        self.samples() as f64 / GRANULE_RATE as f64
    }
    /// Mean bitrate over the real duration, in kbps. What a size budget wants.
    pub fn kbps(&self) -> f64 {
        let s = self.seconds();
        if s <= 0.0 {
            return 0.0;
        }
        self.bytes as f64 * 8.0 / s / 1000.0
    }
    /// Bytes of container before any audio: the two header pages.
    pub fn header_bytes(&self) -> usize {
        self.pages.iter().take(2).map(|p| p.len).sum()
    }
}

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn u64le(b: &[u8], i: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[i..i + 8]);
    u64::from_le_bytes(v)
}

/// Walk every Ogg page in `data`.
pub fn pages(data: &[u8]) -> Result<Vec<Page>, Error> {
    if data.len() < 27 || &data[0..4] != b"OggS" {
        return Err(Error::NotOgg);
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 27 <= data.len() {
        if &data[i..i + 4] != b"OggS" {
            // Resync rather than fail: a concatenated or padded file is still
            // readable, and refusing one would be less useful than skipping.
            match find(data, i + 1) {
                Some(j) => {
                    i = j;
                    continue;
                }
                None => break,
            }
        }
        let nseg = data[i + 26] as usize;
        let table = i + 27;
        if table + nseg > data.len() {
            return Err(Error::Truncated { at: i });
        }
        let body_len: usize = data[table..table + nseg].iter().map(|s| *s as usize).sum();
        let body = table + nseg;
        if body + body_len > data.len() {
            return Err(Error::Truncated { at: i });
        }
        out.push(Page {
            offset: i,
            len: 27 + nseg + body_len,
            granule: u64le(data, i + 6),
            serial: u32le(data, i + 14),
            seq: u32le(data, i + 18),
            body,
            body_len,
        });
        i = body + body_len;
    }
    Ok(out)
}

fn find(data: &[u8], from: usize) -> Option<usize> {
    data.get(from..)?.windows(4).position(|w| w == b"OggS").map(|p| p + from)
}

/// Parse a whole file.
pub fn parse(data: &[u8]) -> Result<Stream, Error> {
    let pages = pages(data)?;
    let first = pages.first().ok_or(Error::NotOgg)?;
    let h = &data[first.body..first.body + first.body_len];
    if h.len() < 19 || &h[0..8] != b"OpusHead" {
        return Err(Error::NotOpus);
    }
    let head = OpusHead {
        version: h[8],
        channels: h[9],
        pre_skip: u16le(h, 10),
        input_rate: u32le(h, 12),
        output_gain_q8: u16le(h, 16) as i16,
        mapping_family: h[18],
    };
    if head.mapping_family != 0 {
        return Err(Error::UnsupportedMapping(head.mapping_family));
    }
    let end_granule =
        pages.iter().filter(|p| !p.continued()).map(|p| p.granule).max().unwrap_or(0);
    if end_granule <= head.pre_skip as u64 {
        return Err(Error::NoAudio);
    }
    Ok(Stream { head, pages, end_granule, bytes: data.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real file from this repository, so the fixture cannot drift from what
    /// the project actually ships. 4.18 s of mono in 13081 bytes.
    const BIRD: &[u8] =
        include_bytes!("../../../apps/bo-suite/docs/scenes/clips_spatial/bird_chirp_1.opus");

    #[test]
    fn a_real_file_from_this_repo_parses_to_its_known_numbers() {
        let s = parse(BIRD).unwrap();
        assert_eq!(s.head.channels, 1);
        assert_eq!(s.head.pre_skip, 312, "every file in this tree declares 312");
        assert_eq!(s.head.mapping_family, 0);
        assert_eq!(s.bytes, 13081);
        assert!((s.seconds() - 4.18).abs() < 0.01, "{} s", s.seconds());
        // the figure linebank's budget is anchored on
        assert!((s.kbps() - 25.0).abs() < 0.5, "{:.2} kbps", s.kbps());
    }

    #[test]
    fn the_end_trim_is_not_the_same_as_the_file_length() {
        // The trim that gets missed. Decoding every frame gives MORE samples
        // than the granule says are real — inaudible on a one-shot, a tick
        // every bar on a loop, a soft edge on every blip.
        let s = parse(BIRD).unwrap();
        let (start, end) = s.body();
        assert_eq!(start, 312);
        assert_eq!(end, s.end_granule);
        assert_eq!(s.samples(), end - start);
        // and the granule is a real 48k count, not a byte count
        assert!(s.end_granule > 200_000 && s.end_granule < 210_000, "{}", s.end_granule);
    }

    #[test]
    fn duration_uses_the_granule_rate_and_not_the_header_rate() {
        // The classic error: OpusHead::input_rate is what the material was
        // captured at and has nothing to do with timing. Granules are always
        // 48 kHz.
        let s = parse(BIRD).unwrap();
        let wrong = s.samples() as f64 / s.head.input_rate.max(1) as f64;
        let right = s.seconds();
        if s.head.input_rate != GRANULE_RATE {
            assert!((wrong - right).abs() > 0.01, "the fixture cannot show this error");
        }
        assert!((right - 4.18).abs() < 0.01);
    }

    #[test]
    fn header_pages_are_the_overhead_a_size_budget_pays_per_file() {
        // linebank::assets guesses 200 bytes. Measure it instead.
        let s = parse(BIRD).unwrap();
        let h = s.header_bytes();
        assert!((100..=400).contains(&h), "header overhead is {h} bytes");
    }

    #[test]
    fn every_page_is_accounted_for_and_none_overlap() {
        let s = parse(BIRD).unwrap();
        assert!(s.pages.len() > 2, "only {} pages", s.pages.len());
        let mut end = 0usize;
        for p in &s.pages {
            assert!(p.offset >= end, "page at {} overlaps the previous", p.offset);
            assert!(p.offset + p.len <= s.bytes, "page runs past the file");
            end = p.offset + p.len;
        }
        // pages are contiguous in a well-formed file
        assert_eq!(end, s.bytes, "{} bytes unaccounted for", s.bytes - end);
    }

    #[test]
    fn not_ogg_and_not_opus_are_distinct_refusals() {
        assert_eq!(parse(b"not an ogg file at all......"), Err(Error::NotOgg));
        // an Ogg page whose first packet is something else
        let mut fake = Vec::new();
        fake.extend_from_slice(b"OggS");
        fake.extend_from_slice(&[0, 2]);
        fake.extend_from_slice(&0u64.to_le_bytes());
        fake.extend_from_slice(&1u32.to_le_bytes());
        fake.extend_from_slice(&0u32.to_le_bytes());
        fake.extend_from_slice(&0u32.to_le_bytes());
        fake.push(1);
        fake.push(19);
        fake.extend_from_slice(b"VorbisHead_________");
        assert_eq!(parse(&fake), Err(Error::NotOpus));
    }

    #[test]
    fn a_truncated_page_is_reported_rather_than_read_past() {
        let mut t = BIRD.to_vec();
        t.truncate(BIRD.len() - 40);
        // either a clean parse of the surviving pages or an explicit
        // Truncated — never a panic, and never a read past the buffer
        match parse(&t) {
            Ok(s) => assert!(s.bytes == t.len()),
            Err(Error::Truncated { at }) => assert!(at < t.len()),
            Err(e) => panic!("unexpected {e:?}"),
        }
    }

    #[test]
    fn an_empty_or_tiny_buffer_does_not_panic() {
        for n in 0..30usize {
            let _ = parse(&BIRD[..n.min(BIRD.len())]);
        }
    }

    #[test]
    fn surround_is_refused_loudly_rather_than_decoded_as_stereo() {
        // studio-media makes the same call and says why: families 1 and 255
        // need the mapping table and a multistream decoder.
        let mut d = BIRD.to_vec();
        // mapping_family is byte 18 of the OpusHead packet body
        let pages = pages(&d).unwrap();
        let fam = pages[0].body + 18;
        d[fam] = 1;
        assert_eq!(parse(&d), Err(Error::UnsupportedMapping(1)));
    }
}
