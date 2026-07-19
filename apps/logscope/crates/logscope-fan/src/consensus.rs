//! consensus.rs — the engine behind "fuzz the prompt into N branches, then merge." It's self-
//! consistency / best-of-N seen as a tree: perturb one prompt into many, let the small model answer
//! each (cheap, parallel-in-spirit even if serial here), then aggregate like a CAPTCHA solver pools
//! noisy reads — cluster the answers, throw out the degenerate "garbage," and take what the majority
//! of branches agree on. All pure + testable; the generation loop and the cluster→message expansion
//! UI sit on top.

/// Small LCG so a fuzz seed reproduces the same fan (same pattern as voiceterm's cmd_cluster).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 { self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); self.0 }
    fn pick(&mut self, n: usize) -> usize { if n == 0 { 0 } else { (self.next() >> 33) as usize % n } }
}

const STOP: &[&str] = &["the","a","an","and","or","but","is","are","was","to","of","in","on","for","it","this","that","with","as","at","be","by","i","you","we","so","do","now"];

fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '_' && c != ':')
        .filter(|t| t.len() >= 2 && !STOP.contains(t))
        .map(|t| t.to_string()).collect()
}

fn jaccard(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() && b.is_empty() { return 1.0; }
    if a.is_empty() || b.is_empty() { return 0.0; }
    let inter = a.iter().filter(|w| b.contains(w)).count();
    let union = a.len() + b.iter().filter(|w| !a.contains(w)).count();
    if union == 0 { 0.0 } else { inter as f32 / union as f32 }
}

/// Perturb one prompt into `n` distinct variants by varying the *framing* (prefix/suffix nudges) —
/// enough to diversify a small model's output without changing the ask. Deterministic given `seed`.
pub fn fuzz(input: &str, n: usize, seed: u64) -> Vec<String> {
    let core = input.trim();
    let prefixes = ["", "Briefly, ", "Step by step, ", "Think carefully — ", "Concisely, ",
                    "In detail, ", "As an expert, ", "Directly: "];
    let suffixes = ["", " Explain the reasoning.", " Be concise.", " Show a short example.",
                    " Keep it minimal."];
    let mut rng = Lcg(seed ^ 0x9e3779b97f4a7c15);
    let mut out: Vec<String> = Vec::new();
    let mut guard = 0;
    while out.len() < n && guard < n * 12 + 50 {
        guard += 1;
        let p = prefixes[rng.pick(prefixes.len())];
        let s = suffixes[rng.pick(suffixes.len())];
        let v = format!("{p}{core}{s}");
        if !out.contains(&v) { out.push(v); }
    }
    // if the combo space ran out before n, top up with a disambiguating marker (still a valid prompt)
    let mut k = 2;
    while out.len() < n { out.push(format!("{core} (take {k})")); k += 1; }
    out
}

/// Group outputs by Jaccard similarity of their content tokens (greedy single-link to a cluster's
/// first member). Returns clusters of indices into `outputs`. `threshold` ~0.3–0.5.
pub fn cluster(outputs: &[String], threshold: f32) -> Vec<Vec<usize>> {
    let toks: Vec<Vec<String>> = outputs.iter().map(|o| tokens(o)).collect();
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for i in 0..outputs.len() {
        let mut placed = false;
        for c in &mut clusters {
            if jaccard(&toks[i], &toks[c[0]]) >= threshold { c.push(i); placed = true; break; }
        }
        if !placed { clusters.push(vec![i]); }
    }
    clusters
}

