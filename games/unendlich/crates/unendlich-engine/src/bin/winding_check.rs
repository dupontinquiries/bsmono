use unendlich_engine::render::mesher::{mesh_chunk_naive, mesh_chunk_greedy, Vertex};
use unendlich_engine::world::blocks::BlockId;
use unendlich_engine::world::chunk::{Chunk, ChunkCoord};

fn cross(a: [f32;3], b: [f32;3]) -> [f32;3] {
    [a[1]*b[2] - a[2]*b[1], a[2]*b[0] - a[0]*b[2], a[0]*b[1] - a[1]*b[0]]
}

fn sub(a: [f32;3], b: [f32;3]) -> [f32;3] {
    [a[0]-b[0], a[1]-b[1], a[2]-b[2]]
}

fn classify_face(v0: &Vertex, v1: &Vertex, v2: &Vertex) -> String {
    let n = cross(sub(v1.position, v0.position), sub(v2.position, v0.position));
    let nm = (n[0].powi(2) + n[1].powi(2) + n[2].powi(2)).sqrt().max(1e-6);
    let nrm = [n[0]/nm, n[1]/nm, n[2]/nm];
    if nrm[1] > 0.9 { format!("+Y top   (verts: {:?} {:?} {:?})", v0.position, v1.position, v2.position) }
    else if nrm[1] < -0.9 { format!("-Y bot   (verts: {:?} {:?} {:?})", v0.position, v1.position, v2.position) }
    else if nrm[0] > 0.9 { format!("+X east  (verts: {:?} {:?} {:?})", v0.position, v1.position, v2.position) }
    else if nrm[0] < -0.9 { format!("-X west  (verts: {:?} {:?} {:?})", v0.position, v1.position, v2.position) }
    else if nrm[2] > 0.9 { format!("+Z south (verts: {:?} {:?} {:?})", v0.position, v1.position, v2.position) }
    else if nrm[2] < -0.9 { format!("-Z north (verts: {:?} {:?} {:?})", v0.position, v1.position, v2.position) }
    else { format!("? {:?}", nrm) }
}

fn main() {
    // 2x1x2 stone slab — should test multi-block merging on multiple axes.
    let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
    for x in 5..7 {
        for z in 5..7 {
            chunk.set(x, 5, z, BlockId::Stone);
        }
    }

    let n = mesh_chunk_naive(&chunk);
    let g = mesh_chunk_greedy(&chunk, None, None, None);

    println!("=== 2x1x2 slab ===");
    println!("Naive: {} verts, {} tris", n.vertex_count(), n.index_count() / 3);
    for (i, idx) in n.indices.chunks(3).enumerate() {
        println!("  tri {}: {}", i, classify_face(&n.vertices[idx[0] as usize], &n.vertices[idx[1] as usize], &n.vertices[idx[2] as usize]));
    }
    println!();
    println!("Greedy: {} verts, {} tris", g.vertex_count(), g.index_count() / 3);
    for (i, idx) in g.indices.chunks(3).enumerate() {
        println!("  tri {}: {}", i, classify_face(&g.vertices[idx[0] as usize], &g.vertices[idx[1] as usize], &g.vertices[idx[2] as usize]));
    }
}
