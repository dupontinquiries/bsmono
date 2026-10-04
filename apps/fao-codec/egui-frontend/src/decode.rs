//! decode — an opened .fao and the frames it can show, ported from fao_app's
//! `decode_fao_frame_to_rgb` (the Player's decode path) and `read_block_dict`.
//!
//! What is here, per first payload byte, transcribed branch for branch:
//!
//!   'D' DCT I-frame          decompress_frame_dct
//!   'P' YUV-split I-frame    decompress_frame_yuv_split
//!   'L' lossy-chroma I-frame decompress_frame_lossy_chroma
//!   'K' block-dict K-frame   decode_frame_from_dict with the FAOBKD01 trailer dict
//!   'T' predicted frame      decode its ANCHOR first (recursively, through the cache), then
//!                            decode_with_anchor over the anchor's YUV
//!   'X' v10 frame            decode_frame chained on the previous frame's RGB
//!   'R' repeat marker (v8)   walk back up to 600 frames to the first non-'R' and show that
//!   other                    raw YUV, raw RGB, or zlib of either — sized by the header
//!
//! v8 frames ('Z', or 'V' immediately followed by '8') render through the same
//! `V8Decoder::render_frame_rgb` fao_app calls, once a `V8MX` multi-web trailer is present.
//! `render_frame_rgb` only ever reads `self.webs` (via `web_for_frame`) and `self.library_refs`
//! — `scene_mesh` and `pose_track` are never consulted by the render path itself (`pose_track`
//! only feeds the OPTIONAL jitter/EMA stabilizer, off unless `FAO_LESS_JITTER`/`FAO_POSE_EMA`
//! are set; the per-frame pose is read straight out of the frame bytes by `V8Frame::from_bytes_
//! any`). So a decoder built from V8MX alone renders correctly; fao_app's V8MS mesh-trailer and
//! V8PT pose-track parsing (three on-disk pose formats, ~120 lines) are not ported — they exist
//! for other consumers (mesh export, the World tab) rather than for turning frames into pixels.
//! `V8LR` (library-referenced webs, needs an external library file via `FAO_LIBRARY`) is also
//! not ported: `Gap::NeedsV8` still covers a file whose webs are all library refs.
//!
//! The anchor cache is the same shape as fao_app's: (index, rgb) pairs, searched newest
//! first, so a T-chain that references one anchor decodes it once.
use fao_codec_lib as fao_v1;
use std::cell::RefCell;

/// Why a frame could not be turned into pixels. Every variant is a sentence in the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gap {
    OutOfRange,
    Empty,
    /// 'V8' / 'Z': needs the V8MX trailer decoder, not ported yet.
    /// 'V8'/'Z' with no V8MX trailer, or one whose webs are all library refs (needs an
    /// external library file — not ported).
    NeedsV8,
    /// 'V4'..'V7': fao_app renders nothing for these either.
    Unrenderable(u8),
    /// 'K' without a FAOBKD01 trailer in the file.
    NeedsDict,
    /// 'R' whose walk-back found no renderable frame within 600.
    RepeatUnresolved,
    /// The codec returned None (corrupt payload or a version this build cannot read).
    Failed(u8),
}
impl std::fmt::Display for Gap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Gap::OutOfRange => write!(f, "frame index past the end of the file"),
            Gap::Empty => write!(f, "empty payload (skip frame with nothing before it)"),
            Gap::NeedsV8 => write!(f, "v8 frame: no V8MX web trailer (or none of its webs render without an external V8LR library)"),
            Gap::Unrenderable(m) => write!(f, "'{}' frame: fao_app renders nothing for this version either", *m as char),
            Gap::NeedsDict => write!(f, "'K' frame but the file has no FAOBKD01 block dictionary"),
            Gap::RepeatUnresolved => write!(f, "'R' repeat marker with no renderable frame in the 600 before it"),
            Gap::Failed(m) => write!(f, "'{}' frame failed to decode", *m as char),
        }
    }
}

