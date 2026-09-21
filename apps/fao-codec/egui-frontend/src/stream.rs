//! stream — read a `.fao` without holding it in memory.
//!
//! ## Why: the player loaded the whole file
//!
//! `player.rs` does `std::fs::read(path)` and `decode::FaoFile::parse` keeps the
//! result as one `Vec<u8>`. For a 600-hour, 60 fps video -- 129.6 million
//! frames -- that is 0.27 TB even at 2 KB a frame, and 2.65 TB at a more usual
//! 20 KB. The Fairphone has 8 GB. So the FORMAT supported long videos and the
//! player could never open one.
//!
//! The format itself is fine, and this was checked rather than assumed:
//! `width`/`height` are u16 (to 65,535 -- past 8K and 16K), `frame_count` is u32
//! (4.29 billion frames, ~19,900 hours at 60 fps), frame offsets are u64, and
//! NO timestamps are stored -- time is frame / fps. So the classic "number too
//! big" at 596.5 hours, where a signed 32-bit millisecond count overflows, does
//! not exist in the container. It only exists in code that computes it; see
//! [`frame_time_ms`].
//!
//! ## Constant memory
//!
//! Index entries are fixed-size (16 bytes, `#[repr(C)]`) directly after the
//! 30-byte header, so entry `i` lives at `30 + i * 16`. The reader seeks there,
//! then seeks to that frame's payload. It holds one header, one entry and one
//! frame -- however long the file.
//!
//! ## No usize on the way to the disk
//!
//! Every offset stays u64 and reaches the file as `SeekFrom::Start(u64)`. The
//! library's `read_fao_frame` does `entry.offset as usize`, which TRUNCATES any
//! offset past 4 GB on a 32-bit target -- armv7 postmarketOS devices among them.

use std::io::{self, Read, Seek, SeekFrom};

use fao_codec_lib as fao_v1;

/// On-disk header size.
pub const HEADER_LEN: u64 = 30;
/// On-disk index entry size: offset u64, size u32, compressed_size u32.
pub const ENTRY_LEN: u64 = 16;
/// On-disk per-frame header size.
///
/// 13 bytes: type, compressed_size u32, recon_size u32, psnr_x10 u16, skip_pct,
/// gmx. NOTE the library's `read_fao_frame` declares 13 but also reads a 14th
/// field, `gmy`, from byte 13 -- which is the first byte of the PAYLOAD, i.e.
/// the codec marker. So every `FaoFrameHeader::gmy` it returns is the marker
/// byte ('D' reads as 68). The payload offset is right; that one field is not.
pub const FRAME_HEADER_LEN: u64 = 13;

/// One index entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub offset: u64,
    pub size: u32,
    pub compressed_size: u32,
}

/// A `.fao` opened for random access, holding nothing but its header.
pub struct FaoReader<R> {
    src: R,
    pub width: u16,
    pub height: u16,
    pub fps_num: u16,
    pub fps_den: u16,
    pub frame_count: u64,
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

impl<R: Read + Seek> FaoReader<R> {
    /// Read the header. Nothing else is read until asked for.
    pub fn open(mut src: R) -> io::Result<Self> {
        // 32 bytes, not 30: `read_fao_header` checks
        // `size_of::<FaoFileHeader>()`, which is 32 with `#[repr(C)]` padding,
        // though it READS the 30-byte on-disk layout. Handing it 32 reuses the
        // canonical parser instead of copying it.
        let mut buf = [0u8; 32];
        src.seek(SeekFrom::Start(0))?;
        src.read_exact(&mut buf)?;
        let h = fao_v1::read_fao_header(&buf).ok_or_else(|| bad("not a FAO2 file"))?;
        Ok(FaoReader {
            src,
            width: h.width,
            height: h.height,
            fps_num: h.fps_num,
            fps_den: h.fps_den,
            frame_count: h.frame_count as u64,
        })
    }

