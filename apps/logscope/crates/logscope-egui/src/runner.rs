//! runner.rs — a HEADLESS conversation-graph runner: the engine of the CLI test suite. It holds the
//! message tree, drives a `Backend` (the real wgpu engine on hardware, or a deterministic mock in the
//! sandbox), and executes the SAME command language as the GUI (`vimmode::parse_command` →
//! `consensus::fuzz` / `decompose` / `bestregion` / `treeexport`) — synchronously, with no egui and no
//! async worker. That's what makes the command flows testable off-GPU: run a script with the mock and
//! check the tree/export/control-flow here; run it with `--model` on the M2 to capture real outputs.
//!
//! Deliberate simplifications vs the GUI (it's a test harness, not the app): the prompt builder is a
//! minimal ChatML/Llama-3/Plain template over the lineage (no open-log injection), continuation (`:ca`)
//! appends a fresh assistant child of each leaf rather than literally resuming the same message, and
//! `:fill` assembles immediately (synchronous) instead of via a later `:assemble`.

use crate::backend::{Backend, TemplateKind};
use crate::vimmode::{self, Command};
use crate::{bestregion, consensus, decompose, explore, quest, treeexport, verdict};
use logscope::llm_backend::GenParams;
use crate::script::Step;

/// Cheap-checker system prompts for `:todo` (run on the small model with a small context).
const TODO_SYS: &str = "You scan one chat turn for unfinished work. If it states a task to do, an open \
question, or an explicit TODO, reply with that item as one short imperative line. If there is nothing \
open, reply exactly: NONE. No preamble.";
const MATCH_SYS: &str = "You judge whether a chat turn addresses a given TODO. Reply with exactly YES or \
NO — YES only if the turn actually answers, resolves, or makes real progress on that specific TODO.";

/// Tiny keyword set for the :todo pre-filter: lowercase alphanumeric words ≥4 chars, common stopwords out.
fn kw(text: &str) -> Vec<String> {
    const STOP: &[&str] = &["this", "that", "with", "from", "have", "will", "your", "what", "when", "does",
        "into", "then", "they", "them", "there", "here", "just", "like", "make", "made", "need", "needs",
        "also", "been", "were", "which", "about", "would", "could", "should", "their"];
    let mut v: Vec<String> = text.to_lowercase().split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 4 && !STOP.contains(w))
        .map(|w| w.to_string()).collect();
    v.sort_unstable(); v.dedup(); v
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role { User, Assistant }

#[derive(Clone)]
pub struct Node { pub role: Role, pub text: String, pub parent: Option<usize> }

/// A non-lineage, labeled edge between two nodes — a "weak link". The `parent` edge is the conversation
/// tree; weak links connect RELATED turns across branches (a TODO and the turn that addresses it, two
/// turns on the same thread). They power merged context (pull linked turns into a prompt, e.g.
/// `:mergectx ~addresses`) and a KV-friendly traversal order (`kv_order` groups linked turns so the
/// engine's anchor pool is reused instead of evicted between unrelated turns).
#[derive(Clone)]
pub struct WeakLink { pub from: usize, pub to: usize, pub label: String, pub weight: f32 }

pub struct Runner {
    pub nodes: Vec<Node>,
    pub links: Vec<WeakLink>,         // labeled cross-branch edges (see WeakLink); built by :todo
    pub active: Option<usize>,        // leaf new turns attach to
    pub models: Vec<(String, Box<dyn Backend>)>, // the model pool — :use switches which one runs
    pub active_model: usize,          // index into `models` that gen() currently drives
    pub system: String,
    pub params: GenParams,
    pub log: Vec<String>,             // per-step notes (printed to stderr by the CLI)
    fill: Option<(String, Vec<decompose::Hole>)>,
    verdicts: Vec<bestregion::Verdict>, // parallel to nodes; filled by :check (else None = use proxy)
    pub check_cmd: String,            // how :check compiles a node's code
    pub check_file: Option<String>,   // where to drop the code (None = temp .rs); set for a real-crate check
    pub gen_time: std::time::Duration, // total time spent in model generation (for KV-speedup timing)
    pub gen_count: usize,
}

impl Runner {
    pub fn new(models: Vec<(String, Box<dyn Backend>)>, system: String, params: GenParams) -> Self {
        Self {
            nodes: Vec::new(), links: Vec::new(), active: None, models, active_model: 0, system, params, log: Vec::new(), fill: None,
            verdicts: Vec::new(), check_cmd: verdict::DEFAULT_CHECK.to_string(), check_file: None,
            gen_time: std::time::Duration::ZERO, gen_count: 0,
        }
    }
    /// The model `gen()` currently runs on — every generation path goes through here, so `:use` (which just
    /// moves `active_model`) is all it takes to route a command to a different model class. Each backend keeps
    /// its OWN KV anchors, so switching models is a full prefill on the new one (correct — it hasn't seen the
    /// lineage), and reuse resumes once you stay on it (e.g. `:fill`'s sub-tasks all share the skeleton prefix
    /// on whichever model is active).
    fn active_tmpl(&self) -> TemplateKind { self.models[self.active_model].1.template() }

