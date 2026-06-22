//! Byte-level BPE tokenizer: text ↔ token ids via a vocab and ranked merges,
//! readable from GGUF metadata (`tokenizer.ggml.tokens` / `.merges`).
//!
//! Two modes:
//!  - raw (`new`): initial symbols are the input bytes directly (used by the
//!    unit fixtures); merge core is shared.
//!  - byte-level (`new_byte_level`, and `from_gguf` when the model is a BPE type):
//!    the GPT-2 byte→unicode remap is applied so input bytes map to the printable
//!    unicode tokens real vocabs store, with a best-effort pre-tokenizer splitting
//!    text into word/number/space/other runs (a single leading space attaches to
//!    the following run, à la GPT-2's ` ?\p{L}+`). Decode reverses the remap.
//!    (SentencePiece/unigram vocabs are a different scheme and not handled here.)
use crate::gguf::{Gguf, Value};
use std::collections::HashMap;

pub struct Tokenizer {
    tokens: Vec<Vec<u8>>,
    vocab: HashMap<Vec<u8>, u32>,
    /// (left_id, right_id) -> (rank, merged_id)
    merges: HashMap<(u32, u32), (u32, u32)>,
    byte_level: bool,
    byte_encoder: Vec<char>,        // byte -> printable unicode char (GPT-2 table)
    byte_decoder: HashMap<char, u8>, // inverse
    /// Control/added tokens (e.g. "<|eot_id|>") matched verbatim in input and
    /// emitted as a single id, longest-first; their text is NOT byte-remapped.
    specials: Vec<(String, u32)>,
    special_ids: std::collections::HashSet<u32>,
}

/// GPT-2 `bytes_to_unicode`: map every byte to a printable codepoint so the BPE
/// operates over a reversible, whitespace-free alphabet (e.g. space → 'Ġ').
fn byte_to_unicode() -> Vec<char> {
    let mut bs: Vec<u32> = Vec::new();
    for b in 0x21u32..=0x7e { bs.push(b); }
    for b in 0xa1u32..=0xac { bs.push(b); }
    for b in 0xaeu32..=0xff { bs.push(b); }
    let mut cs = bs.clone();
    let mut n = 0u32;
    for b in 0u32..256 {
        if !bs.contains(&b) { bs.push(b); cs.push(256 + n); n += 1; }
    }
    let mut table = vec!['\0'; 256];
    for (b, c) in bs.iter().zip(cs.iter()) { table[*b as usize] = char::from_u32(*c).unwrap(); }
    table
}

/// The GPT-2 byte→unicode table (byte index → printable char). Exposed for
/// inspection/validation.
pub fn byte_to_unicode_table() -> Vec<char> { byte_to_unicode() }

fn class(c: char) -> u8 {
    if c.is_whitespace() { 0 } else if c.is_alphabetic() { 1 } else if c.is_numeric() { 2 } else { 3 }
}

/// Best-effort GPT-2-style pre-tokenization: same-class runs, with a single
/// leading space kept with the run that follows it.
fn pretokenize(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < n {
        let mut chunk = String::new();
        if chars[i] == ' ' && i + 1 < n && chars[i + 1] != ' ' { chunk.push(' '); i += 1; }
        if i < n {
            let cl = class(chars[i]);
            while i < n && class(chars[i]) == cl { chunk.push(chars[i]); i += 1; }
        }
        if chunk.is_empty() { chunk.push(chars[i]); i += 1; }
        chunks.push(chunk);
    }
    chunks
}

