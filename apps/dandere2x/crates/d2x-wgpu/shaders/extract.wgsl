// Extract one tile from the resident padded frame into the network's input
// activation buffer: RGBA8 -> planar f32 in [0,1], with pixel-unshuffle folded
// in.
//
// This exists so a frame needs ONE upload instead of one per tile. The reason
// that matters is a wgpu ordering rule: `queue.write_buffer` writes all land
// before any command in the same submit, so per-tile uploads make it
// impossible to encode several tiles into one command buffer — tile 2's input
// would clobber tile 1's before tile 1's passes ran. With the frame resident
// and the per-tile copy happening *inside* the encoder, a whole frame becomes
// one submit and one wait (logscope's "K steps in one submit").
//
// The frame is uploaded UNPADDED: replicate padding is coordinate clamping,
// which costs nothing here and saves building a padded copy of every frame on
// the host (a 1952x1112 allocation, a replicate fill and ~10% more pixels to
// convert and upload, per frame).
//
//   frame   [y][x]  u32 RGBA8, row stride `frame_w`, size frame_w x frame_h
//   out     [c*u*u + (y%u)*u + (x%u)][y/u][x/u]  f32

struct Params {
    frame_w: u32,
    frame_h: u32,
    // Region origin in UNPADDED frame coordinates; may be negative, and reads
    // outside the frame clamp to the edge pixel (replicate).
    origin_x: i32,
    origin_y: i32,
    region_w: u32,
    region_h: u32,
    unshuffle: u32,
    tw: u32,
    th: u32,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> frame: array<u32>;
@group(0) @binding(2) var<storage, read_write> out: array<f32>;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= p.region_w || y >= p.region_h) {
        return;
    }
    let sx = u32(clamp(p.origin_x + i32(x), 0, i32(p.frame_w) - 1));
    let sy = u32(clamp(p.origin_y + i32(y), 0, i32(p.frame_h) - 1));
    let px = frame[sy * p.frame_w + sx];
    // unpack4x8unorm is the exact inverse of the pack the CPU path uses.
    let rgba = unpack4x8unorm(px);
    let u = p.unshuffle;
    let plane = p.tw * p.th;
    // Channel offset from the sub-pixel phase; base index from the coarse grid.
    let phase = (y % u) * u + (x % u);
    let idx = (y / u) * p.tw + (x / u);
    out[(0u * u * u + phase) * plane + idx] = rgba.x;
    out[(1u * u * u + phase) * plane + idx] = rgba.y;
    out[(2u * u * u + phase) * plane + idx] = rgba.z;
}
