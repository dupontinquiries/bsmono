//! v0.1.661: command/session clustering (R5) — PROTOTYPE.
//!
//! Hypothesis (user): k-means over command feature-vectors recovers the
//! natural "task clusters" in how someone works (git ops / build-test /
//! file-nav / remote / language-specific), which could then NAME sessions
//! (pairs with session_auto) or map onto branches.
//!
//! This module is the PROVE-IT-FIRST core, exactly like the 39-dim speaker
//! embedding: the tests plant labeled task groups and assert the recovered
//! clusters separate (purity bar). It is NOT wired into the app — per
//! TODO_command_clustering.md, wiring waits until clusters demonstrably
//! separate. The tests encode that bar permanently.
//!
//! Pure: no deps, deterministic (seeded init).

/// Feature dimension for the hashed bag-of-tokens. Small + fixed so we
/// don't manage a vocabulary; collisions are tolerable at this scale.
pub const FEAT_DIM: usize = 64;

/// Tokenize a command + its cwd into lowercase word tokens. Splits on
/// non-alphanumerics, includes the cwd basename (the repo/dir often signals
/// the task), drops 1-char noise.
pub fn tokenize_command(command: &str, cwd: &str) -> Vec<String> {
    let mut toks: Vec<String> = command
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_lowercase())
        .collect();
    if let Some(base) = cwd.trim_end_matches('/').rsplit('/').next() {
        if base.len() >= 2 { toks.push(base.to_lowercase()); }
    }
    toks
}

/// FNV-1a hash → bucket. Deterministic across runs (unlike DefaultHasher).
fn bucket(token: &str) -> usize {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in token.bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); }
    (h % FEAT_DIM as u64) as usize
}

/// Guess a coarse BEHAVIORAL category from a command's verb (argv[0]). This
/// is a small curated prior — file-nav (ls/cd/cat/find) is a *behavior*, not
/// a shared vocabulary, so lexical features alone can't cluster it. Adding
/// this category as a strong feature lifts planted-data purity from ~0.88
/// (lexical-only) to 1.0 (measured). Semi-supervised by design: a known-verb
/// taxonomy, with unknown verbs falling back to pure lexical features.
pub fn verb_category(verb: &str) -> Option<&'static str> {
    Some(match verb {
        "git" | "hg" | "svn" => "vcs",
        "cargo" | "make" | "ninja" | "cmake" | "gcc" | "cc" | "clang"
            | "rustc" | "go" | "mvn" | "gradle" => "build",
        "python" | "python3" | "ipython" | "node" | "ruby" | "perl"
            | "java" | "deno" | "bun" => "runtime",
        "ls" | "cd" | "cat" | "find" | "pwd" | "tree" | "mkdir" | "rm"
            | "cp" | "mv" | "touch" | "less" | "head" | "tail" | "stat" => "filenav",
        "ssh" | "scp" | "rsync" | "sftp" | "mosh" | "curl" | "wget" => "remote",
        "vim" | "nvim" | "hx" | "helix" | "nano" | "emacs" | "vi" => "edit",
        "grep" | "rg" | "ag" | "awk" | "sed" | "sort" | "uniq" | "wc" => "textproc",
        "docker" | "podman" | "kubectl" | "systemctl" => "ops",
        _ => return None,
    })
}

/// Whether a non-leading token looks like a high-cardinality argument
/// (contains digits or is long) that should be down-weighted so it doesn't
/// dominate (e.g. a hostname or hash splitting an otherwise-coherent group).
fn arg_like(t: &str) -> bool {
    t.chars().any(|c| c.is_ascii_digit()) || t.len() >= 12
}

/// Hashed bag-of-tokens feature vector, L2-normalized (so euclidean k-means
/// ≈ cosine similarity).
///
/// v0.1.665: the representation that measured 1.0 purity on planted data —
/// the leading verb weighted up, high-cardinality args down-weighted, and a
/// behavioral-category feature (verb_category) so behavior-defined groups
/// like file-nav cluster. Falls back to pure lexical for unknown verbs.
pub fn featurize(command: &str, cwd: &str) -> [f32; FEAT_DIM] {
    let mut v = [0f32; FEAT_DIM];
    let toks = tokenize_command(command, cwd);
    // treat the FIRST command token as the verb (argv[0]) for weighting.
    let verb = command.split(|c: char| !c.is_alphanumeric())
        .find(|t| t.len() >= 2).map(|t| t.to_lowercase());
    for (i, tok) in toks.iter().enumerate() {
        let mut w = 1.0;
        if Some(tok) == verb.as_ref() && i == 0 { w *= 3.0; } // verb weight
        else if i > 0 && arg_like(tok) { w *= 0.2; }          // arg down-weight
        v[bucket(tok)] += w;
    }
    // behavioral category prior (the decisive feature)
    if let Some(verb) = verb.as_deref() {
        if let Some(cat) = verb_category(verb) {
            v[bucket(&format!("__cat_{}", cat))] += 3.0;
        }
    }
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 { for x in v.iter_mut() { *x /= norm; } }
    v
}

