//! HUMAN SCALE — and the board, which is what makes it playable.
//!
//! Ported from `game.c`'s `g_human_scale` / `g_skateboard` pair. The design
//! line for the board is three words long —
//!
//! > AIDEN'S SKATEBOARD: human-scale "vehicle mode" — a salvaged deck with
//! > valence trucks. Transform (while human-scale) hops on/off; near-double
//! > pace, board rendered underfoot.
//!
//! — and the interesting part is not in that line at all. It is in two
//! numbers elsewhere in the file: a human moves at **0.45** of a bot's pace,
//! and on the board at **0.85**.
//!
//! ## The scale gap is a mobility gap
//!
//! `scale.rs` establishes that a bot is about five people tall and Two
//! California Plaza is a hundred and thirty. Those are rendering problems.
//! This is the gameplay one: **a world built at Transformer scale is not
//! merely large for a person, it is unwalkable.** At 0.45 pace a crossing
//! that costs a bot fifteen seconds costs Aiden thirty-three, and the game
//! becomes a commute.
//!
//! The board is the fix, and it is a fix rather than a power-up: 0.85 does
//! not make him fast, it makes the map the same size it was for everyone
//! else. That is why the C hangs it on the TRANSFORM key — at human scale
//! transforming is disabled and the board takes its place, so the verb means
//! "become the thing that can cross this" at either scale.
//!
//! ## The guard that is easy to miss
//!
//! `game.c` snaps the avatar back to robot mode whenever scale is toggled,
//! with a comment explaining why: toggling mid-transform would strand you as
//! a human-scale VEHICLE — posed flat or upside down, with mode changes
//! locked. It doubles as a recovery for any stuck transform. That is a real
//! bug fixed defensively, and it survives the port as `Avatar::set_scale`.

/// How big the player currently is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scale {
    /// the robot: what the arenas were built for
    Titan,
    /// Aiden, and the archive era
    Human,
}

/// Pace as a fraction of a bot's, on foot.
pub const HUMAN_PACE: f32 = 0.45;
/// And on the board.
pub const BOARD_PACE: f32 = 0.85;

/// Collider half-extents, metres: (x, y, z).
pub const TITAN_RADIUS: (f32, f32, f32) = (0.5, 1.0, 0.5);
pub const HUMAN_RADIUS: (f32, f32, f32) = (0.28, 0.5, 0.28);

impl Scale {
    pub fn radius(self) -> (f32, f32, f32) {
        match self {
            Scale::Titan => TITAN_RADIUS,
            Scale::Human => HUMAN_RADIUS,
        }
    }
    /// Can this scale fold into a vehicle? A human cannot — the board is
    /// what they have instead, and it is bolted to the same key.
    pub fn can_transform(self) -> bool {
        self == Scale::Titan
    }
    /// The `scale::Class` this frames as, so the camera agrees with the body.
    pub fn class(self) -> crate::scale::Class {
        match self {
            Scale::Titan => crate::scale::Class::Bot,
            Scale::Human => crate::scale::Class::Human,
        }
    }
    /// Height in metres, from `scale::LADDER` rather than a second opinion.
    pub fn height_m(self) -> f32 {
        let id = match self {
            Scale::Titan => "bot",
            Scale::Human => "human",
        };
        crate::scale::sized(id).map(|s| s.height_m).unwrap_or(1.75)
    }
}

/// Why a transform request was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fold {
    /// folded to vehicle, or back
    Transformed,
    /// stepped onto the board, or off it
    Board(bool),
}

pub struct Avatar {
    pub scale: Scale,
    /// on the deck. Only meaningful at human scale.
    pub board: bool,
    /// folded into vehicle mode. Only meaningful at titan scale.
    pub vehicle: bool,
}

impl Default for Avatar {
    fn default() -> Self {
        Avatar { scale: Scale::Titan, board: false, vehicle: false }
    }
}

impl Avatar {
    /// Pace as a fraction of a bot on foot.
    pub fn pace(&self) -> f32 {
        match self.scale {
            Scale::Titan => 1.0,
            Scale::Human => {
                if self.board {
                    BOARD_PACE
                } else {
                    HUMAN_PACE
                }
            }
        }
    }

