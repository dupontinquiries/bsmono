//! hive — the beehive matrix: which cell runs a job.
//!
//! `HiveMind_Strategy_Logscope.md` describes a hive of workers over a matrix
//! of models and machines. Three of its four axes already exist in this crate
//! as separate, correct modules, and nothing joined them:
//!
//! * [`crate::workerkit`] — the ROLE axis. `Reach` says whether a role may
//!   cross a network at all: a speculative draft alternates with its target
//!   every few tokens, so a 30 ms round trip is 37.5% overhead and a 150 ms
//!   one is 75% — the draft must be co-resident. A thinker amortises the same
//!   trip over a whole reply (1.8%).
//! * [`crate::capability`] — what a worker IS, so it can be told its job.
//! * [`crate::coordinator`] — the MODE, i.e. how many cells to light up.
//! * [`crate::continuation`] — the verb: what should happen next.
//!
//! The missing axis is the one that costs the most: **KV warmth**. A cell that
//! already holds the prefix for this prompt starts generating immediately; an
//! identical cell that does not must prefill first, and prefill is the
//! expensive pass (the field log's 1625-token first turn: 66 s of prefill
//! before a single token). The engine's prefix tree already measures this per
//! cache (`kvtree::pick_anchor`, pool cap 3). This module makes it a
//! placement input rather than a per-machine accident.
//!
//! ## The cell
//!
//! A [`Cell`] is one (model, node) pair with a KV state. The matrix is the set
//! of cells the hive can currently reach. Placement answers: given a [`Job`],
//! which cell should run it — or that none should, which is a real answer.
//!
//! ## The rule, in order
//!
//! 1. **Hard reach.** A co-resident role must land on the node holding its
//!    partner. No warmth or speed makes a remote draft correct.
//! 2. **Capability.** A cell whose model cannot do the job is not a candidate.
//! 3. **Warmth, then cost.** Among the survivors, prefer the cell that avoids
//!    a prefill; break ties by the node's own throughput, then by lowest
//!    latency. Warmth wins over raw speed by design: a faster machine that
//!    must prefill 1600 tokens loses to a slower one that must not, which is
//!    exactly the case a naive "fastest node" scheduler gets wrong.
//!
//! Pure: facts in, placement out. No I/O, no model calls, no clock — the same
//! discipline `continuation` keeps, and for the same reason (a scheduler that
//! costs a round trip to decide is one that gets switched off).

use crate::workerkit::Reach;

/// A machine in the hive.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    /// Round-trip latency to the coordinator, milliseconds. 0 for local.
    pub rtt_ms: u32,
    /// Rough decode throughput, tokens/sec, for tie-breaking only.
    pub tok_s: f32,
}

/// One cell of the matrix: a model resident on a node, with whatever KV it
/// currently holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub node: Node,
    /// The model's label — `read_harness`-style, so `8x7B` and `30B-A3B` parse.
    pub model: String,
    /// Longest prefix (in tokens) this cell's KV pool shares with the job's
    /// prompt. From the engine's own `pick_anchor`; 0 means a cold prefill.
    pub warm_prefix: usize,
    /// Whether this cell is already busy with another job.
    pub busy: bool,
}

/// What the hive is being asked to run.
#[derive(Clone, Debug)]
pub struct Job {
    /// The role, which decides whether the network is allowed at all.
    pub reach: Reach,
    /// The node holding this job's partner, for co-resident roles (a
    /// speculative draft's target, a verifier's subject).
    pub partner_node: Option<String>,
    /// Prompt length in tokens — what would have to be prefilled from cold.
    pub prompt_tokens: usize,
    /// Model labels acceptable for this job; empty means any.
    pub models: Vec<String>,
}

/// Why a cell was chosen, so a log line can explain a placement instead of
/// asserting one.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub cell: Cell,
    pub reason: String,
    /// Tokens this placement avoids prefilling versus a cold cell.
    pub saved_prefill: usize,
}

