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
    Curio,
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
        Cat::Curio => ("Curios", "odd treasures, each with a trick of its own", Glyph::Curio),
    };
    Den { cat, name, blurb, glyph, color: cat.color() }
}

/// Every shelf, in catalog order.
pub fn dens() -> [Den; 9] {
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

/// What a point of each stat is worth, in list-price gold, as the Sapling
/// items price it (the going rate the shop's own cheapest gifts set):
/// (STR, INT, PWR, PROT, PEN %-point, flat PEN, AS %-point, HP, MP, CDR
/// %-point, MS %-point).
pub const GOING_RATE: [f32; 11] = [33.0, 30.0, 50.0, 35.0, 11.5, 25.0, 60.0, 4.2, 2.0, 35.0, 45.0];

/// The gold an item's stats alone would cost at [`GOING_RATE`], at today's
/// prices (the list rate times `items::PRICE_CUT`).
pub fn stat_worth(st: &items::Stats) -> f32 {
    let r = GOING_RATE;
    let v = st.strength * r[0] + st.intelligence * r[1] + st.power * r[2] + st.protection * r[3] + st.pen_pct * 100.0 * r[4] + st.pen_flat * r[5] + st.attack_speed * 100.0 * r[6] + st.health * r[7] + st.mana * r[8] + st.cdr * 100.0 * r[9] + st.move_speed * 100.0 * r[10];
    v * items::PRICE_CUT
}

/// One part of an item's explanation: a heading and what it says.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub head: &'static str,
    pub text: String,
}

