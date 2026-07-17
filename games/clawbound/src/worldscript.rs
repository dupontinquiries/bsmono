//! worldscript.rs — a plain-text instruction pipeline for hand-tuning Clawbound's
//! procedurally generated worlds, in the mould of soldier's map forge
//! (`buildscript.rs`) and paperbound's `rts_editor.py`.
//!
//! Clawbound's worlds are procedural — heightfield, biomes, and scattered cover
//! all derive from hashes. A worldscript doesn't REPLACE that; it LAYERS on top:
//!   • density multipliers scale how thickly the generator scatters cover,
//!   • `clear` ops carve open arenas out of the procedural scatter,
//!   • `bush`/`tree`/`rock` ops pin hand-placed cover at exact spots
//!     (a guaranteed bramble by the beacon; a landmark boulder at mid),
//!   • `beacon`/`spawn` ops pin the objective & team start points.
//!
//! Format: a tiny sectioned TEXT file — one op per line, `#` comments — in the
//! same no-serde spirit as the rest of this codebase (and soldier's on-disk
//! `.map`). Designed so tooling — including an LLM shown this header — can
//! author or edit a world without touching code:
//!
//! ```text
//! # clawbound worldscript v1
//! name Foxglove Hollow
//! densities 1.2 0.8 1.0        # tree, reed, rock multipliers
//! clear 0 0 14                 # x z radius — an open arena at centre
//! bush 10.0 -14.5 2.0          # x z radius
//! tree 22 8 1                  # x z kind (0 conifer, 1 broadleaf)
//! rock -30 12 1.4              # x z size
//! beacon 60 -80                # objective position
//! spawn 0 -100 -100            # team x z
//! spawn 1 100 100
//! ```
//!
//! Buildscript principles carried over on purpose:
//!   - **Ops are a sequence, not a snapshot** — diff-able, natural to generate.
//!   - **Unknown/bad lines don't abort the script** — `parse()` collects per-line
//!     errors and keeps going; one typo shouldn't void the rest of the file.
//!   - **`export()` round-trips** — read the current script, propose a change,
//!     hand back a full document.
//!
//! Integration: `set_active()` installs a script process-wide (OnceLock, like
//! `world::GRID`); `world.rs` placement fns consult `active()` to filter
//! cleared areas and append pinned cover, and `sim` reads `beacon`/`spawns`.

use std::sync::OnceLock;

/// Density multipliers applied to the procedural scatter (1.0 = stock).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Densities {
    pub tree: f32,
    pub reed: f32,
    pub rock: f32,
}

impl Default for Densities {
    fn default() -> Self {
        Self { tree: 1.0, reed: 1.0, rock: 1.0 }
    }
}

/// One hand-authored placement / directive.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Carve procedural cover OUT of a disc — an open arena.
    Clear { x: f32, z: f32, r: f32 },
    /// Pin a bush (crouch cover) at an exact spot.
    Bush { x: f32, z: f32, r: f32 },
    /// Pin a tree. kind: 0 conifer, 1 broadleaf.
    Tree { x: f32, z: f32, kind: u8 },
    /// Pin a rock cluster of roughly this size.
    Rock { x: f32, z: f32, s: f32 },
    /// Pin the escape beacon.
    Beacon { x: f32, z: f32 },
    /// Pin a team spawn. team 0 = cats, 1 = hunter side.
    Spawn { team: u8, x: f32, z: f32 },
}

#[derive(Clone, Debug, Default)]
pub struct WorldScript {
    pub name: String,
    pub densities: Densities,
    pub ops: Vec<Op>,
}

impl WorldScript {
    // ── queries the world/sim builders consume ──

    pub fn cleared(&self, x: f32, z: f32) -> bool {
        self.ops.iter().any(|op| match *op {
            Op::Clear { x: cx, z: cz, r } => {
                let dx = x - cx;
                let dz = z - cz;
                dx * dx + dz * dz < r * r
            }
            _ => false,
        })
    }

    pub fn pinned_bushes(&self) -> Vec<(f32, f32, f32)> {
        self.ops
            .iter()
            .filter_map(|op| match *op {
                Op::Bush { x, z, r } => Some((x, z, r)),
                _ => None,
            })
            .collect()
    }

