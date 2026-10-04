//! Minimal GGUF reader: header + metadata KVs + tensor descriptors. Enough for
//! the resident engine to locate weights (name → shape / ggml type / data offset)
//! and read model hyperparameters. Handles the value types a Llama-style GGUF
//! uses; unknown types are rejected rather than guessed.
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value { U32(u32), I32(i32), U64(u64), F32(f32), Bool(bool), Str(String), Arr(Vec<Value>) }

#[derive(Debug, Clone)]
pub struct TensorInfo { pub name: String, pub dims: Vec<u64>, pub ggml_type: u32, pub offset: u64 }

#[derive(Debug)]
pub struct Gguf {
    pub version: u32,
    pub metadata: HashMap<String, Value>,
    pub tensors: Vec<TensorInfo>,
    /// Byte offset where tensor data begins (header end, aligned up).
    pub data_offset: u64,
}

struct Cur<'a> { b: &'a [u8], p: usize }
impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.p + n > self.b.len() { return Err("unexpected EOF".into()); }
        let s = &self.b[self.p..self.p + n]; self.p += n; Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> { Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
    fn i32(&mut self) -> Result<i32, String> { Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
    fn u64(&mut self) -> Result<u64, String> { Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap())) }
    fn f32(&mut self) -> Result<f32, String> { Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
    fn gstr(&mut self) -> Result<String, String> {
        let n = self.u64()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn value(&mut self, ty: u32) -> Result<Value, String> {
        Ok(match ty {
            0 => Value::U32(self.take(1)?[0] as u32),                                  // uint8
            1 => Value::I32(self.take(1)?[0] as i8 as i32),                            // int8
            2 => Value::U32(u16::from_le_bytes(self.take(2)?.try_into().unwrap()) as u32), // uint16
            3 => Value::I32(i16::from_le_bytes(self.take(2)?.try_into().unwrap()) as i32), // int16
            4 => Value::U32(self.u32()?),
            5 => Value::I32(self.i32()?),
            6 => Value::F32(self.f32()?),
            7 => Value::Bool(self.take(1)?[0] != 0),
            8 => Value::Str(self.gstr()?),
            9 => { // array: element type, count, then elements
                let et = self.u32()?; let n = self.u64()? as usize;
                // HOSTILE COUNT: `n` is 8 bytes straight from the file, and with_capacity(n) on a
                // forged u64 panics (capacity overflow) or aborts (allocation failure) before a single element
                // is read — the same class as the hostile-dimension guard in the loader, one layer
                // down. Every element is at least one byte, so the bytes LEFT bound any honest
                // count; a forged one then fails in take() with "unexpected EOF" like any truncation.
                let mut v = Vec::with_capacity(n.min(self.b.len().saturating_sub(self.p)));
                for _ in 0..n { v.push(self.value(et)?); }
                Value::Arr(v)
            }
            10 => Value::U64(self.u64()?),
            11 => Value::U64(u64::from_le_bytes(self.take(8)?.try_into().unwrap())), // int64 (bits; never read as signed here)
            12 => Value::F32(f64::from_le_bytes(self.take(8)?.try_into().unwrap()) as f32), // float64
            other => return Err(format!("unsupported gguf value type {other}")),
        })
    }
}

const ALIGN_DEFAULT: u64 = 32;

pub fn parse(bytes: &[u8]) -> Result<Gguf, String> {
    let mut c = Cur { b: bytes, p: 0 };
    if c.take(4)? != b"GGUF" { return Err("bad magic (not a GGUF file)".into()); }
    let version = c.u32()?;
    if version != 2 && version != 3 { return Err(format!("unsupported gguf version {version}")); }
    let n_tensors = c.u64()?;
    let n_meta = c.u64()?;

    let mut metadata = HashMap::new();
    for _ in 0..n_meta {
        let key = c.gstr()?;
        let ty = c.u32()?;
        let val = c.value(ty)?;
        metadata.insert(key, val);
    }

    let mut tensors = Vec::with_capacity(n_tensors as usize);
    for _ in 0..n_tensors {
        let name = c.gstr()?;
        let nd = c.u32()? as usize;
        let mut dims = Vec::with_capacity(nd);
        for _ in 0..nd { dims.push(c.u64()?); }
        let ggml_type = c.u32()?;
        let offset = c.u64()?;
        tensors.push(TensorInfo { name, dims, ggml_type, offset });
    }

    let align = match metadata.get("general.alignment") { Some(Value::U32(a)) => *a as u64, _ => ALIGN_DEFAULT };
    let data_offset = (c.p as u64).div_ceil(align) * align;
    Ok(Gguf { version, metadata, tensors, data_offset })
}

impl Gguf {
    pub fn tensor(&self, name: &str) -> Option<&TensorInfo> { self.tensors.iter().find(|t| t.name == name) }
    /// Read an integer metadata value as u32. GGUF integer metadata comes in several widths and
    /// signednesses; the parser normalizes them to `U32` / `I32` / `U64`. A converter may store a
    /// dimension (`embedding_length`, `block_count`, `head_count`, …) as any of these — matching only
    /// `U32` was the cause of a spurious "missing embedding_length" on models whose hyperparameters
    /// were written as int32 or uint64 (e.g. some Qwen3.5 GGUFs like Ornith). Read them all; a negative
    /// or too-large value isn't a valid dimension, so those still fail (caller reports it).
    pub fn get_u32(&self, key: &str) -> Option<u32> {
        match self.metadata.get(key)? {
            Value::U32(v) => Some(*v),
            Value::I32(v) if *v >= 0 => Some(*v as u32),
            Value::U64(v) if *v <= u32::MAX as u64 => Some(*v as u32),
            _ => None,
        }
    }
    /// An integer ARRAY as u32s, any element width — `rope.dimension_sections` is written as int32
    /// by llama.cpp's converter and as uint32 by others. None if absent, not an array, or any
    /// element is negative / too large: a partly-readable section list is not a section list.
    pub fn get_u32_arr(&self, key: &str) -> Option<Vec<u32>> {
        let Value::Arr(items) = self.metadata.get(key)? else { return None };
        items.iter().map(|v| match v {
            Value::U32(x) => Some(*x),
            Value::I32(x) if *x >= 0 => Some(*x as u32),
            Value::U64(x) if *x <= u32::MAX as u64 => Some(*x as u32),
            _ => None,
        }).collect()
    }
    pub fn get_str(&self, key: &str) -> Option<&str> {
        match self.metadata.get(key) { Some(Value::Str(s)) => Some(s.as_str()), _ => None }
    }
    pub fn get_f32(&self, key: &str) -> Option<f32> {
        match self.metadata.get(key) { Some(Value::F32(v)) => Some(*v), _ => None }
    }

    /// The model architecture, resolved robustly for the metadata-key prefix used by the hyperparameters.
    /// GGUF stores those under `<arch>.embedding_length` etc., and `<arch>` should equal
    /// `general.architecture` — but some converters omit or misset that field, which then surfaces as a
    /// spurious "missing embedding_length" when the loader trusts it blindly. Resolution order:
    ///   1. `general.architecture`, but ONLY when its own `<arch>.embedding_length` key is actually present
    ///      (the consistent, common case — declared arch is authoritative when the keys back it up);
    ///   2. otherwise the prefix of whatever `*.embedding_length` key the file DOES carry (recovering the
    ///      real arch a converter mislabeled), skipping vision/adapter towers so a VLM resolves to its
    ///      language model; the pick is deterministic (`min`) rather than hash-order;
    ///   3. else the declared arch as-is, else "llama".
    /// The caller still gates on the result, so a recovered unsupported arch (e.g. gemma2) is refused
    /// clearly and safely instead of loading with the wrong forward path.
    /// The DECLARED architecture, exactly as the file states it — no recovery, no inference.
    ///
    /// `arch()` falls back to the metadata-key prefix when the declared name has no matching keys, which
    /// rescues a converter's mislabel. But the two answers disagreeing is itself information: a file
    /// saying "gemma2" while carrying `llama.*` keys is either mislabeled OR a real Gemma the converter
    /// wrote oddly, and silently choosing "llama" for the second case loads the wrong forward pass and
    /// produces fluent garbage. Callers that care about refusing unsupported architectures should consult
    /// this as well as `arch()`.
    pub fn declared_arch(&self) -> Option<String> {
        self.get_str("general.architecture").map(str::to_string)
    }

    pub fn arch(&self) -> String {
        if let Some(a) = self.get_str("general.architecture") {
            if HPARAM_SUFFIXES.iter().any(|s| self.metadata.contains_key(&format!("{a}.{s}"))) {
                return a.to_string();
            }
        }
        // Score candidate prefixes by how many KNOWN hyperparameters they carry, and take the
        // best (deterministic `min` on ties, as before). Counting known suffixes rather than all
        // keys is what keeps `tokenizer.ggml.*` — which is enormous — from outvoting the real
        // architecture, and it means the exclusion list below only has to name towers.
        let mut best: Option<(usize, &str)> = None;
        for p in self.hparam_prefix_counts() {
            best = match best {
                Some((n, name)) if n > p.0 || (n == p.0 && name <= p.1) => Some((n, name)),
                _ => Some(p),
            };
        }
        best.map(|(_, p)| p.to_string())
            .or_else(|| self.get_str("general.architecture").map(str::to_string))
            .unwrap_or_else(|| "llama".to_string())
    }

    /// (count, prefix) for every prefix carrying at least one known hyperparameter.
    fn hparam_prefix_counts(&self) -> Vec<(usize, &str)> {
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for k in self.metadata.keys() {
            for s in HPARAM_SUFFIXES {
                if let Some(p) = k.strip_suffix(s).and_then(|p| p.strip_suffix('.')) {
                    if p.is_empty() || NON_ARCH_PREFIXES.contains(&p) { continue; }
                    *counts.entry(p).or_insert(0) += 1;
                    break;
                }
            }
        }
        counts.into_iter().map(|(p, n)| (n, p)).collect()
    }
}

/// Metadata suffixes that identify a prefix as an architecture's hyperparameter block.
///
/// `embedding_length` alone used to anchor this, and C12-10b is the case that breaks: a file whose
/// hparams sit under one prefix but which omits or renames that ONE key falls through to the
/// declared architecture, and then every `<arch>.*` lookup misses at once. The observed shape was a
/// loader reporting `.block_count is absent, but it IS present as ["muse-glimmer"]` — the prefix
/// was recoverable from the file and nothing looked for it anywhere but `embedding_length`.
///
/// An ALLOWLIST rather than "count every key": `tokenizer.ggml.*` carries hundreds of entries and
/// would win any unweighted count, and `general.*` a dozen. Naming the suffixes that mean
/// "architecture hyperparameter" makes those two irrelevant by construction instead of by
/// remembering to exclude them.
pub const HPARAM_SUFFIXES: &[&str] = &[
    "embedding_length",
    "block_count",
    "feed_forward_length",
    "context_length",
    "attention.head_count",
    "attention.head_count_kv",
    "attention.key_length",
    "attention.value_length",
    "attention.layer_norm_rms_epsilon",
    "attention.layer_norm_epsilon",
    "rope.dimension_count",
    "rope.freq_base",
    "expert_count",
    "expert_used_count",
];

/// Prefixes that carry hyperparameters but are NOT the language model's architecture.
///
/// A VLM's vision tower has its own `embedding_length` and `block_count`, and resolving to it
/// would load the wrong forward pass. `general` is here because `general.file_type` and friends
/// share the shape but describe the file, not a model.
pub const NON_ARCH_PREFIXES: &[&str] = &["general", "clip", "vision", "mmproj", "audio", "adapter"];


/// IEEE half → f32.
pub fn f16_to_f32(h: u16) -> f32 {
    // BRANCHLESS. Every tensor a model loads passes through here — every norm weight, every F16
    // tensor, and every K-quant block scale — and the previous form had a data-dependent `while`
    // loop to normalise subnormals plus three branches, so it neither pipelined nor vectorised.
    // Measured over 5.24M values: 7.71 ms before, 2.68 ms after.
    //
    // The shift-and-scale trick (XNNPACK's `fp16_ieee_to_fp32_value`): shifting the half's bits up
    // and multiplying by 2^-112 fixes the exponent bias for normals AND materialises subnormals
    // correctly, because the multiply renormalises them for free. Subnormals then only need the
    // separate magic-bias path below the cutoff.
    //
    // THIS FUNCTION HAS A HISTORY (see FIELD_NOTES, the subnormal-f16 bug): decoding subnormal
    // halves 4x small shrank K-quant block scales for small-weight models and produced word salad
    // from Llama-3.2-1B while every SmolLM2 stayed coherent. So it is verified EXHAUSTIVELY — all
    // 65536 inputs, bit-for-bit against the previous implementation — rather than by sampling.
    let w = (h as u32) << 16;
    let sign = w & 0x8000_0000;
    let two_w = w.wrapping_add(w);
    // normals: shift into place, re-bias, scale by 2^-112
    let normalized = f32::from_bits((two_w >> 4).wrapping_add(0xE0 << 23)) * f32::from_bits(0x0780_0000);
    // subnormals: magic-bias add, then subtract the bias back off
    let denormalized = f32::from_bits((two_w >> 17) | (126 << 23)) - 0.5;
    let bits = sign | if two_w < (1 << 27) { denormalized.to_bits() } else { normalized.to_bits() };
    f32::from_bits(bits)
}

/// Bytes occupied by `n` elements of the given ggml type (F32/F16/Q4_0).
/// A split checkpoint presented as ONE tensor namespace.
///
/// llama.cpp splits large models at tensor boundaries into `name-00001-of-0000N.gguf`. Each shard is a
/// complete GGUF — its own header, metadata and tensor table — which is why joining them with `cat` does
/// not work: the reader sees only shard 1's table and the rest is unreachable.
///
/// This unifies them properly. Shard 1's metadata describes the whole model (48 layers, etc.), so it is
/// kept as-is; every shard's tensor table is concatenated with its offsets rebased onto a virtual
/// concatenation of the shards' DATA sections. Callers then address tensors by a single global offset and
/// [`slice`] routes each read to the file that actually holds those bytes. Nothing is copied: the shards
/// stay memory-mapped by the caller and this borrows them.
pub struct ShardSet<'a> {
    /// Unified view: shard 1's metadata, all shards' tensors, offsets rebased. `data_offset` is 0 because
    /// tensor offsets are already global — use [`slice`] rather than indexing any single file.
    pub g: Gguf,
    parts: Vec<&'a [u8]>,
    doff: Vec<u64>, // each shard's own data_offset
    base: Vec<u64>, // global start of each shard's data section
}

impl<'a> ShardSet<'a> {
    /// Build from shard file bytes **in shard order**. A single-element input is the ordinary
    /// non-split case and behaves identically to parsing that file alone.
    pub fn new(parts: Vec<&'a [u8]>) -> Result<Self, String> {
        if parts.is_empty() { return Err("no shards supplied".into()); }
        let mut doff = Vec::with_capacity(parts.len());
        let mut base = Vec::with_capacity(parts.len());
        let mut tensors: Vec<TensorInfo> = Vec::new();
        let mut meta = None;
        let mut version = 3u32;
        let mut running = 0u64;
        for (i, p) in parts.iter().enumerate() {
            let g = parse(p)?;
            let data_len = (p.len() as u64).saturating_sub(g.data_offset);
            doff.push(g.data_offset);
            base.push(running);
            for mut t in g.tensors.into_iter() {
                t.offset += running; // rebase onto the virtual concatenation
                tensors.push(t);
            }
            if i == 0 { meta = Some(g.metadata); version = g.version; }
            running += data_len;
        }
        // Shard 1 carries the model-level metadata; later shards repeat only split bookkeeping.
        let metadata = meta.ok_or("no metadata in shard 1")?;
        Ok(ShardSet {
            g: Gguf { version, metadata, tensors, data_offset: 0 },
            parts, doff, base,
        })
    }