    /// The transform key. Means different things at different sizes, which
    /// is the design: *become the thing that can cross this*.
    pub fn fold(&mut self) -> Fold {
        match self.scale {
            Scale::Titan => {
                self.vehicle = !self.vehicle;
                Fold::Transformed
            }
            Scale::Human => {
                self.board = !self.board;
                Fold::Board(self.board)
            }
        }
    }

    /// Change size.
    ///
    /// ALWAYS UNFOLDS FIRST, and the C explains why in a comment worth
    /// keeping: toggling scale mid-transform strands you as a human-scale
    /// vehicle — posed flat or upside down, with mode changes locked. It also
    /// doubles as a recovery for any stuck transform, which is the reason it
    /// is unconditional rather than guarded on "was mid-fold".
    pub fn set_scale(&mut self, to: Scale) {
        self.vehicle = false;
        self.board = false;
        self.scale = to;
    }

    /// Seconds to cross `metres`, given a bot's base speed.
    pub fn crossing_s(&self, metres: f32, bot_speed_ms: f32) -> f32 {
        metres / (bot_speed_ms * self.pace()).max(0.01)
    }
}

/// A bot's ground speed, m/s. The reference every pace is a fraction of.
pub const BOT_SPEED_MS: f32 = 14.0;

// ------------------------------------------------------------- driver ---

