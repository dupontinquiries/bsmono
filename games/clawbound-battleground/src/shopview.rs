//! shopview — what the shop SAYS, separate from how it is drawn.
//!
//! The shop was one egui function that asked the game, decided and painted in
//! the same breath: whether an item could be bought was worked out three
//! times (the card's colour, the Enter key, the click), each slightly
//! differently, and the only reason a purchase could fail -- no gold, a full
//! build -- was a greyed name nobody could read an explanation from. It was
//! also a fixed 960 x 660 window, which on a phone is most of a screen wider
//! than the phone.
//!
//! This is the headless half: every decision the shop makes, as data, with
//! no egui. `ui::draw_shop` renders it and does nothing else. What varies is
//! the shell -- the desktop window, a phone sheet, a gamepad grid -- and all
//! of them read the same [`Offer`]s.
//!
//! ## The theme is a mapping, not a rename
//!
//! The catalog's categories (`items::Cat`) and item names are game data that
//! tests, bots and the build logic key on. The clan theme is how they are
//! PRESENTED: a Strength item is on the Claw shelf. [`den`] is that mapping,
//! in one table, so a later rename of the catalog itself is a data change and
//! not a hunt through draw code.

use crate::items::{self, Cat, Item};
use crate::sim::Game;

/// Build slots. `Game::buy_item` refuses a seventh; this is the same number.
pub const SLOTS: usize = 6;

/// Why an item cannot be bought right now. Ordered by what the player can
/// do about it: a full build needs a sale, gold needs time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Refusal {
    /// Six items already. Sell one first.
    Full,
    /// Not enough gold; `short` is how much more is needed.
    Gold { short: f32 },
}

impl Refusal {
    pub fn text(&self) -> String {
        match *self {
            Refusal::Full => "your hoard is full -- trade one away first".to_string(),
            Refusal::Gold { short } => format!("{short:.0} gold short"),
        }
    }
}

/// A picture for a shelf, drawn by the shell. Named for what it depicts so a
/// shell without vector drawing can fall back to a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Claw,
    Whisker,
    Star,
    Pelt,
    Fang,
    Pounce,
    Heart,
    Trophy,
}

/// One shelf of the hoard: a category as the clans name it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Den {
    pub cat: Cat,
    /// Short shelf name, one word.
    pub name: &'static str,
    /// One line on what the shelf is for, in the stat terms a player needs.
    pub blurb: &'static str,
    pub glyph: Glyph,
    /// Shelf colour, from the catalog so the HUD's item chips agree.
    pub color: [f32; 3],
}

/// The shelf for a category. One table; see the module header.
pub fn den(cat: Cat) -> Den {
    let (name, blurb, glyph) = match cat {
        Cat::Strength => ("Claw", "raw strength for claws and swipes", Glyph::Claw),
        Cat::Intelligence => ("Whisker", "wits, mana and quicker techniques", Glyph::Whisker),
        Cat::Scaling => ("Starlight", "spell power the ancestors lend", Glyph::Star),
        Cat::Protection => ("Pelt", "protection against every blow", Glyph::Pelt),
        Cat::Penetration => ("Fang", "bite through another cat's pelt", Glyph::Fang),
        Cat::AttackSpeed => ("Pounce", "faster strikes", Glyph::Pounce),
        Cat::Health => ("Heart", "more life to lose", Glyph::Heart),
        Cat::Stacking => ("Trophy", "grows with every kill it witnesses", Glyph::Trophy),
    };
    Den { cat, name, blurb, glyph, color: cat.color() }
}

/// Every shelf, in catalog order.
pub fn dens() -> [Den; 8] {
    items::CATS.map(den)
}

/// Tier as the jungle grows: sapling, bough, elder.
pub const fn tier_name(tier: u8) -> &'static str {
    match tier {
        0 | 1 => "Sapling",
        2 => "Bough",
        _ => "Elder",
    }
}

/// One item on a shelf, with everything the shell needs to draw it.
#[derive(Debug, Clone, Copy)]
pub struct Offer {
    pub item: &'static Item,
    /// `None`: buyable now.
    pub refusal: Option<Refusal>,
    /// This shelf is one of the two the player's role builds first.
    pub recommended: bool,
    /// Build slot already holding one, if any. A stacking item shows its
    /// stacks there, so the card says "owned" instead of a price.
    pub owned: Option<usize>,
}

