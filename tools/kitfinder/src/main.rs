//! kitfinder — find the next shared library ("kit") by mining structural clones.
//!
//! This is the token-stream realization of the clone-mining design (Algorithm 1):
//! normalize every function into a rename-insensitive token sequence, then find
//! the code that repeats across crates. It is deliberately NOT a full AST/PDG
//! engine — a lightweight Rust lexer plus a semi-blind normalization gets most of
//! the Type-1/Type-2 (and, via MinHash, approximate Type-3) signal at a fraction
//! of the cost, and runs offline with zero dependencies.
//!
//! Pipeline:
//!   walk .rs files → lex → extract `fn` bodies → normalize (blind LOCALS, keep
//!   control-flow + called APIs + types + fields) → intern tokens →
//!     (a) exact clustering on the normalized sequence (Type-2 clones),
//!     (b) MinHash + LSH near-duplicate clustering (approximate Type-3),
//!     (c) cross-crate aggregation: which crate pairs share the most clone mass →
//!         the next kit.
//!
//! Normalization keeps an identifier's NAME when it is a type/variant/const
//! (uppercase), a call/macro/path segment, or a field/method — so two functions
//! are "the same" only if they have the same structure AND call the same APIs on
//! the same types, with locals possibly renamed and literals possibly differing.
//! That is the "this is duplicated logic worth extracting" signal.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

// ───────────────────────────── lexer ─────────────────────────────

#[derive(Clone)]
enum Tok {
    Kw(String),
    Ident(String),
    Str,
    Num,
    Char,
    Life,
    Punct(String),
}

fn is_kw(w: &str) -> bool {
    matches!(
        w,
        "as" | "break" | "const" | "continue" | "crate" | "dyn" | "else" | "enum" | "extern"
            | "false" | "fn" | "for" | "if" | "impl" | "in" | "let" | "loop" | "match" | "mod"
            | "move" | "mut" | "pub" | "ref" | "return" | "self" | "Self" | "static" | "struct"
            | "super" | "trait" | "true" | "type" | "unsafe" | "use" | "where" | "while"
            | "async" | "await" | "union"
    )
}

const MULTI: &[&str] = &[
    "..=", "<<=", ">>=", "::", "->", "=>", "==", "!=", "<=", ">=", "&&", "||", "<<", ">>", "+=",
    "-=", "*=", "/=", "%=", "&=", "|=", "^=", "..",
];

