//! session_cluster — what this session is mostly about, from how its commands group.
//!
//! `TODO_command_clustering.md` ends with: *"REMAINING (QML): a 'task clusters' view / branch
//! label surfacing; and feeding the dominant cluster label into session_autoname. Backend
//! ready."* The backend is `cmd_cluster` — featurize, k-means, `cluster_labels` — measured to
//! 1.00 purity on the planted set after the behavioural representation landed, and wired into
//! the Qt build in v0.1.720. It was never mounted in this frontend at all.
//!
//! This is the small piece between the two: run the pipeline over a session's blocks and say
//! which label describes most of it.
//!
//! ## Why a dominant label rather than all of them
//!
//! `session_name` produces one line for the locator. A list of cluster labels there would be
//! longer than the name it decorates, and the question the locator answers is "what is this
//! session" — singular.
//!
//! ## Why k is derived, and bounded
//!
//! The Qt method uses `~sqrt(n)` clamped to 2..=6, and this keeps that so the two builds
//! agree about what a session's clusters are. Below two there is nothing to cluster; past six
//! the labels are finer than a one-line name can use, and a session of forty commands split
//! forty ways is not a description of anything.

use crate::cmd_cluster;

/// Fewest commands worth clustering.
///
/// Under this, k-means is describing noise: two commands always split into two clusters and
/// the "dominant" one is a coin toss. A session this short is better named by what it ran.
pub const MIN_COMMANDS: usize = 6;

/// The k bounds the Qt build uses, kept so the two agree.
pub const K_MIN: usize = 2;
pub const K_MAX: usize = 6;

/// `~sqrt(n)`, clamped.
pub fn k_for(n: usize) -> usize {
    ((n as f64).sqrt().round() as usize).clamp(K_MIN, K_MAX)
}

/// The label of the largest cluster, and how much of the session it covers.
///
/// `None` when there is not enough to say — see [`MIN_COMMANDS`]. A caller showing this is
/// expected to say nothing rather than show a guess.
pub fn dominant(commands: &[(String, String)], seed: u64) -> Option<(String, f32)> {
    if commands.len() < MIN_COMMANDS {
        return None;
    }
    let k = k_for(commands.len());
    let points: Vec<_> = commands.iter().map(|(c, d)| cmd_cluster::featurize(c, d)).collect();
    let assign = cmd_cluster::kmeans(&points, k, seed);
    let labels = cmd_cluster::cluster_labels(commands, &assign, k);

    let mut counts = vec![0usize; k];
    for a in &assign {
        if let Some(c) = counts.get_mut(*a) {
            *c += 1;
        }
    }
    let (best, n) = counts.iter().enumerate().max_by_key(|(_, n)| **n)?;
    if *n == 0 {
        return None;
    }
    let label = labels.get(best)?.clone();
    // A label that is just the cluster's index tells the reader nothing — `cluster_labels`
    // falls back to "cluster3" when a cluster has no distinctive token, and putting that in a
    // session name is worse than putting nothing.
    if label.starts_with("cluster") && label[7..].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((label, *n as f32 / commands.len() as f32))
}

/// The dominant label, but only when it covers enough of the session to be a description of
/// it.
///
/// A label covering a fifth of what you did is not what the session is about, and a locator
/// that claims it is teaches the reader to ignore the locator.
pub fn describes(commands: &[(String, String)], seed: u64, min_share: f32) -> Option<String> {
    dominant(commands, seed).filter(|(_, share)| *share >= min_share).map(|(l, _)| l)
}

/// The share a label needs before it names the session.
pub const MIN_SHARE: f32 = 0.4;

#[cfg(test)]
mod tests {
    use super::*;

    fn cmds(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(c, d)| (c.to_string(), d.to_string())).collect()
    }

    /// The end-to-end claim the TODO leaves open: a session's commands produce a label that
    /// describes it. The measured win was the behavioural prior — git commands group as vcs
    /// however differently they are spelled.
    #[test]
    fn a_session_of_mostly_git_is_named_for_it() {
        let c = cmds(&[
            ("git status", "/p"),
            ("git add -A", "/p"),
            ("git commit -m x", "/p"),
            ("git push", "/p"),
            ("git log --oneline", "/p"),
            ("git diff", "/p"),
            ("cargo build", "/p"),
        ]);
        let (label, share) = dominant(&c, 7).expect("a dominant cluster");
        assert!(share > 0.0 && share <= 1.0, "{share}");

        // The label comes from the DOMINANT CLUSTER'S VOCABULARY, and that is not the same
        // as the common verb. `cluster_labels` scores by DISTINCTIVENESS — in-cluster count
        // over overall count — so a cluster of git commands is labelled by what separates it
        // from the others ("diff", "status"), not by "git", which every one of them shares.
        //
        // I asserted `label.contains("git")` first and it returned "diff". That is the
        // pipeline working as measured, not a fault: the whole point of the distinctiveness
        // score is to pick the token that TELLS THE CLUSTERS APART. Worth writing down,
        // because a name like "diff" in the locator looks wrong until you know why.
        let git_vocab = ["git", "status", "add", "commit", "push", "log", "diff", "oneline"];
        assert!(
            git_vocab.contains(&label.as_str()),
            "the label should come from the dominant cluster's vocabulary, got {label:?}"
        );
    }

    /// Under six commands, k-means is describing noise: two commands always split into two
    /// clusters and the "dominant" one is a coin toss.
    #[test]
    fn too_few_commands_says_nothing_rather_than_guessing() {
        assert_eq!(dominant(&cmds(&[("ls", "/p")]), 1), None);
        assert_eq!(dominant(&cmds(&[("ls", "/p"), ("cd x", "/p")]), 1), None);
        assert_eq!(dominant(&[], 1), None);
        assert!(dominant(&cmds(&[("ls", "/p"); MIN_COMMANDS]), 1).is_some(), "at the floor it speaks");
    }

    /// `~sqrt(n)` clamped to 2..=6, the Qt build's rule, kept so the two agree about what a
    /// session's clusters are.
    #[test]
    fn k_follows_the_qt_builds_rule() {
        assert_eq!(k_for(0), K_MIN);
        assert_eq!(k_for(4), K_MIN, "sqrt(4)=2");
        assert_eq!(k_for(9), 3);
        assert_eq!(k_for(1000), K_MAX, "clamped, or the labels are finer than a name can use");
    }

    /// A label covering a fifth of what you did is not what the session is about, and a
    /// locator claiming it is teaches the reader to ignore the locator.
    #[test]
    fn a_thin_majority_does_not_get_to_name_the_session() {
        // Six different behaviours: no cluster covers much.
        let c = cmds(&[
            ("git status", "/p"),
            ("cargo build", "/p"),
            ("ssh box", "/p"),
            ("ls -la", "/p"),
            ("python x.py", "/p"),
            ("curl example.com", "/p"),
            ("make", "/p"),
            ("vim a.rs", "/p"),
        ]);
        // Whatever it picks, an unreasonable share bar refuses it.
        assert_eq!(describes(&c, 3, 0.9), None);
        // And the same call with a floor of nothing returns whatever dominant found, so the
        // filter is what refused rather than the pipeline failing.
        assert_eq!(describes(&c, 3, 0.0).is_some(), dominant(&c, 3).is_some());
    }
}