    pub fn fps(&self) -> f64 {
        if self.fps_den == 0 { 0.0 } else { self.fps_num as f64 / self.fps_den as f64 }
    }

    /// Index entry `i`, read from disk.
    pub fn entry(&mut self, i: u64) -> io::Result<Entry> {
        if i >= self.frame_count {
            return Err(bad("frame index past the end"));
        }
        let at = HEADER_LEN
            .checked_add(i.checked_mul(ENTRY_LEN).ok_or_else(|| bad("index overflow"))?)
            .ok_or_else(|| bad("index overflow"))?;
        let mut b = [0u8; ENTRY_LEN as usize];
        self.src.seek(SeekFrom::Start(at))?;
        self.src.read_exact(&mut b)?;
        // Explicit little-endian, matching the header's `to_le_bytes`. The
        // library reads entries with `read_unaligned`, which is native-endian
        // and right only because every target here is little-endian.
        Ok(Entry {
            offset: u64::from_le_bytes(b[0..8].try_into().unwrap()),
            size: u32::from_le_bytes(b[8..12].try_into().unwrap()),
            compressed_size: u32::from_le_bytes(b[12..16].try_into().unwrap()),
        })
    }

    /// The codec marker of frame `i`: ONE byte, not the whole payload.
    ///
    /// `decode::FaoFile::markers` gets each marker by calling `payload(i)` --
    /// copying the entire frame -- and keeping only byte 0. The player runs that
    /// for every frame on open, so opening a file read all of it. This seeks to
    /// the first payload byte and reads just that. `None` for an empty payload,
    /// matching `payload(i).first()`.
    pub fn marker(&mut self, i: u64) -> io::Result<Option<u8>> {
        let e = self.entry(i)?;
        let mut fh = [0u8; FRAME_HEADER_LEN as usize];
        self.src.seek(SeekFrom::Start(e.offset))?;
        self.src.read_exact(&mut fh)?;
        if u32::from_le_bytes(fh[1..5].try_into().unwrap()) == 0 {
            return Ok(None);
        }
        let mut m = [0u8; 1];
        self.src.read_exact(&mut m)?;
        Ok(Some(m[0]))
    }

    /// The payload of frame `i` -- the bytes whose first is the codec marker.
    pub fn payload(&mut self, i: u64) -> io::Result<Vec<u8>> {
        let e = self.entry(i)?;
        let mut fh = [0u8; FRAME_HEADER_LEN as usize];
        self.src.seek(SeekFrom::Start(e.offset))?;
        self.src.read_exact(&mut fh)?;
        // The frame header's own size field, as `read_fao_frame` uses.
        let n = u32::from_le_bytes(fh[1..5].try_into().unwrap()) as usize;
        let mut payload = vec![0u8; n];
        self.src.read_exact(&mut payload)?;
        Ok(payload)
    }
}

/// Bytes read BEFORE the end of frame data when fetching the trailer.
///
/// `decode::v8mx_from` reads a length prefix from the 4 bytes BEFORE the
/// `V8MX` magic. A tail starting exactly at the end of frame data would put a
/// trailer that begins there at position 0, skip that check, and disagree with
/// the in-memory reader -- which reads the last 4 bytes of the final frame. So
/// the tail starts 4 bytes early and the search starts 4 bytes in: both readers
/// then see identical bytes around the needle.
pub const TRAILER_LEAD: u64 = 4;

/// A trailer larger than this is not a trailer but a corrupt index pointing
/// early into the file; refused rather than read.
pub const MAX_TRAILER: u64 = 512 * 1024 * 1024;

impl<R: Read + Seek> FaoReader<R> {
    /// Everything after the last frame, plus `TRAILER_LEAD` bytes before it.
    /// Returns the bytes and where in them the search should start.
    ///
    /// The end of frame data is the LAST index entry's end -- one 16-byte read.
    /// The in-memory reader takes the max over EVERY entry, which for a 600-hour
    /// file is 2 GB of index to find one boundary. The last entry is the max for
    /// any file `write_fao_container` produced: it assigns offsets starting after
    /// the index and only ever adding, and writes index and frames in that order.
    pub fn trailer(&mut self) -> io::Result<(Vec<u8>, usize)> {
        if self.frame_count == 0 {
            return Ok((Vec::new(), 0));
        }
        let last = self.entry(self.frame_count - 1)?;
        let frames_end = last.offset.saturating_add(last.size as u64);
        let file_end = self.src.seek(SeekFrom::End(0))?;
        if frames_end >= file_end {
            return Ok((Vec::new(), 0));
        }
        let lead = TRAILER_LEAD.min(frames_end);
        let start = frames_end - lead;
        let len = file_end - start;
        if len > MAX_TRAILER {
            return Err(bad("trailer region implausibly large -- corrupt index?"));
        }
        let mut buf = vec![0u8; len as usize];
        self.src.seek(SeekFrom::Start(start))?;
        self.src.read_exact(&mut buf)?;
        Ok((buf, lead as usize))
    }

