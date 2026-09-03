//! THE FORGE — raids written by a model, flown by the machine.
//!
//! The Wardeck's forge rows ask a llama3-class model for a raid. The call
//! goes through `aperture-complete`'s `Provider` — logscope-fan's
//! dependency-free ollama client grown a trait — on a plain worker thread,
//! answered over an mpsc channel the frame loop polls (`forge_async`; the
//! poll lives in main's update, and `try_recv` never blocks a frame).
//!
//! THE CONTRACT WITH THE MODEL is a line format, not JSON, for the same
//! reason the save file is lines: lenient in, canonical out. A model that
//! wraps the answer in prose, misspells a mech, or puts an encounter on
//! the moon still yields a raid — unknown arenas drop the phase, unknown
//! mechs drop the line, coordinates clamp to the C engine's ±95, and only
//! a signal with no valid phase at all is refused. What survives parsing
//! is re-rendered canonically into `echoes.txt` beside the save, which is
//! also what the Wardeck's browser lists — restart-proof, hand-editable.
//!
//! AN ECHO IS NOT THE SIGNAL: it resembles a warden (the model names
//! which, and that decides the finale's champion flavor), it flies through
//! the same `RaidFlow` with the same drops and mechanics, and it BANKS
//! NOTHING — `RaidFlow::scored` is false, so the ladder and the seal stay
//! about the nineteen real signals.
//!
//! Endpoint knobs, all env, all optional:
//! * `BLACKSTAR_LLM_HOST`  ollama host:port  (default `127.0.0.1:11434`)
//! * `BLACKSTAR_LLM_MODEL` model name        (default `llama3`)
//! * `BLACKSTAR_LLM_CMD`   escape hatch: an argv that gets the prompt on
//!   stdin and answers on stdout (`CommandProvider`) — this is how a
//!   `logscope-serve` box on the LAN plugs in: a two-line curl script with
//!   its URL and bearer key.

use std::sync::mpsc;

use aperture_complete::providers::{CommandProvider, OllamaConfig, OllamaProvider};
use aperture_complete::{CompletionRequest, Provider};

use crate::raidcon::{Arena, Boss, Mech, ARENAS, BOSSES};
use crate::raidgen::{strhash, EncKind, EncSpec, PhaseSpec, RaidPlan};

