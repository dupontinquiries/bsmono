//! BOARDING — getting six players in and out of the ship, and which
//! ship it is.
//!
//! Three things that sound separate and are one system, because they
//! all answer "how does a body get from outside the hull to inside
//! it":
//!
//! * **The ramp.** A big stern ramp wide enough for SIX abreast, sized
//!   from the hull it belongs to rather than a fixed number — a lander
//!   and a carrier cannot share a ramp width, and `ship::HULLS` already
//!   spans 28 m to 3000 m. `RampSpec::for_hull` is that sizing, and
//!   `Ramp` is its open/close state, which reuses `airlock::Door`'s own
//!   `Gauge::Launch` timing rather than inventing a second door clock.
//! * **Proximity doors.** Already solved before this module existed:
//!   `Interior::sense_doors`, ticked every frame by `tick_aboard`. This
//!   module only added a change-REPORT on top of it (so a door can
//!   make a sound), and that lives in `interior.rs` beside the layout
//!   it reads — see the note further down.
//! * **Hull selection.** `HullPick` cycles `ship::HULLS`, so `[`/`]` can
//!   swap the spaceship model the way `/` and `=` already swap debug
//!   vehicles. Pure index arithmetic here; actually reloading the model
//!   is the shell's job (see `--boarding`'s own note).
//!
//! All three are pure logic with no renderer and no `world::World`, so
//! every number below is checked by a test rather than by looking at
//! it.

use crate::airlock::{Door, Gauge, Kind};
use crate::ship::{Hull, HULLS};
use glam::Vec3;

/// How many bodies abreast the main ramp must take. The brief's own
/// number, and the reason ramp width is derived rather than authored.
pub const RAMP_ABREAST: usize = 6;

/// (guess) — shoulder-to-shoulder clearance per body, in occupant
/// heights, not metres: `docs/START_HERE.md`'s own convention ("sizes
/// are in occupant-heights") exists because a metre value that didn't
/// move when the thing it measured did caused three bugs in one
/// session. A body is about as wide as half its height, plus elbow
/// room.
pub const LANE_W_H: f32 = 0.62;

/// (guess) — how far the ramp reaches out from the stern, again in
/// occupant heights. Long enough to be a slope rather than a step.
pub const RAMP_RUN_H: f32 = 4.5;

/// (guess) — the steepest a loaded ramp is comfortable to drive up.
/// Real loading ramps sit well under this; 15° is a compromise between
/// "drivable" and "not absurdly long on a 3 km carrier".
pub const RAMP_MAX_GRADE_DEG: f32 = 15.0;

/// A ramp, sized for a specific hull and occupant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RampSpec {
    /// full width, metres — `RAMP_ABREAST` lanes side by side
    pub width_m: f32,
    /// how far it reaches aft of the stern, metres
    pub run_m: f32,
    /// how far the lip sits above the ground when stowed, metres
    pub deck_h_m: f32,
    /// the slope it actually lands at, degrees
    pub grade_deg: f32,
}

impl RampSpec {
    /// Size a ramp for `hull`, given how tall the things using it are.
    ///
    /// Width is `RAMP_ABREAST` lanes and never less — that is the
    /// requirement, not a preference. Run is extended if the deck is
    /// high enough that `RAMP_RUN_H` alone would exceed
    /// `RAMP_MAX_GRADE_DEG`, so a tall hull gets a longer ramp rather
    /// than a steeper one.
    ///
    /// **The deck-height clamp saturates on purpose, and it is visible
    /// in `--boarding`:** every hull from `praetor` (340 m) up gets the
    /// same 11.4 m deck and therefore the same 42.6 m ramp; `warpzone`
    /// at 260 m sits just under the cap and still scales. That is not
    /// the clamp failing to discriminate — it is the statement that a
    /// boarding deck does not keep climbing with hull length. You board
    /// a three-kilometre carrier at a low deck near the waterline, not
    /// at a lip a hundred metres up; the alternative is a ramp longer
    /// than most of the hulls in the table. If a future hull genuinely
    /// wants a high boarding lip, that is a per-hull fact belonging in
    /// `ship::Hull`, not a bigger multiplier here.
    pub fn for_hull(hull: &Hull, occupant_h: f32) -> RampSpec {
        Self::for_hull_len(hull.length, occupant_h)
    }