    pub fn pinned_trees(&self) -> Vec<(f32, f32, u8)> {
        self.ops
            .iter()
            .filter_map(|op| match *op {
                Op::Tree { x, z, kind } => Some((x, z, kind)),
                _ => None,
            })
            .collect()
    }

    pub fn pinned_rocks(&self) -> Vec<(f32, f32, f32)> {
        self.ops
            .iter()
            .filter_map(|op| match *op {
                Op::Rock { x, z, s } => Some((x, z, s)),
                _ => None,
            })
            .collect()
    }

    pub fn beacon(&self) -> Option<(f32, f32)> {
        self.ops.iter().rev().find_map(|op| match *op {
            Op::Beacon { x, z } => Some((x, z)),
            _ => None,
        })
    }

    pub fn spawn(&self, team: u8) -> Option<(f32, f32)> {
        self.ops.iter().rev().find_map(|op| match *op {
            Op::Spawn { team: t, x, z } if t == team => Some((x, z)),
            _ => None,
        })
    }

    /// Filter a procedural placement list through the clear-discs and append
    /// the pinned extras — the one helper every `*_positions` fn needs.
    pub fn shape_positions<T: Copy>(
        &self,
        procedural: Vec<T>,
        pos_of: impl Fn(&T) -> (f32, f32),
        pinned: Vec<T>,
    ) -> Vec<T> {
        let mut v: Vec<T> = procedural
            .into_iter()
            .filter(|t| {
                let (x, z) = pos_of(t);
                !self.cleared(x, z)
            })
            .collect();
        v.extend(pinned);
        v
    }
}

// ── parse / export (per-line error tolerance; round-trips) ───────────────────

/// Parse a worldscript document. NEVER fails outright: bad lines are collected
/// as errors and skipped, everything else still applies.
pub fn parse(src: &str) -> (WorldScript, Vec<String>) {
    let mut ws = WorldScript::default();
    let mut errors = Vec::new();
    for (ln, raw) in src.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let word = it.next().unwrap_or("");
        let rest: Vec<&str> = it.collect();
        let f = |i: usize| -> Result<f32, String> {
            rest.get(i)
                .ok_or_else(|| format!("line {}: missing arg {}", ln + 1, i + 1))?
                .parse::<f32>()
                .map_err(|_| format!("line {}: bad number '{}'", ln + 1, rest[i]))
        };
        let res: Result<(), String> = (|| {
            match word {
                "name" => ws.name = rest.join(" "),
                "densities" => {
                    ws.densities = Densities { tree: f(0)?, reed: f(1)?, rock: f(2)? }
                }
                "clear" => ws.ops.push(Op::Clear { x: f(0)?, z: f(1)?, r: f(2)? }),
                "bush" => ws.ops.push(Op::Bush { x: f(0)?, z: f(1)?, r: f(2)? }),
                "tree" => ws.ops.push(Op::Tree { x: f(0)?, z: f(1)?, kind: f(2)? as u8 }),
                "rock" => ws.ops.push(Op::Rock { x: f(0)?, z: f(1)?, s: f(2)? }),
                "beacon" => ws.ops.push(Op::Beacon { x: f(0)?, z: f(1)? }),
                "spawn" => ws.ops.push(Op::Spawn { team: f(0)? as u8, x: f(1)?, z: f(2)? }),
                other => return Err(format!("line {}: unknown op '{}'", ln + 1, other)),
            }
            Ok(())
        })();
        if let Err(e) = res {
            errors.push(e);
        }
    }
    (ws, errors)
}