/// Choose a cell for `job`, or `None` when the matrix cannot serve it.
pub fn place(job: &Job, matrix: &[Cell]) -> Option<Placement> {
    let mut best: Option<(f64, Placement)> = None;
    for c in matrix {
        if c.busy {
            continue;
        }
        // 1. hard reach
        match job.reach {
            Reach::Colocated => {
                let Some(p) = job.partner_node.as_deref() else { continue };
                if c.node.id != p {
                    continue;
                }
            }
            _ => {}
        }
        // 2. capability
        if !job.models.is_empty() && !job.models.iter().any(|m| m == &c.model) {
            continue;
        }
        // 3. warmth, then cost. Score in seconds-ish: prefill dominates.
        let cold = job.prompt_tokens.saturating_sub(c.warm_prefix);
        let prefill_s = cold as f64 / (c.node.tok_s.max(0.1) as f64 * 8.0); // prefill ~8x decode
        let rtt_s = c.node.rtt_ms as f64 / 1000.0;
        let score = prefill_s + rtt_s;
        let saved = c.warm_prefix.min(job.prompt_tokens);
        let reason = if saved > 0 {
            format!("{} on {} — {saved} of {} prompt tokens already warm ({}ms rtt)",
                    c.model, c.node.id, job.prompt_tokens, c.node.rtt_ms)
        } else {
            format!("{} on {} — cold, {} tokens to prefill at {:.0} tok/s ({}ms rtt)",
                    c.model, c.node.id, cold, c.node.tok_s, c.node.rtt_ms)
        };
        let p = Placement { cell: c.clone(), reason, saved_prefill: saved };
        if best.as_ref().map(|(s, _)| score < *s).unwrap_or(true) {
            best = Some((score, p));
        }
    }
    best.map(|(_, p)| p)
}

/// Why no cell could serve a job — the operator-facing half of `place`
/// returning `None`. Distinguishing these matters: "everything is busy" is a
/// wait, "no model matches" is a configuration error, and "the partner's node
/// has no free cell" is a co-residency squeeze that adding remote machines
/// will not fix.
pub fn explain_empty(job: &Job, matrix: &[Cell]) -> String {
    if matrix.is_empty() {
        return "the hive is empty — no workers are enlisted".into();
    }
    let free = matrix.iter().filter(|c| !c.busy).count();
    if free == 0 {
        return format!("all {} cells are busy — this is a wait, not a failure", matrix.len());
    }
    if job.reach == Reach::Colocated {
        if let Some(p) = job.partner_node.as_deref() {
            let on_node = matrix.iter().filter(|c| c.node.id == p && !c.busy).count();
            if on_node == 0 {
                return format!(
                    "this role must be co-resident with its partner on {p}, which has no free cell. \
                     Adding remote machines cannot help: a draft alternating with its target pays the \
                     round trip PER TOKEN (see workerkit)");
            }
        } else {
            return "a co-resident role was given no partner node — the caller must say what it \
                    must sit beside".into();
        }
    }
    if !job.models.is_empty() {
        return format!("no free cell runs any of {:?} — this is configuration, not load", job.models);
    }
    "no cell matched, and the rules above do not explain why — this is a bug in place()".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, rtt: u32, tok_s: f32) -> Node {
        Node { id: id.into(), rtt_ms: rtt, tok_s }
    }
    fn cell(n: Node, model: &str, warm: usize) -> Cell {
        Cell { node: n, model: model.into(), warm_prefix: warm, busy: false }
    }

    /// The placement a naive scheduler gets wrong: a FASTER machine that must
    /// prefill loses to a slower one that is already warm. This is the whole
    /// reason KV warmth is an axis of the matrix rather than a local detail.
    #[test]
    fn warmth_beats_raw_speed() {
        let matrix = vec![
            cell(node("m2ultra", 30, 60.0), "Qwen3-14B", 0),
            cell(node("mini", 30, 20.0), "Qwen3-14B", 1600),
        ];
        let job = Job { reach: Reach::Lan, partner_node: None, prompt_tokens: 1700, models: vec![] };
        let p = place(&job, &matrix).expect("a cell must serve this");
        assert_eq!(p.cell.node.id, "mini", "the warm slower node must win: {}", p.reason);
        assert_eq!(p.saved_prefill, 1600);
        assert!(p.reason.contains("already warm"));
    }

    /// …but only when the warmth is real. With nothing warm anywhere, the
    /// fastest node wins, so the rule degrades to the obvious one.
    #[test]
    fn with_no_warmth_the_fast_node_wins() {
        let matrix = vec![
            cell(node("m2ultra", 30, 60.0), "Qwen3-14B", 0),
            cell(node("mini", 30, 20.0), "Qwen3-14B", 0),
        ];
        let job = Job { reach: Reach::Lan, partner_node: None, prompt_tokens: 1700, models: vec![] };
        let p = place(&job, &matrix).unwrap();
        assert_eq!(p.cell.node.id, "m2ultra", "{}", p.reason);
        assert_eq!(p.saved_prefill, 0);
        assert!(p.reason.contains("cold"));
    }

    /// Reach is HARD: a co-resident role may not be placed off its partner's
    /// node however warm or fast the alternative is. The tempting cell here is
    /// both warm AND fast, and must still lose.
    #[test]
    fn a_colocated_role_never_leaves_its_partner() {
        let matrix = vec![
            cell(node("elsewhere", 30, 90.0), "draft-1B", 1700),
            cell(node("host", 0, 10.0), "draft-1B", 0),
        ];
        let job = Job {
            reach: Reach::Colocated,
            partner_node: Some("host".into()),
            prompt_tokens: 1700,
            models: vec![],
        };
        let p = place(&job, &matrix).unwrap();
        assert_eq!(p.cell.node.id, "host",
                   "a speculative draft was placed across a network: {}", p.reason);
    }

    #[test]
    fn model_requirements_and_busy_cells_are_respected() {
        let matrix = vec![
            cell(node("a", 0, 50.0), "Qwen3-14B", 900),
            Cell { busy: true, ..cell(node("b", 0, 90.0), "deepseek-coder-6.7b", 1700) },
            cell(node("c", 40, 30.0), "deepseek-coder-6.7b", 0),
        ];
        let job = Job {
            reach: Reach::Lan,
            partner_node: None,
            prompt_tokens: 1700,
            models: vec!["deepseek-coder-6.7b".into()],
        };
        let p = place(&job, &matrix).unwrap();
        assert_eq!(p.cell.node.id, "c", "the busy warm cell must not be taken: {}", p.reason);
    }

    /// An empty answer must be explainable, and the three causes must not be
    /// confused: a wait, a misconfiguration, and a co-residency squeeze call
    /// for different operator actions.
    #[test]
    fn the_three_empty_causes_are_named_apart() {
        let job = |reach, partner: Option<&str>, models: Vec<String>| Job {
            reach, partner_node: partner.map(str::to_string), prompt_tokens: 100, models,
        };
        assert!(explain_empty(&job(Reach::Lan, None, vec![]), &[]).contains("hive is empty"));

        let busy = vec![Cell { busy: true, ..cell(node("a", 0, 50.0), "m", 0) }];
        assert!(explain_empty(&job(Reach::Lan, None, vec![]), &busy).contains("wait, not a failure"));

        let free = vec![cell(node("a", 0, 50.0), "m", 0)];
        let e = explain_empty(&job(Reach::Lan, None, vec!["other".into()]), &free);
        assert!(e.contains("configuration, not load"), "{e}");

        let e = explain_empty(&job(Reach::Colocated, Some("host"), vec![]), &free);
        assert!(e.contains("co-resident") && e.contains("PER TOKEN"), "{e}");
    }
}