fn lex(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let n = b.len();
    let mut i = 0;
    let mut out = Vec::new();
    while i < n {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // comments
        if c == b'/' && i + 1 < n && b[i + 1] == b'/' {
            i += 2;
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < n && b[i + 1] == b'*' {
            i += 2;
            let mut depth = 1;
            while i < n && depth > 0 {
                if c_at(b, i) == b'/' && c_at(b, i + 1) == b'*' {
                    depth += 1;
                    i += 2;
                } else if c_at(b, i) == b'*' && c_at(b, i + 1) == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        // raw strings: r"...", r#"..."#, br"...", br#"..."#
        if (c == b'r' && (c_at(b, i + 1) == b'"' || c_at(b, i + 1) == b'#'))
            || (c == b'b' && c_at(b, i + 1) == b'r')
        {
            if let Some(j) = try_raw_string(b, i) {
                out.push(Tok::Str);
                i = j;
                continue;
            }
        }
        // byte string / byte char
        if c == b'b' && c_at(b, i + 1) == b'"' {
            out.push(Tok::Str);
            i = skip_string(b, i + 1);
            continue;
        }
        if c == b'b' && c_at(b, i + 1) == b'\'' {
            out.push(Tok::Char);
            i = skip_char(b, i + 1);
            continue;
        }
        // normal string
        if c == b'"' {
            out.push(Tok::Str);
            i = skip_string(b, i);
            continue;
        }
        // char literal vs lifetime
        if c == b'\'' {
            if is_char_literal(b, i) {
                out.push(Tok::Char);
                i = skip_char(b, i);
            } else {
                out.push(Tok::Life);
                i += 1;
                while i < n && (b[i] == b'_' || b[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
            }
            continue;
        }
        // number
        if c.is_ascii_digit() {
            out.push(Tok::Num);
            i = read_number(b, i);
            continue;
        }
        // raw identifier r#name
        if c == b'r' && c_at(b, i + 1) == b'#' && is_ident_start(c_at(b, i + 2)) {
            i += 2;
            let s = i;
            while i < n && (b[i] == b'_' || b[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            out.push(Tok::Ident(String::from_utf8_lossy(&b[s..i]).into_owned()));
            continue;
        }
        // identifier / keyword
        if is_ident_start(c) {
            let s = i;
            while i < n && (b[i] == b'_' || b[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            let w = std::str::from_utf8(&b[s..i]).unwrap_or("");
            if is_kw(w) {
                out.push(Tok::Kw(w.to_string()));
            } else {
                out.push(Tok::Ident(w.to_string()));
            }
            continue;
        }
        // punctuation / operators
        let mut matched = false;
        for m in MULTI {
            if b[i..].starts_with(m.as_bytes()) {
                out.push(Tok::Punct((*m).to_string()));
                i += m.len();
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        if c.is_ascii_punctuation() {
            out.push(Tok::Punct((c as char).to_string()));
        }
        i += 1;
    }
    out
}

fn c_at(b: &[u8], i: usize) -> u8 {
    if i < b.len() {
        b[i]
    } else {
        0
    }
}
fn is_ident_start(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphabetic()
}

fn skip_string(b: &[u8], mut i: usize) -> usize {
    // b[i] == '"'
    i += 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => {
                i += 1;
                break;
            }
            _ => i += 1,
        }
    }
    i
}

fn try_raw_string(b: &[u8], i: usize) -> Option<usize> {
    // i at 'r' or 'b' (for br). Find the r, then hashes, then quote.
    let mut p = i;
    if b[p] == b'b' {
        p += 1;
    }
    if c_at(b, p) != b'r' {
        return None;
    }
    p += 1;
    let mut hashes = 0;
    while c_at(b, p) == b'#' {
        hashes += 1;
        p += 1;
    }
    if c_at(b, p) != b'"' {
        return None;
    }
    p += 1;
    // scan to closing " followed by `hashes` #'s
    while p < b.len() {
        if b[p] == b'"' {
            let mut k = 0;
            while k < hashes && c_at(b, p + 1 + k) == b'#' {
                k += 1;
            }
            if k == hashes {
                return Some(p + 1 + hashes);
            }
        }
        p += 1;
    }
    Some(p)
}

fn is_char_literal(b: &[u8], i: usize) -> bool {
    // b[i] == '\''
    if c_at(b, i + 1) == b'\\' {
        return true; // escaped char
    }
    // 'x' : a single char then a closing quote
    c_at(b, i + 2) == b'\''
}

fn skip_char(b: &[u8], mut i: usize) -> usize {
    // b[i] == '\''
    i += 1;
    if c_at(b, i) == b'\\' {
        i += 1;
        match c_at(b, i) {
            b'u' | b'x' => {
                i += 1;
                if c_at(b, i) == b'{' {
                    while i < b.len() && b[i] != b'}' {
                        i += 1;
                    }
                    i += 1;
                } else {
                    while i < b.len() && b[i].is_ascii_hexdigit() {
                        i += 1;
                    }
                }
            }
            _ => i += 1,
        }
    } else if i < b.len() {
        i += 1;
    }
    if c_at(b, i) == b'\'' {
        i += 1;
    }
    i
}

fn read_number(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && (b[i] == b'_' || b[i].is_ascii_alphanumeric()) {
        i += 1;
    }
    if c_at(b, i) == b'.' && c_at(b, i + 1).is_ascii_digit() {
        i += 1;
        while i < b.len() && (b[i] == b'_' || b[i].is_ascii_alphanumeric()) {
            i += 1;
        }
    }
    i
}

// ──────────────────────── function extraction ────────────────────────

struct RawFunc {
    name: String,
    body: Vec<Tok>, // tokens of the body INCLUDING outer braces
}

/// Find `fn IDENT ... { ... }` definitions on the token stream and return the
/// body token slice for each (skips bodyless declarations `fn f();`).
fn extract_functions(toks: &[Tok]) -> Vec<RawFunc> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let is_fn = matches!(&toks[i], Tok::Kw(k) if k == "fn");
        if is_fn {
            if let Some(Tok::Ident(name)) = toks.get(i + 1) {
                // find body open `{` at paren/bracket depth 0, or `;` => no body
                let mut j = i + 2;
                let mut pd = 0i32; // () [] depth
                let mut body_start = None;
                while j < toks.len() {
                    match &toks[j] {
                        Tok::Punct(p) if p == "(" || p == "[" => pd += 1,
                        Tok::Punct(p) if p == ")" || p == "]" => pd -= 1,
                        Tok::Punct(p) if p == "{" && pd == 0 => {
                            body_start = Some(j);
                            break;
                        }
                        Tok::Punct(p) if p == ";" && pd == 0 => break,
                        _ => {}
                    }
                    j += 1;
                }
                if let Some(bs) = body_start {
                    // brace-match
                    let mut depth = 0i32;
                    let mut k = bs;
                    let mut end = bs;
                    while k < toks.len() {
                        match &toks[k] {
                            Tok::Punct(p) if p == "{" => depth += 1,
                            Tok::Punct(p) if p == "}" => {
                                depth -= 1;
                                if depth == 0 {
                                    end = k;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        k += 1;
                    }
                    if end > bs {
                        out.push(RawFunc {
                            name: name.clone(),
                            body: toks[bs..=end].to_vec(),
                        });
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

// ──────────────────────── normalization ────────────────────────

/// Normalize a function body to a rename-insensitive token sequence. Locals are
/// alpha-renamed (`V0`,`V1`,…); types/variants/consts (uppercase), call/macro/
/// path segments, and field/method names KEEP their names; literals collapse to
/// `STR`/`NUM`/`CHR`. Returns interned token ids.
fn normalize(body: &[Tok], interner: &mut Interner) -> Vec<u32> {
    let mut locals: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::with_capacity(body.len());
    for p in 0..body.len() {
        let id = match &body[p] {
            Tok::Kw(k) => interner.get(k),
            Tok::Str => interner.get("STR"),
            Tok::Num => interner.get("NUM"),
            Tok::Char => interner.get("CHR"),
            Tok::Life => interner.get("LT"),
            Tok::Punct(s) => interner.get(s),
            Tok::Ident(name) => {
                let prev = if p > 0 { body.get(p - 1) } else { None };
                let next = body.get(p + 1);
                if keep_name(name, prev, next) {
                    interner.get(&format!("#{name}"))
                } else {
                    let m = locals.len();
                    let v = *locals.entry(name.clone()).or_insert(m);
                    interner.get(&format!("V{v}"))
                }
            }
        };
        out.push(id);
    }
    out
}

/// Keep an identifier's literal name (vs. blinding it as a local) when it names an
/// API/type/field — the part of a function that makes it semantically specific.
fn keep_name(name: &str, prev: Option<&Tok>, next: Option<&Tok>) -> bool {
    // type / variant / const convention
    if name.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false) {
        return true;
    }
    // call, macro, or path segment: ident( / ident! / ident::
    if let Some(Tok::Punct(p)) = next {
        if p == "(" || p == "!" || p == "::" {
            return true;
        }
    }
    // field / method / path segment: .ident  ::ident
    if let Some(Tok::Punct(p)) = prev {
        if p == "." || p == "::" {
            return true;
        }
    }
    false
}

#[derive(Default)]
struct Interner {
    map: HashMap<String, u32>,
}
impl Interner {
    fn get(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        let id = self.map.len() as u32;
        self.map.insert(s.to_string(), id);
        id
    }
}

// ──────────────────────── hashing / minhash ────────────────────────

fn fnv1a(s: &[u32]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &x in s {
        for byte in x.to_le_bytes() {
            h ^= byte as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

fn splitmix_seeds(n: usize) -> Vec<u64> {
    let mut s = 0x243F6A8885A308D3u64;
    (0..n)
        .map(|_| {
            s = s.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            z ^ (z >> 31)
        })
        .collect()
}

fn mix(h: u64, seed: u64) -> u64 {
    let mut z = h ^ seed;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

const KGRAM: usize = 5;
const SIG: usize = 32;
const BANDS: usize = 8;
const ROWS: usize = SIG / BANDS;

fn kgram_hashes(norm: &[u32]) -> Vec<u64> {
    if norm.len() < KGRAM {
        return Vec::new();
    }
    let mut v: Vec<u64> = norm.windows(KGRAM).map(fnv1a).collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn minhash(kgrams: &[u64], seeds: &[u64]) -> Vec<u64> {
    seeds
        .iter()
        .map(|&sd| kgrams.iter().map(|&k| mix(k, sd)).min().unwrap_or(u64::MAX))
        .collect()
}

// ──────────────────────── crate identity ────────────────────────

/// Nearest ancestor directory containing a Cargo.toml (relative to `root`) is the
/// crate; files with none fall back to their first path component under root.
fn crate_of(file: &Path, root: &Path, cache: &mut HashMap<PathBuf, String>) -> String {
    let mut dir = file.parent().map(|p| p.to_path_buf());
    while let Some(d) = dir {
        if !d.starts_with(root) {
            break;
        }
        if let Some(hit) = cache.get(&d) {
            return hit.clone();
        }
        if d.join("Cargo.toml").exists() {
            let name = d
                .strip_prefix(root)
                .ok()
                .map(|r| r.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| ".".to_string());
            cache.insert(d.clone(), name.clone());
            return name;
        }
        if d == root {
            break;
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    // fallback: first component under root
    file.strip_prefix(root)
        .ok()
        .and_then(|r| r.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()))
        .unwrap_or_else(|| "(root)".to_string())
}

fn walk_rs(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(root) else { return };
    for ent in rd.flatten() {
        let p = ent.path();
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if matches!(name.as_ref(), "target" | ".git" | "node_modules" | "vendor") {
                continue;
            }
            walk_rs(&p, out);
        } else if p.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push(p);
        }
    }
}

// ──────────────────────── analysis types ────────────────────────

struct Func {
    crate_id: usize,
    file: String,
    name: String,
    norm: Vec<u32>,
    nhash: u64,
}

struct Cluster {
    members: Vec<usize>,
    tokens: usize,
    crates: Vec<usize>,
    dup_tokens: u64, // (members-1) * tokens
}

// union-find for near-dup clustering
struct Dsu {
    p: Vec<usize>,
}
impl Dsu {
    fn new(n: usize) -> Self {
        Self { p: (0..n).collect() }
    }
    fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.p[r] != r {
            r = self.p[r];
        }
        let mut c = x;
        while self.p[c] != c {
            let n = self.p[c];
            self.p[c] = r;
            c = n;
        }
        r
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.p[ra] = rb;
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| ".".to_string()));
    let min_tokens: usize = arg_val(&args, "--min-tokens").unwrap_or(28);
    let top: usize = arg_val(&args, "--top").unwrap_or(12);
    let do_near = !args.iter().any(|a| a == "--no-near");

    // collect files
    let mut files = Vec::new();
    walk_rs(&root, &mut files);
    files.sort();

    // extract + normalize
    let mut interner = Interner::default();
    let mut crate_cache: HashMap<PathBuf, String> = HashMap::new();
    let mut crate_ids: HashMap<String, usize> = HashMap::new();
    let mut crate_names: Vec<String> = Vec::new();
    let mut funcs: Vec<Func> = Vec::new();
    let mut total_loc: usize = 0;

    for f in &files {
        let Ok(src) = fs::read_to_string(f) else { continue };
        total_loc += src.lines().count();
        let toks = lex(&src);
        let raws = extract_functions(&toks);
        if raws.is_empty() {
            continue;
        }
        let cname = crate_of(f, &root, &mut crate_cache);
        let cid = *crate_ids.entry(cname.clone()).or_insert_with(|| {
            crate_names.push(cname.clone());
            crate_names.len() - 1
        });
        let rel = f.strip_prefix(&root).unwrap_or(f).to_string_lossy().into_owned();
        for rf in raws {
            let norm = normalize(&rf.body, &mut interner);
            if norm.len() < min_tokens {
                continue;
            }
            let nhash = fnv1a(&norm);
            funcs.push(Func { crate_id: cid, file: rel.clone(), name: rf.name, norm, nhash });
        }
    }

    // ── exact (Type-2) clustering: group by nhash, verify by exact norm equality ──
    let mut by_hash: HashMap<u64, Vec<usize>> = HashMap::new();
    for (idx, fc) in funcs.iter().enumerate() {
        by_hash.entry(fc.nhash).or_default().push(idx);
    }
    let mut clusters: Vec<Cluster> = Vec::new();
    for (_h, idxs) in by_hash {
        // split the bucket by exact normalized-sequence equality (guard hash collisions)
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for &ix in &idxs {
            match groups.iter_mut().find(|g| funcs[g[0]].norm == funcs[ix].norm) {
                Some(g) => g.push(ix),
                None => groups.push(vec![ix]),
            }
        }
        for g in groups {
            if g.len() < 2 {
                continue;
            }
            let tokens = funcs[g[0]].norm.len();
            let mut crates: Vec<usize> = g.iter().map(|&i| funcs[i].crate_id).collect();
            crates.sort_unstable();
            crates.dedup();
            let dup_tokens = (g.len() as u64 - 1) * tokens as u64;
            clusters.push(Cluster { members: g, tokens, crates, dup_tokens });
        }
    }
    clusters.sort_by(|a, b| b.dup_tokens.cmp(&a.dup_tokens));

    let total_dup: u64 = clusters.iter().map(|c| c.dup_tokens).sum();
    let xcrate: Vec<&Cluster> = clusters.iter().filter(|c| c.crates.len() >= 2).collect();
    let xcrate_dup: u64 = xcrate.iter().map(|c| c.dup_tokens).sum();

    // ── report header ──
    println!("# kitfinder report");
    println!();
    println!(
        "scanned {} files (~{} LOC) across {} crates; {} functions ≥ {} tokens",
        files.len(),
        total_loc,
        crate_names.len(),
        funcs.len(),
        min_tokens
    );
    println!(
        "exact clone classes: {} ({} duplicated tokens); cross-crate: {} ({} tokens)",
        clusters.len(),
        total_dup,
        xcrate.len(),
        xcrate_dup
    );
    println!();

    // ── top exact clusters ──
    println!("## top exact clone classes (by duplicated tokens)");
    println!();
    for c in clusters.iter().take(top) {
        report_cluster(c, &funcs, &crate_names, &interner);
    }

    // ── kit aggregation: crate-pair shared-clone mass ──
    println!("## cross-crate clone mass — where a kit wants to form");
    println!();
    let mut pair_mass: HashMap<(usize, usize), u64> = HashMap::new();
    let mut crate_mass: HashMap<usize, u64> = HashMap::new();
    for c in &xcrate {
        for i in 0..c.crates.len() {
            *crate_mass.entry(c.crates[i]).or_insert(0) += c.dup_tokens;
            for j in (i + 1)..c.crates.len() {
                let key = (c.crates[i].min(c.crates[j]), c.crates[i].max(c.crates[j]));
                *pair_mass.entry(key).or_insert(0) += c.dup_tokens;
            }
        }
    }
    let mut pairs: Vec<((usize, usize), u64)> = pair_mass.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1));
    println!("crate pairs sharing the most clone mass:");
    for ((a, b), m) in pairs.iter().take(top) {
        println!(
            "  {:>7} tok   {}  ↔  {}",
            m, crate_names[*a], crate_names[*b]
        );
    }
    println!();
    let mut cm: Vec<(usize, u64)> = crate_mass.into_iter().collect();
    cm.sort_by(|a, b| b.1.cmp(&a.1));
    println!("crates most entangled in cross-crate duplication:");
    for (c, m) in cm.iter().take(top) {
        println!("  {:>7} tok   {}", m, crate_names[*c]);
    }
    println!();

    // ── near-duplicate (approximate Type-3) ──
    if do_near {
        println!("## near-duplicate families (MinHash/LSH, ~0.75+ similar, not exact)");
        println!();
        let seeds = splitmix_seeds(SIG);
        let mut sigs: Vec<Option<Vec<u64>>> = Vec::with_capacity(funcs.len());
        for fc in &funcs {
            let kg = kgram_hashes(&fc.norm);
            sigs.push(if kg.is_empty() { None } else { Some(minhash(&kg, &seeds)) });
        }
        // LSH banding → candidate pairs
        let mut buckets: HashMap<(usize, u64), Vec<usize>> = HashMap::new();
        for (ix, sg) in sigs.iter().enumerate() {
            if let Some(sig) = sg {
                for band in 0..BANDS {
                    let slice = &sig[band * ROWS..band * ROWS + ROWS];
                    let key = fnv1a(&slice.iter().flat_map(|x| {
                        [(*x as u32), ((*x >> 32) as u32)]
                    }).collect::<Vec<u32>>());
                    buckets.entry((band, key)).or_default().push(ix);
                }
            }
        }
        let mut dsu = Dsu::new(funcs.len());
        let mut edges = 0usize;
        let mut seen_pairs: HashMap<(usize, usize), ()> = HashMap::new();
        for (_k, members) in buckets {
            if members.len() < 2 || members.len() > 400 {
                continue; // skip degenerate mega-buckets
            }
            for a in 0..members.len() {
                for b in (a + 1)..members.len() {
                    let (x, y) = (members[a], members[b]);
                    if funcs[x].nhash == funcs[y].nhash {
                        continue; // exact clone, not "near"
                    }
                    let key = (x.min(y), y.max(x));
                    if seen_pairs.insert(key, ()).is_some() {
                        continue;
                    }
                    let (sx, sy) = (sigs[x].as_ref().unwrap(), sigs[y].as_ref().unwrap());
                    let agree = sx.iter().zip(sy).filter(|(a, b)| a == b).count();
                    let sim = agree as f64 / SIG as f64;
                    if sim >= 0.75 {
                        dsu.union(x, y);
                        edges += 1;
                    }
                }
            }
        }
        // gather near clusters (that have at least one near edge)
        let mut near: HashMap<usize, Vec<usize>> = HashMap::new();
        for ix in 0..funcs.len() {
            if sigs[ix].is_some() {
                let r = dsu.find(ix);
                near.entry(r).or_default().push(ix);
            }
        }
        let mut near_clusters: Vec<Vec<usize>> =
            near.into_values().filter(|g| g.len() >= 2).collect();
        // rank by spanned crates then size*tokens
        near_clusters.sort_by(|a, b| {
            let sc = |g: &Vec<usize>| -> (usize, u64) {
                let mut cr: Vec<usize> = g.iter().map(|&i| funcs[i].crate_id).collect();
                cr.sort_unstable();
                cr.dedup();
                let mass: u64 = g.iter().map(|&i| funcs[i].norm.len() as u64).sum();
                (cr.len(), mass)
            };
            sc(b).cmp(&sc(a))
        });
        println!("(found {} near edges)", edges);
        println!();
        let mut shown = 0;
        for g in &near_clusters {
            let mut cr: Vec<usize> = g.iter().map(|&i| funcs[i].crate_id).collect();
            cr.sort_unstable();
            cr.dedup();
            if cr.len() < 2 {
                continue; // focus on cross-crate near-dup families
            }
            print_near(g, &funcs, &crate_names);
            shown += 1;
            if shown >= top {
                break;
            }
        }
        if shown == 0 {
            println!("  (no cross-crate near-duplicate families above threshold)");
        }
        println!();
    }

    println!("---");
    println!("note: token-level normalization (locals blinded, APIs/types/control-flow kept).");
    println!("catches Type-1/2 and approximate Type-3 clones; semantic clones (same behavior,");
    println!("different code) need the AST/PDG + embedding layer. Cross-crate mass is the kit signal.");
}

fn report_cluster(c: &Cluster, funcs: &[Func], _names: &[String], interner: &Interner) {
    let span = c.crates.len();
    let flag = if span >= 2 { "  «cross-crate»" } else { "" };
    println!(
        "- {} sites, {} tokens each, {} dup tokens, {} crate(s){}",
        c.members.len(),
        c.tokens,
        c.dup_tokens,
        span,
        flag
    );
    let mut shown = 0;
    for &m in &c.members {
        println!("    {}()  —  {}", funcs[m].name, funcs[m].file);
        shown += 1;
        if shown >= 6 {
            let rest = c.members.len() - shown;
            if rest > 0 {
                println!("    … +{} more", rest);
            }
            break;
        }
    }
    println!("    sig: {}", preview_sig(&funcs[c.members[0]].norm, interner, 18));
    println!();
}

fn print_near(g: &[usize], funcs: &[Func], names: &[String]) {
    let mut cr: Vec<usize> = g.iter().map(|&i| funcs[i].crate_id).collect();
    cr.sort_unstable();
    cr.dedup();
    let mass: u64 = g.iter().map(|&i| funcs[i].norm.len() as u64).sum();
    let crate_list: Vec<&str> = cr.iter().map(|&c| names[c].as_str()).collect();
    println!(
        "- {} near-dup functions across {} crates (~{} tokens): {}",
        g.len(),
        cr.len(),
        mass,
        crate_list.join(", ")
    );
    let mut shown = 0;
    for &m in g {
        println!("    {}()  —  {}", funcs[m].name, funcs[m].file);
        shown += 1;
        if shown >= 6 {
            let rest = g.len() - shown;
            if rest > 0 {
                println!("    … +{} more", rest);
            }
            break;
        }
    }
    println!();
}

/// Render the first `n` normalized tokens back to readable text for eyeballing.
fn preview_sig(norm: &[u32], interner: &Interner, n: usize) -> String {
    // reverse the interner once (small)
    let mut rev: Vec<&str> = vec![""; interner.map.len()];
    for (s, &id) in &interner.map {
        rev[id as usize] = s;
    }
    norm.iter()
        .take(n)
        .map(|&id| rev[id as usize])
        .collect::<Vec<_>>()
        .join(" ")
        + if norm.len() > n { " …" } else { "" }
}

fn arg_val(args: &[String], flag: &str) -> Option<usize> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
}
