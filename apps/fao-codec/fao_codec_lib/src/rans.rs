//! # rANS entropy coder with per-context static tables
//!
//! zstd's entropy stage is generic: its FSE tables are built per
//! compression window over whatever bytes slide past, mixing spectral
//! bands whose statistics differ by orders of magnitude. This coder is
//! the dedicated alternative for the residual's coefficient stream:
//! the caller assigns every symbol a CONTEXT (here: spectral band
//! group × channel class), each context gets its own frequency table
//! built from the actual frame, tables travel compactly, and symbols
//! are coded exactly against their own distribution. Semi-adaptive
//! (per-frame static tables) rather than fully adaptive: rANS decodes
//! in reverse encode order, which makes on-line model updates awkward,
//! and per-frame tables capture most of the win at a fraction of the
//! complexity.
//!
//! Standard byte-renormalizing rANS: state u32, lower bound 1<<16,
//! frequencies normalized to 1<<12. Encoding walks symbols in reverse
//! and the byte stream is reversed on output so decoding is forward.

const PROB_BITS: u32 = 12;
const PROB_SCALE: u32 = 1 << PROB_BITS;
const RANS_L: u32 = 1 << 16;

#[derive(Clone)]
struct Table {
    freq: [u16; 256],
    start: [u16; 256],
    /// slot → symbol lookup for decode.
    slot_sym: Vec<u8>,
}

fn build_table(hist: &[u32; 256]) -> Option<Table> {
    let total: u64 = hist.iter().map(|&h| h as u64).sum();
    if total == 0 {
        return None;
    }
    // Normalize to PROB_SCALE, every seen symbol keeps freq >= 1.
    let mut freq = [0u16; 256];
    let mut assigned: u32 = 0;
    let mut max_sym = 0usize;
    let mut max_freq = 0u32;
    for s in 0..256 {
        if hist[s] == 0 {
            continue;
        }
        let f = ((hist[s] as u64 * PROB_SCALE as u64) / total).max(1) as u32;
        let f = f.min(PROB_SCALE - 1);
        freq[s] = f as u16;
        assigned += f;
        if f > max_freq {
            max_freq = f;
            max_sym = s;
        }
    }
    // Fix rounding drift on the most frequent symbol.
    let diff = PROB_SCALE as i64 - assigned as i64;
    let nf = freq[max_sym] as i64 + diff;
    if nf < 1 {
        return None; // pathological distribution; caller falls back
    }
    freq[max_sym] = nf as u16;
    let mut start = [0u16; 256];
    let mut acc = 0u32;
    let mut slot_sym = vec![0u8; PROB_SCALE as usize];
    for s in 0..256 {
        start[s] = acc as u16;
        for slot in acc..acc + freq[s] as u32 {
            slot_sym[slot as usize] = s as u8;
        }
        acc += freq[s] as u32;
    }
    Some(Table { freq, start, slot_sym })
}

fn serialize_table(t: &Table, out: &mut Vec<u8>) {
    // Count byte meanings: 1..=253 sparse entries; 0 = dense 256-entry
    // format; 255 is RESERVED as the absent-context marker at the call
    // site — a 254/255-symbol table therefore uses the dense format.
    // (Found the hard way: a 255-symbol table serialized as count 255
    // read back as "absent", misaligned the stream, and the slot
    // builder indexed past the table before the sum check could fire.)
    let n = t.freq.iter().filter(|&&f| f > 0).count();
    out.push(n as u8);
    if n > 253 {
        // Dense fallback: emit all 256 freqs as 12-bit pairs (384 B).
        // Marked by count byte 0 (a real table always has >=1 symbol).
        *out.last_mut().unwrap() = 0;
        let mut i = 0;
        while i < 256 {
            let a = t.freq[i] as u32;
            let b = if i + 1 < 256 { t.freq[i + 1] as u32 } else { 0 };
            out.push((a >> 4) as u8);
            out.push((((a & 0xF) << 4) | (b >> 8)) as u8);
            out.push((b & 0xFF) as u8);
            i += 2;
        }
        return;
    }
    for s in 0..256 {
        if t.freq[s] > 0 {
            out.push(s as u8);
            out.push((t.freq[s] >> 8) as u8);
            out.push((t.freq[s] & 0xFF) as u8);
        }
    }
}

fn deserialize_table(data: &[u8], off: &mut usize) -> Option<Table> {
    let n = *data.get(*off)? as usize;
    *off += 1;
    let mut freq = [0u16; 256];
    if n == 0 {
        // dense format
        let mut i = 0;
        while i < 256 {
            let b0 = *data.get(*off)? as u32;
            let b1 = *data.get(*off + 1)? as u32;
            let b2 = *data.get(*off + 2)? as u32;
            *off += 3;
            freq[i] = ((b0 << 4) | (b1 >> 4)) as u16;
            if i + 1 < 256 {
                freq[i + 1] = (((b1 & 0xF) << 8) | b2) as u16;
            }
            i += 2;
        }
    } else {
        for _ in 0..n {
            let s = *data.get(*off)? as usize;
            let hi = *data.get(*off + 1)? as u16;
            let lo = *data.get(*off + 2)? as u16;
            *off += 3;
            freq[s] = (hi << 8) | lo;
        }
    }
    let mut start = [0u16; 256];
    let mut acc = 0u32;
    let mut slot_sym = vec![0u8; PROB_SCALE as usize];
    for s in 0..256 {
        start[s] = acc as u16;
        // Guard BEFORE indexing: a corrupt/misaligned table must fail
        // to None, not panic mid-build.
        if acc + freq[s] as u32 > PROB_SCALE {
            return None;
        }
        for slot in acc..acc + freq[s] as u32 {
            slot_sym[slot as usize] = s as u8;
        }
        acc += freq[s] as u32;
    }
    if acc != PROB_SCALE {
        return None;
    }
    Some(Table { freq, start, slot_sym })
}

