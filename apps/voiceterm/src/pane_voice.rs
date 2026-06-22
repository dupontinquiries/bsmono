//! v0.1.849: parse spoken PANE_VOICE routing commands so multi-pane voice is
//! hands-free. Pure + unit-tested; applied by `pane_voice_dispatch` ONLY when
//! `WEB2QML_PANE_VOICE` is enabled. An unrecognized phrase returns `None` and
//! falls through to the normal "type the command into the pane" path, so normal
//! commands are never hijacked.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneVoiceCmd {
    /// Route subsequent voice commands to a specific pane id.
    SetTarget(u64),
    /// Clear the target → route to the focused pane (the default).
    ClearTarget,
    /// Add a pane to the broadcast set.
    BroadcastAdd(u64),
    /// Broadcast to every pane in the active tab (caller expands).
    BroadcastAll,
    /// Stop broadcasting (clear the set).
    BroadcastClear,
}

/// Spoken number → value. Digits or words, including the common whisper
/// mishears for small pane numbers ("to"/"too" → 2, "for" → 4).
fn word_to_num(s: &str) -> Option<u64> {
    if let Ok(n) = s.parse::<u64>() {
        return Some(n);
    }
    Some(match s {
        "zero" => 0,
        "one" => 1,
        "two" | "to" | "too" => 2,
        "three" => 3,
        "four" | "for" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        "eleven" => 11,
        "twelve" => 12,
        _ => return None,
    })
}