impl Tokenizer {
    pub fn new(tokens: Vec<Vec<u8>>, merge_pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Self {
        Self::build(tokens, merge_pairs, false)
    }
    pub fn new_byte_level(tokens: Vec<Vec<u8>>, merge_pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Self {
        Self::build(tokens, merge_pairs, true)
    }

    fn build(tokens: Vec<Vec<u8>>, merge_pairs: Vec<(Vec<u8>, Vec<u8>)>, byte_level: bool) -> Self {
        let mut vocab = HashMap::new();
        for (i, t) in tokens.iter().enumerate() { vocab.insert(t.clone(), i as u32); }
        let mut merges = HashMap::new();
        for (rank, (a, b)) in merge_pairs.iter().enumerate() {
            if let (Some(&ai), Some(&bi)) = (vocab.get(a), vocab.get(b)) {
                let mut merged = a.clone(); merged.extend_from_slice(b);
                if let Some(&mi) = vocab.get(&merged) { merges.entry((ai, bi)).or_insert((rank as u32, mi)); }
            }
        }
        let byte_encoder = byte_to_unicode();
        let byte_decoder = byte_encoder.iter().enumerate().map(|(b, &c)| (c, b as u8)).collect();
        Self { tokens, vocab, merges, byte_level, byte_encoder, byte_decoder,
            specials: Vec::new(), special_ids: std::collections::HashSet::new() }
    }

    /// Register control/added tokens (id → exact text) matched verbatim during
    /// encode and emitted literally during decode. Longest-first for greedy match.
    pub fn set_specials(&mut self, mut specials: Vec<(String, u32)>) {
        specials.retain(|(s, _)| !s.is_empty());
        specials.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        self.special_ids = specials.iter().map(|(_, id)| *id).collect();
        self.specials = specials;
    }
    pub fn is_special(&self, id: u32) -> bool { self.special_ids.contains(&id) }
    pub fn special_id(&self, text: &str) -> Option<u32> {
        self.specials.iter().find(|(s, _)| s == text).map(|(_, id)| *id)
    }

    pub fn vocab_size(&self) -> usize { self.tokens.len() }
    pub fn is_byte_level(&self) -> bool { self.byte_level }

    /// Greedy lowest-rank merge over a symbol-id sequence.
    fn bpe_merge(&self, mut syms: Vec<u32>) -> Vec<u32> {
        loop {
            let mut best: Option<(usize, u32, u32)> = None; // (pos, rank, merged_id)
            for i in 0..syms.len().saturating_sub(1) {
                if let Some(&(rank, mid)) = self.merges.get(&(syms[i], syms[i + 1])) {
                    if best.map_or(true, |(_, br, _)| rank < br) { best = Some((i, rank, mid)); }
                }
            }
            let Some((pos, _, mid)) = best else { break };
            syms[pos] = mid;
            syms.remove(pos + 1);
        }
        syms
    }

    pub fn encode(&self, text: &str) -> Vec<u32> {
        if self.specials.is_empty() { return self.encode_inner(text); }
        // Split on the earliest special-token match (longest-first), emit its id
        // directly, and byte-BPE the spans between specials.
        let mut out = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let mut best: Option<(usize, usize, u32)> = None; // (pos, len, id)
            for (sp, id) in &self.specials {
                if let Some(pos) = rest.find(sp.as_str()) {
                    if best.map_or(true, |(bp, _, _)| pos < bp) { best = Some((pos, sp.len(), *id)); }
                }
            }
            match best {
                Some((pos, len, id)) => {
                    if pos > 0 { out.extend(self.encode_inner(&rest[..pos])); }
                    out.push(id);
                    rest = &rest[pos + len..];
                }
                None => { out.extend(self.encode_inner(rest)); break; }
            }
        }
        out
    }

    fn encode_inner(&self, text: &str) -> Vec<u32> {
        if !self.byte_level {
            let mut syms = Vec::new();
            for &b in text.as_bytes() { if let Some(&id) = self.vocab.get(&vec![b]) { syms.push(id); } }
            return self.bpe_merge(syms);
        }
        let mut out = Vec::new();
        for chunk in pretokenize(text) {
            let mut syms = Vec::new();
            for &b in chunk.as_bytes() {
                let key = self.byte_encoder[b as usize].to_string().into_bytes();
                if let Some(&id) = self.vocab.get(&key) { syms.push(id); }
            }
            out.extend(self.bpe_merge(syms));
        }
        out
    }

    pub fn decode(&self, ids: &[u32]) -> String {
        String::from_utf8_lossy(&self.decode_bytes(ids)).into_owned()
    }
    /// Raw bytes for `ids` (no lossy UTF-8 conversion). Streaming decoders accumulate
    /// these and emit only complete UTF-8, so a token that is a partial multi-byte
    /// character (common in byte-level BPE / non-ASCII text) doesn't become garbage.
    pub fn decode_bytes(&self, ids: &[u32]) -> Vec<u8> {
        let mut bytes = Vec::new();
        if !self.byte_level {
            for &id in ids { if let Some(t) = self.tokens.get(id as usize) { bytes.extend_from_slice(t); } }
            return bytes;
        }
        for &id in ids {
            let Some(t) = self.tokens.get(id as usize) else { continue };
            if self.special_ids.contains(&id) {
                bytes.extend_from_slice(t);
            } else {
                let s = String::from_utf8_lossy(t);
                for c in s.chars() { if let Some(&b) = self.byte_decoder.get(&c) { bytes.push(b); } }
            }
        }
        bytes
    }

    /// Build from GGUF metadata. Byte-level when `tokenizer.ggml.model` is a BPE
    /// type (gpt2 / llama / bpe); raw otherwise (keeps unit fixtures working).
    pub fn from_gguf(g: &Gguf) -> Result<Self, String> {
        let tokens = match g.metadata.get("tokenizer.ggml.tokens") {
            Some(Value::Arr(a)) => a.iter().map(|v| match v { Value::Str(s) => s.clone().into_bytes(), _ => Vec::new() }).collect(),
            _ => return Err("missing tokenizer.ggml.tokens".into()),
        };
        let merges = match g.metadata.get("tokenizer.ggml.merges") {
            Some(Value::Arr(a)) => a.iter().filter_map(|v| match v {
                Value::Str(s) => { let mut it = s.split(' '); Some((it.next()?.as_bytes().to_vec(), it.next()?.as_bytes().to_vec())) }
                _ => None,
            }).collect(),
            _ => Vec::new(),
        };
        let byte_level = matches!(g.get_str("tokenizer.ggml.model"), Some("gpt2") | Some("llama") | Some("bpe"));
        let mut tk = Self::build(tokens, merges, byte_level);
        // Register control (3) / user-defined (4) tokens as verbatim specials so
        // chat markers like "<|eot_id|>" tokenize to a single id and decode literally.
        if let Some(Value::Arr(types)) = g.metadata.get("tokenizer.ggml.token_type") {
            let mut specials = Vec::new();
            for (i, ty) in types.iter().enumerate() {
                let t = match ty { Value::I32(v) => *v, Value::U32(v) => *v as i32, _ => 0 };
                if (t == 3 || t == 4) && i < tk.tokens.len() {
                    specials.push((String::from_utf8_lossy(&tk.tokens[i]).into_owned(), i as u32));
                }
            }
            tk.set_specials(specials);
        }
        Ok(tk)
    }
}