/// Everything the shop can say about `item` to the cat `player`: what it
/// is, what each stat does in the game's own formulas (worked out on the
/// player's current numbers, so "+30 Protection" becomes "blows land at 71%
/// instead of 78%"), how a trophy fills, what a Curio's trick comes to, who
/// builds it, and what its gold buys against the going rate.
pub fn explain(game: &Game, player: usize, item: &Item) -> Vec<Part> {
    let g = &game.gods[player];
    let def = game.def(player);
    let st = &item.stats;
    let mut out = vec![Part { head: "The gift", text: item.desc.to_string() }];
    let mut does: Vec<String> = Vec::new();
    // power: what STR, INT and PWR come to (sim::recalc)
    let dp = st.power + st.intelligence * 0.9 + st.strength * 0.6;
    if dp > 0.0 {
        let scales: Vec<f32> = def.abilities.iter().map(|a| a.power_scale).filter(|x| *x > 0.0).collect();
        let mean = if scales.is_empty() { 0.0 } else { scales.iter().sum::<f32>() / scales.len() as f32 };
        let mut t = format!("Power {:.0} -> {:.0} (STR counts 0.6, INT 0.9, PWR 1 each). Your basic attacks deal 22% of power: +{:.1} a blow.", g.power, g.power + dp, dp * 0.22);
        if mean > 0.0 {
            t += &format!(" Your abilities add {:.0}% of power on average: about +{:.0} a cast.", mean * 100.0, dp * mean);
        }
        does.push(t);
    }
    if st.protection > 0.0 {
        let now = 100.0 / (100.0 + g.prot);
        let then = 100.0 / (100.0 + g.prot + st.protection);
        does.push(format!("Protection {:.0} -> {:.0}: blows land at {:.0}% of their strength instead of {:.0}% (100 / (100 + protection)), so you last {:.0}% longer.", g.prot, g.prot + st.protection, then * 100.0, now * 100.0, (now / then - 1.0) * 100.0));
    }
    if st.pen_pct > 0.0 || st.pen_flat > 0.0 {
        let foes: Vec<f32> = game.gods.iter().filter(|o| o.team != g.team).map(|o| o.prot).collect();
        let prot = if foes.is_empty() { 60.0 } else { foes.iter().sum::<f32>() / foes.len() as f32 };
        let hit = |pp: f32, pf: f32| 100.0 / (100.0 + (prot * (1.0 - pp) - pf).max(0.0));
        let (a, b) = (hit(g.pen_pct, g.pen_flat), hit(g.pen_pct + st.pen_pct, g.pen_flat + st.pen_flat));
        does.push(format!("Penetration ignores part of their protection. Against the other side's average of {prot:.0}, your hits land at {:.0}% instead of {:.0}%: {:+.0}% damage to cats.", b * 100.0, a * 100.0, (b / a - 1.0) * 100.0));
    }
    if st.attack_speed > 0.0 {
        let base = def.auto_cd;
        let (a, b) = (base / (1.0 + g.atk_speed), base / (1.0 + g.atk_speed + st.attack_speed));
        does.push(format!("A basic attack every {a:.2} s becomes every {b:.2} s: {:.1} more blows a minute.", 60.0 / b - 60.0 / a));
    }
    if st.health > 0.0 {
        does.push(format!("Health {:.0} -> {:.0} ({:+.0}%).", g.max_hp, g.max_hp + st.health, st.health / g.max_hp.max(1.0) * 100.0));
    }
    if st.mana > 0.0 {
        does.push(format!("Mana {:.0} -> {:.0}: about {:.1} more casts of your dearest ability.", g.max_mana, g.max_mana + st.mana, st.mana / def.abilities.iter().map(|a| a.cost).fold(1.0f32, f32::max)));
    }
    if st.cdr > 0.0 {
        let now = g.cdr.min(0.4);
        let then = (g.cdr + st.cdr).min(0.4);
        let lost = (g.cdr + st.cdr - 0.4).max(0.0);
        let mut t = format!("Cooldowns {:.0}% shorter -> {:.0}% shorter (the most is 40%).", now * 100.0, then * 100.0);
        if lost > 0.001 {
            t += &format!(" {:.0} points of it would be wasted over the cap.", lost * 100.0);
        }
        does.push(t);
    }
    if st.move_speed > 0.0 {
        does.push(format!("{:.0}% faster on your feet: {:.1} u a second more.", st.move_speed * 100.0, def.move_speed * st.move_speed));
    }
    if !does.is_empty() {
        out.push(Part { head: "For you, now", text: does.join(" ") });
    }
    if let Some(sk) = &item.stack {
        let what = match sk.grows {
            items::Grow::Strength => "Strength",
            items::Grow::Intelligence => "Intelligence",
            items::Grow::Power => "Power",
            items::Grow::Protection => "Protection",
            items::Grow::Health => "Health",
        };
        let how = match sk.kind {
            items::StackKind::MinionKill => format!("{:.0} minions you finish", sk.cap()),
            items::StackKind::GodKill => format!("{:.0} takedowns of enemy ancestors (kills or assists)", sk.cap()),
            items::StackKind::TowerKill => format!("{:.0} towers your side fells", sk.cap()),
            items::StackKind::Time => format!("{:.0} minutes held", sk.cap() * sk.interval / 60.0),
        };
        out.push(Part { head: "It grows", text: format!("+{} {what} each time, up to +{:.0} more after {how}. Bought early, it pays most; sold, its growth is lost.", fmt_num(sk.per), sk.max) });
    }
    if let Some(pv) = item.passive {
        out.push(Part { head: "Its trick", text: trick(game, player, pv) });
    }
    // who builds it
    let roles: Vec<&str> = [
        (crate::gods::Role::Warrior, "Warriors"),
        (crate::gods::Role::Assassin, "Assassins"),
        (crate::gods::Role::Guardian, "Guardians"),
        (crate::gods::Role::Mage, "Mages"),
        (crate::gods::Role::Hunter, "Hunters"),
    ]
    .iter()
    .filter(|(r, _)| items::role_build_order(*r)[..2].contains(&item.cat))
    .map(|(_, n)| *n)
    .collect();
    let mine = recommended(game, player).contains(&item.cat);
    let who = if item.cat == Cat::Curio {
        "Any cat whose plan its trick fits (above).".to_string()
    } else if roles.is_empty() {
        "No role builds this shelf first; a pick for a particular fight.".to_string()
    } else {
        format!("{} build this shelf first.", roles.join(" and "))
    };
    out.push(Part { head: "Who wants it", text: format!("{who}{}", if mine { " So does your role." } else { "" }) });
    // when: the tier's place in a build, and how far the player is from it
    let refund = (item.cost * crate::consts::SELL_REFUND).round();
    let when = match item.tier {
        0 | 1 => "A Sapling: cheap enough for your first trip home, and the first slot to trade away when the hoard fills",
        2 => "A Bough: the middle of a build, bought once the first Saplings are in",
        _ => "An Elder: a late, finishing piece -- save for it rather than buying a Sapling you will sell",
    };
    let short = item.cost - g.gold;
    let reach = if short <= 0.0 {
        "You can afford it now.".to_string()
    } else {
        let per = (crate::consts::GOLD_MELEE + crate::consts::GOLD_CASTER) * 0.5;
        format!("You are {short:.0} gold short -- about {:.0} minions' worth.", (short / per).ceil())
    };
    out.push(Part { head: "When", text: format!("{when}. {reach} Traded back, it returns {refund:.0}.") });
    // what the gold buys
    let worth = stat_worth(st);
    let pct = worth / item.cost.max(1.0) * 100.0;
    let text = if item.passive.is_some() || item.stack.is_some() {
        format!("Its stats alone are worth about {worth:.0} gold at the going rate ({pct:.0}% of its {:.0}); the rest of the price is the {}.", item.cost, if item.passive.is_some() { "trick" } else { "growth" })
    } else {
        format!("About {worth:.0} gold of stats at the going rate for its {:.0}: {}.", item.cost, if pct >= 104.0 { "a bargain" } else if pct >= 96.0 { "a fair trade" } else { "you pay a little for having it all in one slot" })
    };
    out.push(Part { head: "Worth", text });
    out
}

fn ordinal(n: u32) -> String {
    match n {
        2 => "second".into(),
        3 => "third".into(),
        4 => "fourth".into(),
        5 => "fifth".into(),
        n => format!("{n}th"),
    }
}