/// The classic 4:2:0 → RGB conversion fao_app's Player uses (`yuv_to_rgb_simple`, single
/// thread). Short planes give the same 64-gray fallback the GTK app shows.
pub fn yuv_to_rgb(yuv: &[u8], w: usize, h: usize) -> Vec<u8> {
    let y_sz = w * h;
    let hw = w / 2;
    let hh = h / 2;
    let uv_sz = hw * hh;
    if yuv.len() < y_sz + 2 * uv_sz { return vec![64u8; w * h * 3]; }
    let mut rgb = vec![0u8; w * h * 3];
    let u_plane = &yuv[y_sz..y_sz + uv_sz];
    let v_plane = &yuv[y_sz + uv_sz..y_sz + 2 * uv_sz];
    for py in 0..h {
        let uv_row = (py / 2) * hw;
        let y_row = py * w;
        let rgb_row = py * w * 3;
        for px in 0..w {
            let ux = (px / 2).min(hw.saturating_sub(1));
            let yv = yuv[y_row + px] as f32;
            let uv = u_plane[uv_row + ux] as f32 - 128.0;
            let vv = v_plane[uv_row + ux] as f32 - 128.0;
            let r = (yv + 1.402 * vv).clamp(0.0, 255.0);
            let g = (yv - 0.344 * uv - 0.714 * vv).clamp(0.0, 255.0);
            let b = (yv + 1.772 * uv).clamp(0.0, 255.0);
            let ri = rgb_row + px * 3;
            rgb[ri] = r as u8; rgb[ri + 1] = g as u8; rgb[ri + 2] = b as u8;
        }
    }
    rgb
}

/// fao_app's `rgb_to_yuv420p`: the T-frame path needs its anchor as YUV, and the anchor
/// comes out of the cache as RGB. Box-filtered chroma over each 2×2, BT.601 luma.
pub fn rgb_to_yuv420p(rgb: &[u8], w: usize, h: usize) -> Vec<u8> {
    let y_sz = w * h;
    let hw = w / 2; let hh = h / 2;
    let uv_sz = hw * hh;
    let mut yuv = vec![0u8; y_sz + 2 * uv_sz];
    for py in 0..h {
        for px in 0..w {
            let i = (py * w + px) * 3;
            if i + 2 >= rgb.len() { continue; }
            let (r, g, b) = (rgb[i] as f32, rgb[i + 1] as f32, rgb[i + 2] as f32);
            yuv[py * w + px] = (0.299 * r + 0.587 * g + 0.114 * b).clamp(0.0, 255.0) as u8;
        }
    }
    for cy in 0..hh {
        for cx in 0..hw {
            let (mut sr, mut sg, mut sb) = (0.0f32, 0.0f32, 0.0f32);
            for dy in 0..2 { for dx in 0..2 {
                let (py, px) = (cy * 2 + dy, cx * 2 + dx);
                if py >= h || px >= w { continue; }
                let i = (py * w + px) * 3;
                if i + 2 >= rgb.len() { continue; }
                sr += rgb[i] as f32; sg += rgb[i + 1] as f32; sb += rgb[i + 2] as f32;
            } }
            let (r, g, b) = (sr / 4.0, sg / 4.0, sb / 4.0);
            let u = (-0.169 * r - 0.331 * g + 0.5 * b + 128.0).clamp(0.0, 255.0);
            let v = (0.5 * r - 0.419 * g - 0.081 * b + 128.0).clamp(0.0, 255.0);
            yuv[y_sz + cy * hw + cx] = u as u8;
            yuv[y_sz + uv_sz + cy * hw + cx] = v as u8;
        }
    }
    yuv
}

/// An opened .fao: bytes, header, frame index, the block dictionary if the file carries
/// one, and the anchor cache.
pub struct FaoFile {
    src: Source,
    pub header: fao_v1::FaoFileHeader,
    block_dict: Option<fao_v1::block_dict::BlockDictionary>,
    v8: Option<fao_v1::v8_reconstruction_codec::V8Decoder>,
    cache: RefCell<Vec<(usize, Vec<u8>)>>,
}

/// Anything a `.fao` can be streamed from.
pub trait ReadSeek: std::io::Read + std::io::Seek + Send {}
impl<T: std::io::Read + std::io::Seek + Send> ReadSeek for T {}

/// Where frame bytes come from.
///
/// EVERY DECODE PATH READS THROUGH `payload`, and nothing else touches the
/// storage -- checked: `index`, `header` and `data` had no users outside this
/// file. So streaming is a second arm here, and repeat resolution, the block
/// dictionary, the V8 webs and the anchor cache all run unchanged on top of it.
enum Source {
    /// The whole file in memory -- what `parse` has always done.
    Mem { data: Vec<u8>, index: Vec<fao_v1::FaoFrameEntry> },
    /// Seek-based, constant memory. `RefCell` because seeking mutates the
    /// reader while `decode_rgb` takes `&self`, the same reason `cache` is one.
    Stream { reader: RefCell<crate::stream::FaoReader<Box<dyn ReadSeek>>>, frames: usize },
}

const ANCHOR_CACHE_CAP: usize = 8;
const MAX_R_WALKBACK: usize = 600;

