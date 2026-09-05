//! Land: builds that outlive the turn.
//!
//! Everything so far has been anonymous. A build happens, `last_build`
//! remembers its box until the next one replaces it, and then nothing.
//! That is enough for "make it taller" said immediately and nothing
//! else. It cannot answer "what is this thing", cannot let a second
//! player extend it, and cannot survive a logout.
//!
//! A WORKSET fixes that: an id, a name, an owner, a member list, a
//! bounding box, and THE CONVERSATION THAT MADE IT. The conversation is
//! the part that matters most. "Make this structure larger" is only
//! answerable if the original request is still around — the model needs
//! to know it was a watchtower, in stone, on a rise, before it can
//! sensibly enlarge it.
//!
//! Serialization is a hand-rolled line format rather than serde,
//! because this crate has no dependencies and should keep it that way.
//! The format is greppable, diffable, and survives a field being added.

use std::collections::BTreeMap;

/// Who may change a build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Not a member: may look, may not touch.
    None,
    /// Added by the owner: may build and extend.
    Member,
    /// Made it: may build, extend, and add members.
    Owner,
}

/// One turn of the conversation that shaped a build.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub player: String,
    /// What the player asked for, verbatim.
    pub prose: String,
    /// The IR that resulted. Kept because a follow-up often wants to
    /// know HOW something was made, not only what it was called.
    pub ir: String,
    pub blocks: usize,
}

/// A claimed build.
#[derive(Debug, Clone, PartialEq)]
pub struct Workset {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub members: Vec<String>,
    pub min: (i32, i32, i32),
    pub max: (i32, i32, i32),
    pub turns: Vec<Turn>,
}

impl Workset {
    pub fn access(&self, player: &str) -> Access {
        if player == self.owner { Access::Owner }
        else if self.members.iter().any(|m| m == player) { Access::Member }
        else { Access::None }
    }

    pub fn blocks(&self) -> usize { self.turns.iter().map(|t| t.blocks).sum() }

    pub fn contains(&self, x: i32, z: i32, pad: i32) -> bool {
        x >= self.min.0 - pad && x <= self.max.0 + pad
            && z >= self.min.2 - pad && z <= self.max.2 + pad
    }

    /// Grow the box to include a new edit. A build that gets extended
    /// stays one workset rather than spawning a second overlapping one.
    pub fn absorb(&mut self, min: (i32, i32, i32), max: (i32, i32, i32)) {
        self.min = (self.min.0.min(min.0), self.min.1.min(min.1), self.min.2.min(min.2));
        self.max = (self.max.0.max(max.0), self.max.1.max(max.1), self.max.2.max(max.2));
    }

    /// The one-line summary for `/land`.
    pub fn summary(&self) -> String {
        let (w, h, d) = (self.max.0 - self.min.0 + 1,
                         self.max.1 - self.min.1 + 1,
                         self.max.2 - self.min.2 + 1);
        format!("{} \"{}\" by {} -- {}x{}x{} at {:?}, {} blocks, {} turn(s){}",
                self.id, self.name, self.owner, w, h, d, self.min, self.blocks(),
                self.turns.len(),
                if self.members.is_empty() { String::new() }
                else { format!(", shared with {}", self.members.join(", ")) })
    }

    /// The history a follow-up needs. Trimmed: the last few turns carry
    /// almost all the meaning, and the whole log would crowd out the
    /// terrain read in a small context.
    pub fn history_for_prompt(&self, max_turns: usize) -> String {
        let start = self.turns.len().saturating_sub(max_turns);
        let mut s = format!("THIS IS \"{}\" ({}), built by {}. Its box is {:?}..{:?}.\n\
                             What was asked for so far:",
                            self.name, self.id, self.owner, self.min, self.max);
        for t in &self.turns[start..] {
            s.push_str(&format!("\n  {}: {}", t.player, t.prose));
        }
        s
    }
}

/// Every claim in a world.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Registry {
    sets: BTreeMap<String, Workset>,
    next: u32,
}

impl Registry {
    pub fn new() -> Self { Self::default() }

    pub fn len(&self) -> usize { self.sets.len() }
    pub fn is_empty(&self) -> bool { self.sets.is_empty() }
    pub fn get(&self, id: &str) -> Option<&Workset> { self.sets.get(id) }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Workset> { self.sets.get_mut(id) }
    pub fn all(&self) -> impl Iterator<Item = &Workset> { self.sets.values() }

