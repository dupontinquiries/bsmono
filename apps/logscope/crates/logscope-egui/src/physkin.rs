//! physkin.rs — what the in-scene PHYSICAL UI PANEL is made of, per scene: its board, its edge
//! hardware, how it is held up, and the light fixture beside it.
//!
//! `scene3d::physical_ui_panel` draws the board; this decides what to draw. It used to be a `u32`
//! style code (0 wood / 1 stone / 2 glass / 3 marble) and a `floating` bool chosen in a match inside
//! `scene3d::build`, which meant the board's whole vocabulary was four surfaces and two stands. The
//! reference renders disagree: every panel in them hangs from something (chains off a rock overhang,
//! ropes off a balcony beam), has a light source next to it (a caged lantern, a paper lantern), has
//! round bolt heads at the corners rather than a gilt bracket, and the context panel carries a strip
//! of tabs along its top edge. Those are the cues that make a board read as hardware in the room
//! rather than a texture pasted into it.
//!
//! Pure data + a per-scene table, no geometry, so the decisions are testable without a renderer and
//! so a new scene declares its skin in one place. The renderer maps each field to meshes.

/// The board's face + frame material. The first four are the original style codes, kept as-is so
/// existing scenes render as they did; the rest are new.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Board {
    /// warm oiled wood board (Alley, Forest, Canopy…)
    WoodBoard,
    /// grey stone tablet (Slate, the libraries, the crystalline worlds)
    StoneTablet,
    /// translucent glass pane on thin posts (Atlantis, Bifrost, Mistfall, the sky worlds)
    GlassPane,
    /// cream marble with gold trim (Mansion Garden)
    MarbleGilt,
    /// near-black slate in an iron frame — the cave reference's riveted plaques
    IronSlate,
    /// dark stained timber with a copper rim — Elmbrook's district boards
    DarkTimber,
    /// black basalt with an ember-warm edge — the lava road
    Basalt,
}

/// Edge hardware.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rim {
    /// the original L-shaped gilt corner brackets
    GiltBrackets,
    /// round bolt heads at the corners + a run of small studs along each edge (every reference render)
    Rivets,
    /// a rope lashed round the frame — no metal in this world
    Rope,
    /// nothing beyond the bevel (glass)
    Bare,
}

/// How the board is held up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hang {
    /// on a stand at ground level (the per-board pedestal / posts)
    Stand,
    /// no stand: a short mount stub, the board hovers (the original `floating`)
    Floating,
    /// two chains up to an overhead bar (cave overhang, iron world)
    Chains,
    /// two ropes up to a timber beam (treehouse balcony, village)
    Ropes,
    /// resting on two stacked-stone cairns
    Cairns,
}

/// The light next to the board. `row_glow` already lights the rows from the scene's `SceneLight`;
/// this makes the SOURCE visible, which is the difference between a lit board and a board that
/// glows for no reason.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fixture {
    None,
    /// an iron-caged oil lantern on a hook off the frame's corner
    CagedLantern,
    /// a round paper lantern on a cord
    PaperLantern,
    /// a stub candle on a small bracket shelf
    Candle,
    /// a glowing crystal shard clamped to the frame
    Crystal,
    /// a neon tube along the top edge
    NeonTube,
    /// a bioluminescent bulb / jellyfish globe on a stalk
    BioBulb,
    /// no lamp: sunlight — a small brass sun-catcher / signpost only
    SunCatcher,
}

/// One scene's panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PanelSkin {
    pub board: Board,
    pub rim: Rim,
    pub hang: Hang,
    pub fixture: Fixture,
    /// number of raised tab plates along the top edge (0 = none). The reference context panels all
    /// carry four (FILES / GRAPH / NOTES / PROMPTS); the first is the lit/active one.
    pub tabs: u8,
}

impl Fixture {
    /// The lamp's light as (r, g, b) 0..255. Warm for flame and paper, cool for crystal, neon and
    /// bio-glow. The renderer uses this both for the fixture's emissive and for a faint tint on the
    /// nearest rows, so the lamp visibly lights the board it hangs by.
    pub fn light_rgb(self) -> (u8, u8, u8) {
        match self {
            Fixture::None => (0, 0, 0),
            Fixture::CagedLantern => (0xff, 0xb8, 0x5a),
            Fixture::PaperLantern => (0xff, 0xd6, 0x8e),
            Fixture::Candle => (0xff, 0xc0, 0x60),
            Fixture::Crystal => (0x9a, 0xd8, 0xff),
            Fixture::NeonTube => (0xff, 0x5a, 0xc8),
            Fixture::BioBulb => (0x5a, 0xe6, 0xd0),
            Fixture::SunCatcher => (0xff, 0xe8, 0xb0),
        }
    }

