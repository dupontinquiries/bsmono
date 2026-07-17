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
                let mut v = Vec::with_capacity(n);
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
    pub fn arch(&self) -> String {
        if let Some(a) = self.get_str("general.architecture") {
            if self.metadata.contains_key(&format!("{a}.embedding_length")) { return a.to_string(); }
        }
        let recovered = self.metadata.keys()
            .filter_map(|k| k.strip_suffix(".embedding_length"))
            .filter(|p| !p.is_empty() && !matches!(*p, "general" | "clip" | "vision" | "mmproj"))
            .min();
        recovered.map(str::to_string)
            .or_else(|| self.get_str("general.architecture").map(str::to_string))
            .unwrap_or_else(|| "llama".to_string())
    }
}

/// IEEE half → f32.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1f) as u32;
    let mant = (h & 0x3ff) as u32;
    let bits = if exp == 0 {
        if mant == 0 { sign << 31 } else {
            // subnormal half: value = mant · 2^-24. Normalize so bit 10 becomes the implicit
            // leading 1; the f32 unbiased exponent is then (-14 - shifts). (The old code seeded
            // e=-1 and biased by 127-15, landing 2 too low — every subnormal came out 4× small,
            // which silently shrank K-quant block scales `d` for small-weight models.)
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

/// Bytes occupied by `n` elements of the given ggml type (F32/F16/Q4_0).
/// Human-readable ggml quant type name (for diagnostics / the tensor dump).
pub fn ggml_type_name(t: u32) -> &'static str {
    match t {
        0 => "F32", 1 => "F16", 2 => "Q4_0", 3 => "Q4_1", 6 => "Q5_0", 7 => "Q5_1",
        8 => "Q8_0", 9 => "Q8_1", 10 => "Q2_K", 11 => "Q3_K", 12 => "Q4_K", 13 => "Q5_K",
        14 => "Q6_K", 15 => "Q8_K", 16 => "IQ2_XXS", 20 => "IQ4_NL", 21 => "IQ3_S",
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
        13 => if n % 256 == 0 { Some(n / 256 * 176) } else { None }, // Q5_K super-block
        14 => if n % 256 == 0 { Some(n / 256 * 210) } else { None }, // Q6_K super-block
        _ => None,
    }
}

/// Dequantize a tensor's raw bytes to f32. Supports F32 (0), F16 (1), Q4_0 (2).
/// Q4_0: 18-byte blocks of 32 weights — f16 scale `d`, then 16 bytes whose low
/// nibble is weight j and high nibble weight j+16; w = (nibble - 8) * d.
/// Unpack the j-th 6-bit scale and min from a Q4_K/Q5_K 12-byte scale block
/// (ggml `get_scale_min_k4`). Returns (scale6, min6), each 0..63.
fn get_scale_min_k4(j: usize, q: &[u8]) -> (u8, u8) {
    if j < 4 {
        (q[j] & 63, q[j + 4] & 63)
    } else {
        ((q[j + 4] & 0x0F) | ((q[j - 4] >> 6) << 4),
         (q[j + 4] >> 4) | ((q[j] >> 6) << 4))
    }
}

pub fn dequantize(ggml_type: u32, data: &[u8], n: usize) -> Result<Vec<f32>, String> {
    match ggml_type {
        0 => Ok((0..n).map(|i| f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap())).collect()),
        1 => Ok((0..n).map(|i| f16_to_f32(u16::from_le_bytes([data[i * 2], data[i * 2 + 1]]))).collect()),
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
            let mut out = Vec::with_capacity(n);
            for blk in data[..n / 32 * 34].chunks(34) {
                let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
                for j in 0..32 { out.push(blk[2 + j] as i8 as f32 * d); }
            }
            Ok(out)
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
            let mut out = Vec::with_capacity(n);
            for blk in data[..n / 256 * 144].chunks(144) {
                let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
                let dmin = f16_to_f32(u16::from_le_bytes([blk[2], blk[3]]));
                let scales = &blk[4..16];
                let qs = &blk[16..144];
                let mut y = [0f32; 256];
                let (mut is, mut qoff, mut yoff) = (0usize, 0usize, 0usize);
                for _ in 0..4 {
                    let (sc1, m1) = get_scale_min_k4(is, scales);
                    let (sc2, m2) = get_scale_min_k4(is + 1, scales);
                    let (d1, mm1) = (d * sc1 as f32, dmin * m1 as f32);
                    let (d2, mm2) = (d * sc2 as f32, dmin * m2 as f32);
                    for l in 0..32 {
                        y[yoff + l] = d1 * (qs[qoff + l] & 0xF) as f32 - mm1;
                        y[yoff + 32 + l] = d2 * (qs[qoff + l] >> 4) as f32 - mm2;
                    }
                    qoff += 32; yoff += 64; is += 2;
                }
                out.extend_from_slice(&y);
            }
            Ok(out)
        }
        13 => {
            // Q5_K: 176-byte super-block of 256. f16 `d`, f16 `dmin`, 12 scale bytes,
            // 32 high-bit bytes, 128 nibble bytes. The 5th bit comes from qh (a moving
            // mask u1/u2). w = d·sc·(nib + 16·hi) − dmin·m.
            if n % 256 != 0 { return Err("q5_k element count not a multiple of 256".into()); }
            let mut out = Vec::with_capacity(n);
            for blk in data[..n / 256 * 176].chunks(176) {
                let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
                let dmin = f16_to_f32(u16::from_le_bytes([blk[2], blk[3]]));
                let scales = &blk[4..16];
                let qh = &blk[16..48];
                let qs = &blk[48..176];
                let mut y = [0f32; 256];
                let (mut is, mut qoff, mut yoff) = (0usize, 0usize, 0usize);
                let (mut u1, mut u2) = (1u8, 2u8);
                for _ in 0..4 {
                    let (sc1, m1) = get_scale_min_k4(is, scales);
                    let (sc2, m2) = get_scale_min_k4(is + 1, scales);
                    let (d1, mm1) = (d * sc1 as f32, dmin * m1 as f32);
                    let (d2, mm2) = (d * sc2 as f32, dmin * m2 as f32);
                    for l in 0..32 {
                        let hi1 = if qh[l] & u1 != 0 { 16 } else { 0 };
                        let hi2 = if qh[l] & u2 != 0 { 16 } else { 0 };
                        y[yoff + l] = d1 * ((qs[qoff + l] & 0xF) as i32 + hi1) as f32 - mm1;
                        y[yoff + 32 + l] = d2 * ((qs[qoff + l] >> 4) as i32 + hi2) as f32 - mm2;
                    }
                    qoff += 32; yoff += 64; is += 2; u1 <<= 2; u2 <<= 2;
                }
                out.extend_from_slice(&y);
            }
            Ok(out)
        }
        14 => {
            // Q6_K: 210-byte super-block of 256. 128 low-nibble bytes `ql`, 64 high-2-bit
            // bytes `qh`, 16 int8 sub-block scales, f16 `d`. 6-bit quant = nib | (hi2<<4),
            // recentred by −32. w = d·sc·(q−32). Processed in two 128-wide halves.
            if n % 256 != 0 { return Err("q6_k element count not a multiple of 256".into()); }
            let mut out = Vec::with_capacity(n);
            for blk in data[..n / 256 * 210].chunks(210) {
                let d = f16_to_f32(u16::from_le_bytes([blk[208], blk[209]]));
                let mut y = [0f32; 256];
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
                out.extend_from_slice(&y);
            }
            Ok(out)
        }
        other => Err(format!("unsupported ggml type {other}")),
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
