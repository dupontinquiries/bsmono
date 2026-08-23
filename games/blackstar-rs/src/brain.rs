//! THE BRAIN SEAM — one command vocabulary, two kinds of commander.
//!
//! Ported from the C engine's `combat_json.c` and `commander_ai.c` before that
//! tree is retired. It was the most valuable thing in there and it is not a
//! feature; it is an interface, and the choice it encodes is worth restating
//! in its author's terms:
//!
//! > a commander FSM emits the SAME command an LLM would. So an FSM commander
//! > and an LLM commander are interchangeable behind one interface — the
//! > deterministic layer is a strict subset of what the LLM layer can express,
//! > and both speak the one command vocabulary.
//!
//! That is why the vocabulary is small and verb-shaped — `flock_to`,
//! `flock_spread`, `hold`, `focus`, `retreat`, `ally_follow`. It has to be
//! narrow enough that a state machine can emit all of it and open enough that
//! a language model does not have to be taught a private dialect.
//!
//! ## Two directions
//!
//! **Snapshot** (game → brain) is the combat state plus the ENGAGEMENT
//! SIGNALS — aggression, recent damage, pressure, kills. That second part is
//! what makes the loop worth closing: a brain that only sees positions plays
//! chess, and a brain that sees *how you are fighting* can react to you.
//!
//! **Orders** (brain → game) come back as a small set of commands aimed at a
//! zone rather than at individuals, because a commander commands a squad.
//!
//! ## The tick echo is not bookkeeping
//!
//! Orders carry the tick of the snapshot they answer. The C header describes
//! it as ordering; for an LLM provider it is the whole safety story. A model
//! takes hundreds of milliseconds and the frame cannot wait, so replies arrive
//! stale as a matter of course — and orders that answer a battle two seconds
//! gone are worse than no orders. `Orders::fresh_for` is the gate, and it is
//! the reason an asynchronous brain is safe to plug in at all.
//!
//! ## Tolerant parsing is a requirement, not a courtesy
//!
//! The C parser's contract — "unknown keys are skipped, missing fields take
//! sane defaults, so a chatty LLM response still decodes" — is load-bearing.
//! A model asked for JSON returns JSON wrapped in an explanation roughly
//! whenever it feels like it. A parser that refuses those is a provider that
//! fails intermittently for reasons the player experiences as the enemy going
//! stupid.

use crate::content::{commander, Commander, COMMANDERS};
use glam::Vec3;

// ------------------------------------------------------------ snapshot ---

/// Where a unit stands relative to the player's owned space.
pub const ZONE_FRONT: i32 = 0;
pub const ZONE_MID: i32 = 1;
pub const ZONE_BACK: i32 = 2;
/// A command aimed at every zone.
pub const ZONE_ALL: i32 = -1;

/// How the player is fighting, not just where things are.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Signals {
    /// 0..1, how hard the player is pushing — fire rate and closing speed
    pub aggression: f32,
    /// damage taken lately; decays
    pub recent_dmg: f32,
    /// 0..1, how surrounded they are
    pub pressure: f32,
    pub kills: i32,
    pub enemies_alive: i32,
    pub player_hp_frac: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Unit {
    pub id: i32,
    pub pos: Vec3,
    pub hp: f32,
    pub max_hp: f32,
    pub zone: i32,
    pub alive: bool,
    pub is_commander: bool,
    /// commander id it answers to, or -1
    pub squad: i32,
    /// leaderless and panicking
    pub scattering: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ally {
    pub id: i32,
    pub pos: Vec3,
    pub hp: f32,
    /// 0 follow / 1 engage / 2 down
    pub mode: i32,
    pub target: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub tick: u64,
    pub player_pos: Vec3,
    pub in_vehicle: bool,
    pub signals: Signals,
    pub units: Vec<Unit>,
    pub allies: Vec<Ally>,
}

impl Snapshot {
    pub fn live(&self) -> impl Iterator<Item = &Unit> {
        self.units.iter().filter(|u| u.alive)
    }
    pub fn zone_centre(&self, zone: i32) -> Option<Vec3> {
        let v: Vec<Vec3> =
            self.live().filter(|u| zone == ZONE_ALL || u.zone == zone).map(|u| u.pos).collect();
        if v.is_empty() {
            return None;
        }
        Some(v.iter().fold(Vec3::ZERO, |a, b| a + *b) / v.len() as f32)
    }
}

// -------------------------------------------------------------- orders ---

/// The whole vocabulary. Narrow on purpose: an FSM must be able to emit all
/// of it, and a model must not need a private dialect to speak it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verb {
    FlockTo,
    FlockSpread,
    Hold,
    Focus,
    Retreat,
    AllyFollow,
}