    /// The same sizing from a raw hull length.
    ///
    /// `aboard::FLEET` and `ship::HULLS` are two different catalogs —
    /// FLEET is geometry (five sizes in occupant-heights, what
    /// `Interior::generate` actually builds) and HULLS is the named
    /// fiction (ten ships with classes and berths). `cycle_ship` walks
    /// FLEET, so it has a length and no `Hull`. Rather than fabricate a
    /// `Hull` to satisfy the signature, the length-based form is the
    /// real one and `for_hull` delegates to it. That split is
    /// pre-existing and not something this module should paper over.
    pub fn for_hull_len(length: f32, occupant_h: f32) -> RampSpec {
        let occupant_h = occupant_h.max(0.01);
        let width_m = RAMP_ABREAST as f32 * LANE_W_H * occupant_h;
        // deck height scales with the hull, since a carrier's belly sits
        // further off the ground than a lander's
        let deck_h_m = (length * 0.04).clamp(occupant_h * 0.35, occupant_h * 6.0);
        let want_run = RAMP_RUN_H * occupant_h;
        // grade = atan(rise / run); lengthen the run until it's under the cap
        let max_grade = RAMP_MAX_GRADE_DEG.to_radians();
        let min_run_for_grade = deck_h_m / max_grade.tan();
        let run_m = want_run.max(min_run_for_grade);
        let grade_deg = (deck_h_m / run_m).atan().to_degrees();
        RampSpec { width_m, run_m, deck_h_m, grade_deg }
    }

    /// Lanes actually available at this width — always at least
    /// `RAMP_ABREAST` by construction, but computed rather than assumed
    /// so a future narrower ramp can't silently claim six.
    pub fn lanes(&self, occupant_h: f32) -> usize {
        (self.width_m / (LANE_W_H * occupant_h.max(0.01))).floor() as usize
    }
}

/// The ramp as a moving part. Wraps an `airlock::Door` at
/// `Gauge::Launch` so the open/close clock, the jam/depower failure
/// modes and `passable()` are the ship's existing ones rather than a
/// second implementation that could disagree with them.
#[derive(Clone, Copy, Debug)]
pub struct Ramp {
    pub spec: RampSpec,
    pub door: Door,
}

impl Ramp {
    pub fn new(spec: RampSpec, stern: Vec3) -> Ramp {
        let half = Vec3::new(spec.width_m * 0.5, spec.deck_h_m * 0.5, spec.run_m * 0.5);
        // `Kind::Launch`, deliberately: it is the one kind that FAILS
        // OPEN (`Kind::fails_open`), and a boarding ramp is exactly the
        // door you want gaping rather than sealed when the power dies —
        // the same reasoning `airlock.rs` already gives for the hangar.
        Ramp { spec, door: Door::new(Kind::Launch, Gauge::Launch, stern, half) }
    }
    pub fn lower(&mut self) -> bool {
        self.door.open()
    }
    pub fn raise(&mut self) -> bool {
        self.door.close()
    }
    pub fn update(&mut self, dt: f32) {
        self.door.update(dt);
    }
    /// Can a body walk or drive up it right now?
    pub fn boardable(&self) -> bool {
        self.door.passable()
    }
}

/// PROXIMITY DOORS LIVE IN `interior.rs`, NOT HERE.
///
/// This module originally grew its own `doors_near`/`door_pass` pair
/// before I checked whether the ship already had one. It does:
/// `Interior::sense_doors` has existed for a while and is called every
/// frame from `main.rs`'s `tick_aboard`, so doors already open when you
/// walk at them. Two functions answering "which doors should be open"
/// is precisely the duplication `--parity` exists to find, so the
/// duplicate is gone rather than kept as a second opinion.
///
/// What the duplicate had that the original lacked was a REPORT of what
/// changed — a shell wanting to play a door's opening sound cannot get
/// that from a function returning `()`. That capability moved to
/// `interior::DoorReport` / `Interior::sense_doors_reporting`, next to
/// the layout it belongs to.

/// WHERE THE RAMP ACTUALLY GOES, against real hull geometry.
///
/// `RampSpec` sizes a ramp; this places one. The stern is station 0 of
/// `hullform::Profile` (`station_z(0) == -half_z`), so `a[0]`/`b[0]` are
/// the real half-width and half-height of the hull's aft face — which
/// is the thing a ramp has to fit on and the thing that decides whether
/// six abreast is actually possible on THIS hull rather than in the
/// abstract.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RampMount {
    /// where the ramp hinges on the hull, in hull-local metres
    pub hinge: Vec3,
    /// where its far lip rests, in hull-local metres
    pub foot: Vec3,
    /// the hull's own half-width at the stern face
    pub stern_half_w: f32,
    /// does the ramp actually fit across that face?
    pub fits: bool,
}

