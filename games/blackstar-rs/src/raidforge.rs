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
#[derive(Debug)]
pub struct EchoRaid {
    pub name: String,
    pub resembles: &'static Boss,
    pub phases: Vec<(String, &'static Arena, Vec<EncSpec>)>,
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
pub fn prompt_for(theme: &str) -> String {
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
         Theme: {}\n",
        bosses.join(", "),
        arenas.join(", "),
        theme
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
    Ok(EchoRaid { name, resembles, phases })
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
    out.push_str("end\n");
    out
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
pub fn forge_async(theme: String) -> mpsc::Receiver<Result<String, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut p = provider_from_env();
        let req = CompletionRequest {
            generation: 0,
            prefix: prompt_for(&theme),
            suffix: String::new(),
            lang: "raid".into(),
        };
        let _ = tx.send(p.complete(&req));
    });
    rx
}

#[cfg(test)]
mod tests {
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