    /// Does this fixture read as WARM (flame / paper / sun) rather than cold (crystal / neon /
    /// bio-glow)? Derived from the colour, not a second table — the hue is the mechanism.
    pub fn is_warm(self) -> bool {
        let (r, _g, b) = self.light_rgb();
        r > b
    }
}

impl PanelSkin {
    /// The original four style codes + the floating flag, so scenes that have not been re-skinned
    /// keep exactly the board they had.
    pub const fn legacy(style: u32, floating: bool) -> PanelSkin {
        let board = match style {
            1 => Board::StoneTablet,
            2 => Board::GlassPane,
            3 => Board::MarbleGilt,
            _ => Board::WoodBoard,
        };
        PanelSkin {
            board,
            rim: Rim::GiltBrackets,
            hang: if floating { Hang::Floating } else { Hang::Stand },
            fixture: Fixture::None,
            tabs: 0,
        }
    }

    /// The panel for a named scene.
    ///
    /// Every arm is a decision about what the reference render for that world shows (or, for worlds
    /// without one, what its palette + light say the room is made of). Unknown names — the
    /// procedural rooms — get the plain stone tablet on a stand with rivets and a candle, which is
    /// the neutral "there is a lamp and it is hardware" default.
    pub fn for_scene(name: &str) -> PanelSkin {
        use Board::*;
        use Fixture::*;
        use Hang::*;
        use Rim::*;
        let sk = |board, rim, hang, fixture, tabs| PanelSkin { board, rim, hang, fixture, tabs };
        match name {
            // ── the three reference-driven worlds ──────────────────────────────────────────────
            // cat cave: iron-bound slate plaques hung on chains off the rock, a caged lantern by each
            "Cat Cave" => sk(IronSlate, Rivets, Chains, CagedLantern, 4),
            // Elmbrook, Riverhollow district (evening, treehouse city): dark timber on ropes off a
            // balcony beam, a paper lantern beside it
            "Forest Village" => sk(DarkTimber, Rivets, Ropes, PaperLantern, 4),
            // Elmbrook, Riverside district (daylight, lava-rock path): the SAME board as Riverhollow
            // — one city, one carpentry — but on cairns by the stream in the sun, no lamp lit
            "Lavarock Way" => sk(DarkTimber, Rivets, Cairns, SunCatcher, 4),
            // ── existing worlds, re-skinned toward the references ─────────────────────────────
            "Alley" => sk(WoodBoard, Rivets, Ropes, CagedLantern, 4),
            "Treehouse" => sk(WoodBoard, Rope, Ropes, PaperLantern, 4),
            "Forest" | "Canopy" => sk(WoodBoard, Rope, Stand, PaperLantern, 4),
            "Hollow Jungle" => sk(WoodBoard, Rope, Floating, PaperLantern, 4),
            "Old Library" | "Archive" | "Reading Room" => sk(StoneTablet, GiltBrackets, Stand, Candle, 4),
            "Slate" => sk(StoneTablet, Rivets, Stand, Candle, 4),
            "Recursive Citadel" | "Yellow Brick Road" | "Titan's Ribcage" => sk(StoneTablet, Rivets, Stand, SunCatcher, 4),
            "Fractal Spires" => sk(StoneTablet, Bare, Floating, Crystal, 4),
            "Frozen Bifrost" => sk(GlassPane, Bare, Floating, Crystal, 4),
            "Mansion Garden" => sk(MarbleGilt, GiltBrackets, Stand, SunCatcher, 4),
            "Atlantis" => sk(GlassPane, Bare, Stand, BioBulb, 4),
            "Mistfall" => sk(GlassPane, Bare, Stand, BioBulb, 4),
            "Cloudscape" | "Folding@home" => sk(GlassPane, Bare, Floating, BioBulb, 4),
            _ => sk(StoneTablet, Rivets, Stand, Candle, 4),
        }
    }

    /// Is the board hung from above (so the renderer must build the bar/beam it hangs from)?
    pub fn is_hung(self) -> bool { matches!(self.hang, Hang::Chains | Hang::Ropes) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full scene list `scene3d::build` dispatches on, mirrored from scene-kit so a name added
    /// there without a skin decision here shows up as the default (and this test names it).
    fn bespoke_scenes() -> Vec<&'static str> {
        let mut v: Vec<&'static str> = scene_kit::mesh3d::SCENE_NAMES.to_vec();
        for extra in ["Titan's Ribcage", "Cat Cave", "Forest Village", "Lavarock Way"] {
            if !v.contains(&extra) { v.push(extra); }
        }
        v
    }