    /// The block dictionary, via the SAME search the in-memory reader uses.
    pub fn block_dict(&mut self) -> io::Result<Option<fao_v1::block_dict::BlockDictionary>> {
        let (tail, from) = self.trailer()?;
        Ok(crate::decode::block_dict_from(&tail, from))
    }

    /// The V8MX webs, via the SAME search the in-memory reader uses.
    pub fn v8mx(&mut self) -> io::Result<Option<Vec<fao_v1::v8_reconstruction_codec::DecodedWeb>>> {
        let (tail, from) = self.trailer()?;
        Ok(crate::decode::v8mx_from(&tail, from))
    }
}

/// Presentation time of frame `i`, in milliseconds, without overflow.
///
/// THE 596.5-HOUR TRAP. A signed 32-bit millisecond count overflows at
/// 2,147,483,647 ms = 596.5 hours, which is why so many tools report "number
/// too big" on 600-hour videos. Computed in u64 here (18 billion years of
/// range), and in integer arithmetic so a 600-hour position is exact rather than
/// rounded by a float.
pub fn frame_time_ms(i: u64, fps_num: u16, fps_den: u16) -> Option<u64> {
    if fps_num == 0 {
        return None;
    }
    // i / (num/den) seconds = i * den / num s = i * den * 1000 / num ms.
    i.checked_mul(fps_den as u64)?.checked_mul(1000).map(|v| v / fps_num as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frames(n: usize, w: usize, h: usize) -> Vec<Vec<u8>> {
        (0..n).map(|k| (0..w * h * 3).map(|i| ((i + k * 37) % 251) as u8).collect()).collect()
    }

    /// Same bytes as the in-memory reader, frame for frame.
    #[test]
    fn streaming_matches_the_in_memory_reader() {
        let (w, h) = (32usize, 16usize);
        let bytes = crate::export::write_dct_fao(&frames(4, w, h), w, h, 30.0, 80).unwrap();
        let mem = crate::decode::FaoFile::parse(bytes.clone()).unwrap();
        let mut r = FaoReader::open(Cursor::new(bytes)).unwrap();
        assert_eq!((r.width as usize, r.height as usize, r.frame_count), (w, h, 4));
        for i in 0..4 {
            assert_eq!(r.payload(i as u64).unwrap(), mem.payload(i).unwrap(), "frame {i}");
        }
    }

    /// A reader that pretends to be a file of `len` bytes: real bytes inside
    /// `chunks`, zeros everywhere else, and it records the most it was ever
    /// asked to read at once. Lets a test open a multi-TERABYTE file.
    struct Sparse { len: u64, pos: u64, chunks: Vec<(u64, Vec<u8>)>, biggest_read: usize }

    impl Read for Sparse {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let n = out.len().min((self.len.saturating_sub(self.pos)) as usize);
            self.biggest_read = self.biggest_read.max(n);
            for (k, byte) in out[..n].iter_mut().enumerate() {
                let at = self.pos + k as u64;
                *byte = self.chunks.iter()
                    .find(|(s, d)| at >= *s && at < *s + d.len() as u64)
                    .map(|(s, d)| d[(at - s) as usize]).unwrap_or(0);
            }
            self.pos += n as u64;
            Ok(n)
        }
    }
    impl Seek for Sparse {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            self.pos = match p { SeekFrom::Start(v) => v, SeekFrom::End(v) => (self.len as i64 + v) as u64,
                                 SeekFrom::Current(v) => (self.pos as i64 + v) as u64 };
            Ok(self.pos)
        }
    }

