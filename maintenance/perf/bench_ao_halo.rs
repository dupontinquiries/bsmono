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


// Models the AO sampler (chunk_mesh_octree.comp:284-370): each EMITTED face costs
// 4 corners x 3 ao_solid = 12 samples. Counts how many the r480 halo path converts
// from a root descent into a shared-memory read.
fn main() {
    let field = |x:i32,y:i32,z:i32| -> u8 {
        let h = 16.0 + 6.0*((x as f32*0.19).sin() + (z as f32*0.23).cos());
        if (y as f32) < h { if (y as f32) < h-4.0 {1} else {2} } else {0}
    };
    let t = Tree::build(&field);
    let solid = |x:i32,y:i32,z:i32| -> bool {
        if x<0||y<0||z<0||x>=CHUNK||y>=CHUNK||z>=CHUNK { return false; }
        t.sample_ext(x,y,z).0 != 0
    };
    const FACES: [(i32,i32,i32);6] = [(1,0,0),(-1,0,0),(0,1,0),(0,-1,0),(0,0,1),(0,0,-1)];
    let mut emitted = 0u64; let mut ao_samples = 0u64;
    let mut from_halo = 0u64; let mut multi_oob = 0u64; let mut descent = 0u64;

    for tz in 0..CHUNK/4 { for ty in 0..CHUNK/4 { for tx in 0..CHUNK/4 {
        let (bx,by,bz) = (tx*4, ty*4, tz*4);          // the 4^3 tile, halo covers -1..4
        for lz in 0..4 { for ly in 0..4 { for lx in 0..4 {
            let (x,y,z) = (bx+lx, by+ly, bz+lz);
            if !solid(x,y,z) { continue; }
            for (fi,&(dx,dy,dz)) in FACES.iter().enumerate() {
                if solid(x+dx,y+dy,z+dz) { continue; }   // occluded: no quad
                emitted += 1;
                let _ = fi;
                // 12 samples, all within +/-1 of the cell
                for &(sx,sy,sz) in &[(-1,-1,0),(1,-1,0),(-1,1,0),(1,1,0),
                                     (0,-1,-1),(0,1,-1),(0,-1,1),(0,1,1),
                                     (-1,0,-1),(1,0,-1),(-1,0,1),(1,0,1)] {
                    let (ax,ay,az) = (x+sx, y+sy, z+sz);
                    ao_samples += 1;
                    let oob = (ax<0||ax>=CHUNK) as i32 + (ay<0||ay>=CHUNK) as i32 + (az<0||az>=CHUNK) as i32;
                    if oob > 1 { multi_oob += 1; continue; }          // early-out, unchanged
                    let (hx,hy,hz) = (ax-bx+1, ay-by+1, az-bz+1);
                    if hx>=0&&hx<6&&hy>=0&&hy<6&&hz>=0&&hz<6 { from_halo += 1; } else { descent += 1; }
                }
            }
        }}}
    }}}
    println!("emitted faces        {emitted}");
    println!("AO samples           {ao_samples}");
    println!("  multi-axis OOB     {multi_oob}  (early-out, behaviour unchanged)");
    println!("  served from halo   {from_halo}  <- were root descents, now LDS reads");
    println!("  still a descent    {descent}");
    println!("descents removed:    {:.1}% of AO samples", 100.0*from_halo as f64/ao_samples as f64);
}
