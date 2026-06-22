//! simconvo.rs — graded conversation harness.
//!
//! Replays conversations (built-in, or loaded from an editable text file) through the resident model and
//! records, per conversation: working-context size, prefill cost (same resource-setup / gpu-sync split as
//! --bench), decode tok/s, stop reason, a cheap degeneration score (4-gram repeat ratio), and a short
//! output preview. Writes a Markdown report to paste back.
//!
//! The suite lives as data, not just source: `--simconvo-dump <file.txt>` writes it in an editable plain
//! format, and `--simconvo <gguf> <file.txt>` runs an edited file. Authored, not model-generated (a 135M
//! model writes the mush we're measuring); difficulty is graded by content + context length. Decoding is
//! GREEDY (argmax, no repeat penalty) on purpose — a penalty would hide the looping the degen score catches.
//!
//! Run:   kernel-validate --simconvo <gguf> [suite.txt] [out.md] [max_new=96]
//! Dump:  kernel-validate --simconvo-dump [suite.txt]

use gpu_kernels::{argmax, gguf, sample, tokenizer::Tokenizer, Gpu, SamplerParams};
use std::collections::HashSet;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq)]
enum R { U, A }

struct Convo { id: &'static str, diff: u8, stress: &'static str, turns: &'static [(R, &'static str)] }

/// Owned conversation (built-in or loaded from a file) — the unified form the harness runs.
struct OwnedConvo { id: String, diff: u8, stress: String, turns: Vec<(R, String)> }

// A realistic ~70-line Rust file reused by the harder convos to build genuine context (not filler).
const CHUNK: &str = r#"pub struct Grid { cells: Vec<u8>, w: usize, h: usize }
impl Grid {
    pub fn new(w: usize, h: usize) -> Self { Self { cells: vec![0; w * h], w, h } }
    pub fn get(&self, x: usize, y: usize) -> u8 { self.cells[y * self.w + x] }
    pub fn set(&mut self, x: usize, y: usize, v: u8) { self.cells[y * self.w + x] = v; }
    pub fn decorate(&mut self) {
        for y in 0..self.h {
            for x in 0..self.w {
                if self.get(x, y) == 1 { self.set(x, y, 2); }
            }
        }
    }
}
"#;

// Two larger, realistic modules so the high-ctx convos build genuine context (~700-900 tokens each),
// not repeated filler. They carry real bugs to find: build_mesh remeshes a whole chunk per edit, and
// neighbor_solid treats out-of-chunk as air (border seams).
const BIG: &str = r#"use std::collections::HashMap;
pub const CHUNK: usize = 32;
pub const VOL: usize = CHUNK * CHUNK * CHUNK;
#[derive(Clone)]
pub struct Chunk { pub blocks: Vec<u8>, pub coord: (i32, i32, i32), pub dirty: bool }
impl Chunk {
    pub fn new(coord: (i32, i32, i32)) -> Self { Self { blocks: vec![0; VOL], coord, dirty: true } }
    #[inline] fn idx(x: usize, y: usize, z: usize) -> usize { (y * CHUNK + z) * CHUNK + x }
    pub fn get(&self, x: usize, y: usize, z: usize) -> u8 { self.blocks[Self::idx(x, y, z)] }
    pub fn set(&mut self, x: usize, y: usize, z: usize, v: u8) {
        let i = Self::idx(x, y, z);
        if self.blocks[i] != v { self.blocks[i] = v; self.dirty = true; }
    }
    pub fn is_solid(&self, x: usize, y: usize, z: usize) -> bool { self.get(x, y, z) != 0 }
    pub fn solid_count(&self) -> usize { self.blocks.iter().filter(|&&b| b != 0).count() }
}
pub struct Generator { pub seed: u64 }
impl Generator {
    pub fn new(seed: u64) -> Self { Self { seed } }
    fn hash(&self, x: i32, z: i32) -> u64 {
        let mut h = self.seed ^ (x as u64).wrapping_mul(0x9E3779B97F4A7C15);
        h ^= (z as u64).wrapping_mul(0xC2B2AE3D27D4EB4F);
        h ^= h >> 29; h = h.wrapping_mul(0xBF58476D1CE4E5B9); h ^ (h >> 32)
    }
    pub fn height_at(&self, wx: i32, wz: i32) -> i32 {
        8 + (self.hash(wx >> 3, wz >> 3) % 12) as i32 + (self.hash(wx, wz) % 3) as i32
    }
    pub fn fill_chunk(&self, c: &mut Chunk) {
        let (cx, _cy, cz) = c.coord;
        for z in 0..CHUNK { for x in 0..CHUNK {
            let h = self.height_at(cx * CHUNK as i32 + x as i32, cz * CHUNK as i32 + z as i32);
            let hy = (h.max(0) as usize).min(CHUNK - 1);
            for y in 0..=hy { c.set(x, y, z, if y == hy { 1 } else { 2 }); }
        }}
        c.dirty = true;
    }
}
pub struct World { pub chunks: HashMap<(i32, i32, i32), Chunk>, pub gen: Generator, pub radius: i32 }
impl World {
    pub fn new(seed: u64, radius: i32) -> Self { Self { chunks: HashMap::new(), gen: Generator::new(seed), radius } }
    pub fn ensure(&mut self, coord: (i32, i32, i32)) {
        if !self.chunks.contains_key(&coord) {
            let mut c = Chunk::new(coord); self.gen.fill_chunk(&mut c); self.chunks.insert(coord, c);
        }
    }
    pub fn set_block(&mut self, coord: (i32, i32, i32), x: usize, y: usize, z: usize, v: u8) {
        self.ensure(coord);
        if let Some(c) = self.chunks.get_mut(&coord) { c.set(x, y, z, v); }
    }
    pub fn dirty_chunks(&self) -> Vec<(i32, i32, i32)> {
        self.chunks.values().filter(|c| c.dirty).map(|c| c.coord).collect()
    }
}
"#;