impl FaoFile {
    pub fn parse(data: Vec<u8>) -> Option<Self> {
        let header = fao_v1::read_fao_header(&data)?;
        let index = fao_v1::read_fao_index(&data, header.frame_count as usize);
        if index.is_empty() { return None; }
        let block_dict = read_block_dict(&data, &index);
        let v8 = read_v8mx_trailer(&data, &index)
            .map(|webs| fao_v1::v8_reconstruction_codec::V8Decoder::new(header.width.into(), header.height.into()).with_webs(webs));
        Some(FaoFile { src: Source::Mem { data, index }, header, block_dict, v8, cache: RefCell::new(Vec::new()) })
    }

    /// Open for streaming: reads the header and the trailers, nothing else.
    ///
    /// For files too large to hold -- a 600-hour video is terabytes. Frames are
    /// read on demand through `stream::FaoReader`; the block dictionary and V8
    /// webs come from the same shared searches `parse` uses, over just the tail.
    pub fn open_stream(src: Box<dyn ReadSeek>) -> Option<Self> {
        let mut r = crate::stream::FaoReader::open(src).ok()?;
        if r.frame_count == 0 { return None; }
        let (w, h) = (r.width, r.height);
        let header = fao_v1::FaoFileHeader {
            magic: *b"FAO2", version: 1, width: w, height: h,
            fps_num: r.fps_num, fps_den: r.fps_den,
            // u32 in the format, and FaoReader read it from a u32, so this
            // cannot truncate.
            frame_count: r.frame_count as u32,
            gop_size: 0, quality: 0, flags: 0, total_compressed: 0,
        };
        let block_dict = r.block_dict().ok().flatten();
        let v8 = r.v8mx().ok().flatten()
            .map(|webs| fao_v1::v8_reconstruction_codec::V8Decoder::new(w.into(), h.into()).with_webs(webs));
        let frames = r.frame_count as usize;
        Some(FaoFile { src: Source::Stream { reader: RefCell::new(r), frames }, header, block_dict, v8,
                       cache: RefCell::new(Vec::new()) })
    }

    /// One frame's codec marker, cheaply. Streaming reads ONE byte; `markers`
    /// (every frame, whole payloads) is what made opening read the whole file.
    pub fn marker(&self, idx: usize) -> Option<u8> {
        match &self.src {
            Source::Stream { reader, .. } => reader.borrow_mut().marker(idx as u64).ok().flatten(),
            Source::Mem { .. } => self.payload(idx).and_then(|b| b.first().copied()),
        }
    }
    pub fn width(&self) -> usize { self.header.width as usize }
    pub fn height(&self) -> usize { self.header.height as usize }
    pub fn frame_count(&self) -> usize {
        match &self.src { Source::Mem { index, .. } => index.len(), Source::Stream { frames, .. } => *frames }
    }
    pub fn fps(&self) -> f64 {
        if self.header.fps_den == 0 { 0.0 } else { self.header.fps_num as f64 / self.header.fps_den as f64 }
    }
    pub fn has_block_dict(&self) -> bool { self.block_dict.is_some() }
    pub fn has_v8_webs(&self) -> bool { self.v8.is_some() }

    /// Payload bytes of frame `idx` (what the markers are read from).
    pub fn payload(&self, idx: usize) -> Option<Vec<u8>> {
        match &self.src {
            Source::Mem { data, index } => {
                let entry = index.get(idx)?;
                fao_v1::read_fao_frame(data, entry).map(|(b, _)| b)
            }
            Source::Stream { reader, .. } => reader.borrow_mut().payload(idx as u64).ok(),
        }
    }
    /// First payload byte of every frame — the input `player::build_anchor_map` takes.
    pub fn markers(&self) -> Vec<Option<u8>> {
        (0..self.frame_count()).map(|i| self.payload(i).and_then(|b| b.first().copied())).collect()
    }

    /// Frame `idx` as packed RGB (w*h*3), or the reason it is not showable.
    pub fn decode_rgb(&self, idx: usize) -> Result<Vec<u8>, Gap> {
        if let Some((_, rgb)) = self.cache.borrow().iter().rev().find(|(i, _)| *i == idx) {
            return Ok(rgb.clone());
        }
        let rgb = self.decode_uncached(idx)?;
        let mut c = self.cache.borrow_mut();
        if c.len() >= ANCHOR_CACHE_CAP { c.remove(0); }
        c.push((idx, rgb.clone()));
        Ok(rgb)
    }

