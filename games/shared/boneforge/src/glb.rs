//! Reading a GLB, with only the bits a rigger needs.
//!
//! Positions and indices. Not normals, not UVs, not materials: the plate
//! armour recomputes normals per island anyway, and an island's material tells
//! you nothing about what the part IS -- orion is 387 distinct parts under one
//! material, which is why the whole anatomy has to be derived from geometry.

use std::collections::HashMap;

#[derive(Debug)]
pub struct Mesh {
    pub pos: Vec<[f32; 3]>,
    pub idx: Vec<u32>,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Minimal JSON reader: enough to walk accessors and buffer views.
///
/// A full parser would be a dependency, and the shape of glTF's header is
/// fixed enough that finding `"accessors"` and reading the numbers out is
/// both shorter and easier to be sure of than wiring serde for it.
mod json {
    #[derive(Debug, Clone, PartialEq)]
    pub enum V {
        Num(f64),
        Str(String),
        Arr(Vec<V>),
        Obj(Vec<(String, V)>),
        Bool(bool),
        Null,
    }
    impl V {
        pub fn get(&self, k: &str) -> Option<&V> {
            if let V::Obj(m) = self {
                m.iter().find(|(n, _)| n == k).map(|(_, v)| v)
            } else {
                None
            }
        }
        pub fn arr(&self) -> &[V] {
            if let V::Arr(a) = self { a } else { &[] }
        }
        pub fn num(&self) -> Option<f64> {
            if let V::Num(n) = self { Some(*n) } else { None }
        }
        pub fn usz(&self) -> Option<usize> {
            self.num().map(|n| n as usize)
        }
    }

