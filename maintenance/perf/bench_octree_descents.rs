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

// Faithful model of find_greedy_run (chunk_mesh_octree.comp:770): the run ends on a
// block mismatch OR a solid face-neighbour. BOTH are sampled every step.
fn face_off(x:i32,y:i32,z:i32, face:usize) -> (i32,i32,i32) {
    match face { 0=>(x+1,y,z),1=>(x-1,y,z),2=>(x,y+1,z),3=>(x,y-1,z),4=>(x,y,z+1),_=>(x,y,z-1) }
}

// CURRENT: a full root descent for the cell AND for its face-neighbour, every step.
fn run_current(t:&Tree, x:i32,y:i32,z:i32, axis:usize, face:usize, self_block:u8) -> u32 {
    let mut extent = 1;
    for step in 1..32 {
        let (mut nx,mut ny,mut nz) = (x,y,z);
        match axis {0=>nx+=step,1=>ny+=step,_=>nz+=step};
        if nx<0||ny<0||nz<0||nx>=CHUNK||ny>=CHUNK||nz>=CHUNK { break; }
        let (b,_,_) = t.sample_ext(nx,ny,nz);
        if b != self_block { break; }
        let (fx,fy,fz) = face_off(nx,ny,nz,face);
        let solid = if fx<0||fy<0||fz<0||fx>=CHUNK||fy>=CHUNK||fz>=CHUNK { false }
                    else { t.sample_ext(fx,fy,fz).0 != 0 };
        if solid { break; }
        extent += 1;
    }
    extent
}

// PROPOSED: identical loop, identical conditions, identical result — but each sample
// first consults the covering leaf returned by the previous descent. This is exactly the
// cache the halo loop already uses at chunk_mesh_octree.comp:930. NOT a jump: every step
// is still evaluated, so the face-neighbour test is never skipped.
struct LeafBox { lo:(i32,i32,i32), sz:i32, block:u8, valid:bool }
impl LeafBox {
    fn new()->Self{ LeafBox{lo:(0,0,0),sz:0,block:0,valid:false} }
    fn hit(&self,x:i32,y:i32,z:i32)->bool{
        self.valid && x>=self.lo.0 && x<self.lo.0+self.sz
                   && y>=self.lo.1 && y<self.lo.1+self.sz
                   && z>=self.lo.2 && z<self.lo.2+self.sz
    }
    fn fill(&mut self,b:u8,lo:(i32,i32,i32),sz:i32){ self.block=b; self.lo=lo; self.sz=sz; self.valid=true; }
}

fn run_cached(t:&Tree, x:i32,y:i32,z:i32, axis:usize, face:usize, self_block:u8) -> u32 {
    let mut extent = 1;
    let mut cs = LeafBox::new();   // cache for the run's own cells
    let mut cf = LeafBox::new();   // cache for the face-neighbour line
    for step in 1..32 {
        let (mut nx,mut ny,mut nz) = (x,y,z);
        match axis {0=>nx+=step,1=>ny+=step,_=>nz+=step};
        if nx<0||ny<0||nz<0||nx>=CHUNK||ny>=CHUNK||nz>=CHUNK { break; }
        let b = if cs.hit(nx,ny,nz) { cs.block } else {
            let (bb,lo,sz)=t.sample_ext(nx,ny,nz); cs.fill(bb,lo,sz); bb };
        if b != self_block { break; }
        let (fx,fy,fz) = face_off(nx,ny,nz,face);
        let solid = if fx<0||fy<0||fz<0||fx>=CHUNK||fy>=CHUNK||fz>=CHUNK { false } else {
            let fb = if cf.hit(fx,fy,fz) { cf.block } else {
                let (bb,lo,sz)=t.sample_ext(fx,fy,fz); cf.fill(bb,lo,sz); bb };
            fb != 0 };
        if solid { break; }
        extent += 1;
    }
    extent
}

fn main() {
    let field = |x:i32,y:i32,z:i32| -> u8 {
        let h = 16.0 + 6.0*((x as f32*0.19).sin() + (z as f32*0.23).cos());
        if (y as f32) < h { if (y as f32) < h-4.0 {1} else {2} } else {0}
    };
    let t = Tree::build(&field);
    println!("nodes={}", t.nodes.len());

    // 1. equivalence: identical extents for every cell/axis/face
    let mut checked = 0u64;
    for z in 0..CHUNK { for y in 0..CHUNK { for x in 0..CHUNK {
        let (sb,_,_) = t.sample_ext(x,y,z);
        if sb == 0 { continue; }
        for axis in 0..3 { for face in 0..6 {
            assert_eq!(run_current(&t,x,y,z,axis,face,sb), run_cached(&t,x,y,z,axis,face,sb),
                       "extent differs at {x},{y},{z} axis{axis} face{face}");
            checked += 1;
        }}
    }}}
    println!("equivalence: {checked} (cell,axis,face) runs identical");

    // 2. descent counts
    for (name, f) in [("current (2 descents/step)", run_current as fn(&Tree,i32,i32,i32,usize,usize,u8)->u32),
                      ("leaf-cache memoized",       run_cached)] {
        t.descents.set(0);
        let mut total = 0u64;
        for z in 0..CHUNK { for y in 0..CHUNK { for x in 0..CHUNK {
            let (sb,_,_) = t.sample_ext(x,y,z);
            if sb == 0 { continue; }
            for axis in 0..3 { for face in 0..6 { total += f(&t,x,y,z,axis,face,sb) as u64; } }
        }}}
        println!("{name:28} descents={:>10}  (extents summed: {total})", t.descents.get());
    }
}