const BIG2: &str = r#"use crate::world::{Chunk, CHUNK};
#[derive(Clone, Copy)]
pub struct Vertex { pub pos: [f32; 3], pub normal: [f32; 3], pub ao: f32 }
#[derive(Default)]
pub struct Mesh { pub verts: Vec<Vertex>, pub indices: Vec<u32> }
impl Mesh {
    pub fn clear(&mut self) { self.verts.clear(); self.indices.clear(); }
    fn quad(&mut self, base: [f32; 3], du: [f32; 3], dv: [f32; 3], n: [f32; 3]) {
        let start = self.verts.len() as u32;
        let corner = |s: f32, t: f32| [base[0] + du[0]*s + dv[0]*t, base[1] + du[1]*s + dv[1]*t, base[2] + du[2]*s + dv[2]*t];
        for (s, t) in [(0.0,0.0),(1.0,0.0),(1.0,1.0),(0.0,1.0)] {
            self.verts.push(Vertex { pos: corner(s, t), normal: n, ao: 1.0 });
        }
        self.indices.extend_from_slice(&[start, start+1, start+2, start, start+2, start+3]);
    }
}
const FACES: [([i32; 3], [f32; 3]); 6] = [
    ([1,0,0],[1.0,0.0,0.0]), ([-1,0,0],[-1.0,0.0,0.0]), ([0,1,0],[0.0,1.0,0.0]),
    ([0,-1,0],[0.0,-1.0,0.0]), ([0,0,1],[0.0,0.0,1.0]), ([0,0,-1],[0.0,0.0,-1.0]),
];
fn neighbor_solid(c: &Chunk, x: i32, y: i32, z: i32) -> bool {
    if x < 0 || y < 0 || z < 0 || x >= CHUNK as i32 || y >= CHUNK as i32 || z >= CHUNK as i32 {
        return false; // out of chunk: treated as air — over-meshes chunk borders (seam bug)
    }
    c.is_solid(x as usize, y as usize, z as usize)
}
fn axes_for(d: [i32; 3]) -> ([f32; 3], [f32; 3]) {
    if d[0] != 0 { ([0.0,1.0,0.0],[0.0,0.0,1.0]) }
    else if d[1] != 0 { ([1.0,0.0,0.0],[0.0,0.0,1.0]) }
    else { ([1.0,0.0,0.0],[0.0,1.0,0.0]) }
}
pub fn build_mesh(c: &Chunk) -> Mesh {
    let mut m = Mesh::default();
    for y in 0..CHUNK { for z in 0..CHUNK { for x in 0..CHUNK {
        if !c.is_solid(x, y, z) { continue; }
        for (d, n) in FACES.iter() {
            let (nx, ny, nz) = (x as i32 + d[0], y as i32 + d[1], z as i32 + d[2]);
            if neighbor_solid(c, nx, ny, nz) { continue; }
            let base = [x as f32 + d[0].max(0) as f32, y as f32 + d[1].max(0) as f32, z as f32 + d[2].max(0) as f32];
            let (du, dv) = axes_for(*d);
            m.quad(base, du, dv, *n);
        }
    }}}
    m
}
"#;

// Non-code corpus: a song about childhood/parental conflict, towers & storms, triangulating an
// unknowable truth, and rejecting money for purpose. Tests whether a small model preserves MEANING,
// METAPHOR, and FIRST-PERSON voice under rewrite/continuation — none of which a substring rubric can
// fully capture, so the qual column here is a deliberately blunt "did the load-bearing anchors
// survive" proxy (the interesting question is how far the model drifts from them).
const LYRICS: &str = "talkin' 'bout the past — that's some risky business,\n\
I done dealt with things I never put in sentences,\n\
mother, father friction — I was in the middle,\n\
left me scarred forever though I can't remember";

