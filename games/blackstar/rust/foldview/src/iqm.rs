//! Minimal IQM v2 reader for blackstar's static part meshes.
//! These files carry no skeleton — the fold rig is built in `rig`, not read here.

use anyhow::{bail, Context, Result};

pub const MAGIC: &[u8; 16] = b"INTERQUAKEMODEL\0";
const VA_POSITION: u32 = 0;
const VA_NORMAL: u32 = 2;
const FMT_FLOAT: u32 = 7;

#[derive(Debug, Clone)]
pub struct Mesh {
    pub name: String,
    pub pos: Vec<[f64; 3]>,
    pub nrm: Vec<[f64; 3]>,
    pub idx: Vec<u32>, // local indices into pos/nrm
}

#[derive(Debug, Clone, Default)]
pub struct Model {
    pub meshes: Vec<Mesh>,
}

fn u32_at(b: &[u8], o: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(b.get(o..o + 4).context("truncated")?.try_into()?))
}
fn f32_at(b: &[u8], o: usize) -> Result<f32> {
    Ok(f32::from_le_bytes(b.get(o..o + 4).context("truncated")?.try_into()?))
}

pub fn load(path: &std::path::Path) -> Result<Model> {
    let b = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if b.len() < 124 || &b[..16] != MAGIC {
        bail!("{}: not an IQM file", path.display());
    }
    let h = |i: usize| u32_at(&b, 16 + i * 4); // header word i after magic
    let version = h(0)?;
    if version != 2 {
        bail!("{}: IQM version {version}, want 2", path.display());
    }
    let (num_text, ofs_text) = (h(3)? as usize, h(4)? as usize);
    let (num_meshes, ofs_meshes) = (h(5)? as usize, h(6)? as usize);
    let (num_va, num_verts, ofs_va) = (h(7)? as usize, h(8)? as usize, h(9)? as usize);
    let (num_tris, ofs_tris) = (h(10)? as usize, h(11)? as usize);

    let text = b.get(ofs_text..ofs_text + num_text).context("text table oob")?;
    let cstr = |o: usize| -> String {
        let s = &text[o.min(text.len())..];
        let end = s.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf8_lossy(&s[..end]).into_owned()
    };

    // vertex arrays: type, flags, format, size, offset
    let (mut pos, mut nrm) = (Vec::new(), Vec::new());
    for i in 0..num_va {
        let o = ofs_va + i * 20;
        let (ty, fmt, size, off) =
            (u32_at(&b, o)?, u32_at(&b, o + 8)?, u32_at(&b, o + 12)? as usize, u32_at(&b, o + 16)? as usize);
        if fmt != FMT_FLOAT {
            continue; // blend/color arrays are irrelevant for static shells
        }
        let read3 = |out: &mut Vec<[f64; 3]>| -> Result<()> {
            for v in 0..num_verts {
                let p = off + v * size * 4;
                out.push([f32_at(&b, p)? as f64, f32_at(&b, p + 4)? as f64, f32_at(&b, p + 8)? as f64]);
            }
            Ok(())
        };
        match ty {
            VA_POSITION if size >= 3 => read3(&mut pos)?,
            VA_NORMAL if size >= 3 => read3(&mut nrm)?,
            _ => {}
        }
    }
    if pos.is_empty() {
        bail!("{}: no position array", path.display());
    }
    if nrm.len() != pos.len() {
        nrm = vec![[0.0, 1.0, 0.0]; pos.len()]; // some emitted parts skip normals
    }

    let mut tris = Vec::with_capacity(num_tris * 3);
    for i in 0..num_tris * 3 {
        tris.push(u32_at(&b, ofs_tris + i * 4)?);
    }

    // slice global buffers into per-mesh local buffers
    let mut model = Model::default();
    for m in 0..num_meshes.max(1) {
        let (name, fv, nv, ft, nt) = if num_meshes == 0 {
            ("mesh".to_string(), 0, pos.len() as u32, 0, num_tris as u32)
        } else {
            let o = ofs_meshes + m * 24;
            (cstr(u32_at(&b, o)? as usize), u32_at(&b, o + 8)?, u32_at(&b, o + 12)?, u32_at(&b, o + 16)?, u32_at(&b, o + 20)?)
        };
        let (fv, nv, ft, nt) = (fv as usize, nv as usize, ft as usize, nt as usize);
        let idx = tris
            .get(ft * 3..(ft + nt) * 3)
            .context("mesh tri range oob")?
            .iter()
            .map(|&g| g.checked_sub(fv as u32).context("tri index below mesh base"))
            .collect::<Result<Vec<_>>>()?;
        model.meshes.push(Mesh {
            name,
            pos: pos.get(fv..fv + nv).context("mesh vert range oob")?.to_vec(),
            nrm: nrm[fv..fv + nv].to_vec(),
            idx,
        });
    }
    Ok(model)
}

pub fn centroid(pts: &[[f64; 3]]) -> [f64; 3] {
    let n = pts.len().max(1) as f64;
    let mut c = [0.0; 3];
    for p in pts {
        for a in 0..3 {
            c[a] += p[a] / n;
        }
    }
    c
}

pub fn bbox(pts: &[[f64; 3]]) -> ([f64; 3], [f64; 3]) {
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for p in pts {
        for a in 0..3 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    (lo, hi)
}