pub fn run() {
    use crate::scale;
    println!("HUMAN SCALE — and why the board is a fix, not a power-up\n");

    let mut a = Avatar::default();
    println!("  {:<22} {:>8} {:>8} {:>10}", "", "pace", "height", "collider y");
    for (label, scale, board) in [
        ("a bot", Scale::Titan, false),
        ("Aiden, on foot", Scale::Human, false),
        ("Aiden, on the board", Scale::Human, true),
    ] {
        a.set_scale(scale);
        a.board = board;
        println!(
            "  {:<22} {:>7.0}% {:>7.2}m {:>10.2}",
            label,
            a.pace() * 100.0,
            a.scale.height_m(),
            a.scale.radius().1
        );
    }

    println!("\n  CROSSING THINGS BUILT FOR SOMEBODY ELSE (seconds)\n");
    let spans: [(&str, f32); 4] = [
        ("a warehouse floor", 60.0),
        ("the hollow", 200.0),
        ("Two California Plaza, base to base", 228.0),
        ("a patrol zone", 900.0),
    ];
    println!("  {:<38} {:>7} {:>7} {:>7}", "", "bot", "foot", "board");
    for (name, m) in spans {
        let mut bot = Avatar::default();
        let mut foot = Avatar { scale: Scale::Human, board: false, vehicle: false };
        let mut brd = Avatar { scale: Scale::Human, board: true, vehicle: false };
        println!(
            "  {:<38} {:>6.0}s {:>6.0}s {:>6.0}s",
            name,
            bot.crossing_s(m, BOT_SPEED_MS),
            foot.crossing_s(m, BOT_SPEED_MS),
            brd.crossing_s(m, BOT_SPEED_MS)
        );
        let _ = (&mut bot, &mut foot, &mut brd);
    }

    let foot = Avatar { scale: Scale::Human, board: false, vehicle: false };
    let brd = Avatar { scale: Scale::Human, board: true, vehicle: false };
    let lost = foot.crossing_s(900.0, BOT_SPEED_MS) - brd.crossing_s(900.0, BOT_SPEED_MS);
    println!("\n  the board saves {lost:.0}s on a patrol zone. It does not make Aiden");
    println!("  fast — {:.0}% of a bot is still slower than a bot. It makes the map",
        BOARD_PACE * 100.0);
    println!("  the size it already was for everyone else.");

    println!("\n  THE VERB IS THE SAME KEY AT BOTH SIZES\n");
    let mut v = Avatar::default();
    println!("    titan:  {:?}  -> vehicle {}", v.fold(), v.vehicle);
    v.set_scale(Scale::Human);
    println!("    human:  {:?}  -> board {}", v.fold(), v.board);
    println!("\n  and a human cannot fold: {}", v.scale.can_transform());
    println!("  so `transform` means \"become the thing that can cross this\" either way.");

    // the camera agrees with the body, from the ladder rather than a guess
    println!("\n  THE CAMERA FOLLOWS THE SIZE\n");
    for s in [Scale::Titan, Scale::Human] {
        let (d, up) = scale::camera_for(s.class(), s.height_m());
        println!("    {:<8} {:>6.1} m back, {:>4.1} m up", format!("{s:?}"), d, up);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_human_is_half_a_bot_and_the_board_gives_it_back() {
        let bot = Avatar::default();
        let foot = Avatar { scale: Scale::Human, board: false, vehicle: false };
        let brd = Avatar { scale: Scale::Human, board: true, vehicle: false };
        assert_eq!(bot.pace(), 1.0);
        assert!(foot.pace() < 0.5, "on foot should be less than half a bot");
        assert!(brd.pace() > foot.pace() * 1.8, "the board should be near-double pace");
        // and it is still not as fast as a bot: a fix, not a power-up
        assert!(brd.pace() < 1.0);
    }

    #[test]
    fn the_scale_gap_is_a_mobility_gap() {
        // the claim the module exists for: a Transformer-scale map is not
        // merely large for a person, it is a commute
        let bot = Avatar::default();
        let foot = Avatar { scale: Scale::Human, board: false, vehicle: false };
        let span = 900.0;
        let b = bot.crossing_s(span, BOT_SPEED_MS);
        let f = foot.crossing_s(span, BOT_SPEED_MS);
        assert!(f > b * 2.0, "a patrol zone should be twice the trek: {f} vs {b}");
        let brd = Avatar { scale: Scale::Human, board: true, vehicle: false };
        assert!(
            brd.crossing_s(span, BOT_SPEED_MS) < f * 0.6,
            "the board should recover most of it"
        );
    }

    #[test]
    fn the_transform_key_means_something_at_both_sizes() {
        let mut a = Avatar::default();
        assert_eq!(a.fold(), Fold::Transformed);
        assert!(a.vehicle);
        assert_eq!(a.fold(), Fold::Transformed);
        assert!(!a.vehicle, "it should toggle back");

        a.set_scale(Scale::Human);
        assert!(!a.scale.can_transform(), "a human folded into a vehicle");
        assert_eq!(a.fold(), Fold::Board(true));
        assert!(a.board);
        assert_eq!(a.fold(), Fold::Board(false));
    }

    #[test]
    fn changing_scale_never_strands_you_mid_fold() {
        // The guard `game.c` explains in a comment: toggling scale while in
        // vehicle mode would leave you a human-scale vehicle, posed flat or
        // upside down, with mode changes locked. Unconditional, so it also
        // recovers a stuck transform.
        let mut a = Avatar::default();
        a.fold();
        assert!(a.vehicle);
        a.set_scale(Scale::Human);
        assert!(!a.vehicle, "shrank while folded");
        assert!(!a.board);

        // and the other direction
        a.fold();
        assert!(a.board);
        a.set_scale(Scale::Titan);
        assert!(!a.board, "grew while on the board");
        assert!(!a.vehicle);
    }

    #[test]
    fn a_human_is_a_smaller_target_in_every_axis() {
        let (tx, ty, tz) = Scale::Titan.radius();
        let (hx, hy, hz) = Scale::Human.radius();
        assert!(hx < tx && hy < ty && hz < tz);
        // and roughly half, which is what makes doorways and cover work
        assert!(hy / ty < 0.6);
    }

    #[test]
    fn height_and_camera_come_from_the_ladder_not_from_here() {
        // a second opinion about how tall a person is would let this module
        // and `scale.rs` disagree about the same body
        assert_eq!(Scale::Human.height_m(), crate::scale::sized("human").unwrap().height_m);
        assert_eq!(Scale::Titan.height_m(), crate::scale::sized("bot").unwrap().height_m);
        assert_eq!(Scale::Human.class(), crate::scale::Class::Human);

        let (hd, _) = crate::scale::camera_for(Scale::Human.class(), Scale::Human.height_m());
        let (td, _) = crate::scale::camera_for(Scale::Titan.class(), Scale::Titan.height_m());
        assert!(hd < td, "the camera should come in for a person: {hd} vs {td}");
    }

    #[test]
    fn a_human_at_bot_pace_would_be_the_wrong_fix() {
        // stated as a test because it is the design decision: if the board
        // reached 1.0 the answer to human scale would be "stop being human",
        // and the campaign's whole register goes with it
        assert!(BOARD_PACE < 1.0);
        assert!(BOARD_PACE > HUMAN_PACE * 1.5, "and it has to actually help");
    }
}
