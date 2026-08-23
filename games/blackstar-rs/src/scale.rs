//! SCALE — how big things actually are, and where the camera goes.
//!
//! The engine is already in metres. `DESIGN.md` records the conversion from
//! Rocket League's units — gravity 6.5 m/s², ball radius 0.9325 m — and
//! `physics.rs` seats a body at `half_h: 0.19`, so the playable object is
//! thirty-eight centimetres tall. That is a car, because this began as a car
//! game, and the fiction says it is a Transformer.
//!
//! ## The problem is not the camera. It is that size is not data.
//!
//! `model_tuning.json` carries a per-model `scale`, and Optimus Prime's is
//! **0.4989**. Across 1,012 models the medians by kind land between 0.5 and
//! 1.8 — robots at 0.904, vehicles at 0.530 — which is to say every model,
//! whatever it depicts, is normalised into the same box. A Transformer and a
//! Camaro are the same height in-game.
//!
//! So the camera cannot adjust to size, because nothing knows its size.
//! There is one number per model and it means "fit this in the playable box",
//! not "this is how tall it is". This module is the missing quantity.
//!
//! ## Heights
//!
//! Real where a real figure exists, and cited. Two California Plaza is in the
//! model catalogue (`imported_two_california_plaza_los_angeles`) and is
//! 228 m over 52 floors above ground, which makes it the useful yardstick:
//! it is the tallest thing the LA campaign can stand next to, and the ratio
//! it sets against a person is the range the renderer has to survive.
//!
//! ## The camera does not scale linearly, and the RL number is the outlier
//!
//! The existing chase camera sits 4.9 m behind a 0.38 m body — nearly THIRTEEN
//! times its height. That is not a general rule about cameras; it is a
//! specific fact about a game where the ball matters as much as the car. A
//! human third-person camera is nearer two or three times body height, and a
//! mech wants to fill more of the frame than either.
//!
//! Scaling everything off the RL ratio therefore puts a human nine metres
//! back, which is wrong. So framing is PER CLASS, and size varies within a
//! class by `h^0.85` — sublinear, because a body twice as tall does not want
//! the camera exactly twice as far: some of the extra height fills frame it
//! is fine to fill. The RL ratio is quarantined inside `Class::Vehicle`,
//! which is the only place it was ever true, and the existing feel is
//! therefore untouched.

/// The playable body the whole physics tune assumes: `physics.rs` half_h 0.19.
pub const RL_BODY_H_M: f32 = 0.38;
/// The chase camera that goes with it (`config.rs` defaults).
pub const RL_CAM_DIST_M: f32 = 4.9;

/// How far the camera pulls back as a body gets taller, as an exponent.
///
/// 1.0 keeps apparent size exactly constant. Below 1.0 a bigger body fills
/// more of the frame, which is how something reads as big rather than merely
/// distant. A DIAL — the exponent is a look, not a fact.
pub const SIZE_EXPONENT: f32 = 0.85;

/// What kind of thing is being framed. Framing is per class because the
/// right distance is a genre convention, not a function of height.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Human,
    Vehicle,
    Bot,
    Titan,
    /// The 0.38 m object every physics constant is tuned around.
    ///
    /// Its own class, because it is not a car. A Rocket League Octane is
    /// 118 uu long, which this engine's conversion reads as 1.18 m — a
    /// quarter the length of a real sedan. The units are RL's divided by a
    /// hundred and called metres, and objects inside them are NOT at real
    /// scale relative to one another.
    ///
    /// That is the finding underneath the camera problem, and it is bigger:
    /// drop a 1.75 m person into this world and they are one and a half
    /// times the LENGTH of a car. Giving this body its own class keeps the
    /// legacy tuning intact and stops it being mistaken for a statement
    /// about vehicles.
    EngineBody,
}

impl Class {
    /// Chase distance at one metre of body height, before the exponent.
    ///
    /// `Vehicle` is calibrated to reproduce the existing Rocket League camera
    /// exactly at `RL_BODY_H_M`, so nothing about the current game moves.
    pub fn dist_base(self) -> f32 {
        match self {
            // ~2.2x body: over the shoulder, the room readable around them
            Class::Human => 2.2,
            // a real car, framed the way a driving game frames one
            Class::Vehicle => 3.0,
            // the RL ratio, and ONLY here
            Class::EngineBody => RL_CAM_DIST_M / RL_BODY_H_M.powf(SIZE_EXPONENT),
            // closer in proportion than a human camera: a mech should crowd
            // the frame, because that is what makes it read as heavy
            Class::Bot => 2.6,
            // further in proportion, because the interesting thing about
            // something this size is what it is standing next to
            Class::Titan => 3.4,
        }
    }