// ── assembling the matrix ────────────────────────────────────────────────────

/// What one machine reports about itself: which models it holds resident, how
/// fast it decodes, and — the axis that matters — how much of a given prompt
/// each of its KV pool entries already covers.
///
/// This is the shape a node's own engine can fill without inventing anything:
/// `warm` comes from the prefix tree's `pick_anchor` against the prompt under
/// consideration, never from a guess about conversation shape (see
/// `docs/11-hive-matrix.md` on why a wrong prediction is worse than a cold
/// cell — the schedule commits to it).
#[derive(Clone, Debug, PartialEq)]
pub struct NodeReport {
    pub id: String,
    pub tok_s: f32,
    /// (model label, warm prefix tokens for THIS prompt, busy)
    pub residents: Vec<(String, usize, bool)>,
}

/// Build the matrix from live topology plus each node's report.
///
/// Latency comes from `rendezvous::Route::typical_rtt_ms` rather than a second
/// table in this module: two copies of a latency estimate drift, and the
/// routing layer is the one that knows what a relay costs. A peer that
/// reported nothing contributes no cells — an unreachable or silent machine is
/// absent from the matrix, not a cell that might work.
pub fn assemble(
    local: Option<&NodeReport>,
    peers: &[(String, crate::rendezvous::Route)],
    reports: &[NodeReport],
) -> Vec<Cell> {
    let mut out = Vec::new();
    let mut push = |r: &NodeReport, rtt: u32| {
        for (model, warm, busy) in &r.residents {
            out.push(Cell {
                node: Node { id: r.id.clone(), rtt_ms: rtt, tok_s: r.tok_s },
                model: model.clone(),
                warm_prefix: *warm,
                busy: *busy,
            });
        }
    };
    if let Some(l) = local {
        push(l, 0);
    }
    for (id, route) in peers {
        if local.map(|l| &l.id == id).unwrap_or(false) {
            continue; // the local node is not also a peer of itself
        }
        if let Some(r) = reports.iter().find(|r| &r.id == id) {
            push(r, route.typical_rtt_ms());
        }
    }
    out
}