impl RampMount {
    /// Place `spec` on the stern of `prof`, hinging at `deck_y`.
    ///
    /// `fits` is reported rather than enforced: a ramp too wide for its
    /// hull is a real answer about that hull, and silently narrowing it
    /// would break the six-abreast requirement without saying so. The
    /// caller decides — see `fits_every_hull_it_is_sized_for`, which is
    /// where that answer gets checked across the catalog.
    pub fn place(spec: &RampSpec, prof: &crate::hullform::Profile, deck_y: f32) -> RampMount {
        let stern_z = -prof.half_z;
        let stern_half_w = prof.a[0];
        let hinge = Vec3::new(0.0, deck_y, stern_z);
        // the foot reaches aft (further -z) and down to the ground
        let foot = Vec3::new(0.0, deck_y - spec.deck_h_m, stern_z - spec.run_m);
        RampMount { hinge, foot, stern_half_w, fits: spec.width_m <= stern_half_w * 2.0 }
    }

    /// Straight-line length of the ramp surface, hinge to foot — the
    /// hypotenuse, not the run. What a mesh actually needs.
    pub fn slope_len(&self) -> f32 {
        self.hinge.distance(self.foot)
    }
}

/// The ramp as drawable geometry, in hull-local space.
///
/// A flat slab from the hinge on the stern face down to the foot, with
/// a low kerb down each side so it reads as a ramp rather than a
/// floating quad. Built here rather than in `aboard.rs` because the
/// geometry is this module's — `aboard::hull_skin_mesh` merges it in,
/// the same way it already merges the skin and the cells.
///
/// `travel` is `Door::travel` (0 stowed, 1 fully down): the ramp
/// pivots about its hinge, so a half-open ramp is genuinely halfway,
/// not popped into place at 0.75 the way `passable()` gates walking.
pub fn ramp_mesh(
    spec: &RampSpec,
    mount: &RampMount,
    travel: f32,
) -> crate::meshgen::MeshData {
    use crate::meshgen::{MeshData, Vertex};
    let mut out = MeshData::new();
    let t = travel.clamp(0.0, 1.0);
    // stowed, the ramp lies flat against the stern face (foot level with
    // the hinge); down, it reaches its full drop. Interpolating the FOOT
    // rather than rotating a matrix keeps the hinge exactly on the hull
    // at every t, which is the joint a player sees.
    let foot_down = mount.foot;
    let foot_up = Vec3::new(mount.hinge.x, mount.hinge.y, mount.hinge.z - spec.run_m * 0.15);
    let foot = foot_up.lerp(foot_down, t);

    let hw = spec.width_m * 0.5;
    let along = (foot - mount.hinge).normalize_or(Vec3::NEG_Z);
    let side = Vec3::new(1.0, 0.0, 0.0);
    let up = side.cross(along).normalize_or(Vec3::Y);

    let mut tri = |p0: Vec3, p1: Vec3, p2: Vec3| {
        let n = (p1 - p0).cross(p2 - p0).normalize_or(Vec3::Y);
        for p in [p0, p1, p2] {
            out.verts.push(Vertex { pos: p.to_array(), nrm: n.to_array(), uv: [0.0, 0.0] });
        }
    };

    // the deck: two triangles, hinge edge to foot edge
    let h_l = mount.hinge - side * hw;
    let h_r = mount.hinge + side * hw;
    let f_l = foot - side * hw;
    let f_r = foot + side * hw;
    tri(h_l, f_l, f_r);
    tri(h_l, f_r, h_r);
    // and the underside, wound the other way so it is not one-sided
    tri(h_l, f_r, f_l);
    tri(h_l, h_r, f_r);

    // kerbs: a low lip down each edge, so six abreast has something to
    // not walk off. (guess) — a tenth of the ramp width tall.
    let kerb = up * (spec.width_m * 0.1);
    for s in [-1.0f32, 1.0] {
        let a = mount.hinge + side * (hw * s);
        let b = foot + side * (hw * s);
        tri(a, b, b + kerb);
        tri(a, b + kerb, a + kerb);
        tri(a, b + kerb, b);
        tri(a, a + kerb, b + kerb);
    }
    out
}

