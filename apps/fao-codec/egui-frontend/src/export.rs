//! export — write frames out as a `.fao` file.
//!
//! ## Why this uses the v1 DCT path and not the V2 encoder
//!
//! The Sequence Lab encodes with `v2_encoder::V2Encoder`, and its result type,
//! `fao_codec_lib::FrameResult`, carries NO BITSTREAM. It reports
//! `compressed_size` -- a number -- alongside PSNR and timings, and writes the
//! reconstruction into a caller buffer, but the encoded bytes themselves never
//! leave the encoder. So a V2 encode cannot be saved from here at all. That is a
//! library API gap, not a missing button, and it is the real reason "encode then
//! export" had no path in this frontend.
//!
//! The v1 DCT path does hand back its bytes -- `compress_frame_dct` returns the
//! payload -- and `fao_codec_lib::write_fao_container` wraps payloads in the
//! container. It is also exactly what this frontend's own decoder
//! (`decode::FaoFile`) reads, so an export here is one this app can reopen.
//!
//! When V2 grows a way to return its bitstream, this is where a second writer
//! goes.

use crate::decode::rgb_to_yuv420p;
use fao_codec_lib as fao_v1;

/// Why an export could not be written.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportError {
    /// Nothing to write. An empty `.fao` is a valid-looking file that plays as
    /// nothing, which is worse than refusing.
    NoFrames,
    /// A frame was not `w * h * 3` bytes of RGB. Refused rather than padded or
    /// truncated -- either would write a picture that is not the one given.
    WrongSize { frame: usize, got: usize, want: usize },
    /// Width and height must both be even: the chroma planes are half size in
    /// each direction, and an odd edge has no half.
    OddDimensions { w: usize, h: usize },
    /// The container writer refused.
    Write(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::NoFrames => write!(f, "nothing to export"),
            ExportError::WrongSize { frame, got, want } => {
                write!(f, "frame {frame} is {got} bytes, expected {want}")
            }
            ExportError::OddDimensions { w, h } => {
                write!(f, "{w}x{h}: both sides must be even for 4:2:0")
            }
            ExportError::Write(e) => write!(f, "could not write: {e}"),
        }
    }
}

/// Encode RGB frames as DCT I-frames and write a `.fao` container.
///
/// `quality` is the DCT quality, 1..=100. Every frame is an I-frame: this path
/// has no motion search, so the file is larger than a temporal encode would be
/// -- but every frame decodes on its own, which is what makes it safe to seek.
pub fn write_dct_fao(
    frames: &[Vec<u8>],
    w: usize,
    h: usize,
    fps: f64,
    quality: u8,
) -> Result<Vec<u8>, ExportError> {
    if frames.is_empty() {
        return Err(ExportError::NoFrames);
    }
    if w % 2 != 0 || h % 2 != 0 || w == 0 || h == 0 {
        return Err(ExportError::OddDimensions { w, h });
    }
    let want = w * h * 3;
    let q = quality.clamp(1, 100);
    let mut entries = Vec::with_capacity(frames.len());
    for (i, rgb) in frames.iter().enumerate() {
        if rgb.len() != want {
            return Err(ExportError::WrongSize { frame: i, got: rgb.len(), want });
        }
        let yuv = rgb_to_yuv420p(rgb, w, h);
        let payload = fao_v1::compress_frame_dct(&yuv, w, h, q);
        let n = payload.len();
        // (orig, recon, ftype, enc_bytes, psnr, skip_pct). `orig` empty and the
        // payload in `recon` is how the library's own round-trip test writes it.
        entries.push((Vec::new(), payload, 0u8, n, 0.0f64, 0.0f64));
    }
    let mut out = Vec::new();
    fao_v1::write_fao_container(&mut out, w, h, fps, q as f32 / 100.0, &entries)
        .map_err(|e| ExportError::Write(e.to_string()))?;
    Ok(out)
}

