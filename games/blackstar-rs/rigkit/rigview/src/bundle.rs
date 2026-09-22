//! Load a built rig-viewer page's data exactly the way the page does.
//!
//! Per CLAUDE.md: a harness that measures runtime behaviour must load its data
//! the way the runtime does. So this reads the page itself -- RIG, CAR_META
//! and CAR_B64 -- and decodes the blob with the same layout `loadCar` uses,
//! rather than rebuilding anything from the Python sources.

use base64::Engine;
use serde_json::Value;
use std::io::Read;

pub struct Group {
    pub name: String,
    /// Centred on the group's own middle, as loadCar does.
    pub pos: Vec<[f32; 3]>,
    pub idx: Vec<u32>,
    pub centre: [f32; 3],
    /// Dissolve window [start, duration].
    pub dis: [f32; 2],
    pub stream: bool,
}

fn after<'a>(s: &'a str, pat: &str) -> Result<&'a str, String> {
    s.find(pat).map(|i| &s[i + pat.len()..]).ok_or_else(|| format!("page has no `{pat}`"))
}

fn first_json(s: &str) -> Result<Value, String> {
    serde_json::Deserializer::from_str(s)
        .into_iter::<Value>()
        .next()
        .ok_or("no JSON value")?
        .map_err(|e| e.to_string())
}

pub fn load(html: &str) -> Result<(Vec<Group>, bool), String> {
    let rig = first_json(after(html, "const RIG = ")?)?;
    let meta = first_json(after(html, "const CAR_META = ")?)?;
    let b64 = after(html, "const CAR_B64 = \"")?;
    let b64 = &b64[..b64.find('"').ok_or("unterminated CAR_B64")?];
    let gz = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(&gz[..])
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    let f32_at = |o: usize| f32::from_le_bytes([raw[o], raw[o + 1], raw[o + 2], raw[o + 3]]);
    // same header the page reads: lo at 4, scale at 16, groups from 28
    let lo = [f32_at(4), f32_at(8), f32_at(12)];
    let sc = [f32_at(16), f32_at(20), f32_at(24)];
    let place = &rig["car"]["place"];
    let cubes = rig["car"]["cubes"].as_bool().unwrap_or(true);
    let mut off = 28usize;
    let mut out = Vec::new();
    for m in meta.as_array().ok_or("CAR_META not an array")? {
        let nv = m["nv"].as_u64().unwrap_or(0) as usize;
        let nt = m["nt"].as_u64().unwrap_or(0) as usize;
        let c: Vec<f32> = m["c"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
        let mut pos = Vec::with_capacity(nv);
        for k in 0..nv {
            let mut p = [0f32; 3];
            for a in 0..3 {
                let o = off + (k * 3 + a) * 2;
                let q = u16::from_le_bytes([raw[o], raw[o + 1]]) as f32;
                p[a] = q / 65535.0 * sc[a] + lo[a] - c[a];
            }
            pos.push(p);
        }
        off += nv * 3 * 2;
        let mut idx = Vec::with_capacity(nt * 3);
        for k in 0..nt * 3 {
            let o = off + k * 4;
            idx.push(u32::from_le_bytes([raw[o], raw[o + 1], raw[o + 2], raw[o + 3]]));
        }
        off += nt * 3 * 4;
        let name = m["g"].as_str().unwrap_or("").to_string();
        let pl = &place[&name];
        let dis = pl["dis"].as_array().map(|d| [d[0].as_f64().unwrap() as f32, d[1].as_f64().unwrap() as f32])
            .unwrap_or([0.4, 0.22]);
        let stream = pl["st"].as_u64().unwrap_or(0) == 1;
        out.push(Group { name, pos, idx, centre: [c[0], c[1], c[2]], dis, stream });
    }
    if off != raw.len() {
        return Err(format!("blob walk ended at {off} of {} bytes", raw.len()));
    }
    Ok((out, cubes))
}