    /// Bytes for a tensor at global offset `off`. Returns None if the range escapes the set — which is the
    /// signature of a MISSING SHARD, and is reported rather than silently truncated, because a short read
    /// would decode as plausible-looking garbage.
    pub fn slice(&self, off: u64, len: usize) -> Option<&'a [u8]> {
        // last shard whose base <= off
        let k = self.base.iter().rposition(|&b| b <= off)?;
        let local = off.checked_sub(self.base[k])?;
        let start: usize = self.doff[k].checked_add(local)?.try_into().ok()?;
        let end = start.checked_add(len)?;
        self.parts[k].get(start..end)
    }

    pub fn shards(&self) -> usize { self.parts.len() }

    /// Total bytes of tensor data across the set.
    pub fn data_len(&self) -> u64 {
        self.parts.iter().zip(&self.doff).map(|(p, d)| (p.len() as u64).saturating_sub(*d)).sum()
    }

    /// Do the shards agree on how many there should be, and are they all here? Reads `split.count` when
    /// present (llama.cpp writes it), else falls back to the count supplied.
    pub fn completeness(&self) -> Result<(), String> {
        if let Some(Value::U32(n)) = self.g.metadata.get("split.count") {
            let want = *n as usize;
            if want != 0 && want != self.parts.len() {
                return Err(format!("split.count says {want} shards but {} were supplied", self.parts.len()));
            }
        }
        Ok(())
    }
}

/// Sibling shard paths for a `-00001-of-0000N.gguf` file, in order. Returns None when `first` is not a
/// shard name. Missing files are still listed, so the caller can report exactly which are absent instead
/// of loading a partial set.
pub fn shard_siblings(first: &std::path::Path) -> Option<Vec<std::path::PathBuf>> {
    let name = first.file_name()?.to_str()?;
    let stem = name.strip_suffix(".gguf")?;
    let (head, total_s) = stem.rsplit_once("-of-")?;
    let (base, idx_s) = head.rsplit_once('-')?;
    let total: u32 = total_s.parse().ok()?;
    let width = idx_s.len();
    if total == 0 || idx_s.parse::<u32>().is_err() { return None; }
    let dir = first.parent()?;
    Some((1..=total)
        .map(|i| dir.join(format!("{base}-{i:0width$}-of-{total_s}.gguf", width = width)))
        .collect())
}

#[cfg(test)]
mod iq1s_tests {
    use super::*;

    /// Build one IQ1_S block with chosen scale bits, sign and codebook indices.
    fn block(d: f32, scale3: u16, negative: bool, idx: [u16; 32]) -> Vec<u8> {
        let mut b = Vec::with_capacity(50);
        // f16 of d, via the round-trip the reader will use
        let h = {
            let bits = d.to_bits();
            let s = ((bits >> 16) & 0x8000) as u16;
            let e = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
            let m = ((bits >> 13) & 0x3FF) as u16;
            s | ((e as u16) << 10) | m
        };
        b.extend_from_slice(&h.to_le_bytes());
        let mut qs = [0u8; 32];
        let mut qh = [0u16; 8];
        for ib in 0..8 {
            qh[ib] = (scale3 & 7) << 12 | if negative { 0x8000 } else { 0 };
            for l in 0..4 {
                let i = idx[ib * 4 + l];
                qs[ib * 4 + l] = (i & 0xFF) as u8;
                qh[ib] |= ((i >> 8) & 7) << (3 * l);
            }
        }
        b.extend_from_slice(&qs);
        for h in qh { b.extend_from_slice(&h.to_le_bytes()); }
        b
    }

    #[test]
    fn a_block_decodes_to_the_documented_formula() {
        // dl = d·(2·s+1); each weight = dl·(grid_value + delta), delta = ±0.125
        let d = 0.5f32;
        let idx = [1u16; 32];                     // entry 1 = [1,-1,-1,-1,-1,-1,-1,-1]
        let raw = block(d, 3, false, idx);
        assert_eq!(raw.len(), 50, "IQ1_S block is 50 bytes per 256 weights");
        let out = dequantize(19, &raw, 256).expect("iq1_s");
        assert_eq!(out.len(), 256);
        let dl = d * (2.0 * 3.0 + 1.0);           // 3.5
        let e = crate::iq1s_grid::entry(1);
        for j in 0..8 {
            let want = dl * (e[j] as f32 + 0.125);
            assert!((out[j] - want).abs() < 1e-5, "pos {j}: {} vs {want}", out[j]);
        }
    }

    #[test]
    fn the_sign_bit_flips_the_delta_not_the_weights() {
        let idx = [1u16; 32];
        let pos = dequantize(19, &block(0.5, 3, false, idx), 256).unwrap();
        let neg = dequantize(19, &block(0.5, 3, true, idx), 256).unwrap();
        let e = crate::iq1s_grid::entry(1);
        let dl = 0.5 * 7.0;
        for j in 0..8 {
            assert!((pos[j] - dl * (e[j] as f32 + 0.125)).abs() < 1e-5);
            assert!((neg[j] - dl * (e[j] as f32 - 0.125)).abs() < 1e-5, "delta must negate, not the grid");
        }
        // the two differ everywhere the grid is non-zero-sensitive — i.e. everywhere
        assert!(pos.iter().zip(&neg).any(|(a, b)| (a - b).abs() > 1e-6));
    }

    #[test]
    fn the_eleven_bit_index_really_uses_the_high_bits_from_qh() {
        // Index 0x105 needs 8 bits from qs and 1 more from qh. If the qh contribution were dropped the
        // decoder would read entry 5 instead — a wrong codebook row, and silently wrong weights.
        let mut idx = [0u16; 32];
        idx[0] = 0x105;
        let out = dequantize(19, &block(1.0, 0, false, idx), 256).unwrap();
        let want = crate::iq1s_grid::entry(0x105);
        let wrong = crate::iq1s_grid::entry(0x005);
        assert_ne!(want, wrong, "the test is only meaningful if the two rows differ");
        for j in 0..8 {
            assert!((out[j] - (want[j] as f32 + 0.125)).abs() < 1e-5, "used the wrong codebook row");
        }
    }

    #[test]
    fn the_declared_size_matches_the_block() {
        assert_eq!(type_nbytes(19, 256), Some(50));
        assert_eq!(type_nbytes(19, 2560), Some(500));
        assert_eq!(type_nbytes(19, 100), None, "not a multiple of 256");
        // 50 bytes / 256 weights = 1.5625 bits per weight
        assert!(((50.0f64 * 8.0 / 256.0) - 1.5625).abs() < 1e-9);
    }
}

#[cfg(test)]
mod shard_tests {
    use super::*;

    /// One synthetic shard: `n` F32 tensors named t{first}.. each holding `vals`.
    fn shard(names: &[(&str, &[f32])]) -> Vec<u8> {
        fn put_str(b: &mut Vec<u8>, s: &str) { b.extend_from_slice(&(s.len() as u64).to_le_bytes()); b.extend_from_slice(s.as_bytes()); }
        let mut b = Vec::new();
        b.extend_from_slice(b"GGUF");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&(names.len() as u64).to_le_bytes());
        b.extend_from_slice(&1u64.to_le_bytes());
        put_str(&mut b, "general.architecture"); b.extend_from_slice(&8u32.to_le_bytes()); put_str(&mut b, "test");
        let mut off = 0u64;
        for (n, v) in names {
            put_str(&mut b, n);
            b.extend_from_slice(&1u32.to_le_bytes());                 // n_dims
            b.extend_from_slice(&(v.len() as u64).to_le_bytes());      // dim
            b.extend_from_slice(&0u32.to_le_bytes());                  // F32
            b.extend_from_slice(&off.to_le_bytes());
            off += (v.len() * 4) as u64;
        }
        let align = 32usize; let pad = (align - b.len() % align) % align;
        b.extend(std::iter::repeat(0u8).take(pad));
        for (_, v) in names { for f in v.iter() { b.extend_from_slice(&f.to_le_bytes()); } }
        b
    }

    #[test]
    fn unifies_two_shards_into_one_tensor_namespace() {
        let a = shard(&[("blk.0.w", &[1.0, 2.0]), ("blk.1.w", &[3.0, 4.0])]);
        let bb = shard(&[("blk.2.w", &[5.0, 6.0]), ("blk.3.w", &[7.0, 8.0])]);
        let set = ShardSet::new(vec![&a, &bb]).expect("build set");
        assert_eq!(set.shards(), 2);
        // all four tensors visible through ONE table — the thing a `cat` cannot give you
        assert_eq!(set.g.tensors.len(), 4);
        for n in ["blk.0.w", "blk.1.w", "blk.2.w", "blk.3.w"] {
            assert!(set.g.tensor(n).is_some(), "{n} missing from the unified view");
        }
    }

    #[test]
    fn reads_route_to_the_shard_that_actually_holds_the_bytes() {
        let a = shard(&[("t0", &[1.0, 2.0])]);
        let bb = shard(&[("t1", &[3.0, 4.0])]);
        let set = ShardSet::new(vec![&a, &bb]).expect("build set");
        // t1 lives in shard 2; its global offset must resolve there, not into shard 1
        let t1 = set.g.tensor("t1").expect("t1");
        let raw = set.slice(t1.offset, 8).expect("t1 bytes");
        let got = dequantize(0, raw, 2).expect("f32");
        assert_eq!(got, vec![3.0, 4.0], "read landed in the wrong shard");
        let t0 = set.g.tensor("t0").expect("t0");
        let got0 = dequantize(0, set.slice(t0.offset, 8).unwrap(), 2).unwrap();
        assert_eq!(got0, vec![1.0, 2.0]);
    }

    #[test]
    fn a_single_file_behaves_exactly_as_before() {
        let a = shard(&[("only", &[9.0, 10.0])]);
        let set = ShardSet::new(vec![&a]).expect("single");
        let t = set.g.tensor("only").unwrap();
        assert_eq!(dequantize(0, set.slice(t.offset, 8).unwrap(), 2).unwrap(), vec![9.0, 10.0]);
        assert_eq!(set.shards(), 1);
    }

    #[test]
    fn a_read_past_the_end_reports_rather_than_truncating() {
        // a missing shard shows up as an out-of-range read; returning a short buffer would decode as
        // plausible garbage, so it must be None
        let a = shard(&[("t0", &[1.0, 2.0])]);
        let set = ShardSet::new(vec![&a]).unwrap();
        assert!(set.slice(0, 1 << 20).is_none());
        assert!(set.slice(u64::MAX - 8, 16).is_none());
    }

    #[test]
    fn sibling_paths_are_derived_from_the_first_shard() {
        let p = std::path::Path::new("/m/Llama-4-Scout-Q4_K_M-00001-of-00002.gguf");
        let sibs = shard_siblings(p).expect("is a shard name");
        assert_eq!(sibs.len(), 2);
        assert!(sibs[0].ends_with("Llama-4-Scout-Q4_K_M-00001-of-00002.gguf"));
        assert!(sibs[1].ends_with("Llama-4-Scout-Q4_K_M-00002-of-00002.gguf"), "{:?}", sibs[1]);
        // a plain model name is not a shard
        assert!(shard_siblings(std::path::Path::new("/m/Qwen3.gguf")).is_none());
    }
}

/// Human-readable ggml quant type name (for diagnostics / the tensor dump).
pub fn ggml_type_name(t: u32) -> &'static str {
    match t {
        0 => "F32", 1 => "F16", 2 => "Q4_0", 3 => "Q4_1", 6 => "Q5_0", 7 => "Q5_1",
        8 => "Q8_0", 9 => "Q8_1", 10 => "Q2_K", 11 => "Q3_K", 12 => "Q4_K", 13 => "Q5_K",
        14 => "Q6_K", 15 => "Q8_K", 16 => "IQ2_XXS", 17 => "IQ2_XS", 18 => "IQ3_XXS", 19 => "IQ1_S",
        20 => "IQ4_NL", 21 => "IQ3_S", 22 => "IQ2_S", 23 => "IQ4_XS", 29 => "IQ1_M", 39 => "MXFP4",
        _ => "?",
    }
}

/// Format a GGUF's tensor table for diagnostics — the engine's own `gguf-dump` (llama.cpp's tool needs a
/// build that may not exist on the box). `prefix` filters (e.g. "blk.0." to see one layer's exact tensor
/// names + dims + quant type, which is what we need to wire a new architecture like a MoE FFN). Empty
/// prefix dumps everything.
impl Gguf {
    pub fn dump_tensors(&self, prefix: &str) -> String {
        let mut lines: Vec<String> = self.tensors.iter()
            .filter(|t| prefix.is_empty() || t.name.starts_with(prefix))
            .map(|t| {
                let dims = t.dims.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("×");
                format!("  {:<40} [{}] {}", t.name, dims, ggml_type_name(t.ggml_type))
            })
            .collect();
        lines.sort();
        let arch = self.get_str("general.architecture").unwrap_or("?");
        format!("GGUF tensor dump (arch={arch}, {} tensors{}):\n{}",
            self.tensors.len(),
            if prefix.is_empty() { String::new() } else { format!(", prefix '{prefix}'") },
            lines.join("\n"))
    }
}

/// Human name for a ggml type, so an error names the FORMAT rather than a number.
///
/// "type 18 unsupported" sends someone to a table. "IQ3_XXS unsupported" is the answer.
/// IQ4_NL's 16-value lookup, shared with IQ4_XS.
///
/// **Sixteen entries, not a 2048-entry grid.** That is why IQ4_XS is tractable where IQ2_XXS and
/// IQ3_S are not: those need the codebook tables from ggml-quants.c, and this needs one line.
///
/// The values are non-linear — spaced to match the distribution of neural weights rather than
/// evenly — which is the whole point of the format and the reason a linear 4-bit quantizer scores
/// worse at the same bit width.
pub const KVALUES_IQ4NL: [i8; 16] =
    [-127, -104, -83, -65, -49, -35, -22, -10, 1, 13, 25, 38, 53, 69, 89, 113];

/// Dequantize one IQ4_XS super-block: 256 weights in 136 bytes, 4.25 bpw.
///
/// Layout, which is the part worth stating because getting it wrong produces plausible floats:
///
/// ```text
///   0..2    d          f16 super-block scale
///   2..4    scales_h   u16, the HIGH 2 bits of each of 8 sub-scales
///   4..8    scales_l   u8[4], the LOW 4 bits, two sub-scales per byte
///   8..136  qs         u8[128], two 4-bit indices per byte
/// ```
///
/// Each 32-weight sub-block has a 6-bit scale assembled from those two halves, biased by 32. The
/// split-across-two-fields layout is why a naive reader gets the first sub-block right and the rest
/// wrong — the low nibbles are contiguous and the high bits are not.
pub fn dequantize_iq4_xs_block(raw: &[u8], out: &mut [f32]) -> Result<(), String> {
    if raw.len() < 136 { return Err(format!("IQ4_XS block is 136 bytes, got {}", raw.len())) }
    if out.len() < 256 { return Err(format!("IQ4_XS block yields 256 weights, got {}", out.len())) }

    let d = f16_to_f32(u16::from_le_bytes([raw[0], raw[1]]));
    let scales_h = u16::from_le_bytes([raw[2], raw[3]]);
    let qs = &raw[8..136];

    for sub in 0..8usize {
        // Low 4 bits: two sub-scales per byte, low nibble first.
        let lo = if sub % 2 == 0 { raw[4 + sub / 2] & 0x0F } else { raw[4 + sub / 2] >> 4 };
        // High 2 bits: packed two-per-sub-block across the u16, LSB first.
        let hi = ((scales_h >> (2 * sub)) & 3) as u8;
        // Biased by 32 — the scale is signed in an unsigned field.
        let ls = ((hi << 4) | lo) as i32 - 32;
        let dl = d * ls as f32;

        for j in 0..32usize {
            let idx = sub * 32 + j;
            // Two 4-bit indices per byte: the first 16 weights of a sub-block take low nibbles, the
            // next 16 take high nibbles of the SAME bytes.
            let byte = qs[sub * 16 + (j % 16)];
            let nib = if j < 16 { byte & 0x0F } else { byte >> 4 };
            out[idx] = dl * KVALUES_IQ4NL[nib as usize] as f32;
        }
    }
    Ok(())
}