/// Place a whole fan-out: `n` jobs across the matrix, no cell twice.
///
/// `coordinator::Mode` decides HOW MANY cells to light up; this decides WHICH,
/// and the two were unaware of each other. Greedy in warmth order is right
/// here rather than merely convenient: the branches are independent, so
/// assigning the warmest cell to any one of them saves the same prefill, and
/// the only question is that no cell is double-booked.
///
/// Returns as many placements as the matrix can serve, which may be fewer than
/// asked — an aggressive mode on a two-cell hive is a two-branch fan, not a
/// failure, and the caller can see the shortfall in the length.
pub fn place_fan(job: &Job, matrix: &[Cell], n: usize) -> Vec<Placement> {
    let mut remaining: Vec<Cell> = matrix.to_vec();
    let mut out = Vec::new();
    for _ in 0..n {
        let Some(p) = place(job, &remaining) else { break };
        // take that cell out of circulation, matching on node+model
        if let Some(i) = remaining.iter().position(|c| c.node.id == p.cell.node.id && c.model == p.cell.model) {
            remaining.remove(i);
        }
        out.push(p);
    }
    out
}

#[cfg(test)]
mod assembly_tests {
    use super::*;
    use crate::rendezvous::Route;

    fn report(id: &str, tok_s: f32, residents: &[(&str, usize, bool)]) -> NodeReport {
        NodeReport {
            id: id.into(),
            tok_s,
            residents: residents.iter().map(|(m, w, b)| (m.to_string(), *w, *b)).collect(),
        }
    }

    /// Assembly takes latency from the ROUTING layer, so a relayed peer is
    /// priced as a relay without this module holding its own opinion about
    /// what a relay costs.
    #[test]
    fn latency_comes_from_the_route_not_from_here() {
        let local = report("here", 40.0, &[("Qwen3-14B", 0, false)]);
        let reports = vec![report("lan-box", 20.0, &[("Qwen3-14B", 0, false)]),
                           report("far", 90.0, &[("Qwen3-14B", 0, false)])];
        let peers = vec![("lan-box".to_string(), Route::Lan), ("far".to_string(), Route::Relay)];
        let m = assemble(Some(&local), &peers, &reports);
        assert_eq!(m.len(), 3);
        let rtt = |id: &str| m.iter().find(|c| c.node.id == id).unwrap().node.rtt_ms;
        assert_eq!(rtt("here"), 0);
        assert_eq!(rtt("lan-box"), Route::Lan.typical_rtt_ms());
        assert_eq!(rtt("far"), Route::Relay.typical_rtt_ms());
    }

    /// A peer that reported nothing is ABSENT, not an empty-but-usable node.
    /// The local node is also not duplicated as a peer of itself.
    #[test]
    fn silent_peers_contribute_no_cells() {
        let local = report("here", 40.0, &[("m", 0, false)]);
        let peers = vec![("ghost".to_string(), Route::Lan), ("here".to_string(), Route::Loopback)];
        let m = assemble(Some(&local), &peers, &[local.clone()]);
        assert_eq!(m.len(), 1, "{m:#?}");
        assert_eq!(m[0].node.id, "here");
    }

    /// The end-to-end join: topology in, placement out, and the warm remote
    /// cell wins over the cold local one — the cross-machine case that had no
    /// mechanism before.
    #[test]
    fn a_warm_remote_cell_beats_a_cold_local_one() {
        let local = report("here", 60.0, &[("Qwen3-14B", 0, false)]);
        let reports = vec![report("mini", 20.0, &[("Qwen3-14B", 1600, false)])];
        let peers = vec![("mini".to_string(), Route::Lan)];
        let m = assemble(Some(&local), &peers, &reports);
        let job = Job { reach: Reach::Lan, partner_node: None, prompt_tokens: 1700, models: vec![] };
        let p = place(&job, &m).unwrap();
        assert_eq!(p.cell.node.id, "mini", "{}", p.reason);
        assert_eq!(p.saved_prefill, 1600);
    }