/// 0.0 = looks like real content, 1.0 = degenerate "garbage" (empty, too short, or repetitive —
/// the failure mode small models fall into: looping the same tokens, low lexical variety).
pub fn garbage_score(text: &str) -> f32 {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 3 { return 0.9; }
    let total = words.len() as f32;
    let uniq = { let mut v: Vec<&str> = words.clone(); v.sort_unstable(); v.dedup(); v.len() as f32 };
    let repetition = 1.0 - (uniq / total); // high when few distinct words
    // consecutive immediate repeats ("sort sort sort")
    let consec = words.windows(2).filter(|w| w[0] == w[1]).count() as f32 / (total - 1.0).max(1.0);
    let score = 0.65 * repetition + 0.35 * consec;
    score.clamp(0.0, 1.0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Consensus {
    pub chosen: usize,        // index into outputs: the representative answer to keep/merge
    pub cluster: Vec<usize>,  // the agreeing branches (the winning cluster)
    pub agreement: f32,       // cluster size / number of non-garbage branches (how strong the vote)
    pub kept: usize,          // how many branches survived the garbage filter
}

/// Aggregate N branch outputs into one. Drop degenerate branches, cluster the rest, take the LARGEST
/// cluster (the answer the most branches converged on), and within it pick the cleanest member. Falls
/// back to the least-garbage branch when nothing survives. None only for an empty input.
pub fn consensus(outputs: &[String], cluster_threshold: f32, garbage_cutoff: f32) -> Option<Consensus> {
    if outputs.is_empty() { return None; }
    let good: Vec<usize> = (0..outputs.len()).filter(|&i| garbage_score(&outputs[i]) <= garbage_cutoff).collect();
    if good.is_empty() {
        // everything degenerate — return the least-bad so the user still gets something
        let chosen = (0..outputs.len()).min_by(|&a, &b| garbage_score(&outputs[a]).partial_cmp(&garbage_score(&outputs[b])).unwrap()).unwrap();
        return Some(Consensus { chosen, cluster: vec![chosen], agreement: 0.0, kept: 0 });
    }
    let kept_texts: Vec<String> = good.iter().map(|&i| outputs[i].clone()).collect();
    let clusters = cluster(&kept_texts, cluster_threshold); // indices into kept_texts
    let biggest = clusters.iter().max_by_key(|c| c.len()).unwrap();
    // cleanest member of the winning cluster (lowest garbage)
    let best_local = *biggest.iter().min_by(|&&a, &&b|
        garbage_score(&kept_texts[a]).partial_cmp(&garbage_score(&kept_texts[b])).unwrap()).unwrap();
    let cluster_global: Vec<usize> = biggest.iter().map(|&li| good[li]).collect();
    Some(Consensus {
        chosen: good[best_local],
        cluster: cluster_global.clone(),
        agreement: biggest.len() as f32 / good.len() as f32,
        kept: good.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzz_makes_n_distinct_variants_containing_the_core() {
        let v = fuzz("sort a vec of structs by weight", 12, 7);
        assert_eq!(v.len(), 12);
        let mut s = v.clone(); s.sort(); s.dedup();
        assert_eq!(s.len(), 12, "all distinct");
        assert!(v.iter().all(|x| x.contains("sort a vec of structs by weight")), "core preserved");
        assert_eq!(fuzz("x", 5, 7), fuzz("x", 5, 7), "deterministic for a seed");
    }

    #[test]
    fn garbage_detects_degeneracy() {
        assert!(garbage_score("sort sort sort sort sort sort") > 0.6, "looping repeats = garbage");
        assert!(garbage_score("the the the the the") > 0.6, "low variety = garbage");
        assert!(garbage_score("") > 0.6 && garbage_score("ok") > 0.6, "empty/short = garbage");
        assert!(garbage_score("Use sort_by_key with a closure returning the weight field, descending.") < 0.4, "real answer = clean");
    }

    #[test]
    fn cluster_groups_similar_answers() {
        let outs = vec![
            "use sort_by_key closure weight".to_string(),
            "sort_by_key with a closure on the weight field".to_string(), // ~same idea
            "implement Ord and use BinaryHeap for ordering".to_string(),  // different idea
        ];
        let cs = cluster(&outs, 0.3);
        // the two sort_by_key answers land together; the heap answer is its own cluster
        assert!(cs.iter().any(|c| c.contains(&0) && c.contains(&1)), "similar answers cluster: {cs:?}");
        assert!(cs.iter().any(|c| c == &vec![2]), "the divergent answer is separate: {cs:?}");
    }

    #[test]
    fn consensus_picks_the_majority_clean_answer() {
        let outs = vec![
            "use sort_by_key with a closure returning weight descending".to_string(),
            "sort_by_key closure on weight, reversed for descending order".to_string(),
            "call sort_by_key with the weight closure then reverse".to_string(),
            "garbage garbage garbage garbage garbage".to_string(), // degenerate
            "implement custom Ord then collect into a BinaryHeap".to_string(), // minority idea
        ];
        let c = consensus(&outs, 0.3, 0.6).unwrap();
        assert!(c.cluster.len() >= 3, "the three sort_by_key branches form the majority: {:?}", c.cluster);
        assert!([0, 1, 2].contains(&c.chosen), "chosen from the majority cluster, got {}", c.chosen);
        assert!(!c.cluster.contains(&3), "garbage excluded from the winning cluster");
        assert!(c.agreement >= 0.5, "majority agreement, got {}", c.agreement);
        assert_eq!(c.kept, 4, "one garbage branch filtered");
    }

    #[test]
    fn all_garbage_still_returns_something() {
        let outs = vec!["a a a a a".to_string(), "b b b b b b".to_string()];
        let c = consensus(&outs, 0.3, 0.6).unwrap();
        assert_eq!(c.kept, 0, "nothing survived the filter");
        assert!(c.chosen < 2, "but a least-bad fallback is chosen");
    }
}
