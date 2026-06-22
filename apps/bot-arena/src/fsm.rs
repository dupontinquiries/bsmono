//! An evolvable weighted finite-state machine used as a *generative* bot genome.
//!
//! A genome is N states (up to a slider cap) — each emitting a random character, a
//! pipe `|`, a space, or a command token — wired by up to M weighted out-edges per
//! state (slider cap). "Running" the machine is a weighted random walk that emits a
//! string (the bot's generated command / edit). The structure can be mutated and
//! crossed over (for evolution), rendered to SVG (for the heroes gallery), and
//! serialized.
//!
//! Pure: depends only on `rand`. No GTK, fully testable.

#![allow(dead_code)]

use rand::Rng;
use rand::rngs::SmallRng;
use rand::SeedableRng;

const CHAR_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_-.";
const TOKENS: &[&str] = &["cat", "grep", "sed", "sort", "uniq", "head", "tail", "awk", "wc", "tr"];

#[derive(Clone, Debug, PartialEq)]
pub enum StateKind {
    Char(char),    // emit a single character
    Pipe,          // emit " | "
    Space,         // emit " "
    Token(String), // emit a command-ish word
    Dup,           // re-emit the previous fragment (repetition combinator)
    Mark,          // capture the previous fragment into the register, and emit it
    Recall,        // emit the register's contents (backreference)
}

impl StateKind {
    pub fn emit(&self) -> String {
        match self {
            StateKind::Char(c) => c.to_string(),
            StateKind::Pipe => " | ".into(),
            StateKind::Space => " ".into(),
            StateKind::Token(t) => t.clone(),
            // memory kinds are resolved against walk state in `run`; statelessly empty
            StateKind::Dup | StateKind::Mark | StateKind::Recall => String::new(),
        }
    }
    pub fn label(&self) -> String {
        match self {
            StateKind::Char(c) => format!("'{c}'"),
            StateKind::Pipe => "|".into(),
            StateKind::Space => "␣".into(),
            StateKind::Token(t) => t.clone(),
            StateKind::Dup => "⟳".into(),
            StateKind::Mark => "⊙".into(),
            StateKind::Recall => "↺".into(),
        }
    }
    /// True for kinds whose emission depends on walk history (resolved in `run`).
    pub fn is_memory(&self) -> bool {
        matches!(self, StateKind::Dup | StateKind::Mark | StateKind::Recall)
    }
    fn random(rng: &mut impl Rng) -> StateKind {
        match rng.gen_range(0..12) {
            0..=5 => StateKind::Char(CHAR_ALPHABET[rng.gen_range(0..CHAR_ALPHABET.len())] as char),
            6..=7 => StateKind::Token(TOKENS[rng.gen_range(0..TOKENS.len())].to_string()),
            8 => StateKind::Pipe,
            9 => StateKind::Space,
            10 => StateKind::Dup,
            _ => if rng.gen_bool(0.5) { StateKind::Mark } else { StateKind::Recall },
        }
    }
}

#[derive(Clone, Debug)]
pub struct Fsm {
    pub kinds: Vec<StateKind>,
    pub edges: Vec<Vec<(usize, f32)>>, // per state: (target, weight)
    pub start: usize,
    pub max_conn: usize,
}

impl Fsm {
    /// Build a random machine: `n_states` states, each with 1..=`max_conn` weighted
    /// out-edges to other states.
    pub fn random(n_states: usize, max_conn: usize, rng: &mut impl Rng) -> Fsm {
        let n = n_states.max(1);
        let m = max_conn.clamp(1, n);
        let kinds: Vec<StateKind> = (0..n).map(|_| StateKind::random(rng)).collect();
        let edges: Vec<Vec<(usize, f32)>> = (0..n)
            .map(|i| {
                let k = rng.gen_range(1..=m);
                let mut targets: Vec<usize> = Vec::new();
                let mut guard = 0;
                while targets.len() < k && guard < k * 8 {
                    let t = rng.gen_range(0..n);
                    if t != i && !targets.contains(&t) { targets.push(t); }
                    guard += 1;
                }
                if targets.is_empty() { targets.push((i + 1) % n); }
                targets.into_iter().map(|t| (t, rng.gen_range(0.1..1.0))).collect()
            })
            .collect();
        Fsm { kinds, edges, start: 0, max_conn: m }
    }

    pub fn n_states(&self) -> usize { self.kinds.len() }

    /// Deterministic build from a seed — reproducible heroes.
    pub fn random_seeded(n_states: usize, max_conn: usize, seed: u64) -> Fsm {
        Fsm::random(n_states, max_conn, &mut SmallRng::seed_from_u64(seed))
    }

