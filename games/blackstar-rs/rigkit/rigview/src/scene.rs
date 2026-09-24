//! A dump of what the page's own refresh() put on screen (tools/dump.js):
//! geometries once, then per frame every visible mesh's world matrix,
//! dissolve state and colour. Nothing about the rig is recomputed here.

pub struct Geo { pub verts: Vec<f32>, pub idx: Vec<u32>, pub lo: [f32; 3], pub hi: [f32; 3] }
pub struct Draw { pub geo: usize, pub m: [f32; 16], pub vox: f32, pub stream: f32, pub c: [f32; 3],
                 pub bq: [[f32; 4]; 22], pub bt: [[f32; 4]; 22], pub mo: [f32; 8] }
pub struct Frame { pub t: f32, pub draws: Vec<Draw> }

pub fn load(b: &[u8]) -> Result<(Vec<Geo>, Vec<Frame>), String> {
    if &b[..4] != b"RVSC" { return Err("not a scene dump".into()); }
    let mut o = 4usize;
    let mut u = || { let v = u32::from_le_bytes([b[o], b[o+1], b[o+2], b[o+3]]); o += 4; v };
    let ng = u() as usize; let nf = u() as usize;
    let mut o2 = o;
    let rd_u = |o: &mut usize| { let v = u32::from_le_bytes([b[*o], b[*o+1], b[*o+2], b[*o+3]]); *o += 4; v };
    let rd_f = |o: &mut usize| { let v = f32::from_le_bytes([b[*o], b[*o+1], b[*o+2], b[*o+3]]); *o += 4; v };
    let mut geos = Vec::with_capacity(ng);
    for _ in 0..ng {
        let n = rd_u(&mut o2) as usize; let ni = rd_u(&mut o2) as usize; let fl = rd_u(&mut o2);
        let (hc, hb) = (fl & 1 == 1, fl & 2 == 2);
        let rdv = |o: &mut usize, k: usize| -> Vec<f32> { (0..k).map(|_| { let v = f32::from_le_bytes([b[*o], b[*o+1], b[*o+2], b[*o+3]]); *o += 4; v }).collect() };
        let pos = rdv(&mut o2, n * 3); let nrm = rdv(&mut o2, n * 3);
        let cell = if hc { rdv(&mut o2, n * 3) } else { vec![0.0; n * 3] };
        let bone = if hb { rdv(&mut o2, n) } else { vec![0.0; n] };
        let anch = if hb { rdv(&mut o2, n * 3) } else { vec![0.0; n * 3] };
        let idx: Vec<u32> = if ni > 0 { (0..ni).map(|_| rd_u(&mut o2)).collect() } else { (0..n as u32).collect() };
        let mut verts = Vec::with_capacity(n * 13);
        for i in 0..n { verts.extend_from_slice(&pos[i*3..i*3+3]); verts.extend_from_slice(&nrm[i*3..i*3+3]);
                        verts.extend_from_slice(&cell[i*3..i*3+3]); verts.push(bone[i]); verts.extend_from_slice(&anch[i*3..i*3+3]); }
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for i in 0..n { for k in 0..3 { lo[k] = lo[k].min(pos[i*3+k]); hi[k] = hi[k].max(pos[i*3+k]); } }
        geos.push(Geo { verts, idx, lo, hi });
    }
    let mut frames = Vec::with_capacity(nf);
    for _ in 0..nf {
        let t = rd_f(&mut o2); let nd = rd_u(&mut o2) as usize;
        let mut draws = Vec::with_capacity(nd);
        for _ in 0..nd {
            let geo = rd_u(&mut o2) as usize;
            let mut m = [0f32; 16]; for k in 0..16 { m[k] = rd_f(&mut o2); }
            let vox = rd_f(&mut o2); let stream = rd_f(&mut o2);
            let c = [rd_f(&mut o2), rd_f(&mut o2), rd_f(&mut o2)];
            let mut bq = [[0f32; 4]; 22]; let mut bt = [[0f32; 4]; 22];
            for i in 0..22 { for k in 0..4 { bq[i][k] = rd_f(&mut o2); } }
            for i in 0..22 { for k in 0..4 { bt[i][k] = rd_f(&mut o2); } }
            let mut mo = [0f32; 8]; for k in 0..8 { mo[k] = rd_f(&mut o2); }
            draws.push(Draw { geo, m, vox, stream, c, bq, bt, mo });
        }
        frames.push(Frame { t, draws });
    }
    if o2 != b.len() { return Err(format!("dump walk ended at {o2} of {}", b.len())); }
    Ok((geos, frames))
}