    /// Camera height above the body's foot, as a multiple of body height.
    pub fn height_mult(self) -> f32 {
        match self {
            Class::Human => 1.1,
            Class::Vehicle => 0.9,
            Class::EngineBody => 3.2, // RL again: 1.21 above a 0.38 body
            // BELOW the eyeline on purpose. Looking slightly up at a thing is
            // the cheapest way to make it enormous, and the reason a bot
            // filmed from its own head height looks like a man in a suit.
            Class::Bot => 0.55,
            Class::Titan => 0.35,
        }
    }
}

/// A thing with a size.
pub struct Sized {
    pub id: &'static str,
    pub class: Class,
    /// metres, tip to floor
    pub height_m: f32,
    pub note: &'static str,
}

/// The ladder. Real figures where a real figure exists.
pub static LADDER: &[Sized] = &[
    Sized {
        id: "human",
        class: Class::Human,
        height_m: 1.75,
        note: "the trio, and everyone in the archive era",
    },
    Sized {
        id: "car",
        class: Class::Vehicle,
        height_m: 1.45,
        note: "a 1990s sedan; the car the team stages out of",
    },
    Sized {
        id: "rl_body",
        class: Class::EngineBody,
        height_m: RL_BODY_H_M,
        note: "the body every physics constant in the engine is tuned around",
    },
    Sized {
        id: "scout_bot",
        class: Class::Bot,
        height_m: 5.0,
        note: "the small end: a bot that can stand in a warehouse",
    },
    Sized {
        id: "bot",
        class: Class::Bot,
        height_m: 8.5,
        note: "the standard frame; roughly a two-storey building",
    },
    Sized {
        id: "heavy_bot",
        class: Class::Bot,
        height_m: 14.0,
        note: "combiner limb, siege frame; taller than the buildings it fights in",
    },
    Sized {
        id: "brutalist_block",
        class: Class::Titan,
        height_m: 24.0,
        note: "the low-rise the catalogue has several of; six or seven storeys",
    },
    Sized {
        id: "nemesis",
        class: Class::Titan,
        height_m: 480.0,
        note: "a frigate; twice the tower and it moves",
    },
    Sized {
        id: "widesky",
        class: Class::Titan,
        height_m: 3000.0,
        note: "real: a 3 km carrier from ship::HULLS. the worst case in the game.",
    },
    Sized {
        id: "two_california_plaza",
        class: Class::Titan,
        height_m: 228.0,
        note: "real: 228 m, 52 floors above ground, Bunker Hill. in the catalogue.",
    },
];

pub fn sized(id: &str) -> Option<&'static Sized> {
    LADDER.iter().find(|s| s.id == id)
}

/// The near plane, as a fraction of the chase distance.
///
/// 1/50 keeps the human case at almost exactly the shipped 0.08 m, so nothing
/// about close work changes, while a tower shot 343 m back gets a near plane
/// near seven metres — which costs nothing, because at that distance there is
/// nothing within seven metres of the eye worth drawing.
pub const NEAR_FRACTION: f32 = 1.0 / 50.0;

/// Absolute floor, so a very small subject cannot drive the near plane to
/// zero and take the whole depth buffer with it.
pub const NEAR_FLOOR_M: f32 = 0.05;

/// The near plane for a shot at this distance.
///
/// This is the third lever on the depth problem and the only free one. The
/// engine ships a FIXED 0.08 m near plane (`camera.rs`), which is correct for
/// a body 38 cm tall and absurd for a camera 343 m from a skyscraper: the
/// perspective divide spends most of the depth buffer between 8 cm and a
/// metre, and there is nothing there. See `depth.rs` — at 343 m, moving the
/// near plane from 0.08 m to 1 m takes standard-Z resolution from 82 mm to
/// 5 mm, with no pipeline change at all.
pub fn near_for(distance_m: f32) -> f32 {
    (distance_m * NEAR_FRACTION).max(NEAR_FLOOR_M)
}