    fn decode_uncached(&self, idx: usize) -> Result<Vec<u8>, Gap> {
        let (w, h) = (self.width(), self.height());
        let (yuv_sz, rgb_sz) = (w * h * 3 / 2, w * h * 3);
        let bytes = self.payload(idx).ok_or(Gap::OutOfRange)?;
        let Some(&marker) = bytes.first() else { return Err(Gap::Empty) };

        if marker == fao_v1::v10::V10_MAGIC {
            // v10 chains on the previous frame's pixels; the first frame has none.
            let prev = if idx > 0 { self.decode_rgb(idx - 1).ok() } else { None };
            return fao_v1::v10::decode_frame(&bytes, prev.as_deref(), w, h).ok_or(Gap::Failed(marker));
        }
        if marker == crate::motion::MARKER || marker == crate::motion::MARKER_Q {
            // Shiftquilt: changed blocks intra-coded or predicted from the
            // previous frame's picture. Like M, never first.
            if idx == 0 {
                return Err(Gap::Failed(marker));
            }
            let prev = self.decode_rgb(idx - 1)?;
            return crate::motion::decode(&bytes, &prev, w, h).ok_or(Gap::Failed(marker));
        }
        if marker == crate::patch::MARKER {
            // Quiltstone: the changed blocks over the PREVIOUS frame's picture.
            // A patch frame is never first (the transcoder starts every run with
            // a keyframe), so frame 0 as M is malformed, not "decode on black".
            if idx == 0 {
                return Err(Gap::Failed(marker));
            }
            let prev = self.decode_rgb(idx - 1)?;
            return crate::patch::decode(&bytes, &prev, w, h).ok_or(Gap::Failed(marker));
        }
        if marker == b'R' {
            // v8 repeat marker: show the nearest earlier frame that is not itself a repeat.
            let limit = idx.min(MAX_R_WALKBACK);
            for back in 1..=limit {
                let p = idx - back;
                match self.payload(p) {
                    Some(pb) if !pb.is_empty() && pb[0] != b'R' => return self.decode_rgb(p),
                    _ => continue,
                }
            }
            return Err(Gap::RepeatUnresolved);
        }
        let is_v8 = marker == b'Z' || (marker == b'V' && bytes.get(1) == Some(&b'8'));
        if is_v8 {
            return self.v8.as_ref()
                .and_then(|dec| dec.render_frame_rgb(&bytes, idx as u32))
                .ok_or(Gap::NeedsV8);
        }

        match marker {
            b'D' => fao_v1::decompress_frame_dct(&bytes, w, h).map(|yuv| yuv_to_rgb(&yuv, w, h)),
            b'P' => fao_v1::decompress_frame_yuv_split(&bytes, w, h).map(|yuv| yuv_to_rgb(&yuv, w, h)),
            b'L' => fao_v1::decompress_frame_lossy_chroma(&bytes, w, h).map(|yuv| yuv_to_rgb(&yuv, w, h)),
            b'K' => {
                let dict = self.block_dict.as_ref().ok_or(Gap::NeedsDict)?;
                fao_v1::block_dict::decode_frame_from_dict(&bytes, dict).map(|(yuv, fw, fh)| yuv_to_rgb(&yuv, fw, fh))
            }
            b'V' => return Err(Gap::Unrenderable(marker)),
            b'T' => {
                let (tframe, fw, fh) = fao_v1::v21_predictive::TranslatedFrame::from_bytes(&bytes).ok_or(Gap::Failed(marker))?;
                let anchor_idx = tframe.anchor_ref as usize;
                if anchor_idx == idx { return Err(Gap::Failed(marker)); } // self-ref guard, as in fao_app
                let anchor_rgb = self.decode_rgb(anchor_idx)?;
                let anchor_yuv = rgb_to_yuv420p(&anchor_rgb, fw, fh);
                tframe.decode_with_anchor(&anchor_yuv, fw, fh).map(|yuv| yuv_to_rgb(&yuv, fw, fh))
            }
            _ => {
                if bytes.len() == yuv_sz { Some(yuv_to_rgb(&bytes, w, h)) }
                else if bytes.len() == rgb_sz { Some(bytes) }
                else {
                    fao_v1::decompress_frame(&bytes, yuv_sz).map(|yuv| yuv_to_rgb(&yuv, w, h))
                        .or_else(|| fao_v1::decompress_frame(&bytes, rgb_sz))
                }
            }
        }.ok_or(Gap::Failed(marker))
    }
}

