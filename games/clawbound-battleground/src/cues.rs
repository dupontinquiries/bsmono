//! cues -- which sounds a game event asks for.
//!
//! Pure: an event in, a list of [`Cue`]s out, each a list of bank keys to
//! try in order (most specific first: this ancestor's line for this slot,
//! then this ancestor's generic line, then the archetype's sound, then the
//! generic one). A key with no file is skipped; a cue with no file at all is
//! silence. [`place`] turns a cue's world position into gain and pan for a
//! listener; [`VoiceGate`] keeps thirty cats from talking over each other.
//!
//! The keys (see `soundbank` for the folder layout):
//!
//! | event            | keys                                                        | bus   |
//! |------------------|-------------------------------------------------------------|-------|
//! | wind-up          | `voice/<ancestor>/cast<N>`, `voice/<ancestor>/cast`         | voice |
//! | cast fires       | `sfx/cast/<ancestor>/<N>`, `sfx/cast/<archetype>`, `sfx/cast` | sfx |
//! | attack, nothing  | `sfx/whiff`                                                 | sfx   |
//! | hit              | `sfx/hit_big` (big), `sfx/hit`                              | sfx   |
//! | kill             | `voice/<killer>/kill`; `voice/<victim>/death`               | voice |
//! | tower down       | `sfx/tower_down`                                            | sfx   |
//! | level up         | `sfx/levelup`                                               | sfx   |
//!
//! The announcer's calls (first blood, multi-kills, towers, ...) are not
//! here: `announcer` decides those, one at a time.
//!
//! `<ancestor>` is the ancestor's name, lower case, no spaces
//! ("goldenmane"); `<N>` is the ability slot, 1-4; `<archetype>` is the
//! ability's kind, lower case ("bolt", "nova").

use glam::Vec3;

use crate::mixer::Bus;
use crate::sim::{Event, Game};

#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub keys: Vec<String>,
    pub bus: Bus,
    pub gain: f32,
    /// Where it happens; None: everywhere (the announcer, UI).
    pub pos: Option<Vec3>,
    /// A voice line's speaker (god index).
    pub speaker: Option<usize>,
    pub priority: u8,
}

/// An ancestor's key: its name, lower case, no spaces.
pub fn ancestor_key(game: &Game, god: usize) -> String {
    game.def(god).name.to_ascii_lowercase().replace([' ', '\''], "")
}

fn archetype_key(a: &crate::gods::Archetype) -> String {
    let s = format!("{a:?}");
    s.split([' ', '{', '(']).next().unwrap_or("").to_ascii_lowercase()
}

pub fn cues_for(ev: &Event, game: &Game) -> Vec<Cue> {
    let god_pos = |i: usize| game.gods.get(i).map(|g| g.pos);
    let by_name = |name: &str| (0..game.gods.len()).find(|&i| game.god_name(i) == name);
    match ev {
        Event::CastStart { god, slot, .. } => {
            let a = ancestor_key(game, *god);
            vec![Cue { keys: vec![format!("voice/{a}/cast{}", slot + 1), format!("voice/{a}/cast")], bus: Bus::Voice, gain: 1.0, pos: god_pos(*god), speaker: Some(*god), priority: 6 }]
        }
        Event::Cast { god, slot, .. } => {
            let a = ancestor_key(game, *god);
            let arch = archetype_key(&game.def(*god).abilities[*slot as usize].arch);
            vec![Cue { keys: vec![format!("sfx/cast/{a}/{}", slot + 1), format!("sfx/cast/{arch}"), "sfx/cast".into()], bus: Bus::Sfx, gain: 0.9, pos: god_pos(*god), speaker: None, priority: 5 }]
        }
        Event::Whiff { god } => vec![Cue { keys: vec!["sfx/whiff".into()], bus: Bus::Sfx, gain: 0.6, pos: god_pos(*god), speaker: None, priority: 2 }],
        Event::Damage { pos, big, .. } => {
            let keys = if *big { vec!["sfx/hit_big".into(), "sfx/hit".into()] } else { vec!["sfx/hit".into()] };
            vec![Cue { keys, bus: Bus::Sfx, gain: if *big { 0.9 } else { 0.55 }, pos: Some(*pos), speaker: None, priority: if *big { 4 } else { 1 } }]
        }
        Event::Kill { killer, victim, is_god: true, .. } => {
            let mut v = Vec::new();
            if let Some(k) = by_name(killer) {
                v.push(Cue { keys: vec![format!("voice/{}/kill", ancestor_key(game, k))], bus: Bus::Voice, gain: 1.0, pos: god_pos(k), speaker: Some(k), priority: 7 });
            }
            if let Some(d) = by_name(victim) {
                v.push(Cue { keys: vec![format!("voice/{}/death", ancestor_key(game, d))], bus: Bus::Voice, gain: 1.0, pos: god_pos(d), speaker: Some(d), priority: 7 });
            }
            v
        }
        Event::TowerDown { pos, .. } => vec![Cue { keys: vec!["sfx/tower_down".into()], bus: Bus::Sfx, gain: 1.0, pos: Some(*pos), speaker: None, priority: 6 }],
        Event::LevelUp { pos } => vec![Cue { keys: vec!["sfx/levelup".into()], bus: Bus::Sfx, gain: 0.7, pos: Some(*pos), speaker: None, priority: 3 }],
        _ => Vec::new(),
    }
}

