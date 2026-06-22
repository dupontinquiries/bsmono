//! `sim::title_cinematic` — the pure logic behind the title "splash": a slow
//! Prezi-style camera that pans through a small, frozen scene, the menu pieces
//! that fly in to their own thematic anchors, and the hover-fade timing for the
//! transparent UI. The renderer owns the GPU/egui side; it just samples this
//! module each frame for (a) where the camera is, (b) where each menu piece sits
//! and how opaque it is, and (c) per-widget hover alpha. Keeping it here means
//! the *motion design* is deterministic and testable; only the drawing is
//! compile-on-end. Pure: no IO/render/RNG/clock (the bin passes elapsed time).

/// A camera keyframe: eye position, the point it looks at, and vertical FOV.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPose {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    pub fov_deg: f32,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}
/// Smootherstep (Ken Perlin) — zero 1st & 2nd derivative at the ends, so the
/// camera eases in and out of each keyframe without a velocity jolt.
fn smoother(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

impl CameraPose {
    pub fn lerp(self, other: CameraPose, t: f32) -> CameraPose {
        CameraPose {
            eye: lerp3(self.eye, other.eye, t),
            target: lerp3(self.target, other.target, t),
            fov_deg: lerp(self.fov_deg, other.fov_deg, t),
        }
    }
}

/// A looping camera path through the frozen title scene.
#[derive(Clone, Debug)]
pub struct CinematicPath {
    keys: Vec<CameraPose>,
    /// Seconds spent travelling between each adjacent pair of keyframes.
    seg_secs: f32,
}

impl CinematicPath {
    pub fn new(keys: Vec<CameraPose>, seg_secs: f32) -> CinematicPath {
        CinematicPath { keys, seg_secs: seg_secs.max(0.01) }
    }

    /// The default intro orbit: a slow, gently-banking sweep around a small
    /// scene centred on the origin, dipping in for a hero angle then pulling
    /// back. Interesting, non-axis-aligned angles (not a flat turntable).
    pub fn intro() -> CinematicPath {
        CinematicPath::new(
            vec![
                CameraPose { eye: [14.0, 9.0, 16.0], target: [0.0, 3.5, 0.0], fov_deg: 50.0 },
                CameraPose { eye: [-6.0, 6.5, 18.0], target: [1.0, 3.0, 0.0], fov_deg: 46.0 },
                CameraPose { eye: [-17.0, 11.0, -3.0], target: [0.0, 4.0, 1.0], fov_deg: 52.0 },
                CameraPose { eye: [-2.0, 5.0, -16.0], target: [0.0, 3.0, 0.0], fov_deg: 44.0 },
                CameraPose { eye: [13.0, 13.0, -9.0], target: [0.5, 4.5, 0.0], fov_deg: 54.0 },
            ],
            7.5,
        )
    }

    pub fn total_secs(&self) -> f32 {
        // looping: N keyframes → N segments (last wraps to first)
        self.seg_secs * self.keys.len().max(1) as f32
    }

    /// Sample the eased, looping camera pose at `elapsed_secs`.
    pub fn sample(&self, elapsed_secs: f32) -> CameraPose {
        let n = self.keys.len();
        if n == 0 {
            return CameraPose { eye: [0.0, 5.0, 12.0], target: [0.0; 3], fov_deg: 50.0 };
        }
        if n == 1 {
            return self.keys[0];
        }
        let loop_t = elapsed_secs.rem_euclid(self.total_secs());
        let seg = (loop_t / self.seg_secs) as usize % n;
        let local = (loop_t - seg as f32 * self.seg_secs) / self.seg_secs;
        let a = self.keys[seg];
        let b = self.keys[(seg + 1) % n];
        a.lerp(b, smoother(local))
    }
}

/// The menu pieces, each with its own resting anchor and theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuPiece {
    Singleplayer,
    Multiplayer,
    Settings,
    Quit,
}

impl MenuPiece {
    pub const ALL: [MenuPiece; 4] =
        [MenuPiece::Singleplayer, MenuPiece::Multiplayer, MenuPiece::Settings, MenuPiece::Quit];