    // ── tree helpers ──────────────────────────────────────────────────────────
    fn push(&mut self, role: Role, text: String, parent: Option<usize>) -> usize {
        self.nodes.push(Node { role, text, parent });
        self.verdicts.push(bestregion::Verdict::None); // keep verdicts parallel to nodes
        self.nodes.len() - 1
    }
    fn lineage(&self, leaf: Option<usize>) -> Vec<usize> {
        let mut path = Vec::new();
        let mut cur = leaf;
        while let Some(i) = cur { path.push(i); cur = self.nodes.get(i).and_then(|n| n.parent); }
        path.reverse();
        path
    }
    fn leaf_ids(&self) -> Vec<usize> {
        let n = self.nodes.len();
        let mut is_parent = vec![false; n];
        for i in 0..n { if let Some(p) = self.nodes[i].parent { if p < n { is_parent[p] = true; } } }
        (0..n).filter(|&i| !is_parent[i]).collect()
    }
    /// Leaves of the subtree rooted at `root` (the active node). This is what `:fa`/`:ca` fan over, so a
    /// `:goto` onto the main line scopes a following burst to that branch — without it, `:fa` would widen
    /// EVERY leaf in the whole tree and bursts would compound across the entire frontier (runaway). With
    /// no active node, falls back to all leaves.
    fn subtree_leaves(&self, root: Option<usize>) -> Vec<usize> {
        let root = match root { Some(r) => r, None => return self.leaf_ids() };
        let n = self.nodes.len();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, nd) in self.nodes.iter().enumerate() { if let Some(p) = nd.parent { if p < n { children[p].push(i); } } }
        let mut leaves = Vec::new();
        let mut stack = vec![root];
        while let Some(x) = stack.pop() {
            if children[x].is_empty() { leaves.push(x); } else { stack.extend(children[x].iter().copied()); }
        }
        leaves.sort_unstable();
        leaves
    }
    /// The most recent user turn on the active lineage — the actual "task" for :quest.
    fn last_user_text(&self) -> Option<String> {
        for &i in self.lineage(self.active).iter().rev() {
            if self.nodes[i].role == Role::User { return Some(self.nodes[i].text.clone()); }
        }
        None
    }
    /// Texts of nodes NOT on the active lineage — sibling explorations, used as :quest option context.
    fn offpath_texts(&self) -> Vec<String> {
        let on: std::collections::HashSet<usize> = self.lineage(self.active).into_iter().collect();
        self.nodes.iter().enumerate().filter(|(i, _)| !on.contains(i)).map(|(_, n)| n.text.clone()).collect()
    }

    // ── generation ────────────────────────────────────────────────────────────
    fn build_prompt(&self, leaf: Option<usize>, tmpl: TemplateKind) -> String {
        let turns: Vec<(Role, &str)> = self.lineage(leaf).iter().map(|&i| (self.nodes[i].role, self.nodes[i].text.as_str())).collect();
        build_prompt(tmpl, &self.system, &turns)
    }
    fn gen(&mut self, prompt: &str) -> String {
        let mut out = String::new();
        let cancel = || false;
        let t0 = std::time::Instant::now();
        let am = self.active_model;
        let params = &self.params;                       // disjoint-field borrow: params + models don't overlap
        let be = &mut self.models[am].1;
        let _ = be.generate(prompt, params, &cancel, &mut |s| { out.push_str(s); true });
        self.gen_time += t0.elapsed();                   // `be`/`params` borrows end here (NLL)
        self.gen_count += 1;
        out.trim().to_string()
    }

    // ── weak links (built by :todo; see WeakLink) ────────────────────────────────
    /// Index of the pool's "small" model (the cheap checker), or the active model if there isn't one.
    fn small_model(&self) -> usize { self.models.iter().position(|(n, _)| n == "small").unwrap_or(self.active_model) }

    /// One-shot generation on a SPECIFIC model with a single user turn + custom system — the cheap TODO
    /// checks. Doesn't touch the graph (no node pushed); `user` is the deliberately small context.
    fn gen_raw_with(&mut self, model_idx: usize, system: &str, user: &str) -> String {
        let saved = self.active_model;
        self.active_model = model_idx;
        let tmpl = self.models[model_idx].1.template();
        let prompt = build_prompt(tmpl, system, &[(Role::User, user)]);
        let out = self.gen(&prompt);
        self.active_model = saved;
        out
    }

    /// Weak-link neighbours of `id` (either endpoint), with label + weight — the basis for merged context.
    pub fn weak_neighbors(&self, id: usize) -> Vec<(usize, &str, f32)> {
        self.links.iter().filter_map(|l| {
            if l.from == id { Some((l.to, l.label.as_str(), l.weight)) }
            else if l.to == id { Some((l.from, l.label.as_str(), l.weight)) }
            else { None }
        }).collect()
    }

    /// Order `ids` so weakly-linked turns sit adjacently: union the weak-link connected components, then emit
    /// each group (groups ordered by their earliest member, members in original order). Prefilling/decoding
    /// related turns back-to-back keeps the engine's anchor pool (FIFO, capped) warm instead of evicting it
    /// between unrelated turns — a cheap KV-cache win the batched path can adopt.
    pub fn kv_order(&self, ids: &[usize]) -> Vec<usize> {
        if ids.len() < 2 { return ids.to_vec(); }
        let pos: std::collections::HashMap<usize, usize> = ids.iter().enumerate().map(|(k, &v)| (v, k)).collect();
        fn find(p: &mut [usize], mut x: usize) -> usize { while p[x] != x { p[x] = p[p[x]]; x = p[x]; } x }
        let mut parent: Vec<usize> = (0..ids.len()).collect();
        for l in &self.links {
            if let (Some(&a), Some(&b)) = (pos.get(&l.from), pos.get(&l.to)) {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb { parent[ra] = rb; }
            }
        }
        let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = std::collections::BTreeMap::new();
        for k in 0..ids.len() { let r = find(&mut parent, k); groups.entry(r).or_default().push(k); }
        let mut grp: Vec<Vec<usize>> = groups.into_values().collect();
        grp.sort_by_key(|g| *g.iter().min().unwrap());
        let mut out = Vec::with_capacity(ids.len());
        for mut g in grp { g.sort_unstable(); out.extend(g.into_iter().map(|k| ids[k])); }
        out
    }

    /// Expand any `~label` tokens in a ref list to the node ids touched by weak links of that label (a bare
    /// `~` matches every label), returning (extra ids, the non-`~` refs left for normal resolution). This is
    /// how merged context follows weak links, e.g. `:mergectx ~addresses`.
    fn expand_link_refs(&self, refs: &[String]) -> (Vec<usize>, Vec<String>) {
        let (mut extra, mut rest) = (Vec::new(), Vec::new());
        for r in refs {
            if let Some(label) = r.strip_prefix('~') {
                for l in &self.links {
                    if label.is_empty() || l.label == label { extra.push(l.from); extra.push(l.to); }
                }
            } else { rest.push(r.clone()); }
        }
        extra.sort_unstable(); extra.dedup();
        (extra, rest)
    }

    fn say(&mut self, text: String) {
        let p = self.active;
        let u = self.push(Role::User, text, p);
        let prompt = self.build_prompt(Some(u), self.active_tmpl());
        let reply = self.gen(&prompt);
        let a = self.push(Role::Assistant, reply, Some(u));
        self.active = Some(a);
        self.log.push(format!("say → reply ({} chars); tree={} nodes", self.nodes[a].text.len(), self.nodes.len()));
    }

    /// Append a fresh assistant child of `leaf` (the harness's stand-in for "continue this branch").
    fn continue_leaf(&mut self, leaf: usize) {
        let prompt = self.build_prompt(Some(leaf), self.active_tmpl());
        let reply = self.gen(&prompt);
        self.push(Role::Assistant, reply, Some(leaf));
    }

    /// Append a user turn `text` as a child of `leaf`, then its reply.
    fn branch_say(&mut self, leaf: usize, text: String) {
        let u = self.push(Role::User, text, Some(leaf));
        let prompt = self.build_prompt(Some(u), self.active_tmpl());
        let reply = self.gen(&prompt);
        self.push(Role::Assistant, reply, Some(u));
    }

    /// One batched generation on the active model: N prompts in, N replies out, timed as a single op (but
    /// counted as N calls so the per-call average reflects the batching). The mock falls back to a loop;
    /// the wgpu engine decodes them together (`run_batched`).
    fn gen_batch(&mut self, prompts: &[String]) -> Vec<String> {
        if prompts.is_empty() { return Vec::new(); }
        let t0 = std::time::Instant::now();
        let am = self.active_model;
        let cancel = || false;
        let params = &self.params;
        let be = &mut self.models[am].1;
        let r = be.generate_batched(prompts, params, &cancel);
        self.gen_time += t0.elapsed();
        self.gen_count += prompts.len();
        r.into_iter().map(|s| s.trim().to_string()).collect()
    }

    /// Batched `:fa`: push every (leaf, user-turn) job, build all prompts, decode them in ONE batched pass,
    /// then attach the replies. Same tree as looping `branch_say`, but the branches share the decode.
    fn branch_say_batched(&mut self, jobs: Vec<(usize, String)>) {
        if jobs.is_empty() { return; }
        let tmpl = self.active_tmpl();
        let mut users = Vec::with_capacity(jobs.len());
        let mut prompts = Vec::with_capacity(jobs.len());
        for (leaf, text) in jobs {
            let u = self.push(Role::User, text, Some(leaf));
            prompts.push(self.build_prompt(Some(u), tmpl));
            users.push(u);
        }
        let replies = self.gen_batch(&prompts);
        for (u, reply) in users.into_iter().zip(replies) { self.push(Role::Assistant, reply, Some(u)); }
    }

    /// Batched `:ca`: continue every leaf in one batched pass (a fresh assistant child per leaf).
    fn continue_leaf_batched(&mut self, leaves: &[usize]) {
        if leaves.is_empty() { return; }
        let tmpl = self.active_tmpl();
        let prompts: Vec<String> = leaves.iter().map(|&l| self.build_prompt(Some(l), tmpl)).collect();
        let replies = self.gen_batch(&prompts);
        for (&l, reply) in leaves.iter().zip(replies) { self.push(Role::Assistant, reply, Some(l)); }
    }

    // ── export / report ───────────────────────────────────────────────────────
    fn export_nodes(&self) -> Vec<treeexport::ExportNode> {
        self.nodes.iter().enumerate().map(|(i, n)| treeexport::ExportNode {
            id: i, parent: n.parent,
            role: if n.role == Role::User { "you".into() } else { "assistant".into() },
            text: n.text.clone(),
        }).collect()
    }
    pub fn export(&self, fmt: &str) -> String {
        let nodes = self.export_nodes();
        match fmt {
            "ascii" | "txt" | "tree" => treeexport::ascii(&nodes),
            "git" | "sh" => treeexport::git_script(&nodes),
            _ => treeexport::markdown(&nodes),
        }
    }
    fn best_report(&self) -> String {
        let parents: Vec<Option<usize>> = self.nodes.iter().map(|n| n.parent).collect();
        let checked = self.verdicts.iter().any(|v| *v != bestregion::Verdict::None);
        let verdicts: Vec<bestregion::Verdict> = if checked {
            self.verdicts.clone()
        } else {
            self.nodes.iter().map(|n|
                if n.role == Role::Assistant && n.text.contains("```") { bestregion::Verdict::Pass } else { bestregion::Verdict::None }).collect()
        };
        let src = if checked { "compiled" } else { "code-block proxy" };
        match bestregion::best_region(&parents, &verdicts, 2, 0.5) {
            Some(r) => format!("# best region [{src}]: {} nodes from #{} (depth {}, {}/{} verified)", r.members.len(), r.root, r.max_depth, r.pass, r.total),
            None => format!("# best region [{src}]: nothing qualifies yet"),
        }
    }

    // ── command execution ─────────────────────────────────────────────────────
    /// Run one step; returns text to emit to stdout (export / best / report), if any.
    pub fn run_step(&mut self, step: &Step) -> Option<String> {
        match step {
            Step::Say(t) => { self.say(t.clone()); None }
            Step::Cmd(line) => self.run_command(vimmode::parse_command(line), line),
        }
    }

    pub fn run_command(&mut self, cmd: Command, raw: &str) -> Option<String> {
        match cmd {
            Command::ContinueAll(depth) => {
                for round in 0..depth {
                    let leaves = self.subtree_leaves(self.active);
                    self.continue_leaf_batched(&leaves);   // whole round decoded together
                    self.log.push(format!(":ca round {}/{} → tree={} nodes", round + 1, depth, self.nodes.len()));
                }
                None
            }
            Command::FuzzyAll { depth, width, template } => {
                let task = template.trim().to_string();
                if task.is_empty() { self.log.push(":fa needs a template (CLI has no composer)".into()); return None; }
                for round in 0..depth {
                    let leaves = self.subtree_leaves(self.active);
                    let mut jobs: Vec<(usize, String)> = Vec::new();
                    for l in leaves {
                        let lt: String = self.nodes[l].text.chars().take(200).collect();
                        let filled = vimmode::fill_template(&task, &lt, l);
                        for v in consensus::fuzz(&filled, width, 0x51_05_ed ^ l as u64) { jobs.push((l, v)); }
                    }
                    self.branch_say_batched(jobs);          // the whole round's branches decoded together
                    self.log.push(format!(":fa round {}/{} (w={width}) → tree={} nodes", round + 1, depth, self.nodes.len()));
                }
                None
            }
            Command::Fill(path) => {
                let skeleton = if !path.trim().is_empty() {
                    std::fs::read_to_string(path.trim()).unwrap_or_default()
                } else {
                    let raw = self.nodes.iter().rev().find(|n| n.role == Role::Assistant).map(|n| n.text.clone()).unwrap_or_default();
                    decompose::extract_code_block(&raw).unwrap_or(raw)  // a reply's skeleton is usually fenced
                };
                let holes = decompose::find_holes(&skeleton);
                if holes.is_empty() { self.log.push(":fill — no todo!() holes found".into()); return None; }
                // synchronous: fill every hole (KV-shared skeleton prefix), then assemble immediately
                let mut fills = Vec::new();
                for h in &holes {
                    let prompt_text = decompose::subtask_prompt(&skeleton, h);
                    // each sub-task is its own user turn off the active leaf (siblings sharing the skeleton)
                    let parent = self.active;
                    let u = self.push(Role::User, prompt_text.clone(), parent);
                    let prompt = self.build_prompt(Some(u), self.active_tmpl());
                    let body = self.gen(&prompt);
                    self.push(Role::Assistant, body.clone(), Some(u));   // keep the full reply in the tree
                    fills.push((h.clone(), decompose::extract_body(&body))); // but splice only the code
                }
                let assembled = decompose::assemble(&skeleton, &fills);
                self.fill = Some((skeleton, holes.clone()));
                self.log.push(format!(":fill → {} holes filled + assembled", holes.len()));
                Some(format!("# assembled ({} holes):\n```rust\n{}\n```", holes.len(), assembled))
            }
            Command::Assemble => {
                if let Some((skel, holes)) = self.fill.clone() {
                    let n = holes.len();
                    let mut bodies: Vec<String> = self.nodes.iter().rev().filter(|m| m.role == Role::Assistant).take(n).map(|m| decompose::extract_body(&m.text)).collect();
                    bodies.reverse();
                    if bodies.len() < n { self.log.push(":assemble — not enough replies yet".into()); return None; }
                    let fills: Vec<(decompose::Hole, String)> = holes.into_iter().zip(bodies).collect();
                    Some(format!("# assembled:\n```rust\n{}\n```", decompose::assemble(&skel, &fills)))
                } else { self.log.push(":assemble — run :fill first".into()); None }
            }
            Command::Best => Some(self.best_report()),
            // :check — the compile oracle: compile every assistant node's code and store a real Pass/Fail
            // verdict (replacing the code-block proxy that :best falls back to). With a real-crate check
            // configured (--check / --check-file) this is the keep-green / prune-red signal.
            Command::Check => {
                let (mut pass, mut fail, mut checked) = (0usize, 0usize, 0usize);
                for i in 0..self.nodes.len() {
                    if self.nodes[i].role != Role::Assistant { continue; }
                    let code = verdict::extract_code(&self.nodes[i].text);
                    let v = verdict::compile(&code, &self.check_cmd, self.check_file.as_deref());
                    self.verdicts[i] = v;
                    match v {
                        bestregion::Verdict::Pass => { pass += 1; checked += 1; }
                        bestregion::Verdict::Fail | bestregion::Verdict::Partial => { fail += 1; checked += 1; }
                        bestregion::Verdict::None => {}
                    }
                }
                self.log.push(format!(":check → {checked} graded: {pass} pass / {fail} fail  (`{}`)", self.check_cmd));
                Some(format!("# check: {pass}/{checked} compile  (cmd: {})", self.check_cmd))
            }
            // :explore — exploratory compilation over the current fan. Each assistant leaf under the active
            // node is a candidate; group them by parent (the subtask). Compile every DISTINCT candidate
            // exactly once through a batched verdict cache, write the verdicts back onto the nodes (like
            // :check), then keep the best-BUILDING candidate per subtask — a Pass beats a higher-quality
            // Fail, because a thing that doesn't compile isn't a candidate. The batched cache means K
            // samples × N subtasks collapse onto the set of things that are actually different.
            Command::Explore => {
                let leaves = self.subtree_leaves(self.active);
                let mut idx: Vec<usize> = Vec::new(); // node id per candidate (parallel to cands/evals)
                let mut cands: Vec<explore::Candidate> = Vec::new();
                for l in leaves {
                    if self.nodes[l].role != Role::Assistant { continue; }
                    let subtask = self.nodes[l].parent.unwrap_or(l) as u64;
                    // quality proxy: non-empty extracted-code length — a weak "more complete" tiebreak,
                    // only ever compared among candidates that reached the SAME verdict.
                    let q = verdict::extract_code(&self.nodes[l].text).trim().len() as f32;
                    idx.push(l);
                    cands.push(explore::Candidate::new(subtask, self.nodes[l].text.clone(), q));
                }
                if cands.is_empty() { self.log.push(":explore — no assistant candidates in the fan".into()); return None; }
                let mut oracle = explore::CompileOracle::new(self.check_cmd.clone(), self.check_file.clone());
                let mut cache = explore::VerdictCache::new();
                let evals = explore::evaluate_batch(&cands, &mut cache, &mut oracle);
                for (l, e) in idx.iter().zip(&evals) { self.verdicts[*l] = e.verdict; } // fold verdicts back
                let winners = explore::select_winners(&cands, &evals);
                let (hits, misses) = cache.stats();
                let built = winners.iter().filter(|w| w.built).count();
                // Land the tree on the best-building branch (like :best): the first node whose winning
                // candidate actually compiled.
                if let Some(w) = winners.iter().find(|w| w.built) {
                    if let Some(pos) = evals.iter().position(|e| e.hash == w.hash && e.verdict == bestregion::Verdict::Pass) {
                        self.active = Some(idx[pos]);
                    }
                }
                self.log.push(format!(
                    ":explore → {} subtask(s), {} candidate(s); {} distinct compile(s), {} served from cache; {}/{} build",
                    winners.len(), cands.len(), misses, hits, built, winners.len()));
                let mut report = format!("# explore: {}/{} subtasks build  ({} compiles, {} saved by the batched cache)\n",
                    built, winners.len(), misses, hits);
                // Surface the winning code per subtask — the actual compiling deliverable.
                for (subtask, content) in explore::winner_content(&winners, &cands) {
                    let w = winners.iter().find(|w| w.subtask == subtask);
                    let tag = if w.map(|w| w.built).unwrap_or(false) { "BUILDS" } else { "best-effort (nothing built)" };
                    let code = verdict::extract_code(content);
                    let code = code.trim();
                    report.push_str(&format!("\n## subtask {subtask} — {tag}\n```rust\n{}\n```\n",
                        if code.is_empty() { "(no code in reply)" } else { code }));
                }
                Some(report)
            }
            // :quest [N] — clarify first. Ask the model (out of band — no tree nodes) for the N most
            // important questions about the task, upgrade open ones to multiple-choice from sibling-branch
            // context, and emit a fillable Q/A template. Answer it (by hand or from another branch), then
            // :ans <file> folds the decisions into a focused generation.
            Command::Quest(n) => {
                let task = self.last_user_text().unwrap_or_else(|| self.active.map(|i| self.nodes[i].text.clone()).unwrap_or_default());
                if task.is_empty() { self.log.push(":quest — no task yet (say something first)".into()); return None; }
                let context = self.offpath_texts();
                let out = self.gen(&quest::quest_prompt(&task, n, &context));
                let mut qs = quest::parse_questions(&out);
                qs.truncate(n);
                for q in qs.iter_mut() {
                    if matches!(q.kind, quest::QKind::Open) {
                        let opts = quest::harvest_options(&q.text, &context, 3);
                        if opts.len() >= 2 { q.kind = quest::QKind::Choice(opts); }
                    }
                }
                self.log.push(format!(":quest → {} questions ({} from cross-branch context)", qs.len(),
                    qs.iter().filter(|q| matches!(q.kind, quest::QKind::Choice(_))).count()));
                if qs.is_empty() { None } else { Some(format!("# clarifying questions — fill the A: lines, then :ans <file>\n\n{}", quest::render_template(&qs))) }
            }
            Command::Answer(path) => {
                match std::fs::read_to_string(path.trim()) {
                    Ok(text) => {
                        let pairs = quest::parse_answers(&text);
                        if pairs.is_empty() { self.log.push(":ans — no filled A: lines found".into()); None }
                        else {
                            self.say(quest::clarified_context(&pairs)); // focused turn carrying the decisions
                            self.log.push(format!(":ans → folded {} decisions into a targeted reply", pairs.len()));
                            None
                        }
                    }
                    Err(e) => { self.log.push(format!(":ans — can't read {}: {e}", path.trim())); None }
                }
            }
            Command::Open(path) => {
                match std::fs::read_to_string(path.trim()) {
                    Ok(text) => { let n = self.import(&text); self.log.push(format!(":open {} → {} turns", path.trim(), n)); }
                    Err(e) => self.log.push(format!(":open — can't read {}: {e}", path.trim())),
                }
                None
            }
            Command::Goto(n) => { if !self.nodes.is_empty() { self.active = Some(n.saturating_sub(1).min(self.nodes.len() - 1)); } None }
            Command::Export(fmt) => Some(self.export(&fmt)),
            // :use <name> — route subsequent commands to a different model in the pool. The whole point of the
            // multi-model setup: scaffold with a big model (`:use big`, then a skeleton turn), fill with a small
            // one (`:use small`, then `:fill`), grade with the oracle (`:check`). No-op-with-warning if unknown.
            Command::Use(name) => {
                let want = name.trim();
                match self.models.iter().position(|(n, _)| n == want) {
                    Some(i) => {
                        self.active_model = i;
                        self.log.push(format!(":use {want} → model {}/{} ({})", i + 1, self.models.len(), self.models[i].1.name()));
                    }
                    None => {
                        let have: Vec<&str> = self.models.iter().map(|(n, _)| n.as_str()).collect();
                        self.log.push(format!(":use — no model named '{want}' in the pool (have: {})", have.join(", ")));
                    }
                }
                None
            }
            Command::Stop => { self.log.push(":stop (no-op in the synchronous CLI)".into()); None }
            Command::MergeCtx { refs, instruction } => {
                let (linked, plain) = self.expand_link_refs(&refs);
                let (mut ids, bad) = vimmode::resolve_refs(self.nodes.len(), &plain);
                for b in &bad { self.log.push(format!(":mergectx — '{b}' didn't resolve (ambiguous prefix or unknown id)")); }
                for x in linked { if !ids.contains(&x) { ids.push(x); } } // weak-linked nodes (e.g. ~addresses)
                if ids.is_empty() { self.log.push(":mergectx — no nodes resolved; nothing merged".into()); return None; }
                // Flatten the referenced nodes into ONE context — a numbered candidate list, deliberately
                // WITHOUT each node's branching ancestry, so the picker compares the replies side by side
                // (fan out cheaply, then hand the shortlist to a big model). Attached at root for the same
                // reason: build_prompt(this) sees only the candidates + the instruction, not an inherited thread.
                let mut merged = String::from("Below are candidate responses, each from a different branch.\n");
                for (k, &i) in ids.iter().enumerate() {
                    merged.push_str(&format!("\n## Candidate {} · id {}\n{}\n", k + 1, vimmode::short_id(i), self.nodes[i].text.trim()));
                }
                let instr = if instruction.trim().is_empty() {
                    "Pick the single best candidate. Begin with its id, then say why it wins and where the others fall short.".to_string()
                } else { instruction.trim().to_string() };
                merged.push_str(&format!("\n---\n{instr}"));
                let u = self.push(Role::User, merged, None);
                self.active = Some(u);
                let prompt = self.build_prompt(Some(u), self.active_tmpl());
                let reply = self.gen(&prompt);
                let a = self.push(Role::Assistant, reply, Some(u));
                self.active = Some(a);
                self.log.push(format!(":mergectx {} nodes → {} picked (tree={} nodes)", ids.len(), self.models[self.active_model].0, self.nodes.len()));
                None
            }
            Command::Strategy { refs, focus } => {
                // Default to the whole tree (a true bird's-eye); with refs, just those branches.
                let ids: Vec<usize> = if refs.is_empty() {
                    (0..self.nodes.len()).collect()
                } else {
                    let (ok, bad) = vimmode::resolve_refs(self.nodes.len(), &refs);
                    for b in &bad { self.log.push(format!(":strategy — '{b}' didn't resolve (ambiguous prefix or unknown id)")); }
                    ok
                };
                if ids.is_empty() { self.log.push(":strategy — nothing to summarize".into()); return None; }
                // Flatten the selected turns (each capped) into one transcript, prefixed by the
                // summarizer-worker prompt; generate the condensed synthesis with the active model and
                // attach it at root, so the digest opens a fresh, smaller space rather than extending a branch.
                let mut transcript = String::new();
                for &i in &ids {
                    let role = if self.nodes[i].role == Role::User { "User" } else { "Assistant" };
                    let t: String = self.nodes[i].text.chars().take(600).collect();
                    transcript.push_str(&format!("\n[{role} · {}] {}\n", vimmode::short_id(i), t.trim()));
                }
                let mut body = String::from(vimmode::STRATEGIST_PROMPT);
                if !focus.trim().is_empty() { body.push_str(&format!("\n\nFocus especially on: {}", focus.trim())); }
                body.push_str(&format!("\n\n--- CONVERSATION ({} turns) ---\n{transcript}", ids.len()));
                let u = self.push(Role::User, body, None);
                self.active = Some(u);
                let reply = { let prompt = self.build_prompt(Some(u), self.active_tmpl()); self.gen(&prompt) };
                let a = self.push(Role::Assistant, reply, Some(u));
                self.active = Some(a);
                self.log.push(format!(":strategy → condensed {} turns with {} (tree={} nodes)", ids.len(), self.models[self.active_model].0, self.nodes.len()));
                None
            }
            Command::Segment { refs, focus } => {
                let ids: Vec<usize> = if refs.is_empty() {
                    (0..self.nodes.len()).collect()
                } else {
                    let (ok, bad) = vimmode::resolve_refs(self.nodes.len(), &refs);
                    for b in &bad { self.log.push(format!(":segment — '{b}' didn't resolve (ambiguous prefix or unknown id)")); }
                    ok
                };
                if ids.is_empty() { self.log.push(":segment — nothing to map".into()); return None; }
                // Flatten with #position markers so the segmenter can cite the spans it cuts; the worker
                // prompt rides on top. Attach the block-map at root — a fresh skeleton beside the chat.
                let mut transcript = String::new();
                for (pos, &i) in ids.iter().enumerate() {
                    let role = if self.nodes[i].role == Role::User { "User" } else { "Assistant" };
                    let t: String = self.nodes[i].text.chars().take(500).collect();
                    transcript.push_str(&format!("\n[#{} {role} · {}] {}\n", pos + 1, vimmode::short_id(i), t.trim()));
                }
                let mut body = String::from(vimmode::SEGMENTER_PROMPT);
                if !focus.trim().is_empty() { body.push_str(&format!("\n\nPay special attention to: {}", focus.trim())); }
                body.push_str(&format!("\n\n--- TRANSCRIPT ({} turns) ---\n{transcript}", ids.len()));
                let u = self.push(Role::User, body, None);
                self.active = Some(u);
                let reply = { let prompt = self.build_prompt(Some(u), self.active_tmpl()); self.gen(&prompt) };
                let a = self.push(Role::Assistant, reply, Some(u));
                self.active = Some(a);
                self.log.push(format!(":segment → mapped {} turns into blocks with {} (tree={} nodes)", ids.len(), self.models[self.active_model].0, self.nodes.len()));
                None
            }
            Command::Todo { refs, focus } => {
                let scope: Vec<usize> = if refs.is_empty() { (0..self.nodes.len()).collect() } else {
                    let (ok, bad) = vimmode::resolve_refs(self.nodes.len(), &refs);
                    for b in &bad { self.log.push(format!(":todo — '{b}' didn't resolve")); }
                    ok
                };
                if scope.len() < 2 { self.log.push(":todo — need at least two turns to link".into()); return None; }
                let sm = self.small_model();
                let want = focus.trim().to_string();
                // PASS 1 — extract one open item per turn (small model, small context)
                let mut todos: Vec<(usize, String)> = Vec::new();
                for &i in &scope {
                    let mut txt: String = self.nodes[i].text.chars().take(420).collect();
                    if !want.is_empty() { txt = format!("(Only items about: {want})\n{txt}"); }
                    let line = self.gen_raw_with(sm, TODO_SYS, &txt).lines().next().unwrap_or("").trim().to_string();
                    if !line.is_empty() && !line.eq_ignore_ascii_case("none") && line.len() < 200 { todos.push((i, line)); }
                }
                if todos.is_empty() { self.log.push(":todo — no open items found".into()); return None; }
                // PASS 2 — cheap keyword pre-filter, then a small-model YES/NO match → a weak link
                let mut created = 0usize;
                for (ti, todo) in &todos {
                    let tkw = kw(&format!("{todo} {}", self.nodes[*ti].text));
                    let mut cands: Vec<(usize, usize)> = scope.iter().copied().filter(|&j| j != *ti)
                        .map(|j| { let jkw = kw(&self.nodes[j].text); (j, tkw.iter().filter(|w| jkw.contains(w)).count()) })
                        .filter(|&(_, ov)| ov > 0).collect();
                    cands.sort_by(|a, b| b.1.cmp(&a.1));
                    cands.truncate(4); // only check the few most-overlapping candidates (cheap samples)
                    let n_c = cands.len().max(1) as f32;
                    for (rank, (cj, ov)) in cands.into_iter().enumerate() {
                        let cand: String = self.nodes[cj].text.chars().take(420).collect();
                        let q = format!("TODO: {todo}\n\nTurn:\n{cand}\n\nDoes the turn address this TODO?");
                        if self.gen_raw_with(sm, MATCH_SYS, &q).to_ascii_uppercase().contains("YES") {
                            let w = ((ov as f32).min(6.0) / 6.0) * (1.0 - rank as f32 / (2.0 * n_c)); // overlap, decayed by rank
                            self.links.push(WeakLink { from: *ti, to: cj, label: "addresses".into(), weight: w });
                            created += 1;
                        }
                    }
                }
                for (ti, todo) in &todos { self.log.push(format!(":todo  open  {} · {todo}", vimmode::short_id(*ti))); }
                let start = self.links.len() - created;
                for l in &self.links[start..] {
                    self.log.push(format!(":todo  link  {} ⇢ {} [{}] w={:.2}", vimmode::short_id(l.from), vimmode::short_id(l.to), l.label, l.weight));
                }
                let order: Vec<String> = self.kv_order(&scope).iter().map(|&i| vimmode::short_id(i)).collect();
                self.log.push(format!(":todo → {} open item(s), {} weak link(s); merge them with ':mergectx ~addresses'; KV-order: {}",
                    todos.len(), created, order.join(" ")));
                None
            }
            other => { self.log.push(format!("(skipped — not wired in CLI yet: {raw} → {other:?})")); None }
        }
    }

    fn import(&mut self, text: &str) -> usize {
        let turns = parse_chat_md(text);
        if turns.is_empty() { return 0; }
        self.nodes.clear(); self.active = None;
        let mut parent = None;
        for (role, t) in &turns { let i = self.push(*role, t.clone(), parent); parent = Some(i); self.active = Some(i); }
        turns.len()
    }
}