impl Verb {
    pub fn all() -> [Verb; 6] {
        [Verb::FlockTo, Verb::FlockSpread, Verb::Hold, Verb::Focus, Verb::Retreat, Verb::AllyFollow]
    }
    /// The wire name. Stable — a model was told these.
    pub fn name(self) -> &'static str {
        match self {
            Verb::FlockTo => "flock_to",
            Verb::FlockSpread => "flock_spread",
            Verb::Hold => "hold",
            Verb::Focus => "focus",
            Verb::Retreat => "retreat",
            Verb::AllyFollow => "ally_follow",
        }
    }
    pub fn parse(s: &str) -> Option<Verb> {
        Verb::all().into_iter().find(|v| v.name() == s.trim())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Command {
    pub verb: Verb,
    /// 0/1/2, or `ZONE_ALL`
    pub zone: i32,
    pub point: Vec3,
    /// verb-specific: radius, mode, urgency
    pub param0: f32,
    pub param1: f32,
}

impl Command {
    pub fn new(verb: Verb, zone: i32, point: Vec3) -> Command {
        Command { verb, zone, point, param0: 0.0, param1: 0.0 }
    }
    /// Would the game refuse this? A model can emit nonsense and the game
    /// must not act on it.
    pub fn sane(&self) -> bool {
        (self.zone == ZONE_ALL || (ZONE_FRONT..=ZONE_BACK).contains(&self.zone))
            && self.point.is_finite()
            && self.param0.is_finite()
            && self.param1.is_finite()
    }
}

/// At most this many commands per reply. A model that returns forty orders is
/// not commanding, it is listing.
pub const MAX_CMDS: usize = 16;

#[derive(Clone, Debug, Default)]
pub struct Orders {
    /// echoes the snapshot this answers
    pub tick: u64,
    pub speaker: String,
    pub cmds: Vec<Command>,
}

impl Orders {
    /// Are these still worth obeying?
    ///
    /// The gate that makes an asynchronous brain safe. A model reply that
    /// answers a battle `stale_ticks` old describes a fight that is over, and
    /// acting on it reads as the enemy behaving randomly.
    pub fn fresh_for(&self, now: u64, stale_ticks: u64) -> bool {
        now.saturating_sub(self.tick) <= stale_ticks
    }
    /// Drop anything malformed and cap the length. Applied to every reply,
    /// from either kind of brain.
    pub fn sanitised(mut self) -> Orders {
        self.cmds.retain(|c| c.sane());
        self.cmds.truncate(MAX_CMDS);
        self
    }
}

/// What issues orders. The seam: swap the implementation, keep the game.
pub trait Brain {
    fn think(&mut self, snap: &Snapshot, dt: f32) -> Option<Orders>;
    fn who(&self) -> &str;
}

// ---------------------------------------------------------------- json ---

fn f(v: f32) -> String {
    format!("{v:.3}")
}

/// Snapshot as JSON, for a brain that reads text.
pub fn write_snapshot(s: &Snapshot) -> String {
    let mut o = String::with_capacity(1024);
    o.push_str(&format!(
        "{{\"tick\":{},\"player\":[{},{},{}],\"in_vehicle\":{},",
        s.tick,
        f(s.player_pos.x),
        f(s.player_pos.y),
        f(s.player_pos.z),
        s.in_vehicle
    ));
    let g = &s.signals;
    o.push_str(&format!(
        "\"signals\":{{\"aggression\":{},\"recent_dmg\":{},\"pressure\":{},\"kills\":{},\"enemies_alive\":{},\"player_hp\":{}}},",
        f(g.aggression), f(g.recent_dmg), f(g.pressure), g.kills, g.enemies_alive, f(g.player_hp_frac)
    ));
    o.push_str("\"units\":[");
    for (i, u) in s.live().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push_str(&format!(
            "{{\"id\":{},\"pos\":[{},{},{}],\"hp\":{},\"zone\":{},\"squad\":{},\"scattering\":{}}}",
            u.id,
            f(u.pos.x),
            f(u.pos.y),
            f(u.pos.z),
            f(u.hp / u.max_hp.max(0.001)),
            u.zone,
            u.squad,
            u.scattering
        ));
    }
    o.push_str("],\"allies\":[");
    for (i, a) in s.allies.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push_str(&format!(
            "{{\"id\":{},\"pos\":[{},{},{}],\"mode\":{},\"target\":{}}}",
            a.id,
            f(a.pos.x),
            f(a.pos.y),
            f(a.pos.z),
            a.mode,
            a.target
        ));
    }
    o.push_str("]}");
    o
}

/// Pull orders out of whatever a brain said.
///
/// Deliberately scanning rather than strict: it finds the first `{`-balanced
/// region containing a `"cmds"` array and reads what it recognises. A model
/// asked for JSON will wrap it in prose, fence it in backticks, or append an
/// explanation, and every one of those is a reply worth obeying.
pub fn read_orders(text: &str) -> Option<Orders> {
    let body = extract_object(text)?;
    let mut o = Orders {
        tick: num(&body, "tick").unwrap_or(0.0) as u64,
        speaker: string_field(&body, "speaker").unwrap_or_else(|| "brain".into()),
        cmds: Vec::new(),
    };
    let arr = after_key(&body, "cmds")?;
    for chunk in objects_in(arr) {
        let Some(name) = string_field(chunk, "verb") else { continue };
        let Some(verb) = Verb::parse(&name) else { continue };
        let p = point(chunk).unwrap_or(Vec3::ZERO);
        o.cmds.push(Command {
            verb,
            zone: num(chunk, "zone").unwrap_or(-1.0) as i32,
            point: p,
            param0: num(chunk, "param0").unwrap_or(0.0),
            param1: num(chunk, "param1").unwrap_or(0.0),
        });
    }
    Some(o.sanitised())
}

