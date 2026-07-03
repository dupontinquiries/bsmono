//! voicefan.rs — turn live multi-SPEAKER speech into large conversation-tree fanouts.
//!
//! Upstream (the voiceterm speaker stack + whisper — the IO seam) attributes each transcribed utterance
//! to a speaker via voiceprint `identify`, producing an [`Utterance`] (`speaker` id, display `name`,
//! `text`, timestamp). This module expands each utterance into investigation branches through the
//! `speculate` ruleset (clarify / decompose / precheck / anticipate), tags every branch with its
//! originating speaker so results land under that speaker's node, and bounds the whole thing with a
//! [`FanBudget`] — interleaving branches round-robin across speakers so one talker can't hog the fanout
//! (the same balanced-fan-in idea as voiceterm's `mic_scheduler`). Branches carry `dispatch::Task`s for
//! the fleet to run; [`fold`] regroups the completed outputs into a per-speaker tree.
//!
//! Pure + unit-tested. The seams are upstream (whisper transcription + speaker attribution) and
//! downstream (the fleet `Dispatcher` and inserting the folded nodes into the real conversation graph).
#![allow(dead_code)]

use crate::dispatch::Task;
use crate::speculate::{investigation_lanes, Lane, LaneKind, SpecConfig};
use std::collections::HashMap;

pub type SpeakerId = String;

/// A speaker-attributed transcript fragment — what whisper + the speaker stack emit per utterance.
#[derive(Clone, Debug)]
pub struct Utterance {
    pub speaker: SpeakerId, // durable speaker id ("spk_…")
    pub name: String,       // display name (mutable upstream)
    pub text: String,       // the transcribed utterance
    pub t_ms: u64,          // capture timestamp
}

/// Bounds the fanout so "large" stays affordable. `max_total_lanes` caps total branches across all
/// speakers this round (size it from the fleet's spare capacity / `mic_scheduler` budget).
#[derive(Clone, Debug)]
pub struct FanBudget { pub max_total_lanes: usize }
impl Default for FanBudget { fn default() -> Self { Self { max_total_lanes: 24 } } }

/// One generated investigation branch, attributed to the speaker/utterance that seeded it.
#[derive(Clone, Debug)]
pub struct Branch { pub speaker: SpeakerId, pub utterance: usize, pub lane: Lane, pub task: Task }

/// The planned fanout: branches across all speakers (task ids globally unique so one `Dispatcher` runs
/// the whole batch).
#[derive(Debug, Default)]
pub struct Fanout { pub branches: Vec<Branch> }

/// Expand a batch of speaker utterances into a large, fair, bounded fanout. Each utterance is turned into
/// investigation lanes by `speculate`; branches are interleaved **round-robin across utterances** so the
/// `max_total_lanes` cap is shared fairly rather than consumed by the first (chattiest) speaker.
pub fn plan_fanout(utterances: &[Utterance], spec: &SpecConfig, budget: &FanBudget) -> Fanout {
    let per: Vec<Vec<Lane>> = utterances.iter().map(|u| investigation_lanes(&u.text, spec)).collect();
    let mut branches = Vec::new();
    let mut next_id = 0u64;
    let mut round = 0usize;
    while branches.len() < budget.max_total_lanes {
        let mut progressed = false;
        for (i, lanes) in per.iter().enumerate() {
            if branches.len() >= budget.max_total_lanes { break; }
            if let Some(lane) = lanes.get(round) {
                progressed = true;
                let task = Task { id: next_id, payload: lane.prompt.clone() };
                next_id += 1;
                branches.push(Branch { speaker: utterances[i].speaker.clone(), utterance: i, lane: lane.clone(), task });
            }
        }
        if !progressed { break; } // every utterance's lanes exhausted
        round += 1;
    }
    Fanout { branches }
}

