//! IS THERE ANYTHING UNDER ME?
//!
//! The engine knows how to do planets-as-ground and does not know how to do
//! space, and the shape of that gap is one function signature:
//!
//! ```text
//!     World::ground_h(x, z) -> f32
//! ```
//!
//! That is a heightmap world model written into a type. It takes two
//! coordinates, so "down" is globally `-Y`; it returns a height rather than an
//! option, so there is ALWAYS ground; and 72 call sites across spawning,
//! camera clamps, bot pathing and physics are built on both assumptions. It is
//! why KUIPER — an asteroid field — has an infinite near-flat plane under it,
//! and why removing that plane is not a one-line change.
//!
//! The odd part is that the engine already knows better everywhere else.
//! `GravNode` carries `down: Vec3` and `inward: bool` ("a world PULLS you down
//! to it"), and `World::gravity_dir(p)` returns the local down at any point.
//! The gravity system was generalised to bodies and the GROUND QUERY never
//! caught up.
//!
//! So this is that query, in the shape space actually needs:
//!
//! ```text
//!     footing::below(world, p, reach) -> Option<Contact>
//! ```
//!
//! Three differences, each carrying one of the assumptions out:
//!
//! * it takes a POSITION, not an (x, z) pair, so there is no global down
//! * it asks `gravity_dir` which way down IS, so a body's surface answers
//!   instead of a plane
//! * it returns an OPTION, so open space is representable — which is the
//!   whole thing the engine could not say before
//!
//! ## What this does not do
//!
//! It does not migrate anything. All 72 callers still use `ground_h` and still
//! work exactly as they did, because each one needs its own answer to "what
//! does `None` mean here" — spawn picks a body, a camera clamp stops clamping,
//! a bot flies instead of walks, physics falls forever. Those are gameplay
//! decisions that want a runtime, and doing them blind is how a floor the
//! renderer hides but physics still collides with gets shipped.
//!
//! ## AND THE FLOOR IS DEEPER THAN `ground_h`
//!
//! Writing this found the rest of the problem. `World::sdf` opens with
//!
//! ```text
//!     let mut d = p.y - grid_h(&self.tgrid, p.x, p.z);
//! ```
//!
//! so the terrain plane is not merely one query's assumption — it is the first
//! term of the distance field that physics, collision and the raymarcher all
//! share, and there is no shapes-only variant to ask instead. On a field world
//! this module can decline to consult `ground_h` and still march straight into
//! the same plane through the SDF.
//!
//! `the_phantom_floor_is_in_the_sdf_itself` pins that down as a characterisation
//! test rather than a wish: it asserts the floor IS found, so that whoever
//! removes it gets a failure telling them this module's field path can finally
//! be trusted. Removing the floor properly means giving the SDF a terrain
//! toggle — which is a change to the one function every system depends on, and
//! wants a runtime.
//!
//! What this does is make the correct query exist and be true, so the
//! migration is mechanical rather than a design problem, and so the next
//! system written reaches for the one that works in space.

use glam::Vec3;

use crate::world::World;

/// A surface found under a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub point: Vec3,
    /// Away from the surface. On a plane this is `+Y`; on a body it points
    /// out from the body, which is the difference the whole module is about.
    pub normal: Vec3,
    /// How far down the surface was, along local down.
    pub dist: f32,
}

/// How close to a surface counts as touching it.
pub const SKIN: f32 = 0.05;
/// Sphere-trace step cap. A march that has not converged by here is looking
/// at something far enough away to be "no footing" for any caller's purpose.
const MAX_STEPS: u32 = 64;

/// What is under `p`, within `reach`, along the local down.
///
/// `None` means open space — nothing within reach in the direction gravity
/// says is down. On a field world that is the common case and the correct
/// answer; on a grounded world it means you are very high up.
pub fn below(world: &World, p: Vec3, reach: f32) -> Option<Contact> {
    let down = world.gravity_dir(p).try_normalize().unwrap_or(Vec3::NEG_Y);

    // A FIELD WORLD HAS NO PLANE. `World::kuiper` already records this — the
    // asteroid field's terrain grid exists only because every map gets one,
    // and consulting it here would reproduce the phantom floor in the very
    // query written to see past it.
    if !world.kuiper {
        // Grounded world: the heightmap is the answer and down is -Y, which
        // is exactly the case `ground_h` was written for. Use it rather than
        // marching, because it is exact and the march is not.
        let h = world.ground_h(p.x, p.z);
        let dist = p.y - h;
        if dist < -SKIN || dist > reach {
            return None;
        }
        return Some(Contact { point: Vec3::new(p.x, h, p.z), normal: Vec3::Y, dist: dist.max(0.0) });
    }

    // Field world: sphere-trace the SDF along local down. The SDF already
    // contains the bodies, so this finds whichever one gravity is pointing at
    // and nothing when gravity is pointing at nothing.
    let mut t = 0.0f32;
    for _ in 0..MAX_STEPS {
        let at = p + down * t;
        let d = world.sdf(at);
        if !d.is_finite() {
            return None;
        }
        if d < SKIN {
            // the outward normal is the SDF gradient, which for a body is
            // "away from its centre" without needing to know which body
            let n = sdf_normal(world, at).unwrap_or(-down);
            return Some(Contact { point: at, normal: n, dist: t });
        }
        t += d.max(SKIN);
        if t > reach {
            return None;
        }
    }
    None
}