static SUITE: &[Convo] = &[
    Convo { id: "fn-reverse", diff: 1, stress: "tiny code, short ctx",
        turns: &[(R::U, "Write a Rust function that reverses a string. Just the function.")] },
    Convo { id: "explain-result", diff: 1, stress: "short factual",
        turns: &[(R::U, "In one short paragraph, what is Rust's Result<T, E> for?")] },
    Convo { id: "struct-impl", diff: 2, stress: "small multi-part code",
        turns: &[(R::U, "Write a Rust struct Counter with a `new` and an `incr` method that returns the new value.")] },
    Convo { id: "fix-bug", diff: 2, stress: "spot+fix a bug",
        turns: &[(R::U, "This Rust panics on empty input. Fix it:\n\nfn first(v: &[i32]) -> i32 { v[0] }")] },
    Convo { id: "multi-turn", diff: 3, stress: "carry context across turns",
        turns: &[
            (R::U, "Define a Rust trait Shape with an `area(&self) -> f64` method."),
            (R::A, "trait Shape { fn area(&self) -> f64; }"),
            (R::U, "Now implement it for a Circle { r: f64 } using PI = 3.14159."),
            (R::A, "struct Circle { r: f64 } impl Shape for Circle { fn area(&self) -> f64 { 3.14159 * self.r * self.r } }"),
            (R::U, "Add a Rectangle { w: f64, h: f64 } impl too.")] },
    Convo { id: "reason-over-file", diff: 3, stress: "reason about an embedded file",
        turns: &[(R::U, "Here is a file:\n\n%FILE%\n\nThe decorate() pass is slow on big grids. In two sentences, why, and what would speed it up?")] },
    Convo { id: "refactor-2file", diff: 4, stress: "two files in context, cross-file change",
        turns: &[(R::U, "File a.rs:\n\n%FILE%\n\nFile b.rs:\n\npub fn run(g: &mut Grid) { g.decorate(); }\n\nMove decorate() out of Grid into b.rs as a free function `decorate(g: &mut Grid)`. Show both files after.")] },
    Convo { id: "continue-midfile", diff: 4, stress: "continue a half-written file (the case that broke)",
        turns: &[
            (R::U, "Write a Rust function `fn render(g: &Grid) -> Vec<u8>` that returns one byte per cell, value*16."),
            (R::A, "fn render(g: &Grid) -> Vec<u8> {\n    let mut out = Vec::with_capacity(g.w * g.h);\n    for y in 0..g.h {\n        for x in 0..g.w {\n            out.push("),
            (R::U, "continue")] },
    Convo { id: "long-synthesis", diff: 5, stress: "long-ish ctx, synthesis",
        turns: &[
            (R::U, "Read this module:\n\n%FILE%\n\nI also have a renderer that reads Grid::get in a hot loop, a loader that fills cells from disk, and a decorator that rewrites 1->2. Walk me through the data flow and name the one method that every part depends on."),
            (R::A, "The shared dependency is Grid::get / the cells buffer; loader writes cells, decorate rewrites them, renderer reads them via get."),
            (R::U, "Given that, if I made cells private and only exposed get/set, which of those three would break, and why?")] },
    Convo { id: "underspecified-long", diff: 5, stress: "ambiguous + long -> degeneration probe",
        turns: &[(R::U, "%FILE%\n\n%FILE%\n\nMake it better and faster and also fix the thing. Rewrite everything the right way.")] },
    Convo { id: "bigfile-1k", diff: 5, stress: "~1k ctx, find the coarse remesh",
        turns: &[(R::U, "Module world.rs:\n\n%BIG%\n\nA single block edit calls World::set_block, and the renderer remeshes any chunk whose dirty flag is set by rebuilding it from scratch. In two sentences: which method makes a single-block edit reprocess all 32768 cells, and what granularity would fix it?")] },
    Convo { id: "bigfile-2k", diff: 5, stress: "~1.8k ctx, two files, cross-file trace",
        turns: &[
            (R::U, "world.rs:\n\n%BIG%\n\nmesh.rs:\n\n%BIG2%\n\nTrace what happens to the mesh when I call world.set_block(c, 5, 5, 5, 0) to carve a block. Which chunks must remesh, and does the current code remesh the correct set?"),
            (R::A, "set_block marks that chunk dirty and build_mesh rebuilds it fully. Neighboring chunks are not re-marked, so a block on a chunk border leaves a seam, because neighbor_solid treats out-of-chunk cells as air."),
            (R::U, "Right. What's the minimal change to neighbor_solid and the dirty-marking to kill the seam without remeshing the whole world?")] },
    Convo { id: "bigfile-3k", diff: 5, stress: "~2.4k ctx, three files, design under load",
        turns: &[
            (R::U, "Three files.\n\nworld.rs:\n\n%BIG%\n\nmesh.rs:\n\n%BIG2%\n\nlegacy grid.rs:\n\n%FILE%\n\nWe want per-edit incremental remeshing: only the touched chunk plus any neighbor across the shared face. Design the dirty-tracking and the cross-chunk neighbor lookup mesh.rs needs."),
            (R::A, "Mark the edited chunk dirty, and if the edit is on a face boundary (coord 0 or CHUNK-1) also mark the adjacent chunk. neighbor_solid must accept a World reference to read across chunk borders instead of returning air."),
            (R::U, "Good. Now name every existing method that changes signature or body, and what the grid.rs legacy decorate() pass should become in this design.")] },

    // ── non-code: lyric / narrative reasoning (preserve meaning, metaphor, voice) ──
    Convo { id: "lyric-narrative-repair", diff: 3, stress: "preserve meaning while improving flow",
        turns: &[(R::U, "Improve the flow and rhyme scheme of this verse without changing the meaning:\n\n%LYRICS%")] },
    Convo { id: "metaphor-consistency", diff: 4, stress: "preserve four metaphors across a revision",
        turns: &[(R::U, "The verse uses these recurring metaphors:\n- tower = self / perspective\n- storm = conflict / hardship\n- buried alive truth = suppressed reality\n- triangulation = reconstructing truth from conflicting accounts\n\nRevise this stanza while preserving all four metaphors:\n\n%LYRICS%")] },
    Convo { id: "narrative-continuation", diff: 4, stress: "continue an autobiographical story",
        turns: &[(R::U, "Continue this narrative:\n\nhad to lose a lot just to find where I'm at,\nturned the pain to gains, had to make my map,\nalways three sides to the story, needed the third point to triangulate where I was at\n\nThe next section should explain: parents divorced when the narrator was young; both had conflicting versions of events; the truth was never fully known; the narrator had to move forward anyway.")] },
    Convo { id: "missing-information-reasoning", diff: 4, stress: "infer what an absent symbol means",
        turns: &[(R::U, "Analyze this lyric:\n\nalways three sides to the story, needed the third point to triangulate where I was at\n\nWhat does the missing third point symbolize?")] },
    Convo { id: "motif-unification", diff: 4, stress: "find sonic/thematic unity in line endings",
        turns: &[(R::U, "These lines end separate sections:\n\nleft me scarred forever though I can't remember\ntower crumbled in the storm I had to weather\nain't no Plan B when you're the only messenger\nbeen baptized in music, grew up under Blue Sky\n\nSuggest ways to create sonic or thematic unity between these endings without changing their meaning.")] },
    Convo { id: "rhyme-vs-meaning", diff: 5, stress: "choose semantics over a clean rhyme",
        turns: &[(R::U, "The line\n\nmother, father friction — I was in the distance\n\nrhymes well but is factually wrong: the narrator was a powerless child trapped in the middle of parental conflict. Generate alternatives that preserve the meaning even if the rhyme becomes weaker.")] },
    Convo { id: "autobiography-to-purpose", diff: 5, stress: "bridge past to future, justify a value shift",
        turns: &[(R::U, "Bridge these sections.\n\nPAST: parental conflict; missing truth; fragmented memory.\n\nFUTURE:\nthey was talkin' numbers, I was talkin' purpose,\nI don't chase the surface, I just scratch beneath it\n\nThe transition should explain why the narrator rejected a purely financial definition of success.")] },
    Convo { id: "thematic-cohesion", diff: 5, stress: "whole-song symbol evolution",
        turns: &[(R::U, "Analyze the recurring symbols: tower, storm, lightning, buried alive truth, Blue Sky, TripLit. Explain how each evolves from the beginning of the song to the ending.")] },
    Convo { id: "lyric-editing-constraints", diff: 5, stress: "constrained rewrite (hard multi-constraint)",
        turns: &[(R::U, "Rewrite the following verse. Requirements: preserve meaning; preserve first-person perspective; keep references to tower, storm, and truth; maintain autobiographical tone; improve internal rhyme density; do NOT introduce alcohol references; do NOT remove references to family conflict.\n\nVerse:\n\n%LYRICS%")] },
    Convo { id: "contradiction-detection", diff: 5, stress: "spot contradictions across revisions",
        turns: &[(R::U, "Examine these revisions of the same lyric and identify contradictions introduced by later edits:\n\nVersion A: mother, father friction — I was in the distance\nVersion B: mother, father friction — I was in the middle\nVersion C: mother father fightin' — baby me the witness\n\nExplain which version best matches a narrator describing childhood exposure to parental conflict.")] },
    Convo { id: "longform-song-arc", diff: 5, stress: "structure reasoning over a 4-act outline",
        turns: &[(R::U, "Given this outline — Act 1: tower, ambition, pressure. Act 2: divorce, conflicting stories, buried truth, missing third point. Act 3: dropping out, rejecting empty success, pursuing purpose. Act 4: music, healing, legacy. Explain how the emotional arc progresses and identify the central thesis of the song.")] },
];