/// Encode `(context, symbol)` pairs given in DECODE order. Returns the
/// full self-contained blob: [n_ctx:u8][tables][sym_count:u32][rans bytes].
/// Returns None when any context's table is degenerate — caller keeps
/// its fallback path.
pub fn encode(pairs: &[(u8, u8)], n_ctx: usize) -> Option<Vec<u8>> {
    let mut hists = vec![[0u32; 256]; n_ctx];
    for &(c, s) in pairs {
        hists[c as usize][s as usize] += 1;
    }
    let mut tables: Vec<Option<Table>> = Vec::with_capacity(n_ctx);
    for h in &hists {
        if h.iter().any(|&v| v > 0) {
            tables.push(Some(build_table(h)?));
        } else {
            tables.push(None);
        }
    }
    let mut out = Vec::new();
    out.push(n_ctx as u8);
    for t in &tables {
        match t {
            Some(t) => serialize_table(t, &mut out),
            None => out.push(255), // 255 = absent context marker
        }
    }
    if std::env::var("FAO_RANS_DEBUG").is_ok() {
        eprintln!(
            "rans: {} syms, {} ctxs used, table_bytes {}",
            pairs.len(),
            tables.iter().filter(|t| t.is_some()).count(),
            out.len() - 1
        );
    }
    out.extend_from_slice(&(pairs.len() as u32).to_le_bytes());
    // rANS encode in reverse, then reverse the byte stream.
    let mut state: u32 = RANS_L;
    let mut bytes: Vec<u8> = Vec::with_capacity(pairs.len());
    for &(c, s) in pairs.iter().rev() {
        let t = tables[c as usize].as_ref().unwrap();
        let f = t.freq[s as usize] as u32;
        let st = t.start[s as usize] as u32;
        let x_max = ((RANS_L >> PROB_BITS) << 8) * f;
        while state >= x_max {
            bytes.push((state & 0xFF) as u8);
            state >>= 8;
        }
        state = ((state / f) << PROB_BITS) + (state % f) + st;
    }
    out.extend_from_slice(&state.to_le_bytes());
    bytes.reverse();
    out.extend_from_slice(&bytes);
    Some(out)
}

/// Decode against a caller-supplied context rule: `ctx_of` is called
/// with the symbol index AND the symbols decoded so far, and must return
/// the same context the encoder used. The history parameter is what
/// makes data-dependent context models (neighbor conditioning, run
/// states) decodable: the decoder has exactly the same past the encoder
/// had, one symbol at a time.
pub fn decode(blob: &[u8], ctx_of: &mut dyn FnMut(usize, &[u8]) -> u8) -> Option<Vec<u8>> {
    let mut off = 0usize;
    let n_ctx = *blob.get(off)? as usize;
    off += 1;
    let mut tables: Vec<Option<Table>> = Vec::with_capacity(n_ctx);
    for _ in 0..n_ctx {
        if *blob.get(off)? == 255 {
            off += 1;
            tables.push(None);
        } else {
            tables.push(Some(deserialize_table(blob, &mut off)?));
        }
    }
    let n_sym = u32::from_le_bytes([
        *blob.get(off)?,
        *blob.get(off + 1)?,
        *blob.get(off + 2)?,
        *blob.get(off + 3)?,
    ]) as usize;
    off += 4;
    let mut state = u32::from_le_bytes([
        *blob.get(off)?,
        *blob.get(off + 1)?,
        *blob.get(off + 2)?,
        *blob.get(off + 3)?,
    ]);
    off += 4;
    let mut out = Vec::with_capacity(n_sym);
    for i in 0..n_sym {
        let t = tables[ctx_of(i, &out) as usize].as_ref()?;
        let slot = state & (PROB_SCALE - 1);
        let s = t.slot_sym[slot as usize];
        let f = t.freq[s as usize] as u32;
        let st = t.start[s as usize] as u32;
        state = f * (state >> PROB_BITS) + slot - st;
        while state < RANS_L {
            state = (state << 8) | (*blob.get(off)? as u32);
            off += 1;
        }
        out.push(s);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_multi_context_skewed() {
        // Two contexts with very different distributions — like a DC band
        // vs a high-AC band. Roundtrip must be exact and the coded size
        // must beat a naive 1 byte/symbol on the skewed data.
        let mut pairs: Vec<(u8, u8)> = Vec::new();
        let mut x = 12345u32;
        for i in 0..20_000 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            let ctx = (i % 2) as u8;
            let s = if ctx == 0 {
                // near-zero-centered narrow
                ((x >> 24) as i32 % 5 - 2) as i8 as u8
            } else {
                // mostly zero with rare spikes
                if x % 17 == 0 { (x >> 20) as u8 } else { 0 }
            };
            pairs.push((ctx, s));
        }
        let blob = encode(&pairs, 2).expect("encode");
        assert!(
            blob.len() < pairs.len() / 2,
            "should beat 4 bits/symbol on this data: {} vs {}",
            blob.len(),
            pairs.len()
        );
        let mut i_ctx = 0usize;
        let _ = i_ctx;
        let decoded = decode(&blob, &mut |i, _hist| (i % 2) as u8).expect("decode");
        assert_eq!(decoded.len(), pairs.len());
        for (d, (_, s)) in decoded.iter().zip(&pairs) {
            assert_eq!(d, s);
        }
    }
}