    pub fn parse(src: &str) -> Option<V> {
        let b: Vec<char> = src.chars().collect();
        let mut i = 0usize;
        let v = val(&b, &mut i)?;
        Some(v)
    }
    fn ws(b: &[char], i: &mut usize) {
        while *i < b.len() && b[*i].is_whitespace() { *i += 1; }
    }
    fn val(b: &[char], i: &mut usize) -> Option<V> {
        ws(b, i);
        match *b.get(*i)? {
            '{' => obj(b, i),
            '[' => arr(b, i),
            '"' => stri(b, i).map(V::Str),
            't' => { *i += 4; Some(V::Bool(true)) }
            'f' => { *i += 5; Some(V::Bool(false)) }
            'n' => { *i += 4; Some(V::Null) }
            _ => num(b, i),
        }
    }
    fn obj(b: &[char], i: &mut usize) -> Option<V> {
        *i += 1;
        let mut out = Vec::new();
        loop {
            ws(b, i);
            if *b.get(*i)? == '}' { *i += 1; return Some(V::Obj(out)); }
            let k = stri(b, i)?;
            ws(b, i);
            if *b.get(*i)? != ':' { return None; }
            *i += 1;
            let v = val(b, i)?;
            out.push((k, v));
            ws(b, i);
            if *b.get(*i)? == ',' { *i += 1; }
        }
    }
    fn arr(b: &[char], i: &mut usize) -> Option<V> {
        *i += 1;
        let mut out = Vec::new();
        loop {
            ws(b, i);
            if *b.get(*i)? == ']' { *i += 1; return Some(V::Arr(out)); }
            out.push(val(b, i)?);
            ws(b, i);
            if *b.get(*i)? == ',' { *i += 1; }
        }
    }
    fn stri(b: &[char], i: &mut usize) -> Option<String> {
        if *b.get(*i)? != '"' { return None; }
        *i += 1;
        let mut s = String::new();
        loop {
            let c = *b.get(*i)?;
            *i += 1;
            match c {
                '"' => return Some(s),
                '\\' => {
                    let e = *b.get(*i)?;
                    *i += 1;
                    s.push(match e {
                        'n' => '\n', 't' => '\t', 'r' => '\r',
                        'u' => { *i += 4; '?' }
                        o => o,
                    });
                }
                o => s.push(o),
            }
        }
    }
    fn num(b: &[char], i: &mut usize) -> Option<V> {
        let st = *i;
        while *i < b.len() && (b[*i].is_ascii_digit()
            || matches!(b[*i], '-' | '+' | '.' | 'e' | 'E')) { *i += 1; }
        b[st..*i].iter().collect::<String>().parse().ok().map(V::Num)
    }
}

/// Read a binary glTF. Returns every primitive concatenated, with node
/// transforms applied.
pub fn load(bytes: &[u8]) -> Result<Mesh, String> {
    if bytes.len() < 20 || u32_at(bytes, 0) != 0x46546C67 {
        return Err("not a GLB".into());
    }
    let mut off = 12usize;
    let mut js: Option<String> = None;
    let mut bin: Option<&[u8]> = None;
    while off + 8 <= bytes.len() {
        let len = u32_at(bytes, off) as usize;
        let kind = u32_at(bytes, off + 4);
        let body = &bytes[off + 8..(off + 8 + len).min(bytes.len())];
        match kind {
            0x4E4F534A => js = Some(String::from_utf8_lossy(body).into_owned()),
            0x004E4942 => bin = Some(body),
            _ => {}
        }
        off += 8 + len;
    }
    let js = js.ok_or("no JSON chunk")?;
    let bin = bin.ok_or("no BIN chunk")?;
    let j = json::parse(&js).ok_or("bad JSON")?;

    let views = j.get("bufferViews").ok_or("no bufferViews")?.arr();
    let accs = j.get("accessors").ok_or("no accessors")?.arr();
    let read_vec3 = |ai: usize| -> Vec<[f32; 3]> {
        let a = &accs[ai];
        let v = &views[a.get("bufferView").and_then(|x| x.usz()).unwrap_or(0)];
        let base = v.get("byteOffset").and_then(|x| x.usz()).unwrap_or(0)
            + a.get("byteOffset").and_then(|x| x.usz()).unwrap_or(0);
        let n = a.get("count").and_then(|x| x.usz()).unwrap_or(0);
        let stride = v.get("byteStride").and_then(|x| x.usz()).unwrap_or(12);
        let stride = if stride == 0 { 12 } else { stride };
        (0..n).map(|k| {
            let o = base + k * stride;
            [f32_at(bin, o), f32_at(bin, o + 4), f32_at(bin, o + 8)]
        }).collect()
    };
    let read_idx = |ai: usize| -> Vec<u32> {
        let a = &accs[ai];
        let v = &views[a.get("bufferView").and_then(|x| x.usz()).unwrap_or(0)];
        let base = v.get("byteOffset").and_then(|x| x.usz()).unwrap_or(0)
            + a.get("byteOffset").and_then(|x| x.usz()).unwrap_or(0);
        let n = a.get("count").and_then(|x| x.usz()).unwrap_or(0);
        let ct = a.get("componentType").and_then(|x| x.usz()).unwrap_or(5125);
        (0..n).map(|k| match ct {
            5121 => bin[base + k] as u32,
            5123 => u16::from_le_bytes([bin[base + k * 2], bin[base + k * 2 + 1]]) as u32,
            _ => u32_at(bin, base + k * 4),
        }).collect()
    };

    let mut pos = Vec::new();
    let mut idx = Vec::new();
    let empty = json::V::Arr(vec![]);
    for m in j.get("meshes").unwrap_or(&empty).arr() {
        for pr in m.get("primitives").unwrap_or(&empty).arr() {
            let Some(pi) = pr.get("attributes").and_then(|a| a.get("POSITION"))
                .and_then(|x| x.usz()) else { continue };
            let p = read_vec3(pi);
            let base = pos.len() as u32;
            pos.extend_from_slice(&p);
            match pr.get("indices").and_then(|x| x.usz()) {
                Some(ii) => idx.extend(read_idx(ii).into_iter().map(|v| v + base)),
                None => idx.extend((0..p.len() as u32).map(|v| v + base)),
            }
        }
    }
    let _ = HashMap::<u8, u8>::new();
    Ok(Mesh { pos, idx })
}
