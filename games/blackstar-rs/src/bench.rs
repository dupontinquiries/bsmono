//! AUTHORING — rolling a bot, keeping it, and sending it to somebody.
//!
//! `[` and `]` already cycle the chassis. The authoring keys sit next to
//! them, which is the right place for them: **`0` rerolls** and **`s` saves
//! to a slot**. Both are free today — checked, not assumed.
//!
//! ## A character is a seed, not a mesh
//!
//! This is the decision everything else follows from. A generated frame could
//! be stored as the voxels it came out as, and then sharing one means sending
//! a model file. Storing the SEED instead makes a character a short string
//! somebody can paste into a chat window, and makes rerolling free.
//!
//! The cost is that a seed only means something against the checkpoint that
//! produced it. So an export carries the checkpoint's fingerprint, and
//! importing against a different one is a **mismatch that is reported rather
//! than silently rendering somebody else's bot wrong**. A shared character
//! that quietly comes out different is worse than one that refuses.
//!
//! ## It has to work with no checkpoint at all
//!
//! `BLACKSTAR_CHECKPOINT_FILE=/path/to/file.safetensors` points at a trained
//! generator. Most people will not have one, and the feature cannot be a dead
//! button for them: with no checkpoint the roll falls back to the parts
//! library, which is real geometry and always present. Same keys, same slots,
//! same export — a narrower space of results.
//!
//! That fallback is not a consolation prize. `voxel.rs` measured the critics
//! separating real parts from noise by about five percent, which is to say
//! the learned generator has to be good before it beats picking real parts,
//! and until it is, the library IS the better generator.

use std::fmt::Write as _;

/// Where a trained generator is found.
pub const CHECKPOINT_ENV: &str = "BLACKSTAR_CHECKPOINT_FILE";
/// How many authored characters a player keeps.
pub const SLOTS: usize = 8;
/// Bumped when the export format changes shape.
pub const FORMAT: u32 = 1;

/// What produced a character.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Source {
    /// a trained generator, identified by a fingerprint of its checkpoint
    Model { fingerprint: u32 },
    /// assembled from `catalog/representatives`; always available
    Library,
}

impl Source {
    /// Short tag for the export string.
    pub fn tag(&self) -> String {
        match self {
            Source::Model { fingerprint } => format!("m{fingerprint:08x}"),
            Source::Library => "lib".into(),
        }
    }
    pub fn parse(s: &str) -> Option<Source> {
        if s == "lib" {
            return Some(Source::Library);
        }
        let hex = s.strip_prefix('m')?;
        u32::from_str_radix(hex, 16).ok().map(|fingerprint| Source::Model { fingerprint })
    }
}

/// An authored frame: everything needed to reproduce it, and nothing else.
#[derive(Clone, PartialEq, Debug)]
pub struct Character {
    pub seed: u64,
    pub source: Source,
    /// what the player called it; may be empty
    pub name: String,
}

impl Character {
    pub fn new(seed: u64, source: Source) -> Character {
        Character { seed, source, name: String::new() }
    }

    /// The share string. Short enough to paste into a message.
    ///
    /// `bsc1-<seed hex>-<source>-<name>`; the name is last so it can contain
    /// dashes without breaking the parse.
    pub fn export(&self) -> String {
        let mut s = String::new();
        let _ = write!(s, "bsc{FORMAT}-{:x}-{}", self.seed, self.source.tag());
        if !self.name.is_empty() {
            let _ = write!(s, "-{}", sanitise(&self.name));
        }
        s
    }

    pub fn import(text: &str) -> Result<Character, ImportError> {
        let t = text.trim();
        let rest = t.strip_prefix("bsc").ok_or(ImportError::NotACharacter)?;
        let (ver, rest) = rest.split_once('-').ok_or(ImportError::NotACharacter)?;
        let ver: u32 = ver.parse().map_err(|_| ImportError::NotACharacter)?;
        if ver != FORMAT {
            return Err(ImportError::WrongFormat(ver));
        }
        let (seed_hex, rest) = rest.split_once('-').ok_or(ImportError::NotACharacter)?;
        let seed = u64::from_str_radix(seed_hex, 16).map_err(|_| ImportError::NotACharacter)?;
        let (src, name) = match rest.split_once('-') {
            Some((s, n)) => (s, n.to_string()),
            None => (rest, String::new()),
        };
        let source = Source::parse(src).ok_or(ImportError::NotACharacter)?;
        Ok(Character { seed, source, name })
    }