    /// The on-screen anchor (normalised 0..1 in the viewport) the piece flies to
    /// and rests at — each a deliberate, asymmetric "custom location" rather than
    /// a centred stack.
    pub fn anchor01(self) -> [f32; 2] {
        match self {
            MenuPiece::Singleplayer => [0.18, 0.34],
            MenuPiece::Multiplayer => [0.30, 0.52],
            MenuPiece::Settings => [0.20, 0.70],
            MenuPiece::Quit => [0.12, 0.86],
        }
    }

    /// Where the piece starts before flying in (off the edge it drifts from),
    /// also normalised; the renderer eases from here to `anchor01`.
    pub fn fly_from01(self) -> [f32; 2] {
        match self {
            MenuPiece::Singleplayer => [-0.25, 0.30],
            MenuPiece::Multiplayer => [-0.35, 0.52],
            MenuPiece::Settings => [-0.30, 0.74],
            MenuPiece::Quit => [-0.20, 0.92],
        }
    }

    /// Seconds after the splash begins before this piece starts flying in —
    /// a staggered cascade so they arrive one after another.
    pub fn fly_delay_secs(self) -> f32 {
        match self {
            MenuPiece::Singleplayer => 0.35,
            MenuPiece::Multiplayer => 0.60,
            MenuPiece::Settings => 0.85,
            MenuPiece::Quit => 1.10,
        }
    }

    /// A one-line theme hint the renderer can use for tint/icon flavour.
    pub fn theme(self) -> &'static str {
        match self {
            MenuPiece::Singleplayer => "ember/forge — warm gold",
            MenuPiece::Multiplayer => "sky/air — pale cyan",
            MenuPiece::Settings => "stone/earth — slate grey",
            MenuPiece::Quit => "dusk/ash — muted violet",
        }
    }
}

/// The animated state of one menu piece this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PieceState {
    pub pos01: [f32; 2],
    pub alpha01: f32,
}

/// Compute a piece's current screen position + opacity. `fly_secs` is how long
/// the fly-in takes once it starts.
pub fn piece_state(piece: MenuPiece, elapsed_secs: f32, fly_secs: f32) -> PieceState {
    let fly_secs = fly_secs.max(0.01);
    let t = ((elapsed_secs - piece.fly_delay_secs()) / fly_secs).clamp(0.0, 1.0);
    let e = smoother(t);
    let from = piece.fly_from01();
    let to = piece.anchor01();
    PieceState {
        pos01: [lerp(from[0], to[0], e), lerp(from[1], to[1], e)],
        alpha01: e, // fades in as it flies
    }
}

/// A hover-fade animator for the transparent UI: alpha eases toward 1 while a
/// widget is hovered and back toward a resting alpha when not. Frame-rate
/// independent via an exponential approach.
#[derive(Clone, Copy, Debug)]
pub struct HoverFade {
    alpha: f32,
    rest: f32,
    /// approach rate per second (higher = snappier)
    rate: f32,
}

impl HoverFade {
    pub fn new(rest_alpha: f32) -> HoverFade {
        HoverFade { alpha: rest_alpha, rest: rest_alpha.clamp(0.0, 1.0), rate: 9.0 }
    }
    pub fn alpha(&self) -> f32 {
        self.alpha
    }
    /// Advance by `dt_secs`, easing toward full opacity if `hovered`, else rest.
    pub fn step(&mut self, hovered: bool, dt_secs: f32) -> f32 {
        let target = if hovered { 1.0 } else { self.rest };
        // exponential smoothing: alpha += (target-alpha)*(1-e^{-rate*dt})
        let k = 1.0 - (-self.rate * dt_secs.max(0.0)).exp();
        self.alpha += (target - self.alpha) * k;
        self.alpha = self.alpha.clamp(0.0, 1.0);
        self.alpha
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// F1 — per-menu vignette scenes (push-in framing)
//
// Each menu screen frames a small 3D "set" with a flat PUSH-IN (dolly along the
// view axis toward the subject), distinct from the looping `intro()` orbit. This
// module owns the *motion design + scene selection* purely; the renderer builds
// the actual `dragon_forge` models / scene props from the descriptor and flies
// the real camera along the sampled path. Pure: no IO/render/RNG/clock.
// ─────────────────────────────────────────────────────────────────────────────

use crate::sim::dragon_archetype::Genome;
use crate::sim::title_flow::Screen;

/// Which vignette "set" is staged behind a menu screen.
///
/// The renderer turns this into geometry: `Tree` → a hero tree with wandering
/// fauna props; `BendingDragon` → a `dragon_forge` model built from the carried
/// random [`Genome`] mid-bending-pose; `Waterfall` → a calm waterfall + mist.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Vignette {
    /// Singleplayer: a tree with fauna milling around its base.
    Tree,
    /// Multiplayer: a dragon bending, with RANDOM genetics per view.
    /// Carries the seed the renderer passes to `Genome::from_seed` →
    /// `dragon_forge::build`.
    BendingDragon { seed: u64 },
    /// Settings: a peaceful waterfall.
    Waterfall,
    /// Fallback (Title/Credits/etc.): the existing orbiting intro scene, no set.
    None,
}

impl Vignette {
    /// A short theme hint for the renderer (tint / ambient flavour).
    pub fn theme(self) -> &'static str {
        match self {
            Vignette::Tree => "verdant grove — dappled green/gold",
            Vignette::BendingDragon { .. } => "storm sky — charged cyan",
            Vignette::Waterfall => "cool mist — teal/white",
            Vignette::None => "neutral dusk",
        }
    }