impl Fanout {
    /// The dispatch tasks for the whole fanout (feed to a `Dispatcher`).
    pub fn tasks(&self) -> Vec<Task> { self.branches.iter().map(|b| b.task.clone()).collect() }
    pub fn len(&self) -> usize { self.branches.len() }
    pub fn is_empty(&self) -> bool { self.branches.is_empty() }
    /// Distinct speakers in first-seen order.
    pub fn speakers(&self) -> Vec<SpeakerId> {
        let mut seen = Vec::new();
        for b in &self.branches { if !seen.contains(&b.speaker) { seen.push(b.speaker.clone()); } }
        seen
    }
    /// How many branches each speaker got (id → count).
    pub fn share(&self) -> HashMap<SpeakerId, usize> {
        let mut m = HashMap::new();
        for b in &self.branches { *m.entry(b.speaker.clone()).or_insert(0) += 1; }
        m
    }
}

/// A speaker's folded results: the (lane kind, output) leaves produced from their utterances.
#[derive(Debug)]
pub struct SpeakerNode { pub speaker: SpeakerId, pub leaves: Vec<(LaneKind, String)> }

/// Regroup completed branch outputs (from the `Dispatcher`, keyed by task id) into a per-speaker tree,
/// speakers in first-seen order. Branches with no result yet are skipped.
pub fn fold(fanout: &Fanout, results: &HashMap<u64, String>) -> Vec<SpeakerNode> {
    let mut order: Vec<SpeakerId> = Vec::new();
    let mut map: HashMap<SpeakerId, Vec<(LaneKind, String)>> = HashMap::new();
    for b in &fanout.branches {
        if let Some(out) = results.get(&b.task.id) {
            if !map.contains_key(&b.speaker) { order.push(b.speaker.clone()); }
            map.entry(b.speaker.clone()).or_default().push((b.lane.kind, out.clone()));
        }
    }
    order.into_iter().map(|s| SpeakerNode { leaves: map.remove(&s).unwrap_or_default(), speaker: s }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(spk: &str, text: &str) -> Utterance {
        Utterance { speaker: spk.into(), name: spk.into(), text: text.into(), t_ms: 0 }
    }
    fn spec() -> SpecConfig {
        SpecConfig { aggressive: true, max_lanes: 4, min_context_chars: 8, kinds: LaneKind::all() }
    }

    #[test]
    fn fans_each_speaker_out_and_tags_branches() {
        let utts = vec![
            u("spk_a", "parse a large csv and compute stats per region"),
            u("spk_b", "then plot the regional totals as a bar chart"),
        ];
        let fan = plan_fanout(&utts, &spec(), &FanBudget { max_total_lanes: 24 });
        assert_eq!(fan.len(), 8, "2 speakers × 4 lanes each");
        assert_eq!(fan.speakers(), vec!["spk_a".to_string(), "spk_b".to_string()]);
        // task ids are globally unique 0..n
        let ids: Vec<u64> = fan.tasks().iter().map(|t| t.id).collect();
        assert_eq!(ids, (0..8).collect::<Vec<_>>());
        // every branch is attributed and its payload is the lane prompt
        assert!(fan.branches.iter().all(|b| (b.speaker == "spk_a" || b.speaker == "spk_b") && b.task.payload == b.lane.prompt));
    }

    #[test]
    fn budget_caps_and_shares_fairly_round_robin() {
        let utts = vec![
            u("spk_a", "parse a large csv and compute stats per region"),
            u("spk_b", "then plot the regional totals as a bar chart"),
        ];
        let fan = plan_fanout(&utts, &spec(), &FanBudget { max_total_lanes: 5 });
        assert_eq!(fan.len(), 5, "capped at the budget");
        let share = fan.share();
        // round-robin: a,b,a,b,a → 3 to the first speaker, 2 to the second (fair, not 5/0)
        assert_eq!(share["spk_a"], 3);
        assert_eq!(share["spk_b"], 2);
    }

    #[test]
    fn nothing_to_fan_when_not_aggressive_or_thin() {
        let utts = vec![u("spk_a", "hello there everyone")];
        let off = SpecConfig { aggressive: false, ..spec() };
        assert!(plan_fanout(&utts, &off, &FanBudget::default()).is_empty());
        let thin = vec![u("spk_a", "hi")];
        assert!(plan_fanout(&thin, &spec(), &FanBudget::default()).is_empty());
    }

    #[test]
    fn fold_groups_outputs_into_per_speaker_tree() {
        let utts = vec![
            u("spk_a", "parse a large csv and compute stats per region"),
            u("spk_b", "then plot the regional totals as a bar chart"),
        ];
        let fan = plan_fanout(&utts, &spec(), &FanBudget { max_total_lanes: 4 }); // a,b,a,b
        // mock the fleet returning an output for every task
        let results: HashMap<u64, String> = fan.tasks().iter().map(|t| (t.id, format!("lead#{}", t.id))).collect();
        let tree = fold(&fan, &results);
        assert_eq!(tree.len(), 2, "one node per speaker");
        assert_eq!(tree[0].speaker, "spk_a");
        assert_eq!(tree[0].leaves.len(), 2, "spk_a got 2 of the 4 branches");
        assert_eq!(tree[1].leaves.len(), 2);
        // first lane kind for each speaker is Clarify (speculate's first kind)
        assert_eq!(tree[0].leaves[0].0, LaneKind::Clarify);
    }

    // ── end-to-end: speech batch → fanout → fleet (loopback) → fold into the tree ──
    use crate::dispatch::{Dispatcher, Executor, Worker};
    use crate::enlist::{Msg, PeerId, Transport};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Bus { inbox: HashMap<PeerId, Vec<(PeerId, Msg)>>, peers: Vec<PeerId> }
    struct Loop { me: PeerId, bus: Rc<RefCell<Bus>> }
    impl Transport for Loop {
        fn broadcast(&mut self, m: &Msg) {
            let peers = self.bus.borrow().peers.clone();
            let mut b = self.bus.borrow_mut();
            for p in peers { if p != self.me { b.inbox.entry(p).or_default().push((self.me.clone(), m.clone())); } }
        }
        fn send(&mut self, to: &PeerId, m: &Msg) { self.bus.borrow_mut().inbox.entry(to.clone()).or_default().push((self.me.clone(), m.clone())); }
        fn recv(&mut self) -> Vec<(PeerId, Msg)> { self.bus.borrow_mut().inbox.get_mut(&self.me).map(std::mem::take).unwrap_or_default() }
    }
    struct Echo; // a stand-in "small model" that answers each lane
    impl Executor for Echo {
        fn run(&mut self, task: u64, _payload: &str) -> Result<String, String> { Ok(format!("lead#{task}")) }
    }

    #[test]
    fn end_to_end_speech_to_fanout_to_tree() {
        let utts = vec![
            u("spk_a", "parse a large csv and compute stats per region"),
            u("spk_b", "then plot the regional totals as a bar chart"),
        ];
        let fan = plan_fanout(&utts, &spec(), &FanBudget { max_total_lanes: 24 }); // 8 branches
        // run all branch tasks across two cheap-model members over the loopback bus
        let cid = "c:1".to_string();
        let mids = ["m0:1", "m1:1"];
        let bus = Rc::new(RefCell::new(Bus { inbox: HashMap::new(),
            peers: std::iter::once(cid.clone()).chain(mids.iter().map(|s| s.to_string())).collect() }));
        let mut ct = Loop { me: cid.clone(), bus: bus.clone() };
        let members: Vec<(PeerId, u32)> = mids.iter().map(|m| (m.to_string(), 4)).collect();
        let mut disp = Dispatcher::new(&cid, &members, fan.tasks(), 1);
        let mut workers: Vec<(Worker, Loop, Echo)> = mids.iter()
            .map(|id| (Worker::new(id), Loop { me: id.to_string(), bus: bus.clone() }, Echo)).collect();
        disp.dispatch(&mut ct);
        for _ in 0..20 {
            for (w, wt, ex) in workers.iter_mut() { w.work(wt, ex); }
            disp.pump(&mut ct);
            disp.dispatch(&mut ct);
            if disp.is_complete() { break; }
        }
        assert!(disp.is_complete());
        let tree = fold(&fan, disp.results());
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].leaves.len() + tree[1].leaves.len(), 8, "every branch folded back under a speaker");
    }
}