    #[test]
    fn the_three_reference_worlds_hang_their_boards_and_light_them() {
        // What the reference renders show, stated as assertions: cave plaques hang on chains with a
        // caged lantern; the treehouse city hangs them on ropes with a paper lantern; the sunlit
        // riverside sets them on stones and has no lamp lit.
        let cave = PanelSkin::for_scene("Cat Cave");
        assert_eq!(cave.hang, Hang::Chains);
        assert_eq!(cave.fixture, Fixture::CagedLantern);
        assert!(cave.is_hung());
        let village = PanelSkin::for_scene("Forest Village");
        assert_eq!(village.hang, Hang::Ropes);
        assert_eq!(village.fixture, Fixture::PaperLantern);
        let river = PanelSkin::for_scene("Lavarock Way");
        assert_eq!(river.hang, Hang::Cairns);
        assert!(!river.is_hung());
        assert_eq!(river.fixture, Fixture::SunCatcher);
    }

    #[test]
    fn elmbrook_districts_share_one_carpentry_and_differ_in_how_the_board_sits() {
        // One city, two districts: the board is the same timber with the same rivets, and ONLY the
        // suspension and lamp change with the time of day. Asserting both halves — a refactor that
        // made them identical or that gave Riverside its own board would both be wrong.
        let a = PanelSkin::for_scene("Forest Village");
        let b = PanelSkin::for_scene("Lavarock Way");
        assert_eq!((a.board, a.rim, a.tabs), (b.board, b.rim, b.tabs));
        assert_ne!(a.hang, b.hang);
        assert_ne!(a.fixture, b.fixture);
    }

    #[test]
    fn the_table_is_not_collapsed() {
        // A per-scene table earns its place by varying. Count distinct skins across every bespoke
        // scene and require a real spread — and a floor on how many scenes were examined, so the
        // test cannot pass by scanning an empty list.
        let scenes = bespoke_scenes();
        assert!(scenes.len() >= 18, "scanned {}", scenes.len());
        let mut distinct: Vec<PanelSkin> = Vec::new();
        let mut hung = 0;
        let mut lit = 0;
        for name in &scenes {
            let s = PanelSkin::for_scene(name);
            if !distinct.contains(&s) { distinct.push(s); }
            if s.is_hung() { hung += 1; }
            if s.fixture != Fixture::None { lit += 1; }
        }
        assert!(distinct.len() >= 10, "{} distinct skins", distinct.len());
        assert!((3..=8).contains(&hung), "{hung} hung boards — some hang, most stand");
        assert_eq!(lit, scenes.len(), "every bespoke scene shows where its board's light comes from");
    }

    #[test]
    fn legacy_codes_map_to_the_boards_they_meant() {
        assert_eq!(PanelSkin::legacy(0, false).board, Board::WoodBoard);
        assert_eq!(PanelSkin::legacy(1, false).board, Board::StoneTablet);
        assert_eq!(PanelSkin::legacy(2, true).board, Board::GlassPane);
        assert_eq!(PanelSkin::legacy(3, false).board, Board::MarbleGilt);
        assert_eq!(PanelSkin::legacy(2, true).hang, Hang::Floating);
        assert_eq!(PanelSkin::legacy(2, false).hang, Hang::Stand);
        assert_eq!(PanelSkin::legacy(7, false).fixture, Fixture::None);
    }

    #[test]
    fn flame_and_paper_are_warm_and_crystal_neon_bio_are_cold() {
        for f in [Fixture::CagedLantern, Fixture::PaperLantern, Fixture::Candle, Fixture::SunCatcher] {
            assert!(f.is_warm(), "{f:?}");
        }
        for f in [Fixture::Crystal, Fixture::NeonTube, Fixture::BioBulb] {
            assert!(!f.is_warm(), "{f:?}");
        }
        // an unlit fixture is black, and black must not claim warmth
        assert!(!Fixture::None.is_warm());
    }

    #[test]
    fn a_default_scene_still_has_hardware_and_a_lamp() {
        let d = PanelSkin::for_scene("Somewhere New");
        assert_eq!(d.rim, Rim::Rivets);
        assert_ne!(d.fixture, Fixture::None);
        assert_eq!(d.tabs, 4);
    }
}