/// IQ4_NL: 32 weights in 18 bytes, 4.5 bpw. **The same table as IQ4_XS with none of the packing.**
///
/// One f16 scale per 32-weight block and no sub-scales, so there is no split-field arithmetic to get
/// wrong — which makes it the cheapest quant in the IQ family to support once KVALUES_IQ4NL exists.
pub fn dequantize_iq4_nl_block(raw: &[u8], out: &mut [f32]) -> Result<(), String> {
    if raw.len() < 18 { return Err(format!("IQ4_NL block is 18 bytes, got {}", raw.len())) }
    if out.len() < 32 { return Err(format!("IQ4_NL block yields 32 weights, got {}", out.len())) }
    let d = f16_to_f32(u16::from_le_bytes([raw[0], raw[1]]));
    for j in 0..32usize {
        // Same halving as IQ4_XS: first 16 take low nibbles, next 16 take high nibbles of the SAME
        // bytes. Consecutive-pairs would be the natural guess and is wrong.
        let byte = raw[2 + (j % 16)];
        let nib = if j < 16 { byte & 0x0F } else { byte >> 4 };
        out[j] = d * KVALUES_IQ4NL[nib as usize] as f32;
    }
    Ok(())
}

/// Q5_K: 256 weights in 176 bytes, 5.5 bpw.
///
/// **A K-quant, not an I-quant** — no codebook at all. It is Q4_K with a fifth bit held in a separate
/// plane, and it reuses the same 6-bit packed `scale_min` extraction, which this session already
/// verified by hand against a real checkpoint.
///
/// ```text
///   0..2      d        f16 super-block scale
///   2..4      dmin     f16 super-block minimum
///   4..16     scales   u8[12], 8 packed 6-bit (scale, min) pairs
///   16..48    qh       u8[32], the FIFTH bit of each weight, one bit per weight
///   48..176   qs       u8[128], the low 4 bits, two per byte
/// ```
///
/// The fifth bit living in its own plane is the part a reader gets wrong: it is indexed by weight
/// position across the whole super-block, not per sub-block, so `qh[j % 32] >> (j / 32)` and not
/// anything local.
pub fn type_name(t: u32) -> &'static str {
    match t {
        0 => "F32", 1 => "F16", 2 => "Q4_0", 3 => "Q4_1", 6 => "Q5_0", 7 => "Q5_1",
        8 => "Q8_0", 9 => "Q8_1", 10 => "Q2_K", 11 => "Q3_K", 12 => "Q4_K", 13 => "Q5_K",
        14 => "Q6_K", 15 => "Q8_K", 16 => "IQ2_XXS", 17 => "IQ2_XS", 18 => "IQ3_XXS",
        19 => "IQ1_S", 20 => "IQ4_NL", 21 => "IQ3_S", 22 => "IQ2_S", 23 => "IQ4_XS",
        24 => "I8", 25 => "I16", 26 => "I32", 27 => "I64", 28 => "F64", 29 => "IQ1_M",
        30 => "BF16", 34 => "TQ1_0", 35 => "TQ2_0",
        39 => "MXFP4",
        _ => "unknown",
    }
}

/// Every quant type in a checkpoint that this engine cannot read, with what it would cost to add.
///
/// **A UD ("unsloth dynamic") quant is a MIXTURE.** The file is called IQ1_S and its attention
/// tensors are IQ3_XXS, because the sensitive layers are kept wider on purpose. So supporting the
/// name on the box is never enough, and failing on the FIRST unsupported tensor costs a round trip
/// per type — the operator downloads 30 GiB, runs, learns one type, and repeats.
///
/// This reports all of them at once: what, how many tensors, and how many bytes, so the decision
/// about which to implement can be made from the sizes rather than from the order the loader
/// happened to walk.
pub struct MissingType { pub ggml_type: u32, pub tensors: usize, pub bytes: u64 }

pub fn unsupported_types(
    tensors: impl IntoIterator<Item = (u32, u64)>,
    supported: &[u32],
) -> Vec<MissingType> {
    let mut m: std::collections::BTreeMap<u32, (usize, u64)> = Default::default();
    for (t, bytes) in tensors {
        if supported.contains(&t) { continue }
        let e = m.entry(t).or_insert((0, 0));
        e.0 += 1;
        e.1 += bytes;
    }
    // Largest first: the decision about what to implement follows the bytes, not the walk order.
    let mut v: Vec<_> = m.into_iter()
        .map(|(t, (n, b))| MissingType { ggml_type: t, tensors: n, bytes: b }).collect();
    v.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    v
}

/// The message for an operator holding a 30 GiB file that will not load.
pub fn unsupported_report(missing: &[MissingType], total_bytes: u64) -> String {
    if missing.is_empty() { return "every tensor type in this file is supported".into() }
    let mut out = vec!["this checkpoint uses quant types the engine cannot read:".to_string()];
    for m in missing {
        let share = if total_bytes > 0 { m.bytes as f64 * 100.0 / total_bytes as f64 } else { 0.0 };
        out.push(format!("  {:<9} (type {:>2})  {:>4} tensors  {:>6.1} MiB  {:.0}% of the file",
            type_name(m.ggml_type), m.ggml_type, m.tensors,
            m.bytes as f64 / (1024.0 * 1024.0), share));
    }
    out.push("a 'UD' quant is a MIXTURE — the file is named for its smallest type and keeps sensitive tensors wider, so the name on the box does not say what is inside".into());
    out.push("listed together rather than one per run, because finding them one at a time costs a download each".into());
    out
        .join("\n")
}

pub fn type_nbytes(ggml_type: u32, n: usize) -> Option<usize> {
    match ggml_type {
        0 => Some(n * 4),
        1 => Some(n * 2),
        2 => if n % 32 == 0 { Some(n / 32 * 18) } else { None },
        3 => if n % 32 == 0 { Some(n / 32 * 20) } else { None }, // Q4_1: f16 d + f16 m + 16B nibbles
        6 => if n % 32 == 0 { Some(n / 32 * 22) } else { None }, // Q5_0: f16 d + 4B qh + 16B nibbles
        7 => if n % 32 == 0 { Some(n / 32 * 24) } else { None }, // Q5_1: f16 d + f16 m + 4B qh + 16B nibbles
        8 => if n % 32 == 0 { Some(n / 32 * 34) } else { None }, // Q8_0: f16 scale + 32×i8
        12 => if n % 256 == 0 { Some(n / 256 * 144) } else { None }, // Q4_K super-block
        // MXFP4: E8M0 scale byte + 16B of packed nibbles per 32 weights. The quant the current Qwen
        // MoE repackagings ship in, so a wrong size here mis-slices every expert tensor in the file.
        39 => if n % 32 == 0 { Some(n / 32 * crate::mxfp4::MXFP4_BLOCK) } else { None },
        // Q2_K: 84 bytes per 256 — 16 scale bytes, 64 quant bytes, d and dmin as f16.
        // Q3_K: 110 bytes per 256 — 32 high-bit bytes, 64 quant, 12 scale, d as f16.
        //
        // Sizes only. Neither has a dequantizer yet, so a checkpoint of these types is REPORTED
        // rather than read — which is the honest half: `type_nbytes` returning Some means the loader
        // can compute a footprint and say what the file costs, and `dequantize` returning None means
        // it refuses to invent values. Knowing the size without being able to read it is exactly what
        // a memory plan needs and a forward pass must not have.
        10 => if n % 256 == 0 { Some(n / 256 * 84) } else { None },   // Q2_K
        11 => if n % 256 == 0 { Some(n / 256 * 110) } else { None },  // Q3_K
        19 => if n % 256 == 0 { Some(n / 256 * 50) } else { None },  // IQ1_S
        // IQ1_M: 56 bytes per 256 weights, 1.75 bpw. THE SIZE IS ALL THAT IS SUPPORTED — see
        // dequantize, which refuses it. Knowing the size lets the loader report an honest "this
        // format needs a dequantizer" instead of "unknown tensor type", which is the difference
        // between a five-minute answer and an afternoon.
        29 => if n % 256 == 0 { Some(n / 256 * 56) } else { None },  // IQ1_M
        // Sizes for the rest of the IQ family. KNOWING A SIZE IS NOT SUPPORTING A TYPE — dequantize
        // still refuses these. It is what lets the report state honest byte shares, and a share is
        // what decides which one to implement first.
        13 => if n % 256 == 0 { Some(n / 256 * 176) } else { None }, // Q5_K
        16 => if n % 256 == 0 { Some(n / 256 * 66) } else { None },  // IQ2_XXS, 2.06 bpw
        17 => if n % 256 == 0 { Some(n / 256 * 74) } else { None },  // IQ2_XS,  2.31 bpw
        18 => if n % 256 == 0 { Some(n / 256 * 98) } else { None },  // IQ3_XXS, 3.06 bpw
        20 => if n % 32  == 0 { Some(n / 32  * 18) } else { None },  // IQ4_NL,  4.5 bpw
        21 => if n % 256 == 0 { Some(n / 256 * 110) } else { None }, // IQ3_S,   3.44 bpw
        22 => if n % 256 == 0 { Some(n / 256 * 82) } else { None },  // IQ2_S,   2.56 bpw
        23 => if n % 256 == 0 { Some(n / 256 * 136) } else { None }, // IQ4_XS,  4.25 bpw
        14 => if n % 256 == 0 { Some(n / 256 * 210) } else { None }, // Q6_K super-block
        _ => None,
    }
}

/// Dequantize a tensor's raw bytes to f32. Supports F32 (0), F16 (1), Q4_0 (2).
/// Q4_0: 18-byte blocks of 32 weights — f16 scale `d`, then 16 bytes whose low
/// nibble is weight j and high nibble weight j+16; w = (nibble - 8) * d.
/// Unpack the j-th 6-bit scale and min from a Q4_K/Q5_K 12-byte scale block
/// (ggml `get_scale_min_k4`). Returns (scale6, min6), each 0..63.
pub(crate) fn get_scale_min_k4(j: usize, q: &[u8]) -> (u8, u8) {
    if j < 4 {
        (q[j] & 63, q[j + 4] & 63)
    } else {
        ((q[j + 4] & 0x0F) | ((q[j - 4] >> 6) << 4),
         (q[j + 4] >> 4) | ((q[j] >> 6) << 4))
    }
}

/// Run a per-super-block transform across cores.
///
/// Every k-quant decodes one fixed-size block into a fixed-size run of floats with no cross-block state,
/// so splitting the range over threads produces bit-identical output — this is a scheduling change, not a
/// numerical one. It matters because these loops are the CPU cost of loading any model whose tensors do
/// not take the GPU conversion path (Q6_K tensors, and every expert on the MoE fallback), and they were
/// running on one core while the rest of the machine idled.
///
/// Small inputs stay sequential: thread setup costs more than it saves below a few dozen blocks.
fn par_blocks<F>(out: &mut [f32], data: &[u8], out_per: usize, in_per: usize, f: F)
where F: Fn(&mut [f32], &[u8]) + Sync {
    let nblk = out.len() / out_per.max(1);
    let threads = std::thread::available_parallelism().map(|t| t.get()).unwrap_or(1).min(nblk);
    if threads <= 1 || nblk < 64 {
        for (o, d) in out.chunks_mut(out_per).zip(data.chunks(in_per)) { f(o, d); }
        return;
    }
    let per = nblk.div_ceil(threads);
    std::thread::scope(|sc| {
        for (o, d) in out.chunks_mut(out_per * per).zip(data.chunks(in_per * per)) {
            let f = &f;
            sc.spawn(move || {
                for (oo, dd) in o.chunks_mut(out_per).zip(d.chunks(in_per)) { f(oo, dd); }
            });
        }
    });
}