    /// THE POINT OF THIS MODULE. A 5 TB file -- a 600-hour video's order of
    /// size -- with its last frame 4.5 TB in. The in-memory reader cannot open
    /// this at all; `read_fao_frame`'s `offset as usize` would truncate it on a
    /// 32-bit target. This reader seeks straight to it, and never reads more
    /// than one frame at once.
    #[test]
    fn a_frame_four_and_a_half_terabytes_in_is_reachable() {
        let far: u64 = 4_500_000_000_000;
        let payload = b"D-the-far-frame".to_vec();
        let mut header = Vec::new();
        header.extend_from_slice(b"FAO2");
        header.extend_from_slice(&1u16.to_le_bytes());      // version
        header.extend_from_slice(&7680u16.to_le_bytes());   // 8K wide
        header.extend_from_slice(&4320u16.to_le_bytes());
        header.extend_from_slice(&6000u16.to_le_bytes());   // 60.00 fps
        header.extend_from_slice(&100u16.to_le_bytes());
        header.extend_from_slice(&129_600_000u32.to_le_bytes()); // 600 h @ 60
        header.extend_from_slice(&30u16.to_le_bytes());
        header.extend_from_slice(&[80, 0]);
        header.extend_from_slice(&0u64.to_le_bytes());
        assert_eq!(header.len() as u64, HEADER_LEN);
        // The LAST frame's index entry, pointing 4.5 TB in.
        let last = 129_600_000u64 - 1;
        let mut entry = Vec::new();
        entry.extend_from_slice(&far.to_le_bytes());
        entry.extend_from_slice(&0u32.to_le_bytes());
        entry.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        // Its frame header then payload.
        let mut frame = vec![0u8; FRAME_HEADER_LEN as usize];
        frame[1..5].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(&payload);

        let src = Sparse {
            len: 5_000_000_000_000, pos: 0, biggest_read: 0,
            chunks: vec![(0, header), (HEADER_LEN + last * ENTRY_LEN, entry), (far, frame)],
        };
        let mut r = FaoReader::open(src).unwrap();
        assert_eq!((r.width, r.height, r.frame_count), (7680, 4320, 129_600_000));
        assert_eq!(r.entry(last).unwrap().offset, far, "the u64 offset survived");
        assert_eq!(r.payload(last).unwrap(), payload);
        assert!(r.src.biggest_read <= 64, "read {} bytes at once from a 5 TB file", r.src.biggest_read);
    }