/// A parsed echo: everything needed to rebuild a `RaidPlan` on demand.
#[derive(Debug, Clone)]
pub struct EchoRaid {
    pub name: String,
    pub resembles: &'static Boss,
    pub phases: Vec<(String, &'static Arena, Vec<EncSpec>)>,
    /// Whoever the model asked for, RAW — not yet checked against the
    /// campaign. `parse` has no business knowing how far in a player is, and
    /// the same forged raid can be legal for one save and a spoiler for
    /// another, so admission is the caller's job via `casting::admit`.
    pub cast_request: Vec<String>,
}

impl EchoRaid {
    pub fn id(&self) -> String {
        format!("echo:{:08x}", strhash(&self.name) as u32)
    }
    pub fn to_plan(&self) -> RaidPlan {
        RaidPlan {
            boss: self.resembles,
            seed: strhash(&self.name),
            phases: self
                .phases
                .iter()
                .map(|(name, arena, enc)| PhaseSpec {
                    name: name.clone(),
                    arena,
                    enc: enc.clone(),
                })
                .collect(),
        }
    }
}

/// The grounded prompt: the model gets the REAL rosters, the exact line
/// format, and the theme — and nothing to invent but the fiction.
pub fn prompt_for(theme: &str, cast: &[&str]) -> String {
    let arenas: Vec<&str> = ARENAS.iter().map(|a| a.id).collect();
    let bosses: Vec<&str> = BOSSES.iter().map(|b| b.id).collect();
    format!(
        "You design a raid for a game. Answer with ONLY these lines, no prose, no markdown:\n\
         signal <a short evocative raid name>\n\
         resembles <exactly one warden id from: {}>\n\
         Then exactly 3 phase blocks. Each block is one line:\n\
         phase <exactly one arena id from: {}> <a short phase name>\n\
         followed by 2 to 5 encounter lines:\n\
         enc <purge|ground|air|sync> <anchor|standard> <x> <z>\n\
         where x and z are integers between -95 and 95. At most one anchor per phase.\n\
         Finish with the line:\n\
         end\n\
         Theme: {}\n{}",
        bosses.join(", "),
        arenas.join(", "),
        theme,
        // WHO IT MAY CAST. Empty until the campaign has introduced someone,
        // and the line is omitted entirely rather than sent empty — a model
        // given "cast: " with nothing after it invents a name, and the parser
        // would then drop it, which looks like the forge ignoring the model.
        // `casting::admit` is still the authority; this only tells the model
        // what is worth asking for.
        if cast.is_empty() {
            String::new()
        } else {
            format!(
                "You may optionally add ONE line before `end`:\n\
                 cast <exactly one id from: {}>\n\
                 Only add it if that character genuinely belongs in this raid.\n",
                cast.join(", ")
            )
        }
    )
}

fn mech_of(s: &str) -> Option<Mech> {
    match s.to_ascii_lowercase().as_str() {
        "purge" => Some(Mech::Purge),
        "ground" => Some(Mech::Ground),
        "air" => Some(Mech::Air),
        "sync" => Some(Mech::Sync),
        _ => None,
    }
}

/// Lenient in: scan the text for our lines, tolerate anything around them,
/// keep what validates, and refuse only a signal with nothing flyable in
/// it. The reasons are collected so a sputtering forge can say WHY.
pub fn parse(text: &str) -> Result<EchoRaid, String> {
    let mut name = String::new();
    let mut resembles: &'static Boss = &BOSSES[0];
    let mut phases: Vec<(String, &'static Arena, Vec<EncSpec>)> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    let mut cast_request: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches(['-', '*', '>', ' ']);
        let Some((k, rest)) = line.split_once(' ') else {
            if line == "end" {
                break;
            }
            continue;
        };
        let rest = rest.trim();
        match k.to_ascii_lowercase().as_str() {
            "signal" => name = rest.chars().take(48).collect(),
            // A NAME THE MODEL ASKED FOR, kept unvalidated on purpose. The
            // prompt offers a roster, but a model that ignores it and invents
            // someone must not fail the whole raid — same leniency as an
            // unknown mech dropping one line. `casting::admit` decides what
            // survives, and it needs the campaign log this function lacks.
            "cast" => {
                if cast_request.len() < 4 {
                    cast_request.push(rest.chars().take(40).collect());
                }
            }
            "resembles" => match BOSSES.iter().find(|b| b.id == rest) {
                Some(b) => resembles = b,
                None => dropped.push(format!("unknown warden '{rest}'")),
            },
            "phase" => {
                if phases.len() >= 4 {
                    dropped.push("phase past the cap".into());
                    continue;
                }
                let (aid, pname) = rest.split_once(' ').unwrap_or((rest, "unnamed"));
                match ARENAS.iter().find(|a| a.id == aid) {
                    Some(a) => phases.push((pname.chars().take(40).collect(), a, Vec::new())),
                    None => dropped.push(format!("unknown arena '{aid}'")),
                }
            }
            "enc" => {
                let Some((pname, _, enc)) = phases.last_mut() else {
                    dropped.push("enc before any phase".into());
                    continue;
                };
                let _ = pname;
                if enc.len() >= 6 {
                    dropped.push("enc past the cap".into());
                    continue;
                }
                let mut it = rest.split_whitespace();
                let (m, kd, x, z) = (it.next(), it.next(), it.next(), it.next());
                let Some(mech) = m.and_then(mech_of) else {
                    dropped.push(format!("unknown mech in '{rest}'"));
                    continue;
                };
                let kind = match kd.map(|s| s.to_ascii_lowercase()) {
                    Some(ref s) if s == "anchor" => EncKind::Anchor,
                    _ => EncKind::Standard,
                };
                // one anchor per phase, the generator's own invariant
                let kind = if matches!(kind, EncKind::Anchor)
                    && enc.iter().any(|e| matches!(e.kind, EncKind::Anchor))
                {
                    EncKind::Standard
                } else {
                    kind
                };
                let coord = |s: Option<&str>| {
                    s.and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0).clamp(-95.0, 95.0)
                };
                enc.push(EncSpec {
                    x: coord(x),
                    z: coord(z),
                    kind,
                    mech,
                    min_units: mech.min_units(),
                });
            }
            _ => {}
        }
    }
    phases.retain(|(_, _, enc)| !enc.is_empty());
    if name.is_empty() {
        name = "unnamed echo".into();
    }
    if phases.is_empty() {
        return Err(if dropped.is_empty() {
            "the answer held no phases".into()
        } else {
            format!("nothing flyable survived: {}", dropped.join("; "))
        });
    }
    Ok(EchoRaid { name, resembles, phases, cast_request })
}