/// Where the chase camera goes for a body of this class and height.
/// Returns `(distance_m, height_above_foot_m)`.
pub fn camera_for(class: Class, height_m: f32) -> (f32, f32) {
    let h = height_m.max(0.05);
    (class.dist_base() * h.powf(SIZE_EXPONENT), class.height_mult() * h)
}

/// How much bigger `a` is than `b`. The number the renderer has to survive.
pub fn ratio(a: &Sized, b: &Sized) -> f32 {
    a.height_m / b.height_m
}

/// What a model's `model_tuning.json` scale SHOULD be, given how tall the
/// thing it depicts actually is.
///
/// The existing values normalise every model into the playable box; this is
/// the conversion to a world where a bot and a person can stand in the same
/// shot. Kept as a function rather than a rewritten table because the
/// existing scales are still correct for the current game, and changing them
/// is a decision about when to move, not a bug to fix.
pub fn true_scale(current_scale: f32, depicted_h_m: f32) -> f32 {
    current_scale * (depicted_h_m / RL_BODY_H_M)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn the_existing_camera_is_reproduced_exactly() {
        // the calibration that makes this safe to adopt: nothing about the
        // current game moves. If this fails, the change is not additive.
        let (d, h) = camera_for(Class::EngineBody, RL_BODY_H_M);
        assert!(close(d, RL_CAM_DIST_M, 0.01), "dist {d} should be {RL_CAM_DIST_M}");
        assert!(close(h, 1.21, 0.02), "height {h} should be 1.21");
    }

    #[test]
    fn the_rl_ratio_does_not_escape_its_class() {
        // scaling everything off RL's thirteen-times-body chase camera puts a
        // person nine metres back, which is the bug this class split exists
        // to prevent
        let (human_d, _) = camera_for(Class::Human, 1.75);
        assert!(human_d > 2.0 && human_d < 5.0, "a human camera at {human_d} m is wrong");

        let rl_ratio = RL_CAM_DIST_M / RL_BODY_H_M;
        assert!(rl_ratio > 12.0, "sanity: RL really is that far back");
        assert!(human_d / 1.75 < 4.0, "the RL ratio leaked into the human class");
    }

    #[test]
    fn a_bot_is_framed_closer_in_proportion_than_a_person() {
        // what makes something read as heavy: it crowds the frame, and the
        // camera is below its eyeline
        let (bd, bh) = camera_for(Class::Bot, 8.5);
        let (hd, hh) = camera_for(Class::Human, 1.75);
        assert!(bd / 8.5 < hd / 1.75, "the bot camera is proportionally further out");
        assert!(bh / 8.5 < hh / 1.75, "the bot camera is not below the eyeline");
        // and in absolute terms it is still much further away
        assert!(bd > hd);
    }

    #[test]
    fn distance_grows_with_size_but_not_as_fast() {
        // sublinear: twice as tall is less than twice as far, or a big thing
        // never fills the frame
        let (small, _) = camera_for(Class::Bot, 5.0);
        let (big, _) = camera_for(Class::Bot, 10.0);
        assert!(big > small, "a bigger body needs more room");
        assert!(big < small * 2.0, "distance scaled linearly; nothing will look big");
        // monotonic across the whole ladder
        let mut prev = 0.0;
        for h in [1.0f32, 2.0, 5.0, 10.0, 50.0, 228.0] {
            let (d, _) = camera_for(Class::Bot, h);
            assert!(d > prev, "not monotonic at {h}");
            prev = d;
        }
    }

    #[test]
    fn the_camera_is_never_inside_the_body() {
        // the failure that made this necessary: a 4.9 m chase distance around
        // an 8.5 m bot puts the eye inside its chest
        for s in LADDER {
            let (d, _) = camera_for(s.class, s.height_m);
            assert!(
                d > s.height_m * 0.4,
                "{} is {} m tall and the camera is {d} m back — inside it",
                s.id,
                s.height_m
            );
        }
        // and the number that motivated it, stated plainly: the existing
        // fixed chase distance is NEARER THAN A BOT IS TALL, so pointing it
        // at an 8.5 m frame puts the eye somewhere around its knees with the
        // head out of shot
        let bot_h = sized("bot").unwrap().height_m;
        assert!(
            RL_CAM_DIST_M < bot_h,
            "the fixed {RL_CAM_DIST_M} m camera would be fine for a {bot_h} m bot?"
        );
        let (fixed_for_bot, _) = (RL_CAM_DIST_M, 0.0);
        let (proper, _) = camera_for(Class::Bot, bot_h);
        assert!(proper > fixed_for_bot * 3.0, "the correction should be large");
    }

    #[test]
    fn the_biggest_hull_does_not_fit_the_frustum_and_the_test_says_so() {
        // I ended the scale work on "a person and Two California Plaza is a
        // 130:1 span and a 343 m pullback". `ship::HULLS` has a THREE
        // KILOMETRE carrier in it, which is thirteen times the tower, and
        // framing it puts the camera 3,069 m back against a far plane of
        // 2,600. The ship would be entirely behind it.
        //
        // Not a bug in the camera and not a bug in the hull table: they were
        // written apart and never asked about each other. Stated as a test
        // rather than fixed here, because the fix is a decision — move the
        // far plane, frame big hulls differently, or accept that a carrier is
        // never shown whole — and it wants somebody who can look at all three.
        let widesky = sized("widesky").unwrap();
        let (d, _) = camera_for(widesky.class, widesky.height_m);
        assert!(
            d > crate::camera::Camera::FAR_M,
            "the carrier now fits at {d} m; if the far plane moved, strike this"
        );

        // everything smaller does fit, so the boundary is real rather than
        // the whole class being broken
        for id in ["nemesis", "two_california_plaza", "brutalist_block"] {
            let s = sized(id).unwrap();
            let (d, _) = camera_for(s.class, s.height_m);
            assert!(d < crate::camera::Camera::FAR_M, "{id} at {d} m is past the far plane too");
        }
    }

    #[test]
    fn every_hull_in_the_ship_table_has_a_framing() {
        // the ladder and `ship::HULLS` are separate tables describing the
        // same objects; this is the join nobody had made
        for h in crate::ship::HULLS {
            let (d, up) = camera_for(Class::Titan, h.length);
            assert!(d > h.length * 0.4, "{} is framed inside itself", h.id);
            assert!(up > 0.0);
        }
    }

    #[test]
    fn the_ladder_spans_the_range_the_renderer_has_to_survive() {
        let human = sized("human").unwrap();
        let bot = sized("bot").unwrap();
        let tower = sized("two_california_plaza").unwrap();

        // a bot is about five people tall
        assert!(close(ratio(bot, human), 4.86, 0.2), "{}", ratio(bot, human));
        // and the tower is 130 people
        assert!(ratio(tower, human) > 125.0 && ratio(tower, human) < 135.0);
        // the depth range in one shot, which is what actually breaks renderers
        assert!(ratio(tower, human) > 100.0, "the near/far span is the hard part");

        for s in LADDER {
            assert!(s.height_m > 0.0 && !s.note.is_empty());
        }
        let ids: std::collections::BTreeSet<_> = LADDER.iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), LADDER.len(), "duplicate ladder id");
    }

    #[test]
    fn the_near_plane_follows_the_shot_and_leaves_close_work_alone() {
        // the human case must not move: 3.5 m back is the shot the shipped
        // 0.08 m near plane was chosen for
        let (hd, _) = camera_for(Class::Human, 1.75);
        assert!(
            close(near_for(hd), 0.08, 0.02),
            "the human near plane moved to {}",
            near_for(hd)
        );
        // and a tower shot gets metres of near plane, which is the point
        let (td, _) = camera_for(Class::Titan, 228.0);
        assert!(near_for(td) > 5.0, "tower near plane {} m is still tiny", near_for(td));
        // monotone, and floored
        assert_eq!(near_for(0.0), NEAR_FLOOR_M);
        let mut prev = 0.0;
        for d in [1.0f32, 4.0, 16.0, 100.0, 343.0] {
            let n = near_for(d);
            assert!(n >= prev, "not monotone at {d}");
            prev = n;
        }
    }

    #[test]
    fn wiring_it_into_the_camera_does_not_disturb_the_current_game() {
        // `camera.rs` now derives its near plane from the shot distance
        // instead of hardcoding 0.08 m. That is a live change to the
        // renderer, which is the one thing here I cannot look at, so the
        // safety argument has to be arithmetic.
        //
        // The gameplay camera sits 4.9 m back. The near plane moves 0.08 ->
        // 0.098 m, and nothing in a chase camera is within ten centimetres of
        // the eye, so nothing new clips.
        let shipped = near_for(RL_CAM_DIST_M);
        assert!(
            shipped > 0.05 && shipped < 0.12,
            "the gameplay near plane moved to {shipped} m, which is not a nudge"
        );
        // and it never goes the wrong way: closer shots get a closer near
        // plane, so anything that fitted before still fits
        for d in [0.5f32, 1.0, 2.0, RL_CAM_DIST_M] {
            assert!(near_for(d) < d * 0.5, "near plane at {d} m eats the subject");
        }
        // the far plane is shared with the analysis rather than repeated
        assert_eq!(crate::camera::Camera::FAR_M as f64, crate::depth::FAR_M);
    }

    #[test]
    fn a_size_aware_near_plane_fixes_most_of_the_depth_problem_for_free() {
        // the claim, checked against `depth.rs` rather than asserted here.
        // This is the lever that needs no pipeline change, and it is worth
        // knowing how far it gets on its own before arguing for reversed Z.
        use crate::depth;
        let (td, _) = camera_for(Class::Titan, 228.0);
        let shipped = depth::resolution_m(td as f64, depth::NEAR_M, depth::FAR_M, false);
        let scaled = depth::resolution_m(td as f64, near_for(td) as f64, depth::FAR_M, false);
        assert!(
            scaled < shipped / 10.0,
            "a scaled near plane should be an order of magnitude better: {scaled} vs {shipped}"
        );
        // ... and reversed Z alone beats it.
        let reversed_only = depth::resolution_m(td as f64, depth::NEAR_M, depth::FAR_M, true);
        assert!(reversed_only < scaled, "reversed Z should still win outright");

        // THE TWO LEVERS ARE ALTERNATIVES, NOT ADDITIVE, and I assumed
        // otherwise until the report printed all four rows side by side.
        // Reversed Z gets its precision from float32 being dense near zero,
        // which is where it puts the FAR plane; pushing the near plane out on
        // top of that redistributes the same budget and gives some back. So
        // with reversed Z you keep the small near plane and keep the close
        // range for free.
        let both = depth::resolution_m(td as f64, near_for(td) as f64, depth::FAR_M, true);
        assert!(
            both > reversed_only,
            "if these composed, the recommendation would be different: \
             both {both} vs reversed-only {reversed_only}"
        );
    }

    #[test]
    fn the_engine_body_is_not_a_car_and_is_not_filed_as_one() {
        // the deeper finding: RL's units are its own divided by a hundred, so
        // objects are not at real scale RELATIVE TO EACH OTHER. The tuning
        // body is 0.38 m where a real car is 1.45 — nearly four times out —
        // and a 1.75 m person dropped into that world is one and a half times
        // the LENGTH of a car.
        let body = sized("rl_body").unwrap();
        let car = sized("car").unwrap();
        assert_eq!(body.class, Class::EngineBody, "the tuning body is not a vehicle");
        assert!(car.height_m / body.height_m > 3.0, "they should be far apart");

        // and a real car gets a real driving camera, not RL's
        let (car_d, _) = camera_for(Class::Vehicle, car.height_m);
        let (body_d, _) = camera_for(Class::EngineBody, body.height_m);
        assert!(car_d < 6.0, "a car chase camera at {car_d} m is a helicopter");
        assert!(close(body_d, RL_CAM_DIST_M, 0.01), "the legacy body must not move");
    }

    #[test]
    fn true_scale_undoes_the_normalisation() {
        // Optimus Prime ships at 0.4989, which fits him in the playable box.
        // At his real height he should be an order of magnitude larger.
        let real = true_scale(0.4989, 8.5);
        assert!(real > 10.0, "true scale {real} is not bot-sized");
        // and the identity: a model that really is body-sized does not move
        assert!(close(true_scale(1.0, RL_BODY_H_M), 1.0, 1e-6));
    }
}
