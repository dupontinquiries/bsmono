//! Number of voxel shards a hit kicks off, from damage magnitude
//! (handoff/BODY_PART_MESHING.md). Capped at `max`. Pure, deterministic.

pub fn shard_count(damage: f32, scale: f32, max: u32) -> u32 {
    let n = (damage.max(0.0) * scale.max(0.0)) as u32;
    n.min(max)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn more_damage_more_shards() { assert!(shard_count(10.0, 1.0, 100) > shard_count(2.0, 1.0, 100)); }
    #[test] fn zero_damage_no_shards() { assert_eq!(shard_count(0.0, 5.0, 100), 0); }
    #[test] fn capped_at_max() { assert_eq!(shard_count(1000.0, 1.0, 50), 50); }
    #[test] fn monotone() {
        let (mut prev, mut d) = (0u32, 0.0f32);
        while d < 60.0 { let n = shard_count(d, 1.0, 100); assert!(n >= prev); prev = n; d += 1.0; }
    }
    #[test] fn bounded() {
        let mut d = 0.0f32;
        while d < 200.0 { assert!(shard_count(d, 2.0, 40) <= 40); d += 5.0; }
    }
    #[test] fn deterministic() { assert_eq!(shard_count(7.0, 3.0, 100), shard_count(7.0, 3.0, 100)); }
}