impl Offer {
    pub fn buyable(&self) -> bool {
        self.refusal.is_none()
    }
}

/// The same rule `Game::buy_item` applies, stated once for the shell.
pub fn can_buy(gold: f32, owned: usize, item: &Item) -> Result<(), Refusal> {
    if owned >= SLOTS {
        Err(Refusal::Full)
    } else if gold < item.cost {
        Err(Refusal::Gold { short: item.cost - gold })
    } else {
        Ok(())
    }
}

/// The two shelves the player's role builds first.
pub fn recommended(game: &Game, player: usize) -> [Cat; 2] {
    let o = items::role_build_order(game.def(player).role);
    [o[0], o[1]]
}

/// The offers on one shelf, cheapest first (the catalog's own order within a
/// shelf is by tier, and within a tier it is arbitrary).
pub fn offers(game: &Game, player: usize, cat: Cat) -> Vec<Offer> {
    let g = &game.gods[player];
    let rec = recommended(game, player).contains(&cat);
    let mut v: Vec<Offer> = items::all()
        .iter()
        .filter(|it| it.cat == cat)
        .map(|item| Offer {
            item,
            refusal: can_buy(g.gold, g.items.len(), item).err(),
            recommended: rec,
            owned: g.items.iter().position(|&id| id == item.id),
        })
        .collect();
    v.sort_by(|a, b| a.item.cost.partial_cmp(&b.item.cost).unwrap_or(std::cmp::Ordering::Equal).then(a.item.id.cmp(&b.item.id)));
    v
}

/// What to buy next, if anything: the dearest item the player can afford
/// now on a recommended shelf that they do not already hold. A pointer, not
/// a decision -- the shell shows it, the player chooses.
pub fn suggestion(game: &Game, player: usize) -> Option<&'static Item> {
    let rec = recommended(game, player);
    rec.iter()
        .flat_map(|&c| offers(game, player, c))
        .filter(|o| o.buyable() && o.owned.is_none())
        .map(|o| o.item)
        .max_by(|a, b| a.cost.partial_cmp(&b.cost).unwrap_or(std::cmp::Ordering::Equal))
}

/// What makes a trophy grow, in words.
pub const fn stack_kind_name(k: items::StackKind) -> &'static str {
    match k {
        items::StackKind::MinionKill => "grows with each minion felled",
        items::StackKind::GodKill => "grows with each ancestor taken down",
        items::StackKind::TowerKill => "grows with each tower",
        items::StackKind::Time => "grows as the match goes on",
    }
}

/// Stat line for an item in the shelf's words: short tokens, biggest first
/// is not attempted -- catalog order is stat order, which players learn.
pub fn stat_tokens(s: &items::Stats) -> Vec<String> {
    let mut v = Vec::new();
    let mut add = |x: f32, pct: bool, tag: &str| {
        if x > 0.0 {
            v.push(if pct { format!("+{:.0}% {tag}", x * 100.0) } else { format!("+{x:.0} {tag}") });
        }
    };
    add(s.strength, false, "STR");
    add(s.intelligence, false, "INT");
    add(s.power, false, "PWR");
    add(s.protection, false, "PROT");
    add(s.pen_pct, true, "PEN");
    add(s.pen_flat, false, "flat PEN");
    add(s.attack_speed, true, "AS");
    add(s.health, false, "HP");
    add(s.mana, false, "MP");
    add(s.cdr, true, "CDR");
    add(s.move_speed, true, "MS");
    v
}

/// How the shell should lay itself out for a screen `width` x `height`
/// logical points. Two columns need room for a card column and a build
/// column; below that the build becomes a second tab rather than a squeezed
/// column. A phone held landscape is WIDE but short (~844 x 390): two
/// columns fit, the named shelf stones and the subtitle do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Shelf and build side by side, full chrome.
    Wide,
    /// Side by side, compact chrome: glyph-only shelves, no subtitle.
    Short,
    /// One column; the build is behind a tab.
    Narrow,
}

pub const WIDE_MIN: f32 = 760.0;
pub const TALL_MIN: f32 = 560.0;

