//! Minimal safetensors reader: 8-byte LE header length, JSON header, raw
//! data. Only what `tools/pth2st.py` writes (F32, C-contiguous). Kept
//! dependency-free on purpose; the model file is the only consumer.

use std::collections::HashMap;
use std::path::Path;

pub struct SafeTensors {
    data: Vec<u8>,
    entries: HashMap<String, (Vec<usize>, usize, usize)>,
    pub metadata: HashMap<String, String>,
}

impl SafeTensors {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let data = std::fs::read(path)?;
        anyhow::ensure!(data.len() >= 8, "safetensors: file too short");
        let hlen = u64::from_le_bytes(data[..8].try_into().unwrap()) as usize;
        anyhow::ensure!(data.len() >= 8 + hlen, "safetensors: truncated header");
        let header: serde_json::Value = serde_json::from_slice(&data[8..8 + hlen])?;
        let obj = header.as_object().ok_or_else(|| anyhow::anyhow!("safetensors: header is not an object"))?;
        let mut entries = HashMap::new();
        let mut metadata = HashMap::new();
        let base = 8 + hlen;
        for (k, v) in obj {
            if k == "__metadata__" {
                if let Some(m) = v.as_object() {
                    for (mk, mv) in m {
                        metadata.insert(mk.clone(), mv.as_str().unwrap_or("").to_string());
                    }
                }
                continue;
            }
            let dtype = v.get("dtype").and_then(|d| d.as_str()).unwrap_or("");
            anyhow::ensure!(dtype == "F32", "safetensors: {k} is {dtype}, only F32 supported (convert with pth2st.py)");
            let shape: Vec<usize> = v.get("shape").and_then(|s| s.as_array()).map(|a| a.iter().filter_map(|x| x.as_u64()).map(|x| x as usize).collect()).unwrap_or_default();
            let off = v.get("data_offsets").and_then(|o| o.as_array()).ok_or_else(|| anyhow::anyhow!("{k}: no data_offsets"))?;
            let (s, e) = (off[0].as_u64().unwrap() as usize, off[1].as_u64().unwrap() as usize);
            anyhow::ensure!(base + e <= data.len() && e >= s, "{k}: data_offsets out of range");
            anyhow::ensure!((e - s) == shape.iter().product::<usize>() * 4, "{k}: size/shape mismatch");
            entries.insert(k.clone(), (shape, base + s, base + e));
        }
        Ok(SafeTensors { data, entries, metadata })
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub fn shape(&self, name: &str) -> Option<&[usize]> {
        self.entries.get(name).map(|(s, _, _)| s.as_slice())
    }

    /// Tensor as f32 (copied; the file may be unaligned).
    pub fn f32(&self, name: &str) -> anyhow::Result<Vec<f32>> {
        let (_, s, e) = self.entries.get(name).ok_or_else(|| anyhow::anyhow!("model has no tensor {name}"))?;
        Ok(self.data[*s..*e].chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
    }
}