/// One Q4_K super-block. Split out of the match arm so the FMA-enabled wrapper below can INLINE it:
/// `#[target_feature]` applies to the body it annotates, and a closure called through a generic is
/// a separate item that does not inherit the feature. The first attempt wrapped the loop in a
/// `with_fma(|| ...)` closure and emitted no `vfmadd` at all — the disassembly said so before the
/// benchmark did.
#[inline(always)]
fn q4k_block_core(y: &mut [f32], blk: &[u8]) {
    let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
    let dmin = f16_to_f32(u16::from_le_bytes([blk[2], blk[3]]));
    let scales = &blk[4..16];
    let qs = &blk[16..144];
    for j in 0..4 {
        let is = j * 2;
        let (sc1, m1) = get_scale_min_k4(is, scales);
        let (sc2, m2) = get_scale_min_k4(is + 1, scales);
        let (d1, mm1) = (d * sc1 as f32, dmin * m1 as f32);
        let (d2, mm2) = (d * sc2 as f32, dmin * m2 as f32);
        // MUL_ADD IS DELIBERATE — see the note in the Q4_K arm: the shader contracts this into an
        // FMA and cannot be stopped, so the CPU reference must round once too or it disagrees with
        // the GPU by an ULP that the re-quantizer turns into hundreds of thousands of differing
        // words. What is NOT deliberate is that on baseline x86-64 `mul_add` is a call into
        // compiler_builtins' `fmaf`. Hence the dispatch below.
        let q = &qs[j * 32..j * 32 + 32];
        let (lo, hi) = y[j * 64..j * 64 + 64].split_at_mut(32);
        for ((o1, o2), &qb) in lo.iter_mut().zip(hi.iter_mut()).zip(q.iter()) {
            *o1 = d1.mul_add((qb & 0xF) as f32, -mm1);
            *o2 = d2.mul_add((qb >> 4) as f32, -mm2);
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "fma")]
unsafe fn q4k_block_fma(y: &mut [f32], blk: &[u8]) { q4k_block_core(y, blk) }

/// Dispatch ONCE PER SUPER-BLOCK (256 weights), not per element.
fn q4k_block(y: &mut [f32], blk: &[u8]) {
    #[cfg(target_arch = "x86_64")]
    if crate::fmapath::has_fma() {
        // SAFETY: guarded by the runtime detection this line performs.
        return unsafe { q4k_block_fma(y, blk) };
    }
    q4k_block_core(y, blk)
}

/// F16 -> f32 over FIXED blocks of 64.
///
/// bench-dequant (optimized, one thread) put the SIMPLEST formats at the bottom: F16 336 and Q8_0
/// 375 M weights/s, under Q4_K's 710 with its nibbles and 6-bit scales. That ordering means
/// something is in the vectorizer's way, not that the work is hard. Here it was the
/// `chunks_exact(2).map(..).collect()` shape: per-element iterator state and no trip count the
/// compiler could plan around. Blocks of 64 with array-typed windows give it both.
/// 363 -> 489 M weights/s at baseline x86-64, 338 -> 633 with AVX2; bit-identical, remainder included.
fn dequant_f16(data: &[u8], n: usize) -> Vec<f32> {
    let mut out = vec![0f32; n];
    let mut oc = out.chunks_exact_mut(64);
    let mut ic = data[..n * 2].chunks_exact(128);
    for (o, c) in (&mut oc).zip(&mut ic) {
        let o: &mut [f32; 64] = o.try_into().unwrap();
        let c: &[u8; 128] = c.try_into().unwrap();
        for j in 0..64 { o[j] = f16_to_f32(u16::from_le_bytes([c[2 * j], c[2 * j + 1]])); }
    }
    for (o, c) in oc.into_remainder().iter_mut().zip(ic.remainder().chunks_exact(2)) {
        *o = f16_to_f32(u16::from_le_bytes([c[0], c[1]]));
    }
    out
}

/// Q8_0 -> f32: 34-byte blocks of 32 — f16 scale `d`, then 32 signed int8; w = q·d.
///
/// This arm did `out.push(..)` thirty-two times a block — a capacity check and a length store per
/// WEIGHT, which no vectorizer sees through — over `chunks(34)`, which also bounds-checks every
/// `blk[2 + j]`. Its neighbour Q4_0 already decoded into a local array and ran 1.4x faster for a
/// harder format. Fixed-size views into a pre-sized output: 373 -> 769 M weights/s, bit-identical.
fn dequant_q8_0(data: &[u8], n: usize) -> Vec<f32> {
    let mut out = vec![0f32; n];
    for (blk, o) in data.chunks_exact(34).zip(out.chunks_exact_mut(32)) {
        let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
        let q: &[u8; 32] = blk[2..34].try_into().unwrap();
        let o: &mut [f32; 32] = o.try_into().unwrap();
        for (w, &b) in o.iter_mut().zip(q.iter()) { *w = b as i8 as f32 * d; }
    }
    out
}

pub fn dequantize(ggml_type: u32, data: &[u8], n: usize) -> Result<Vec<f32>, String> {
    match ggml_type {
        // CHUNKS, NOT INDICES. Both of these ran `(0..n).map(|i| ... data[i * k ..] ...)`, which
        // is a bounds check and a `try_into().unwrap()` panic path per element — the loop cannot
        // vectorise with a panic in it. `chunks_exact` hands the compiler a known-width window and
        // no failure path. These two arms carry every F32 and F16 tensor in every model, including
        // every norm weight of a quantised one, so they are not a rare path.
        0 => {
            if data.len() < n * 4 { return Err("f32 data truncated".into()); }
            Ok(data[..n * 4].chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
        }
        1 => {
            if data.len() < n * 2 { return Err("f16 data truncated".into()); }
            Ok(dequant_f16(&data[..n * 2], n))
        }
        2 => {
            if n % 32 != 0 { return Err("q4_0 element count not a multiple of 32".into()); }
            let mut out = Vec::with_capacity(n);
            for blk in data[..n / 32 * 18].chunks(18) {
                let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
                let mut vals = [0f32; 32];
                for j in 0..16 {
                    let b = blk[2 + j];
                    vals[j] = ((b & 0xf) as i32 - 8) as f32 * d;
                    vals[j + 16] = (((b >> 4) & 0xf) as i32 - 8) as f32 * d;
                }
                out.extend_from_slice(&vals);
            }
            Ok(out)
        }
        8 => {
            // Q8_0: 34-byte blocks of 32 — f16 scale `d`, then 32 signed int8; w = d·q.
            if n % 32 != 0 { return Err("q8_0 element count not a multiple of 32".into()); }
            // The other arms all check this; Q8_0 indexed straight into a short slice and PANICKED.
            if data.len() < n / 32 * 34 { return Err("q8_0 data truncated".into()); }
            Ok(dequant_q8_0(&data[..n / 32 * 34], n))
        }
        6 => {
            // Q5_0: 22-byte blocks of 32 — f16 `d`, 4-byte `qh` (one 5th-bit per weight),
            // 16 nibble bytes. weight = d·((nib | 5th_bit<<4) − 16). Layout mirrors Q4_0
            // (low nibble = weight j, high nibble = weight j+16); the 5th bit for weight w
            // is bit w of qh.
            if n % 32 != 0 { return Err("q5_0 element count not a multiple of 32".into()); }
            let mut out = Vec::with_capacity(n);
            for blk in data[..n / 32 * 22].chunks(22) {
                let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
                let qh = u32::from_le_bytes([blk[2], blk[3], blk[4], blk[5]]);
                let qs = &blk[6..22];
                let mut vals = [0f32; 32];
                for j in 0..16 {
                    let xh0 = (((qh >> j) << 4) & 0x10) as u8;        // 5th bit of weight j
                    let xh1 = ((qh >> (j + 12)) & 0x10) as u8;        // 5th bit of weight j+16
                    vals[j] = (((qs[j] & 0x0F) | xh0) as i32 - 16) as f32 * d;
                    vals[j + 16] = (((qs[j] >> 4) | xh1) as i32 - 16) as f32 * d;
                }
                out.extend_from_slice(&vals);
            }
            Ok(out)
        }
        7 => {
            // Q5_1: 24-byte blocks of 32 — f16 `d`, f16 `m`, 4-byte `qh`, 16 nibble bytes.
            // weight = d·(nib | 5th_bit<<4) + m (unsigned 0..31, no recentre).
            if n % 32 != 0 { return Err("q5_1 element count not a multiple of 32".into()); }
            let mut out = Vec::with_capacity(n);
            for blk in data[..n / 32 * 24].chunks(24) {
                let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
                let m = f16_to_f32(u16::from_le_bytes([blk[2], blk[3]]));
                let qh = u32::from_le_bytes([blk[4], blk[5], blk[6], blk[7]]);
                let qs = &blk[8..24];
                let mut vals = [0f32; 32];
                for j in 0..16 {
                    let xh0 = (((qh >> j) << 4) & 0x10) as u8;
                    let xh1 = ((qh >> (j + 12)) & 0x10) as u8;
                    vals[j] = ((qs[j] & 0x0F) | xh0) as f32 * d + m;
                    vals[j + 16] = ((qs[j] >> 4) | xh1) as f32 * d + m;
                }
                out.extend_from_slice(&vals);
            }
            Ok(out)
        }
        3 => {
            // Q4_1: 20-byte blocks of 32 — f16 scale `d`, f16 min `m`, 16 nibble bytes
            // (low = weight j, high = weight j+16); w = d·nib + m (nibble unsigned 0..15).
            if n % 32 != 0 { return Err("q4_1 element count not a multiple of 32".into()); }
            let mut out = Vec::with_capacity(n);
            for blk in data[..n / 32 * 20].chunks(20) {
                let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
                let m = f16_to_f32(u16::from_le_bytes([blk[2], blk[3]]));
                let mut vals = [0f32; 32];
                for j in 0..16 {
                    let b = blk[4 + j];
                    vals[j] = (b & 0xf) as f32 * d + m;
                    vals[j + 16] = ((b >> 4) & 0xf) as f32 * d + m;
                }
                out.extend_from_slice(&vals);
            }
            Ok(out)
        }
        12 => {
            // Q4_K: 144-byte super-block of 256 = 8 sub-blocks of 32. f16 `d`, f16 `dmin`,
            // 12 bytes of packed 6-bit scales/mins, 128 nibble bytes. w = d·sc·nib − dmin·m.
            if n % 256 != 0 { return Err("q4_k element count not a multiple of 256".into()); }
            if data.len() < n / 256 * 144 { return Err("q4_k data truncated".into()); }
            let mut out = vec![0f32; n];
            par_blocks(&mut out, &data[..n / 256 * 144], 256, 144, q4k_block);
            Ok(out)
        }
        // IQ1_M IS NOT IQ1_S WITH A DIFFERENT BLOCK SIZE. Both are ~1.6-1.75 bpw codebook
        // formats and they lay their scales out differently: IQ1_S carries per-32 sub-scales in
        // the block header, IQ1_M packs per-8 scales alongside the codebook indices. A
        // dequantizer for one produces GARBAGE from the other — silently, because every length
        // check passes and the output is plausible floats.
        //
        // Refused by name, so a run says which format it needs rather than failing as "unknown
        // tensor type" or, worse, decoding it wrong. dot_iq1_s and MATVEC_IQ1_S are IQ1_S only.
        // IQ4_NL: the same table as IQ4_XS with none of the packing, so it costs a loop.
        20 => {
            if n % 32 != 0 { return Err(format!("IQ4_NL needs a multiple of 32 weights, got {n}")) }
            let mut out = vec![0.0f32; n];
            for (b, chunk) in (0..n / 32).zip(out.chunks_mut(32)) {
                let off = b * 18;
                if off + 18 > data.len() { return Err(format!("IQ4_NL block {b} runs past the data")) }
                dequantize_iq4_nl_block(&data[off..off + 18], chunk)?;
            }
            return Ok(out);
        }
        // Q5_K: a K-quant, no codebook. Present in the Scout UD file AND in GLM-4.7-Flash's
        // ffn_down, so it unblocks two checkpoints rather than one.
        //
        // Routed to kquant::dequant_q5_k. A SECOND implementation used to live in this file and it
        // is the one kernel-validate caught at max|gpu-cpu| = 18.9 on two machines: it read the
        // nibbles as interleaved pairs (Q4_0's layout — weight j from qs[j/2], low/high by parity)
        // where ggml's K-quants put the LOW nibbles of 32 bytes first and the HIGH nibbles of the
        // same 32 bytes second. Every non-uniform Q5_K tensor decoded to scrambled weights. The
        // uniform-fixture tests below couldn't see it — a layout bug is invisible when every
        // nibble holds the same value — which is why a non-uniform vector now lives in kquant.
        13 => return crate::kquant::dequant_q5_k(data, n),
        // IQ4_XS: 21% of a UD-IQ1_S checkpoint by bytes, and the only one of its seven types that
        // needs no codebook grid — sixteen values against IQ2/IQ3's 2048.
        23 => {
            if n % 256 != 0 { return Err(format!("IQ4_XS needs a multiple of 256 weights, got {n}")) }
            let mut out = vec![0.0f32; n];
            for (b, chunk) in (0..n / 256).zip(out.chunks_mut(256)) {
                let off = b * 136;
                if off + 136 > data.len() {
                    return Err(format!("IQ4_XS block {b} runs past the tensor data"));
                }
                dequantize_iq4_xs_block(&data[off..off + 136], chunk)?;
            }
            return Ok(out);
        }
        // The K-quants that were missing. Scalar formats like Q4_K and Q6_K, which is why they are
        // here and the codebook IQ types are not. Q2_K unlocks the 39.6 GB Q2_K Scout checkpoint.
        10 => return crate::kquant::dequant_q2_k(data, n),
        11 => return crate::kquant::dequant_q3_k(data, n),
        // IQ4_NL and IQ4_XS: the two IQ formats that are NOT codebook formats. Sixteen
        // non-linear values indexed by a nibble, so the whole "codebook" is one small table — which
        // is why these two are supported and IQ2_XXS is not.
        // IQ1_M now READS. It was refused by name because its per-8 scale layout differs from
        // IQ1_S's and the IQ1_S path would have decoded it to garbage silently — the refusal was
        // right, and iq1m.rs is what replaces it rather than relaxing it.
        // IQ2_XXS: the first CODEBOOK format here, decoded against a grid fetched from
        // ggml-common.h rather than recalled. 18% of a UD-IQ1_S Scout.
        21 => return crate::iq23::dequant_iq3_s(data, n),
        18 => return crate::iq23::dequant_iq3_xxs(data, n),
        39 => return crate::mxfp4::dequant_mxfp4(data, n),
        22 => return crate::iq23::dequant_iq2_s(data, n),
        16 => return crate::iq2xxs::dequant_iq2_xxs(data, n),
        17 => return crate::iq2xs::dequant_iq2_xs(data, n),
        29 => return crate::iq1m::dequant_iq1_m(data, n),
        19 => {
            // IQ1_S: 50-byte super-block of 256 weights = f16 `d`, 32 nibble-index bytes `qs`, and
            // 8 u16 `qh`. Each group of 8 weights takes an 11-BIT codebook index: 8 bits from qs plus 3
            // more from qh, indexing iq1s_grid's 2048 ternary entries. Per 32-weight sub-block, qh also
            // carries a 3-bit scale (dl = d·(2·s+1)) and a sign bit selecting ±IQ1S_DELTA.
            //
            // Ported from llama.cpp dequantize_row_iq1_s (ggml/src/ggml-quants.c). The codebook is
            // transcribed, not derived — see iq1s_grid.rs.
            if n % 256 != 0 { return Err("iq1_s element count not a multiple of 256".into()); }
            let nblk = n / 256;
            if data.len() < nblk * 50 { return Err("iq1_s data truncated".into()); }
            let mut out = vec![0f32; n];
            par_blocks(&mut out, &data[..nblk * 50], 256, 50, |y, blk| {
                let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
                let qs = &blk[2..34];
                let qh: Vec<u16> = (0..8).map(|i| u16::from_le_bytes([blk[34 + 2 * i], blk[35 + 2 * i]])).collect();
                for ib in 0..8 {
                    let h = qh[ib];
                    let dl = d * (2 * ((h >> 12) & 7) + 1) as f32;
                    let delta = if h & 0x8000 != 0 { -crate::iq1s_grid::IQ1S_DELTA } else { crate::iq1s_grid::IQ1S_DELTA };
                    for l in 0..4 {
                        let idx = qs[ib * 4 + l] as usize | ((((h >> (3 * l)) & 7) as usize) << 8);
                        let g = crate::iq1s_grid::entry(idx);
                        for j in 0..8 {
                            y[ib * 32 + l * 8 + j] = dl * (g[j] as f32 + delta);
                        }
                    }
                }
            });
            Ok(out)
        }
        14 => {
            // Q6_K: 210-byte super-block of 256. 128 low-nibble bytes `ql`, 64 high-2-bit
            // bytes `qh`, 16 int8 sub-block scales, f16 `d`. 6-bit quant = nib | (hi2<<4),
            // recentred by −32. w = d·sc·(q−32). Processed in two 128-wide halves.
            if n % 256 != 0 { return Err("q6_k element count not a multiple of 256".into()); }
            if data.len() < n / 256 * 210 { return Err("q6_k data truncated".into()); }
            let mut out = vec![0f32; n];
            par_blocks(&mut out, &data[..n / 256 * 210], 256, 210, |y, blk| {
                let d = f16_to_f32(u16::from_le_bytes([blk[208], blk[209]]));
                for half in 0..2usize {
                    let ql = &blk[half * 64..half * 64 + 64];
                    let qh = &blk[128 + half * 32..128 + half * 32 + 32];
                    let sc = &blk[192 + half * 8..192 + half * 8 + 8];
                    let sci = |k: usize| sc[k] as i8 as f32;
                    for l in 0..32 {
                        let is = l / 16;
                        let q1 = ((ql[l] & 0xF) | (((qh[l] >> 0) & 3) << 4)) as i32 - 32;
                        let q2 = ((ql[l + 32] & 0xF) | (((qh[l] >> 2) & 3) << 4)) as i32 - 32;
                        let q3 = ((ql[l] >> 4) | (((qh[l] >> 4) & 3) << 4)) as i32 - 32;
                        let q4 = ((ql[l + 32] >> 4) | (((qh[l] >> 6) & 3) << 4)) as i32 - 32;
                        let o = half * 128;
                        y[o + l] = d * sci(is) * q1 as f32;
                        y[o + l + 32] = d * sci(is + 2) * q2 as f32;
                        y[o + l + 64] = d * sci(is + 4) * q3 as f32;
                        y[o + l + 96] = d * sci(is + 6) * q4 as f32;
                    }
                }
            });
            Ok(out)
        }
        other => Err(format!("unsupported ggml type {other}")),
    }
}

#[cfg(test)]
mod kquant_tests {
    use super::*;

    /// The two rewritten arms against the loops they replaced, VERBATIM, bit for bit — including
    /// every F16 length class around the 64-wide block (0, 1, 63, 64, 65, 127, 128, 129 …) and
    /// every half that exists, so subnormals, zeros, infinities and NaNs all pass through.
    #[test]
    fn the_f16_and_q8_0_arms_are_bit_identical_to_the_loops_they_replaced() {
        let f16_before = |data: &[u8], n: usize| -> Vec<f32> {
            data[..n * 2].chunks_exact(2).map(|c| f16_to_f32(u16::from_le_bytes([c[0], c[1]]))).collect() };
        let q8_before = |data: &[u8], n: usize| -> Vec<f32> {
            let mut out = Vec::with_capacity(n);
            for blk in data[..n / 32 * 34].chunks(34) {
                let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
                for j in 0..32 { out.push(blk[2 + j] as i8 as f32 * d); }
            }
            out };
        let same = |a: &[f32], b: &[f32]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits());
        let all_halves: Vec<u8> = (0..=u16::MAX).flat_map(|h| h.to_le_bytes()).collect();
        for n in [0usize, 1, 2, 63, 64, 65, 127, 128, 129, 1000, 65536] {
            assert!(same(&dequantize(1, &all_halves, n).unwrap(), &f16_before(&all_halves, n)), "f16 n={n}");
        }
        let mut x = 0x1234_5678u32;
        let mut raw: Vec<u8> = (0..34 * 300).map(|_| { x = x.wrapping_mul(1664525).wrapping_add(1013904223); (x >> 24) as u8 }).collect();
        for (b, blk) in raw.chunks_mut(34).enumerate() {   // scales: normal, subnormal, zero, negative, inf, NaN
            let h: u16 = match b % 6 { 0 => 0x2C00 + b as u16, 1 => 0x0003, 2 => 0x0000, 3 => 0xAC00, 4 => 0x7C00, _ => 0x7E01 };
            blk[0] = h as u8; blk[1] = (h >> 8) as u8;
        }
        for blocks in [0usize, 1, 2, 7, 300] {
            assert!(same(&dequantize(8, &raw, blocks * 32).unwrap(), &q8_before(&raw, blocks * 32)), "q8_0 blocks={blocks}");
        }
        assert_eq!(dequantize(8, &raw[..100], 320).unwrap_err(), "q8_0 data truncated", "a short buffer is an error now, not an index panic");
        assert!(dequantize(8, &raw, 33).is_err());
    }

    // f32 → f16 bits (round-to-nearest-even), for building exact test blocks.
    fn f16_bits(x: f32) -> u16 {
        let b = x.to_bits();
        let sign = ((b >> 16) & 0x8000) as u16;
        let exp = ((b >> 23) & 0xFF) as i32 - 127 + 15;
        let mant = b & 0x7FFFFF;
        if exp <= 0 { return sign; }
        if exp >= 0x1F { return sign | 0x7C00; }
        sign | ((exp as u16) << 10) | ((mant >> 13) as u16)
    }

    #[test]
    fn q4_k_dequant_recovers_known_superblock() {
        // Build ONE Q4_K super-block (256 weights, 144 bytes) with dmin=0 so the min path drops out and the
        // output is simply d·scale·nibble. All 8 sub-block scales = 1 (low-4 path of get_scale_min_k4), all
        // nibbles = 5, d = 0.5. Expected: every weight = 0.5·1·5 = 2.5.
        let d: f32 = 0.5;
        let mut blk = vec![0u8; 144];
        blk[0..2].copy_from_slice(&f16_bits(d).to_le_bytes());   // d
        blk[2..4].copy_from_slice(&f16_bits(0.0).to_le_bytes()); // dmin = 0
        // scales[0..12]: get_scale_min_k4(j<4) reads q[j]&63 for the scale. Set the first 4 bytes' low-6
        // bits to 1 → sub-block scales 0..3 = 1. The 6-bit-min bytes (q[4..8]) stay 0 → mins = 0. For j>=4
        // the packed path also yields scale 1 if we set bytes appropriately; simplest: set all 12 to a value
        // whose low-6 bits are 1 AND whose high-2 bits are 0 → 0x01. Then j<4: q[j]&63=1. j>=4:
        // (q[j+4]&0x0F)|((q[j-4]>>6)<<4) = (1&0xF)|((1>>6)<<4) = 1. Good — all scales = 1, all mins:
        // j<4: q[j+4]&63 = 1 (!), so mins would be 1 — but dmin=0 zeroes the min term regardless. OK.
        for b in blk[4..16].iter_mut() { *b = 0x01; }
        // 128 nibble bytes: nibble 5 in both low and high → byte 0x55.
        for b in blk[16..144].iter_mut() { *b = 0x55; }
        let out = dequantize(12, &blk, 256).unwrap();
        assert_eq!(out.len(), 256);
        for (i, &w) in out.iter().enumerate() {
            assert!((w - 2.5).abs() < 1e-3, "weight {i} = {w}, expected 2.5 (d·sc·nib = 0.5·1·5)");
        }
    }

    #[test]
    fn q4_k_min_term_subtracts() {
        // Same block but dmin = 1.0 and the min 6-bit fields = 1 (from the 0x01 scale bytes, j<4 path).
        // Now w = d·sc·nib − dmin·m = 0.5·1·5 − 1.0·1 = 1.5 for the first two sub-blocks (j=0,1 use mins
        // from q[4],q[5] low-6 = 1). We only assert the very first weight to keep the min-index reasoning
        // simple (sub-block 0, min from get_scale_min_k4(0) = q[4]&63 = 1).
        let mut blk = vec![0u8; 144];
        blk[0..2].copy_from_slice(&f16_bits(0.5).to_le_bytes());
        blk[2..4].copy_from_slice(&f16_bits(1.0).to_le_bytes());
        for b in blk[4..16].iter_mut() { *b = 0x01; }
        for b in blk[16..144].iter_mut() { *b = 0x55; }
        let out = dequantize(12, &blk, 256).unwrap();
        assert!((out[0] - 1.5).abs() < 1e-3, "first weight = {} , expected 1.5 (2.5 − 1.0)", out[0]);
    }

    #[test]
    fn q4_k_rejects_bad_length() {
        assert!(dequantize(12, &[0u8; 144], 200).is_err());
    }
}

#[cfg(test)]
mod iq4nl_tests {
    use super::*;
    #[test]
    fn iq4_nl_dequant_matches_codebook() {
        // Build one 32-weight IQ4_NL block with scale d=1.0 (f16 0x3C00). Indices 0..15 in the low nibbles
        // and 0..15 again in the high nibbles → dequant must return the codebook values twice (d=1).
        const KV: [i8; 16] = [-127, -104, -83, -65, -49, -35, -22, -10, 1, 13, 25, 38, 53, 69, 89, 113];
        let mut blk = Vec::new();
        blk.extend_from_slice(&0x3C00u16.to_le_bytes()); // d = 1.0
        for j in 0..16u8 { blk.push(j | (j << 4)); }     // low nibble j, high nibble j
        assert_eq!(blk.len(), 18);
        // sizing
        assert_eq!(type_nbytes(20, 32), Some(18));
        let got = dequantize(20, &blk, 32).unwrap();
        assert_eq!(got.len(), 32);
        for j in 0..16 {
            assert!((got[j] - KV[j] as f32).abs() < 1e-6, "low nibble {j}: {} vs {}", got[j], KV[j]);
            assert!((got[j + 16] - KV[j] as f32).abs() < 1e-6, "high nibble {j}");
        }
    }

    #[test]
    fn iq4_nl_scale_applies() {
        // d = 2.0 (f16 0x4000): every value doubles.
        let mut blk = Vec::new();
        blk.extend_from_slice(&0x4000u16.to_le_bytes());
        for _ in 0..16u8 { blk.push(0x00); } // all index 0 → codebook[-127]
        let got = dequantize(20, &blk, 32).unwrap();
        assert!((got[0] - (-127.0 * 2.0)).abs() < 1e-4, "scaled: {}", got[0]);
    }

    #[test]
    fn iq4_nl_rejects_bad_length() {
        assert!(dequantize(20, &[0u8; 18], 20).is_err(), "n not a multiple of 32");
    }
}

#[cfg(test)]
mod dump_tests {
    use super::*;
    use std::collections::HashMap;
    #[test]
    fn dump_filters_and_formats() {
        let g = Gguf {
            version: 3,
            metadata: [("general.architecture".to_string(), Value::Str("qwen3moe".into()))].into_iter().collect::<HashMap<_,_>>(),
            tensors: vec![
                TensorInfo { name: "token_embd.weight".into(), dims: vec![2048, 151936], ggml_type: 12, offset: 0 },
                TensorInfo { name: "blk.0.ffn_gate_inp.weight".into(), dims: vec![2048, 128], ggml_type: 0, offset: 0 },
                TensorInfo { name: "blk.0.ffn_gate_exps.weight".into(), dims: vec![2048, 768, 128], ggml_type: 12, offset: 0 },
                TensorInfo { name: "blk.1.ffn_gate_inp.weight".into(), dims: vec![2048, 128], ggml_type: 0, offset: 0 },
            ],
            data_offset: 0,
        };
        let d = g.dump_tensors("blk.0.");
        assert!(d.contains("arch=qwen3moe"));
        assert!(d.contains("blk.0.ffn_gate_inp.weight") && d.contains("2048×128"));
        assert!(d.contains("blk.0.ffn_gate_exps.weight") && d.contains("2048×768×128") && d.contains("Q4_K"));
        assert!(!d.contains("blk.1."), "prefix filter excludes other layers");
        assert!(!d.contains("token_embd"), "prefix filter excludes top-level tensors");
        // full dump includes everything
        assert!(g.dump_tensors("").contains("token_embd.weight"));
    }
}

#[cfg(test)]
mod q5_tests {
    use super::dequantize;
    // f16 1.0 = 0x3C00, 0.5 = 0x3800 (little-endian byte pairs below)
    #[test]
    fn q5_0_block() {
        // d=1.0, qh=all-ones, qs=all 0xFF -> every weight (15|16)-16 = 15
        let mut blk = vec![0x00u8, 0x3C, 0xFF, 0xFF, 0xFF, 0xFF];
        blk.extend_from_slice(&[0xFFu8; 16]);
        let out = dequantize(6, &blk, 32).unwrap();
        assert_eq!(out.len(), 32);
        assert!(out.iter().all(|&x| (x - 15.0).abs() < 1e-3), "got {out:?}");
        // d=1.0, qh=0, qs=0 -> every weight (0)-16 = -16
        let mut b2 = vec![0x00u8, 0x3C, 0, 0, 0, 0];
        b2.extend_from_slice(&[0u8; 16]);
        let o2 = dequantize(6, &b2, 32).unwrap();
        assert!(o2.iter().all(|&x| (x + 16.0).abs() < 1e-3), "got {o2:?}");
    }
    #[test]
    fn q5_1_block() {
        // d=1.0, m=0.5, qh=0, qs=0 -> weight = 0*1 + 0.5 = 0.5
        let mut blk = vec![0x00u8, 0x3C, 0x00, 0x38, 0, 0, 0, 0];
        blk.extend_from_slice(&[0u8; 16]);
        let out = dequantize(7, &blk, 32).unwrap();
        assert!(out.iter().all(|&x| (x - 0.5).abs() < 1e-3), "got {out:?}");
    }
}

#[cfg(test)]
mod f16_tests {
    use super::f16_to_f32;
    #[test]
    fn subnormals_and_normals() {
        // exact reference: f16 bits -> f32 via the IEEE definition
        let reference = |h: u16| -> f32 {
            let s = if h >> 15 == 1 { -1.0 } else { 1.0 };
            let e = (h >> 10) & 0x1f; let m = (h & 0x3ff) as f32;
            if e == 0 { s * m * 2f32.powi(-24) }              // subnormal: m·2^-24
            else if e == 0x1f { if m == 0.0 { s * f32::INFINITY } else { f32::NAN } }
            else { s * (1.0 + m / 1024.0) * 2f32.powi(e as i32 - 15) }
        };
        for h in [0x0001u16, 0x0200, 0x03ff, 0x0010, 0x8200, 0x3c00, 0x4000, 0x1234, 0x7000] {
            let (got, want) = (f16_to_f32(h), reference(h));
            assert!((got - want).abs() <= want.abs() * 1e-6 + 1e-30, "h={h:#06x} got={got:e} want={want:e}");
        }
        // smallest subnormal and a mid one, by absolute value
        assert!((f16_to_f32(0x0001) - 2f32.powi(-24)).abs() < 1e-30);
        assert!((f16_to_f32(0x0200) - 2f32.powi(-15)).abs() < 1e-12);
    }
}

#[cfg(test)]
mod kquant_subnormal_tests {
    use super::dequantize;
    // These guard the 13-round bug: K-quant block scales `d` are f16, and small-weight
    // models (e.g. Llama-3.2-1B) push `d` into the subnormal range. A regressed subnormal
    // f16 decode silently shrinks every weight ~4×. Each test builds a block whose `d` is the
    // subnormal half 0x0200 (= 2^-15) and asserts the dequantized magnitude is exact.
    const D_BITS: [u8; 2] = [0x00, 0x02]; // f16 0x0200 = 2^-15

    #[test]
    fn q6_k_subnormal_scale() {
        // d=2^-15, all int8 sub-scales=1, all 6-bit q=63 (q-32=31) -> every value = 31·2^-15.
        let mut blk = vec![0xFFu8; 128];            // ql: both nibbles 0xF
        blk.extend_from_slice(&[0xFFu8; 64]);       // qh: every 2-bit field = 3
        blk.extend_from_slice(&[0x01u8; 16]);       // scales: int8 = 1
        blk.extend_from_slice(&D_BITS);             // d (f16) at bytes 208..210
        assert_eq!(blk.len(), 210);
        let out = dequantize(14, &blk, 256).unwrap();
        let want = 31.0 * 2f32.powi(-15);
        assert!(out.iter().all(|&x| (x - want).abs() <= want * 1e-4),
            "q6_k subnormal: got {} want {want} (a 4x-small value means the f16 subnormal bug is back)", out[0]);
    }

    #[test]
    fn q4_k_subnormal_scale() {
        // d=2^-15, dmin=0, sub-scale[0]=1, first low nibble=15 -> y[0] = 15·2^-15.
        let mut blk = Vec::new();
        blk.extend_from_slice(&D_BITS);             // d = 2^-15
        blk.extend_from_slice(&[0x00, 0x00]);       // dmin = 0
        let mut scales = [0u8; 12]; scales[0] = 1;  // get_scale_min_k4(0) -> sc=1, m=0
        blk.extend_from_slice(&scales);
        let mut qs = [0u8; 128]; qs[0] = 0x0F;      // first element low nibble = 15
        blk.extend_from_slice(&qs);
        assert_eq!(blk.len(), 144);
        let out = dequantize(12, &blk, 256).unwrap();
        let want = 15.0 * 2f32.powi(-15);
        assert!((out[0] - want).abs() <= want * 1e-4,
            "q4_k subnormal: got {} want {want} (4x-small means the f16 subnormal bug is back)", out[0]);
    }
}

#[cfg(test)]
mod arch_tests {
    use super::*;
    use std::collections::HashMap;

    fn g(pairs: Vec<(&str, Value)>) -> Gguf {
        let mut m = HashMap::new();
        for (k, v) in pairs { m.insert(k.to_string(), v); }
        Gguf { version: 3, metadata: m, tensors: vec![], data_offset: 0 }
    }
    fn emb(prefix: &str) -> (&'static str, Value) { (Box::leak(format!("{prefix}.embedding_length").into_boxed_str()), Value::U32(4096)) }

    #[test]
    fn a_forged_array_count_is_an_error_not_an_abort() {
        // header + one metadata entry: key "k", type 9 (array), element type 4 (u32), count u64::MAX
        let mut b = Vec::new();
        b.extend_from_slice(b"GGUF"); b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes()); b.extend_from_slice(&1u64.to_le_bytes());
        b.extend_from_slice(&1u64.to_le_bytes()); b.push(b'k');
        b.extend_from_slice(&9u32.to_le_bytes()); b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&u64::MAX.to_le_bytes());
        b.extend_from_slice(&7u32.to_le_bytes()); // one real element, then the file ends
        assert_eq!(parse(&b).err().as_deref(), Some("unexpected EOF"));
    }

    #[test]
    fn get_u32_arr_reads_any_integer_width_and_refuses_a_negative() {
        let mut g = Gguf { version: 3, metadata: HashMap::new(), tensors: Vec::new(), data_offset: 0 };
        g.metadata.insert("i32".into(), Value::Arr(vec![Value::I32(11), Value::I32(11), Value::I32(10), Value::I32(0)]));
        g.metadata.insert("u32".into(), Value::Arr(vec![Value::U32(16), Value::U32(24)]));
        g.metadata.insert("neg".into(), Value::Arr(vec![Value::I32(11), Value::I32(-1)]));
        g.metadata.insert("str".into(), Value::Str("x".into()));
        assert_eq!(g.get_u32_arr("i32"), Some(vec![11, 11, 10, 0]), "int32 is what llama.cpp's converter writes");
        assert_eq!(g.get_u32_arr("u32"), Some(vec![16, 24]));
        assert_eq!(g.get_u32_arr("neg"), None, "a partly-readable section list is not a section list");
        assert_eq!(g.get_u32_arr("str"), None);
        assert_eq!(g.get_u32_arr("absent"), None);
    }

    #[test]
    fn get_u32_reads_any_integer_width() {
        // GGUF may store a dimension as int32 or uint64, not only uint32. Reading only U32 was the cause
        // of the Ornith/Qwen3.5 "missing embedding_length" (the key was present, as int32/uint64).
        let m = g(vec![
            ("qwen3_5.embedding_length", Value::I32(3584)), // int32-encoded dim
            ("qwen3_5.block_count", Value::U64(36)),        // uint64-encoded count
            ("qwen3_5.attention.head_count", Value::U32(28)),
        ]);
        assert_eq!(m.get_u32("qwen3_5.embedding_length"), Some(3584));
        assert_eq!(m.get_u32("qwen3_5.block_count"), Some(36));
        assert_eq!(m.get_u32("qwen3_5.attention.head_count"), Some(28));
        // arch() resolves qwen3_5 once the int32-encoded key is readable
        let m2 = g(vec![
            ("general.architecture", Value::Str("qwen3_5".into())),
            ("qwen3_5.embedding_length", Value::I32(3584)),
        ]);
        assert_eq!(m2.arch(), "qwen3_5");
        // a negative value is not a valid dimension → still None (caller reports)
        assert_eq!(g(vec![("x.embedding_length", Value::I32(-1))]).get_u32("x.embedding_length"), None);
    }

    #[test]
    fn arch_recovers_a_prefix_that_has_no_embedding_length_at_all() {
        // C12-10b. The anchor used to be `embedding_length` ALONE, so a file whose hparams sit
        // under one prefix but which omits or renames that single key fell through to the declared
        // architecture — and then every `<arch>.*` lookup missed at once. The observed shape was a
        // loader saying `.block_count is absent, but it IS present as ["muse-glimmer"]`: the prefix
        // was right there in the file and nothing looked for it outside one key.
        let m = g(vec![
            ("general.architecture", Value::Str("llama".into())),
            ("muse-glimmer.block_count", Value::U32(48)),
            ("muse-glimmer.attention.head_count", Value::U32(32)),
        ]);
        assert_eq!(m.arch(), "muse-glimmer",
            "the prefix must be recoverable from any known hyperparameter, not only embedding_length");
    }

    #[test]
    fn the_prefix_with_the_most_hyperparameters_wins() {
        // A stray key under some other prefix must not outrank the real block. Asserted as a
        // comparison so it survives either side gaining a key.
        let m = g(vec![
            ("stray.context_length", Value::U32(2048)),
            ("realarch.block_count", Value::U32(48)),
            ("realarch.attention.head_count", Value::U32(32)),
            ("realarch.feed_forward_length", Value::U32(11008)),
        ]);
        assert_eq!(m.arch(), "realarch");
    }

    #[test]
    fn the_tokenizer_block_can_never_win() {
        // `tokenizer.ggml.*` is the largest metadata block in a real GGUF by a wide margin, so any
        // unweighted "most keys wins" count resolves the architecture to the tokenizer. The
        // allowlist makes that impossible by construction rather than by an exclusion someone has
        // to remember; this pins it, since the failure would be silent and total.
        let mut pairs = vec![
            ("qwen3.block_count", Value::U32(28)),
        ];
        for k in ["tokenizer.ggml.model", "tokenizer.ggml.pre", "tokenizer.ggml.bos_token_id",
                  "tokenizer.ggml.eos_token_id", "tokenizer.ggml.padding_token_id",
                  "tokenizer.ggml.add_bos_token", "tokenizer.ggml.context_length"] {
            pairs.push((k, Value::U32(1)));
        }
        assert_eq!(g(pairs).arch(), "qwen3");
    }

    #[test]
    fn general_and_the_towers_are_still_excluded_under_the_wider_net() {
        // Widening the anchor widens what the exclusions have to hold back. `general.*` now shares
        // the shape of a hparam block (general.file_type and friends), and a VLM's vision tower has
        // a full one of its own.
        let m = g(vec![
            ("clip.block_count", Value::U32(24)),
            ("clip.embedding_length", Value::U32(1024)),
            ("clip.attention.head_count", Value::U32(16)),
            ("qwen2.block_count", Value::U32(28)),
        ]);
        assert_eq!(m.arch(), "qwen2", "a vision tower must never become the language architecture");
        // And with ONLY a tower present, the tower still does not win — better to fall back to the
        // declared name and let the caller refuse than to load a vision block as a language model.
        let only_tower = g(vec![
            ("general.architecture", Value::Str("llava".into())),
            ("vision.block_count", Value::U32(24)),
        ]);
        assert_eq!(only_tower.arch(), "llava");
    }

    #[test]
    fn a_tie_is_broken_deterministically() {
        // Two prefixes with equally many hyperparameters: the answer must not depend on HashMap
        // iteration order, or the same file loads differently between runs.
        let pairs = vec![
            ("bbb.block_count", Value::U32(1)),
            ("aaa.block_count", Value::U32(1)),
        ];
        let first = g(pairs.clone()).arch();
        for _ in 0..20 { assert_eq!(g(pairs.clone()).arch(), first, "arch() must be deterministic"); }
        assert_eq!(first, "aaa", "ties resolve to the lexicographic minimum, as before");
    }

    #[test]
    fn arch_trusts_general_architecture_when_consistent() {
        let m = g(vec![("general.architecture", Value::Str("qwen2".into())), emb("qwen2")]);
        assert_eq!(m.arch(), "qwen2");
    }

    #[test]
    fn arch_recovers_prefix_when_general_architecture_is_missing() {
        // the reported failure: no general.architecture, but the hyperparameters are under "qwen2.*".
        // Old code defaulted to "llama" → "missing embedding_length"; now we recover "qwen2" and load.
        let m = g(vec![emb("qwen2"), ("qwen2.block_count", Value::U32(28))]);
        assert_eq!(m.arch(), "qwen2", "recover the real arch from the metadata key prefix");
    }

    #[test]
    fn arch_recovers_prefix_when_general_architecture_lies() {
        // general.architecture says llama but the keys are gemma2 → resolve to gemma2 so the caller's
        // guard refuses it CORRECTLY (safe: never runs gemma2 weights through the llama path) instead of
        // emitting a confusing "missing embedding_length".
        let m = g(vec![("general.architecture", Value::Str("llama".into())), emb("gemma2")]);
        assert_eq!(m.arch(), "gemma2");
    }

    #[test]
    fn arch_skips_vision_tower_for_a_vlm() {
        // a VLM carries clip.embedding_length beside the language tower's; resolve to the language arch
        let m = g(vec![emb("clip"), emb("llama")]);
        assert_eq!(m.arch(), "llama");
        // and if general.architecture is present + consistent, it wins outright
        let m2 = g(vec![("general.architecture", Value::Str("llama".into())), emb("clip"), emb("llama")]);
        assert_eq!(m2.arch(), "llama");
    }

    #[test]
    fn arch_defaults_to_llama_when_nothing_is_declared() {
        assert_eq!(g(vec![]).arch(), "llama");
    }
}

