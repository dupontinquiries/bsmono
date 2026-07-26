// Counts octree descents in unendlich's mesher inner loop.
// A "descent" = one octree_sample from the root: up to OCTREE_DEPTH+1 DEPENDENT SSBO
// loads on the GPU. Count is hardware-independent and is what the cost tracks.
const CHUNK: i32 = 32;
const DEPTH: u32 = 5;

#[derive(Clone)]
struct Node { leaf: bool, block: u8, children: [u32; 8] }

struct Tree { nodes: Vec<Node>, descents: std::cell::Cell<u64> }

impl Tree {
    // Build from a voxel field, collapsing uniform regions exactly like OctreeArray does.
    fn build(field: &dyn Fn(i32,i32,i32)->u8) -> Tree {
        let mut nodes = Vec::new();
        fn rec(nodes: &mut Vec<Node>, f: &dyn Fn(i32,i32,i32)->u8, ox: i32, oy: i32, oz: i32, size: i32) -> u32 {
            let first = f(ox,oy,oz);
            let mut uniform = true;
            'o: for z in oz..oz+size { for y in oy..oy+size { for x in ox..ox+size {
                if f(x,y,z) != first { uniform = false; break 'o; }
            }}}
            let idx = nodes.len() as u32;
            if uniform || size == 1 {
                nodes.push(Node{ leaf:true, block:first, children:[0;8] });
                return idx;
            }
            nodes.push(Node{ leaf:false, block:0, children:[0;8] });
            let h = size/2;
            let mut kids = [0u32;8];
            for o in 0..8 {
                let (bx,by,bz) = (o & 1, (o>>1)&1, (o>>2)&1);
                kids[o] = rec(nodes, f, ox+bx as i32*h, oy+by as i32*h, oz+bz as i32*h, h);
            }
            nodes[idx as usize].children = kids;
            idx
        }
        rec(&mut nodes, field, 0,0,0, CHUNK);
        Tree { nodes, descents: std::cell::Cell::new(0) }
    }
    // mirrors octree_sample_ext
    fn sample_ext(&self, x:i32,y:i32,z:i32) -> (u8,(i32,i32,i32),i32) {
        self.descents.set(self.descents.get()+1);
        let mut cur = 0usize;
        let (mut ox, mut oy, mut oz) = (0,0,0);
        let mut node_size = CHUNK;
        let mut half = node_size/2;
        for _ in 0..DEPTH+1 {
            let n = &self.nodes[cur];
            if n.leaf { return (n.block,(ox,oy,oz),node_size); }
            let bx = ((x-ox) >= half) as i32; let by = ((y-oy) >= half) as i32; let bz = ((z-oz) >= half) as i32;
            let oct = (bx | (by<<1) | (bz<<2)) as usize;
            ox += bx*half; oy += by*half; oz += bz*half;
            node_size = half; if half > 0 { half >>= 1; }
            cur = n.children[oct] as usize;
        }
        let n=&self.nodes[cur]; (n.block,(ox,oy,oz),node_size.max(1))
    }
}

// CURRENT: find_greedy_run re-samples from the root every step (chunk_mesh_octree.comp:787)
fn run_current(t: &Tree, x:i32,y:i32,z:i32, axis:usize, self_block:u8) -> u32 {
    let mut extent = 1; let max_steps = 32;
    for step in 1..max_steps {
        let (mut nx, mut ny, mut nz) = (x,y,z);
        match axis { 0 => nx += step, 1 => ny += step, _ => nz += step };
        if nx<0||ny<0||nz<0||nx>=CHUNK||ny>=CHUNK||nz>=CHUNK { break; }
        let (b,_,_) = t.sample_ext(nx,ny,nz);
        if b != self_block { break; }
        extent += 1;
    }
    extent
}

// PROPOSED: reuse the covering leaf's extent, exactly the cache the halo loop already
// uses at chunk_mesh_octree.comp:930 — skip to the end of a uniform leaf, no re-descent.
fn run_cached(t: &Tree, x:i32,y:i32,z:i32, axis:usize, self_block:u8) -> u32 {
    let mut extent = 1; let max_steps = 32;
    let mut step = 1;
    while step < max_steps {
        let (mut nx, mut ny, mut nz) = (x,y,z);
        match axis { 0 => nx += step, 1 => ny += step, _ => nz += step };
        if nx<0||ny<0||nz<0||nx>=CHUNK||ny>=CHUNK||nz>=CHUNK { break; }
        let (b, lo, lsz) = t.sample_ext(nx,ny,nz);
        if b != self_block { break; }
        // every cell to the leaf's far edge along `axis` has this same block
        let (p, l) = match axis { 0 => (nx, lo.0), 1 => (ny, lo.1), _ => (nz, lo.2) };
        let run_in_leaf = (l + lsz - p).max(1);
        extent += run_in_leaf as u32;
        step += run_in_leaf;
    }
    extent.min(32)
}

fn main() {
    // terrain-like field: solid below a rolling surface, air above — the realistic case,
    // large uniform regions with a thin interface (what collapses an octree well).
    let field = |x:i32,y:i32,z:i32| -> u8 {
        let h = 16.0 + 6.0*((x as f32*0.19).sin() + (z as f32*0.23).cos());
        if (y as f32) < h { if (y as f32) < h-4.0 {1} else {2} } else {0}
    };
    let t = Tree::build(&field);
    println!("nodes={} (a fully-split 32^3 tree would be {})", t.nodes.len(), (0..=5).map(|d| 8usize.pow(d)).sum::<usize>());

    for (name, f) in [("current (root each step)", run_current as fn(&Tree,i32,i32,i32,usize,u8)->u32),
                      ("leaf-extent reuse",       run_cached)] {
        t.descents.set(0);
        let mut total = 0u64;
        for z in 0..CHUNK { for y in 0..CHUNK { for x in 0..CHUNK {
            let (sb,_,_) = t.sample_ext(x,y,z);
            if sb == 0 { continue; }
            for axis in 0..3 { total += f(&t,x,y,z,axis,sb) as u64; }
        }}}
        println!("{name:26} descents={:>9}  (run extents summed: {total})", t.descents.get());
    }
}