/// Is `p` standing on something?
pub fn grounded(world: &World, p: Vec3, skin: f32) -> bool {
    below(world, p, skin.max(SKIN)).is_some()
}

fn sdf_normal(world: &World, p: Vec3) -> Option<Vec3> {
    let e = 0.08;
    let n = Vec3::new(
        world.sdf(p + Vec3::X * e) - world.sdf(p - Vec3::X * e),
        world.sdf(p + Vec3::Y * e) - world.sdf(p - Vec3::Y * e),
        world.sdf(p + Vec3::Z * e) - world.sdf(p - Vec3::Z * e),
    );
    n.try_normalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{styles, World};

    fn world_named(name: &str) -> World {
        let s = styles().iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no {name}"));
        World::generate(0xF007, s, 0.5, 1.0)
    }

    #[test]
    fn a_grounded_world_has_ground_under_you() {
        let w = world_named("MEADOW");
        let h = w.ground_h(0.0, 0.0);
        let c = below(&w, Vec3::new(0.0, h + 5.0, 0.0), 50.0).expect("MEADOW has ground");
        assert!((c.dist - 5.0).abs() < 0.01, "found the ground {:.2} down, not 5", c.dist);
        assert!(c.normal.dot(Vec3::Y) > 0.9, "a plane's normal is not up: {:?}", c.normal);
    }

    #[test]
    fn the_phantom_floor_is_in_the_sdf_itself() {
        // CHARACTERISATION, not approval. KUIPER is an asteroid field and has
        // an infinite near-flat plane under it, and skipping `ground_h` is not
        // enough to see past it: `World::sdf`'s first term IS that plane, so a
        // march down through open field hits it with a +Y normal.
        //
        // Asserted so that removing the floor FAILS here and tells whoever did
        // it that this module's field path can now be trusted. A test that
        // quietly tolerated the floor would leave the bug invisible again.
        let w = world_named("KUIPER");
        let high = Vec3::new(0.0, 300.0, 0.0);
        let c = below(&w, high, 2000.0)
            .expect("if this is now None the floor is gone — update this test, the bug is fixed");
        assert!(
            c.normal.dot(Vec3::Y) > 0.8,
            "found something in the field that is not the flat floor: {:?}",
            c.normal
        );
    }

    #[test]
    fn open_space_is_representable_even_though_the_floor_is_still_there() {
        // The type can say "nothing", which is the sentence `ground_h` has no
        // way to form. Bounded by reach on purpose: this asserts the QUERY can
        // decline, not that KUIPER is currently floorless — it is not, per the
        // test above, and conflating the two is how the first version of this
        // passed for the wrong reason.
        let w = world_named("KUIPER");
        let high = Vec3::new(0.0, 4000.0, 0.0);
        assert!(below(&w, high, 200.0).is_none(), "a 200 m reach found the floor 4 km down");
        assert!(!grounded(&w, high, 1.0), "a point in vacuum reported as grounded");
        // and the old query cannot decline at all — it answers with a number
        let _always_a_number: f32 = w.ground_h(high.x, high.z);
    }

    #[test]
    fn reach_bounds_the_answer() {
        // A caller asking "is there ground within a metre" must not be told
        // about something forty metres down. This is what lets a physics step
        // use the same query as a camera clamp.
        let w = world_named("MEADOW");
        let h = w.ground_h(0.0, 0.0);
        let high = Vec3::new(0.0, h + 40.0, 0.0);
        assert!(below(&w, high, 1.0).is_none(), "a 1 m reach found ground 40 m down");
        assert!(below(&w, high, 60.0).is_some(), "a 60 m reach missed ground 40 m down");
    }

    #[test]
    fn being_under_the_surface_is_not_footing() {
        // Below the ground there is nothing to stand ON, and a query that
        // said otherwise would let a buried spawn report as safe.
        let w = world_named("MEADOW");
        let h = w.ground_h(0.0, 0.0);
        assert!(
            below(&w, Vec3::new(0.0, h - 5.0, 0.0), 50.0).is_none(),
            "found footing from five metres underground"
        );
    }
}
