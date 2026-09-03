//! THE SEAM, GUARDED — which modules have been pulled off the engine, and
//! a test that keeps them off it.
//!
//! `worldhost::WorldHost` is the line between the game and the engine: the
//! game asks questions about the world, the engine answers them, and neither
//! knows the other's type. The trait has existed for a while and the split
//! was drifting anyway, because nothing stopped a ported module from reaching
//! for `crate::world::World` again the next time it needed one more fact. A
//! seam that is only a convention lasts exactly as long as the person who
//! remembers it.
//!
//! So this module holds no logic. It holds the LIST — the modules that are
//! through the seam — and one test per entry that reads the module's own
//! source at compile time and fails if the concrete world got back in.
//!
//! ## Why `include_str!` and not a directory walk
//!
//! The check runs on text embedded at build time, so it cannot be defeated by
//! the working directory, cannot break in a sandbox with no source tree
//! beside the binary, and costs nothing at runtime. A walker would be more
//! general and less trustworthy.
//!
//! ## Adding to the list
//!
//! Port a module, then add it here. If porting needs a fact the trait cannot
//! express, ADD IT TO THE TRAIT rather than keeping the `&World` parameter —
//! that is the whole discipline. Two additions came out of `footing` this way
//! (`gravity_dir`, because "down" is a property of the world; `ground_plane`,
//! because `ground_h` returns an `f32` and so can only say HOW HIGH, never
//! WHETHER) and one out of `camera` (`platform_vel`). All three default to
//! the flat-world answer, so existing hosts stayed correct without changing.
//!
//! ## What is NOT here, and why that is not a failure
//!
//! `hud` draws and `bots` plays this game's specific sport — a HUD reaching
//! into `world.rings` is a frontend touching its own frontend, and bots asking
//! for `route_locate` want skydribble content, not a world. Neither belongs
//! behind the trait.
//!
//! `physics` is the real one, and it is deliberately outside: `worldhost`'s
//! own header says so. It touches the concrete world ~90 times against ~15
//! that the trait covers, and what it wants — `bodies`, `shapes`,
//! `launchers`, `portals` — is not "what is the world like here" but "give me
//! your solids so I can integrate against them". Putting that behind a trait
//! means either a per-contact virtual call in the inner loop or a shape
//! vocabulary both engines must share. That is a design decision with a
//! measurable cost, not a mechanical port, and it should not be started by
//! adding one more method to this trait.

#[cfg(test)]
mod tests {
    /// (module name, its source). Every entry has been ported; the test
    /// below is what keeps it that way.
    const THROUGH_THE_SEAM: &[(&str, &str)] = &[
        ("footing", include_str!("footing.rs")),
        ("camera", include_str!("camera.rs")),
    ];


    /// The shared contract from `worldhost::conformance`, run against the
    /// REAL tile world. The unendlich adapter runs the identical suite in its
    /// own crate, which is the only evidence that swapping the engine under
    /// these modules is safe rather than merely intended.
    #[test]
    fn the_tile_world_satisfies_the_shared_contract() {
        use crate::world::{styles, World};
        use glam::Vec3;
        use worldhost::conformance::{check, Probe};
        use worldhost::WorldHost;

        for name in ["MEADOW", "KUIPER"] {
            let s = styles().iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no {name}"));
            let w = World::generate(0xF007, s, 0.5, 1.0);
            let host: &dyn WorldHost = &w;
            let h = host.ground_h(0.0, 0.0);
            check(
                host,
                &Probe {
                    open: Vec3::new(0.0, h + 60.0, 0.0),
                    buried: Some(Vec3::new(0.0, h - 20.0, 0.0)),
                    column: (0.0, 0.0),
                },
            );
        }
    }

    #[test]
    fn ported_modules_do_not_reach_for_the_concrete_world() {
        for (name, src) in THROUGH_THE_SEAM {
            for (i, line) in src.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                assert!(
                    !code.contains("crate::world::World"),
                    "{name}.rs:{} imports the concrete world again — if it needs a fact \
                     the trait cannot express, add it to WorldHost instead:\n  {}",
                    i + 1,
                    line.trim()
                );
                assert!(
                    !code.contains("&World") && !code.contains("&mut World"),
                    "{name}.rs:{} takes the concrete world again:\n  {}",
                    i + 1,
                    line.trim()
                );
            }
        }
    }

    #[test]
    fn every_ported_module_actually_uses_the_trait() {
        // The inverse mistake: dropping the `&World` parameter by deleting the
        // call rather than by moving it. An entry that no longer mentions the
        // trait at all has left the list silently.
        for (name, src) in THROUGH_THE_SEAM {
            assert!(
                src.contains("WorldHost"),
                "{name}.rs is listed as through the seam but never names WorldHost"
            );
        }
    }

    #[test]
    fn the_trait_still_answers_what_the_ported_modules_ask() {
        // Cheap contract check against the trait's own empty world: the
        // defaults must be the flat-world answers, because that is the promise
        // that let `gravity_dir`, `ground_plane` and `platform_vel` be added
        // without touching any existing host.
        use glam::Vec3;
        use worldhost::{Flat, WorldHost};
        let f = Flat { y: 0.0 };
        assert_eq!(f.gravity_dir(Vec3::new(3.0, 9.0, -2.0)), Vec3::NEG_Y, "default down is not -Y");
        assert!(f.ground_plane(), "a flat world must claim a ground plane");
        assert_eq!(f.platform_vel(Vec3::ZERO, 10.0), None, "a static world must carry nobody");
    }
}