fn builtin() -> Vec<OwnedConvo> {
    SUITE.iter().map(|c| OwnedConvo {
        id: c.id.to_string(), diff: c.diff, stress: c.stress.to_string(),
        turns: c.turns.iter().map(|(r, t)| (*r, t.to_string())).collect(),
    }).collect()
}

fn expand(t: &str) -> String { t.replace("%BIG2%", BIG2).replace("%BIG%", BIG).replace("%FILE%", CHUNK).replace("%LYRICS%", LYRICS) }
fn role(r: R) -> &'static str { match r { R::U => "user", R::A => "assistant" } }

/// Detect the chat-template family from the tokenizer's special tokens and format accordingly. Also the
/// fix for the handoff leak: each model gets its OWN template, not another model's markers.
fn build_prompt(tok: &Tokenizer, turns: &[(R, String)]) -> (String, &'static str) {
    if tok.special_id("<|start_header_id|>").is_some() {
        let mut s = String::from("<|begin_of_text|>");
        for (r, t) in turns { s.push_str(&format!("<|start_header_id|>{}<|end_header_id|>\n\n{}<|eot_id|>", role(*r), expand(t))); }
        s.push_str("<|start_header_id|>assistant<|end_header_id|>\n\n");
        (s, "llama3")
    } else if tok.special_id("<|im_start|>").is_some() {
        let mut s = String::new();
        for (r, t) in turns { s.push_str(&format!("<|im_start|>{}\n{}<|im_end|>\n", role(*r), expand(t))); }
        s.push_str("<|im_start|>assistant\n");
        (s, "chatml")
    } else {
        let mut s = String::new();
        for (r, t) in turns { s.push_str(&format!("{}: {}\n", if *r == R::U { "User" } else { "Assistant" }, expand(t))); }
        s.push_str("Assistant:");
        (s, "plain")
    }
}

// ── editable plain-text suite format ────────────────────────────────────────────────────────────
// '# id | diff | stress', then U:/A: turns (multi-line until the next marker). %FILE% expands at run time.
fn flush(cur: &mut Option<OwnedConvo>, turn: &mut Option<(R, String)>) {
    if let (Some(c), Some((r, mut s))) = (cur.as_mut(), turn.take()) {
        while s.ends_with('\n') { s.pop(); }
        c.turns.push((r, s));
    }
}
fn parse_suite(text: &str) -> Vec<OwnedConvo> {
    let mut out = Vec::new();
    let mut cur: Option<OwnedConvo> = None;
    let mut turn: Option<(R, String)> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            flush(&mut cur, &mut turn);
            if let Some(c) = cur.take() { if !c.turns.is_empty() { out.push(c); } }
            let p: Vec<&str> = rest.splitn(3, '|').map(|x| x.trim()).collect();
            cur = Some(OwnedConvo {
                id: p.get(0).copied().unwrap_or("convo").to_string(),
                diff: p.get(1).and_then(|x| x.parse().ok()).unwrap_or(3),
                stress: p.get(2).copied().unwrap_or("").to_string(),
                turns: Vec::new(),
            });
        } else if let Some(rest) = line.strip_prefix("U:") {
            flush(&mut cur, &mut turn);
            turn = Some((R::U, rest.strip_prefix(' ').unwrap_or(rest).to_string()));
        } else if let Some(rest) = line.strip_prefix("A:") {
            flush(&mut cur, &mut turn);
            turn = Some((R::A, rest.strip_prefix(' ').unwrap_or(rest).to_string()));
        } else if let Some((_, s)) = turn.as_mut() {
            s.push('\n'); s.push_str(line);
        }
    }
    flush(&mut cur, &mut turn);
    if let Some(c) = cur.take() { if !c.turns.is_empty() { out.push(c); } }
    out
}
fn dump_suite(convos: &[OwnedConvo]) -> String {
    let mut s = String::new();
    s.push_str("# simconvo suite — edit, then run:  kernel-validate --simconvo <gguf> this_file.txt\n");
    s.push_str("# format: '# id | diff | stress', then U:/A: turns (multi-line until next marker). %FILE% expands to a sample Rust file.\n\n");
    for c in convos {
        s.push_str(&format!("# {} | {} | {}\n", c.id, c.diff, c.stress));
        for (r, t) in &c.turns {
            let tag = if *r == R::U { "U:" } else { "A:" };
            let mut lines = t.split('\n');
            s.push_str(&format!("{} {}\n", tag, lines.next().unwrap_or("")));
            for l in lines { s.push_str(l); s.push('\n'); }
        }
        s.push('\n');
    }
    s
}
pub fn dump_to(path: &str) {
    match std::fs::write(path, dump_suite(&builtin())) {
        Ok(_) => eprintln!("simconvo: wrote suite -> {path}\n  edit it, then: kernel-validate --simconvo <gguf> {path}"),
        Err(e) => eprintln!("simconvo: write {path}: {e}"),
    }
}