/// Orders back to JSON — round trip, logging, and the shape a model is shown.
pub fn write_orders(o: &Orders) -> String {
    let mut s = format!("{{\"tick\":{},\"speaker\":\"{}\",\"cmds\":[", o.tick, o.speaker);
    for (i, c) in o.cmds.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"verb\":\"{}\",\"zone\":{},\"point\":[{},{},{}],\"param0\":{},\"param1\":{}}}",
            c.verb.name(),
            c.zone,
            f(c.point.x),
            f(c.point.y),
            f(c.point.z),
            f(c.param0),
            f(c.param1)
        ));
    }
    s.push_str("]}");
    s
}

/// The outermost balanced `{...}` that mentions `"cmds"`.
fn extract_object(t: &str) -> Option<&str> {
    let bytes = t.as_bytes();
    for (start, _) in t.char_indices().filter(|(_, c)| *c == '{') {
        let mut depth = 0i32;
        let mut in_str = false;
        let mut esc = false;
        for i in start..bytes.len() {
            let c = bytes[i] as char;
            if esc {
                esc = false;
                continue;
            }
            match c {
                '\\' if in_str => esc = true,
                '"' => in_str = !in_str,
                '{' if !in_str => depth += 1,
                '}' if !in_str => {
                    depth -= 1;
                    if depth == 0 {
                        let seg = &t[start..=i];
                        if seg.contains("\"cmds\"") {
                            return Some(seg);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn after_key<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let k = format!("\"{key}\"");
    let i = s.find(&k)? + k.len();
    let rest = s[i..].trim_start();
    Some(rest.strip_prefix(':').map(str::trim_start).unwrap_or(rest))
}

fn num(s: &str, key: &str) -> Option<f32> {
    let v = after_key(s, key)?;
    let end = v.find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '.' || c == 'e'))?;
    v[..end].parse().ok()
}

fn string_field(s: &str, key: &str) -> Option<String> {
    let v = after_key(s, key)?.strip_prefix('"')?;
    Some(v[..v.find('"')?].to_string())
}

fn point(s: &str) -> Option<Vec3> {
    let v = after_key(s, "point")?.strip_prefix('[')?;
    let inner = &v[..v.find(']')?];
    let n: Vec<f32> = inner.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    (n.len() == 3).then(|| Vec3::new(n[0], n[1], n[2]))
}

/// Split a JSON array body into its top-level `{...}` members.
fn objects_in(arr: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start, mut in_str, mut esc) = (0i32, 0usize, false, false);
    for (i, c) in arr.char_indices() {
        if esc {
            esc = false;
            continue;
        }
        match c {
            '\\' if in_str => esc = true,
            '"' => in_str = !in_str,
            '{' if !in_str => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            '}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    out.push(&arr[start..=i]);
                }
            }
            ']' if !in_str && depth == 0 => break,
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------- the FSM commander ---

/// What a commander is currently doing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Posture {
    /// feeling the player out, holding
    Probe,
    /// committing
    Press,
    /// spreading to surround
    Encircle,
    /// falling back, harassing
    Fallback,
    /// regrouping the squad
    Rally,
}

/// Seconds between decisions. A commander that re-thinks every frame twitches;
/// one that thinks rarely is a statue. From the C `think_cd`.
pub const THINK_INTERVAL_S: f32 = 1.25;

/// The deterministic commander.
///
/// Reads the battlefield, picks a posture from its doctrine, and emits the
/// same `Orders` an LLM would. Its four dials are `content::COMMANDERS` —
/// the same table the C loaded from `commanders.json`, already in this tree.
pub struct Fsm {
    pub doctrine: &'static Commander,
    pub posture: Posture,
    pub squad: i32,
    think_cd: f32,
    state_t: f32,
}

impl Fsm {
    pub fn new(archetype: &str, squad: i32) -> Fsm {
        Fsm {
            doctrine: commander(archetype).unwrap_or(&COMMANDERS[0]),
            posture: Posture::Probe,
            squad,
            think_cd: 0.0,
            state_t: 0.0,
        }
    }

    /// Choose a posture from the battlefield and the doctrine.
    ///
    /// The order of these arms IS the doctrine's priority, and it is the part
    /// worth arguing with: hurt-and-cautious beats everything, because a
    /// commander that presses while its squad dies is not a doctrine, it is a
    /// bug. Then rally, then encircle, then press.
    /// The units this commander actually speaks for.
    ///
    /// `Fsm.squad` was carried across from the C's `squad_id` and read by
    /// nothing, which was not merely dead — it meant `choose` assessed
    /// strength across the WHOLE battlefield, so a commander whose own squad
    /// was dying kept pressing because somebody else's was healthy. A
    /// negative squad means "everything", for the single-commander case.
    fn mine<'a>(&self, s: &'a Snapshot) -> impl Iterator<Item = &'a Unit> + 'a {
        let squad = self.squad;
        s.live().filter(move |u| squad < 0 || u.squad == squad)
    }

    fn choose(&self, s: &Snapshot) -> Posture {
        let d = self.doctrine;
        let live = self.mine(s).count().max(1) as f32;
        let strength: f32 =
            self.mine(s).map(|u| u.hp / u.max_hp.max(0.001)).sum::<f32>() / live;
        let scattered = self.mine(s).filter(|u| u.scattering).count() as f32 / live;

        // NOTE ON THE THRESHOLDS. 0.45 strength and 0.4 caution are guesses,
        // and they have a visible consequence: `aggressor` (caution 0.15) and
        // `surrounder` (0.30) sit below the caution bar, so neither ever falls
        // back and both are state-insensitive on this axis. That reads as
        // correct doctrine — an aggressor presses through it — but it is a
        // number choosing that, not a design decision, and `--brain doctrines`
        // shows it in one glance. Worth a playthrough before trusting.
        //
        // hurt, and cautious enough to mind
        if strength < 0.45 && d.caution > 0.4 {
            return Posture::Fallback;
        }
        // the squad has come apart and this commander cares
        if scattered > 0.3 && d.regroup > 0.5 {
            return Posture::Rally;
        }
        // numbers on the field and a taste for using them
        if live >= 3.0 && d.encircle > 0.6 {
            return Posture::Encircle;
        }
        // the player is pushing and we are willing to meet it
        if s.signals.aggression > 0.5 && d.aggression > 0.5 {
            return Posture::Press;
        }
        if d.aggression > 0.75 {
            return Posture::Press;
        }
        Posture::Probe
    }

    /// Turn a posture into commands. This is the whole reason the FSM is a
    /// "strict subset": every posture maps onto verbs a model could also
    /// have chosen, so the game downstream cannot tell which sent them.
    fn orders_for(&self, s: &Snapshot) -> Orders {
        let p = s.player_pos;
        let urgency = self.doctrine.aggression;
        let mut cmds = Vec::new();
        match self.posture {
            Posture::Probe => {
                cmds.push(Command::new(Verb::Hold, ZONE_ALL, p));
            }
            Posture::Press => {
                let mut c = Command::new(Verb::Focus, ZONE_ALL, p);
                c.param0 = urgency;
                cmds.push(c);
                cmds.push(Command::new(Verb::FlockTo, ZONE_FRONT, p));
                // the middle follows the front in rather than standing off:
                // a press that commits only its front rank is a probe with
                // extra steps
                cmds.push(Command::new(Verb::FlockTo, ZONE_MID, p));
            }
            Posture::Encircle => {
                let mut c = Command::new(Verb::FlockSpread, ZONE_ALL, p);
                // radius shrinks as the doctrine's appetite for it grows:
                // closing the ring is what encircling is FOR
                c.param0 = 18.0 - 8.0 * self.doctrine.encircle;
                cmds.push(c);
                cmds.push(Command::new(Verb::Hold, ZONE_BACK, p));
            }
            Posture::Fallback => {
                let mut c = Command::new(Verb::Retreat, ZONE_ALL, p);
                c.param0 = self.doctrine.caution;
                cmds.push(c);
            }
            Posture::Rally => {
                let rally = s.zone_centre(ZONE_BACK).unwrap_or(p);
                cmds.push(Command::new(Verb::FlockTo, ZONE_ALL, rally));
            }
        }
        Orders { tick: s.tick, speaker: self.doctrine.id.to_string(), cmds }.sanitised()
    }
}

impl Brain for Fsm {
    fn think(&mut self, snap: &Snapshot, dt: f32) -> Option<Orders> {
        self.state_t += dt;
        self.think_cd -= dt;
        if self.think_cd > 0.0 {
            return None;
        }
        self.think_cd = THINK_INTERVAL_S;
        let want = self.choose(snap);
        if want != self.posture {
            self.posture = want;
            self.state_t = 0.0;
        }
        Some(self.orders_for(snap))
    }
    fn who(&self) -> &str {
        self.doctrine.id
    }
}

// ----------------------------------------------------- the LLM provider ---

/// Text in, text out. The whole dependency an LLM brain has on the world.
///
/// Deliberately this small so `blackstar-rs` does not take a dependency on an
/// inference stack. The monorepo's own `apps/logscope` is the intended
/// provider — it is a Rust LLM workspace with a `gpu-gemm` wgpu→Vulkan engine
/// and a local GGUF path, so it can serve this on the same devices the game
/// targets — but an Ollama HTTP call, a canned fixture, or a test double
/// satisfies it equally. The game should not know which.
pub trait Completion {
    /// `None` when the provider is busy or unavailable. NOT an error: a brain
    /// that cannot answer this tick is normal, and the game keeps its last
    /// orders.
    fn complete(&mut self, prompt: &str) -> Option<String>;
    fn name(&self) -> &str;
}

/// The prompt a model is given. Kept as a constant because it is an interface:
/// the verb list here and `Verb::name` must not drift.
pub fn system_prompt() -> String {
    let verbs: Vec<&str> = Verb::all().iter().map(|v| v.name()).collect();
    format!(
        "You command a squad against the player. Reply with JSON only:\n\
         {{\"tick\":<echo the snapshot tick>,\"speaker\":\"<your name>\",\"cmds\":[\
         {{\"verb\":\"...\",\"zone\":0|1|2|-1,\"point\":[x,y,z],\"param0\":0.0,\"param1\":0.0}}]}}\n\
         verbs: {}\n\
         zone: 0 front, 1 mid, 2 back, -1 all. At most {} commands.\n\
         Echo the tick you were given.",
        verbs.join(", "),
        MAX_CMDS
    )
}

/// A commander backed by a language model.
///
/// Interchangeable with `Fsm` behind `Brain`, which is the point of the whole
/// module. It is *not* interchangeable in latency, so it does the honest
/// thing: asks on the same cadence, and drops replies that came back stale.
pub struct LlmBrain<C: Completion> {
    pub provider: C,
    /// how many ticks late a reply may be and still be obeyed
    pub stale_ticks: u64,
    think_cd: f32,
    /// what to fall back on when the model declines or answers rubbish
    pub fallback: Fsm,
    pub last_rejected: Option<&'static str>,
}

impl<C: Completion> LlmBrain<C> {
    pub fn new(provider: C, fallback_archetype: &str, squad: i32) -> LlmBrain<C> {
        LlmBrain {
            provider,
            stale_ticks: 90,
            think_cd: 0.0,
            fallback: Fsm::new(fallback_archetype, squad),
            last_rejected: None,
        }
    }
}

impl<C: Completion> Brain for LlmBrain<C> {
    fn think(&mut self, snap: &Snapshot, dt: f32) -> Option<Orders> {
        self.think_cd -= dt;
        if self.think_cd > 0.0 {
            return None;
        }
        self.think_cd = THINK_INTERVAL_S;
        self.last_rejected = None;

        let prompt = format!("{}\n\n{}", system_prompt(), write_snapshot(snap));
        // EVERY FAILURE FALLS BACK RATHER THAN STALLING. A squad that stops
        // taking orders because a model is loading reads as broken AI, and
        // the player has no way to know the difference.
        let reason = match self.provider.complete(&prompt) {
            None => "provider declined",
            Some(text) => match read_orders(&text) {
                None => "no orders in the reply",
                Some(o) if !o.fresh_for(snap.tick, self.stale_ticks) => "reply was stale",
                Some(o) if o.cmds.is_empty() => "reply had no usable commands",
                Some(mut o) => {
                    // trust the model for the verbs, never for the tick
                    o.tick = snap.tick;
                    return Some(o);
                }
            },
        };
        self.last_rejected = Some(reason);
        // the fallback shares the cadence, so it has already been ticked past
        // its own cooldown; ask it directly
        self.fallback.think_cd = 0.0;
        self.fallback.think(snap, dt)
    }
    fn who(&self) -> &str {
        self.provider.name()
    }
}

// ------------------------------------------------------------- driver ---

/// A provider that returns canned text. The test double, and the thing that
/// lets `--brain llm` run with no model present.
pub struct Canned {
    pub replies: Vec<String>,
    pub n: usize,
}
impl Completion for Canned {
    fn complete(&mut self, _prompt: &str) -> Option<String> {
        let r = self.replies.get(self.n).cloned();
        self.n += 1;
        r
    }
    fn name(&self) -> &str {
        "canned"
    }
}

/// A battlefield to think about: a squad, some of it scattered, pressing you.
fn scenario(tick: u64, hurt: f32, scattering: usize) -> Snapshot {
    let mut units = Vec::new();
    for i in 0..6 {
        units.push(Unit {
            id: i,
            pos: Vec3::new((i as f32 - 3.0) * 4.0, 0.0, 12.0),
            hp: 100.0 * hurt,
            max_hp: 100.0,
            zone: (i % 3) as i32,
            alive: true,
            is_commander: i == 0,
            squad: 0,
            scattering: (i as usize) < scattering,
        });
    }
    Snapshot {
        tick,
        player_pos: Vec3::ZERO,
        in_vehicle: false,
        signals: Signals {
            aggression: 0.7,
            recent_dmg: 12.0,
            pressure: 0.4,
            kills: 2,
            enemies_alive: 6,
            player_hp_frac: 0.8,
        },
        units,
        allies: vec![Ally { id: 100, pos: Vec3::new(2.0, 0.0, 1.0), hp: 80.0, mode: 0, target: -1 }],
    }
}

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--brain").unwrap_or(0);
    match argv.get(i + 1).map(String::as_str).unwrap_or("doctrines") {
        "doctrines" => doctrines(),
        "hive" => hive_demo(argv.get(i + 2).map(String::as_str)),
        "wire" => wire(),
        "snapshot" => println!("{}", write_snapshot(&scenario(42, 1.0, 0))),
        "prompt" => println!("{}", system_prompt()),
        other => {
            println!("unknown: --brain {other}");
            println!("try: doctrines | hive | wire | snapshot | prompt");
        }
    }
}

fn doctrines() {
    println!("COMMANDERS — the same battlefield, five doctrines\n");
    let cases = [("fresh", 1.0, 0), ("hurt", 0.3, 0), ("coming apart", 0.8, 4)];
    print!("  {:<12}", "");
    for (label, _, _) in &cases {
        print!(" {:<26}", label);
    }
    println!();
    for c in COMMANDERS {
        print!("  {:<12}", c.id);
        for (_, hp, scat) in &cases {
            let mut f = Fsm::new(c.id, 0);
            let s = scenario(1, *hp, *scat);
            let o = f.think(&s, 99.0).unwrap();
            let verbs: Vec<&str> = o.cmds.iter().map(|x| x.verb.name()).collect();
            print!(" {:<26}", format!("{:?}: {}", f.posture, verbs.join("+")));
        }
        println!();
    }
    println!("\n  the same four dials `content::COMMANDERS` already carried; the C");
    println!("  engine loaded them from commanders.json and had the FSM this");
    println!("  tree did not.");
}

/// The other kind of brain: the same vocabulary reached by a sneakier policy.
fn hive_demo(only: Option<&str>) {
    use crate::hive::{Strain, Swarm};
    println!("THE HIVE — the same verbs, a completely different policy\n");
    println!("  a commander issues orders. the hive does not have one: excitement");
    println!("  spreads by PROXIMITY, and the swarm tips when enough are near.\n");
    println!("  {:<10} {:>6} {:>7} {:>8}   {}", "strain", "gate", "mass", "radius", "as the prey closes");
    let strains: Vec<Strain> = match only.and_then(Strain::parse) {
        Some(s) => vec![s],
        None => {
            if let Some(bad) = only {
                println!("  (no strain '{bad}'; showing all)\n");
            }
            Strain::all().to_vec()
        }
    };
    for st in strains {
        let d = st.doctrine();
        let mut h = Swarm::new(st);
        let mut trail = Vec::new();
        for dist in [200.0f32, 60.0, 30.0, 18.0, 8.0, 3.0] {
            h.think(&scenario_swarm(dist, 6), 99.0);
            trail.push(match h.state {
                crate::hive::Hive::Crawl => ".",
                crate::hive::Hive::Dance => "d",
                crate::hive::Hive::Feed => "F",
            });
        }
        println!(
            "  {:<10} {:>6.1} {:>7} {:>8.1}   {}",
            st.name(),
            h.dance_gate(),
            d.feed_mass,
            d.recruit_radius,
            trail.join(" ")
        );
    }
    println!("\n  six infected spread over ~45 m; distances 200 60 30 18 8 3");
    println!("  ( . crawl   d dance   F feed )\n");
    println!("  the patient strains hold longest before tipping their hand. and");
    println!("  because there is no commander, there is nothing to decapitate --");
    println!("  but the DANCE can be interrupted: put distance back and the whole");
    println!("  thing subsides to crawling rather than chasing.");
}

fn scenario_swarm(dist: f32, n: usize) -> Snapshot {
    let units = (0..n)
        .map(|i| Unit {
            id: i as i32,
            // spread them out, so `recruit_radius` decides who answers the
            // dance. Clumped, every strain tips the instant a scout arrives
            // and the dance phase is never visible at all.
            pos: Vec3::new((i as f32 - 2.5) * 9.0, 0.0, dist),
            hp: 100.0,
            max_hp: 100.0,
            zone: 1,
            alive: true,
            is_commander: false,
            squad: -1,
            scattering: false,
        })
        .collect();
    Snapshot {
        tick: 1,
        player_pos: Vec3::ZERO,
        in_vehicle: false,
        signals: Signals { enemies_alive: n as i32, ..Default::default() },
        units,
        allies: Vec::new(),
    }
}

fn wire() {
    println!("THE SEAM — an FSM and a model are the same thing downstream\n");
    let snap = scenario(7, 1.0, 0);

    let mut fsm = Fsm::new("surrounder", 0);
    let a = fsm.think(&snap, 99.0).unwrap();
    println!("  FSM ({}):", fsm.who());
    println!("    {}\n", write_orders(&a));

    // a model that answers correctly, wrapped in the prose they always add
    let chatty = format!(
        "Sure! Given the squad is spread across all three zones I'd close the \
         ring:\n```json\n{}\n```\nThat should pressure them.",
        write_orders(&a)
    );
    let mut llm = LlmBrain::new(Canned { replies: vec![chatty], n: 0 }, "surrounder", 0);
    let b = llm.think(&snap, 99.0).unwrap();
    println!("  LLM (reply wrapped in prose and a code fence):");
    println!("    {}\n", write_orders(&b));
    println!(
        "  identical downstream: {}",
        if write_orders(&a) == write_orders(&b) { "yes" } else { "NO" }
    );

    println!("\n  and when the provider declines, or answers rubbish, or answers late:");
    for (label, reply) in [
        ("declines", None),
        ("rubbish", Some("I'm sorry, I can't help with that.".to_string())),
        ("stale", Some(write_orders(&Orders { tick: 0, speaker: "x".into(), cmds: a.cmds.clone() }))),
    ] {
        let mut l = LlmBrain::new(
            Canned { replies: reply.into_iter().collect(), n: 0 },
            "surrounder",
            0,
        );
        l.stale_ticks = 2;
        let got = l.think(&snap, 99.0);
        println!(
            "    {:<10} -> {:<28} {} commands still issued",
            label,
            l.last_rejected.unwrap_or("accepted"),
            got.map(|o| o.cmds.len()).unwrap_or(0)
        );
    }
    println!("\n  a squad that stops taking orders because a model is loading reads");
    println!("  as broken AI, and the player cannot tell the difference. So every");
    println!("  failure falls back to the FSM rather than stalling.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_round_trip_through_json() {
        let o = Orders {
            tick: 99,
            speaker: "blackstar".into(),
            cmds: vec![
                Command { verb: Verb::FlockSpread, zone: ZONE_ALL, point: Vec3::new(1.0, 2.0, 3.0), param0: 12.0, param1: 0.0 },
                Command::new(Verb::Hold, ZONE_BACK, Vec3::ZERO),
            ],
        };
        let back = read_orders(&write_orders(&o)).expect("should parse its own output");
        assert_eq!(back.tick, o.tick);
        assert_eq!(back.speaker, o.speaker);
        assert_eq!(back.cmds.len(), 2);
        assert_eq!(back.cmds[0].verb, Verb::FlockSpread);
        assert_eq!(back.cmds[0].point, Vec3::new(1.0, 2.0, 3.0));
        assert!((back.cmds[0].param0 - 12.0).abs() < 0.01);
        assert_eq!(back.cmds[1].zone, ZONE_BACK);
    }

    #[test]
    fn a_chatty_reply_still_decodes() {
        // THE REQUIREMENT, not a courtesy. A model asked for JSON returns JSON
        // wrapped in an explanation roughly whenever it feels like it, and a
        // parser that refuses those is a provider that fails intermittently
        // for reasons the player experiences as the enemy going stupid.
        let inner = write_orders(&Orders {
            tick: 5,
            speaker: "x".into(),
            cmds: vec![Command::new(Verb::Focus, ZONE_ALL, Vec3::ZERO)],
        });
        for wrapper in [
            format!("Certainly. Here are the orders:\n```json\n{inner}\n```\nHope that helps!"),
            format!("{inner}"),
            format!("<thinking>they are exposed</thinking>{inner}"),
            format!("  \n\n{inner}\n\n"),
        ] {
            let o = read_orders(&wrapper).unwrap_or_else(|| panic!("failed on: {wrapper}"));
            assert_eq!(o.cmds.len(), 1, "on: {wrapper}");
            assert_eq!(o.cmds[0].verb, Verb::Focus);
        }
        // and a reply with no orders in it is None rather than empty orders,
        // so the caller can tell "said nothing" from "said do nothing"
        assert!(read_orders("I can't help with that.").is_none());
    }

    #[test]
    fn unknown_verbs_and_junk_fields_are_skipped_not_fatal() {
        let j = r#"{"tick":3,"speaker":"m","mood":"smug","cmds":[
            {"verb":"nuke_from_orbit","zone":0,"point":[0,0,0]},
            {"verb":"hold","zone":1,"point":[1,1,1],"extra":"ignored"}]}"#;
        let o = read_orders(j).expect("should parse");
        assert_eq!(o.cmds.len(), 1, "the made-up verb should be dropped, not the reply");
        assert_eq!(o.cmds[0].verb, Verb::Hold);
    }

    #[test]
    fn nonsense_commands_are_refused_before_the_game_sees_them() {
        let o = Orders {
            tick: 1,
            speaker: "x".into(),
            cmds: vec![
                Command::new(Verb::Hold, 47, Vec3::ZERO),
                Command::new(Verb::Hold, ZONE_MID, Vec3::new(f32::NAN, 0.0, 0.0)),
                Command::new(Verb::Focus, ZONE_ALL, Vec3::ZERO),
            ],
        }
        .sanitised();
        assert_eq!(o.cmds.len(), 1, "a bad zone and a NaN point should both go");
        assert_eq!(o.cmds[0].verb, Verb::Focus);
    }

    #[test]
    fn the_fsm_emits_only_verbs_a_model_could_also_have_sent() {
        // "the deterministic layer is a strict subset" — checked rather than
        // asserted, across every archetype and several battlefields
        for c in COMMANDERS {
            for (hp, scat) in [(1.0, 0), (0.3, 0), (0.8, 4), (0.2, 5)] {
                let mut f = Fsm::new(c.id, 0);
                let o = f.think(&scenario(1, hp, scat), 99.0).expect("should decide");
                assert!(!o.cmds.is_empty(), "{} said nothing at hp {hp}", c.id);
                for cmd in &o.cmds {
                    assert!(cmd.sane(), "{} emitted an unusable command", c.id);
                    assert!(Verb::parse(cmd.verb.name()).is_some(), "verb not on the wire");
                }
                // and it round-trips, so the game cannot tell the source
                assert!(read_orders(&write_orders(&o)).is_some());
            }
        }
    }

    #[test]
    fn doctrine_actually_changes_the_answer() {
        // if every archetype behaves the same the table is decoration
        let s = scenario(1, 1.0, 0);
        let mut seen = std::collections::BTreeSet::new();
        for c in COMMANDERS {
            let mut f = Fsm::new(c.id, 0);
            f.think(&s, 99.0);
            seen.insert(format!("{:?}", f.posture));
        }
        assert!(seen.len() >= 2, "all five doctrines chose the same posture: {seen:?}");

        // and the specific claim: a cautious doctrine falls back when hurt
        // where an aggressive one does not
        let hurt = scenario(1, 0.3, 0);
        let mut warden = Fsm::new("warden", 0);
        let mut aggro = Fsm::new("aggressor", 0);
        warden.think(&hurt, 99.0);
        aggro.think(&hurt, 99.0);
        assert_eq!(warden.posture, Posture::Fallback);
        assert_ne!(aggro.posture, Posture::Fallback, "the aggressor should press through it");
    }

    #[test]
    fn a_commander_reads_its_own_squad_and_not_the_whole_battlefield() {
        // `Fsm.squad` came across from the C and was read by nothing, which
        // was not merely dead code: `choose` averaged strength over EVERY
        // live unit, so a commander whose own squad was being wiped out kept
        // pressing on the strength of somebody else's.
        let mut units = Vec::new();
        for i in 0..8 {
            units.push(Unit {
                id: i,
                pos: Vec3::new(i as f32, 0.0, 10.0),
                // squad 0 is nearly dead; squad 1 is fresh
                hp: if i < 4 { 8.0 } else { 100.0 },
                max_hp: 100.0,
                zone: 1,
                alive: true,
                is_commander: i == 0 || i == 4,
                squad: if i < 4 { 0 } else { 1 },
                scattering: false,
            });
        }
        let s = Snapshot { tick: 1, units, ..Default::default() };

        let mut dying = Fsm::new("warden", 0);
        let mut fresh = Fsm::new("warden", 1);
        dying.think(&s, 99.0);
        fresh.think(&s, 99.0);
        assert_eq!(dying.posture, Posture::Fallback, "its own squad is at 8% health");
        assert_ne!(fresh.posture, Posture::Fallback, "the other squad is untouched");

        // and a negative squad still means "the whole field", for the
        // single-commander case the rest of the tests use
        let mut all = Fsm::new("warden", -1);
        all.think(&s, 99.0);
        assert_ne!(all.posture, Posture::Fallback, "averaged, the field is healthy");
    }

    #[test]
    fn a_commander_thinks_on_a_cadence_rather_than_every_frame() {
        let mut f = Fsm::new("aggressor", 0);
        let s = scenario(1, 1.0, 0);
        let mut decisions = 0;
        for _ in 0..(60 * 10) {
            if f.think(&s, 1.0 / 60.0).is_some() {
                decisions += 1;
            }
        }
        // ten seconds at 1.25 s apart
        assert!((7..=9).contains(&decisions), "decided {decisions} times in 10 s");
    }

    #[test]
    fn stale_orders_are_refused() {
        // the gate that makes an asynchronous brain safe at all: a model reply
        // answering a battle two seconds gone describes a fight that is over
        let o = Orders { tick: 100, speaker: "x".into(), cmds: vec![] };
        assert!(o.fresh_for(100, 90));
        assert!(o.fresh_for(150, 90));
        assert!(!o.fresh_for(500, 90), "a 400-tick-old reply is not worth obeying");
    }

    #[test]
    fn every_provider_failure_falls_back_instead_of_stalling() {
        let snap = scenario(7, 1.0, 0);
        let good = write_orders(&Orders {
            tick: 7,
            speaker: "m".into(),
            cmds: vec![Command::new(Verb::Retreat, ZONE_ALL, Vec3::ZERO)],
        });
        let stale = write_orders(&Orders {
            tick: 0,
            speaker: "m".into(),
            cmds: vec![Command::new(Verb::Retreat, ZONE_ALL, Vec3::ZERO)],
        });
        for (label, reply, expect_reject) in [
            ("accepted", Some(good), false),
            ("declined", None, true),
            ("rubbish", Some("no thanks".into()), true),
            ("stale", Some(stale), true),
            ("empty", Some(r#"{"tick":7,"cmds":[]}"#.into()), true),
        ] {
            let mut l = LlmBrain::new(
                Canned { replies: reply.into_iter().collect(), n: 0 },
                "aggressor",
                0,
            );
            l.stale_ticks = 2;
            let got = l.think(&snap, 99.0);
            assert!(got.is_some(), "{label}: the squad stopped taking orders");
            assert!(!got.unwrap().cmds.is_empty(), "{label}: orders were empty");
            assert_eq!(l.last_rejected.is_some(), expect_reject, "{label}");
        }
    }

    #[test]
    fn the_model_is_trusted_for_verbs_and_never_for_the_tick() {
        // a model that echoes the wrong tick would otherwise poison freshness
        // for every later reply
        let snap = scenario(500, 1.0, 0);
        let lying = write_orders(&Orders {
            tick: 499,
            speaker: "m".into(),
            cmds: vec![Command::new(Verb::Focus, ZONE_ALL, Vec3::ZERO)],
        });
        let mut l =
            LlmBrain::new(Canned { replies: vec![lying], n: 0 }, "aggressor", 0);
        let o = l.think(&snap, 99.0).unwrap();
        assert_eq!(o.tick, 500, "the tick should be the snapshot's, not the model's");
    }

    #[test]
    fn the_prompt_and_the_wire_names_cannot_drift() {
        // the prompt tells a model the verb list; if `Verb::name` changes and
        // the prompt does not, the model is asked for a dialect nothing parses
        let p = system_prompt();
        for v in Verb::all() {
            assert!(p.contains(v.name()), "the prompt never mentions '{}'", v.name());
        }
        assert!(p.contains(&MAX_CMDS.to_string()));
    }

    #[test]
    fn a_snapshot_carries_the_signals_not_just_the_positions() {
        // "a brain that only sees positions plays chess" — the signals are
        // the reason the loop is worth closing
        let j = write_snapshot(&scenario(3, 1.0, 0));
        for key in ["aggression", "recent_dmg", "pressure", "kills", "player_hp"] {
            assert!(j.contains(key), "the snapshot omits '{key}'");
        }
        assert!(j.contains("\"tick\":3"));
        // dead units are not the brain's problem
        let mut s = scenario(3, 1.0, 0);
        s.units[0].alive = false;
        let j2 = write_snapshot(&s);
        assert!(j2.matches("\"id\":").count() < j.matches("\"id\":").count());
    }
}
