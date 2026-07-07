// Models the PER-FRAME creature mesh rebuild at fullgame line ~7470:
//   for each of 200 creatures: creature_to_vertices(cr) -> Vec<Vertex>, extend.
// A dragon "forge body" is many boxes (body segs, head, neck, 4 legs, 2 wings,
// tail). Each box = 24 verts (4/face × 6) + 36 indices. We model the per-box
// vertex generation (transform 8 corners, emit per-face verts with normals +
// color + a little trig for wing flex) and the Vec growth, every frame.
use std::time::Instant;

#[derive(Clone, Copy, Default)]
struct V { pos:[f32;3], nrm:[f32;3], col:[f32;3] }

const DRAGONS: usize = 100;
const FAUNA: usize = 100;
const DRAGON_BOXES: usize = 22; // body(4)+neck(3)+head(2)+legs(4)+wings(6 segs)+tail(3)
const FAUNA_BOXES: usize = 8;   // smaller quadruped
const FRAMES: usize = 600;

#[inline]
fn emit_box(out: &mut Vec<V>, idx: &mut Vec<u32>, center:[f32;3], half:[f32;3], flex:f32) {
    // wing-flex style rotation of corners (a little trig per box, as the real
    // forge_to_vertices does for animated parts)
    let (s,c) = flex.sin_cos();
    let base = out.len() as u32;
    // 6 faces × 4 verts
    let corners = [
        [-1.,-1.,-1.],[1.,-1.,-1.],[1.,1.,-1.],[-1.,1.,-1.],
        [-1.,-1.,1.],[1.,-1.,1.],[1.,1.,1.],[-1.,1.,1.],
    ];
    let faces = [[0,1,2,3],[5,4,7,6],[4,0,3,7],[1,5,6,2],[3,2,6,7],[4,5,1,0]];
    let norms = [[0.,0.,-1.],[0.,0.,1.],[-1.,0.,0.],[1.,0.,0.],[0.,1.,0.],[0.,-1.,0.]];
    for (fi,f) in faces.iter().enumerate() {
        for &ci in f.iter() {
            let cc = corners[ci];
            // scale by half-extents, apply flex rotation about Z, translate
            let x = cc[0]*half[0]; let y = cc[1]*half[1]; let z = cc[2]*half[2];
            let rx = x*c - y*s; let ry = x*s + y*c;
            out.push(V{
                pos:[center[0]+rx, center[1]+ry, center[2]+z],
                nrm:norms[fi],
                col:[0.4+0.1*s, 0.3, 0.2],
            });
        }
        idx.extend_from_slice(&[base,base+1,base+2,base,base+2,base+3]);
    }
}

fn creature_to_vertices(out:&mut Vec<V>, idx:&mut Vec<u32>, origin:[f32;3], boxes:usize, t:f32) {
    for b in 0..boxes {
        let off = b as f32 * 0.5;
        let flex = if b >= boxes.saturating_sub(6) { (t*3.0 + off).sin()*0.4 } else { 0.0 }; // wings flex
        emit_box(out, idx, [origin[0]+off, origin[1], origin[2]], [0.5,0.4,0.3], flex);
    }
}

fn main() {
    // positions
    let mut pos = vec![[0f32;3]; DRAGONS+FAUNA];
    let mut s=0x9e37u64; let mut rnd=||{s^=s<<13;s^=s>>7;s^=s<<17;(s>>11)as f32/(1u64<<53)as f32};
    for p in pos.iter_mut(){ *p=[rnd()*256.-128., rnd()*64., rnd()*256.-128.]; }

    // ---- per-frame full rebuild (what the bin does now) ----
    let mut max_verts=0; let mut max_idx=0;
    let t0=Instant::now();
    for f in 0..FRAMES {
        let t = f as f32 * 0.016;
        let mut everts:Vec<V>=Vec::new();
        let mut eindices:Vec<u32>=Vec::new();
        for i in 0..DRAGONS { creature_to_vertices(&mut everts,&mut eindices,pos[i],DRAGON_BOXES,t); }
        for i in 0..FAUNA   { creature_to_vertices(&mut everts,&mut eindices,pos[DRAGONS+i],FAUNA_BOXES,t); }
        max_verts=everts.len(); max_idx=eindices.len();
        std::hint::black_box((&everts,&eindices));
    }
    let per_frame = t0.elapsed().as_secs_f64()*1e3/FRAMES as f64;
    println!("PER-FRAME creature mesh rebuild: {:.3} ms/frame", per_frame);
    println!("  ({} dragons×{} boxes + {} fauna×{} boxes => {} verts, {} indices, fresh Vec each frame)",
        DRAGONS,DRAGON_BOXES,FAUNA,FAUNA_BOXES,max_verts,max_idx);
    println!("  NOTE: host is llvmpipe CPU (slower than M2); RELATIVE magnitude is the point.");
}