/// The pane number following the word "pane" (or "panel", a frequent mishear).
fn pane_num(toks: &[&str]) -> Option<u64> {
    for (i, t) in toks.iter().enumerate() {
        if *t == "pane" || *t == "panel" {
            if let Some(next) = toks.get(i + 1) {
                if let Some(n) = word_to_num(next) {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn norm(phrase: &str) -> String {
    phrase
        .trim()
        .trim_end_matches(['.', '!', '?', ','])
        .trim()
        .to_lowercase()
}

/// Parse a spoken phrase into a routing command, or `None` if it isn't one.
pub fn parse(phrase: &str) -> Option<PaneVoiceCmd> {
    let p = norm(phrase);
    if p.is_empty() {
        return None;
    }
    // Stop / clear broadcast (check the most specific phrases first).
    const STOP_BCAST: &[&str] = &[
        "stop broadcast", "stop broadcasting", "clear broadcast", "broadcast off",
        "end broadcast", "cancel broadcast", "stop sync", "unsync panes",
    ];
    if STOP_BCAST.contains(&p.as_str()) {
        return Some(PaneVoiceCmd::BroadcastClear);
    }
    // Clear target → route to the focused pane.
    const CLEAR_TGT: &[&str] = &[
        "clear target", "untarget", "send here", "target here", "route here",
        "voice here", "stop targeting", "target this pane", "send to this pane",
    ];
    if CLEAR_TGT.contains(&p.as_str()) {
        return Some(PaneVoiceCmd::ClearTarget);
    }
    // Broadcast to all panes.
    const BCAST_ALL: &[&str] = &[
        "broadcast to all", "broadcast all", "broadcast to all panes",
        "send to all", "send to all panes", "broadcast everywhere",
        "sync all panes", "sync panes", "broadcast to every pane",
    ];
    if BCAST_ALL.contains(&p.as_str()) {
        return Some(PaneVoiceCmd::BroadcastAll);
    }

    let toks: Vec<&str> = p.split_whitespace().collect();
    if let Some(id) = pane_num(&toks) {
        let is_broadcast =
            p.starts_with("broadcast") || p.contains("add pane") || p.contains("broadcast to pane");
        if is_broadcast {
            return Some(PaneVoiceCmd::BroadcastAdd(id));
        }
        let targets = p.starts_with("send ")
            || p.starts_with("target ")
            || p.starts_with("route ")
            || p.starts_with("voice ")
            || p.contains("to pane")
            || p.contains("target pane");
        if targets {
            return Some(PaneVoiceCmd::SetTarget(id));
        }
    }
    None
}

// ─────────────────────────── per-speaker routing ───────────────────────────

use std::collections::{BTreeSet, HashMap};

/// Where a speaker's next utterance should go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routing {
    /// No routing set → write to the focused pane (default).
    Focused,
    /// A single target pane.
    Target(u64),
    /// Broadcast to a set of panes (sorted, de-duplicated).
    Broadcast(Vec<u64>),
}

/// v0.1.854: per-speaker voice routing. Each enrolled speaker gets their OWN
/// target pane and broadcast set, so two people can drive different panes at the
/// same time. Keyed by speaker id (`speakers::current_active_id()`). A single or
/// unknown speaker resolves to the empty-string key — one entry — so solo use is
/// unchanged. Pure + unit-tested; the Backend holds exactly one of these.
#[derive(Default)]
pub struct VoiceRouting {
    target: HashMap<String, u64>,
    broadcast: HashMap<String, BTreeSet<u64>>,
}

impl VoiceRouting {
    pub fn target(&self, spk: &str) -> Option<u64> {
        self.target.get(spk).copied()
    }
    pub fn set_target(&mut self, spk: &str, pane: u64) {
        self.target.insert(spk.to_string(), pane);
    }
    pub fn clear_target(&mut self, spk: &str) {
        self.target.remove(spk);
    }

    /// This speaker's broadcast set, sorted ascending (empty if none).
    pub fn broadcast(&self, spk: &str) -> Vec<u64> {
        self.broadcast.get(spk).map(|s| s.iter().copied().collect()).unwrap_or_default()
    }
    /// Toggle one pane in this speaker's broadcast set; returns the new membership.
    pub fn broadcast_toggle(&mut self, spk: &str, pane: u64) -> bool {
        let set = self.broadcast.entry(spk.to_string()).or_default();
        let now_in = if set.remove(&pane) { false } else { set.insert(pane); true };
        if set.is_empty() { self.broadcast.remove(spk); }
        now_in
    }
    /// Add one pane; returns the set's new size.
    pub fn broadcast_add(&mut self, spk: &str, pane: u64) -> usize {
        let set = self.broadcast.entry(spk.to_string()).or_default();
        set.insert(pane);
        set.len()
    }
    /// Replace this speaker's broadcast set; returns the new size.
    pub fn set_broadcast_all(&mut self, spk: &str, panes: impl IntoIterator<Item = u64>) -> usize {
        let set: BTreeSet<u64> = panes.into_iter().collect();
        let n = set.len();
        if n == 0 { self.broadcast.remove(spk); } else { self.broadcast.insert(spk.to_string(), set); }
        n
    }
    pub fn clear_broadcast(&mut self, spk: &str) {
        self.broadcast.remove(spk);
    }

    /// A pane closed: drop it from every speaker's routing so nothing keeps
    /// writing to a dead pane.
    pub fn forget_pane(&mut self, pane: u64) {
        self.target.retain(|_, t| *t != pane);
        for set in self.broadcast.values_mut() { set.remove(&pane); }
        self.broadcast.retain(|_, s| !s.is_empty());
    }

    /// Resolve where THIS speaker's utterance goes: broadcast set if non-empty,
    /// else the single target if set, else the focused pane.
    pub fn resolve(&self, spk: &str) -> Routing {
        let b = self.broadcast(spk);
        if !b.is_empty() { return Routing::Broadcast(b); }
        match self.target(spk) {
            Some(t) => Routing::Target(t),
            None => Routing::Focused,
        }
    }

    /// Snapshot of every speaker with active routing: (speaker, target, broadcast).
    /// Used to render a multi-user routing panel.
    pub fn entries(&self) -> Vec<(String, Option<u64>, Vec<u64>)> {
        let mut keys: BTreeSet<&String> = BTreeSet::new();
        keys.extend(self.target.keys());
        keys.extend(self.broadcast.keys());
        keys.into_iter()
            .map(|k| (k.clone(), self.target.get(k).copied(), self.broadcast(k)))
            .collect()
    }
}

/// v0.1.856: bind physical microphone *sources* to panes. With one mic per
/// person, utterances captured on a given input device route straight to that
/// device's pane — no diarization needed, because the hardware already tells us
/// the source. Keyed by a stable mic source id (the cpal device name). Pure +
/// unit-tested; the Backend holds one of these and the capture pipeline tags each
/// utterance with the device it came from.
#[derive(Default)]
pub struct MicBindings {
    bind: HashMap<String, u64>,
}

impl MicBindings {
    pub fn bind(&mut self, mic: &str, pane: u64) {
        self.bind.insert(mic.to_string(), pane);
    }
    pub fn unbind(&mut self, mic: &str) {
        self.bind.remove(mic);
    }
    pub fn target(&self, mic: &str) -> Option<u64> {
        self.bind.get(mic).copied()
    }
    /// A pane closed → drop any mic bound to it.
    pub fn forget_pane(&mut self, pane: u64) {
        self.bind.retain(|_, p| *p != pane);
    }
    pub fn is_empty(&self) -> bool {
        self.bind.is_empty()
    }
    /// (mic, pane) pairs sorted by mic id — for a binding panel.
    pub fn entries(&self) -> Vec<(String, u64)> {
        let mut v: Vec<(String, u64)> = self.bind.iter().map(|(k, &p)| (k.clone(), p)).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::PaneVoiceCmd::*;
    use super::*;

    #[test]
    fn targets() {
        assert_eq!(parse("send to pane 2"), Some(SetTarget(2)));
        assert_eq!(parse("send this to pane two"), Some(SetTarget(2)));
        assert_eq!(parse("target pane 3"), Some(SetTarget(3)));
        assert_eq!(parse("route to pane 4"), Some(SetTarget(4)));
        assert_eq!(parse("voice to pane for"), Some(SetTarget(4))); // "for" -> 4
        assert_eq!(parse("Target pane 1."), Some(SetTarget(1)));
        assert_eq!(parse("send to panel 5"), Some(SetTarget(5))); // "panel" mishear
    }

    #[test]
    fn broadcast() {
        assert_eq!(parse("broadcast to all"), Some(BroadcastAll));
        assert_eq!(parse("broadcast to all panes"), Some(BroadcastAll));
        assert_eq!(parse("sync panes"), Some(BroadcastAll));
        assert_eq!(parse("broadcast to pane 2"), Some(BroadcastAdd(2)));
        assert_eq!(parse("add pane 3"), Some(BroadcastAdd(3)));
    }

    #[test]
    fn clears() {
        assert_eq!(parse("stop broadcasting"), Some(BroadcastClear));
        assert_eq!(parse("broadcast off"), Some(BroadcastClear));
        assert_eq!(parse("clear target"), Some(ClearTarget));
        assert_eq!(parse("send here"), Some(ClearTarget));
    }

    #[test]
    fn non_commands_fall_through() {
        // ordinary commands must NOT be hijacked
        assert_eq!(parse("cargo test"), None);
        assert_eq!(parse("cat pane.txt"), None); // "pane.txt" is not a bare "pane <n>"
        assert_eq!(parse("list files"), None);
        assert_eq!(parse("git status"), None);
        assert_eq!(parse("send the report"), None); // no pane number
        assert_eq!(parse(""), None);
    }

    #[test]
    fn routing_is_per_speaker_and_independent() {
        let mut r = VoiceRouting::default();
        r.set_target("alice", 1);
        r.set_target("bob", 2);
        // each speaker keeps their own target — the whole point of multi-user
        assert_eq!(r.resolve("alice"), Routing::Target(1));
        assert_eq!(r.resolve("bob"), Routing::Target(2));
        // an unrelated speaker is unaffected
        assert_eq!(r.resolve("carol"), Routing::Focused);
        // clearing one doesn't touch the other
        r.clear_target("alice");
        assert_eq!(r.resolve("alice"), Routing::Focused);
        assert_eq!(r.resolve("bob"), Routing::Target(2));
    }

    #[test]
    fn broadcast_beats_target_and_is_per_speaker() {
        let mut r = VoiceRouting::default();
        r.set_target("alice", 5);
        r.broadcast_add("alice", 2);
        r.broadcast_add("alice", 1);
        // broadcast wins over a set target, and comes back sorted+deduped
        assert_eq!(r.resolve("alice"), Routing::Broadcast(vec![1, 2]));
        assert!(!r.broadcast_toggle("alice", 1)); // remove 1
        assert_eq!(r.resolve("alice"), Routing::Broadcast(vec![2]));
        r.clear_broadcast("alice");
        // falls back to the still-set target once broadcast is empty
        assert_eq!(r.resolve("alice"), Routing::Target(5));
        // bob never had anything
        assert_eq!(r.resolve("bob"), Routing::Focused);
    }

    #[test]
    fn set_broadcast_all_and_forget_pane() {
        let mut r = VoiceRouting::default();
        assert_eq!(r.set_broadcast_all("alice", [3, 1, 2, 1]), 3); // deduped
        assert_eq!(r.resolve("alice"), Routing::Broadcast(vec![1, 2, 3]));
        r.set_target("bob", 2);
        // a pane closing is dropped from everyone's routing
        r.forget_pane(2);
        assert_eq!(r.resolve("alice"), Routing::Broadcast(vec![1, 3]));
        assert_eq!(r.resolve("bob"), Routing::Focused); // bob's target was pane 2
    }

    #[test]
    fn solo_unknown_speaker_uses_empty_key() {
        // current_active_id() returns "" when no speaker is identified — solo use
        let mut r = VoiceRouting::default();
        r.set_target("", 7);
        assert_eq!(r.resolve(""), Routing::Target(7));
    }

    #[test]
    fn mic_bindings_route_by_source() {
        let mut m = MicBindings::default();
        assert!(m.is_empty());
        m.bind("Blue Yeti", 1);
        m.bind("USB Audio CODEC", 2);
        // each mic routes to its own pane
        assert_eq!(m.target("Blue Yeti"), Some(1));
        assert_eq!(m.target("USB Audio CODEC"), Some(2));
        // an unbound mic has no target (caller falls back to speaker/focused)
        assert_eq!(m.target("Built-in"), None);
        // entries come back sorted by mic id
        assert_eq!(m.entries(), vec![
            ("Blue Yeti".to_string(), 1),
            ("USB Audio CODEC".to_string(), 2),
        ]);
        // rebinding a mic moves it
        m.bind("Blue Yeti", 3);
        assert_eq!(m.target("Blue Yeti"), Some(3));
    }

    #[test]
    fn mic_bindings_forget_pane_and_unbind() {
        let mut m = MicBindings::default();
        m.bind("mic-a", 1);
        m.bind("mic-b", 2);
        m.bind("mic-c", 1); // two mics on pane 1
        // closing pane 1 drops both mics bound to it, leaves mic-b
        m.forget_pane(1);
        assert_eq!(m.target("mic-a"), None);
        assert_eq!(m.target("mic-c"), None);
        assert_eq!(m.target("mic-b"), Some(2));
        m.unbind("mic-b");
        assert!(m.is_empty());
    }
}