    /// Recombine two machines: child has min(|a|,|b|) states; each state's emission
    /// and out-edges are inherited from one parent or the other (edges clamped to the
    /// child's state range). The structural crossover for FSM evolution.
    pub fn crossover(a: &Fsm, b: &Fsm, rng: &mut impl Rng) -> Fsm {
        let n = a.kinds.len().min(b.kinds.len()).max(1);
        let max_conn = a.max_conn.max(b.max_conn).max(1);
        let mut kinds = Vec::with_capacity(n);
        let mut edges = Vec::with_capacity(n);
        for i in 0..n {
            kinds.push(if rng.gen_bool(0.5) { a.kinds[i].clone() } else { b.kinds[i].clone() });
            let src = if rng.gen_bool(0.5) { &a.edges[i] } else { &b.edges[i] };
            let mut e: Vec<(usize, f32)> = src.iter().copied().filter(|(t, _)| *t < n).take(max_conn).collect();
            if e.is_empty() { e.push(((i + 1) % n, rng.gen_range(0.1..1.0))); }
            edges.push(e);
        }
        Fsm { kinds, edges, start: 0, max_conn }
    }

    /// Walk the machine and emit a sequence of edit-operation *names* drawn from
    /// `ops`. This turns an FSM into a **mutation program**: rather than the engine
    /// picking an op at random each step, an evolved FSM emits a deterministic
    /// (per-seed) program of ops, mapping each state to `ops[state % ops.len()]`.
    /// The bridge from "FSM hero" to a policy the engine could actually run as a
    /// bot's edit sequence.
    pub fn run_ops(&self, ops: &[String], max_steps: usize, rng: &mut impl Rng) -> Vec<String> {
        let mut out = Vec::new();
        if ops.is_empty() || self.kinds.is_empty() { return out; }
        let mut cur = self.start.min(self.kinds.len() - 1);
        for _ in 0..max_steps {
            out.push(ops[cur % ops.len()].clone());
            let outs = &self.edges[cur];
            if outs.is_empty() { break; }
            let total: f32 = outs.iter().map(|(_, w)| *w).sum();
            if total <= 0.0 { break; }
            let mut pick = rng.gen_range(0.0..total);
            let mut next = outs[0].0;
            for (t, w) in outs {
                if pick < *w { next = *t; break; }
                pick -= *w;
            }
            cur = next;
        }
        out
    }

    /// Weighted random walk that emits a string — the bot's "command". Memory kinds
    /// (`Dup`/`Mark`/`Recall`) are resolved here against a `last` fragment and a single
    /// register, giving the generated output repetition and backreference structure.
    pub fn run(&self, max_steps: usize, rng: &mut impl Rng) -> String {
        let mut out = String::new();
        let mut last = String::new();
        let mut reg = String::new();
        let mut cur = self.start.min(self.kinds.len().saturating_sub(1));
        for _ in 0..max_steps {
            let frag = match &self.kinds[cur] {
                StateKind::Dup => last.clone(),
                StateKind::Mark => { reg = last.clone(); last.clone() }
                StateKind::Recall => reg.clone(),
                other => other.emit(),
            };
            if !frag.is_empty() { last = frag.clone(); }
            out.push_str(&frag);
            let outs = &self.edges[cur];
            if outs.is_empty() { break; }
            let total: f32 = outs.iter().map(|(_, w)| *w).sum();
            if total <= 0.0 { break; }
            let mut pick = rng.gen_range(0.0..total);
            let mut next = outs[0].0;
            for (t, w) in outs {
                if pick < *w { next = *t; break; }
                pick -= *w;
            }
            cur = next;
        }
        out
    }

    /// One random structural mutation, preserving invariants (targets in range,
    /// ≤ max_conn edges per node).
    pub fn mutate(&mut self, rng: &mut impl Rng) {
        let n = self.kinds.len();
        if n == 0 { return; }
        match rng.gen_range(0..4) {
            0 => { // reweight a random edge
                let s = rng.gen_range(0..n);
                if !self.edges[s].is_empty() {
                    let e = rng.gen_range(0..self.edges[s].len());
                    self.edges[s][e].1 = rng.gen_range(0.1..1.0);
                }
            }
            1 => { // add an edge if under the cap
                let s = rng.gen_range(0..n);
                if self.edges[s].len() < self.max_conn {
                    let t = rng.gen_range(0..n);
                    if t != s && !self.edges[s].iter().any(|(x, _)| *x == t) {
                        self.edges[s].push((t, rng.gen_range(0.1..1.0)));
                    }
                }
            }
            2 => { // remove an edge (keep at least one)
                let s = rng.gen_range(0..n);
                if self.edges[s].len() > 1 {
                    let e = rng.gen_range(0..self.edges[s].len());
                    self.edges[s].remove(e);
                }
            }
            _ => { // change a state's emission
                let s = rng.gen_range(0..n);
                self.kinds[s] = StateKind::random(rng);
            }
        }
    }