/// Which ship model is loaded. Index into `ship::HULLS`; cycling wraps
/// both ways, same one-control-both-directions shape as
/// `holomap::Holo::cycle_skin` and `flow::Flow::hub_cycle`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HullPick {
    pub index: usize,
}

impl Default for HullPick {
    fn default() -> HullPick {
        HullPick { index: 0 }
    }
}

impl HullPick {
    pub fn new(index: usize) -> HullPick {
        HullPick { index: index.min(HULLS.len().saturating_sub(1)) }
    }
    pub fn hull(&self) -> &'static Hull {
        &HULLS[self.index.min(HULLS.len() - 1)]
    }
    /// `[` and `]`: previous and next hull. Wraps.
    pub fn cycle(&mut self, d: i32) -> &'static Hull {
        let n = HULLS.len() as i32;
        self.index = ((self.index as i32 + d).rem_euclid(n)) as usize;
        self.hull()
    }
    /// Look one up by id, for a save that persists a hull choice
    /// (`Save::hull` is already a `usize` index into this same table).
    pub fn by_id(id: &str) -> Option<HullPick> {
        HULLS.iter().position(|h| h.id == id).map(|index| HullPick { index })
    }
}

/// `--boarding`: prints every hull's derived ramp, so the six-abreast
/// requirement and the grade cap can be read across the whole catalog
/// rather than checked on one example.
///
/// What this does NOT do: actually swap the loaded model. `HullPick`
/// is the choice; reloading the mesh behind it is shell work in
/// `main.rs` (which already holds the model cache), and doing it here
/// would put asset loading inside a pure-logic module.
pub fn run() {
    let occupant_h = 1.9; // (guess) a bot's standing height, metres
    println!("BOARDING — ramps sized for {RAMP_ABREAST} abreast at occupant_h {occupant_h} m\n");
    println!("{:<10} {:>8} {:>8} {:>8} {:>8} {:>6}", "hull", "len", "width", "run", "deck", "grade");
    for h in HULLS {
        let r = RampSpec::for_hull(h, occupant_h);
        println!(
            "{:<10} {:>8.0} {:>8.2} {:>8.2} {:>8.2} {:>5.1}°  ({} lanes)",
            h.id,
            h.length,
            r.width_m,
            r.run_m,
            r.deck_h_m,
            r.grade_deg,
            r.lanes(occupant_h)
        );
    }
    println!("\nmounted against real fitted hull geometry (interior::generate):");
    println!(
        "  NOTE: this section uses each ship's OWN `Interior::occupant_h`, which is the\n  \
         engine's current body scale, NOT the {occupant_h} m assumed above. They disagree\n  \
         because the player is still a Rocket-League car standing in for a Transformer\n  \
         (`docs/PRIORITIES.md` §2, `--rescale`). Both tables are honest about their own\n  \
         premise; the ramp that ships depends on which body actually wins that decision."
    );
    println!("{:<10} {:>7} {:>9} {:>9} {:>9}  {}", "hull", "occ", "sternW", "slope", "hingeZ", "6-abreast fits?");
    for h in HULLS {
        // `Interior::generate` is the same fit the game uses, so this is
        // the real stern width, not an assumed box.
        let ship = crate::interior::Interior::generate(h.length);
        let r = RampSpec::for_hull(h, ship.occupant_h);
        let m = RampMount::place(&r, ship.profile(), ship.deck_y);
        println!(
            "{:<10} {:>7.2} {:>9.2} {:>9.2} {:>9.1}  {}",
            h.id,
            ship.occupant_h,
            m.stern_half_w * 2.0,
            m.slope_len(),
            m.hinge.z,
            if m.fits { "yes" } else { "NO — stern too narrow" }
        );
    }
    println!(
        "  Every row says yes, and that is WEAKER evidence than it looks: `Interior::\n  \
         generate` derives `occupant_h` FROM hull length (see the `occ` column climb\n  \
         0.17 -> 18.75), so ramp width scales with the hull and a six-abreast ramp can\n  \
         hardly fail to fit. The fit check is close to tautological here. It becomes a\n  \
         real constraint only once occupant height is FIXED by the scale decision above\n  \
         and hulls stop resizing the bodies that walk into them."
    );

    println!("\nhull cycle ([ and ]):");
    let mut pick = HullPick::default();
    for _ in 0..3 {
        let h = pick.cycle(1);
        println!("  ] -> {} ({})", h.name, h.id);
    }
    for _ in 0..2 {
        let h = pick.cycle(-1);
        println!("  [ -> {} ({})", h.name, h.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OCC: f32 = 1.9;

    #[test]
    fn every_hull_takes_six_abreast() {
        for h in HULLS {
            let r = RampSpec::for_hull(h, OCC);
            assert!(
                r.lanes(OCC) >= RAMP_ABREAST,
                "{} only fits {} abreast",
                h.id,
                r.lanes(OCC)
            );
        }
    }

    #[test]
    fn no_hulls_ramp_exceeds_the_grade_cap() {
        for h in HULLS {
            let r = RampSpec::for_hull(h, OCC);
            assert!(
                r.grade_deg <= RAMP_MAX_GRADE_DEG + 1e-3,
                "{} ramp is {:.1}°, over the {RAMP_MAX_GRADE_DEG}° cap",
                h.id,
                r.grade_deg
            );
        }
    }

    #[test]
    fn a_taller_deck_gets_a_longer_ramp_not_a_steeper_one() {
        let lander = RampSpec::for_hull(crate::ship::hull("hawkdrop").unwrap(), OCC);
        let carrier = RampSpec::for_hull(crate::ship::hull("widesky").unwrap(), OCC);
        assert!(carrier.deck_h_m > lander.deck_h_m, "test premise: carrier sits higher");
        assert!(carrier.run_m > lander.run_m, "the taller hull must extend its run");
        assert!(carrier.grade_deg <= RAMP_MAX_GRADE_DEG + 1e-3);
    }

    #[test]
    fn ramp_width_scales_with_the_occupant_not_the_hull() {
        let h = crate::ship::hull("spearhead").unwrap();
        let small = RampSpec::for_hull(h, 1.0);
        let big = RampSpec::for_hull(h, 4.0);
        assert!(big.width_m > small.width_m, "a bigger body needs a wider ramp");
        // and both still take six
        assert!(small.lanes(1.0) >= RAMP_ABREAST);
        assert!(big.lanes(4.0) >= RAMP_ABREAST);
    }

    #[test]
    fn ramp_specs_are_deterministic() {
        for h in HULLS {
            assert_eq!(RampSpec::for_hull(h, OCC), RampSpec::for_hull(h, OCC));
        }
    }

    #[test]
    fn the_deck_clamp_saturates_deliberately_on_the_biggest_hulls() {
        // Found by RUNNING `--boarding`, not by reading the code. First
        // version of this test also got the BOUNDARY wrong -- it claimed
        // warpzone (260 m) was saturated, and the failure showed it is
        // not: 260 * 0.04 = 10.4, under the 11.4 cap. Saturation starts
        // at praetor (340 * 0.04 = 13.6, clamped). Pinned as intent (see
        // `for_hull`'s doc) so it reads as a decision rather than the
        // clamp quietly failing to tell hulls apart, and so making it
        // per-hull later breaks this loudly.
        let saturated = ["praetor", "nemesis", "ark", "widesky"];
        let first = RampSpec::for_hull(crate::ship::hull(saturated[0]).unwrap(), OCC);
        for id in &saturated[1..] {
            let r = RampSpec::for_hull(crate::ship::hull(id).unwrap(), OCC);
            assert_eq!(r, first, "{id} should share the saturated deck clamp");
        }
        // the hull just under the cap must still be distinct, or the
        // clamp is swallowing more than it should
        let under = RampSpec::for_hull(crate::ship::hull("warpzone").unwrap(), OCC);
        assert!(under.deck_h_m < first.deck_h_m, "warpzone sits under the cap and must differ");
        // and the small end must still scale
        let small = RampSpec::for_hull(crate::ship::hull("hawkdrop").unwrap(), OCC);
        assert!(small.deck_h_m < under.deck_h_m, "small hulls must still scale");
    }

    #[test]
    fn a_ramp_is_not_boardable_until_it_has_actually_lowered() {
        let h = crate::ship::hull("spearhead").unwrap();
        let mut r = Ramp::new(RampSpec::for_hull(h, OCC), Vec3::ZERO);
        assert!(!r.boardable(), "a fresh ramp starts stowed");
        r.lower();
        assert!(!r.boardable(), "lowering is not lowered");
        for _ in 0..2000 {
            r.update(0.01);
        }
        assert!(r.boardable(), "a fully lowered ramp must be boardable");
    }

    #[test]
    fn a_raised_ramp_stops_being_boardable() {
        let h = crate::ship::hull("spearhead").unwrap();
        let mut r = Ramp::new(RampSpec::for_hull(h, OCC), Vec3::ZERO);
        r.lower();
        for _ in 0..2000 {
            r.update(0.01);
        }
        assert!(r.boardable());
        r.raise();
        for _ in 0..2000 {
            r.update(0.01);
        }
        assert!(!r.boardable());
    }

    #[test]
    fn the_ramp_admits_everything_a_launch_gauge_does() {
        let h = crate::ship::hull("praetor").unwrap();
        let r = Ramp::new(RampSpec::for_hull(h, OCC), Vec3::ZERO);
        assert!(r.door.admits(Gauge::Personnel));
        assert!(r.door.admits(Gauge::Vehicle));
        assert!(r.door.admits(Gauge::Launch));
    }

    #[test]
    fn the_ramp_mounts_on_the_real_stern_face() {
        // against ACTUAL generated hull geometry, not an assumed box:
        // station 0 of the fitted profile is the stern.
        let ship = crate::interior::Interior::generate(110.0);
        let prof = ship.profile();
        let hull = crate::ship::hull("darkwing").unwrap();
        let spec = RampSpec::for_hull(hull, ship.occupant_h);
        let m = RampMount::place(&spec, prof, ship.deck_y);

        assert!((m.hinge.z - -prof.half_z).abs() < 1e-4, "the hinge must sit on the stern face");
        assert!(m.foot.z < m.hinge.z, "the foot must reach AFT of the hinge");
        assert!(m.foot.y < m.hinge.y, "the foot must reach DOWN from the hinge");
        assert!(m.slope_len() > spec.run_m, "the slope is the hypotenuse, longer than the run");
    }

    #[test]
    fn slope_length_matches_the_run_and_rise_it_was_built_from() {
        let ship = crate::interior::Interior::generate(110.0);
        let hull = crate::ship::hull("darkwing").unwrap();
        let spec = RampSpec::for_hull(hull, ship.occupant_h);
        let m = RampMount::place(&spec, ship.profile(), ship.deck_y);
        let expect = (spec.run_m.powi(2) + spec.deck_h_m.powi(2)).sqrt();
        assert!((m.slope_len() - expect).abs() < 1e-3, "{} vs {expect}", m.slope_len());
    }

    #[test]
    fn ramp_mount_reports_fit_against_the_hulls_own_stern_width() {
        // The interesting case is the SMALL hulls: a six-abreast ramp is
        // a fixed width, and a lander's stern may simply be narrower
        // than it. This test does not demand every hull fits -- it
        // demands the report is HONEST about which do, so a design
        // decision (narrower ramp? fewer abreast on landers? bigger
        // landers?) is made deliberately rather than hidden.
        let mut any_fit = false;
        for len in [28.0f32, 110.0, 340.0] {
            let ship = crate::interior::Interior::generate(len);
            let prof = ship.profile();
            let spec = RampSpec {
                width_m: 7.068,
                run_m: 10.0,
                deck_h_m: 2.0,
                grade_deg: 11.3,
            };
            let m = RampMount::place(&spec, prof, ship.deck_y);
            assert_eq!(
                m.fits,
                spec.width_m <= m.stern_half_w * 2.0,
                "the fit report must agree with the geometry it read"
            );
            any_fit |= m.fits;
        }
        assert!(any_fit, "no hull at any size fits a six-abreast ramp -- that is a real problem");
    }

    #[test]
    fn the_two_occupant_scales_still_disagree_and_that_is_recorded() {
        // The ramp is sized from occupant height, and the codebase has
        // TWO answers for what an occupant is: the ~1.9 m bot the
        // fiction wants, and the engine's actual body (`Interior`'s own
        // `occupant_h`, inherited from the Rocket League car -- see
        // `docs/PRIORITIES.md` §2 and `--rescale`). This is not a bug
        // in `boarding.rs`; it is the open scale decision showing up
        // here too. Pinned so that resolving the scale question breaks
        // this test loudly and someone re-reads the ramp numbers,
        // rather than the two quietly converging or diverging further.
        let ship = crate::interior::Interior::generate(110.0);
        assert!(
            (ship.occupant_h - OCC).abs() > 0.5,
            "the engine occupant ({}) and the assumed bot ({OCC}) have converged -- \
             if the scale decision was made, re-derive the ramp constants and update this test",
            ship.occupant_h
        );
    }

    #[test]
    fn the_ramp_mesh_has_geometry_and_finite_vertices() {
        let ship = crate::interior::Interior::generate(110.0);
        let spec = RampSpec::for_hull_len(110.0, ship.occupant_h);
        let m = RampMount::place(&spec, ship.profile(), ship.deck_y);
        let mesh = ramp_mesh(&spec, &m, 1.0);
        assert!(!mesh.verts.is_empty(), "a lowered ramp must produce triangles");
        assert_eq!(mesh.verts.len() % 3, 0, "triangles come in threes");
        for v in &mesh.verts {
            for c in v.pos {
                assert!(c.is_finite(), "non-finite vertex {:?}", v.pos);
            }
            for c in v.nrm {
                assert!(c.is_finite(), "non-finite normal {:?}", v.nrm);
            }
        }
    }

    #[test]
    fn a_stowed_ramp_is_not_where_a_lowered_one_is() {
        // the travel parameter has to actually move the geometry --
        // otherwise the door animates and the ramp teleports, or worse,
        // never moves at all and the mesh silently lies about the state.
        let ship = crate::interior::Interior::generate(110.0);
        let spec = RampSpec::for_hull_len(110.0, ship.occupant_h);
        let m = RampMount::place(&spec, ship.profile(), ship.deck_y);
        let up = ramp_mesh(&spec, &m, 0.0);
        let down = ramp_mesh(&spec, &m, 1.0);
        assert_eq!(up.verts.len(), down.verts.len(), "same topology at both ends");
        let lowest = |mesh: &crate::meshgen::MeshData| {
            mesh.verts.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min)
        };
        assert!(
            lowest(&down) < lowest(&up) - 1e-3,
            "a lowered ramp must reach further down than a stowed one"
        );
    }

    #[test]
    fn the_ramp_hinge_stays_on_the_hull_at_every_travel() {
        // the hinge is the joint a player looks at; it must not drift as
        // the ramp swings, at any point in the animation.
        let ship = crate::interior::Interior::generate(110.0);
        let spec = RampSpec::for_hull_len(110.0, ship.occupant_h);
        let m = RampMount::place(&spec, ship.profile(), ship.deck_y);
        for step in 0..=10 {
            let t = step as f32 / 10.0;
            let mesh = ramp_mesh(&spec, &m, t);
            // some vertex must still be at the hinge z, within the kerb height
            let near_hinge = mesh
                .verts
                .iter()
                .any(|v| (v.pos[2] - m.hinge.z).abs() < spec.width_m * 0.2);
            assert!(near_hinge, "ramp detached from the hull at travel {t}");
        }
    }

    #[test]
    fn hull_cycle_wraps_both_directions() {
        let mut p = HullPick::new(0);
        let first = p.hull().id;
        p.cycle(-1);
        assert_eq!(p.hull().id, HULLS[HULLS.len() - 1].id, "] from the first wraps to the last");
        p.cycle(1);
        assert_eq!(p.hull().id, first);
    }

    #[test]
    fn hull_cycle_visits_every_hull_exactly_once_per_lap() {
        let mut p = HullPick::new(0);
        let mut seen = Vec::new();
        for _ in 0..HULLS.len() {
            seen.push(p.hull().id);
            p.cycle(1);
        }
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), HULLS.len(), "a lap must show every hull");
        assert_eq!(p.index, 0, "and land back where it started");
    }

    #[test]
    fn hull_by_id_finds_every_hull_and_nothing_else() {
        for h in HULLS {
            assert_eq!(HullPick::by_id(h.id).unwrap().hull().id, h.id);
        }
        assert!(HullPick::by_id("not_a_hull").is_none());
    }

    #[test]
    fn hull_pick_clamps_an_out_of_range_index_rather_than_panicking() {
        let p = HullPick::new(9999);
        assert_eq!(p.hull().id, HULLS[HULLS.len() - 1].id);
    }
}