    /// Build the random dragon genome for a `BendingDragon` vignette, ready to
    /// hand to `dragon_forge::build(&g, res)`. Returns `None` for other scenes.
    /// Deterministic in the carried seed (the bin chooses a fresh seed per view).
    pub fn dragon_genome(self) -> Option<Genome> {
        match self {
            Vignette::BendingDragon { seed } => Some(Genome::from_seed(seed)),
            _ => None,
        }
    }
}

/// A flat push-in (dolly): the camera starts pulled back and eases straight in
/// toward the subject along the view axis, holding the look-target steady. This
/// is the menu framing — contrast with [`CinematicPath::intro`]'s orbit.
///
/// Implemented as a two-keyframe [`CinematicPath`] (far → near, same target),
/// sampled with the module's smootherstep easing. Because `CinematicPath` loops,
/// a push-in that should settle is read via [`push_in_pose`] which clamps time
/// to the single inbound segment instead of looping back out.
#[derive(Clone, Debug)]
pub struct PushIn {
    pub far: CameraPose,
    pub near: CameraPose,
    /// Seconds for the full dolly from `far` to `near`.
    pub secs: f32,
}

impl PushIn {
    pub fn new(far: CameraPose, near: CameraPose, secs: f32) -> PushIn {
        PushIn { far, near, secs: secs.max(0.01) }
    }

    /// Sample the eased push-in at `elapsed_secs`, clamped so it settles at
    /// `near` (does not loop back). Same target throughout → a pure dolly.
    pub fn pose(&self, elapsed_secs: f32) -> CameraPose {
        let t = (elapsed_secs / self.secs).clamp(0.0, 1.0);
        self.far.lerp(self.near, smoother(t))
    }

    /// Equivalent looping `CinematicPath` (far→near→far) for code paths that
    /// want a `CinematicPath` handle; most callers use [`PushIn::pose`].
    pub fn as_path(&self) -> CinematicPath {
        CinematicPath::new(vec![self.far, self.near], self.secs)
    }
}

/// The push-in framing for a given vignette. Each set is staged near the origin;
/// the camera dollies straight in toward the subject from a pulled-back start.
pub fn push_in_for(v: Vignette) -> PushIn {
    match v {
        // Tree: pull back low and wide, ease in to a warm hero framing.
        Vignette::Tree => PushIn::new(
            CameraPose { eye: [0.0, 4.5, 22.0], target: [0.0, 4.0, 0.0], fov_deg: 50.0 },
            CameraPose { eye: [0.0, 4.0, 12.0], target: [0.0, 4.0, 0.0], fov_deg: 46.0 },
            3.0,
        ),
        // Dragon: a touch higher, push in to read the bending pose.
        Vignette::BendingDragon { .. } => PushIn::new(
            CameraPose { eye: [2.0, 6.0, 24.0], target: [0.0, 5.0, 0.0], fov_deg: 52.0 },
            CameraPose { eye: [1.0, 5.5, 13.0], target: [0.0, 5.0, 0.0], fov_deg: 47.0 },
            3.2,
        ),
        // Waterfall: tall framing, gentle slow push toward the falls.
        Vignette::Waterfall => PushIn::new(
            CameraPose { eye: [0.0, 7.0, 23.0], target: [0.0, 6.0, 0.0], fov_deg: 48.0 },
            CameraPose { eye: [0.0, 6.5, 14.0], target: [0.0, 6.0, 0.0], fov_deg: 45.0 },
            3.4,
        ),
        // No vignette → a neutral, mild push toward the origin.
        Vignette::None => PushIn::new(
            CameraPose { eye: [0.0, 5.0, 20.0], target: [0.0, 4.0, 0.0], fov_deg: 50.0 },
            CameraPose { eye: [0.0, 5.0, 13.0], target: [0.0, 4.0, 0.0], fov_deg: 48.0 },
            3.0,
        ),
    }
}