/// fao_app's V8MX location logic: find the FIRST `V8MX` magic after the frame data, find where
/// it ends (the next `V8PT`/`V8MS`/`V8RP`/`V8LR` magic, or — more precisely, when present — the
/// u32 LE length stored in the 4 bytes immediately BEFORE the magic), and parse that span with
/// `V8Decoder::parse_v8mx`, the same function fao_app calls. Only V8MX is located: V8MS (mesh)
/// and V8PT (pose track) are not read by this port (see the module doc — `render_frame_rgb`
/// never consults either), and V8LR/V8AT/V9DC are not searched for at all.
fn read_v8mx_trailer(data: &[u8], index: &[fao_v1::FaoFrameEntry]) -> Option<Vec<fao_v1::v8_reconstruction_codec::DecodedWeb>> {
    v8mx_from(data, frame_data_end(index).min(data.len()))
}

/// Where frame data ends: the furthest `offset + size` over the index.
///
/// Saturating, because the index is the first thing read from the file and a damaged entry can
/// carry an offset next to u64::MAX: `offset + size` then overflows, which in a debug build is a
/// panic on a bad FILE (the damaged-file harness found three such bytes in one key frame) and in
/// release wraps to a small number that would make the trailer search start inside the frames.
/// Callers clamp to `data.len()`, so saturating to the top is the right lie.
fn frame_data_end(index: &[fao_v1::FaoFrameEntry]) -> usize {
    index.iter().map(|e| usize::try_from(e.offset.saturating_add(e.size as u64)).unwrap_or(usize::MAX)).max().unwrap_or(0)
}

/// The V8MX trailer, searched for from `search_start` in `data`.
///
/// SPLIT FROM `read_v8mx_trailer` so the streaming reader runs the SAME search
/// over just the file's tail. It reads a length prefix from the 4 bytes BEFORE
/// the magic -- so a caller handing it a tail must include those 4 bytes; see
/// `stream::FaoReader::trailer`.
pub(crate) fn v8mx_from(data: &[u8], search_start: usize) -> Option<Vec<fao_v1::v8_reconstruction_codec::DecodedWeb>> {
    if data.len() < search_start + 4 { return None; }
    let pos = (search_start..data.len().saturating_sub(4)).find(|&i| &data[i..i + 4] == b"V8MX")?;
    let mut end = data.len();
    for i in (pos + 5)..data.len().saturating_sub(4) {
        if matches!(&data[i..i + 4], b"V8PT" | b"V8MS" | b"V8RP" | b"V8LR") { end = i; break; }
    }
    if pos >= 4 {
        let stated_len = u32::from_le_bytes(data[pos - 4..pos].try_into().unwrap()) as usize;
        let computed_end = pos + stated_len;
        if stated_len > 0 && computed_end <= data.len() && computed_end <= end { end = computed_end; }
    }
    let webs = fao_v1::v8_reconstruction_codec::V8Decoder::parse_v8mx(&data[pos..end])?;
    if webs.is_empty() { None } else { Some(webs) }
}

/// fao_app's `read_block_dict`: the dictionary trailer is `FAOBKD01` + u32 LE size + bytes,
/// searched for after the last frame's data.
fn read_block_dict(data: &[u8], index: &[fao_v1::FaoFrameEntry]) -> Option<fao_v1::block_dict::BlockDictionary> {
    block_dict_from(data, frame_data_end(index).min(data.len()))
}