#[cfg(test)]
mod iq1m_tests {
    use super::*;

    #[test]
    fn IQ1_M_has_a_known_SIZE_so_the_loader_can_report_honestly() {
        // Knowing the size lets a run say "this format needs a dequantizer" instead of "unknown
        // tensor type" — the difference between a five-minute answer and an afternoon.
        assert_eq!(type_nbytes(29, 256), Some(56));
        assert_eq!(type_nbytes(29, 512), Some(112));
        assert_eq!(type_nbytes(19, 256), Some(50), "IQ1_S is 50, IQ1_M is 56 — different formats");
    }

    #[test]
    fn IQ1_M_is_decoded_by_its_OWN_path_and_not_IQ1_S_s() {
        // This asserted a REFUSAL when written, and the refusal was right: IQ1_M's scale layout
        // differs from IQ1_S's, so the IQ1_S path would have decoded it to plausible garbage.
        //
        // iq1m.rs REPLACES the refusal rather than relaxing it, and the assertion flips to the
        // positive so a regression fails here rather than passing on a premise that has changed.
        let v = dequantize(29, &vec![0u8; 56], 256).expect("IQ1_M reads now");
        assert_eq!(v.len(), 256);
        // The two formats must not share a decoder: their block sizes differ and feeding one to the
        // other is the silent corruption the original refusal existed to prevent.
        assert_ne!(type_nbytes(29, 256), type_nbytes(19, 256));
        assert!(dequantize(19, &vec![0u8; 56], 256).is_ok(), "IQ1_S still reads its own 50-byte form");
    }

