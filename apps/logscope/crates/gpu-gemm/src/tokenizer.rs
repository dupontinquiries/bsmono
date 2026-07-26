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
    /// `tokenizer.ggml.bos_token_id` — the sequence-start token, when the vocab declares one.
    pub bos_id: Option<u32>,
    /// `tokenizer.ggml.add_bos_token` — llama.cpp prepends BOS to every prompt when true.
    /// DeepSeek-LLM/Coder depend on it: their chat template has NO in-body special tokens,
    /// so without this the model never sees its start token at all.
    pub add_bos: bool,
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
        Self { bos_id: None, add_bos: false, tokens, vocab, merges, byte_level, byte_encoder, byte_decoder,
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

    /// Every CONTROL special token id — the tokenizer's special set minus the ones that carry real
    /// response content. A model that emits a control token (<|user|>, <|assistant|>, <|observation|>,
    /// <sop>, [gMASK], image/video/audio markers, …) has ended its turn: those must stop generation, not
    /// be detokenized into the chat as text (the "added stuff on the end" bug on converted checkpoints,
    /// Colibri #298). Content-bearing specials — <think>, <tool_call>, <arg_key>, <arg_value> and their
    /// end tags — are EXCLUDED so reasoning/tool-call output is preserved. Reading the tokenizer's own
    /// special flags (not a hardcoded eos name list) is what catches control tokens a converted GGUF's
    /// config forgot to declare.
    pub fn control_stop_ids(&self) -> Vec<u32> {
        // substrings that mark a special as CONTENT, not a turn boundary — kept out of the stop set
        const CONTENT: &[&str] = &["think", "tool_call", "arg_key", "arg_value", "tool_response", "code", "answer"];
        self.specials.iter()
            .filter(|(text, _)| {
                let t = text.to_ascii_lowercase();
                !CONTENT.iter().any(|c| t.contains(c))
            })
            .map(|(_, id)| *id)
            .collect()
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
        tk.bos_id = g.get_u32("tokenizer.ggml.bos_token_id");
        tk.add_bos = g.get_bool("tokenizer.ggml.add_bos_token").unwrap_or(false);
        // Register control (3) / user-defined (4) tokens as verbatim specials so
        // chat markers like "<|eot_id|>" tokenize to a single id and decode literally.
        let mut specials = Vec::new();
        if let Some(Value::Arr(types)) = g.metadata.get("tokenizer.ggml.token_type") {
            for (i, ty) in types.iter().enumerate() {
                let t = match ty { Value::I32(v) => *v, Value::U32(v) => *v as i32, _ => 0 };
                if (t == 3 || t == 4) && i < tk.tokens.len() {
                    specials.push((String::from_utf8_lossy(&tk.tokens[i]).into_owned(), i as u32));
                }
            }
        }
        // Fallback: control markers shaped like <|...|> (ChatML <|im_start|>/<|im_end|>, Llama <|eot_id|>,
        // <|endoftext|>) MUST tokenize to a single id, else the chat template's turn structure is encoded as
        // literal text and the model sees a malformed prompt. Some GGUF conversions omit or mis-set
        // token_type (Qwen3 showed "0 special tokens" while Llama3 got 8), so register marker-shaped vocab
        // tokens here even when token_type didn't flag them.
        let flagged: std::collections::HashSet<u32> = specials.iter().map(|(_, id)| *id).collect();
        for (i, tok) in tk.tokens.iter().enumerate() {
            if flagged.contains(&(i as u32)) { continue; }
            let s = String::from_utf8_lossy(tok);
            if s.len() >= 4 && s.starts_with("<|") && s.ends_with("|>") {
                specials.push((s.into_owned(), i as u32));
            }
        }
        tk.set_specials(specials);
        // Diagnostic: did the ChatML/Llama markers actually register? If these are None, the vocab doesn't
        // store them as literal "<|...|>" strings (so the <|...|> scan can't see them); if they're Some but
        // the prompt still shows "0 special tokens", the mismatch is in encode, not registration. Also dump
        // the raw stored strings at the known Qwen special ids to see what the vocab actually holds.
        let show = |id: u32| tk.tokens.get(id as usize).map(|t| String::from_utf8_lossy(t).into_owned());
        eprintln!("tokenizer: {} specials registered | im_start={:?} im_end={:?} endoftext={:?} eot_id={:?} | raw[151643..=151645]={:?},{:?},{:?}",
            tk.specials.len(), tk.special_id("<|im_start|>"), tk.special_id("<|im_end|>"),
            tk.special_id("<|endoftext|>"), tk.special_id("<|eot_id|>"),
            show(151643), show(151644), show(151645));
        Ok(tk)
    }
}

#[cfg(test)]
mod control_stop_tests {
    use super::*;

    fn tok_with_specials(specials: &[(&str, u32)]) -> Tokenizer {
        // minimal byte-level tokenizer; only the special set matters for this test
        let tokens: Vec<Vec<u8>> = (0..256u32).map(|b| vec![b as u8]).collect();
        let mut t = Tokenizer::new_byte_level(tokens, Vec::new());
        t.set_specials(specials.iter().map(|(s, id)| (s.to_string(), *id)).collect());
        t
    }

    #[test]
    fn control_stop_ids_excludes_content_specials() {
        // Mix of turn-boundary control tokens and content-bearing specials (GLM/Llama-ish).
        let t = tok_with_specials(&[
            ("<|user|>", 1), ("<|assistant|>", 2), ("<|observation|>", 3),
            ("<sop>", 4), ("[gMASK]", 5), ("<|begin_of_image|>", 6),
            ("<think>", 10), ("</think>", 11), ("<tool_call>", 12), ("<arg_key>", 13),
        ]);
        let stop: std::collections::HashSet<u32> = t.control_stop_ids().into_iter().collect();
        // control tokens → stop
        for id in [1u32, 2, 3, 4, 5, 6] {
            assert!(stop.contains(&id), "control special {id} must be a stop");
        }
        // content-bearing specials → NOT stop (reasoning / tool-call output is real content)
        for id in [10u32, 11, 12, 13] {
            assert!(!stop.contains(&id), "content special {id} must NOT be a stop");
        }
    }

    #[test]
    fn control_stop_ids_empty_when_no_specials() {
        let t = tok_with_specials(&[]);
        assert!(t.control_stop_ids().is_empty());
    }
}