/// Canonical out: the exact line format the prompt asks for, so the file
/// beside the save is also a worked example of the contract.
pub fn render(e: &EchoRaid) -> String {
    let mut out = format!("signal {}\nresembles {}\n", e.name, e.resembles.id);
    for (pname, arena, enc) in &e.phases {
        out.push_str(&format!("phase {} {}\n", arena.id, pname));
        for s in enc {
            let mech = match s.mech {
                Mech::Purge => "purge",
                Mech::Ground => "ground",
                Mech::Air => "air",
                Mech::Sync => "sync",
            };
            let kind = match s.kind {
                EncKind::Anchor => "anchor",
                EncKind::Standard => "standard",
            };
            out.push_str(&format!("enc {mech} {kind} {:.0} {:.0}\n", s.x, s.z));
        }
    }
    // THE CAST HAS TO SURVIVE THE FILE. `parse` reads a cast line and
    // `render` did not write one, so every echo lost its cast the moment it
    // was saved — the model's answer thrown away one step after being
    // accepted. The round-trip test did not catch it because it predates the
    // field, which is the general hazard: a test named "round trips" only
    // covers what it was written against.
    for id in &e.cast_request {
        out.push_str(&format!("cast {id}\n"));
    }
    out.push_str("end\n");
    out
}

/// Echoes split by whether this save can actually field them.
///
/// A forged raid that names someone the player has not met is NOT waste and
/// must not be discarded: it is a raid that is not eligible YET. Generation is
/// the expensive part — a model call on a worker thread — and eligibility is
/// a cheap function of the campaign log, so the right shape is to keep
/// everything and filter at the point of display. The same echo becomes
/// flyable when the campaign catches up, at no further cost.
///
/// Returns (ready, held). `held` carries the reason so the console can say
/// "waiting on someone you have not met" rather than hiding the raid.
///
/// PARTLY SUPERSEDED, and worth knowing before reaching for it. `casting::
/// billing` redacts an unmet character to `???` per viewer instead of
/// excluding them, which is the better answer for MULTIPLAYER: nobody is
/// locked out of a raid their friends are running, and nobody is spoiled,
/// because identity resolves against each player's own log. A held raid is a
/// player who cannot play; a redacted one is a player with a question.
///
/// This is kept for the SOLO case and as the strict option — a player who
/// wants their own forge output to follow their own pace, and a host who
/// genuinely wants a lobby gated. `partition` takes the log as a parameter, so
/// strict, per-viewer and open are one call apart rather than three designs.
pub fn catalog_split(log: &crate::campaign::CampaignLog) -> (Vec<EchoRaid>, Vec<(EchoRaid, String)>) {
    partition(catalog(), log)
}

/// The partition itself, over echoes already in hand.
///
/// Split out from `catalog_split` because that one reads a file, and a test
/// against it can only exercise whatever happens to be on disk — so a
/// regression that held NOTHING passed cleanly. This is the same
/// headless-core/shell split the rest of the tree uses, applied after the
/// break-check caught it.
pub fn partition(
    echoes: Vec<EchoRaid>,
    log: &crate::campaign::CampaignLog,
) -> (Vec<EchoRaid>, Vec<(EchoRaid, String)>) {
    let mut ready = Vec::new();
    let mut held = Vec::new();
    for e in echoes {
        let unmet: Vec<&str> = e
            .cast_request
            .iter()
            .filter_map(|w| crate::casting::entry(w))
            .filter(|c| crate::casting::castable(c.id, log).is_none())
            .map(|c| c.id)
            .collect();
        if unmet.is_empty() {
            ready.push(e);
        } else {
            let why = format!("held: {}", unmet.join(", "));
            held.push((e, why));
        }
    }
    (ready, held)
}

fn echoes_path() -> std::path::PathBuf {
    crate::save::path().with_file_name("echoes.txt")
}

/// Every echo on file, parse-on-read so a hand-edited file gets the same
/// leniency a model does.
pub fn catalog() -> Vec<EchoRaid> {
    let Ok(text) = std::fs::read_to_string(echoes_path()) else {
        return Vec::new();
    };
    text.split("\nend")
        .filter(|b| b.contains("signal "))
        .filter_map(|b| parse(&format!("{b}\nend")).ok())
        .collect()
}

pub fn find(id: &str) -> Option<EchoRaid> {
    catalog().into_iter().find(|e| e.id() == id)
}

/// Take a model's raw answer, keep what validates, put it on file.
/// Returns the signal's name for the toast.
pub fn accept(text: &str) -> Result<String, String> {
    let e = parse(text)?;
    let mut all = std::fs::read_to_string(echoes_path()).unwrap_or_default();
    // same name = same signal: re-forging replaces rather than duplicates
    if let Some(old) = catalog().into_iter().find(|x| x.id() == e.id()) {
        all = all.replace(&render(&old), "");
    }
    all.push_str(&render(&e));
    std::fs::write(echoes_path(), all).map_err(|x| format!("could not keep it: {x}"))?;
    Ok(e.name)
}