/// The FAOBKD01 trailer, searched for from `search_start`. Split like
/// `v8mx_from` so both readers share one search.
pub(crate) fn block_dict_from(data: &[u8], search_start: usize) -> Option<fao_v1::block_dict::BlockDictionary> {
    let needle = b"FAOBKD01";
    if data.len() < 12 { return None; }
    let at = (search_start..data.len().saturating_sub(needle.len()))
        .find(|&i| &data[i..i + needle.len()] == needle)? + needle.len();
    if at + 4 > data.len() { return None; }
    let dict_size = u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize;
    let start = at + 4;
    if start + dict_size > data.len() { return None; }
    fao_v1::block_dict::BlockDictionary::from_bytes(&data[start..start + dict_size])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-frame .fao written by the lib's own container writer, both DCT I-frames of a
    /// gradient. The decoder must read them back to exactly what the codec's own
    /// decompress gives — the plumbing (header, index, payload, dispatch, cache) is what
    /// this proves, not the codec.
    fn gradient_rgb(w: usize, h: usize, phase: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 3);
        for y in 0..h { for x in 0..w {
            v.push(((x * 255) / w.max(1)) as u8); v.push(((y * 255) / h.max(1)) as u8); v.push(phase);
        } }
        v
    }
    fn two_frame_file() -> (Vec<u8>, usize, usize) {
        let (w, h) = (32usize, 16usize);
        let mut frames = Vec::new();
        for phase in [40u8, 200u8] {
            let yuv = rgb_to_yuv420p(&gradient_rgb(w, h, phase), w, h);
            let payload = fao_v1::compress_frame_dct(&yuv, w, h, 80);
            assert_eq!(payload[0], b'D');
            let n = payload.len();
            frames.push((Vec::new(), payload, 0u8, n, 40.0f64, 0.0f64));
        }
        let mut out = Vec::new();
        fao_v1::write_fao_container(&mut out, w, h, 30.0, 0.8, &frames).unwrap();
        (out, w, h)
    }

    /// STREAMED DECODES ARE THE IN-MEMORY DECODES, pixel for pixel, frame for
    /// frame. Every decode path reads through `payload`, so this is the proof
    /// that swapping the storage underneath changed nothing above it.
    #[test]
    fn a_streamed_file_decodes_identically_to_an_in_memory_one() {
        let (w, h) = (32usize, 16usize);
        let frames: Vec<Vec<u8>> = [10u8, 90, 170, 250]
            .iter().map(|&p| gradient_rgb(w, h, p)).collect();
        let bytes = crate::export::write_dct_fao(&frames, w, h, 30.0, 80).unwrap();
        let mem = FaoFile::parse(bytes.clone()).unwrap();
        let st = FaoFile::open_stream(Box::new(std::io::Cursor::new(bytes))).unwrap();
        assert_eq!((st.width(), st.height(), st.frame_count()), (mem.width(), mem.height(), mem.frame_count()));
        assert!((st.fps() - mem.fps()).abs() < 1e-9);
        for i in 0..mem.frame_count() {
            assert_eq!(st.marker(i), mem.marker(i), "marker {i}");
            assert_eq!(st.decode_rgb(i).unwrap(), mem.decode_rgb(i).unwrap(), "frame {i}");
        }
        assert_eq!(st.decode_rgb(mem.frame_count()), Err(Gap::OutOfRange));
    }

    /// And the trailers: a streamed file finds the same block dictionary.
    #[test]
    fn a_streamed_file_finds_the_block_dictionary() {
        let (w, h) = (32usize, 16usize);
        let mut bytes = crate::export::write_dct_fao(&[gradient_rgb(w, h, 5)], w, h, 30.0, 80).unwrap();
        let mut dict = fao_v1::block_dict::BlockDictionary::new();
        for k in 0..4u8 { dict.intern(vec![k; 64]); }
        let payload = dict.to_bytes();
        bytes.extend_from_slice(b"FAOBKD01");
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);
        let mem = FaoFile::parse(bytes.clone()).unwrap();
        let st = FaoFile::open_stream(Box::new(std::io::Cursor::new(bytes))).unwrap();
        assert!(mem.has_block_dict(), "fixture must carry a dictionary");
        assert_eq!(st.has_block_dict(), mem.has_block_dict());
    }

    #[test]
    fn a_dct_file_decodes_to_the_codecs_own_answer() {
        let (data, w, h) = two_frame_file();
        let f = FaoFile::parse(data.clone()).expect("parses");
        assert_eq!((f.width(), f.height(), f.frame_count()), (w, h, 2));
        assert_eq!(f.markers(), vec![Some(b'D'), Some(b'D')]);
        for idx in 0..2 {
            let payload = f.payload(idx).unwrap();
            let want = yuv_to_rgb(&fao_v1::decompress_frame_dct(&payload, w, h).unwrap(), w, h);
            assert_eq!(f.decode_rgb(idx).unwrap(), want, "frame {idx}");
        }
        // The second decode is the cache; it must be the same bytes, not a stale frame.
        assert_eq!(f.decode_rgb(1).unwrap(), f.decode_rgb(1).unwrap());
        assert_ne!(f.decode_rgb(0).unwrap(), f.decode_rgb(1).unwrap(), "frames differ by phase");
        assert_eq!(f.decode_rgb(2), Err(Gap::OutOfRange));
    }

    #[test]
    fn gaps_are_named_not_blank() {
        assert!(Gap::NeedsV8.to_string().contains("V8MX"));
        assert!(Gap::Unrenderable(b'V').to_string().contains("fao_app"));
        assert!(Gap::Failed(b'T').to_string().contains("'T'"));
    }

    /// Builds a valid `V8MX` trailer byte-for-byte per `V8Decoder::parse_v8mx`'s own reader:
    /// version 1, one standalone (`kind=2`) web whose mesh blob is the v1 layout (n_verts,
    /// n_indices, n_colors headers, then the arrays), zstd-compressed. A one-triangle mesh —
    /// enough to prove the render path runs, not to look like anything.
    fn one_triangle_v8mx_trailer() -> Vec<u8> {
        let verts: [[f32; 3]; 3] = [[-0.5, -0.5, 2.0], [0.5, -0.5, 2.0], [0.0, 0.5, 2.0]];
        let colors: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let indices: [u32; 3] = [0, 1, 2];
        let mut raw = Vec::new();
        raw.extend_from_slice(&3u32.to_le_bytes()); // n_verts
        raw.extend_from_slice(&3u32.to_le_bytes()); // n_indices
        raw.extend_from_slice(&3u32.to_le_bytes()); // n_colors
        for v in verts { for c in v { raw.extend_from_slice(&c.to_le_bytes()); } }
        for i in indices { raw.extend_from_slice(&i.to_le_bytes()); }
        for c in colors { for ch in c { raw.extend_from_slice(&ch.to_le_bytes()); } }
        let blob = zstd::encode_all(std::io::Cursor::new(&raw[..]), 1).unwrap();

        let mut web = Vec::new();
        web.extend_from_slice(&1u32.to_le_bytes());  // chunk_id
        web.extend_from_slice(&0u32.to_le_bytes());  // canonical_id (unused for kind=2)
        web.extend_from_slice(&0u32.to_le_bytes());  // frame_start
        web.extend_from_slice(&9u32.to_le_bytes());  // frame_end
        web.push(2);                                 // kind: standalone
        for v in identity_4x4() { web.extend_from_slice(&v.to_le_bytes()); }
        web.extend_from_slice(&(blob.len() as u32).to_le_bytes());
        web.extend_from_slice(&blob);

        let mut trailer = Vec::new();
        trailer.extend_from_slice(b"V8MX");
        trailer.push(1); // trailer_version 1 (legacy, no per-blob tag byte)
        trailer.extend_from_slice(&1u32.to_le_bytes()); // n_webs
        trailer.extend_from_slice(&web);
        trailer
    }
    fn identity_4x4() -> [f32; 16] {
        let mut m = [0.0f32; 16];
        for i in 0..4 { m[i * 4 + i] = 1.0; }
        m
    }

    /// A one-frame .fao whose payload is a raw (unwrapped) `V8Frame` and whose trailer is the
    /// V8MX above — end to end through `FaoFile::parse` and `decode_rgb`, the same path a real
    /// v8 export takes, down to the identical needle-search this port copied from fao_app.
    #[test]
    fn a_v8_frame_renders_through_the_real_v8_decoder() {
        let (w, h) = (32usize, 32usize);
        let frame = fao_v1::v8_reconstruction_codec::V8Frame {
            camera_pose: fao_v1::v8_reconstruction_codec::CameraPose::default(),
            features: Vec::new(),
        };
        let payload = frame.to_bytes();
        assert_eq!(&payload[0..2], b"V8", "V8Frame::to_bytes should start with the v8 magic");
        let mut out = Vec::new();
        fao_v1::write_fao_container(&mut out, w, h, 30.0, 0.8,
            &[(Vec::new(), payload.clone(), payload[0], payload.len(), 0.0, 0.0)]).unwrap();
        out.extend_from_slice(&one_triangle_v8mx_trailer());

        let f = FaoFile::parse(out).expect("parses with a trailer appended");
        assert!(f.has_v8_webs(), "the V8MX trailer should have been found and parsed");
        assert_eq!(f.markers(), vec![Some(b'V')]);
        match f.decode_rgb(0) {
            Ok(rgb) => {
                assert_eq!(rgb.len(), w * h * 3);
                assert!(rgb.iter().any(|&b| b != 0), "a rasterized triangle should not be all-black");
            }
            Err(g) => panic!("expected a rendered frame, got a gap: {g}"),
        }
    }

    /// Without a V8MX trailer, the same frame is a named gap, not a silent black frame.
    #[test]
    fn a_v8_frame_with_no_trailer_is_a_named_gap_not_a_crash() {
        let (w, h) = (16usize, 16usize);
        let frame = fao_v1::v8_reconstruction_codec::V8Frame {
            camera_pose: fao_v1::v8_reconstruction_codec::CameraPose::default(),
            features: Vec::new(),
        };
        let payload = frame.to_bytes();
        let mut out = Vec::new();
        fao_v1::write_fao_container(&mut out, w, h, 30.0, 0.8,
            &[(Vec::new(), payload.clone(), payload[0], payload.len(), 0.0, 0.0)]).unwrap();
        let f = FaoFile::parse(out).unwrap();
        assert!(!f.has_v8_webs());
        assert_eq!(f.decode_rgb(0), Err(Gap::NeedsV8));
    }

    #[test]
    fn yuv_rgb_round_trip_is_close_on_flat_color() {
        let (w, h) = (8usize, 8usize);
        let rgb: Vec<u8> = (0..w * h).flat_map(|_| [120u8, 60, 200]).collect();
        let back = yuv_to_rgb(&rgb_to_yuv420p(&rgb, w, h), w, h);
        for (a, b) in rgb.iter().zip(&back) { assert!((*a as i32 - *b as i32).abs() <= 3, "{a} vs {b}"); }
    }


    /// The plane codec IS the frame codec's luma path: a plane coded alone
    /// decodes to exactly the luma the frame codec produces for the same image.
    #[test]
    fn a_plane_codes_exactly_like_a_frames_luma() {
        for (w, h) in [(48usize, 32usize), (512, 64)] {
            let rgb: Vec<u8> = (0..w * h * 3).map(|i| (((i / 3) % w * 7 ^ (i / 3) / w * 5) % 256) as u8).collect();
            let yuv = rgb_to_yuv420p(&rgb, w, h);
            for q in [10u8, 24, 80, 100] {
                let frame = fao_v1::decompress_frame_dct(&fao_v1::compress_frame_dct(&yuv, w, h, q), w, h).unwrap();
                let plane = fao_v1::decompress_plane_dct(&fao_v1::compress_plane_dct(&yuv[..w * h], w, h, q), w, h).unwrap();
                assert_eq!(plane, frame[..w * h], "{w}x{h} q{q}");
            }
        }
        let p = fao_v1::compress_plane_dct(&vec![9u8; 64], 8, 8, 50);
        assert!(fao_v1::decompress_plane_dct(&p, 16, 8).is_none(), "size mismatch rejected");
        assert!(fao_v1::decompress_plane_dct(&p[..5], 8, 8).is_none(), "truncated rejected");
    }

    /// Every truncation and a sweep of single-byte corruptions of a REAL .fao (key, patch and
    /// repeat frames from the transcoder), through parse and every frame's decode. The Player
    /// opens whatever file it is given -- and, since streaming playback, partial downloads -- so
    /// damaged input must come back as `None` / a `Gap`, never a panic.
    #[test]
    fn damaged_files_are_refused_not_panicked_on() {
        let have = std::process::Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false);
        if !have { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-damage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.mp4");
        assert!(std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=64x48:rate=10:duration=1.2", "-frames:v", "12", "-pix_fmt", "yuv420p"]).arg(&src).status().unwrap().success());
        let out = dir.join("clip.fao");
        crate::transcode::transcode_to_file_with(src.to_str().unwrap(), &out, 70, None, |_, _| {}, None, true).unwrap();
        let good = std::fs::read(&out).unwrap();
        let f = FaoFile::parse(good.clone()).expect("the undamaged file parses");
        let n = f.frame_count();
        assert!(n >= 10);
        for i in 0..n { f.decode_rgb(i).unwrap_or_else(|g| panic!("undamaged frame {i}: {g}")); }

        let mut cases: Vec<(String, Vec<u8>)> = (0..good.len()).map(|k| (format!("truncated to {k}"), good[..k].to_vec())).collect();
        for k in 0..good.len() {
            for v in [0x00u8, 0xFF] {
                if good[k] != v {
                    let mut c = good.clone();
                    c[k] = v;
                    cases.push((format!("byte {k} = {v:#04x}"), c));
                }
            }
        }
        let prev = std::panic::take_hook();
        // Silence the per-case output, but keep WHERE each panic happened for the report.
        static LAST_AT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
        std::panic::set_hook(Box::new(|info| {
            if let Ok(mut l) = LAST_AT.lock() { *l = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default(); }
        }));
        let mut panics: std::collections::BTreeMap<String, (usize, String)> = Default::default();
        for (what, data) in &cases {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let Some(f) = FaoFile::parse(data.clone()) {
                    for i in 0..f.frame_count().min(64) { let _ = f.decode_rgb(i); }
                }
            }));
            if let Err(e) = r {
                let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                let at = LAST_AT.lock().map(|l| l.clone()).unwrap_or_default();
                let key: String = format!("{at}  {}", msg.chars().take(90).collect::<String>());
                panics.entry(key).or_insert((0, what.clone())).0 += 1;
            }
        }
        std::panic::set_hook(prev);
        let _ = std::fs::remove_dir_all(&dir);
        let report: Vec<String> = panics.iter().map(|(m, (c, first))| format!("{c:5}x  first: {first:<22}  {m}")).collect();
        assert!(panics.is_empty(), "{} of {} damaged files panicked:\n{}", panics.values().map(|v| v.0).sum::<usize>(), cases.len(), report.join("\n"));
    }
}