/// Writes a `.fao` one frame at a time, in constant memory.
///
/// ## Why: every existing writer held the whole video
///
/// `write_fao_container` takes every frame as a slice, and `transcode` also
/// returned the finished file as one `Vec<u8>` -- three full copies of a video
/// in memory at once. For an hour of 1080p60 that is terabytes. This writes each
/// frame as it arrives and holds only the running counts.
///
/// ## The index is at the FRONT, and the frame count is not known in advance
///
/// The format puts the index right after the header, so it must be sized before
/// any frame is written -- and for an MKV the frame count is not in the stream
/// header (`nb_frames` is absent; checked with ffprobe). So the caller reserves
/// index slots from duration x rate plus a margin. Unused slots are a harmless
/// gap before the first frame: every reader reads exactly `frame_count` entries
/// and follows each entry's offset, never assuming frames start at the index's
/// end. Over-reserving costs 16 bytes a slot, so be generous; running out is an
/// error, not a silent truncation.
pub struct FaoStreamWriter<W: std::io::Write + std::io::Seek> {
    out: W,
    reserved: u64,
    count: u64,
    next_off: u64,
    total_comp: u64,
}

const HEADER: u64 = 30;
const ENTRY: u64 = 16;

impl<W: std::io::Write + std::io::Seek> FaoStreamWriter<W> {
    /// Start a file. `fps_num / fps_den` is written as given -- pass the source's
    /// own rational (e.g. from ffprobe) or `fao_codec_lib::fps_ratio(fps)`.
    pub fn new(mut out: W, w: usize, h: usize, fps_num: u16, fps_den: u16, quality: u8,
               reserve: u64) -> std::io::Result<Self> {
        use std::io::{Error, ErrorKind, SeekFrom};
        if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 || w > u16::MAX as usize || h > u16::MAX as usize {
            return Err(Error::new(ErrorKind::InvalidInput, format!("{w}x{h}: must be even and <= 65535")));
        }
        if reserve == 0 || reserve > u32::MAX as u64 {
            return Err(Error::new(ErrorKind::InvalidInput, "reserve must be 1..=u32::MAX frames"));
        }
        out.seek(SeekFrom::Start(0))?;
        out.write_all(b"FAO2")?;
        out.write_all(&1u16.to_le_bytes())?;
        out.write_all(&(w as u16).to_le_bytes())?;
        out.write_all(&(h as u16).to_le_bytes())?;
        out.write_all(&fps_num.to_le_bytes())?;
        out.write_all(&fps_den.to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;          // frame_count: patched by `finish`
        out.write_all(&30u16.to_le_bytes())?;         // gop_size, as write_fao_container
        out.write_all(&[quality])?;
        out.write_all(&[0u8])?;                       // flags
        out.write_all(&0u64.to_le_bytes())?;          // total_compressed: patched
        // The reserved index, zeroed, written in chunks rather than one huge Vec.
        let zeros = [0u8; 64 * 1024];
        let mut left = reserve * ENTRY;
        while left > 0 {
            let n = left.min(zeros.len() as u64) as usize;
            out.write_all(&zeros[..n])?;
            left -= n as u64;
        }
        Ok(FaoStreamWriter { out, reserved: reserve, count: 0, next_off: HEADER + reserve * ENTRY, total_comp: 0 })
    }

    /// Append one frame: its 13-byte header and payload, then its index entry.
    /// `psnr` is stored as psnr x 10, as `write_fao_container` does.
    pub fn push(&mut self, payload: &[u8], frame_type: u8, psnr: f64) -> std::io::Result<()> {
        use std::io::{Error, ErrorKind, SeekFrom};
        if self.count >= self.reserved {
            return Err(Error::new(ErrorKind::Other,
                format!("index full: {} slots reserved -- reserve more", self.reserved)));
        }
        let len = payload.len();
        if len > u32::MAX as usize - 13 {
            return Err(Error::new(ErrorKind::InvalidInput, "frame over 4 GB"));
        }
        let at = self.next_off;
        self.out.seek(SeekFrom::Start(at))?;
        // Byte-for-byte the frame layout `write_fao_container` emits.
        self.out.write_all(&[frame_type])?;
        self.out.write_all(&(len as u32).to_le_bytes())?;       // compressed_size
        self.out.write_all(&(len as u32).to_le_bytes())?;       // recon_size
        self.out.write_all(&((psnr * 10.0) as u16).to_le_bytes())?;
        self.out.write_all(&[0u8])?;                              // skip_pct
        self.out.write_all(&[0u8])?;                              // gmx
        self.out.write_all(payload)?;
        self.out.seek(SeekFrom::Start(HEADER + self.count * ENTRY))?;
        self.out.write_all(&at.to_le_bytes())?;
        self.out.write_all(&((13 + len) as u32).to_le_bytes())?;
        self.out.write_all(&(len as u32).to_le_bytes())?;
        self.next_off = at + 13 + len as u64;
        self.count += 1;
        self.total_comp += len as u64;
        Ok(())
    }

    pub fn frames(&self) -> u64 { self.count }

    /// Patch the frame count and total into the header, and hand back the writer.
    pub fn finish(mut self) -> std::io::Result<W> {
        use std::io::SeekFrom;
        self.out.seek(SeekFrom::Start(14))?;
        self.out.write_all(&(self.count as u32).to_le_bytes())?;
        self.out.seek(SeekFrom::Start(22))?;
        self.out.write_all(&self.total_comp.to_le_bytes())?;
        self.out.seek(SeekFrom::Start(self.next_off))?;
        self.out.flush()?;
        Ok(self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::{yuv_to_rgb, FaoFile};

    fn gradient(w: usize, h: usize, phase: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                v.push((x * 255 / w.max(1)) as u8 ^ phase);
                v.push((y * 255 / h.max(1)) as u8);
                v.push(phase);
            }
        }
        v
    }

    /// THE ROUND TRIP. What is exported must reopen in this app's own decoder,
    /// with the right shape, and every frame must decode to exactly what the
    /// codec produces for it -- not approximately, since both sides run the
    /// same deterministic DCT.
    #[test]
    fn an_export_reopens_and_decodes_to_the_codecs_answer() {
        let (w, h) = (32usize, 16usize);
        let frames = vec![gradient(w, h, 10), gradient(w, h, 90), gradient(w, h, 200)];
        let bytes = write_dct_fao(&frames, w, h, 30.0, 80).expect("writes");
        let f = FaoFile::parse(bytes).expect("reopens");
        assert_eq!((f.width(), f.height(), f.frame_count()), (w, h, 3));
        for (i, rgb) in frames.iter().enumerate() {
            let yuv = rgb_to_yuv420p(rgb, w, h);
            let payload = fao_v1::compress_frame_dct(&yuv, w, h, 80);
            let want = yuv_to_rgb(&fao_v1::decompress_frame_dct(&payload, w, h).unwrap(), w, h);
            assert_eq!(f.decode_rgb(i).unwrap(), want, "frame {i}");
        }
    }

    /// Frames stay IN ORDER and DISTINCT -- an exporter that wrote the first
    /// frame three times would pass a shape check.
    #[test]
    fn frames_keep_their_order() {
        let (w, h) = (32usize, 16usize);
        let frames = vec![gradient(w, h, 0), gradient(w, h, 255)];
        let f = FaoFile::parse(write_dct_fao(&frames, w, h, 24.0, 90).unwrap()).unwrap();
        assert_ne!(f.decode_rgb(0).unwrap(), f.decode_rgb(1).unwrap());
    }

    /// Higher quality must actually mean closer to the source.
    #[test]
    fn quality_changes_fidelity() {
        let (w, h) = (32usize, 16usize);
        let src = gradient(w, h, 77);
        let err = |q: u8| -> u64 {
            let f = FaoFile::parse(write_dct_fao(&[src.clone()], w, h, 30.0, q).unwrap()).unwrap();
            f.decode_rgb(0).unwrap().iter().zip(&src)
                .map(|(a, b)| (*a as i64 - *b as i64).unsigned_abs()).sum()
        };
        assert!(err(95) < err(10), "q95 {} should beat q10 {}", err(95), err(10));
    }

    fn payloads(frames: &[Vec<u8>], w: usize, h: usize, q: u8) -> Vec<Vec<u8>> {
        frames.iter().map(|rgb| fao_v1::compress_frame_dct(&rgb_to_yuv420p(rgb, w, h), w, h, q)).collect()
    }

    /// BYTE-IDENTICAL to the library writer when the reserve is exact. This is
    /// the proof the streaming writer produces the real format, not a lookalike.
    #[test]
    fn streaming_output_is_byte_identical_to_the_library_writer() {
        let (w, h) = (32usize, 16usize);
        let frames = vec![gradient(w, h, 10), gradient(w, h, 90), gradient(w, h, 200)];
        let ps = payloads(&frames, w, h, 80);
        let entries: Vec<_> = ps.iter().map(|p| (Vec::new(), p.clone(), b'D', p.len(), 0.0f64, 0.0f64)).collect();
        let mut lib = Vec::new();
        fao_v1::write_fao_container(&mut lib, w, h, 30.0, 0.8, &entries).unwrap();
        let (n, d) = fao_v1::fps_ratio(30.0);
        let mut sw = FaoStreamWriter::new(std::io::Cursor::new(Vec::new()), w, h, n, d, 80, 3).unwrap();
        for p in &ps { sw.push(p, b'D', 0.0).unwrap(); }
        let streamed = sw.finish().unwrap().into_inner();
        assert_eq!(streamed, lib, "streamed file differs from write_fao_container's");
    }

    /// Over-reserved: a gap before the first frame, and every reader still
    /// decodes it identically -- in memory AND streamed.
    #[test]
    fn an_over_reserved_index_decodes_identically() {
        let (w, h) = (32usize, 16usize);
        let frames = vec![gradient(w, h, 1), gradient(w, h, 128), gradient(w, h, 250)];
        let ps = payloads(&frames, w, h, 80);
        let exact = write_dct_fao(&frames, w, h, 30.0, 80).unwrap();
        let (n, d) = fao_v1::fps_ratio(30.0);
        let mut sw = FaoStreamWriter::new(std::io::Cursor::new(Vec::new()), w, h, n, d, 80, 500).unwrap();
        for p in &ps { sw.push(p, b'D', 0.0).unwrap(); }
        let gappy = sw.finish().unwrap().into_inner();
        assert!(gappy.len() > exact.len() + 400 * 16, "the gap is really there");
        let a = FaoFile::parse(exact).unwrap();
        let b = FaoFile::parse(gappy.clone()).unwrap();
        let c = FaoFile::open_stream(Box::new(std::io::Cursor::new(gappy))).unwrap();
        assert_eq!(b.frame_count(), 3);
        for i in 0..3 {
            assert_eq!(b.decode_rgb(i).unwrap(), a.decode_rgb(i).unwrap(), "in-memory frame {i}");
            assert_eq!(c.decode_rgb(i).unwrap(), a.decode_rgb(i).unwrap(), "streamed frame {i}");
        }
    }

    /// A 144 fps header survives -- the rate game captures actually use.
    #[test]
    fn a_high_refresh_rate_is_stored_exactly() {
        let (n, d) = fao_v1::fps_ratio(144.0);
        let mut sw = FaoStreamWriter::new(std::io::Cursor::new(Vec::new()), 16, 16, n, d, 80, 1).unwrap();
        sw.push(&fao_v1::compress_frame_dct(&rgb_to_yuv420p(&vec![0u8; 16*16*3], 16, 16), 16, 16, 80), b'D', 0.0).unwrap();
        let f = FaoFile::parse(sw.finish().unwrap().into_inner()).unwrap();
        assert!((f.fps() - 144.0).abs() < 1e-9, "{}", f.fps());
    }

    /// Running out of reserved slots is an error, not a silent truncation.
    #[test]
    fn exceeding_the_reserve_is_an_error() {
        let mut sw = FaoStreamWriter::new(std::io::Cursor::new(Vec::new()), 16, 16, 30000, 1000, 80, 1).unwrap();
        sw.push(b"D1", b'D', 0.0).unwrap();
        assert!(sw.push(b"D2", b'D', 0.0).unwrap_err().to_string().contains("index full"));
        assert!(FaoStreamWriter::new(std::io::Cursor::new(Vec::new()), 15, 16, 1, 1, 80, 1).is_err(), "odd width");
    }

    #[test]
    fn empty_and_malformed_input_is_refused() {
        assert_eq!(write_dct_fao(&[], 32, 16, 30.0, 80), Err(ExportError::NoFrames));
        assert!(matches!(write_dct_fao(&[vec![0; 10]], 32, 16, 30.0, 80),
                         Err(ExportError::WrongSize { frame: 0, got: 10, .. })));
        assert!(matches!(write_dct_fao(&[vec![0; 33 * 16 * 3]], 33, 16, 30.0, 80),
                         Err(ExportError::OddDimensions { .. })));
    }

}