fn ngram_repeat(toks: &[u32], n: usize) -> f64 {
    if toks.len() < n + 1 { return 0.0; }
    let mut seen = HashSet::new();
    let (mut rep, mut tot) = (0u32, 0u32);
    for w in toks.windows(n) { tot += 1; if !seen.insert(w.to_vec()) { rep += 1; } }
    if tot == 0 { 0.0 } else { rep as f64 / tot as f64 }
}

// Quality signal ("how well does it do"), judge-free: each convo with a known answer carries a
// rubric of the identifiers/facts a correct final answer must name. An entry with `|` is a set of
// alternatives worth one point (e.g. the fix-bug answer can say Option OR get OR bounds). This is
// the deterministic foothold the metrics page is built on — same rubric scores a human transcript,
// so people and models land on one quality axis. Hand-rolled now; a learned embedding only earns
// its keep once these labels exist and show structure.
fn rubric_for(id: &str) -> &'static [&'static str] {
    match id {
        "fn-reverse"       => &["rev"],
        "struct-impl"      => &["impl", "incr|increment"],
        "fix-bug"          => &["option|get|is_empty|empty|bounds|panic|none"],
        "reason-over-file" => &["index|get"],
        "bigfile-1k"       => &["build_mesh", "whole|all|entire|32768|rebuild|coarse"],
        "bigfile-2k"       => &["neighbor_solid", "border|seam|air|adjacent|neighbor"],
        "bigfile-3k"       => &["neighbor_solid", "dirty", "border|neighbor|adjacent|world"],
        // non-code: blunt "load-bearing anchors survived" proxies (creative tasks resist substring
        // scoring — that gap IS the measurement: how far the model drifts from the meaning/metaphors).
        "lyric-narrative-repair"        => &["mother|father|parent", "scar|trauma|hurt|pain", "memory|remember|forget|recall"],
        "metaphor-consistency"          => &["tower", "storm", "buried|truth|suppress", "triangulat|third point|reconstruct"],
        "narrative-continuation"        => &["divorc|separat|split", "conflict|version|differ|both sides", "truth", "forward|move on|anyway|kept going"],
        "missing-information-reasoning" => &["truth", "unavailable|missing|unknown|never known|cannot|can't", "reconstruct|verify|independent|reference|third"],
        "motif-unification"             => &["rhyme|sonic|assonance|slant|sound|cadence", "theme|motif|imagery|symbol", "weather|messenger|remember|-er"],
        "rhyme-vs-meaning"              => &["middle|trapped|caught|between|powerless|stuck", "child|young|kid|helpless|powerless"],
        "autobiography-to-purpose"      => &["purpose", "money|number|financial|wealth|rich|material", "deeper|beneath|beyond|meaning|not money|surface"],
        "thematic-cohesion"             => &["tower", "storm", "lightning", "truth", "sky", "evolv|change|transform|progress|shift|grow"],
        "lyric-editing-constraints"     => &["tower", "storm", "truth", "mother|father|family|parent"],
        "contradiction-detection"       => &["middle|witness|caught", "distance", "contradict|inconsisten|differ|conflict|mismatch|best match"],
        "longform-song-arc"             => &["tower|ambition|pressure", "divorce|truth|missing", "purpose|reject|dropping|empty", "music|healing|legacy", "thesis|arc|central|theme"],
        _ => &[],
    }
}

/// (hits, total, "✓term ✗term" detail). Case-insensitive substring; `|` = one point if any alt hits.
fn score_rubric(rubric: &[&str], text: &str) -> (usize, usize, String) {
    if rubric.is_empty() { return (0, 0, String::new()); }
    let lc = text.to_lowercase();
    let mut hits = 0;
    let mut detail = String::new();
    for &entry in rubric {
        let ok = entry.split('|').any(|alt| lc.contains(&alt.to_lowercase()));
        if ok { hits += 1; }
        detail.push_str(if ok { "✓" } else { "✗" });
        detail.push_str(entry.split('|').next().unwrap_or(entry));
        detail.push(' ');
    }
    (hits, rubric.len(), detail.trim_end().to_string())
}

/// Determinism + alternative-matching self-check (no model/gguf needed): `--gate-rubric`.
pub fn gate_rubric() {
    let cases: &[(&str, &str, usize, usize)] = &[
        // (convo id, model answer text, expected hits, expected total)
        ("bigfile-1k", "The method that reprocesses all 32768 cells is set_chunk.", 1, 2), // wrong method, right symptom
        ("bigfile-1k", "build_mesh rebuilds the whole chunk on every edit.", 2, 2),        // fully correct
        ("bigfile-2k", "neighbor_solid treats out-of-chunk cells as air, leaving a border seam.", 2, 2),
        ("bigfile-2k", "You should remesh the chunk again.", 0, 2),                        // misses both
        ("fix-bug", "first should return Option and handle the empty slice.", 1, 1),
        ("fix-bug", "return v[0]", 0, 1),
        ("fn-reverse", "use s.chars().rev().collect()", 1, 1),
        // non-code rubrics: all four metaphors present vs none; meaning-over-rhyme anchor present.
        ("metaphor-consistency", "the tower still stands through the storm; the buried truth needs triangulation to reconstruct", 4, 4),
        ("metaphor-consistency", "here is a nicer sounding poem about feelings", 0, 4),
        ("rhyme-vs-meaning", "a powerless child caught in the middle of it all", 2, 2),
    ];
    let mut ok = true;
    for &(id, text, eh, et) in cases {
        let (h, t, d) = score_rubric(rubric_for(id), text);
        let pass = h == eh && t == et;
        ok &= pass;
        println!("  [{}] {h}/{t} (want {eh}/{et}) {} | {d}", id, if pass { "✓" } else { "✗ FAIL" });
    }
    println!("gate-rubric: {}", if ok { "all cases pass ✓ — scorer deterministic & alt-matching" } else { "FAILURES ✗" });
}