    #[test]
    fn a_non_block_aligned_IQ1_M_tensor_has_no_size() {
        assert_eq!(type_nbytes(29, 300), None);
    }
}

#[cfg(test)]
mod mixed_quant_tests {
    use super::*;

    /// The types the engine can actually read today.
    const SUPPORTED: &[u32] = &[0, 1, 8, 12, 14, 19];

    #[test]
    fn the_UD_IQ1_S_failure_is_reported_as_a_FORMAT_not_a_number() {
        // The real log said "type 18 unsupported for blk.0.attn_q.weight", which sends someone to a
        // table. IQ3_XXS is the answer.
        assert_eq!(type_name(18), "IQ3_XXS");
        assert_eq!(type_name(19), "IQ1_S");
        assert_eq!(type_name(29), "IQ1_M");
    }

    #[test]
    fn EVERY_unsupported_type_is_reported_in_one_pass() {
        // Failing on the first costs a round trip per type: download 30 GiB, run, learn one type,
        // repeat. A UD quant has several.
        let tensors = vec![(18, 1000), (18, 2000), (22, 500), (12, 9999), (19, 8888)];
        let m = unsupported_types(tensors, SUPPORTED);
        assert_eq!(m.len(), 2, "IQ3_XXS and IQ2_S, not just the first one hit");
    }

    #[test]
    fn they_are_ordered_by_BYTES_not_by_walk_order() {
        // The decision about what to implement follows the size, not the order the loader happened
        // to reach them.
        let m = unsupported_types(vec![(22, 10), (18, 5000)], SUPPORTED);
        assert_eq!(m[0].ggml_type, 18);
    }

    #[test]
    fn the_report_explains_that_a_UD_QUANT_IS_A_MIXTURE() {
        // The thing that made this surprising: a file named IQ1_S whose attention tensors are
        // IQ3_XXS, because the sensitive layers are kept wider on purpose.
        let r = unsupported_report(&unsupported_types(vec![(18, 1024 * 1024)], SUPPORTED), 2 * 1024 * 1024);
        assert!(r.contains("IQ3_XXS"));
        assert!(r.contains("the name on the box does not say what is inside"));
        assert!(r.contains("50% of the file"));
    }

    #[test]
    fn a_fully_supported_file_says_so() {
        assert!(unsupported_types(vec![(12, 1), (19, 2)], SUPPORTED).is_empty());
        assert!(unsupported_report(&[], 100).contains("every tensor type"));
    }

    #[test]
    fn the_report_says_WHY_it_lists_them_together() {
        // So the next person does not "helpfully" restore fail-fast behaviour.
        let r = unsupported_report(&unsupported_types(vec![(18, 1)], SUPPORTED), 1);
        assert!(r.contains("costs a download each"));
    }
}

#[cfg(test)]
mod iq4xs_tests {
    use super::*;

    /// A block with a known scale and every nibble zero.
    fn block(d: f32, sub_scales: [u8; 8], nibbles: u8) -> Vec<u8> {
        let mut b = vec![0u8; 136];
        // d as f16
        // f16 by hand rather than pulling a crate in for a test fixture: 1.0 is 0x3C00 and the
        // exponent arithmetic is only needed for the values these tests use.
        let bits: u16 = if d == 1.0 { 0x3C00 } else if d == 2.0 { 0x4000 } else { 0x3C00 };
        b[0..2].copy_from_slice(&bits.to_le_bytes());
        let mut hi = 0u16;
        for (i, s) in sub_scales.iter().enumerate() {
            let biased = (*s as i32 + 32) as u8;      // undo the -32 the decoder applies
            if i % 2 == 0 { b[4 + i / 2] |= biased & 0x0F } else { b[4 + i / 2] |= (biased & 0x0F) << 4 }
            hi |= (((biased >> 4) & 3) as u16) << (2 * i);
        }
        b[2..4].copy_from_slice(&hi.to_le_bytes());
        for x in b[8..136].iter_mut() { *x = nibbles | (nibbles << 4); }
        b
    }

    #[test]
    fn the_LOOKUP_is_sixteen_values_not_a_grid() {
        // Why IQ4_XS is tractable where IQ2_XXS and IQ3_S are not: those need the 2048-entry
        // codebooks from ggml-quants.c and this needs one line.
        assert_eq!(KVALUES_IQ4NL.len(), 16);
        // Non-linear on purpose — spaced to match weight distribution, which is why it beats a
        // linear 4-bit quantizer at the same width.
        let gaps: Vec<i32> = KVALUES_IQ4NL.windows(2).map(|w| (w[1] - w[0]) as i32).collect();
        assert!(gaps.iter().min() != gaps.iter().max(), "a linear table would defeat the format");
    }

    #[test]
    fn a_block_of_ZERO_INDICES_gives_the_table_s_first_value_scaled() {
        // Index 0 is -127, not zero — a format where the low index means zero would waste a level,
        // and assuming it does is the commonest way to misread this table.
        let mut out = vec![0.0f32; 256];
        dequantize_iq4_xs_block(&block(1.0, [1; 8], 0), &mut out).unwrap();
        assert_eq!(out[0], -127.0, "index 0 maps to -127");
        assert!(out.iter().all(|v| *v == -127.0));
    }

    #[test]
    fn each_SUB_BLOCK_takes_its_own_scale() {
        // The split-across-two-fields layout is why a naive reader gets the first sub-block right
        // and the rest wrong: low nibbles are contiguous, high bits are not.
        let mut out = vec![0.0f32; 256];
        dequantize_iq4_xs_block(&block(1.0, [1, 2, 3, 4, 5, 6, 7, 8], 0), &mut out).unwrap();
        for sub in 0..8 {
            let expect = (sub as f32 + 1.0) * -127.0;
            assert_eq!(out[sub * 32], expect, "sub-block {sub}");
        }
    }

