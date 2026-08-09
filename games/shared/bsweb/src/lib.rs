//! bsweb — read the parts web from the engines.
//!
//! The Python side builds the web: 40,742 parts from 1,012 vehicles in 36MB,
//! geometry stored once and referenced by placements, every part annotated. The
//! engines could not read a byte of it, which made the whole representation a
//! build-time artefact rather than something a game could stream.
//!
//! ## Dependency-free on purpose
//!
//! No serde, no flate2. `netcode` next door is the same, and for the same
//! reason: this crate is meant to be linked by two engines with different
//! toolchains and pinned rustc versions, and a JSON crate in the graph is a
//! version conflict waiting to happen. The index is a flat JSON object written
//! by our own tool, so the few fields needed are extracted directly.
//!
//! The container is gzip; callers pass the already-decompressed bytes. That is
//! deliberate — an engine already has a decompressor (glTF, textures, save
//! files all need one) and forcing a second is worse than asking for the bytes.
//!
//! ## Material properties travel with the geometry
//!
//! The reason this matters beyond size: a part carries what it IS, not just
//! where its triangles are — `name` (roadwheel, block, disc), `size`,
//! `instances`, and the material fields from the texture analysis. A renderer
//! reading the web gets shading parameters for free, which is the alternative to
//! shipping normal maps: **the numbers a normal map would have implied, without
//! the pixels**.

/// A part's geometry and what the harvest knows about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub id: String,
    /// Harvest category: roadwheel, block, disc, strut, tread_link…
    pub name: Option<String>,
    /// Which model it was first found in.
    pub from: Option<String>,
    /// Rotation-invariant shape signature; placements refer to parts by this.
    pub sig: Option<String>,
    pub instances: u32,
    pub size: f32,
    /// Index into the geometry table — several parts may share one entry.
    pub geom: usize,
    pub nv: usize,
    pub nt: usize,
    /// 0 = zigzag-varint deltas, 2 = u16, 4 = u32.
    pub isize: u8,
    pub lo: [f32; 3],
    pub span: [f32; 3],
    /// Inferred material properties, when the texture analysis covered it.
    pub material: Material,
}

/// What the maps imply about a surface, as scalars rather than textures.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Material {
    /// Mean of the spec map, 0..1. Absent stays None rather than defaulting to
    /// zero: "not measured" and "perfectly matte" shade very differently, and a
    /// renderer that cannot tell them apart makes every unmeasured surface
    /// look wet or dead.
    pub reflectivity: Option<f32>,
    /// Spread of the spec map: uniform bright is polished, dark and noisy is
    /// worn paint.
    pub reflect_variation: Option<f32>,
    /// Fraction of the surface that glows — coverage rather than mean, because
    /// a bright seam and a dim wash have similar means and different looks.
    pub emissive_coverage: Option<f32>,
    /// How much relief the normal map claimed, if there was one.
    pub normal_strength: Option<f32>,
}

impl Material {
    /// Is anything actually known here?
    pub fn is_measured(&self) -> bool {
        self.reflectivity.is_some()
            || self.emissive_coverage.is_some()
            || self.normal_strength.is_some()
    }
}

/// The whole web.
#[derive(Debug, Clone, Default)]
pub struct Web {
    pub bits: u8,
    pub delta: bool,
    pub parts: Vec<Part>,
    payload: Vec<u8>,
    /// (vertex bytes, index bytes) per geometry entry.
    geom: Vec<(usize, usize)>,
}

#[derive(Debug, PartialEq)]
pub enum WebError {
    BadMagic,
    Truncated,
    BadIndex(&'static str),
    UnknownVersion(u32),
}

const MAGIC: &[u8] = b"BSWEB\x01";

impl Web {
    /// Parse decompressed web bytes.
    pub fn parse(bytes: &[u8]) -> Result<Web, WebError> {
        if bytes.len() < MAGIC.len() + 8 || &bytes[..MAGIC.len()] != MAGIC {
            return Err(WebError::BadMagic);
        }
        let mut p = MAGIC.len();
        let nidx = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap()) as usize;
        let npay = u32::from_le_bytes(bytes[p + 4..p + 8].try_into().unwrap()) as usize;
        p += 8;
        // Refused rather than clamped: a truncated web would otherwise parse
        // into plausible-looking parts with geometry read from whatever
        // follows, which is worse than not loading.
        if bytes.len() < p + nidx + npay {
            return Err(WebError::Truncated);
        }
        let idx = &bytes[p..p + nidx];
        let payload = bytes[p + nidx..p + nidx + npay].to_vec();
        let text = core::str::from_utf8(idx).map_err(|_| WebError::BadIndex("utf8"))?;