struct Row {
    id: String, diff: u8, stress: String,
    ctx: usize, prefill_ms: f64, pf_sync: f64, pf_setup: f64,
    out: usize, toks_s: f64, stop: &'static str, degen: f64, preview: String,
    qual: Option<(usize, usize)>, qdetail: String,
}

pub fn run(args: &[String], idx: usize) {
    let path = match args.get(idx + 1) { Some(p) => p.clone(), None => { eprintln!("--simconvo <gguf> [suite.txt] [out.md] [max_new=96]"); return; } };
    let mut out_md = format!("{path}.simconvo.md");
    let mut suite_file: Option<String> = None;
    let mut max_new: usize = 96;
    for a in &args[(idx + 2).min(args.len())..] {
        if let Ok(n) = a.parse::<usize>() { max_new = n; }
        else if a.ends_with(".md") { out_md = a.clone(); }
        else { suite_file = Some(a.clone()); }
    }

    let _mm = match gpu_kernels::mmap_ro(&path) { Ok(m) => m, Err(e) => { eprintln!("mmap {path}: {e}"); return; } }; let bytes = &_mm[..];
    let g = match gguf::parse(&bytes) { Ok(g) => g, Err(e) => { eprintln!("parse gguf: {e}"); return; } };
    let tok = match Tokenizer::from_gguf(&g) { Ok(t) => t, Err(e) => { eprintln!("tokenizer: {e}"); return; } };
    let gpu = Gpu::new();
    eprintln!("simconvo: adapter = {}", gpu.adapter_name);
    let model = match gpu.load_gguf_quantized(&bytes) { Ok(m) => m, Err(e) => { eprintln!("load: {e}"); return; } };
    gpu.prewarm(&model);
    let eos = g.get_u32("tokenizer.ggml.eos_token_id")
        .or_else(|| tok.special_id("<|im_end|>"))
        .or_else(|| tok.special_id("<|eot_id|>"));

    let convos = match &suite_file {
        Some(f) => match std::fs::read_to_string(f) {
            Ok(t) => { let v = parse_suite(&t); eprintln!("simconvo: loaded {} convos from {f}", v.len()); v }
            Err(e) => { eprintln!("read {f}: {e}; using built-in"); builtin() }
        },
        None => builtin(),
    };
    eprintln!("simconvo: d={} layers={} heads={}/{} ff={} vocab={} | eos={eos:?} | running {} conversations…",
        model.d, model.n_layers, model.n_heads, model.n_kv_heads, model.h, model.vocab, convos.len());

    let mut rows: Vec<Row> = Vec::new();
    let mut tmpl = "?";
    for c in &convos {
        let (prompt, kind) = build_prompt(&tok, &c.turns);
        tmpl = kind;
        let ids = tok.encode(&prompt);
        let ctx = ids.len();
        let cache = gpu.new_cache(model.n_layers, ctx + max_new + 16, model.kv_dim());

        gpu_kernels::gpu_counters_reset();
        let t0 = Instant::now();
        // Memory-bounded prefill (chunk=512): transients peak at O(512·ff) instead of O(T·ff), so
        // large models past ~1k ctx stop OOMing (DeepSeek-8B crashed at 2167 on prefill_batched).
        // Gated bit-exact vs prefill_batched for T<=512 (every original convo) and argmax-identical
        // above (see --gate-prefill). Same wall cost; bounded peak memory.
        let logits = gpu.prefill_chunked(&model, &cache, &ids, 512);
        let prefill_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let pf_sync = gpu_kernels::gpu_sync_ns() as f64 / 1e6;
        let pf_setup = gpu_kernels::gpu_setup_ns() as f64 / 1e6;

        let mut out: Vec<u32> = Vec::new();
        let mut next = argmax(&logits);
        let mut pos = ctx;
        let mut stop = "cap";
        let t1 = Instant::now();
        while out.len() < max_new {
            if Some(next) == eos { stop = "eos"; break; }
            out.push(next);
            let lg = gpu.decode_step(&model, &cache, next, pos);
            pos += 1;
            next = argmax(&lg);
        }
        let toks_s = out.len() as f64 / t1.elapsed().as_secs_f64().max(1e-6);
        let degen = ngram_repeat(&out, 4);
        let text = tok.decode(&out);
        let (qh, qt, qdetail) = score_rubric(rubric_for(&c.id), &text);
        let qual = if qt > 0 { Some((qh, qt)) } else { None };
        let preview: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect();

        eprintln!("  [{}] diff{} ctx={ctx} prefill={prefill_ms:.0}ms decode={toks_s:.1}t/s out={} stop={stop} degen={:.0}%{}",
            c.id, c.diff, out.len(), degen * 100.0,
            qual.map(|(h, t)| format!(" qual={h}/{t}")).unwrap_or_default());
        rows.push(Row { id: c.id.clone(), diff: c.diff, stress: c.stress.clone(), ctx, prefill_ms, pf_sync, pf_setup,
            out: out.len(), toks_s, stop, degen, preview, qual, qdetail });
    }

    let mut md = String::new();
    md.push_str("# simconvo report\n\n");
    md.push_str(&format!("model: `{}`\n\n", path));
    md.push_str(&format!("adapter: {} · d={} layers={} heads={}/{} ff={} vocab={} · template={} · eos={:?} · max_new={}\n\n",
        gpu.adapter_name, model.d, model.n_layers, model.n_heads, model.n_kv_heads, model.h, model.vocab, tmpl, eos, max_new));
    md.push_str("| id | diff | stress | ctx tok | prefill ms | pf gpu-sync | pf setup | decode tok/s | out tok | stop | degen % | qual |\n");
    md.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for r in &rows {
        md.push_str(&format!("| {} | {} | {} | {} | {:.0} | {:.0} | {:.0} | {:.1} | {} | {} | {:.0} | {} |\n",
            r.id, r.diff, r.stress, r.ctx, r.prefill_ms, r.pf_sync, r.pf_setup, r.toks_s, r.out, r.stop, r.degen * 100.0,
            r.qual.map(|(h, t)| format!("{h}/{t}")).unwrap_or_else(|| "—".into())));
    }
    md.push_str("\n## output previews (greedy; degen% = repeated 4-grams; qual = rubric facts hit)\n\n");
    for r in &rows {
        md.push_str(&format!("**{}** (diff {}, ctx {}): {}\n\n", r.id, r.diff, r.ctx,
            if r.preview.is_empty() { "<empty>" } else { &r.preview }));
        if let Some((h, t)) = r.qual {
            md.push_str(&format!("&nbsp;&nbsp;rubric {h}/{t}: {}\n\n", r.qdetail));
        }
    }
    md.push_str("\n_Read: prefill ms vs ctx tok shows the cliff; pf gpu-sync ≈ prefill ms ⇒ GPU-bound prefill. ");
    md.push_str("decode tok/s falling with ctx is the attention-over-KV slope. degen% climbing with diff/ctx is quality breakdown. ");
    md.push_str("qual = rubric facts a correct answer must name (judge-free); the 'how well does it do' axis, comparable across models and human transcripts._\n");

    match std::fs::write(&out_md, &md) {
        Ok(_) => eprintln!("simconvo: wrote report -> {out_md}\n  (open it, or paste it back for diagnostics)"),
        Err(e) => eprintln!("simconvo: write {out_md}: {e}"),
    }
}

