//! gguf — read `tokenizer.chat_template` (and its named variants) out of a GGUF file's metadata
//! without loading the model. The header is small and at the front: magic, version, tensor count,
//! KV count, then the KV pairs; every value type has a fixed encoding, so unknown keys are skipped
//! by size. Reads the first few MB at most; a 200 GB Ornith costs the same as a 1 GB Llama.
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Templates {
    /// `tokenizer.chat_template`, the default.
    pub default: Option<String>,
    /// `tokenizer.chat_template.<name>` variants (`tool_use`, `rag`, …), as some GGUFs ship them.
    pub named: Vec<(String, String)>,
    /// `general.architecture`, useful for saying which family this is.
    pub architecture: Option<String>,
    pub name: Option<String>,
}

impl Templates {
    /// The template to use for a chat with tools: the `tool_use` variant when present, else default.
    pub fn for_tools(&self) -> Option<&str> {
        self.named.iter().find(|(n, _)| n == "tool_use").map(|(_, t)| t.as_str()).or(self.default.as_deref())
    }
}

struct Cursor<R: Read> { r: R, version: u32 }

impl<R: Read> Cursor<R> {
    fn u8(&mut self) -> Result<u8, String> { let mut b = [0u8; 1]; self.r.read_exact(&mut b).map_err(|e| e.to_string())?; Ok(b[0]) }
    fn u32(&mut self) -> Result<u32, String> { let mut b = [0u8; 4]; self.r.read_exact(&mut b).map_err(|e| e.to_string())?; Ok(u32::from_le_bytes(b)) }
    fn u64(&mut self) -> Result<u64, String> {
        if self.version == 1 { return self.u32().map(|v| v as u64); }
        let mut b = [0u8; 8]; self.r.read_exact(&mut b).map_err(|e| e.to_string())?; Ok(u64::from_le_bytes(b))
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u64()? as usize;
        if n > 64 << 20 { return Err(format!("string of {n} bytes: not a GGUF")); }
        let mut b = vec![0u8; n]; self.r.read_exact(&mut b).map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&b).to_string())
    }
    fn skip(&mut self, n: u64) -> Result<(), String> { let mut left = n; let mut buf = [0u8; 8192]; while left > 0 { let k = left.min(buf.len() as u64) as usize; self.r.read_exact(&mut buf[..k]).map_err(|e| e.to_string())?; left -= k as u64; } Ok(()) }
    /// Skip one value of type `t`, returning it only when it is a string (the only type we keep).
    fn value(&mut self, t: u32) -> Result<Option<String>, String> {
        match t {
            0 | 1 | 7 => self.skip(1)?,                 // u8 i8 bool
            2 | 3 => self.skip(2)?,                     // u16 i16
            4 | 5 | 6 => self.skip(4)?,                 // u32 i32 f32
            10 | 11 | 12 => self.skip(8)?,              // u64 i64 f64
            8 => return self.string().map(Some),
            9 => {                                      // array: elem type, count, elems
                let et = self.u32()?; let n = self.u64()?;
                for _ in 0..n { self.value(et)?; }
            }
            other => return Err(format!("unknown GGUF value type {other}")),
        }
        Ok(None)
    }
}

pub fn read_templates<R: Read + Seek>(mut r: R) -> Result<Templates, String> {
    r.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut magic = [0u8; 4]; r.read_exact(&mut magic).map_err(|e| e.to_string())?;
    if &magic != b"GGUF" { return Err("not a GGUF file".into()); }
    let mut c = Cursor { r, version: 0 };
    c.version = c.u32()?;
    if !(1..=3).contains(&c.version) { return Err(format!("GGUF version {}", c.version)); }
    let _n_tensors = c.u64()?;
    let n_kv = c.u64()?;
    let mut t = Templates::default();
    for _ in 0..n_kv {
        let key = c.string()?;
        let ty = c.u32()?;
        let v = c.value(ty)?;
        if let Some(s) = v {
            match key.as_str() {
                "tokenizer.chat_template" => t.default = Some(s),
                "general.architecture" => t.architecture = Some(s),
                "general.name" => t.name = Some(s),
                k if k.starts_with("tokenizer.chat_template.") => t.named.push((k["tokenizer.chat_template.".len()..].to_string(), s)),
                _ => {}
            }
        }
    }
    Ok(t)
}

pub fn read_templates_from(path: &Path) -> Result<Templates, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    read_templates(std::io::BufReader::new(f))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor as IoCursor;

    /// A minimal GGUF v3 header with a few KV pairs of mixed types and no tensors.
    fn synth(kvs: &[(&str, u32, Vec<u8>)]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"GGUF"); b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes()); b.extend_from_slice(&(kvs.len() as u64).to_le_bytes());
        for (k, ty, val) in kvs {
            b.extend_from_slice(&(k.len() as u64).to_le_bytes()); b.extend_from_slice(k.as_bytes());
            b.extend_from_slice(&ty.to_le_bytes()); b.extend_from_slice(val);
        }
        b
    }
    fn s(v: &str) -> Vec<u8> { let mut b = (v.len() as u64).to_le_bytes().to_vec(); b.extend_from_slice(v.as_bytes()); b }

    #[test]
    fn finds_the_template_past_other_types_and_arrays() {
        let arr = { let mut b = 8u32.to_le_bytes().to_vec(); b.extend_from_slice(&2u64.to_le_bytes()); b.extend(s("a")); b.extend(s("b")); b };
        let bytes = synth(&[
            ("general.architecture", 8, s("qwen35moe")),
            ("qwen35moe.block_count", 4, 40u32.to_le_bytes().to_vec()),
            ("tokenizer.ggml.tokens", 9, arr),
            ("general.file_type", 4, 15u32.to_le_bytes().to_vec()),
            ("tokenizer.chat_template.tool_use", 8, s("{% for m in messages %}{{ m.content }}{% endfor %}")),
            ("tokenizer.chat_template", 8, s("{{ bos_token }}{% for m in messages %}<|im_start|>{{ m.role }}\n{{ m.content }}<|im_end|>\n{% endfor %}")),
            ("some.f64", 12, 1.5f64.to_le_bytes().to_vec()),
        ]);
        let t = read_templates(IoCursor::new(bytes)).unwrap();
        assert_eq!(t.architecture.as_deref(), Some("qwen35moe"));
        assert!(t.default.as_deref().unwrap().contains("<|im_start|>"));
        assert_eq!(t.named.len(), 1);
        assert!(t.for_tools().unwrap().starts_with("{% for m in messages"), "tool_use variant wins for tool chats");
        assert!(read_templates(IoCursor::new(b"not gguf".to_vec())).is_err());
    }
}