/// Voice lines further than this from the listener are not spoken.
pub const VOICE_RANGE: f32 = 35.0;
/// Sound fades to nothing by this distance.
pub const HEARING: f32 = 60.0;

/// A cue's gain and pan for a listener at `ear` whose right is `right`.
/// None: too far to hear (or a voice line out of range).
pub fn place(cue: &Cue, ear: Vec3, right: Vec3) -> Option<(f32, f32)> {
    let Some(p) = cue.pos else { return Some((cue.gain, 0.0)) };
    let d = Vec3::new(p.x - ear.x, 0.0, p.z - ear.z);
    let dist = d.length();
    if cue.bus == Bus::Voice && cue.speaker.is_some() && dist > VOICE_RANGE {
        return None;
    }
    if dist > HEARING {
        return None;
    }
    let fall = 1.0 / (1.0 + dist * 0.08);
    let pan = if dist < 0.5 { 0.0 } else { (d / dist).dot(right).clamp(-1.0, 1.0) * 0.85 };
    Some((cue.gain * fall, pan))
}

/// Each cat speaks at most once per COOLDOWN seconds (the announcer is not
/// gated): a cat casting three times in a row barks once.
#[derive(Debug, Clone, Default)]
pub struct VoiceGate {
    last: std::collections::HashMap<usize, f32>,
}

pub const COOLDOWN: f32 = 2.5;

impl VoiceGate {
    pub fn allow(&mut self, cue: &Cue, now: f32) -> bool {
        let Some(s) = cue.speaker else { return true };
        if cue.bus != Bus::Voice {
            return true;
        }
        match self.last.get(&s) {
            Some(&t) if now - t < COOLDOWN => false,
            _ => {
                self.last.insert(s, now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Game {
        Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9])
    }

    #[test]
    fn a_cast_asks_for_the_ancestors_line_then_the_generic_one() {
        let g = game();
        let a = ancestor_key(&g, 0);
        assert!(!a.contains(' ') && a == a.to_ascii_lowercase());
        let c = cues_for(&Event::CastStart { god: 0, slot: 1, aim: Vec3::ZERO, dur: 0.3 }, &g);
        assert_eq!(c[0].keys, vec![format!("voice/{a}/cast2"), format!("voice/{a}/cast")]);
        assert_eq!((c[0].bus, c[0].speaker), (Bus::Voice, Some(0)));
        let c = cues_for(&Event::Cast { god: 0, slot: 0, aim: Vec3::ZERO }, &g);
        assert_eq!(c[0].keys[0], format!("sfx/cast/{a}/1"));
        assert!(c[0].keys[1].starts_with("sfx/cast/") && c[0].keys[2] == "sfx/cast");
    }

    #[test]
    fn a_kill_is_the_killer_and_the_victim() {
        let g = game();
        let c = cues_for(&Event::Kill { killer: g.god_name(0).to_string(), victim: g.god_name(5).to_string(), victim_team: g.gods[5].team, is_god: true }, &g);
        assert_eq!(c.len(), 2, "the announcer's call is the announcer's, not a cue");
        assert!(c[0].keys[0].ends_with("/kill") && c[0].speaker == Some(0));
        assert!(c[1].keys[0].ends_with("/death") && c[1].speaker == Some(5));
        assert!(cues_for(&Event::Kill { killer: "x".into(), victim: "y".into(), victim_team: g.gods[5].team, is_god: false }, &g).is_empty(), "a minion kill says nothing");
    }

    /// Left is left: a sound to the listener's right pans right; far away is
    /// quieter; a far voice line is not spoken; the announcer is everywhere.
    #[test]
    fn placement_pans_fades_and_limits_voices() {
        let cue = Cue { keys: vec![], bus: Bus::Sfx, gain: 1.0, pos: Some(Vec3::new(10.0, 0.0, 0.0)), speaker: None, priority: 1 };
        let (g, pan) = place(&cue, Vec3::ZERO, Vec3::X).unwrap();
        assert!(pan > 0.8 && g < 1.0);
        let far = Cue { pos: Some(Vec3::new(40.0, 0.0, 0.0)), ..cue.clone() };
        assert!(place(&far, Vec3::ZERO, Vec3::X).unwrap().0 < g);
        let voice = Cue { bus: Bus::Voice, speaker: Some(3), ..far.clone() };
        assert!(place(&voice, Vec3::ZERO, Vec3::X).is_none(), "40 u away: not spoken");
        let ann = Cue { pos: None, bus: Bus::Voice, ..cue.clone() };
        assert_eq!(place(&ann, Vec3::ZERO, Vec3::X), Some((1.0, 0.0)));
    }

    #[test]
    fn a_cat_barks_once_per_cooldown() {
        let mut gate = VoiceGate::default();
        let line = Cue { keys: vec![], bus: Bus::Voice, gain: 1.0, pos: None, speaker: Some(2), priority: 6 };
        assert!(gate.allow(&line, 0.0));
        assert!(!gate.allow(&line, 1.0));
        assert!(gate.allow(&Cue { speaker: Some(3), ..line.clone() }, 1.0), "another cat may speak");
        assert!(gate.allow(&line, COOLDOWN + 0.1));
        let ann = Cue { speaker: None, ..line };
        assert!(gate.allow(&ann, 0.0) && gate.allow(&ann, 0.0), "the announcer is not gated");
    }
}