    #[test]
    fn a_HIGH_scale_exercises_the_two_bits_in_scales_h() {
        // Scales above 15 need the high field, which is where a decoder that reads only the low
        // nibble silently produces a quarter of the right magnitude.
        let mut out = vec![0.0f32; 256];
        dequantize_iq4_xs_block(&block(1.0, [31; 8], 0), &mut out).unwrap();
        assert_eq!(out[0], 31.0 * -127.0);
    }

    #[test]
    fn the_LOW_and_HIGH_nibbles_split_the_sub_block_in_half() {
        // First 16 weights take low nibbles, next 16 take high nibbles of the SAME bytes.
        let mut b = block(1.0, [1; 8], 0);
        for x in b[8..136].iter_mut() { *x = 0x0F; }   // low nibble 15, high nibble 0
        let mut out = vec![0.0f32; 256];
        dequantize_iq4_xs_block(&b, &mut out).unwrap();
        assert_eq!(out[0], 113.0, "low nibble 15 -> last table value");
        assert_eq!(out[16], -127.0, "high nibble 0 -> first");
    }

    #[test]
    fn a_SHORT_block_is_refused_rather_than_read_past() {
        assert!(dequantize_iq4_xs_block(&vec![0u8; 100], &mut vec![0.0; 256]).is_err());
    }

    #[test]
    fn dequantize_ROUTES_type_23_now() {
        let data = block(1.0, [1; 8], 0);
        let v = dequantize(23, &data, 256).expect("IQ4_XS should dequantize");
        assert_eq!(v.len(), 256);
        // And a misaligned length is refused rather than truncated.
        assert!(dequantize(23, &data, 300).is_err());
    }
}

#[cfg(test)]
mod iq4_wiring_tests {
    use super::*;

    #[test]
    fn dequantize_ACCEPTS_iq4_nl_and_iq4_xs() {
        // The decoders are useless if dequantize still refuses them, and a module that compiles but
        // is never reached is the shape of MOE_MATVEC_IQ1_S sitting undispatched.
        let nl = vec![0u8; 18];
        let xs = vec![0u8; 136];
        assert!(dequantize(20, &nl, 32).is_ok(), "IQ4_NL");
        assert!(dequantize(23, &xs, 256).is_ok(), "IQ4_XS");
    }

    #[test]
    fn the_still_UNSUPPORTED_iq_types_are_still_refused() {
        // Adding two must not accidentally open the others: a codebook format read through a
        // non-linear decoder produces plausible floats and a model that generates confident nonsense.
        // 29 (IQ1_M) left the list when iq1m.rs landed. The remaining five are the CODEBOOK
        // formats that each need a 2048-entry grid transcribed — IQ1_M was tractable precisely
        // because it reuses the grid IQ1_S already carries.
        // 16 (IQ2_XXS) left when the grids were fetched. The four remaining need iq2xs_grid,
        // iq3xxs_grid and iq3s_grid wired to their decoders — the tables are already here.
        // Only IQ2_XS (17) and IQ3_S (21) remain. Both grids are already in iqgrids.rs, so what
        // is missing is decoder logic rather than data — which is a much better position than the
        // one this list started in.
        // ONLY IQ2_XS (17) REMAINS, and it is not in a UD-IQ1_S Scout at all. Every type in that
        // checkpoint now reads.
        // AND NOW NONE REMAIN: iq2xs.rs decodes 17 (checked against gguf-py). What this test
        // guards has inverted — every i-quant ggml defines reads here, and a regression would be
        // one of them silently refusing again. Positive assertion, every type, zero block.
        for (t, bytes) in [(16u32, 66usize), (17, 74), (18, 98), (19, 50), (20, 18 * 8), (21, 110), (22, 82), (23, 136), (29, 56)] {
            assert!(dequantize(t, &vec![0u8; bytes], 256).is_ok(), "type {t} ({}) must read", type_name(t));
            assert_eq!(type_nbytes(t, 256), Some(bytes), "type_nbytes({t}) disagrees with its decoder's block");
        }
        // a type ggml does not define stays refused
        assert!(dequantize(4, &vec![0u8; 512], 256).is_err());
    }

    #[test]
    fn the_sizes_agree_with_the_decoders() {
        // type_nbytes was added before the decoders so the loader could report honestly. If they
        // disagree the report was wrong about the bytes it counted.
        assert_eq!(type_nbytes(20, 32), Some(crate::iq4::IQ4_NL_BLOCK));
        assert_eq!(type_nbytes(23, 256), Some(crate::iq4::IQ4_XS_BLOCK));
        assert_eq!(type_nbytes(17, 256), Some(crate::iq2xs::IQ2_XS_BLOCK));
    }
}

#[cfg(test)]
mod iq4nl_q5k_tests {
    use super::*;

    #[test]
    fn IQ4_NL_splits_its_nibbles_the_same_way_IQ4_XS_does() {
        // First 16 take low nibbles, next 16 take high nibbles of the SAME bytes. Consecutive pairs
        // is the natural guess and is wrong, which is why it gets a test rather than a comment.
        let mut b = vec![0u8; 18];
        b[0..2].copy_from_slice(&0x3C00u16.to_le_bytes());   // d = 1.0
        for x in b[2..18].iter_mut() { *x = 0x0F; }           // low nibble 15, high nibble 0
        let mut out = vec![0.0f32; 32];
        dequantize_iq4_nl_block(&b, &mut out).unwrap();
        assert_eq!(out[0], 113.0, "low nibble 15 -> last table value");
        assert_eq!(out[16], -127.0, "high nibble 0 -> first");
    }

    #[test]
    fn IQ4_NL_shares_the_table_rather_than_copying_it() {
        // One table, two formats. A second copy would drift, and drift in a lookup produces plausible
        // weights rather than an error.
        let mut b = vec![0u8; 18];
        b[0..2].copy_from_slice(&0x3C00u16.to_le_bytes());
        let mut out = vec![0.0f32; 32];
        dequantize_iq4_nl_block(&b, &mut out).unwrap();
        assert_eq!(out[0], KVALUES_IQ4NL[0] as f32);
    }

    /// Q5_K with every scale byte set to one value.
    ///
    /// Deliberately NOT trying to encode a chosen scale and min separately. My first fixture did,
    /// got the sub-block 4..7 packing backwards, and produced a test failure whose message accused
    /// the DECODER — which is the exact hazard of a fixture more complicated than what it tests.
    ///
    /// Filling all twelve bytes gives every sub-block the same non-zero scale, which is all the
    /// property below needs.
    fn q5k_uniform(scale_byte: u8, high_bit: bool) -> Vec<u8> {
        let mut b = vec![0u8; 176];
        b[0..2].copy_from_slice(&0x3C00u16.to_le_bytes());    // d = 1.0
        b[2..4].copy_from_slice(&0x3C00u16.to_le_bytes());    // dmin = 1.0
        for x in b[4..16].iter_mut() { *x = scale_byte; }
        if high_bit { for x in b[16..48].iter_mut() { *x = 0xFF; } }
        b
    }

    // These four used to call a private block decoder that lived in THIS file. That decoder read
    // the nibbles interleaved (Q4_0's layout) and every one of them passed anyway, because a
    // uniform fixture puts the same value in both nibbles of every byte — a layout bug permutes
    // weights and a permutation of identical values is invisible. They now go through
    // `dequantize(13, ...)`, the path the loader ships; the layout itself is pinned by the
    // non-uniform vector in kquant::tests.

    #[test]
    fn EVERY_sub_block_decodes_including_the_last() {
        // The fifth bit is indexed across the SUPER-BLOCK, not the sub-block. A per-sub-block
        // index gives the right answer for the first 32 weights and leaves the rest untouched,
        // so the last weight is the one that catches it.
        let out = dequantize(13, &q5k_uniform(0x11, true), 256).unwrap();
        assert!(out.iter().all(|v| v.is_finite()), "every weight decoded");
        assert_ne!(out[255], 0.0, "the last weight is where a local index would have left a hole");
        assert_ne!(out[128], 0.0, "and the middle, past the sub-block-4 packing boundary");
    }

    #[test]
    fn the_HIGH_BIT_changes_every_weight() {
        // q = lo | (hi << 4), so setting the plane must move all 256 — not merely the first block.
        let with = dequantize(13, &q5k_uniform(0x11, true), 256).unwrap();
        let without = dequantize(13, &q5k_uniform(0x11, false), 256).unwrap();
        assert!(with.iter().zip(&without).all(|(a, b)| a != b), "the fifth bit reached every weight");
    }

    #[test]
    fn Q5_K_subtracts_a_MINIMUM() {
        // dl*q - ml. A decoder that forgets the minimum produces weights offset by a constant —
        // coherent text, subtly wrong, which is this family's specialty.
        let out = dequantize(13, &q5k_uniform(0x11, false), 256).unwrap();
        assert!(out[0] <= 0.0, "q=0 leaves -min, which cannot be positive: {}", out[0]);
    }

    #[test]
    fn both_formats_REFUSE_a_short_block() {
        assert!(dequantize_iq4_nl_block(&vec![0u8; 10], &mut vec![0.0; 32]).is_err());
        assert!(dequantize(13, &vec![0u8; 100], 256).is_err());
    }

    #[test]
    fn dequantize_routes_BOTH_new_types() {
        let mut nl = vec![0u8; 18];
        nl[0..2].copy_from_slice(&0x3C00u16.to_le_bytes());
        assert_eq!(dequantize(20, &nl, 32).unwrap().len(), 32);
        assert_eq!(dequantize(13, &q5k_uniform(0x11, false), 256).unwrap().len(), 256);
        // Misaligned lengths refused rather than truncated.
        assert!(dequantize(20, &nl, 40).is_err());
        assert!(dequantize(13, &q5k_uniform(0x11, false), 300).is_err());
    }
}

/// Elements per block by ggml type CLASS — the only geometry the parallel splitter
/// needs. F32/F16 are 1; the q*_0/q*_1/q8 row formats are 32; every K-quant and IQ
/// type is a 256-element superblock. Bytes-per-block is DERIVED from the exact data
/// length rather than tabulated per type, so this cannot drift from `dequantize`'s
/// own per-type code above.
pub fn block_elems(ggml_type: u32) -> usize {
    match ggml_type {
        0 | 1 => 1,          // f32, f16
        2..=9 => 32,         // q4_0/q4_1/q5_0/q5_1/q8_0/q8_1 family
        _ => 256,            // K-quants and the IQ family
    }
}

/// Block-parallel `dequantize`: cut the tensor on block boundaries, run the SERIAL
/// per-type code on each chunk across the pool, write disjoint output ranges.
/// Same answer as `dequantize` by construction — each chunk goes through the same
/// scalar code — measured 5x-per-byte slower than the Q4_K GPU path on the UD-IQ1_S
/// Scout load (0.26 vs 1.25 GB/s), which is a thread-count problem until the IQ
/// family gets its GPU dequant (see reference-docs/DIFFUSION_REFERENCE.md).
pub fn dequantize_par(pool: &crate::cpupool::Pool, ggml_type: u32, data: &[u8], n: usize) -> Result<Vec<f32>, String> {
    let be = block_elems(ggml_type);
    let nblocks = n / be.max(1);
    if pool.threads() <= 1 || nblocks < 64 || n % be != 0 || data.len() % nblocks.max(1) != 0 {
        return dequantize(ggml_type, data, n);
    }
    let bpb = data.len() / nblocks; // exact bytes per block, derived not tabulated
    let mut out = vec![0f32; n];
    let out_addr = out.as_mut_ptr() as usize;
    let err: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let chunks = (pool.threads() * 4).min(nblocks);
    let per = nblocks.div_ceil(chunks);
    pool.run(|tid, nt| {
        let mut c = tid;
        while c < chunks {
            let (b0, b1) = (c * per, ((c + 1) * per).min(nblocks));
            if b0 < b1 {
                match dequantize(ggml_type, &data[b0 * bpb..b1 * bpb], (b1 - b0) * be) {
                    Ok(v) => unsafe {
                        // Disjoint by construction: chunk c owns elements [b0*be, b1*be).
                        std::ptr::copy_nonoverlapping(v.as_ptr(), (out_addr as *mut f32).add(b0 * be), v.len());
                    },
                    Err(e) => { *err.lock().unwrap() = Some(e); }
                }
            }
            c += nt;
        }
    });
    match err.into_inner().unwrap() { Some(e) => Err(e), None => Ok(out) }
}

/// Per-layer census of GATED DELTANET / SSM blocks: `(ssm_layers, total_layers)`.
///
/// Qwen3.5 and Qwen3.6 35B-A3B carry `ssm_a`, `ssm_conv1d`, `ssm_dt`, `ssm_alpha`, `ssm_beta` and
/// `ssm_out` alongside the usual `attn_qkv`, because a Gated DeltaNet layer reuses the attention
/// projections for its q/k/v and adds the recurrent state on top. Only about one layer in four is a
/// real softmax-attention block, and those have no `ssm_*` at all.
///
/// This matters because the family name predicts the wrong thing. c4-804 registered `qwen35moe`
/// against the `qwen3moe` shape on the reasoning that "that is what the family has been", and the
/// operator's tensor dump falsified it on the first load: blk.0 is a DeltaNet layer. Counting the
/// blocks turns "unknown architecture" into a number that says how much of the model has nowhere to
/// run — which is the difference between a missing registry line and a missing forward pass.
/// How many `blk.N.` blocks the TENSORS describe, regardless of what metadata claims.
///
/// `{arch}.block_count` requires knowing the hparam prefix, and the prefix is not reliably the
/// architecture name: this loader already carries an override for files whose hparams sit under a
/// different one ("hparam prefix is 'dream.' — using the file's prefix"). That override runs AFTER
/// the recurrent census, so the census looked up `qwen35moe.block_count`, got None, counted zero
/// blocks and stayed silent on a model with thirty recurrent layers — which is how the operator got
/// a row-count error from deep in the attention loader instead of "this model is hybrid".
///
/// The tensor names cannot have that problem. They are the thing being loaded.
pub fn block_count_from_tensors(g: &Gguf) -> usize {
    let mut max = None;
    for t in &g.tensors {
        let Some(rest) = t.name.strip_prefix("blk.") else { continue };
        let Some((num, _)) = rest.split_once('.') else { continue };
        if let Ok(i) = num.parse::<usize>() {
            max = Some(max.map_or(i, |m: usize| m.max(i)));
        }
    }
    max.map_or(0, |m| m + 1)
}

pub fn ssm_layer_census(g: &Gguf, n_layers: usize) -> (usize, usize) {
    // A caller that could not resolve the block count gets the tensors' answer rather than zero.
    // Returning (0, 0) reads as "no recurrent layers", which is the one wrong thing to say.
    let n_layers = if n_layers == 0 { block_count_from_tensors(g) } else { n_layers };
    let mut ssm = 0;
    for i in 0..n_layers {
        let p = format!("blk.{i}.");
        if g.tensors.iter().any(|t| t.name.starts_with(&p) && t.name[p.len()..].starts_with("ssm_")) {
            ssm += 1;
        }
    }
    (ssm, n_layers)
}

#[cfg(test)]
mod ssm_census {
    use super::*;

    fn ti(name: &str) -> TensorInfo {
        TensorInfo { name: name.to_string(), dims: vec![1], ggml_type: 0, offset: 0 }
    }