    /// A fan takes distinct cells, warmest first, and stops at what the matrix
    /// can serve rather than returning duplicates to fill the count.
    #[test]
    fn a_fan_never_double_books_a_cell() {
        let local = report("a", 30.0, &[("m", 900, false), ("m", 0, false)]);
        let reports = vec![report("b", 30.0, &[("m", 400, false)])];
        let peers = vec![("b".to_string(), Route::Lan)];
        let m = assemble(Some(&local), &peers, &reports);
        assert_eq!(m.len(), 3);

        let job = Job { reach: Reach::Lan, partner_node: None, prompt_tokens: 1000, models: vec![] };
        let fan = place_fan(&job, &m, 3);
        assert_eq!(fan.len(), 3);
        assert_eq!(fan[0].saved_prefill, 900, "warmest first");
        assert_eq!(fan[1].saved_prefill, 400);
        assert_eq!(fan[2].saved_prefill, 0);

        // asking for more than the matrix holds is a shorter fan, not a repeat
        let over = place_fan(&job, &m, 8);
        assert_eq!(over.len(), 3, "the fan invented cells to fill the count");
    }
}

// ── the wire: a node reporting its cells ─────────────────────────────────────

/// Wire version for a hive report. Separate from `announce`'s: the two travel
/// the same fabric but change for different reasons, and a shared version byte
/// would force a lockstep upgrade of both.
pub const REPORT_V1: u8 = 1;
const REPORT_TAG: &[u8] = b"logscope-hive-report-v1\0";

/// Bound on what one datagram may claim, so a hostile or broken peer cannot
/// make a receiver allocate. A machine with more than this many resident
/// models is not a case this protocol needs to serve.
pub const MAX_RESIDENTS: usize = 64;

impl NodeReport {
    /// Encode for the fabric. Same conventions as `announce::Announce`:
    /// version byte, tag, length-prefixed UTF-8 fields, little-endian numbers.
    pub fn payload(&self) -> Vec<u8> {
        let mut v = vec![REPORT_V1];
        v.extend_from_slice(REPORT_TAG);
        let put_str = |v: &mut Vec<u8>, s: &str| {
            v.extend_from_slice(&(s.len() as u32).to_le_bytes());
            v.extend_from_slice(s.as_bytes());
        };
        put_str(&mut v, &self.id);
        v.extend_from_slice(&self.tok_s.to_le_bytes());
        let n = self.residents.len().min(MAX_RESIDENTS);
        v.extend_from_slice(&(n as u32).to_le_bytes());
        for (model, warm, busy) in self.residents.iter().take(n) {
            put_str(&mut v, model);
            v.extend_from_slice(&(*warm as u64).to_le_bytes());
            v.push(if *busy { 1 } else { 0 });
        }
        v
    }

    /// Parse a report datagram body. Refuses rather than clamps — a corrupt
    /// datagram must not become a plausible one, which is `announce`'s rule
    /// and matters more here: a fabricated warmth number would misroute work.
    pub fn parse(buf: &[u8]) -> Result<NodeReport, crate::announce::Reject> {
        use crate::announce::Reject;
        if buf.is_empty() { return Err(Reject::Malformed("empty")); }
        if buf[0] != REPORT_V1 { return Err(Reject::Version(buf[0])); }
        if buf.len() < 1 + REPORT_TAG.len() || &buf[1..1 + REPORT_TAG.len()] != REPORT_TAG {
            return Err(Reject::Malformed("wrong tag"));
        }
        let mut o = 1 + REPORT_TAG.len();
        let field = |o: &mut usize| -> Result<String, Reject> {
            if buf.len() < *o + 4 { return Err(Reject::Malformed("truncated length")); }
            let n = u32::from_le_bytes([buf[*o], buf[*o + 1], buf[*o + 2], buf[*o + 3]]) as usize;
            *o += 4;
            if buf.len() < *o + n { return Err(Reject::Malformed("truncated field")); }
            let s = String::from_utf8(buf[*o..*o + n].to_vec())
                .map_err(|_| Reject::Malformed("field is not utf-8"))?;
            *o += n;
            Ok(s)
        };
        let id = field(&mut o)?;
        if buf.len() < o + 4 { return Err(Reject::Malformed("truncated tok_s")); }
        let tok_s = f32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
        o += 4;
        if !tok_s.is_finite() || tok_s < 0.0 {
            return Err(Reject::Malformed("tok_s is not a finite non-negative rate"));
        }
        if buf.len() < o + 4 { return Err(Reject::Malformed("truncated resident count")); }
        let n = u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()) as usize;
        o += 4;
        if n > MAX_RESIDENTS { return Err(Reject::Malformed("too many residents")); }
        let mut residents = Vec::with_capacity(n);
        for _ in 0..n {
            let model = field(&mut o)?;
            if buf.len() < o + 9 { return Err(Reject::Malformed("truncated resident")); }
            let warm = u64::from_le_bytes(buf[o..o + 8].try_into().unwrap()) as usize;
            o += 8;
            let busy = match buf[o] { 0 => false, 1 => true, _ => return Err(Reject::Malformed("busy is not a bool")) };
            o += 1;
            residents.push((model, warm, busy));
        }
        Ok(NodeReport { id, tok_s, residents })
    }
}