/// Select the vignette for a menu screen. `dragon_seed` is consumed only by the
/// Multiplayer screen (the bin passes a fresh random seed each time that screen
/// is shown, so the dragon's genetics differ per view).
pub fn vignette_for(screen: Screen, dragon_seed: u64) -> Vignette {
    match screen {
        Screen::NewGame => Vignette::Tree,
        Screen::Multiplayer => Vignette::BendingDragon { seed: dragon_seed },
        Screen::Settings => Vignette::Waterfall,
        // Title / LoadSlots / Credits / ConfirmQuit keep the orbiting intro.
        _ => Vignette::None,
    }
}

/// Convenience: the (vignette, push-in framing) pair for a screen in one call —
/// what the bin samples each frame after `menu.screen()` changes.
pub fn vignette_and_framing(screen: Screen, dragon_seed: u64) -> (Vignette, PushIn) {
    let v = vignette_for(screen, dragon_seed);
    (v, push_in_for(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_path_loops_and_eases() {
        let p = CinematicPath::intro();
        let a = p.sample(0.0);
        // at t=0 we're exactly on the first keyframe
        assert!((a.eye[0] - 14.0).abs() < 1e-3);
        // looping: one full period returns to the start
        let b = p.sample(p.total_secs());
        assert!((a.eye[0] - b.eye[0]).abs() < 1e-2);
        // mid-segment is between the two keyframes
        let m = p.sample(p.total_secs() / 10.0);
        assert!(m.eye[0] < 14.0 && m.eye[0] > -17.0);
    }

    #[test]
    fn pieces_fly_in_staggered_and_fade() {
        // before its delay, a piece is at its start point and invisible
        let s0 = piece_state(MenuPiece::Quit, 0.0, 0.5);
        assert_eq!(s0.alpha01, 0.0);
        assert!((s0.pos01[0] - MenuPiece::Quit.fly_from01()[0]).abs() < 1e-4);
        // well after delay+fly, it rests at its anchor, fully opaque
        let s1 = piece_state(MenuPiece::Quit, 5.0, 0.5);
        assert!((s1.alpha01 - 1.0).abs() < 1e-3);
        assert!((s1.pos01[0] - MenuPiece::Quit.anchor01()[0]).abs() < 1e-3);
        // Singleplayer leads Quit (arrives earlier)
        let mid = 0.9;
        assert!(piece_state(MenuPiece::Singleplayer, mid, 0.5).alpha01
            > piece_state(MenuPiece::Quit, mid, 0.5).alpha01);
    }

    #[test]
    fn hover_fade_rises_and_settles() {
        let mut f = HoverFade::new(0.35);
        assert!((f.alpha() - 0.35).abs() < 1e-6);
        for _ in 0..120 {
            f.step(true, 1.0 / 60.0);
        }
        assert!(f.alpha() > 0.98, "hovering eases to opaque");
        for _ in 0..120 {
            f.step(false, 1.0 / 60.0);
        }
        assert!((f.alpha() - 0.35).abs() < 0.02, "settles back to rest alpha");
    }

    // ── F1 — vignette selection + push-in framing ──

    #[test]
    fn screen_selects_the_right_vignette() {
        assert!(matches!(vignette_for(Screen::NewGame, 0), Vignette::Tree));
        assert!(matches!(vignette_for(Screen::Settings, 0), Vignette::Waterfall));
        assert!(matches!(
            vignette_for(Screen::Multiplayer, 7),
            Vignette::BendingDragon { seed: 7 }
        ));
        // Non-vignette screens keep the orbit (None).
        for s in [Screen::Title, Screen::LoadSlots, Screen::Credits, Screen::ConfirmQuit] {
            assert!(matches!(vignette_for(s, 0), Vignette::None), "{s:?} should be None");
        }
    }

    #[test]
    fn multiplayer_dragon_seed_is_carried_and_random_per_view() {
        // The bin passes a fresh seed per view; different seeds → different genomes.
        let v1 = vignette_for(Screen::Multiplayer, 111);
        let v2 = vignette_for(Screen::Multiplayer, 222);
        assert_eq!(v1, Vignette::BendingDragon { seed: 111 });
        assert_ne!(v1, v2);
        let g1 = v1.dragon_genome().expect("dragon vignette yields a genome");
        let g2 = v2.dragon_genome().expect("dragon vignette yields a genome");
        // Distinct seeds should (overwhelmingly) give distinct genomes.
        assert_ne!(g1, g2, "random genetics differ per view");
        // Same seed is deterministic.
        let g1b = vignette_for(Screen::Multiplayer, 111).dragon_genome().unwrap();
        assert_eq!(g1, g1b);
    }

    #[test]
    fn non_dragon_vignettes_have_no_genome() {
        assert!(vignette_for(Screen::NewGame, 0).dragon_genome().is_none());
        assert!(vignette_for(Screen::Settings, 0).dragon_genome().is_none());
        assert!(vignette_for(Screen::Title, 0).dragon_genome().is_none());
    }

    #[test]
    fn push_in_is_a_dolly_not_an_orbit() {
        // The look-target stays fixed (dolly), and the camera moves strictly
        // closer to it along the run (push-in, not pull-out or orbit).
        let p = push_in_for(Vignette::Tree);
        let start = p.pose(0.0);
        let end = p.pose(10.0); // clamped to settle at `near`
        // target unchanged
        assert!((start.target[2] - end.target[2]).abs() < 1e-4);
        // distance to target strictly decreased
        let d = |c: &CameraPose| {
            let dx = c.eye[0] - c.target[0];
            let dy = c.eye[1] - c.target[1];
            let dz = c.eye[2] - c.target[2];
            (dx * dx + dy * dy + dz * dz).sqrt()
        };
        assert!(d(&end) < d(&start), "push-in must move closer: {} -> {}", d(&start), d(&end));
    }

    #[test]
    fn push_in_settles_and_does_not_loop() {
        let p = push_in_for(Vignette::Waterfall);
        let near = p.pose(p.secs * 2.0); // well past the end
        let at_end = p.pose(p.secs);
        // clamped: sampling past the end equals the settled `near` pose
        assert!((near.eye[2] - at_end.eye[2]).abs() < 1e-4);
        assert!((near.eye[2] - p.near.eye[2]).abs() < 1e-3, "settles exactly at near");
        // start equals far
        assert!((p.pose(0.0).eye[2] - p.far.eye[2]).abs() < 1e-4);
    }

    #[test]
    fn push_in_monotone_inbound() {
        // z-distance should be non-increasing across the dolly (eased but monotone).
        let p = push_in_for(Vignette::BendingDragon { seed: 1 });
        let mut prev = f32::INFINITY;
        let mut s = 0.0;
        while s <= p.secs {
            let z = p.pose(s).eye[2];
            assert!(z <= prev + 1e-4, "eye z not monotone inbound at {s}: {prev} -> {z}");
            prev = z;
            s += p.secs / 20.0;
        }
    }

    #[test]
    fn vignette_and_framing_pairs_match_parts() {
        let (v, pf) = vignette_and_framing(Screen::Multiplayer, 42);
        assert_eq!(v, vignette_for(Screen::Multiplayer, 42));
        let direct = push_in_for(v);
        assert!((pf.far.eye[2] - direct.far.eye[2]).abs() < 1e-6);
        assert!((pf.secs - direct.secs).abs() < 1e-6);
    }

    #[test]
    fn as_path_round_trips_endpoints() {
        let p = push_in_for(Vignette::Tree);
        let path = p.as_path();
        // path keyframe 0 is far; sampling at t=0 hits it
        let a = path.sample(0.0);
        assert!((a.eye[2] - p.far.eye[2]).abs() < 1e-3);
    }
}