        let version = num_field(text, "\"version\":").unwrap_or(1.0) as u32;
        if version != 1 {
            return Err(WebError::UnknownVersion(version));
        }
        let bits = num_field(text, "\"bits\":").unwrap_or(16.0) as u8;
        let delta = text.contains("\"delta\":true");

        let geom = parse_geom(text)?;
        let parts = parse_parts(text)?;
        Ok(Web { bits, delta, parts, payload, geom })
    }

    /// Decode one part's vertices, in its own local space.
    pub fn vertices(&self, part: &Part) -> Option<Vec<[f32; 3]>> {
        let (vs, ve) = self.geom_range(part.geom)?;
        let vb = &self.payload[vs..vs + self.geom.get(part.geom)?.0];
        let _ = ve;
        let wide = self.bits > 8;
        let step = if wide { 6 } else { 3 };
        let maxq = if wide { 65535.0f32 } else { 255.0f32 };
        if vb.len() < part.nv * step {
            return None;
        }
        let mut out = Vec::with_capacity(part.nv);
        for i in 0..part.nv {
            let mut v = [0.0f32; 3];
            for d in 0..3 {
                let q = if wide {
                    u16::from_le_bytes([vb[i * 6 + d * 2], vb[i * 6 + d * 2 + 1]]) as f32
                } else {
                    vb[i * 3 + d] as f32
                };
                v[d] = part.lo[d] + q / maxq * part.span[d];
            }
            out.push(v);
        }
        Some(out)
    }

    /// Decode one part's triangles.
    pub fn triangles(&self, part: &Part) -> Option<Vec<[u32; 3]>> {
        let (vs, _ve) = self.geom_range(part.geom)?;
        let (vlen, ilen) = *self.geom.get(part.geom)?;
        let ib = &self.payload[vs + vlen..vs + vlen + ilen];
        let mut out = Vec::with_capacity(part.nt);
        match part.isize {
            0 => {
                // Zigzag varint deltas against a running index. Read as a
                // stream: the width is variable, so there is no offset
                // arithmetic to get wrong.
                let mut pos = 0usize;
                let mut cur: i64 = 0;
                for _ in 0..part.nt {
                    let mut t = [0u32; 3];
                    for slot in t.iter_mut() {
                        let mut shift = 0u32;
                        let mut z: u64 = 0;
                        loop {
                            let b = *ib.get(pos)?;
                            pos += 1;
                            z |= ((b & 0x7F) as u64) << shift;
                            if b < 0x80 {
                                break;
                            }
                            shift += 7;
                        }
                        let d = ((z >> 1) as i64) ^ -((z & 1) as i64);
                        cur += d;
                        if cur < 0 {
                            return None;
                        }
                        *slot = cur as u32;
                    }
                    out.push(t);
                }
            }
            2 => {
                for i in 0..part.nt {
                    let o = i * 6;
                    if o + 6 > ib.len() {
                        return None;
                    }
                    out.push([
                        u16::from_le_bytes([ib[o], ib[o + 1]]) as u32,
                        u16::from_le_bytes([ib[o + 2], ib[o + 3]]) as u32,
                        u16::from_le_bytes([ib[o + 4], ib[o + 5]]) as u32,
                    ]);
                }
            }
            4 => {
                for i in 0..part.nt {
                    let o = i * 12;
                    if o + 12 > ib.len() {
                        return None;
                    }
                    let g = |k: usize| {
                        u32::from_le_bytes([ib[o + k], ib[o + k + 1], ib[o + k + 2], ib[o + k + 3]])
                    };
                    out.push([g(0), g(4), g(8)]);
                }
            }
            _ => return None,
        }
        Some(out)
    }

    /// Parts of a given harvest category, for a slot picker.
    pub fn by_category<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a Part> {
        let want = name.to_string();
        self.parts.iter().filter(move |p| p.name.as_deref() == Some(want.as_str()))
    }

    /// Find a part by its shape signature — how placements refer to geometry.
    pub fn by_sig(&self, sig: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.sig.as_deref() == Some(sig))
    }

    fn geom_range(&self, gi: usize) -> Option<(usize, usize)> {
        let mut off = 0usize;
        for (i, (v, ix)) in self.geom.iter().enumerate() {
            if i == gi {
                return Some((off, off + v + ix));
            }
            off += v + ix;
        }
        None
    }
}