fn provider_from_env() -> Box<dyn Provider> {
    if let Ok(cmd) = std::env::var("BLACKSTAR_LLM_CMD") {
        let argv: Vec<String> = cmd.split_whitespace().map(str::to_string).collect();
        if !argv.is_empty() {
            return Box::new(CommandProvider::new(argv));
        }
    }
    Box::new(OllamaProvider::new(OllamaConfig {
        host: std::env::var("BLACKSTAR_LLM_HOST").unwrap_or_else(|_| "127.0.0.1:11434".into()),
        model: std::env::var("BLACKSTAR_LLM_MODEL").unwrap_or_else(|_| "llama3".into()),
        num_predict: 512,
        temperature: 0.7,
        use_suffix: false,
    }))
}

/// Light the forge. One worker thread, one blocking call, one answer on
/// the channel — the frame loop polls with `try_recv` and never waits.
pub fn forge_async(theme: String, cast: Vec<String>) -> mpsc::Receiver<Result<String, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut p = provider_from_env();
        let req = CompletionRequest {
            generation: 0,
            prefix: prompt_for(&theme, &cast.iter().map(|s| s.as_str()).collect::<Vec<_>>()),
            suffix: String::new(),
            lang: "raid".into(),
            // No file: the forge prompts from a THEME string, not from a
            // buffer on disk. `None` is the honest answer and disables the
            // sibling-read and `@read` paths structurally — see the field's
            // own doc in aperture-complete.
            path: None,
        };
        let _ = tx.send(p.complete(&req));
    });
    rx
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_cast_survives_the_round_trip_to_file() {
        // THE BUG THIS CAUGHT. `parse` read a cast line and `render` wrote
        // none, so every echo lost its cast the moment it was saved — a model
        // call thrown away one step after being accepted. The existing
        // round-trip test did not notice because it predates the field, which
        // is the general hazard with a test named "round trips": it covers
        // what it was written against and nothing added later.
        let text = "signal The Long Quiet\nresembles speakerman\n\
                    phase unending opening\nenc purge standard 0 0\n\
                    cast billy_the_fire_clone\nend";
        let a = super::parse(text).expect("valid");
        let b = super::parse(&super::render(&a)).expect("re-parse");
        assert_eq!(a.cast_request, b.cast_request, "the cast did not survive render");
        assert_eq!(b.cast_request, vec!["billy_the_fire_clone".to_string()]);
        // and the rest still round-trips, so the fix did not break the file
        assert_eq!(a.name, b.name);
        assert_eq!(a.phases.len(), b.phases.len());
    }

    #[test]
    fn an_ineligible_echo_is_held_rather_than_discarded() {
        // Generation is the expensive part; eligibility is a cheap function of
        // the campaign log. So a raid naming someone unmet is not waste, it is
        // not-yet — and the compute is banked instead of spent again later.
        use crate::campaign::CampaignLog;
        let e = super::parse(
            "signal Held\nresembles speakerman\nphase unending a\n\
             enc purge standard 0 0\ncast billy_the_fire_clone\nend",
        )
        .expect("valid");

        // Exercise `partition` itself, not a re-implementation of its rule.
        // The first version of this test checked the predicate by hand and
        // stayed green when the partition was changed to hold nothing — the
        // break-check caught it, and splitting the pure half out of the
        // file-reading half is what made the real function testable.
        let fresh = CampaignLog::from_line("");
        let (ready, held) = super::partition(vec![e.clone()], &fresh);
        assert!(ready.is_empty(), "a fresh save was offered an echo casting Billy");
        assert_eq!(held.len(), 1, "the echo was discarded instead of held");
        assert!(held[0].1.contains("billy"), "the hold does not say who: {}", held[0].1);

        let met = CampaignLog::from_line("the_tide_forges");
        let (ready, held) = super::partition(vec![e.clone()], &met);
        assert_eq!(ready.len(), 1, "meeting Billy did not release the echo");
        assert!(held.is_empty());

        // an echo with no cast at all is always ready
        let plain = super::parse(
            "signal Plain\nresembles speakerman\nphase unending a\nenc purge standard 0 0\nend",
        )
        .expect("valid");
        assert_eq!(super::partition(vec![plain], &fresh).0.len(), 1, "a castless echo was held");
    }

    #[test]
    fn a_cast_line_survives_parsing_and_a_bad_one_does_not_kill_the_raid() {
        // Closing the loop. The prompt offers a cast line, so the parser has
        // to read it — telling a model it may say something and then dropping
        // the answer is worse than never asking. Kept UNVALIDATED here: the
        // same forged raid is legal for one save and a spoiler for another,
        // and `parse` has no campaign log.
        let good = "signal The Long Quiet\nresembles speakerman\n\
                    phase unending opening\nenc purge standard 0 0\n\
                    cast billy_the_fire_clone\nend";
        let r = super::parse(good).expect("a valid echo");
        assert_eq!(r.cast_request, vec!["billy_the_fire_clone".to_string()]);

        // a model that invents someone must not fail the whole raid — same
        // leniency an unknown mech gets
        let junk = good.replace("billy_the_fire_clone", "a_bot_that_never_was");
        let r2 = super::parse(&junk).expect("an invented name must not refuse the raid");
        assert_eq!(r2.cast_request, vec!["a_bot_that_never_was".to_string()]);
        assert_eq!(r2.phases.len(), 1, "the raid lost a phase over a cast line");

        // and no cast line at all is normal, not an error
        let bare = good.replace("cast billy_the_fire_clone\n", "");
        assert!(super::parse(&bare).expect("still valid").cast_request.is_empty());
    }

    #[test]
    fn the_prompt_offers_only_characters_the_player_has_met() {
        // The wiring, not the rule. `casting::castable` is tested on its own;
        // this is the guard that the forge actually ASKS it — the failure this
        // session has made four times is a correct function nothing calls.
        let empty = super::prompt_for("a siege under closing weather", &[]);
        assert!(
            !empty.contains("cast <"),
            "a fresh save was offered a cast line: {empty}"
        );
        let with = super::prompt_for("a siege", &["billy_the_fire_clone", "the_morpher"]);
        assert!(with.contains("cast <"), "a met roster produced no cast line");
        assert!(with.contains("billy_the_fire_clone"), "the roster is not in the prompt");
        // the model is told it is optional, because a forced cameo every raid
        // is what makes presence stop meaning anything
        assert!(with.contains("optionally"), "the cast line reads as mandatory");
    }

    use super::*;

    fn sample() -> String {
        let a = ARENAS[0].id;
        let b = ARENAS[1 % ARENAS.len()].id;
        format!(
            "Sure! Here is your raid:\n\
             signal The Hollow Chorus\n\
             resembles {}\n\
             phase {a} the outer choir\n\
             enc purge standard 10 -20\n\
             enc air standard 40 40\n\
             phase {b} the throat\n\
             enc sync standard -30 0\n\
             enc ground anchor 0 60\n\
             end\n",
            BOSSES[0].id
        )
    }

    #[test]
    fn a_chatty_answer_still_parses_and_round_trips() {
        let e = parse(&sample()).expect("the leniency is the point");
        assert_eq!(e.name, "The Hollow Chorus");
        assert_eq!(e.phases.len(), 2);
        assert_eq!(e.phases[0].2.len(), 2);
        // canonical out re-parses to the same shape
        let again = parse(&render(&e)).unwrap();
        assert_eq!(again.name, e.name);
        assert_eq!(again.phases.len(), e.phases.len());
        assert_eq!(again.id(), e.id());
    }

    #[test]
    fn garbage_is_refused_with_a_reason_not_a_panic() {
        let err = parse("i cannot help with that").unwrap_err();
        assert!(err.contains("no phases"), "{err}");
        let err = parse("signal x\nphase not_an_arena y\nenc purge standard 0 0\nend").unwrap_err();
        assert!(err.contains("unknown arena"), "{err}");
    }

    #[test]
    fn coordinates_clamp_and_anchors_stay_singular() {
        let a = ARENAS[0].id;
        let text = format!(
            "signal x\nphase {a} y\nenc purge anchor 900 -900\nenc ground anchor 0 0\nend"
        );
        let e = parse(&text).unwrap();
        let enc = &e.phases[0].2;
        assert_eq!(enc[0].x, 95.0);
        assert_eq!(enc[0].z, -95.0);
        assert!(matches!(enc[0].kind, EncKind::Anchor));
        assert!(
            matches!(enc[1].kind, EncKind::Standard),
            "a second anchor must demote, per the generator's own invariant"
        );
    }

    #[test]
    fn an_echo_builds_a_flyable_plan() {
        let e = parse(&sample()).unwrap();
        let plan = e.to_plan();
        assert_eq!(plan.phases.len(), 2);
        assert!(plan.phases.iter().all(|p| !p.enc.is_empty()));
        // SYNC brought its plate quorum through the mech's own default
        let sync = plan.phases[1].enc.iter().find(|x| matches!(x.mech, Mech::Sync)).unwrap();
        assert_eq!(sync.min_units, Mech::Sync.min_units());
    }
}