    /// Can this be rendered with what is loaded right now?
    ///
    /// A shared character that quietly comes out DIFFERENT is worse than one
    /// that refuses, so this is checked before anything is drawn.
    pub fn playable_with(&self, have: &Source) -> Playable {
        match (&self.source, have) {
            (Source::Library, _) => Playable::Yes,
            (Source::Model { fingerprint: a }, Source::Model { fingerprint: b }) if a == b => {
                Playable::Yes
            }
            (Source::Model { fingerprint }, _) => Playable::NeedsCheckpoint(*fingerprint),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Playable {
    Yes,
    /// the seed means nothing without this checkpoint
    NeedsCheckpoint(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImportError {
    NotACharacter,
    /// from a newer or older build
    WrongFormat(u32),
}

impl ImportError {
    pub fn message(self) -> String {
        match self {
            ImportError::NotACharacter => "that is not a character code".into(),
            ImportError::WrongFormat(v) => {
                format!("that character is format {v} and this build reads {FORMAT}")
            }
        }
    }
}

/// Names go into a share string, so they cannot contain its separator.
fn sanitise(n: &str) -> String {
    n.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(24)
        .collect()
}

/// The player's slots, and the roll in progress.
pub struct Bench {
    /// what a checkpoint, if any, was loaded as
    pub source: Source,
    /// the frame currently on the turntable
    pub current: Character,
    pub slots: [Option<Character>; SLOTS],
    /// rolls since the bench opened, for the seed stream
    rolls: u64,
}

impl Bench {
    /// Open the bench, reading `BLACKSTAR_CHECKPOINT_FILE`.
    pub fn open(checkpoint: Option<&str>) -> Bench {
        let source = match checkpoint {
            Some(p) if !p.is_empty() => Source::Model { fingerprint: fingerprint(p) },
            _ => Source::Library,
        };
        Bench {
            current: Character::new(seed_from(0, &source), source.clone()),
            source,
            slots: Default::default(),
            rolls: 0,
        }
    }

    /// From the environment, which is how the game will call it.
    pub fn from_env() -> Bench {
        Bench::open(std::env::var(CHECKPOINT_ENV).ok().as_deref())
    }

    /// `0` — a new frame.
    pub fn reroll(&mut self) -> &Character {
        self.rolls += 1;
        self.current = Character::new(seed_from(self.rolls, &self.source), self.source.clone());
        &self.current
    }

    /// `s` — keep this one. Returns the slot it went to, or `None` when full.
    ///
    /// Refuses rather than overwriting. Losing an authored character to a
    /// keypress is the one failure this whole feature cannot afford, and
    /// "the bench is full" is a thing a player can act on.
    pub fn save(&mut self, name: &str) -> Option<usize> {
        let i = self.slots.iter().position(Option::is_none)?;
        let mut c = self.current.clone();
        c.name = sanitise(name);
        self.slots[i] = Some(c);
        Some(i)
    }

    pub fn clear(&mut self, i: usize) -> bool {
        if i < SLOTS && self.slots[i].is_some() {
            self.slots[i] = None;
            return true;
        }
        false
    }

    pub fn full(&self) -> bool {
        self.slots.iter().all(Option::is_some)
    }

    pub fn kept(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// Bring somebody else's character in.
    pub fn adopt(&mut self, code: &str) -> Result<(usize, Playable), ImportError> {
        let c = Character::import(code)?;
        let p = c.playable_with(&self.source);
        let i = self.slots.iter().position(Option::is_none).ok_or(ImportError::NotACharacter)?;
        self.slots[i] = Some(c);
        Ok((i, p))
    }

    /// The whole bench as save-file text.
    pub fn line(&self) -> String {
        self.slots
            .iter()
            .filter_map(|s| s.as_ref().map(|c| c.export()))
            .collect::<Vec<_>>()
            .join(",")
    }

    pub fn from_line(line: &str, checkpoint: Option<&str>) -> Bench {
        let mut b = Bench::open(checkpoint);
        for (i, part) in line.split(',').filter(|p| !p.is_empty()).enumerate() {
            if i >= SLOTS {
                break;
            }
            if let Ok(c) = Character::import(part) {
                b.slots[i] = Some(c);
            }
        }
        b
    }
}

/// A checkpoint's identity, from its path.
///
/// A path rather than the file's contents: reading a multi-gigabyte
/// safetensors to hash it, at startup, to decide whether a share code
/// matches, is not a trade worth making. A path collision produces a wrong
/// "playable" answer; hashing the file makes every launch slower for
/// everybody. If it becomes a real problem the fix is a small header hash,
/// not the whole file.
pub fn fingerprint(path: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in path.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    h
}

/// Seeds for successive rolls. Deterministic per source, so two people with
/// the same checkpoint see the same first frame.
fn seed_from(n: u64, source: &Source) -> u64 {
    let base = match source {
        Source::Model { fingerprint } => *fingerprint as u64,
        Source::Library => 0x1B_1A_C5,
    };
    let mut s = base.wrapping_mul(0x9E3779B97F4A7C15) ^ n.wrapping_mul(0xBF58476D1CE4E5B9);
    s ^= s >> 30;
    s = s.wrapping_mul(0xBF58476D1CE4E5B9);
    s ^= s >> 27;
    s
}

// ------------------------------------------------------------- driver ---

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--bench").unwrap_or(0);
    let code = argv.get(i + 1).map(String::as_str);

    let ck = std::env::var(CHECKPOINT_ENV).ok();
    println!("THE BENCH — roll a frame, keep it, send it\n");
    match &ck {
        Some(p) => println!("  {CHECKPOINT_ENV}={p}"),
        None => {
            println!("  {CHECKPOINT_ENV} is not set, so the roll comes from the parts");
            println!("  library. Same keys, same slots, same export — a narrower space.");
            println!("  That is not a consolation prize: `--voxel` measured the critics");
            println!("  separating real parts from noise by five percent, so a trained");
            println!("  generator has to be good before it beats picking real parts.");
        }
    }

    let mut b = Bench::from_env();
    println!("\n  source: {:?}\n", b.source);

    if let Some(code) = code {
        println!("  IMPORTING {code}\n");
        match b.adopt(code) {
            Err(e) => println!("    refused: {}", e.message()),
            Ok((slot, Playable::Yes)) => println!("    slot {slot}, and it will render"),
            Ok((slot, Playable::NeedsCheckpoint(f))) => {
                println!("    slot {slot}, but it needs checkpoint m{f:08x}");
                println!("    and this one is {}. NOT rendered — a shared character", b.source.tag());
                println!("    that quietly comes out different is worse than one that refuses.");
            }
        }
        return;
    }

    println!("  [ and ] cycle the chassis. 0 rerolls, s saves.\n");
    for _ in 0..4 {
        let c = b.reroll().clone();
        println!("    0  ->  seed {:016x}   {}", c.seed, c.export());
    }
    for (n, name) in [(1, "spanwright"), (2, "toll"), (3, "offcut")] {
        let _ = n;
        b.reroll();
        match b.save(name) {
            Some(i) => println!("    s  ->  slot {i}: {}", b.slots[i].as_ref().unwrap().export()),
            None => println!("    s  ->  the bench is full"),
        }
    }

    println!("\n  THE BENCH ({}/{} kept)\n", b.kept(), SLOTS);
    for (i, s) in b.slots.iter().enumerate() {
        match s {
            Some(c) => println!("    {i}  {:<28} {}", c.name, c.export()),
            None => println!("    {i}  —"),
        }
    }

    let shared = b.slots[0].as_ref().unwrap().export();
    println!("\n  SENDING ONE TO A FRIEND\n");
    println!("    {shared}");
    println!("\n  a character is a SEED, not a mesh — short enough to paste into a");
    println!("  message, and rerolling costs nothing. The price is that a seed only");
    println!("  means something against the checkpoint that made it, so the code");
    println!("  carries a fingerprint and a mismatch is reported rather than");
    println!("  rendered wrong.");

    // the round trip, not just the line — a bench that serialises and does
    // not come back is the `finds` bug again
    let line = b.line();
    println!("\n  AND IT SURVIVES A SAVE FILE\n    {line}");
    let back = Bench::from_line(&line, std::env::var(CHECKPOINT_ENV).ok().as_deref());
    println!(
        "    reloaded {}/{} — slot 0 is {}",
        back.kept(),
        SLOTS,
        back.slots[0].as_ref().map(|c| c.name.as_str()).unwrap_or("—")
    );

    // and the guard against the failure this feature cannot afford
    let mut fullb = Bench::open(None);
    for _ in 0..SLOTS {
        fullb.reroll();
        fullb.save("x");
    }
    println!("\n  WHEN THE BENCH IS FULL\n");
    fullb.reroll();
    println!("    full: {}   s -> {:?}", fullb.full(), fullb.save("one more"));
    println!("    it refuses rather than overwriting. losing an authored character");
    println!("    to a keypress is the one failure this cannot afford.");
    fullb.clear(3);
    println!("    clearing slot 3 makes room: s -> {:?}", fullb.save("now"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_round_trips_through_its_share_code() {
        for c in [
            Character::new(0x1234_5678_9ABC_DEF0, Source::Library),
            Character { seed: 42, source: Source::Model { fingerprint: 0xDEADBEEF }, name: "toll".into() },
            Character::new(0, Source::Library),
            Character { seed: u64::MAX, source: Source::Library, name: "a".into() },
        ] {
            let code = c.export();
            let back = Character::import(&code).unwrap_or_else(|e| panic!("{code}: {e:?}"));
            assert_eq!(back, c, "via {code}");
        }
    }

    #[test]
    fn a_share_code_is_short_enough_to_paste() {
        let c = Character { seed: u64::MAX, source: Source::Model { fingerprint: u32::MAX }, name: "averylongcharactername".into() };
        assert!(c.export().len() < 60, "{} chars: {}", c.export().len(), c.export());
        assert!(!c.export().contains(' '), "a code with a space will not survive a chat window");
    }

    #[test]
    fn a_name_cannot_break_the_parse() {
        // the separator is a dash and names are last, but a name full of
        // dashes should still come back as itself
        let c = Character { seed: 7, source: Source::Library, name: "a-b-c d!".into() };
        let back = Character::import(&c.export()).unwrap();
        assert_eq!(back.name, "a_b_c_d_");
        assert_eq!(back.seed, 7);
    }

    #[test]
    fn a_mismatched_checkpoint_refuses_rather_than_rendering_wrong() {
        // THE RULE. A shared character that quietly comes out different is
        // worse than one that will not open.
        let mine = Source::Model { fingerprint: 1111 };
        let theirs = Character::new(9, Source::Model { fingerprint: 2222 });
        assert_eq!(theirs.playable_with(&mine), Playable::NeedsCheckpoint(2222));
        assert_eq!(theirs.playable_with(&Source::Library), Playable::NeedsCheckpoint(2222));
        // the same checkpoint is fine
        let ours = Character::new(9, Source::Model { fingerprint: 1111 });
        assert_eq!(ours.playable_with(&mine), Playable::Yes);
        // and a library character plays for everyone, which is the point of
        // the fallback existing at all
        let lib = Character::new(9, Source::Library);
        assert_eq!(lib.playable_with(&mine), Playable::Yes);
        assert_eq!(lib.playable_with(&Source::Library), Playable::Yes);
    }

    #[test]
    fn rubbish_is_refused_with_something_a_player_can_read() {
        for bad in ["", "hello", "bsc1", "bsc1-", "bsc1-zzz-lib", "bsc1-ff-nope"] {
            let e = Character::import(bad).unwrap_err();
            assert!(!e.message().is_empty(), "{bad} gave no message");
        }
        // a version from another build says so specifically
        match Character::import("bsc9-ff-lib") {
            Err(ImportError::WrongFormat(9)) => {}
            other => panic!("expected a format error, got {other:?}"),
        }
    }

    #[test]
    fn the_bench_works_with_no_checkpoint_at_all() {
        // most people will not have one, and the feature cannot be a dead
        // button for them
        let mut b = Bench::open(None);
        assert_eq!(b.source, Source::Library);
        let first = b.current.clone();
        b.reroll();
        assert_ne!(b.current.seed, first.seed, "rerolling did nothing");
        assert_eq!(b.save("a"), Some(0));
        assert_eq!(b.kept(), 1);
        assert_eq!(b.slots[0].as_ref().unwrap().source, Source::Library);
    }

    #[test]
    fn saving_refuses_when_full_rather_than_overwriting() {
        // losing an authored character to a keypress is the one failure this
        // feature cannot afford
        let mut b = Bench::open(None);
        for i in 0..SLOTS {
            b.reroll();
            assert_eq!(b.save("x"), Some(i));
        }
        assert!(b.full());
        b.reroll();
        assert_eq!(b.save("one more"), None, "it overwrote a slot");
        assert_eq!(b.kept(), SLOTS);
        // clearing makes room again
        assert!(b.clear(3));
        assert_eq!(b.save("now"), Some(3));
        assert!(!b.clear(99), "cleared a slot that does not exist");
    }

    #[test]
    fn two_people_with_the_same_checkpoint_see_the_same_first_frame() {
        // deterministic per source, so "roll five times and look at the
        // fourth" is a thing one player can tell another
        let a = Bench::open(Some("/models/frames-v3.safetensors"));
        let b = Bench::open(Some("/models/frames-v3.safetensors"));
        assert_eq!(a.current, b.current);
        let c = Bench::open(Some("/models/frames-v4.safetensors"));
        assert_ne!(a.current.seed, c.current.seed, "different checkpoints, same seed");
        // and the library stream is its own
        assert_ne!(Bench::open(None).current.seed, a.current.seed);
    }

    #[test]
    fn rolls_do_not_repeat_in_any_reasonable_session() {
        let mut b = Bench::open(None);
        let mut seen = std::collections::BTreeSet::new();
        seen.insert(b.current.seed);
        for _ in 0..5000 {
            b.reroll();
            assert!(seen.insert(b.current.seed), "a seed came round again");
        }
    }

    #[test]
    fn the_bench_survives_a_save_file() {
        let mut b = Bench::open(None);
        for n in ["toll", "offcut", "gauge"] {
            b.reroll();
            b.save(n);
        }
        let line = b.line();
        let back = Bench::from_line(&line, None);
        assert_eq!(back.kept(), 3);
        for i in 0..3 {
            assert_eq!(back.slots[i], b.slots[i], "slot {i} did not survive");
        }
        // an empty line is an empty bench, not a panic
        assert_eq!(Bench::from_line("", None).kept(), 0);
        // and one corrupt entry does not lose the others
        let damaged = format!("{},rubbish", line);
        assert_eq!(Bench::from_line(&damaged, None).kept(), 3);
    }

    #[test]
    fn adopting_somebody_elses_character_keeps_the_verdict() {
        let mut b = Bench::open(None);
        let theirs = Character { seed: 5, source: Source::Model { fingerprint: 77 }, name: "spanwright".into() };
        let (slot, p) = b.adopt(&theirs.export()).unwrap();
        assert_eq!(b.slots[slot].as_ref().unwrap().name, "spanwright");
        assert_eq!(p, Playable::NeedsCheckpoint(77), "it was accepted as renderable");
        // it is KEPT even though it cannot be rendered — you do not lose a
        // friend's character because you have not downloaded a file
        assert_eq!(b.kept(), 1);
    }

    #[test]
    fn a_checkpoint_fingerprint_is_stable_and_distinguishes() {
        assert_eq!(fingerprint("/a/b.safetensors"), fingerprint("/a/b.safetensors"));
        assert_ne!(fingerprint("/a/b.safetensors"), fingerprint("/a/c.safetensors"));
        assert_ne!(fingerprint(""), fingerprint("x"));
    }
}