    /// The 3:1 layout the operator's dump implies — three DeltaNet layers then one attention layer.
    #[test]
    fn it_counts_only_the_layers_that_carry_ssm_tensors() {
        let mut g = Gguf { version: 3, metadata: HashMap::new(), tensors: Vec::new(), data_offset: 0 };
        for i in 0..8 {
            g.tensors.push(ti(&format!("blk.{i}.attn_qkv.weight")));
            if i % 4 != 3 {                       // 0,1,2 / 4,5,6 are DeltaNet
                g.tensors.push(ti(&format!("blk.{i}.ssm_conv1d.weight")));
                g.tensors.push(ti(&format!("blk.{i}.ssm_a")));
            }
        }
        assert_eq!(ssm_layer_census(&g, 8), (6, 8), "six of eight layers are DeltaNet");
    }

    /// A plain transformer must census as zero — otherwise every Qwen3 load would claim to be hybrid.
    #[test]
    fn a_conventional_model_censuses_as_zero() {
        let mut g = Gguf { version: 3, metadata: HashMap::new(), tensors: Vec::new(), data_offset: 0 };
        for i in 0..4 { g.tensors.push(ti(&format!("blk.{i}.attn_qkv.weight"))); }
        assert_eq!(ssm_layer_census(&g, 4), (0, 4));
    }

    /// Prefix matching must not confuse blk.1 with blk.10 — an off-by-a-block census would misreport
    /// the ratio on any model with ten or more layers.
    #[test]
    fn block_numbers_do_not_prefix_match_each_other() {
        let mut g = Gguf { version: 3, metadata: HashMap::new(), tensors: Vec::new(), data_offset: 0 };
        g.tensors.push(ti("blk.10.ssm_a"));
        assert_eq!(ssm_layer_census(&g, 2), (0, 2), "blk.10 must not count as blk.1");
        assert_eq!(ssm_layer_census(&g, 11).0, 1);
    }
}

#[cfg(test)]
mod block_count_from_names {
    use super::*;
    fn ti(name: &str) -> TensorInfo {
        TensorInfo { name: name.to_string(), dims: vec![1], ggml_type: 0, offset: 0 }
    }
    fn g_with(names: &[&str]) -> Gguf {
        Gguf { version: 3, metadata: HashMap::new(),
               tensors: names.iter().map(|n| ti(n)).collect(), data_offset: 0 }
    }

    /// The count comes from the highest block index, not the number of blk.* tensors — a layer with
    /// eleven tensors is still one layer.
    #[test]
    fn it_counts_blocks_not_tensors() {
        let g = g_with(&["blk.0.attn_qkv.weight", "blk.0.ssm_a", "blk.0.ffn_up.weight",
                         "blk.1.attn_qkv.weight", "token_embd.weight", "output_norm.weight"]);
        assert_eq!(block_count_from_tensors(&g), 2);
    }

    /// Double digits must not sort as strings — "blk.9" beating "blk.10" would under-count every
    /// model with ten or more layers, which is all of them.
    #[test]
    fn double_digit_blocks_are_counted_numerically() {
        let g = g_with(&["blk.9.attn_qkv.weight", "blk.10.attn_qkv.weight", "blk.39.ssm_a"]);
        assert_eq!(block_count_from_tensors(&g), 40);
    }

    /// A census given zero must consult the tensors, because zero is exactly what a failed metadata
    /// lookup produces and (0, 0) reads as "not recurrent".
    #[test]
    fn a_zero_block_count_falls_back_to_the_tensors() {
        let mut names: Vec<String> = Vec::new();
        for i in 0..40 {
            names.push(format!("blk.{i}.attn_qkv.weight"));
            if i < 30 { names.push(format!("blk.{i}.ssm_conv1d.weight")); }
        }
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let g = g_with(&refs);
        assert_eq!(ssm_layer_census(&g, 0), (30, 40),
            "the operator's Qwen3.6 shape: 30 of 40 recurrent, found without any metadata");
        assert_eq!(ssm_layer_census(&g, 40), (30, 40), "and unchanged when the count IS known");
    }

    #[test]
    fn malformed_names_do_not_panic_or_inflate() {
        let g = g_with(&["blk.notanumber.weight", "blk.", "blkx.3.weight", "blk.2.attn_qkv.weight"]);
        assert_eq!(block_count_from_tensors(&g), 3);
    }
}

/// Read an hparam that CANNOT legitimately be zero, distinguishing absent from present-and-zero.
///
/// `get_u32(...).unwrap_or(0)` collapses those two cases, and the collapse is only harmless when
/// zero is a real answer. For `expert_count` it is — zero means dense. For `block_count` and
/// `embedding_length` it is not: a model with no layers or no width cannot exist, so zero always
/// means the key was missing, which in practice means the hparam PREFIX is wrong.
///
/// That has cost real time twice. The converter in c4-791 read `n_routed_experts` from a config
/// that spells it `num_experts`, defaulted to zero, and would have described a 256-expert MoE as
/// dense. The recurrent census in c4-815 read `{prefix}.block_count`, got nothing, censused zero
/// blocks and reported a hybrid model as conventional. Both loaded. Neither complained.
pub fn required_u32(g: &Gguf, prefix: &str, key: &str) -> Result<u32, String> {
    let full = format!("{prefix}.{key}");
    match g.get_u32(&full) {
        Some(v) if v > 0 => Ok(v),
        Some(_) => Err(format!("{full} is present but ZERO, which is not a possible value — the file \
is malformed or was written by a converter that did not fill it in")),
        None => {
            // Name a prefix that WOULD have worked, because the usual cause is that hparams sit
            // under a different one than general.architecture (this loader already carries an
            // override for that: "hparam prefix is 'dream.' — using the file's prefix").
            let alt: Vec<String> = g.metadata.keys()
                .filter(|k| k.ends_with(&format!(".{key}")))
                .map(|k| k[..k.len() - key.len() - 1].to_string())
                .collect();
            let hint = if alt.is_empty() {
                "and no key of that name exists under ANY prefix".to_string()
            } else {
                format!("but it IS present as {:?} — the hparam prefix is not the architecture name", alt)
            };
            Err(format!("{full} is absent, {hint}"))
        }
    }
}

#[cfg(test)]
mod required_hparams {
    use super::*;

    fn g_with(pairs: &[(&str, u32)]) -> Gguf {
        let mut m = HashMap::new();
        for (k, v) in pairs { m.insert(k.to_string(), Value::U32(*v)); }
        Gguf { version: 3, metadata: m, tensors: Vec::new(), data_offset: 0 }
    }

    #[test]
    fn a_present_value_is_returned() {
        let g = g_with(&[("qwen3.block_count", 40)]);
        assert_eq!(required_u32(&g, "qwen3", "block_count"), Ok(40));
    }

    /// The case that cost two investigations: present under a DIFFERENT prefix. The error must name
    /// the prefix that would have worked, because "absent" alone sends the reader to the file.
    #[test]
    fn a_prefix_mismatch_names_the_prefix_that_works() {
        let g = g_with(&[("qwen3moe.block_count", 40)]);
        let e = required_u32(&g, "qwen35moe", "block_count").unwrap_err();
        assert!(e.contains("qwen3moe"), "must name the working prefix: {e}");
        assert!(e.contains("not the architecture name"), "and say what that means: {e}");
    }

    /// Absent everywhere is a different problem from absent-here, and must read differently.
    #[test]
    fn absent_everywhere_says_so() {
        let g = g_with(&[("qwen3.embedding_length", 2048)]);
        let e = required_u32(&g, "qwen3", "block_count").unwrap_err();
        assert!(e.contains("under ANY prefix"), "{e}");
    }

    /// Present-and-zero is malformed, not missing — collapsing the two is the bug this exists for.
    #[test]
    fn present_but_zero_is_its_own_error() {
        let g = g_with(&[("qwen3.block_count", 0)]);
        let e = required_u32(&g, "qwen3", "block_count").unwrap_err();
        assert!(e.contains("present but ZERO"), "{e}");
        assert!(!e.contains("absent"), "a zero is not an absence: {e}");
    }
}

/// Which blocks carry an MTP / next-token-prediction head, and how many layers the metadata claims.
///
/// MTP is a draft head trained alongside the model and shipped IN the same file, under
/// `blk.{N}.nextn.*` (eh_proj, enorm, hnorm, shared_head_head, ...) with `{arch}.nextn_predict_layers`
/// declaring how many. llama.cpp consumes it as `--spec-type draft-mtp`.
///
/// This matters here more than for most engines. The phase-balance verdict has been saying the same
/// thing for a dozen bundles — "decode has nothing in flight to cover memory latency; what is
/// missing is something producing more than one candidate token, which is speculative decoding" —
/// and the operator's decode sits at 3% of roofline. Speculative decoding has always been the named
/// lever and always carried the same cost: finding, loading and aligning a separate draft model.
///
/// An MTP head removes that cost. The draft is already in the file, already shares the tokenizer,
/// already trained on the same data. Qwen3.8-27B-MTP reports 77.9% acceptance and ~2x throughput
/// from exactly this.
///
/// Detection only. Nothing here runs the head — but a file that carries one and is loaded as if it
/// did not is leaving that factor on the table silently, which is the kind of thing this loader
/// should say out loud.
pub fn mtp_census(g: &Gguf, prefix: &str) -> (usize, Option<u32>) {
    let n_layers = block_count_from_tensors(g);
    let mut with_head = 0;
    for i in 0..n_layers {
        let p = format!("blk.{i}.nextn.");
        if g.tensors.iter().any(|t| t.name.starts_with(&p)) { with_head += 1; }
    }
    (with_head, g.get_u32(&format!("{prefix}.nextn_predict_layers")))
}

#[cfg(test)]
mod mtp_detection {
    use super::*;

    fn ti(name: &str) -> TensorInfo {
        TensorInfo { name: name.to_string(), dims: vec![1], ggml_type: 0, offset: 0 }
    }
    fn g_with(names: &[String], kv: &[(&str, u32)]) -> Gguf {
        let mut m = HashMap::new();
        for (k, v) in kv { m.insert(k.to_string(), Value::U32(*v)); }
        Gguf { version: 3, metadata: m, tensors: names.iter().map(|n| ti(n)).collect(), data_offset: 0 }
    }

    #[test]
    fn a_model_with_no_draft_head_reports_none() {
        let names: Vec<String> = (0..4).map(|i| format!("blk.{i}.attn_qkv.weight")).collect();
        assert_eq!(mtp_census(&g_with(&names, &[]), "qwen38"), (0, None));
    }

    /// The real shape: a head on the last block or two, with the count in metadata.
    #[test]
    fn the_draft_head_is_found_and_counted() {
        let mut names: Vec<String> = (0..64).map(|i| format!("blk.{i}.attn_qkv.weight")).collect();
        for t in ["eh_proj", "enorm", "hnorm", "shared_head_head"] {
            names.push(format!("blk.63.nextn.{t}"));
        }
        let (n, declared) = mtp_census(&g_with(&names, &[("qwen38.nextn_predict_layers", 1)]), "qwen38");
        assert_eq!(n, 1, "one block carries the head");
        assert_eq!(declared, Some(1));
    }

    /// Metadata and tensors can disagree, and the tensors are what would actually run. Reporting
    /// both is the same discipline the recurrent census learned in c4-815, where trusting a
    /// prefixed metadata key over the tensor names hid a hybrid model.
    #[test]
    fn tensors_and_metadata_are_reported_separately() {
        let mut names: Vec<String> = (0..8).map(|i| format!("blk.{i}.attn_qkv.weight")).collect();
        names.push("blk.7.nextn.eh_proj".to_string());
        // Metadata claims two, only one is present.
        let (n, declared) = mtp_census(&g_with(&names, &[("x.nextn_predict_layers", 2)]), "x");
        assert_eq!((n, declared), (1, Some(2)),
            "the caller must be able to see the disagreement rather than being handed one number");
    }

    /// A wrong prefix loses the metadata but never the tensors — the failure mode that cost three
    /// theories in c4-815.
    #[test]
    fn the_tensor_count_survives_a_prefix_mismatch() {
        let mut names: Vec<String> = (0..4).map(|i| format!("blk.{i}.attn_qkv.weight")).collect();
        names.push("blk.3.nextn.eh_proj".to_string());
        let (n, declared) = mtp_census(&g_with(&names, &[("right.nextn_predict_layers", 1)]), "wrong");
        assert_eq!(n, 1, "tensors do not depend on a prefix");
        assert_eq!(declared, None);
    }
}

/// The f16 decoder is verified EXHAUSTIVELY, not sampled.
///
/// It has form: the subnormal-f16 bug in FIELD_NOTES decoded subnormal halves 4x small, which
/// shrank K-quant block scales for small-weight models and produced word salad from Llama-3.2-1B
/// while every SmolLM2 on the same engine stayed coherent. It needed a large-d, small-weight real
/// model to surface, and no sampled test would have caught it — but all 65536 inputs takes
/// milliseconds, so there is no reason to sample.
#[cfg(test)]
mod f16_exhaustive {
    use super::f16_to_f32;

    /// The reference is the previous implementation, kept here verbatim. Comparing against it over
    /// the whole domain is what makes replacing a numerically load-bearing function safe.
    fn reference(h: u16) -> f32 {
        let sign = ((h >> 15) & 1) as u32;
        let exp = ((h >> 10) & 0x1f) as u32;
        let mant = (h & 0x3ff) as u32;
        let bits = if exp == 0 {
            if mant == 0 { sign << 31 } else {
                let mut e = -14i32; let mut m = mant;
                while (m & 0x400) == 0 { m <<= 1; e -= 1; }
                (sign << 31) | (((e + 127) as u32) << 23) | ((m & 0x3ff) << 13)
            }
        } else if exp == 0x1f {
            (sign << 31) | (0xff << 23) | (mant << 13)
        } else {
            (sign << 31) | (((exp as i32 + 127 - 15) as u32) << 23) | (mant << 13)
        };
        f32::from_bits(bits)
    }

    #[test]
    fn all_65536_halves_decode_bit_identically() {
        let mut bad = Vec::new();
        for h in 0..=u16::MAX {
            let (a, b) = (reference(h), f16_to_f32(h));
            // NaN payloads are compared by bits too — a decoder that turns a quiet NaN into a
            // signalling one is a decoder that changed something.
            if a.to_bits() != b.to_bits() && !(a.is_nan() && b.is_nan()) {
                bad.push((h, a, b));
                if bad.len() > 4 { break; }
            }
        }
        assert!(bad.is_empty(), "f16 decode diverged: {bad:?}");
    }

    /// The specific class that caused the word-salad bug, called out so a regression names itself.
    #[test]
    fn subnormal_halves_keep_their_magnitude() {
        for h in 1u16..0x0400 {                       // exponent 0, mantissa != 0
            let v = f16_to_f32(h);
            let expect = h as f32 * 2f32.powi(-24);   // subnormal half = mant * 2^-24
            assert_eq!(v.to_bits(), expect.to_bits(), "subnormal {h:#06x} decoded as {v:e}");
        }
        // and the boundary either side of the subnormal/normal cutoff
        assert_eq!(f16_to_f32(0x0400), 2f32.powi(-14));
        assert_eq!(f16_to_f32(0x03FF).to_bits(), (1023.0 * 2f32.powi(-24)).to_bits());
    }

    #[test]
    fn the_special_values_survive() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert!(f16_to_f32(0x8000) == 0.0 && f16_to_f32(0x8000).is_sign_negative(), "negative zero");
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xBC00), -1.0);
        assert_eq!(f16_to_f32(0x7C00), f32::INFINITY);
        assert_eq!(f16_to_f32(0xFC00), f32::NEG_INFINITY);
        assert!(f16_to_f32(0x7E00).is_nan());
        assert_eq!(f16_to_f32(0x7BFF), 65504.0, "largest finite half");
    }
}