fn fmt_num(x: f32) -> String {
    if (x - x.round()).abs() < 1e-3 { format!("{x:.0}") } else { format!("{x:.1}") }
}

/// A Curio's passive in words, with its numbers worked for this cat.
fn trick(game: &Game, player: usize, pv: items::Passive) -> String {
    use items::Passive::*;
    let g = &game.gods[player];
    match pv {
        NinthLife { hp, cd } => format!("The next blow that would kill you leaves you standing at {:.0}% health ({:.0} of your {:.0}). It rings once every {cd:.0} s; after it has saved you, the next death is real. Best for a cat the enemy dives first.", hp * 100.0, g.max_hp * hp, g.max_hp),
        Censer { radius, ms, atk } => format!("An aura {radius:.0} u across: you and every allied cat inside move {:.0}% and attack {:.0}% faster. It does not stack with a second censer, so one per team. Best on a cat that fights beside others.", ms * 100.0, atk * 100.0),
        Pounce { every, bonus, scale } => format!("Blows are counted; every {} deals {bonus:.0} + {:.0}% of power extra -- {:.0} at your {:.0} power, on top of the blow. Faster attacks reach it sooner.", ordinal(every), scale * 100.0, bonus + scale * g.power, g.power),
        Moonmilk { after, pct } => format!("Untouched for {after:.1} s (normally 6), you mend {:.1}% of health a second (normally 1.2%): {:.0} a second for you, a full heal in about {:.0} s. For cats that duck out of a fight and come back.", pct * 100.0, g.max_hp * pct, 1.0 / pct),
        Thorns { pct } => format!("{:.0}% of every blow an enemy cat lands on you goes back to it, through its protection. A cat hitting you for 200 takes {:.0} itself. Best against those who must hit you to win.", pct * 100.0, 200.0 * pct),
        Wander { radius, ms } => format!("{:.0}% faster while no enemy cat is within {radius:.0} u: for crossing the map to a fight. It drops away the moment you are close to one, so it will not win a chase.", ms * 100.0),
        Tide { pct } => format!("Every ability that hits an enemy cat heals you {:.0}% of the damage it did: a 300-damage burst mends {:.0}. Basic attacks do not count.", pct * 100.0, 300.0 * pct),
        Bramble { slow, secs } => format!("Each basic attack that lands on a cat slows it {:.0}% for {secs:.1} s, refreshed by the next. At your {:.2} s between blows a target {} slowed.", slow * 100.0, game.auto_interval(player), if game.auto_interval(player) <= secs { "stays" } else { "is often" }),
    }
}

/// The first sentence of `text` (to its first ". "), for a card with room
/// for a line; the study pane shows the rest.
pub fn first_sentence(text: &str) -> &str {
    match text.find(". ") {
        Some(i) => &text[..=i],
        None => text,
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

    /// Every item gets a full explanation: the gift, what its stats do for
    /// this cat, how it grows (trophies), its trick (Curios), who wants it
    /// and what its gold buys -- and no number in it is NaN or infinite.
    #[test]
    fn every_item_is_explained_in_full() {
        let g = game();
        for item in items::all() {
            let parts = explain(&g, 0, item);
            let heads: Vec<&str> = parts.iter().map(|p| p.head).collect();
            assert_eq!(heads[0], "The gift");
            assert!(heads.contains(&"Who wants it") && heads.contains(&"Worth"), "{}: {heads:?}", item.name);
            assert_eq!(heads.contains(&"For you, now"), stat_tokens(&item.stats).len() > 0, "{}", item.name);
            assert_eq!(heads.contains(&"It grows"), item.stack.is_some(), "{}", item.name);
            assert_eq!(heads.contains(&"Its trick"), item.passive.is_some(), "{}", item.name);
            let all: String = parts.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join(" ");
            assert!(!all.contains("NaN") && !all.contains("inf"), "{}: {all}", item.name);
            assert!(all.len() > 200, "{}: only {} characters", item.name, all.len());
        }
    }

    /// The numbers are the game's: a protection item's "blows land at"
    /// figure is 100 / (100 + protection) with and without it.
    #[test]
    fn the_explanation_works_the_real_formula() {
        let g = game();
        let item = items::all().iter().find(|i| i.name == "Stonehide Mail").unwrap();
        let text = explain(&g, 0, item).into_iter().find(|p| p.head == "For you, now").unwrap().text;
        let then = 100.0 / (100.0 + g.gods[0].prot + item.stats.protection) * 100.0;
        assert!(text.contains(&format!("land at {then:.0}%")), "{text}");
        assert!(stat_worth(&item.stats) > 0.0);
    }

    #[test]
    fn stat_tokens_match_the_stats() {
        let s = items::Stats { strength: 20.0, attack_speed: 0.05, ..items::Stats::ZERO };
        assert_eq!(stat_tokens(&s), vec!["+20 STR".to_string(), "+5% AS".to_string()]);
    }
}
