// gitmind-core/src/maintenance.rs
//
// Borrowed performance wins from git itself. We deliberately reuse git's
// decades of tuning rather than reinvent it, and these are the techniques
// that map cleanly onto how *we* use git:
//
//   - commit-graph (--reachable): precomputed generation numbers make
//     ancestry/topo walks O(useful) instead of O(history). Our checkpoint
//     timeline and `git log` views ride directly on this.
//   - changed-path Bloom filters (--changed-paths): path-limited log gets
//     dramatically faster, which is exactly what query.rs does when it
//     filters history by language (a pathspec). Free speedup for search.
//   - packed-refs (pack-refs --all): we create a *ref per checkpoint and
//     per review*. Left loose that's one inode per snapshot — death by a
//     thousand files. Packing them is essential, not optional, for us.
//   - repack: fold loose objects into a delta-compressed pack.
//
// What we DON'T borrow and why (the honest part): git's pack delta-format
// itself — we use content-defined chunking + zstd in the chunk store for
// large/binary files, which git's pack handles poorly. And sparse-index /
// partial-clone are checkout-scaling features that belong to the frontend
// phase, not the core.
//
// Everything here is a thin wrapper over real git subcommands so we
// inherit git's correctness and future improvements for free.

use crate::backend::{GitCli, RepoBackend};
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaintenanceReport {
    pub commit_graph: bool,
    pub changed_paths: bool,
    pub refs_packed: bool,
    pub repacked: bool,
}

pub struct Maintenance<'a> {
    repo: &'a GitCli,
}

impl<'a> Maintenance<'a> {
    pub fn new(repo: &'a GitCli) -> Self {
        Maintenance { repo }
    }

    /// Write the commit-graph with generation numbers and changed-path
    /// Bloom filters. Accelerates timeline walks and language-filtered
    /// search (query.rs).
    pub fn write_commit_graph(&self) -> Result<bool, String> {
        self.repo
            .git(&["commit-graph", "write", "--reachable", "--changed-paths"])?;
        Ok(self.has_commit_graph())
    }

    /// Pack all loose refs into packed-refs. Critical for us because each
    /// checkpoint/review is a ref.
    pub fn pack_refs(&self) -> Result<bool, String> {
        self.repo.git(&["pack-refs", "--all"])?;
        Ok(self.repo.root().join(".git").join("packed-refs").exists())
    }

    /// Delta-compress loose objects into a pack and drop the loose copies.
    pub fn repack(&self) -> Result<(), String> {
        self.repo.git(&["repack", "-d"]).map(|_| ())
    }

    /// Run the full maintenance pass. Safe to run often; git makes each
    /// step incremental.
    pub fn run_all(&self) -> Result<MaintenanceReport, String> {
        let commit_graph = self.write_commit_graph().unwrap_or(false);
        let refs_packed = self.pack_refs().unwrap_or(false);
        let repacked = self.repack().is_ok();
        Ok(MaintenanceReport {
            commit_graph,
            changed_paths: commit_graph, // written in the same pass
            refs_packed,
            repacked,
        })
    }

    fn has_commit_graph(&self) -> bool {
        let info = self.repo.root().join(".git").join("objects").join("info");
        info.join("commit-graph").exists() || info.join("commit-graphs").is_dir()
    }
}

/// Convenience for callers that only have a path.
pub fn commit_graph_path(repo_root: &Path) -> std::path::PathBuf {
    repo_root.join(".git").join("objects").join("info").join("commit-graph")
}