// ── minimal prompt builder (template over the lineage) ────────────────────────
/// Neutralize special-token sigils in CONTENT only (not the structural markers we add), the same fix the
/// GUI uses: a zero-width space after the `<` of `<|`/`<｜` reads as text and no longer tokenizes as a
/// control token, so an imported message containing `<|im_end|>` can't corrupt the turn structure.
fn neutralize(s: &str) -> String {
    s.replace("<|", "<\u{200b}|").replace("<\u{ff5c}", "<\u{200b}\u{ff5c}")
}

fn build_prompt(tmpl: TemplateKind, system: &str, turns: &[(Role, &str)]) -> String {
    let mut s = String::new();
    match tmpl {
        TemplateKind::ChatML => {
            if !system.is_empty() { s.push_str(&format!("<|im_start|>system\n{}<|im_end|>\n", neutralize(system))); }
            for (role, text) in turns {
                let r = if *role == Role::User { "user" } else { "assistant" };
                s.push_str(&format!("<|im_start|>{r}\n{}<|im_end|>\n", neutralize(text)));
            }
            s.push_str("<|im_start|>assistant\n");
        }
        TemplateKind::Llama3 => {
            s.push_str("<|begin_of_text|>");
            if !system.is_empty() { s.push_str(&format!("<|start_header_id|>system<|end_header_id|>\n\n{}<|eot_id|>", neutralize(system))); }
            for (role, text) in turns {
                let r = if *role == Role::User { "user" } else { "assistant" };
                s.push_str(&format!("<|start_header_id|>{r}<|end_header_id|>\n\n{}<|eot_id|>", neutralize(text)));
            }
            s.push_str("<|start_header_id|>assistant<|end_header_id|>\n\n");
        }
        TemplateKind::Llama4 => {
            // Llama-3's header discipline with Llama4's marker spellings (<|header_start|>, <|eot|>).
            s.push_str("<|begin_of_text|>");
            if !system.is_empty() { s.push_str(&format!("<|header_start|>system<|header_end|>\n\n{}<|eot|>", neutralize(system))); }
            for (role, text) in turns {
                let r = if *role == Role::User { "user" } else { "assistant" };
                s.push_str(&format!("<|header_start|>{r}<|header_end|>\n\n{}<|eot|>", neutralize(text)));
            }
            s.push_str("<|header_start|>assistant<|header_end|>\n\n");
        }
        TemplateKind::Gemma => {
            // Gemma 4: <|turn>ROLE\n…<turn|> (changed from Gemma 3), native system role; BOS added by backend.
            if !system.is_empty() { s.push_str(&format!("<|turn>system\n{}<turn|>\n", neutralize(system))); }
            for (role, text) in turns {
                let r = if *role == Role::User { "user" } else { "model" };
                s.push_str(&format!("<|turn>{r}\n{}<turn|>\n", neutralize(text)));
            }
            s.push_str("<|turn>model\n");
        }
        TemplateKind::Plain => {
            if !system.is_empty() { s.push_str(&format!("{system}\n\n")); }
            for (role, text) in turns {
                let r = if *role == Role::User { "User" } else { "Assistant" };
                s.push_str(&format!("{r}: {text}\n"));
            }
            s.push_str("Assistant:");
        }
    }
    s
}