/// A peer's report plus when it arrived, so stale warmth can be dropped.
#[derive(Clone, Debug)]
pub struct HeldReport {
    pub report: NodeReport,
    pub at_s: u64,
}

/// How long a warmth claim stays credible.
///
/// Shorter than `announce::MAX_AGE_S` on purpose: an ADDRESS stays true for
/// a while, but warmth is invalidated by the peer's very next turn — it may
/// have taken another job and evicted the prefix this report described. A
/// stale warmth number is worse than none, because placement commits to it.
pub const REPORT_MAX_AGE_S: u64 = 10;

/// Reports still fresh enough to place work against.
pub fn fresh_reports(held: &[HeldReport], now_s: u64) -> Vec<NodeReport> {
    held.iter()
        .filter(|h| now_s.saturating_sub(h.at_s) <= REPORT_MAX_AGE_S && h.at_s <= now_s + 2)
        .map(|h| h.report.clone())
        .collect()
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use crate::announce::Reject;

    fn sample() -> NodeReport {
        NodeReport {
            id: "m2ultra".into(),
            tok_s: 62.5,
            residents: vec![("Qwen3-14B".into(), 1600, false), ("draft-1B".into(), 0, true)],
        }
    }

    #[test]
    fn a_report_round_trips() {
        let r = sample();
        assert_eq!(NodeReport::parse(&r.payload()).unwrap(), r);
    }

    /// Every truncation must be refused, not clamped into a plausible report:
    /// a report that parses with a fabricated warmth number misroutes work,
    /// which is worse than a dropped datagram.
    #[test]
    fn every_truncation_is_refused() {
        let full = sample().payload();
        for cut in 1..full.len() {
            match NodeReport::parse(&full[..cut]) {
                Err(_) => {}
                Ok(r) => panic!("a {cut}-byte prefix parsed as {r:?}"),
            }
        }
        assert!(matches!(NodeReport::parse(&[]), Err(Reject::Malformed("empty"))));
    }

    #[test]
    fn a_wrong_version_or_tag_is_named_apart() {
        let mut p = sample().payload();
        p[0] = 9;
        assert!(matches!(NodeReport::parse(&p), Err(Reject::Version(9))));
        let mut p = sample().payload();
        p[3] = b'X';
        assert!(matches!(NodeReport::parse(&p), Err(Reject::Malformed("wrong tag"))));
    }

    /// A peer claiming absurdities is refused at the boundary rather than
    /// allocated for.
    #[test]
    fn hostile_counts_and_rates_are_refused() {
        let mut p = sample().payload();
        // resident count sits right after id-field and the 4-byte tok_s
        let count_at = 1 + REPORT_TAG.len() + 4 + "m2ultra".len() + 4;
        p[count_at..count_at + 4].copy_from_slice(&(u32::MAX).to_le_bytes());
        assert!(matches!(NodeReport::parse(&p), Err(Reject::Malformed("too many residents"))));

        let mut p = sample().payload();
        let rate_at = 1 + REPORT_TAG.len() + 4 + "m2ultra".len();
        p[rate_at..rate_at + 4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(matches!(NodeReport::parse(&p), Err(Reject::Malformed(_))));
    }

    /// Warmth expires FAST. A report older than REPORT_MAX_AGE_S is dropped,
    /// because the peer may have evicted that prefix on its very next turn —
    /// and a report from the future is dropped too, the same replay reasoning
    /// announce::freshness uses.
    #[test]
    fn stale_and_future_reports_are_dropped() {
        let held = vec![
            HeldReport { report: sample(), at_s: 100 },
            HeldReport { report: NodeReport { id: "old".into(), ..sample() }, at_s: 80 },
            HeldReport { report: NodeReport { id: "future".into(), ..sample() }, at_s: 200 },
        ];
        let fresh = fresh_reports(&held, 105);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].id, "m2ultra");
    }
}