    /// Claim a new build. Ids are short and typeable: a player has to
    /// enter one by hand in `/land add`.
    pub fn claim(&mut self, name: &str, owner: &str,
                 min: (i32, i32, i32), max: (i32, i32, i32)) -> String {
        self.next += 1;
        let id = format!("w{}", self.next);
        self.sets.insert(id.clone(), Workset {
            id: id.clone(), name: name.to_string(), owner: owner.to_string(),
            members: Vec::new(), min, max, turns: Vec::new(),
        });
        id
    }

    /// Worksets whose box covers a point, nearest-centre first.
    pub fn at(&self, x: i32, z: i32, pad: i32) -> Vec<&Workset> {
        let mut v: Vec<&Workset> = self.sets.values().filter(|w| w.contains(x, z, pad)).collect();
        // Distance to centre, then AREA. Two concentric claims have the
        // same centre distance, so distance alone leaves the order to
        // the map's iteration and picks the wrong one half the time.
        // The tightest claim covering a point is the one the player
        // means: a shed inside a town square is the shed.
        v.sort_by_key(|w| {
            let cx = (w.min.0 + w.max.0) / 2;
            let cz = (w.min.2 + w.max.2) / 2;
            let d = (cx - x).pow(2) + (cz - z).pow(2);
            let area = (w.max.0 - w.min.0 + 1) as i64 * (w.max.2 - w.min.2 + 1) as i64;
            (d as i64, area)
        });
        v
    }

    /// May this player build at this spot?
    ///
    /// Unclaimed land is open. Inside someone's claim, only the owner
    /// and members. Returns the blocking workset so the caller can say
    /// WHOSE land it is — "you cannot build here" without a name is the
    /// kind of message that makes people quit.
    pub fn may_build(&self, player: &str, x: i32, z: i32) -> Result<(), &Workset> {
        for w in self.at(x, z, 0) {
            if w.access(player) == Access::None { return Err(w); }
        }
        Ok(())
    }

    pub fn add_member(&mut self, id: &str, actor: &str, player: &str) -> Result<(), String> {
        let w = self.sets.get_mut(id).ok_or_else(|| format!("no workset '{id}'"))?;
        if w.access(actor) != Access::Owner {
            return Err(format!("only {} can share \"{}\"", w.owner, w.name));
        }
        if w.owner == player {
            return Err(format!("{player} already owns it"));
        }
        if w.members.iter().any(|m| m == player) {
            return Err(format!("{player} is already on \"{}\"", w.name));
        }
        w.members.push(player.to_string());
        Ok(())
    }

    pub fn record(&mut self, id: &str, turn: Turn,
                  min: (i32, i32, i32), max: (i32, i32, i32)) {
        if let Some(w) = self.sets.get_mut(id) {
            w.absorb(min, max);
            w.turns.push(turn);
        }
    }

    // ── persistence ──────────────────────────────────────────────
    //
    // A hand-rolled line format, not serde: this crate has no
    // dependencies and should keep it that way. Greppable, diffable,
    // and an unknown key is skipped rather than fatal, so a save from a
    // newer build still loads.

    pub fn save(&self) -> String {
        let mut s = String::from("# unendlich worksets v1\n");
        for w in self.sets.values() {
            s.push_str(&format!("set {}\n", w.id));
            s.push_str(&format!("name {}\n", escape(&w.name)));
            s.push_str(&format!("owner {}\n", escape(&w.owner)));
            for m in &w.members { s.push_str(&format!("member {}\n", escape(m))); }
            s.push_str(&format!("box {} {} {} {} {} {}\n",
                                w.min.0, w.min.1, w.min.2, w.max.0, w.max.1, w.max.2));
            for t in &w.turns {
                s.push_str(&format!("turn {} {} {}\n",
                                    t.blocks, escape(&t.player), escape(&t.prose)));
                for line in t.ir.lines() {
                    s.push_str(&format!("ir {}\n", escape(line)));
                }
            }
        }
        s
    }