/// Parse a `:export md`-shaped chat (## You / ## Assistant sections per turn) back into turns.
fn parse_chat_md(text: &str) -> Vec<(Role, String)> {
    let mut turns: Vec<(Role, String)> = Vec::new();
    let mut role: Option<Role> = None;
    let mut buf = String::new();
    let flush = |turns: &mut Vec<(Role, String)>, role: Option<Role>, buf: &str| {
        if let Some(r) = role { let t = buf.trim(); if !t.is_empty() { turns.push((r, t.to_string())); } }
    };
    for line in text.lines() {
        let l = line.trim_start();
        let header = l.strip_prefix("## ").or_else(|| l.strip_prefix("# "));
        if let Some(h) = header {
            let hl = h.trim().to_lowercase();
            let next = if hl.starts_with("you") || hl.starts_with("user") { Some(Role::User) }
                       else if hl.starts_with("assistant") || hl.starts_with("asst") { Some(Role::Assistant) }
                       else { None };
            if next.is_some() { flush(&mut turns, role, &buf); buf.clear(); role = next; continue; }
        }
        if role.is_some() { buf.push_str(line); buf.push('\n'); }
    }
    flush(&mut turns, role, &buf);
    turns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chatml_prompt_has_turns_and_primer() {
        let turns = [(Role::User, "hi"), (Role::Assistant, "hello"), (Role::User, "more")];
        let p = build_prompt(TemplateKind::ChatML, "be terse", &turns);
        assert!(p.contains("<|im_start|>system\nbe terse<|im_end|>"));
        assert!(p.contains("<|im_start|>user\nmore<|im_end|>"));
        assert!(p.ends_with("<|im_start|>assistant\n"), "ends on the assistant primer");
    }

    #[test]
    fn content_markers_are_neutralized() {
        let turns = [(Role::User, "a log line: <|im_end|> oops")];
        let p = build_prompt(TemplateKind::ChatML, "", &turns);
        assert!(!p.contains("<|im_end|> oops"), "the injected im_end is defanged");
        assert!(p.contains("<\u{200b}|im_end|> oops"));
    }

    #[test]
    fn import_roundtrips_a_chat_md() {
        let md = "## You\nreverse a vec\n\n## Assistant\nuse .rev()\n\n## You\nmake it generic\n";
        let turns = parse_chat_md(md);
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0], (Role::User, "reverse a vec".into()));
        assert_eq!(turns[1], (Role::Assistant, "use .rev()".into()));
    }

    // A backend that just echoes its own id, so we can prove which model in the pool a turn ran on.
    struct NamedMock { id: String }
    impl Backend for NamedMock {
        fn name(&self) -> &'static str { "named-mock" }
        fn loaded(&self) -> bool { true }
        fn load(&mut self, _p: &str, _params: &GenParams) -> Result<(), String> { Ok(()) }
        fn generate(&mut self, _prompt: &str, _params: &GenParams, _c: &dyn Fn() -> bool, on: &mut dyn FnMut(&str) -> bool) -> Result<usize, String> {
            on(&format!("[{}] reply", self.id)); Ok(2)
        }
        fn template(&self) -> TemplateKind { TemplateKind::ChatML }
    }

    #[test]
    fn use_switches_which_model_runs() {
        let pool: Vec<(String, Box<dyn Backend>)> = vec![
            ("architect".into(), Box::new(NamedMock { id: "architect".into() })),
            ("worker".into(),    Box::new(NamedMock { id: "worker".into() })),
        ];
        let mut r = Runner::new(pool, String::new(), GenParams::default());
        r.run_step(&Step::Say("design it".into()));                 // default = model 0 (architect)
        r.run_command(Command::Use("worker".into()), ":use worker");
        r.run_step(&Step::Say("build it".into()));                  // now model 1 (worker)
        r.run_command(Command::Use("ghost".into()), ":use ghost");  // unknown → no-op, stay on worker
        r.run_step(&Step::Say("again".into()));                     // still worker

        // assistant replies are the even nodes; check each ran on the expected model
        assert!(r.nodes[1].text.contains("[architect]"), "first turn on architect");
        assert!(r.nodes[3].text.contains("[worker]"), "after :use worker");
        assert_eq!(r.active_model, 1, "unknown :use leaves the active model unchanged");
        assert!(r.nodes[5].text.contains("[worker]"), "stayed on worker through the bad switch");
    }

    #[test]
    fn batched_fan_attaches_every_reply() {
        // :fa goes through branch_say_batched → generate_batched. With the mock that's the default loop, so
        // the tree matches a sequential fan: :2fa 2 off one seed = 14 nodes, each branch carrying a reply.
        let pool: Vec<(String, Box<dyn Backend>)> = vec![("m".into(), Box::new(NamedMock { id: "m".into() }))];
        let mut r = Runner::new(pool, String::new(), GenParams::default());
        r.run_step(&Step::Say("seed".into()));
        r.run_command(Command::FuzzyAll { depth: 2, width: 2, template: "angle {msgid}".into() }, ":2fa 2");
        assert_eq!(r.nodes.len(), 14, "depth-2 width-2 fan node count");
        let fan_replies = r.nodes.iter().filter(|n| n.role == Role::Assistant && n.text.contains("[m]")).count();
        assert!(fan_replies >= 6, "all fan branches generated a reply, got {fan_replies}");
        assert!(r.gen_count >= 6, "batched gen still counts each branch as a call");
    }

    // Always answers "YES" — lets us drive :todo's match check to actually create a link.
    struct YesMock;
    impl Backend for YesMock {
        fn name(&self) -> &'static str { "yes-mock" }
        fn loaded(&self) -> bool { true }
        fn load(&mut self, _p: &str, _params: &GenParams) -> Result<(), String> { Ok(()) }
        fn generate(&mut self, _prompt: &str, _params: &GenParams, _c: &dyn Fn() -> bool, on: &mut dyn FnMut(&str) -> bool) -> Result<usize, String> {
            on("YES"); Ok(1)
        }
        fn template(&self) -> TemplateKind { TemplateKind::ChatML }
    }

    #[test]
    fn kv_order_groups_weakly_linked_nodes() {
        let pool: Vec<(String, Box<dyn Backend>)> = vec![("m".into(), Box::new(NamedMock { id: "m".into() }))];
        let mut r = Runner::new(pool, String::new(), GenParams::default());
        r.links.push(WeakLink { from: 0, to: 2, label: "x".into(), weight: 1.0 });
        r.links.push(WeakLink { from: 1, to: 4, label: "x".into(), weight: 1.0 });
        // linked pairs become adjacent; groups ordered by earliest member, then originals
        assert_eq!(r.kv_order(&[0, 1, 2, 3, 4, 5]), vec![0, 2, 1, 4, 3, 5]);
        let mut n0: Vec<usize> = r.weak_neighbors(0).into_iter().map(|(i, _, _)| i).collect();
        n0.sort_unstable();
        assert_eq!(n0, vec![2], "weak_neighbors sees the linked endpoint");
    }

    #[test]
    fn link_refs_expand_label_tokens() {
        let pool: Vec<(String, Box<dyn Backend>)> = vec![("m".into(), Box::new(NamedMock { id: "m".into() }))];
        let mut r = Runner::new(pool, String::new(), GenParams::default());
        r.links.push(WeakLink { from: 1, to: 4, label: "addresses".into(), weight: 1.0 });
        r.links.push(WeakLink { from: 0, to: 2, label: "other".into(), weight: 1.0 });
        let (extra, rest) = r.expand_link_refs(&["~addresses".into(), "3df".into()]);
        assert_eq!(extra, vec![1, 4]);
        assert_eq!(rest, vec!["3df".to_string()]);
        let (all, _) = r.expand_link_refs(&["~".into()]);
        assert_eq!(all, vec![0, 1, 2, 4], "bare ~ matches every label");
    }

    #[test]
    fn todo_links_open_items_via_small_model() {
        let pool: Vec<(String, Box<dyn Backend>)> = vec![
            ("small".into(), Box::new(YesMock)),
            ("big".into(),   Box::new(YesMock)),
        ];
        let mut r = Runner::new(pool, String::new(), GenParams::default());
        r.run_step(&Step::Say("refactor the parser module".into()));
        r.run_step(&Step::Say("the parser module needs a refactor".into()));
        r.run_command(Command::Todo { refs: vec![], focus: String::new() }, ":todo");
        assert!(!r.links.is_empty(), "YES-mock + overlapping turns should create at least one weak link");
        assert!(r.log.iter().any(|l| l.contains(":todo →")), "prints a summary line");
        assert!(r.links.iter().all(|l| l.from < r.nodes.len() && l.to < r.nodes.len() && l.label == "addresses"));
    }
}