/// Emit a document that `parse()` reconstructs exactly (round-trip).
pub fn export(ws: &WorldScript) -> String {
    let mut out = String::from("# clawbound worldscript v1\n");
    if !ws.name.is_empty() {
        out.push_str(&format!("name {}\n", ws.name));
    }
    if ws.densities != Densities::default() {
        out.push_str(&format!(
            "densities {} {} {}\n",
            ws.densities.tree, ws.densities.reed, ws.densities.rock
        ));
    }
    for op in &ws.ops {
        match *op {
            Op::Clear { x, z, r } => out.push_str(&format!("clear {} {} {}\n", x, z, r)),
            Op::Bush { x, z, r } => out.push_str(&format!("bush {} {} {}\n", x, z, r)),
            Op::Tree { x, z, kind } => out.push_str(&format!("tree {} {} {}\n", x, z, kind)),
            Op::Rock { x, z, s } => out.push_str(&format!("rock {} {} {}\n", x, z, s)),
            Op::Beacon { x, z } => out.push_str(&format!("beacon {} {}\n", x, z)),
            Op::Spawn { team, x, z } => {
                out.push_str(&format!("spawn {} {} {}\n", team, x, z))
            }
        }
    }
    out
}

// ── process-wide active script (OnceLock, like world::GRID) ──────────────────

static ACTIVE: OnceLock<Option<WorldScript>> = OnceLock::new();

/// Install the active worldscript BEFORE the world meshes are built (i.e.
/// before `world::init_world()` / the `build_*` calls). Idempotent: first call
/// wins, matching the bake-once world grid.
pub fn set_active(ws: Option<WorldScript>) {
    let _ = ACTIVE.set(ws);
}

pub fn active() -> Option<&'static WorldScript> {
    ACTIVE.get().and_then(|o| o.as_ref())
}

/// Convenience: load from a file path and install; errors are printed, a bad
/// file simply leaves the stock procedural world.
pub fn load_and_activate(path: &str) -> bool {
    match std::fs::read_to_string(path) {
        Ok(src) => {
            let (ws, errors) = parse(&src);
            for e in &errors {
                eprintln!("[worldscript] {e}");
            }
            let ok = !ws.ops.is_empty() || ws.densities != Densities::default();
            set_active(Some(ws));
            ok
        }
        Err(e) => {
            eprintln!("[worldscript] could not read {path}: {e}");
            set_active(None);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "\
# clawbound worldscript v1
name Foxglove Hollow
densities 1.2 0.8 1.0
clear 0 0 14
bush 10.0 -14.5 2.0
tree 22 8 1
rock -30 12 1.4
beacon 60 -80
spawn 0 -100 -100
spawn 1 100 100
";

    #[test]
    fn parse_round_trips_through_export() {
        let (ws, errs) = parse(DOC);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(ws.name, "Foxglove Hollow");
        assert_eq!(ws.ops.len(), 7);
        let (ws2, errs2) = parse(&export(&ws));
        assert!(errs2.is_empty());
        assert_eq!(ws2.ops, ws.ops);
        assert_eq!(ws2.densities, ws.densities);
        assert_eq!(ws2.name, ws.name);
    }

    #[test]
    fn bad_lines_are_collected_not_fatal() {
        let (ws, errs) = parse("bush 1 2 3\nfrobnicate 9\nbush nope 2 3\nbeacon 4 5\n");
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert_eq!(ws.pinned_bushes(), vec![(1.0, 2.0, 3.0)]);
        assert_eq!(ws.beacon(), Some((4.0, 5.0)));
    }

    #[test]
    fn clear_filters_and_pins_extend() {
        let (ws, _) = parse("clear 0 0 10\nbush 50 50 2\n");
        let procedural = vec![(1.0f32, 1.0f32, 1.5f32), (30.0, 30.0, 1.5)];
        let shaped = ws.shape_positions(procedural, |b| (b.0, b.1), ws.pinned_bushes());
        // the (1,1) bush sat inside the cleared disc; (30,30) survives; pin appended
        assert_eq!(shaped, vec![(30.0, 30.0, 1.5), (50.0, 50.0, 2.0)]);
        assert!(ws.cleared(3.0, 4.0));
        assert!(!ws.cleared(8.0, 8.0)); // r=10: (8,8) is √128 ≈ 11.3 out
    }

    #[test]
    fn last_spawn_and_beacon_win() {
        let (ws, _) = parse("beacon 1 1\nbeacon 2 2\nspawn 0 5 5\nspawn 0 9 9\n");
        assert_eq!(ws.beacon(), Some((2.0, 2.0)));
        assert_eq!(ws.spawn(0), Some((9.0, 9.0)));
        assert_eq!(ws.spawn(1), None);
    }
}