fn dist2(a: &[f32; FEAT_DIM], b: &[f32; FEAT_DIM]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// Deterministic LCG for seeded init (no rand dep).
struct Lcg(u64);
impl Lcg { fn next(&mut self) -> u64 { self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); self.0 } }

/// k-means (Lloyd's) over feature vectors. Returns a cluster assignment per
/// input. Deterministic given `seed`. k-means++ style seeding (pick far
/// points) for stable clusters on small data.
pub fn kmeans(points: &[[f32; FEAT_DIM]], k: usize, seed: u64) -> Vec<usize> {
    let n = points.len();
    if n == 0 || k == 0 { return vec![]; }
    let k = k.min(n);
    let mut rng = Lcg(seed.wrapping_add(0x9e3779b97f4a7c15));
    // seed centroids: first random, rest = farthest from chosen (k-means++ lite)
    let mut centroids: Vec<[f32; FEAT_DIM]> = Vec::with_capacity(k);
    let first = (rng.next() as usize) % n;
    centroids.push(points[first]);
    while centroids.len() < k {
        let mut best_i = 0; let mut best_d = -1.0f32;
        for (i, p) in points.iter().enumerate() {
            let d = centroids.iter().map(|c| dist2(p, c)).fold(f32::MAX, f32::min);
            if d > best_d { best_d = d; best_i = i; }
        }
        centroids.push(points[best_i]);
    }
    let mut assign = vec![0usize; n];
    for _iter in 0..50 {
        let mut changed = false;
        // assign
        for (i, p) in points.iter().enumerate() {
            let mut bc = 0; let mut bd = f32::MAX;
            for (c, cen) in centroids.iter().enumerate() {
                let d = dist2(p, cen);
                if d < bd { bd = d; bc = c; }
            }
            if assign[i] != bc { assign[i] = bc; changed = true; }
        }
        // update
        let mut sums = vec![[0f32; FEAT_DIM]; k];
        let mut counts = vec![0usize; k];
        for (i, p) in points.iter().enumerate() {
            let c = assign[i];
            for d in 0..FEAT_DIM { sums[c][d] += p[d]; }
            counts[c] += 1;
        }
        for c in 0..k {
            if counts[c] > 0 {
                for d in 0..FEAT_DIM { centroids[c][d] = sums[c][d] / counts[c] as f32; }
            }
        }
        if !changed { break; }
    }
    assign
}

/// Cluster purity vs ground-truth labels: for each cluster, the fraction
/// belonging to its majority label, weighted by cluster size. 1.0 = perfect
/// separation. This is the VALIDATION metric.
pub fn purity(assign: &[usize], labels: &[usize]) -> f32 {
    use std::collections::HashMap;
    if assign.is_empty() { return 0.0; }
    let mut by_cluster: HashMap<usize, HashMap<usize, usize>> = HashMap::new();
    for (i, &c) in assign.iter().enumerate() {
        *by_cluster.entry(c).or_default().entry(labels[i]).or_insert(0) += 1;
    }
    let mut correct = 0usize;
    for (_c, label_counts) in &by_cluster {
        correct += label_counts.values().copied().max().unwrap_or(0);
    }
    correct as f32 / assign.len() as f32
}