fn num_field(text: &str, key: &str) -> Option<f64> {
    let i = text.find(key)? + key.len();
    let rest = &text[i..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn parse_geom(text: &str) -> Result<Vec<(usize, usize)>, WebError> {
    let i = text.find("\"geom\":[").ok_or(WebError::BadIndex("geom"))? + 8;
    let rest = &text[i..];
    let end = rest.find(']').ok_or(WebError::BadIndex("geom end"))?;
    let mut out = Vec::new();
    for chunk in rest[..end].split('}') {
        if !chunk.contains("\"v\":") {
            continue;
        }
        let v = num_field(chunk, "\"v\":").ok_or(WebError::BadIndex("v"))? as usize;
        let ix = num_field(chunk, "\"i\":").ok_or(WebError::BadIndex("i"))? as usize;
        out.push((v, ix));
    }
    Ok(out)
}

fn str_field(chunk: &str, key: &str) -> Option<String> {
    let i = chunk.find(key)? + key.len();
    let rest = chunk[i..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    let body = &rest[1..];
    let end = body.find('"')?;
    Some(body[..end].to_string())
}

fn arr3(chunk: &str, key: &str) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    if let Some(i) = chunk.find(key) {
        let rest = &chunk[i + key.len()..];
        if let Some(open) = rest.find('[') {
            if let Some(close) = rest[open..].find(']') {
                for (k, s) in rest[open + 1..open + close].split(',').enumerate().take(3) {
                    out[k] = s.trim().parse().unwrap_or(0.0);
                }
            }
        }
    }
    out
}

fn parse_parts(text: &str) -> Result<Vec<Part>, WebError> {
    let i = text.find("\"parts\":[").ok_or(WebError::BadIndex("parts"))? + 9;
    let rest = &text[i..];
    let mut out = Vec::new();
    for chunk in rest.split('{') {
        if !chunk.contains("\"geom\":") {
            continue;
        }
        let chunk = &chunk[..chunk.find('}').unwrap_or(chunk.len())];
        out.push(Part {
            id: str_field(chunk, "\"id\":").unwrap_or_default(),
            name: str_field(chunk, "\"name\":"),
            from: str_field(chunk, "\"from\":"),
            sig: str_field(chunk, "\"sig\":"),
            instances: num_field(chunk, "\"instances\":").unwrap_or(0.0) as u32,
            size: num_field(chunk, "\"size\":").unwrap_or(0.0) as f32,
            geom: num_field(chunk, "\"geom\":").ok_or(WebError::BadIndex("geom idx"))? as usize,
            nv: num_field(chunk, "\"nv\":").ok_or(WebError::BadIndex("nv"))? as usize,
            nt: num_field(chunk, "\"nt\":").ok_or(WebError::BadIndex("nt"))? as usize,
            isize: num_field(chunk, "\"isize\":").unwrap_or(2.0) as u8,
            lo: arr3(chunk, "\"lo\":"),
            span: arr3(chunk, "\"span\":"),
            material: Material {
                reflectivity: num_field(chunk, "\"reflectivity\":").map(|v| v as f32),
                reflect_variation: num_field(chunk, "\"reflect_variation\":").map(|v| v as f32),
                emissive_coverage: num_field(chunk, "\"emissive_coverage\":").map(|v| v as f32),
                normal_strength: num_field(chunk, "\"normal_strength\":").map(|v| v as f32),
            },
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A web built by hand in the same shape `build_web.py` writes, so the
    /// tests exercise the real layout rather than a convenient one.
    fn tiny_web(bits: u8, delta: bool) -> Vec<u8> {
        // One part: a unit tetrahedron, 4 verts, 4 tris.
        let verts: [[u8; 3]; 4] = [[0, 0, 0], [255, 0, 0], [0, 255, 0], [0, 0, 255]];
        let tris: [[u32; 3]; 4] = [[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]];
        let mut vb = Vec::new();
        for v in verts {
            for d in 0..3 {
                if bits > 8 {
                    let q = (v[d] as u16) * 257;
                    vb.extend_from_slice(&q.to_le_bytes());
                } else {
                    vb.push(v[d]);
                }
            }
        }
        let mut ib = Vec::new();
        if delta {
            let mut prev: i64 = 0;
            for t in tris {
                for k in t {
                    let d = k as i64 - prev;
                    prev = k as i64;
                    let mut z = ((d << 1) ^ (d >> 63)) as u64;
                    while z >= 0x80 {
                        ib.push(((z & 0x7F) | 0x80) as u8);
                        z >>= 7;
                    }
                    ib.push(z as u8);
                }
            }
        } else {
            for t in tris {
                for k in t {
                    ib.extend_from_slice(&(k as u16).to_le_bytes());
                }
            }
        }
        let isize_field = if delta { 0 } else { 2 };
        let idx = format!(
            "{{\"version\":1,\"bits\":{},\"delta\":{},\"parts\":[{{\"id\":\"part_wheel_aa\",\
             \"catalog\":\"t\",\"geom\":0,\"nv\":4,\"nt\":4,\"isize\":{},\
             \"lo\":[0.0,0.0,0.0],\"span\":[2.0,2.0,2.0],\"name\":\"wheel\",\
             \"from\":\"tank--patton\",\"sig\":\"abc123\",\"instances\":7,\
             \"size\":0.5,\"reflectivity\":0.62,\"emissive_coverage\":0.11}}],\
             \"geom\":[{{\"v\":{},\"i\":{}}}]}}",
            bits, delta, isize_field, vb.len(), ib.len()
        );
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(idx.len() as u32).to_le_bytes());
        out.extend_from_slice(&((vb.len() + ib.len()) as u32).to_le_bytes());
        out.extend_from_slice(idx.as_bytes());
        out.extend_from_slice(&vb);
        out.extend_from_slice(&ib);
        out
    }

    #[test]
    fn a_web_round_trips_geometry_at_8_bit_with_delta_indices() {
        // The format the builder now defaults to for size.
        let w = Web::parse(&tiny_web(8, true)).expect("parses");
        assert_eq!(w.bits, 8);
        assert!(w.delta);
        let p = &w.parts[0];
        let v = w.vertices(p).expect("vertices");
        let t = w.triangles(p).expect("triangles");
        assert_eq!(v.len(), 4);
        assert_eq!(t, vec![[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]]);
        assert!((v[1][0] - 2.0).abs() < 1e-3, "255 maps to the full span: {:?}", v[1]);
    }

    #[test]
    fn the_16_bit_fixed_index_form_still_reads() {
        // Older webs must keep working — the default was 16-bit absolute until
        // the size work, and a reader that only handled the new form would
        // silently orphan every previously built web.
        let w = Web::parse(&tiny_web(16, false)).expect("parses");
        assert_eq!(w.bits, 16);
        let p = &w.parts[0];
        assert_eq!(w.triangles(p).unwrap()[3], [1, 2, 3]);
        let v = w.vertices(p).unwrap();
        assert!((v[2][1] - 2.0).abs() < 1e-3);
    }

    #[test]
    fn annotations_and_material_properties_survive() {
        // The reason the web is worth reading at all: a part says what it IS.
        let w = Web::parse(&tiny_web(8, true)).unwrap();
        let p = &w.parts[0];
        assert_eq!(p.name.as_deref(), Some("wheel"));
        assert_eq!(p.from.as_deref(), Some("tank--patton"));
        assert_eq!(p.sig.as_deref(), Some("abc123"));
        assert_eq!(p.instances, 7);
        assert!((p.material.reflectivity.unwrap() - 0.62).abs() < 1e-6);
        assert!((p.material.emissive_coverage.unwrap() - 0.11).abs() < 1e-6);
        assert!(p.material.is_measured());
    }

    #[test]
    fn an_unmeasured_material_is_none_rather_than_zero() {
        // "Not measured" and "perfectly matte" shade very differently, and a
        // renderer that cannot tell them apart makes every unmeasured surface
        // look wet or dead.
        let m = Material::default();
        assert!(m.reflectivity.is_none());
        assert!(!m.is_measured());
    }

    #[test]
    fn a_truncated_web_is_refused_rather_than_half_read() {
        // Otherwise it parses into plausible parts whose geometry is read from
        // whatever follows in memory.
        let full = tiny_web(8, true);
        assert_eq!(Web::parse(&full[..full.len() - 4]).unwrap_err(), WebError::Truncated);
    }

    #[test]
    fn bad_magic_is_refused() {
        let mut b = tiny_web(8, true);
        b[2] = b'X';
        assert_eq!(Web::parse(&b).unwrap_err(), WebError::BadMagic);
        assert_eq!(Web::parse(&[]).unwrap_err(), WebError::BadMagic);
    }

    #[test]
    fn a_newer_version_is_refused_rather_than_guessed_at() {
        let b = tiny_web(8, true);
        let s = String::from_utf8_lossy(&b).replace("\"version\":1", "\"version\":2");
        assert!(matches!(Web::parse(s.as_bytes()), Err(WebError::UnknownVersion(2))));
    }

    #[test]
    fn parts_are_findable_by_category_and_signature() {
        // The two lookups a builder and a reconstructor need.
        let w = Web::parse(&tiny_web(8, true)).unwrap();
        assert_eq!(w.by_category("wheel").count(), 1);
        assert_eq!(w.by_category("strut").count(), 0);
        assert!(w.by_sig("abc123").is_some());
        assert!(w.by_sig("nope").is_none());
    }
}