    pub fn load(text: &str) -> Self {
        let mut r = Registry::new();
        let mut cur: Option<Workset> = None;
        let mut turn: Option<Turn> = None;
        let flush_turn = |cur: &mut Option<Workset>, turn: &mut Option<Turn>| {
            if let (Some(w), Some(t)) = (cur.as_mut(), turn.take()) { w.turns.push(t); }
        };
        for line in text.lines() {
            let line = line.trim_end();
            if line.starts_with('#') || line.is_empty() { continue; }
            let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "set" => {
                    flush_turn(&mut cur, &mut turn);
                    if let Some(w) = cur.take() { r.sets.insert(w.id.clone(), w); }
                    cur = Some(Workset {
                        id: rest.to_string(), name: String::new(), owner: String::new(),
                        members: Vec::new(), min: (0, 0, 0), max: (0, 0, 0),
                        turns: Vec::new() });
                    // Keep `next` ahead of every id seen, or a load
                    // followed by a claim reuses an id and two builds
                    // become one.
                    if let Some(n) = rest.strip_prefix('w').and_then(|s| s.parse::<u32>().ok()) {
                        r.next = r.next.max(n);
                    }
                }
                "name" => if let Some(w) = cur.as_mut() { w.name = unescape(rest); },
                "owner" => if let Some(w) = cur.as_mut() { w.owner = unescape(rest); },
                "member" => if let Some(w) = cur.as_mut() { w.members.push(unescape(rest)); },
                "box" => if let Some(w) = cur.as_mut() {
                    let n: Vec<i32> = rest.split_whitespace()
                        .filter_map(|v| v.parse().ok()).collect();
                    if n.len() == 6 {
                        w.min = (n[0], n[1], n[2]);
                        w.max = (n[3], n[4], n[5]);
                    }
                },
                "turn" => {
                    flush_turn(&mut cur, &mut turn);
                    let mut it = rest.splitn(3, ' ');
                    let blocks = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                    let player = unescape(it.next().unwrap_or(""));
                    let prose = unescape(it.next().unwrap_or(""));
                    turn = Some(Turn { player, prose, ir: String::new(), blocks });
                }
                // IR is stored one line per record and reassembled with
                // a TRAILING newline, matching the convention every
                // producer uses. Without it a save/load round trip
                // silently strips the last newline, and the parser's
                // last line then behaves differently after a reload
                // than it did before one.
                "ir" => if let Some(t) = turn.as_mut() {
                    t.ir.push_str(&unescape(rest));
                    t.ir.push('\n');
                },
                // An unknown key is SKIPPED, not fatal: a save written
                // by a newer build must still load in an older one.
                _ => {}
            }
        }
        flush_turn(&mut cur, &mut turn);
        if let Some(w) = cur.take() { r.sets.insert(w.id.clone(), w); }
        r
    }
}