// ───────────────────────── branch/K experiment (--simbranch) ─────────────────────────
// Thesis under test (the "super-keyboard"): forking K continuations at a decision point and
// reconciling across them (a) beats greedy on the turns that break, and (b) divergence PREDICTS
// where greedy fails — the place a human interjection has the most leverage. We measure it on the
// rubric axis: each branch's hit-vector over the convo's known facts. fact-divergence is the
// branch disagreement; best-of-K / consensus are the two reconciliation operators; the summary
// correlates divergence with greedy's misses.

fn fact_hits(rubric: &[&str], text: &str) -> Vec<bool> {
    let lc = text.to_lowercase();
    rubric.iter().map(|e| e.split('|').any(|a| lc.contains(&a.to_lowercase()))).collect()
}
fn count(v: &[bool]) -> usize { v.iter().filter(|&&b| b).count() }
// Gini-style branch disagreement per fact, averaged: 0 = all branches agree, 0.5 = maximal split.
fn fact_divergence(hv: &[Vec<bool>], n: usize) -> f64 {
    if hv.is_empty() || n == 0 { return 0.0; }
    let k = hv.len() as f64;
    (0..n).map(|f| { let p = hv.iter().filter(|v| v[f]).count() as f64 / k; 2.0 * p * (1.0 - p) }).sum::<f64>() / n as f64
}
fn trigrams(t: &str) -> HashSet<String> {
    let w: Vec<&str> = t.split_whitespace().collect();
    w.windows(3).map(|x| x.join(" ")).collect()
}
fn text_divergence(texts: &[String]) -> f64 {
    let g: Vec<HashSet<String>> = texts.iter().map(|t| trigrams(t)).collect();
    let (mut tot, mut n) = (0.0, 0);
    for i in 0..g.len() { for j in (i + 1)..g.len() {
        let inter = g[i].intersection(&g[j]).count() as f64;
        let uni = g[i].union(&g[j]).count().max(1) as f64;
        tot += 1.0 - inter / uni; n += 1;
    }}
    if n == 0 { 0.0 } else { tot / n as f64 }
}
fn best_of(hv: &[Vec<bool>]) -> usize { hv.iter().map(|v| count(v)).max().unwrap_or(0) }
fn consensus(hv: &[Vec<bool>], n: usize) -> usize {
    let k = hv.len();
    if k == 0 { return 0; }
    (0..n).filter(|&f| hv.iter().filter(|v| v[f]).count() * 2 > k).count()
}

/// Self-test of the experiment's math (no GPU/model): `--gate-branch`.
pub fn gate_branch() {
    let hv = vec![vec![true, true], vec![true, false], vec![true, false]]; // 3 branches, 2 facts
    let fd = fact_divergence(&hv, 2);          // fact0 agree (0), fact1 split 1/3 -> 4/9; mean 2/9
    let bo = best_of(&hv);                      // branch0 hits both -> 2
    let cs = consensus(&hv, 2);                // fact0 3/3 hit, fact1 1/3 -> 1
    let td = text_divergence(&["a b c d".into(), "a b c d".into(), "x y z w".into()]); // (0+1+1)/3
    let ok = (fd - 2.0 / 9.0).abs() < 1e-9 && bo == 2 && cs == 1 && (td - 2.0 / 3.0).abs() < 1e-9;
    println!("  fact_div={fd:.4} (want .2222)  best_of={bo} (2)  consensus={cs} (1)  text_div={td:.4} (want .6667)");
    println!("gate-branch: {}", if ok { "math checks ✓" } else { "FAIL ✗" });
}