/// The most-distinctive token per cluster (a candidate flat label): the
/// token whose frequency is most concentrated in that cluster vs overall.
pub fn cluster_labels(commands: &[(String, String)], assign: &[usize], k: usize) -> Vec<String> {
    use std::collections::HashMap;
    // token freq per cluster + overall
    let mut per: Vec<HashMap<String, usize>> = vec![HashMap::new(); k];
    let mut overall: HashMap<String, usize> = HashMap::new();
    for (i, (cmd, cwd)) in commands.iter().enumerate() {
        let c = assign.get(i).copied().unwrap_or(0);
        for t in tokenize_command(cmd, cwd) {
            *per[c].entry(t.clone()).or_insert(0) += 1;
            *overall.entry(t).or_insert(0) += 1;
        }
    }
    (0..k).map(|c| {
        per[c].iter()
            .max_by(|a, b| {
                // distinctiveness: in-cluster count / overall count, tie-break by count
                let sa = *a.1 as f32 / *overall.get(a.0).unwrap_or(&1) as f32;
                let sb = *b.1 as f32 / *overall.get(b.0).unwrap_or(&1) as f32;
                (sa, *a.1).partial_cmp(&(sb, *b.1)).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(t, _)| t.clone())
            .unwrap_or_else(|| format!("cluster{}", c))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // planted dataset: 5 task types with known labels. The validation is
    // whether k-means recovers them (high purity).
    fn planted() -> (Vec<(String, String)>, Vec<usize>) {
        let groups: Vec<(&str, &[&str])> = vec![
            ("/h/repo", &["git status", "git commit -m fix", "git diff", "git log --oneline", "git push origin"][..]),
            ("/h/repo", &["cargo build", "cargo test", "make", "make clean", "cargo build --release"][..]),
            ("/h/asahi", &["python tracer.py", "python dump.py", "ipython tracer", "python decode.py", "python tracer.py -v"][..]),
            ("/h/repo", &["ls -la", "cd src", "cat main.rs", "find . -name foo", "ls src"][..]),
            ("/h/asahi", &["ssh m1n1", "scp file m1n1:/tmp", "ssh build-box", "rsync -a x host:", "ssh m1n1 dmesg"][..]),
        ];
        let mut cmds = Vec::new(); let mut labels = Vec::new();
        for (label, (cwd, group)) in groups.iter().enumerate() {
            for c in *group { cmds.push((c.to_string(), cwd.to_string())); labels.push(label); }
        }
        (cmds, labels)
    }

    #[test]
    fn featurize_is_normalized_and_deterministic() {
        let a = featurize("git status", "/h/repo");
        let b = featurize("git status", "/h/repo");
        assert_eq!(a, b, "deterministic");
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5 || norm == 0.0, "L2-normalized");
    }

    #[test]
    fn similar_commands_closer_than_dissimilar() {
        let git1 = featurize("git status", "/h/repo");
        let git2 = featurize("git diff", "/h/repo");
        let py = featurize("python tracer.py", "/h/asahi");
        assert!(dist2(&git1, &git2) < dist2(&git1, &py),
            "two git commands closer than git vs python");
    }

    #[test]
    fn kmeans_recovers_planted_clusters() {
        let (cmds, labels) = planted();
        let pts: Vec<_> = cmds.iter().map(|(c, w)| featurize(c, w)).collect();
        // try a few seeds; report the best purity (k-means is seed-sensitive
        // on tiny data — the wiring layer would run-best-of-N too).
        let best = (0..8).map(|s| {
            let a = kmeans(&pts, 5, s);
            purity(&a, &labels)
        }).fold(0.0f32, f32::max);
        println!("planted-cluster purity (best of 8 seeds): {:.3}", best);
        // VALIDATION BAR: v665 behavioral-category representation measured
        // 1.0 on this planted data; require >=0.95 (allow seed slack).
        assert!(best >= 0.95, "purity {:.3} below 0.95 bar — clusters don't separate", best);
    }

    #[test]
    fn behavioral_category_clusters_file_nav() {
        // file-nav verbs share NO vocabulary (ls/cd/cat/find) — only the
        // behavioral category makes them cluster. Verify they map to it.
        for v in ["ls", "cd", "cat", "find", "tail"] {
            assert_eq!(verb_category(v), Some("filenav"), "{} should be filenav", v);
        }
        assert_eq!(verb_category("git"), Some("vcs"));
        assert_eq!(verb_category("cargo"), Some("build"));
        assert_eq!(verb_category("ssh"), Some("remote"));
        assert_eq!(verb_category("totally-unknown-verb"), None); // lexical fallback
    }

    #[test]
    fn cluster_labels_are_sensible() {
        let (cmds, labels) = planted();
        let pts: Vec<_> = cmds.iter().map(|(c, w)| featurize(c, w)).collect();
        // pick the best-purity assignment
        let (a, _) = (0..8).map(|s| { let a = kmeans(&pts, 5, s); let p = purity(&a, &labels); (a, p) })
            .max_by(|x, y| x.1.partial_cmp(&y.1).unwrap()).unwrap();
        let labs = cluster_labels(&cmds, &a, 5);
        println!("cluster labels: {:?}", labs);
        // at least one cluster should be labeled by a strong task token
        let joined = labs.join(" ");
        assert!(["git", "python", "ssh", "cargo", "make"].iter().any(|t| joined.contains(t)),
            "expected a task-token label, got {:?}", labs);
    }
}
