//! r661: a small **bounded** practice arena for the Bending Tutorial.
//!
//! Pure geometry: given a spawn origin `[px, sy, pz]` (with `sy` the surface y),
//! produce the list of `(position, block)` cells to stamp — a floor, a bedrock
//! underlayer, a `WALL_H`-high perimeter wall, a cleared interior air column (so
//! the surrounding terrain never intrudes), and seven element pedestals (one per
//! bending element, each topped with a representative marker block).
//!
//! The bin stamps these via `chunk_manager.set_block` once the tutorial world's
//! spawn chunks are loaded — exactly like the landmark monument. Because the
//! walls are bedrock-based and the floor has a bedrock underlayer, the play area
//! reads as a contained room rather than open world.

use crate::world::blocks::BlockId;

/// Inner half-extent of the arena floor → a `(2*HALF+1)`-square room.
pub const HALF: i32 = 11;
/// Wall height above the floor.
pub const WALL_H: i32 = 5;
/// Ring radius for the element pedestals.
pub const RING_R: f32 = 7.0;

/// The seven bending elements, each mapped to a representative placeable block.
/// (Frequencies/behaviour live in `tutorial_ladder`; here we only need a marker.)
pub const ELEMENTS: [(&str, BlockId); 7] = [
    ("Earth", BlockId::Stone),
    ("Water", BlockId::Ice),
    ("Fire", BlockId::Magma),
    ("Air", BlockId::Glass),
    ("Storm", BlockId::Crystal),
    ("Brine", BlockId::Quartz),
    ("Information", BlockId::GlowCrystal),
];

/// Build the arena block list centred on `origin = [px, sy, pz]` (`sy` = surface).
/// The floor top sits at `sy`; walls rise `sy+1 ..= sy+WALL_H`. Later entries in
/// the returned list intentionally override earlier ones at the same cell (the
/// centre dais and the pedestals overwrite floor/air), so callers should apply
/// them in order.
pub fn arena_blocks(origin: [i32; 3]) -> Vec<([i32; 3], BlockId)> {
    let [ox, sy, oz] = origin;
    let mut out = Vec::new();

    for dx in -HALF..=HALF {
        for dz in -HALF..=HALF {
            let on_edge = dx == -HALF || dx == HALF || dz == -HALF || dz == HALF;
            // Bedrock underlayer + stone-brick floor everywhere.
            out.push(([ox + dx, sy - 1, oz + dz], BlockId::Bedrock));
            out.push(([ox + dx, sy, oz + dz], BlockId::StoneBricks));
            if on_edge {
                // Perimeter wall: bedrock base course, brick above.
                for h in 1..=WALL_H {
                    let b = if h == 1 { BlockId::Bedrock } else { BlockId::Bricks };
                    out.push(([ox + dx, sy + h, oz + dz], b));
                }
            } else {
                // Interior: clear the air column so terrain never intrudes.
                for h in 1..=(WALL_H + 3) {
                    out.push(([ox + dx, sy + h, oz + dz], BlockId::Air));
                }
            }
        }
    }

    // r679: a solid roof caps the hub at wall height so the surrounding open
    // world is NEVER visible from inside — it reads as a dedicated interior
    // chamber, not an arena dropped into procedural terrain. A 4×4 lattice of
    // GlowCrystal keeps it lit; the rest is brick.
    let roof_y = sy + WALL_H + 1;
    for dx in -HALF..=HALF {
        for dz in -HALF..=HALF {
            let lit = dx.rem_euclid(4) == 0 && dz.rem_euclid(4) == 0;
            let b = if lit { BlockId::GlowCrystal } else { BlockId::Bricks };
            out.push(([ox + dx, roof_y, oz + dz], b));
        }
    }

    // Seven element pedestals in a ring: a 2-high cobblestone pillar topped with
    // the element's marker block.
    for (i, (_name, top)) in ELEMENTS.iter().enumerate() {
        let ang = std::f32::consts::TAU * (i as f32) / (ELEMENTS.len() as f32);
        let px = ox + (RING_R * ang.cos()).round() as i32;
        let pz = oz + (RING_R * ang.sin()).round() as i32;
        out.push(([px, sy + 1, pz], BlockId::Cobblestone));
        out.push(([px, sy + 2, pz], BlockId::Cobblestone));
        out.push(([px, sy + 3, pz], *top));
    }

    // Centre dais: a single gold marker flush with the floor where the player
    // starts (overrides the floor stone-brick at that cell).
    out.push(([ox, sy, oz], BlockId::GoldBlock));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_is_full_square_and_bounded() {
        let cells = arena_blocks([100, 64, -40]);
        let side = (2 * HALF + 1) as usize;
        // every floor cell present at sy
        let floor: Vec<_> = cells.iter().filter(|(p, _)| p[1] == 64).collect();
        assert_eq!(floor.len(), side * side + 1); // +1 for the gold dais override
        // all cells within the arena footprint
        assert!(cells
            .iter()
            .all(|(p, _)| (p[0] - 100).abs() <= HALF && (p[2] + 40).abs() <= HALF));
    }

    #[test]
    fn centre_is_gold_and_wins() {
        let cells = arena_blocks([0, 32, 0]);
        // the LAST cell at the centre-floor position should be the gold dais
        let centre = cells
            .iter()
            .filter(|(p, _)| *p == [0, 32, 0])
            .last()
            .map(|(_, b)| *b);
        assert_eq!(centre, Some(BlockId::GoldBlock));
    }

    #[test]
    fn seven_pedestal_tops_match_elements() {
        let cells = arena_blocks([0, 32, 0]);
        for (_name, top) in ELEMENTS.iter() {
            assert!(
                cells.iter().any(|(p, b)| p[1] == 35 && b == top),
                "missing pedestal top {:?}",
                top
            );
        }
    }

    #[test]
    fn walls_reach_full_height() {
        let cells = arena_blocks([0, 32, 0]);
        // a corner column should have blocks at sy+1..=sy+WALL_H
        let corner_heights: Vec<i32> = cells
            .iter()
            .filter(|(p, _)| p[0] == HALF && p[2] == HALF && p[1] > 32)
            .map(|(p, _)| p[1])
            .collect();
        for h in 1..=WALL_H {
            assert!(corner_heights.contains(&(32 + h)));
        }
    }
}
