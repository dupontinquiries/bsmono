//! mob_instances.rs — pure CPU-side mob instance-buffer packer.
//!
//! Turns a list of mob transforms/params into a tightly-packed little-endian
//! byte buffer, sorted by `(mesh_id, material_id)` so the world pass can draw
//! each mesh/material run as one instanced batch. Pure data layout — no `ash`,
//! no `vk`, no rendering types (R1). Deterministic, panic-free (R11/R21). The
//! GPU upload + draw wiring is the coordinator's integration pass; this module
//! only decides bytes and batch ranges.

/// One mob as the gameplay/sim side describes it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MobInstance {
    pub mesh_id: u32,
    pub material_id: u32,
    pub position: [f32; 3],
    pub rotation: f32, // yaw, radians
    pub scale: f32,
    pub tint: [f32; 4],
}

/// The packed, GPU-facing form of one instance. Field order here defines the
/// serialized byte layout (see [`INSTANCE_STRIDE`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct InstanceRaw {
    pub position: [f32; 3],
    pub rotation: f32,
    pub scale: f32,
    pub tint: [f32; 4],
    pub mesh_id: u32,
    pub material_id: u32,
}

/// Bytes per packed instance: 3+1+1 f32 + 4 f32 + 2 u32 = 44 bytes.
pub const INSTANCE_STRIDE: usize = (3 + 1 + 1) * 4 + 4 * 4 + 2 * 4;

impl From<&MobInstance> for InstanceRaw {
    fn from(m: &MobInstance) -> Self {
        InstanceRaw {
            position: m.position,
            rotation: m.rotation,
            scale: m.scale,
            tint: m.tint,
            mesh_id: m.mesh_id,
            material_id: m.material_id,
        }
    }
}

/// A contiguous run of instances sharing a `(mesh_id, material_id)` — one draw
/// batch. `first` is the instance index into the packed buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Batch {
    pub mesh_id: u32,
    pub material_id: u32,
    pub first: u32,
    pub count: u32,
}

/// Result of packing: the instance bytes plus the per-(mesh,material) batches.
#[derive(Clone, PartialEq, Debug)]
pub struct Packed {
    pub bytes: Vec<u8>,
    pub stride: usize,
    pub count: usize,
    pub batches: Vec<Batch>,
}

#[inline]
fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}
#[inline]
fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn write_raw(out: &mut Vec<u8>, r: &InstanceRaw) {
    for &c in &r.position {
        put_f32(out, c);
    }
    put_f32(out, r.rotation);
    put_f32(out, r.scale);
    for &c in &r.tint {
        put_f32(out, c);
    }
    put_u32(out, r.mesh_id);
    put_u32(out, r.material_id);
}

/// Pack mobs into a sorted, batched instance buffer. The sort is **stable**:
/// mobs sharing a `(mesh_id, material_id)` keep their original relative order.
pub fn pack(mobs: &[MobInstance]) -> Packed {
    let mut order: Vec<usize> = (0..mobs.len()).collect();
    order.sort_by_key(|&i| (mobs[i].mesh_id, mobs[i].material_id)); // stable

    let mut bytes = Vec::with_capacity(mobs.len() * INSTANCE_STRIDE);
    for &i in &order {
        write_raw(&mut bytes, &InstanceRaw::from(&mobs[i]));
    }

    let mut batches = Vec::new();
    let mut start = 0usize;
    while start < order.len() {
        let key = (mobs[order[start]].mesh_id, mobs[order[start]].material_id);
        let mut end = start + 1;
        while end < order.len()
            && (mobs[order[end]].mesh_id, mobs[order[end]].material_id) == key
        {
            end += 1;
        }
        batches.push(Batch {
            mesh_id: key.0,
            material_id: key.1,
            first: start as u32,
            count: (end - start) as u32,
        });
        start = end;
    }

    Packed {
        bytes,
        stride: INSTANCE_STRIDE,
        count: mobs.len(),
        batches,
    }
}

#[inline]
fn rd_f32(b: &[u8], p: usize) -> f32 {
    f32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]])
}
#[inline]
fn rd_u32(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]])
}