pub fn run_branch(args: &[String], idx: usize) {
    let path = match args.get(idx + 1) { Some(p) => p.clone(), None => { eprintln!("--simbranch <gguf> [suite.txt] [K=5] [temp=0.8] [max_new=64]"); return; } };
    let mut suite_file: Option<String> = None;
    let (mut k_branches, mut temp, mut max_new) = (5usize, 0.8f32, 64usize);
    let mut seen_num = 0;
    for a in &args[(idx + 2).min(args.len())..] {
        if let Ok(f) = a.parse::<f32>() { if a.contains('.') { temp = f; continue; } }
        if let Ok(n) = a.parse::<usize>() { match seen_num { 0 => k_branches = n, _ => max_new = n }; seen_num += 1; }
        else if !a.ends_with(".md") { suite_file = Some(a.clone()); }
    }
    let _mm = match gpu_kernels::mmap_ro(&path) { Ok(m) => m, Err(e) => { eprintln!("mmap {path}: {e}"); return; } }; let bytes = &_mm[..];
    let g = match gguf::parse(&bytes) { Ok(g) => g, Err(e) => { eprintln!("parse: {e}"); return; } };
    let tok = match Tokenizer::from_gguf(&g) { Ok(t) => t, Err(e) => { eprintln!("tok: {e}"); return; } };
    let gpu = Gpu::new();
    eprintln!("simbranch: adapter = {}", gpu.adapter_name);
    let model = match gpu.load_gguf_quantized(&bytes) { Ok(m) => m, Err(e) => { eprintln!("load: {e}"); return; } };
    gpu.prewarm(&model);
    let eos = g.get_u32("tokenizer.ggml.eos_token_id").or_else(|| tok.special_id("<|im_end|>")).or_else(|| tok.special_id("<|eot_id|>"));
    let convos = match &suite_file { Some(f) => std::fs::read_to_string(f).map(|t| parse_suite(&t)).unwrap_or_else(|_| builtin()), None => builtin() };
    let sp = SamplerParams { temperature: temp, top_k: 40, top_p: 0.95, repeat_penalty: 1.0, repeat_last_n: 0, min_p: 0.05 };
    eprintln!("simbranch: K={k_branches} temp={temp} max_new={max_new} | scoring convos with a rubric…");

    // decode one continuation from the fork; greedy if rng is None
    let decode = |ids: &[u32], rng: &mut Option<u64>| -> Vec<u32> {
        let cache = gpu.new_cache(model.n_layers, ids.len() + max_new + 16, model.kv_dim());
        let logits = gpu.prefill_chunked(&model, &cache, ids, 512);
        let pick = |lg: &[f32], r: &mut Option<u64>| match r { Some(s) => sample(lg, &sp, s), None => argmax(lg) };
        let mut out = Vec::new();
        let mut next = pick(&logits, rng);
        let mut pos = ids.len();
        while out.len() < max_new {
            if Some(next) == eos { break; }
            out.push(next);
            let lg = gpu.decode_step(&model, &cache, next, pos); pos += 1;
            next = pick(&lg, rng);
        }
        out
    };

    let mut rows: Vec<(String, u8, usize, usize, usize, usize, f64, f64, f64)> = Vec::new(); // id,diff,facts,g,best,cons,fdiv,tdiv,gdegen
    for c in &convos {
        let rub = rubric_for(&c.id);
        if rub.is_empty() { continue; } // divergence needs known facts
        let (prompt, _) = build_prompt(&tok, &c.turns);
        let ids = tok.encode(&prompt);
        let g_out = decode(&ids, &mut None);
        let g_text = tok.decode(&g_out);
        let g_hits = count(&fact_hits(rub, &g_text));
        let g_degen = ngram_repeat(&g_out, 4);
        let mut hv = Vec::new(); let mut texts = Vec::new();
        for b in 0..k_branches {
            let mut rng = Some((b as u64).wrapping_mul(0x9E3779B97F4A7C15) | 1);
            let o = decode(&ids, &mut rng);
            let t = tok.decode(&o);
            hv.push(fact_hits(rub, &t)); texts.push(t);
        }
        let (fdiv, tdiv) = (fact_divergence(&hv, rub.len()), text_divergence(&texts));
        let (best, cons) = (best_of(&hv).max(g_hits), consensus(&hv, rub.len()));
        eprintln!("  [{}] facts={} greedy={g_hits} bestK={best} consensus={cons} fact_div={fdiv:.2} text_div={tdiv:.2} g_degen={:.0}%",
            c.id, rub.len(), g_degen * 100.0);
        rows.push((c.id.clone(), c.diff, rub.len(), g_hits, best, cons, fdiv, tdiv, g_degen));
    }

    // correlation read: is divergence higher where greedy missed facts?
    let (mut miss_d, mut miss_n, mut hit_d, mut hit_n) = (0.0, 0, 0.0, 0);
    for r in &rows { if r.3 < r.2 { miss_d += r.6; miss_n += 1; } else { hit_d += r.6; hit_n += 1; } }
    let mean = |s: f64, n: usize| if n == 0 { f64::NAN } else { s / n as f64 };
    let lift: usize = rows.iter().map(|r| r.4.saturating_sub(r.3)).sum();

    let mut md = String::from("# simbranch report — fork K, reconcile, correlate\n\n");
    md.push_str(&format!("model: `{path}`\n\nK={k_branches} · temp={temp} · max_new={max_new} · facts-rubric axis\n\n"));
    md.push_str("| id | diff | facts | greedy | best-of-K | consensus | fact_div | text_div | greedy degen % |\n|---|---|---|---|---|---|---|---|---|\n");
    for r in &rows {
        md.push_str(&format!("| {} | {} | {} | {} | {} | {} | {:.2} | {:.2} | {:.0} |\n", r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8 * 100.0));
    }
    md.push_str(&format!("\n**Thesis A (quality):** best-of-K added **{lift}** rubric facts over greedy across the suite.\n"));
    md.push_str(&format!("**Thesis B (divergence predicts failure):** mean fact_div where greedy MISSED a fact = **{:.3}** vs where greedy got them all = **{:.3}**. ",
        mean(miss_d, miss_n), mean(hit_d, hit_n)));
    md.push_str("If the miss-side is higher, divergence flags exactly the turns worth forking / steering.\n");
    let out_md = format!("{path}.simbranch.md");
    let _ = std::fs::write(&out_md, &md);
    eprintln!("simbranch: best-of-K lift = +{lift} facts | div(miss)={:.3} vs div(hit)={:.3}\nsimbranch: wrote {out_md}",
        mean(miss_d, miss_n), mean(hit_d, hit_n));
}