/// Newlines would break the line format; a literal backslash-n would be
/// ambiguous with an escaped one.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut esc = false;
    for c in s.chars() {
        if esc {
            out.push(match c { 'n' => '\n', o => o });
            esc = false;
        } else if c == '\\' { esc = true; } else { out.push(c); }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(p: &str, prose: &str) -> Turn {
        Turn { player: p.into(), prose: prose.into(),
               ir: "cylinder @0,0 r=3 h=9\n".into(), blocks: 200 }
    }

    #[test]
    fn a_claim_gets_an_id_a_player_can_type() {
        let mut r = Registry::new();
        let id = r.claim("watchtower", "kess", (0, 64, 0), (9, 80, 9));
        assert_eq!(id, "w1");
        assert!(id.len() <= 6, "ids go into /land add by hand");
        assert_eq!(r.get(&id).unwrap().access("kess"), Access::Owner);
        assert_eq!(r.get(&id).unwrap().access("someone"), Access::None);
    }

    #[test]
    fn only_the_owner_can_share_and_members_can_then_build() {
        let mut r = Registry::new();
        let id = r.claim("tower", "kess", (0, 64, 0), (9, 80, 9));
        assert!(r.add_member(&id, "intruder", "friend").is_err(),
                "a stranger cannot add members");
        r.add_member(&id, "kess", "friend").unwrap();
        assert_eq!(r.get(&id).unwrap().access("friend"), Access::Member);
        assert!(r.may_build("friend", 4, 4).is_ok());
        assert!(r.may_build("stranger", 4, 4).is_err());
        // adding twice is refused, and so is adding the owner
        assert!(r.add_member(&id, "kess", "friend").is_err());
        assert!(r.add_member(&id, "kess", "kess").is_err());
    }

    #[test]
    fn a_refusal_names_whose_land_it_is() {
        // "You cannot build here" without a name is the message that
        // makes people quit.
        let mut r = Registry::new();
        r.claim("kess's tower", "kess", (0, 64, 0), (9, 80, 9));
        let blocked = r.may_build("stranger", 4, 4).unwrap_err();
        assert_eq!(blocked.owner, "kess");
        assert!(blocked.summary().contains("kess's tower"));
    }

    #[test]
    fn unclaimed_land_is_open() {
        let mut r = Registry::new();
        r.claim("tower", "kess", (0, 64, 0), (9, 80, 9));
        assert!(r.may_build("anyone", 500, 500).is_ok());
    }

    #[test]
    fn the_conversation_is_what_makes_a_follow_up_possible() {
        // "Make this structure larger" is answerable only if the
        // original request survives: the model has to know it was a
        // watchtower in stone before it can sensibly enlarge it.
        let mut r = Registry::new();
        let id = r.claim("watchtower", "kess", (0, 64, 0), (9, 80, 9));
        r.record(&id, turn("kess", "a small stone watchtower on this hill"),
                 (0, 64, 0), (9, 80, 9));
        r.record(&id, turn("kess", "add a door facing the path"), (0, 64, 0), (9, 80, 9));
        let h = r.get(&id).unwrap().history_for_prompt(5);
        assert!(h.contains("watchtower"));
        assert!(h.contains("a small stone watchtower on this hill"));
        assert!(h.contains("add a door"));
    }

    #[test]
    fn history_is_trimmed_so_it_cannot_crowd_out_the_terrain() {
        let mut r = Registry::new();
        let id = r.claim("t", "kess", (0, 0, 0), (1, 1, 1));
        for i in 0..40 {
            r.record(&id, turn("kess", &format!("change number {i}")), (0, 0, 0), (1, 1, 1));
        }
        let h = r.get(&id).unwrap().history_for_prompt(4);
        assert!(h.contains("change number 39"), "the recent turns are the ones kept");
        assert!(!h.contains("change number 3\n"), "the old ones are dropped");
        assert!(h.len() < 500, "and the block stays small: {} chars", h.len());
    }

    #[test]
    fn extending_a_build_grows_its_box_rather_than_making_a_second_one() {
        let mut r = Registry::new();
        let id = r.claim("tower", "kess", (0, 64, 0), (9, 80, 9));
        r.record(&id, turn("kess", "taller"), (0, 64, 0), (9, 95, 9));
        let w = r.get(&id).unwrap();
        assert_eq!(w.max.1, 95);
        assert_eq!(r.len(), 1, "one workset, not two overlapping");
    }

    #[test]
    fn a_saved_world_reloads_identically() {
        let mut r = Registry::new();
        let a = r.claim("watchtower", "kess", (0, 64, 0), (9, 80, 9));
        r.add_member(&a, "kess", "friend").unwrap();
        r.record(&a, turn("kess", "a small stone watchtower"), (0, 64, 0), (9, 80, 9));
        r.record(&a, turn("friend", "add a balcony"), (0, 64, 0), (12, 80, 9));
        r.claim("bridge", "friend", (20, 64, 0), (40, 70, 4));
        let text = r.save();
        let back = Registry::load(&text);
        assert_eq!(back, r, "a round trip must be exact");
    }

    #[test]
    fn loading_does_not_reuse_an_id_and_merge_two_builds() {
        let mut r = Registry::new();
        r.claim("a", "k", (0, 0, 0), (1, 1, 1));
        r.claim("b", "k", (9, 0, 9), (10, 1, 10));
        let mut back = Registry::load(&r.save());
        let fresh = back.claim("c", "k", (50, 0, 50), (51, 1, 51));
        assert_eq!(fresh, "w3", "the counter survives a reload");
        assert_eq!(back.len(), 3);
    }

    #[test]
    fn awkward_text_survives_the_line_format() {
        let mut r = Registry::new();
        let id = r.claim("the \"big\" hall\\north", "kess", (0, 0, 0), (1, 1, 1));
        r.record(&id, Turn {
            player: "kess".into(),
            prose: "make it\ntwo lines and a \\ backslash".into(),
            ir: "cylinder @0,0 r=2\ntree @4,4 oak\n".into(),
            blocks: 5,
        }, (0, 0, 0), (1, 1, 1));
        let back = Registry::load(&r.save());
        assert_eq!(back, r, "newlines and backslashes must round-trip");
        assert_eq!(back.get(&id).unwrap().turns[0].ir.lines().count(), 2);
    }

    #[test]
    fn an_unknown_key_from_a_newer_save_is_skipped_not_fatal() {
        let text = "# unendlich worksets v1\nset w1\nname hall\nowner kess\n\
                    lighting 42\nbox 0 0 0 4 4 4\n";
        let r = Registry::load(text);
        assert_eq!(r.len(), 1);
        assert_eq!(r.get("w1").unwrap().name, "hall");
        assert_eq!(r.get("w1").unwrap().max, (4, 4, 4));
    }

    #[test]
    fn overlapping_claims_list_nearest_first() {
        let mut r = Registry::new();
        r.claim("far", "a", (0, 0, 0), (100, 10, 100));
        r.claim("near", "b", (48, 0, 48), (52, 10, 52));
        let hits = r.at(50, 50, 0);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].name, "near", "the tightest claim is the one you meant");
    }
}