    /// Render the machine to a standalone SVG (state graph; node colour by kind,
    /// edge thickness by weight). This is the heroes-gallery "save to SVG".
    pub fn to_svg(&self) -> String {
        let n = self.kinds.len();
        let (w, h) = (640.0_f32, 640.0_f32);
        let (cx, cy, r) = (w / 2.0, h / 2.0, (w / 2.0) - 90.0);
        let pos: Vec<(f32, f32)> = (0..n).map(|i| {
            let a = (i as f32) / (n.max(1) as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            (cx + r * a.cos(), cy + r * a.sin())
        }).collect();

        let mut s = String::new();
        s.push_str(&format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" font-family="ui-monospace,monospace">"#));
        s.push_str(r##"<defs><marker id="a" markerWidth="9" markerHeight="9" refX="8" refY="3" orient="auto"><path d="M0,0 L8,3 L0,6 Z" fill="#888"/></marker></defs>"##);
        s.push_str(&format!(r##"<rect width="{w}" height="{h}" fill="#0f1116"/>"##));

        // edges first (under nodes)
        for (i, outs) in self.edges.iter().enumerate() {
            for (t, weight) in outs {
                let (x1, y1) = pos[i];
                let (x2, y2) = pos[*t];
                let sw = 0.6 + weight * 3.4;
                if i == *t {
                    s.push_str(&format!(r##"<circle cx="{:.1}" cy="{:.1}" r="16" fill="none" stroke="#4a5568" stroke-width="{:.1}"/>"##, x1, y1 - 28.0, sw));
                } else {
                    // shorten endpoints so the arrow sits at the node edge
                    let (dx, dy) = (x2 - x1, y2 - y1);
                    let len = (dx * dx + dy * dy).sqrt().max(1.0);
                    let (ux, uy) = (dx / len, dy / len);
                    let (sx, sy) = (x1 + ux * 26.0, y1 + uy * 26.0);
                    let (ex, ey) = (x2 - ux * 30.0, y2 - uy * 30.0);
                    s.push_str(&format!(r##"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="#4a5568" stroke-width="{:.1}" marker-end="url(#a)"/>"##, sx, sy, ex, ey, sw));
                }
            }
        }
        // nodes
        for (i, (x, y)) in pos.iter().enumerate() {
            let (fill, stroke) = match &self.kinds[i] {
                StateKind::Char(_) => ("#1e3a5f", "#4d9de0"),
                StateKind::Pipe => ("#5a3d1e", "#e0903d"),
                StateKind::Space => ("#2d3340", "#7d8799"),
                StateKind::Token(_) => ("#1e4030", "#3dd68c"),
                StateKind::Dup | StateKind::Mark | StateKind::Recall => ("#3a2a4d", "#a87fd6"),
            };
            let ring = if i == self.start { r#" stroke-dasharray="3 2""# } else { "" };
            s.push_str(&format!(r#"<circle cx="{x:.1}" cy="{y:.1}" r="24" fill="{fill}" stroke="{stroke}" stroke-width="2"{ring}/>"#));
            s.push_str(&format!(r##"<text x="{x:.1}" y="{:.1}" fill="#e6e9ef" font-size="13" text-anchor="middle">{}</text>"##, y + 4.0, xml_escape(&self.kinds[i].label())));
        }
        // footer: surface the functional tools — canonical (greedy) signature and
        // structural metrics — so each hero shows what it deterministically generates.
        let sig: String = self.run_greedy(48).chars().take(40).collect();
        let footer = format!("sig: {}   ·   {}/{} reachable   ·   bf {:.1}",
            if sig.is_empty() { "∅".into() } else { sig },
            self.reachable_count(), self.kinds.len(), self.branching_factor());
        s.push_str(&format!(r##"<text x="{:.1}" y="{:.1}" fill="#9aa3b2" font-size="13" text-anchor="middle">{}</text>"##,
            w / 2.0, h - 18.0, xml_escape(&footer)));
        s.push_str("</svg>");
        s
    }

    /// Deterministic walk: at each step take the highest-weight edge (ties broken by
    /// lowest target index), stopping when a state repeats on this path. No RNG — this
    /// is the machine's canonical "signature" output, stable across calls. Memory kinds
    /// are resolved as in `run`.
    pub fn run_greedy(&self, max_steps: usize) -> String {
        let mut out = String::new();
        let (mut last, mut reg) = (String::new(), String::new());
        if self.kinds.is_empty() { return out; }
        let mut cur = self.start.min(self.kinds.len() - 1);
        let mut seen = vec![false; self.kinds.len()];
        for _ in 0..max_steps {
            if seen[cur] { break; }
            seen[cur] = true;
            let frag = match &self.kinds[cur] {
                StateKind::Dup => last.clone(),
                StateKind::Mark => { reg = last.clone(); last.clone() }
                StateKind::Recall => reg.clone(),
                other => other.emit(),
            };
            if !frag.is_empty() { last = frag.clone(); }
            out.push_str(&frag);
            let outs = &self.edges[cur];
            if outs.is_empty() { break; }
            // highest weight, tie → lowest target index
            let next = outs.iter().fold(None::<(usize, f32)>, |acc, &(t, w)| match acc {
                Some((bt, bw)) if bw > w || (bw == w && bt <= t) => Some((bt, bw)),
                _ => Some((t, w)),
            }).map(|(t, _)| t).unwrap_or(cur);
            cur = next;
        }
        out
    }

    /// Number of states reachable from the start (BFS over edges).
    pub fn reachable_count(&self) -> usize {
        if self.kinds.is_empty() { return 0; }
        let mut seen = vec![false; self.kinds.len()];
        let mut stack = vec![self.start.min(self.kinds.len() - 1)];
        let mut n = 0;
        while let Some(s) = stack.pop() {
            if seen[s] { continue; }
            seen[s] = true; n += 1;
            for (t, _) in &self.edges[s] { if !seen[*t] { stack.push(*t); } }
        }
        n
    }

    /// Mean out-degree — the structural branching factor.
    pub fn branching_factor(&self) -> f64 {
        if self.edges.is_empty() { return 0.0; }
        self.edges.iter().map(|e| e.len()).sum::<usize>() as f64 / self.edges.len() as f64
    }

    /// Transpose: reverse every edge. Turns "what this state leads to" into "what leads
    /// here" — a structural operator that yields a very different generator from the
    /// same topology. `max_conn` widens to fit the new out-degrees.
    pub fn reverse(&self) -> Fsm {
        let n = self.kinds.len();
        let mut edges: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n];
        for (i, outs) in self.edges.iter().enumerate() {
            for &(t, w) in outs { edges[t].push((i, w)); }
        }
        let max_conn = edges.iter().map(|e| e.len()).max().unwrap_or(self.max_conn).max(self.max_conn).max(1);
        Fsm { kinds: self.kinds.clone(), edges, start: self.start.min(n.saturating_sub(1)), max_conn }
    }

    /// Drop states unreachable from the start and reindex — keeps evolved machines tidy
    /// without changing what they generate.
    pub fn prune_unreachable(&self) -> Fsm {
        let n = self.kinds.len();
        if n == 0 { return self.clone(); }
        let start = self.start.min(n - 1);
        let mut seen = vec![false; n];
        let mut stack = vec![start];
        while let Some(s) = stack.pop() {
            if seen[s] { continue; }
            seen[s] = true;
            for (t, _) in &self.edges[s] { if !seen[*t] { stack.push(*t); } }
        }
        let keep: Vec<usize> = (0..n).filter(|i| seen[*i]).collect();
        let remap: std::collections::HashMap<usize, usize> =
            keep.iter().enumerate().map(|(new, &old)| (old, new)).collect();
        let kinds = keep.iter().map(|&i| self.kinds[i].clone()).collect();
        let edges = keep.iter().map(|&i| {
            self.edges[i].iter().filter_map(|&(t, w)| remap.get(&t).map(|&nt| (nt, w))).collect()
        }).collect();
        Fsm { kinds, edges, start: remap[&start], max_conn: self.max_conn }
    }

    /// Merge structurally-identical states (same emission kind and same set of
    /// (target,weight) edges, weights rounded) into one, redirecting edges. A cheap
    /// one-pass minimization that shrinks redundant evolved machines.
    pub fn dedup_states(&self) -> Fsm {
        let n = self.kinds.len();
        if n == 0 { return self.clone(); }
        let sig = |i: usize| -> (String, Vec<(usize, i32)>) {
            let mut e: Vec<(usize, i32)> = self.edges[i].iter()
                .map(|&(t, w)| (t, (w * 1000.0) as i32)).collect();
            e.sort();
            (format!("{:?}", self.kinds[i]), e)
        };
        let mut rep: std::collections::HashMap<(String, Vec<(usize, i32)>), usize> = Default::default();
        let mut remap = vec![0usize; n];
        for i in 0..n {
            let s = sig(i);
            let r = *rep.entry(s).or_insert(i);
            remap[i] = r;
        }
        // representatives only, reindexed
        let reps: Vec<usize> = (0..n).filter(|&i| remap[i] == i).collect();
        let new_index: std::collections::HashMap<usize, usize> =
            reps.iter().enumerate().map(|(new, &old)| (old, new)).collect();
        let kinds = reps.iter().map(|&i| self.kinds[i].clone()).collect();
        let edges: Vec<Vec<(usize, f32)>> = reps.iter().map(|&i| {
            self.edges[i].iter().map(|&(t, w)| (new_index[&remap[t]], w)).collect()
        }).collect();
        let start = new_index[&remap[self.start.min(n - 1)]];
        Fsm { kinds, edges, start, max_conn: self.max_conn }
    }

    /// Sequence two machines: `b`'s states are appended after `a`'s, and `a`'s final
    /// state gains an edge into `b`'s start, so a walk can flow from one into the other.
    /// Compositional breeding that grows longer generators than mutation alone.
    pub fn concat(a: &Fsm, b: &Fsm) -> Fsm {
        let off = a.kinds.len();
        let mut kinds = a.kinds.clone();
        kinds.extend(b.kinds.iter().cloned());
        let mut edges = a.edges.clone();
        if off > 0 {
            edges[off - 1].push((off + b.start.min(b.kinds.len().saturating_sub(1)), 0.8));
        }
        for outs in &b.edges {
            edges.push(outs.iter().map(|&(t, w)| (t + off, w)).collect());
        }
        let max_conn = a.max_conn.max(b.max_conn).max(1) + 1;
        Fsm { kinds, edges, start: a.start.min(off.saturating_sub(1)), max_conn }
    }

    /// Enumerate distinct strings the machine can emit within `max_depth` steps (DFS
    /// over all paths, memory kinds resolved per path), capped at `cap` results. A
    /// finite *sample of the machine's language* — useful for inspecting what a hero
    /// actually generates, beyond a single random run.
    pub fn language_sample(&self, max_depth: usize, cap: usize) -> Vec<String> {
        let mut found = std::collections::BTreeSet::new();
        if self.kinds.is_empty() { return Vec::new(); }
        // stack of (state, depth, emitted-so-far, last, reg)
        let mut stack = vec![(self.start.min(self.kinds.len() - 1), 0usize, String::new(), String::new(), String::new())];
        while let Some((cur, depth, acc, last, reg)) = stack.pop() {
            if found.len() >= cap { break; }
            let frag = match &self.kinds[cur] {
                StateKind::Dup => last.clone(),
                StateKind::Mark => reg_then(&last),
                StateKind::Recall => reg.clone(),
                other => other.emit(),
            };
            let mut acc2 = acc.clone();
            acc2.push_str(&frag);
            let last2 = if frag.is_empty() { last.clone() } else { frag.clone() };
            let reg2 = if matches!(self.kinds[cur], StateKind::Mark) { last.clone() } else { reg.clone() };
            let outs = &self.edges[cur];
            if depth + 1 >= max_depth || outs.is_empty() {
                if !acc2.is_empty() { found.insert(acc2); }
                continue;
            }
            for &(t, _) in outs {
                stack.push((t, depth + 1, acc2.clone(), last2.clone(), reg2.clone()));
            }
        }
        found.into_iter().take(cap).collect()
    }
}

#[inline]
fn reg_then(last: &str) -> String { last.to_string() }

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('\'', "&#39;").replace('"', "&quot;")
}

/// Normalized similarity of two strings via longest common subsequence:
/// 2*LCS / (|a| + |b|), in [0, 1]. The fitness signal for evolving toward a target.
pub fn similarity(a: &str, b: &str) -> f64 {
    let (av, bv): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (n, m) = (av.len(), bv.len());
    if n == 0 && m == 0 { return 1.0; }
    if n == 0 || m == 0 { return 0.0; }
    let mut dp = vec![0usize; m + 1];
    for i in 1..=n {
        let mut prev = 0; // dp[i-1][j-1]
        for j in 1..=m {
            let tmp = dp[j];
            dp[j] = if av[i - 1] == bv[j - 1] { prev + 1 } else { dp[j].max(dp[j - 1]) };
            prev = tmp;
        }
    }
    2.0 * dp[m] as f64 / (n + m) as f64
}

/// Optional structural genetic operators for FSM evolution. All off = the original
/// crossover+mutation GA (bit-identical, since the toggles short-circuit before any RNG
/// draw). Each adds a different kind of structural variety or housekeeping.
#[derive(Clone, Copy, Default, Debug)]
pub struct EvoOps {
    pub concat: bool,  // sometimes breed by sequencing two parents (longer programs)
    pub reverse: bool, // sometimes transpose a child's edges (new generator, same topology)
    pub dedup: bool,   // merge structurally-identical states (legibility)
    pub prune: bool,   // drop unreachable states (legibility)
}

/// Evolve a population of FSMs to emit a string close to `target`. Elitist GA with
/// crossover + mutation; deterministic given `seed`. Returns (best machine, its
/// emitted output, best fitness). This makes the FSM a genome that actually *learns*
/// — the foundation for scoring evolved heroes.
pub fn evolve_toward(target: &str, n_states: usize, max_conn: usize, pop: usize, gens: usize, seed: u64) -> (Fsm, String, f64) {
    evolve_toward_with(target, n_states, max_conn, pop, gens, seed, EvoOps::default())
}

/// As `evolve_toward`, with the optional structural operators in `ops` enabled.
pub fn evolve_toward_with(target: &str, n_states: usize, max_conn: usize, pop: usize, gens: usize, seed: u64, ops: EvoOps) -> (Fsm, String, f64) {
    let mut rng = SmallRng::seed_from_u64(seed);
    let steps = (target.chars().count() * 2).max(8);
    let pop = pop.max(2);
    let mut machines: Vec<Fsm> = (0..pop).map(|_| Fsm::random(n_states, max_conn, &mut rng)).collect();
    let score = |f: &Fsm, rng: &mut SmallRng| similarity(&f.run(steps, rng), target);

    let mut best = machines[0].clone();
    let mut best_out = best.run(steps, &mut rng);
    let mut best_fit = similarity(&best_out, target);
    let size_cap = (n_states * 3).max(n_states + 2);

    for _ in 0..gens {
        let mut scored: Vec<(f64, usize)> = machines.iter().enumerate().map(|(i, f)| (score(f, &mut rng), i)).collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        let (top_fit, top_i) = scored[0];
        if top_fit > best_fit {
            best_fit = top_fit;
            best = machines[top_i].clone();
            best_out = best.run(steps, &mut rng);
        }
        let keep = (pop / 2).max(1);
        let survivors: Vec<Fsm> = scored.iter().take(keep).map(|&(_, i)| machines[i].clone()).collect();
        let mut next = Vec::with_capacity(pop);
        next.push(best.clone());
        while next.len() < pop {
            let a = &survivors[rng.gen_range(0..survivors.len())];
            let b = &survivors[rng.gen_range(0..survivors.len())];
            // concat: sequence two parents (bounded so programs don't grow without limit)
            let mut child = if ops.concat && a.n_states() + b.n_states() <= size_cap && rng.gen_bool(0.25) {
                Fsm::concat(a, b)
            } else {
                Fsm::crossover(a, b, &mut rng)
            };
            let muts = 1 + rng.gen_range(0..3);
            for _ in 0..muts { child.mutate(&mut rng); }
            // reverse: transpose edges for structural variety
            if ops.reverse && rng.gen_bool(0.15) { child = child.reverse(); }
            // housekeeping (no RNG): keep machines compact and legible
            if ops.prune { child = child.prune_unreachable(); }
            if ops.dedup { child = child.dedup_states(); }
            if child.kinds.is_empty() { child = Fsm::random(n_states, max_conn, &mut rng); }
            next.push(child);
        }
        machines = next;
    }
    (best, best_out, best_fit)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn run_ops_emits_from_vocabulary_and_is_deterministic() {
        let ops: Vec<String> = ["flip-op", "tweak-num", "dup-line", "no-op"]
            .iter().map(|s| s.to_string()).collect();
        let fsm = Fsm::random_seeded(6, 3, 999);
        let a = fsm.run_ops(&ops, 20, &mut SmallRng::seed_from_u64(7));
        let b = fsm.run_ops(&ops, 20, &mut SmallRng::seed_from_u64(7));
        assert_eq!(a, b, "same fsm + same rng seed → identical op program");
        assert!(!a.is_empty());
        assert!(a.iter().all(|o| ops.contains(o)), "every emitted op is from the vocabulary");
        // empty vocabulary → empty program (no panic)
        assert!(fsm.run_ops(&[], 20, &mut SmallRng::seed_from_u64(7)).is_empty());
    }

    #[test]
    fn random_respects_caps_and_runs() {
        let mut r = rand::thread_rng();
        let f = Fsm::random(8, 3, &mut r);
        assert_eq!(f.n_states(), 8);
        for outs in &f.edges {
            assert!(!outs.is_empty() && outs.len() <= 3, "1..=max_conn edges");
            for (t, w) in outs { assert!(*t < 8 && *w > 0.0); }
        }
        let cmd = f.run(40, &mut r);
        assert!(!cmd.is_empty(), "running emits a command");
    }

    #[test]
    fn mutate_preserves_invariants() {
        let mut r = rand::thread_rng();
        let mut f = Fsm::random(6, 2, &mut r);
        for _ in 0..200 { f.mutate(&mut r); }
        for outs in &f.edges {
            assert!(outs.len() <= 2, "stays under max_conn");
            for (t, _) in outs { assert!(*t < 6); }
        }
    }

    #[test]
    fn svg_export_is_wellformed_ish() {
        let mut r = rand::thread_rng();
        let f = Fsm::random(5, 2, &mut r);
        let svg = f.to_svg();
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert_eq!(svg.matches(r#"r="24""#).count(), 5, "one node circle per state");
    }

    #[test]
    fn seeded_is_deterministic() {
        let a = Fsm::random_seeded(7, 3, 42);
        let b = Fsm::random_seeded(7, 3, 42);
        let mut r1 = SmallRng::seed_from_u64(1);
        let mut r2 = SmallRng::seed_from_u64(1);
        assert_eq!(a.run(30, &mut r1), b.run(30, &mut r2), "same seed → same machine + output");
    }

    #[test]
    fn crossover_stays_valid() {
        let mut r = SmallRng::seed_from_u64(9);
        let a = Fsm::random(6, 3, &mut r);
        let b = Fsm::random(9, 2, &mut r);
        let child = Fsm::crossover(&a, &b, &mut r);
        assert_eq!(child.n_states(), 6, "child takes the smaller state count");
        for outs in &child.edges {
            assert!(!outs.is_empty());
            for (t, _) in outs { assert!(*t < 6, "edges clamped to child range"); }
        }
    }

    #[test]
    fn similarity_bounds() {
        assert_eq!(similarity("abc", "abc"), 1.0);
        assert_eq!(similarity("abc", "xyz"), 0.0);
        assert!(similarity("abc", "aXbXc") > 0.5);
    }

    #[test]
    fn evolution_improves_and_is_reproducible() {
        // baseline: best of an unevolved random batch
        let mut r = SmallRng::seed_from_u64(7);
        let steps = 8;
        let baseline = (0..40)
            .map(|_| similarity(&Fsm::random(10, 3, &mut r).run(steps, &mut r), "abcd"))
            .fold(0.0_f64, f64::max);
        let (_f, _out, fit) = evolve_toward("abcd", 10, 3, 40, 25, 7);
        assert!(fit >= baseline, "evolution does no worse than the best random ({fit} vs {baseline})");
        // determinism: same seed → same final fitness
        let (_f2, _o2, fit2) = evolve_toward("abcd", 10, 3, 40, 25, 7);
        assert_eq!(fit, fit2, "seeded evolution is reproducible");
    }

    // ----- functional toolkit -------------------------------------------------

    // helper: a hand-built machine with explicit kinds/edges
    fn machine(kinds: Vec<StateKind>, edges: Vec<Vec<(usize, f32)>>) -> Fsm {
        let max_conn = edges.iter().map(|e| e.len()).max().unwrap_or(1).max(1);
        Fsm { kinds, edges, start: 0, max_conn }
    }

    #[test]
    fn run_greedy_is_deterministic_and_stable() {
        let f = Fsm::random_seeded(12, 3, 99);
        let a = f.run_greedy(40);
        let b = f.run_greedy(40);
        assert_eq!(a, b, "greedy run is stable across calls (no RNG)");
        assert!(a.chars().count() <= 4000, "terminates via the cycle guard");
    }

    #[test]
    fn dup_repeats_and_recall_backreferences() {
        // s0 emits 'x', s1 Mark (capture+emit 'x'), s2 Recall (emit register), s3 Dup
        let f = machine(
            vec![StateKind::Char('x'), StateKind::Mark, StateKind::Recall, StateKind::Dup],
            vec![vec![(1, 1.0)], vec![(2, 1.0)], vec![(3, 1.0)], vec![(3, 1.0)]],
        );
        let out = f.run_greedy(5);
        // x (s0) · x (mark re-emits last) · x (recall reg) · x (dup last) = "xxxx"
        assert_eq!(out, "xxxx", "memory kinds repeat/backreference (got {out:?})");
    }

    #[test]
    fn reverse_transposes_edges() {
        let f = Fsm::random_seeded(8, 3, 5);
        let total: usize = f.edges.iter().map(|e| e.len()).sum();
        let r = f.reverse();
        let rtotal: usize = r.edges.iter().map(|e| e.len()).sum();
        assert_eq!(total, rtotal, "edge count preserved under transpose");
        // an edge i->t in f must appear as t->i in r
        for (i, outs) in f.edges.iter().enumerate() {
            for &(t, _) in outs {
                assert!(r.edges[t].iter().any(|&(x, _)| x == i), "reversed edge present");
            }
        }
    }

    #[test]
    fn prune_keeps_only_reachable() {
        // s0->s1 ; s2 is an island
        let f = machine(
            vec![StateKind::Char('a'), StateKind::Char('b'), StateKind::Char('c')],
            vec![vec![(1, 1.0)], vec![(0, 1.0)], vec![(0, 1.0)]],
        );
        assert_eq!(f.reachable_count(), 2);
        let p = f.prune_unreachable();
        assert_eq!(p.n_states(), 2, "island dropped");
        assert_eq!(p.reachable_count(), 2);
    }

    #[test]
    fn dedup_merges_identical_states() {
        // s1 and s2 are structurally identical (same kind, same edge to s0)
        let f = machine(
            vec![StateKind::Char('a'), StateKind::Char('b'), StateKind::Char('b')],
            vec![vec![(1, 0.5), (2, 0.5)], vec![(0, 1.0)], vec![(0, 1.0)]],
        );
        let d = f.dedup_states();
        assert!(d.n_states() < f.n_states(), "identical states merged ({} -> {})", f.n_states(), d.n_states());
        // still valid: all targets in range
        for outs in &d.edges { for &(t, _) in outs { assert!(t < d.n_states()); } }
    }

    #[test]
    fn concat_sequences_two_machines() {
        let a = Fsm::random_seeded(4, 2, 1);
        let b = Fsm::random_seeded(5, 2, 2);
        let c = Fsm::concat(&a, &b);
        assert_eq!(c.n_states(), a.n_states() + b.n_states());
        for outs in &c.edges { for &(t, _) in outs { assert!(t < c.n_states(), "targets in range"); } }
        // b's region is reachable from a's tail
        assert!(c.reachable_count() >= a.n_states(), "a's states reachable");
    }

    #[test]
    fn evo_ops_run_safely_and_bound_growth() {
        let all = EvoOps { concat: true, reverse: true, dedup: true, prune: true };
        let (best, _out, fit) = evolve_toward_with("cargo test", 8, 3, 30, 25, 3, all);
        assert!((0.0..=1.0).contains(&fit), "fitness in range");
        // concat is bounded + dedup/prune counteract growth → machine stays sane
        assert!(best.n_states() >= 1 && best.n_states() <= 8 * 3, "size bounded ({} states)", best.n_states());
        for outs in &best.edges { for &(t, _) in outs { assert!(t < best.n_states(), "valid targets"); } }
        // enabling ops is reproducible under a fixed seed
        let (_b2, _o2, fit2) = evolve_toward_with("cargo test", 8, 3, 30, 25, 3, all);
        assert_eq!(fit, fit2, "seeded evolution-with-ops is reproducible");
    }

    #[test]
    fn evo_default_matches_explicit_off() {
        // the toggles-off path must be identical to the original evolve_toward
        let (_a, _o, f1) = evolve_toward("abcd", 10, 3, 40, 25, 7);
        let (_b, _p, f2) = evolve_toward_with("abcd", 10, 3, 40, 25, 7, EvoOps::default());
        assert_eq!(f1, f2, "default EvoOps leaves evolution bit-identical");
    }

    #[test]
    fn language_sample_is_distinct_and_bounded() {
        let f = Fsm::random_seeded(10, 3, 42);
        let langs = f.language_sample(5, 16);
        assert!(langs.len() <= 16, "respects the cap");
        let mut sorted = langs.clone(); sorted.dedup();
        assert_eq!(sorted.len(), langs.len(), "results are distinct");
    }
}