    /// A file carrying a real FAOBKD01 block dictionary trailer.
    fn file_with_dict() -> (Vec<u8>, Vec<Vec<u8>>) {
        let (w, h) = (32usize, 16usize);
        let mut bytes = crate::export::write_dct_fao(&frames(2, w, h), w, h, 30.0, 80).unwrap();
        let mut dict = fao_v1::block_dict::BlockDictionary::new();
        let blocks: Vec<Vec<u8>> = (0..5u8).map(|k| vec![k; 64]).collect();
        for b in &blocks { dict.intern(b.clone()); }
        let payload = dict.to_bytes();
        bytes.extend_from_slice(b"FAOBKD01");
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);
        (bytes, blocks)
    }

    /// The streaming reader finds the SAME dictionary the in-memory one does,
    /// with the same blocks in it.
    #[test]
    fn the_block_dictionary_is_found_without_the_whole_file() {
        let (bytes, blocks) = file_with_dict();
        let mem = crate::decode::FaoFile::parse(bytes.clone()).unwrap();
        assert!(mem.has_block_dict(), "fixture must actually carry a dictionary");
        let mut r = FaoReader::open(Cursor::new(bytes)).unwrap();
        let d = r.block_dict().unwrap().expect("found by streaming");
        for (id, b) in blocks.iter().enumerate() {
            assert_eq!(d.get(id as u32), Some(b.as_slice()), "block {id}");
        }
    }

    /// No trailer: both readers agree there is nothing.
    #[test]
    fn a_file_without_a_trailer_has_none() {
        let (w, h) = (32usize, 16usize);
        let bytes = crate::export::write_dct_fao(&frames(2, w, h), w, h, 30.0, 80).unwrap();
        let mem = crate::decode::FaoFile::parse(bytes.clone()).unwrap();
        let mut r = FaoReader::open(Cursor::new(bytes)).unwrap();
        assert_eq!(mem.has_block_dict(), r.block_dict().unwrap().is_some());
        assert!(r.v8mx().unwrap().is_none());
    }

    /// The tail includes the 4 bytes before frame data ends, so V8MX's
    /// length-prefix check sees what the in-memory reader sees.
    #[test]
    fn the_trailer_starts_four_bytes_early() {
        let (bytes, _) = file_with_dict();
        let mut r = FaoReader::open(Cursor::new(bytes.clone())).unwrap();
        let last = r.entry(r.frame_count - 1).unwrap();
        let frames_end = (last.offset + last.size as u64) as usize;
        let (tail, from) = r.trailer().unwrap();
        assert_eq!(from, TRAILER_LEAD as usize);
        assert_eq!(&tail[..], &bytes[frames_end - 4..], "tail = the 4 lead bytes + everything after");
    }

    /// One byte, and the same byte the full payload starts with.
    #[test]
    fn marker_matches_the_payloads_first_byte() {
        let (w, h) = (32usize, 16usize);
        let bytes = crate::export::write_dct_fao(&frames(3, w, h), w, h, 30.0, 80).unwrap();
        let mut r = FaoReader::open(Cursor::new(bytes)).unwrap();
        for i in 0..3u64 {
            let full = r.payload(i).unwrap();
            assert_eq!(r.marker(i).unwrap(), full.first().copied(), "frame {i}");
        }
    }

    #[test]
    fn past_the_end_is_an_error_not_a_panic() {
        let (w, h) = (16usize, 16usize);
        let bytes = crate::export::write_dct_fao(&frames(1, w, h), w, h, 30.0, 80).unwrap();
        let mut r = FaoReader::open(Cursor::new(bytes)).unwrap();
        assert!(r.payload(1).is_err());
        assert!(r.entry(u64::MAX).is_err(), "an absurd index must not overflow");
        assert!(FaoReader::open(Cursor::new(b"not a fao".to_vec())).is_err());
    }

    /// 600 hours at 60 fps: past the 596.5-hour signed-32-bit overflow, exact.
    #[test]
    fn a_600_hour_timestamp_does_not_overflow() {
        let last = 600u64 * 3600 * 60 - 1;
        let ms = frame_time_ms(last, 6000, 100).unwrap();
        assert!(ms > i32::MAX as u64, "this is exactly the value that breaks i32");
        assert_eq!(ms, 2_159_999_983, "exact: frame 129,599,999 at 60 fps");
        // NTSC-style fractional rate stays exact in integers.
        assert_eq!(frame_time_ms(60_000, 60000, 1001).unwrap(), 1_001_000);
        assert_eq!(frame_time_ms(1, 0, 100), None, "zero fps is not a rate");
    }
}