pub fn layout_for(width: f32, height: f32) -> Layout {
    if !(width.is_finite() && width >= WIDE_MIN) {
        Layout::Narrow
    } else if height.is_finite() && height >= TALL_MIN {
        Layout::Wide
    } else {
        Layout::Short
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Game {
        Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9])
    }

    /// The shell's rule and the game's rule agree on every item at every
    /// gold level that matters: just short, exactly enough, and full.
    #[test]
    fn can_buy_is_buy_items_rule() {
        for item in items::all() {
            for (gold, owned) in [(item.cost - 1.0, 0usize), (item.cost, 0), (item.cost, SLOTS - 1), (item.cost * 10.0, SLOTS)] {
                let mut g = game();
                g.gods[0].gold = gold;
                g.gods[0].items = vec![0; owned];
                g.gods[0].stacks = vec![0.0; owned];
                let said = can_buy(gold, owned, item).is_ok();
                let did = g.buy_item(0, item.id);
                assert_eq!(said, did, "{} at {gold} gold with {owned} owned: shop said {said}, game did {did}", item.name);
            }
        }
    }

    #[test]
    fn a_refusal_says_what_to_do() {
        let it = &items::all()[0];
        match can_buy(it.cost - 120.0, 0, it) {
            Err(r @ Refusal::Gold { short }) => {
                assert!((short - 120.0).abs() < 1e-3);
                assert!(r.text().contains("120"), "{}", r.text());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(can_buy(1e9, SLOTS, it), Err(Refusal::Full), "full outranks gold: a sale is what fixes it");
        assert!(Refusal::Full.text().contains("trade"));
    }

    #[test]
    fn every_shelf_has_a_distinct_name_and_glyph() {
        let d = dens();
        for (i, a) in d.iter().enumerate() {
            assert!(!a.name.is_empty() && !a.name.contains(' '), "one word: {}", a.name);
            assert!(!a.blurb.is_empty());
            for b in &d[i + 1..] {
                assert_ne!(a.name, b.name);
                assert_ne!(a.glyph, b.glyph);
            }
        }
    }

    #[test]
    fn offers_cover_the_shelf_cheapest_first_and_mark_ownership() {
        let mut g = game();
        g.gods[0].gold = 1000.0;
        let first = offers(&g, 0, Cat::Strength);
        assert_eq!(first.len(), items::all().iter().filter(|i| i.cat == Cat::Strength).count());
        assert!(first.windows(2).all(|w| w[0].item.cost <= w[1].item.cost));
        let cheap = first[0].item;
        assert!(first[0].buyable());
        assert!(g.buy_item(0, cheap.id));
        let after = offers(&g, 0, Cat::Strength);
        let o = after.iter().find(|o| o.item.id == cheap.id).unwrap();
        assert_eq!(o.owned, Some(0));
        assert!(after.iter().any(|o| matches!(o.refusal, Some(Refusal::Gold { .. }))), "dear items are refused for gold");
    }

    #[test]
    fn recommendation_follows_the_role() {
        let g = game();
        let rec = recommended(&g, 0);
        for c in items::CATS {
            let marked = offers(&g, 0, c).iter().all(|o| o.recommended);
            assert_eq!(marked, rec.contains(&c), "{c:?}");
        }
    }

    #[test]
    fn the_suggestion_is_affordable_recommended_and_new() {
        let mut g = game();
        g.gods[0].gold = 0.0;
        assert!(suggestion(&g, 0).is_none(), "nothing to suggest with no gold");
        g.gods[0].gold = 1900.0;
        let s = suggestion(&g, 0).expect("something at 1900 gold");
        assert!(s.cost <= 1900.0);
        assert!(recommended(&g, 0).contains(&s.cat));
        assert!(g.buy_item(0, s.id));
        g.gods[0].gold = 1900.0;
        assert_ne!(suggestion(&g, 0).map(|i| i.id), Some(s.id), "not the one already held");
    }

    #[test]
    fn a_phone_gets_one_column_or_compact_chrome() {
        assert_eq!(layout_for(390.0, 844.0), Layout::Narrow, "portrait phone");
        assert_eq!(layout_for(844.0, 390.0), Layout::Short, "landscape phone");
        assert_eq!(layout_for(1280.0, 720.0), Layout::Wide);
        assert_eq!(layout_for(f32::NAN, 720.0), Layout::Narrow);
    }

    #[test]
    fn stat_tokens_match_the_stats() {
        let s = items::Stats { strength: 20.0, attack_speed: 0.05, ..items::Stats::ZERO };
        assert_eq!(stat_tokens(&s), vec!["+20 STR".to_string(), "+5% AS".to_string()]);
    }
}
