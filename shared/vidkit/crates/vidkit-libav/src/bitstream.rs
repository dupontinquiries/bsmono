//! Container packets into what a V4L2 stateful decoder takes. Pure.
//!
//! MP4/MKV carry H.264/HEVC as length-prefixed NAL units with the parameter
//! sets in the codec's extradata (avcC / hvcC); V4L2's H264/HEVC formats
//! take an Annex B byte stream (start codes, parameter sets in band). VP9 in
//! WebM may pack a hidden alt-ref and a shown frame into one "superframe";
//! V4L2's VP9 format takes one frame per buffer.
//!
//! libavcodec's bitstream filters do both, but bsf.h is not in
//! ffmpeg-sys-next's bindings and AVBSFContext would have to be declared by
//! hand -- a few dozen lines of pure Rust, tested against the CLI, are the
//! safer seam.

/// Length size and parameter sets (SPS, PPS; for HEVC VPS first) from
/// avcC or hvcC extradata. `None` when the extradata is already Annex B
/// (or absent): packets are then passed through.
pub fn param_sets(codec: Codec, extradata: &[u8]) -> Result<Option<(usize, Vec<Vec<u8>>)>, String> {
    if extradata.is_empty() || extradata.starts_with(&[0, 0, 1]) || extradata.starts_with(&[0, 0, 0, 1]) {
        return Ok(None);
    }
    let mut sets = Vec::new();
    let take = |d: &[u8], at: &mut usize| -> Result<Vec<u8>, String> {
        let n = u16::from_be_bytes([*d.get(*at).ok_or("short")?, *d.get(*at + 1).ok_or("short")?]) as usize;
        let s = d.get(*at + 2..*at + 2 + n).ok_or("parameter set past the end")?.to_vec();
        *at += 2 + n;
        Ok(s)
    };
    match codec {
        Codec::H264 => {
            let e = extradata;
            if e.len() < 7 || e[0] != 1 {
                return Err("not avcC".into());
            }
            let len = (e[4] & 3) as usize + 1;
            let mut at = 6;
            for _ in 0..(e[5] & 0x1f) {
                sets.push(take(e, &mut at)?);
            }
            let npps = *e.get(at).ok_or("no PPS count")?;
            at += 1;
            for _ in 0..npps {
                sets.push(take(e, &mut at)?);
            }
            Ok(Some((len, sets)))
        }
        Codec::Hevc => {
            let e = extradata;
            if e.len() < 23 {
                return Err("not hvcC".into());
            }
            let len = (e[21] & 3) as usize + 1;
            let mut at = 23;
            for _ in 0..e[22] {
                at += 1; // NAL type of this array
                let n = u16::from_be_bytes([*e.get(at).ok_or("short")?, *e.get(at + 1).ok_or("short")?]);
                at += 2;
                for _ in 0..n {
                    sets.push(take(e, &mut at)?);
                }
            }
            Ok(Some((len, sets)))
        }
        Codec::Vp9 => Ok(None),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
    Vp9,
}

/// One length-prefixed packet as Annex B, parameter sets first when
/// `sets` is given (key frames: a decoder joining there needs them).
pub fn to_annexb(pkt: &[u8], len_size: usize, sets: Option<&[Vec<u8>]>, out: &mut Vec<u8>) -> Result<(), String> {
    out.clear();
    for s in sets.unwrap_or(&[]) {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(s);
    }
    let mut at = 0;
    while at + len_size <= pkt.len() {
        let n = pkt[at..at + len_size].iter().fold(0usize, |a, &b| a << 8 | b as usize);
        at += len_size;
        let nal = pkt.get(at..at + n).ok_or("NAL unit past the end of the packet")?;
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
        at += n;
    }
    if at != pkt.len() {
        return Err("trailing bytes after the last NAL unit".into());
    }
    Ok(())
}

/// The frames of a VP9 packet: several for a superframe, else the packet.
pub fn vp9_frames(pkt: &[u8]) -> Vec<&[u8]> {
    let Some(&last) = pkt.last() else { return vec![] };
    if last & 0xe0 == 0xc0 {
        let frames = (last & 7) as usize + 1;
        let mag = ((last >> 3) & 3) as usize + 1;
        let index = 2 + mag * frames;
        if pkt.len() >= index && pkt[pkt.len() - index] == last {
            let mut out = Vec::with_capacity(frames);
            let mut at = 0usize;
            let mut idx = pkt.len() - index + 1;
            for _ in 0..frames {
                let size = (0..mag).fold(0usize, |a, i| a | (pkt[idx + i] as usize) << (8 * i));
                idx += mag;
                if at + size > pkt.len() - index {
                    return vec![pkt];
                }
                out.push(&pkt[at..at + size]);
                at += size;
            }
            return out;
        }
    }
    vec![pkt]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avcc_is_unpacked() {
        // version 1, profile 0x64, compat 0, level 0x1f, 4-byte lengths,
        // one SPS (3 bytes), one PPS (2 bytes).
        let e = [1, 0x64, 0, 0x1f, 0xff, 0xe1, 0, 3, 0x67, 1, 2, 1, 0, 2, 0x68, 9];
        let (len, sets) = param_sets(Codec::H264, &e).unwrap().unwrap();
        assert_eq!(len, 4);
        assert_eq!(sets, vec![vec![0x67, 1, 2], vec![0x68, 9]]);
        let mut out = Vec::new();
        to_annexb(&[0, 0, 0, 2, 0x65, 7, 0, 0, 0, 1, 0x06], 4, Some(&sets), &mut out).unwrap();
        assert_eq!(out, vec![0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 9, 0, 0, 0, 1, 0x65, 7, 0, 0, 0, 1, 0x06]);
        assert!(to_annexb(&[0, 0, 0, 9, 1], 4, None, &mut out).is_err());
    }

    #[test]
    fn annexb_extradata_passes_through() {
        assert_eq!(param_sets(Codec::H264, &[0, 0, 0, 1, 0x67]).unwrap(), None);
        assert_eq!(param_sets(Codec::Hevc, &[]).unwrap(), None);
    }

    #[test]
    fn superframes_split() {
        // Two frames of 3 and 2 bytes, 1-byte sizes: marker 0b110_00_001.
        let m = 0xc1u8;
        let pkt = [1, 2, 3, 4, 5, m, 3, 2, m];
        assert_eq!(vp9_frames(&pkt), vec![&[1u8, 2, 3][..], &[4, 5][..]]);
        assert_eq!(vp9_frames(&[9, 8, 7]), vec![&[9u8, 8, 7][..]]);
    }
}