/// Decode a packed buffer back into instances. Trailing bytes shorter than one
/// stride are ignored (panic-free). Inverse of [`pack`]'s serialization.
pub fn unpack(bytes: &[u8]) -> Vec<InstanceRaw> {
    let mut out = Vec::with_capacity(bytes.len() / INSTANCE_STRIDE);
    let mut o = 0usize;
    while o + INSTANCE_STRIDE <= bytes.len() {
        let position = [rd_f32(bytes, o), rd_f32(bytes, o + 4), rd_f32(bytes, o + 8)];
        let rotation = rd_f32(bytes, o + 12);
        let scale = rd_f32(bytes, o + 16);
        let tint = [
            rd_f32(bytes, o + 20),
            rd_f32(bytes, o + 24),
            rd_f32(bytes, o + 28),
            rd_f32(bytes, o + 32),
        ];
        let mesh_id = rd_u32(bytes, o + 36);
        let material_id = rd_u32(bytes, o + 40);
        out.push(InstanceRaw {
            position,
            rotation,
            scale,
            tint,
            mesh_id,
            material_id,
        });
        o += INSTANCE_STRIDE;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }

    fn mob(mesh: u32, mat: u32, tag: f32) -> MobInstance {
        MobInstance {
            mesh_id: mesh,
            material_id: mat,
            position: [tag, tag + 1.0, tag + 2.0],
            rotation: tag * 0.1,
            scale: 1.0 + tag,
            tint: [tag, 0.5, 0.25, 1.0],
        }
    }

    #[test]
    fn prop_length_is_count_times_stride() {
        let mut seed = 0xA11CE5u32;
        for n in 0..40usize {
            let mobs: Vec<_> = (0..n)
                .map(|_| {
                    mob(
                        (lcg(&mut seed) * 4.0) as u32,
                        (lcg(&mut seed) * 3.0) as u32,
                        lcg(&mut seed),
                    )
                })
                .collect();
            let p = pack(&mobs);
            assert_eq!(p.count, n);
            assert_eq!(p.stride, INSTANCE_STRIDE);
            assert_eq!(p.bytes.len(), n * INSTANCE_STRIDE);
        }
    }

    #[test]
    fn prop_pack_unpack_round_trips() {
        let mut seed = 0xBEEF11u32;
        for n in 0..40usize {
            let mobs: Vec<_> = (0..n)
                .map(|_| {
                    mob(
                        (lcg(&mut seed) * 5.0) as u32,
                        (lcg(&mut seed) * 4.0) as u32,
                        lcg(&mut seed) * 100.0,
                    )
                })
                .collect();
            let p = pack(&mobs);
            let got = unpack(&p.bytes);

            // expected = the mobs in the same stable sorted order pack used.
            let mut order: Vec<usize> = (0..mobs.len()).collect();
            order.sort_by_key(|&i| (mobs[i].mesh_id, mobs[i].material_id));
            let expected: Vec<InstanceRaw> =
                order.iter().map(|&i| InstanceRaw::from(&mobs[i])).collect();

            assert_eq!(got, expected, "round-trip mismatch n={}", n);
            for r in &got {
                for &c in &r.position {
                    assert!(c.is_finite());
                }
                assert!(r.rotation.is_finite() && r.scale.is_finite());
            }
        }
    }

    #[test]
    fn sort_is_stable_within_batch() {
        // Equal (mesh,material) keep input order; distinct positions tag order.
        let mobs = vec![
            mob(2, 0, 10.0),
            mob(1, 0, 20.0),
            mob(2, 0, 30.0),
            mob(1, 0, 40.0),
            mob(2, 0, 50.0),
        ];
        let raws = unpack(&pack(&mobs).bytes);
        // mesh 1 batch first (sorted), preserving input order 20 then 40.
        assert_eq!(raws[0].mesh_id, 1);
        assert_eq!(raws[0].position[0], 20.0);
        assert_eq!(raws[1].position[0], 40.0);
        // mesh 2 batch next, preserving 10, 30, 50.
        assert_eq!(raws[2].position[0], 10.0);
        assert_eq!(raws[3].position[0], 30.0);
        assert_eq!(raws[4].position[0], 50.0);
    }

    #[test]
    fn batches_cover_all_instances_in_order() {
        let mobs = vec![
            mob(3, 1, 1.0),
            mob(1, 0, 2.0),
            mob(3, 1, 3.0),
            mob(1, 2, 4.0),
        ];
        let p = pack(&mobs);
        // batches are sorted, contiguous, and cover every instance exactly once.
        let mut total = 0u32;
        let mut cursor = 0u32;
        let mut prev_key: Option<(u32, u32)> = None;
        for b in &p.batches {
            assert_eq!(b.first, cursor, "batch not contiguous");
            assert!(b.count >= 1);
            if let Some(k) = prev_key {
                assert!((b.mesh_id, b.material_id) > k, "batches not sorted/unique");
            }
            prev_key = Some((b.mesh_id, b.material_id));
            cursor += b.count;
            total += b.count;
        }
        assert_eq!(total as usize, mobs.len());
    }

    #[test]
    fn unpack_ignores_trailing_partial() {
        let mobs = vec![mob(0, 0, 1.0), mob(0, 0, 2.0)];
        let mut bytes = pack(&mobs).bytes;
        bytes.extend_from_slice(&[0u8; 7]); // a partial trailing record
        assert_eq!(unpack(&bytes).len(), 2);
    }
}
