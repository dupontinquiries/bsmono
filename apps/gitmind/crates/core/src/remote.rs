// gitmind-core/src/remote.rs
//
// The seam for backups and cross-device sync. Everything that can hold a
// copy of the repo's data — a local backup folder, a LAN peer, a cloud
// drive, or the future DHT — implements one trait. Frontends and the
// backup scheduler talk to the trait, never to a specific provider, so
// adding the DHT later is a new impl, not a rewrite.
//
// What travels over a RemoteTarget is the chunk store + the ref set
// (real refs and refs/checkpoints/*). Because the store is already
// content-addressed, dedup, encryption, and resumable transfer all fall
// out naturally: we sync the set of chunk Oids the target is missing.

use crate::oid::Oid;

/// A place that can store and return chunks + refs. Implementations:
///   - LocalFolder   : a "backup folder" on another disk / mount.
///   - RcloneRemote  : anything rclone speaks (S3, B2, Dropbox, Google
///                     Drive, and Proton Drive via its community backend).
///   - LanPeer       : a trusted device on the same network (QUIC).
///   - DhtTarget     : (future) content-addressed fetch over your DHT.
pub trait RemoteTarget {
    /// Stable id for the connection manager UI + dedup of targets.
    fn id(&self) -> &str;

    /// Which of these chunk Oids the target does NOT already have. Lets a
    /// backup push only the delta — the dedup win extends across devices.
    fn missing(&self, want: &[Oid]) -> Result<Vec<Oid>, String>;

    /// Push one chunk's bytes (already zstd-compressed, optionally
    /// encrypted by the caller). Idempotent: pushing an existing chunk is
    /// a no-op so retries are safe.
    fn put_chunk(&self, oid: Oid, bytes: &[u8]) -> Result<(), String>;

    /// Fetch one chunk's bytes for a restore/rollback.
    fn get_chunk(&self, oid: Oid) -> Result<Vec<u8>, String>;

    /// Publish a ref (e.g. refs/checkpoints/main -> commit oid) so a peer
    /// can reconstruct the timeline, not just loose chunks.
    fn put_ref(&self, name: &str, target: &str) -> Result<(), String>;

    /// List published refs for reconstruction.
    fn list_refs(&self) -> Result<Vec<(String, String)>, String>;
}

/// How bytes are protected before they leave the device. The default for
/// any non-local target is Encrypted: backups to a peer or cloud are
/// end-to-end encrypted, so a trusted device stores ciphertext it cannot
/// read without the shared key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protection {
    /// Local backup folder on hardware you control — plaintext chunks
    /// (still zstd-compressed). Cheapest; fine for same-machine rollback.
    Plain,
    /// End-to-end encrypted with a per-repo key (age/X25519). Use for any
    /// off-device target: LAN peer, cloud, DHT.
    Encrypted,
}

/// Identity for a trusted device in the sync mesh. Key exchange pairs two
/// devices once (out-of-band: BLE proximity or a shown code); thereafter
/// bulk transfer runs over WiFi LAN. BLE is pairing/discovery only — its
/// throughput is far too low for media sync.
#[derive(Clone, Debug)]
pub struct DeviceIdentity {
    pub name: String,
    /// X25519 public key, hex. The private half never leaves the device.
    pub pubkey_hex: String,
}

// Concrete impls (LocalFolder, RcloneRemote, LanPeer, DhtTarget) land in
// the backup/sync phase; this file fixes the contract they share. See
// BACKUP_SYNC.md for the transport, encryption, and rollback design.

// ── External-tool command builders ──────────────────────────────────────
// We shell out to rclone (cloud, incl. Proton Drive) and rsync/ssh (LAN
// hosts) rather than reimplementing transports. The command CONSTRUCTION
// is pure and unit-tested here; the thin exec layer runs these argv lists.
// Building argv as a Vec (never a shell string) avoids quoting/injection
// bugs entirely.

/// `rclone copy <local> <remote>:<path>` — push a directory (the backup
/// folder / chunk store mirror) to any rclone backend.
pub fn rclone_copy_cmd(local: &str, remote_name: &str, remote_path: &str) -> Vec<String> {
    vec![
        "rclone".into(),
        "copy".into(),
        "--transfers".into(),
        "8".into(),
        local.into(),
        format!("{remote_name}:{remote_path}"),
    ]
}

/// `rsync -az --delete <local>/ <ssh_dest>` — mirror to a host over SSH.
/// Trailing slash on source so contents (not the dir) are synced.
pub fn rsync_push_cmd(local: &str, ssh_dest: &str) -> Vec<String> {
    let src = if local.ends_with('/') { local.to_string() } else { format!("{local}/") };
    vec!["rsync".into(), "-az".into(), "--delete".into(), src, ssh_dest.into()]
}

/// Auto-distribute selected branches to a host over SSH after each commit
/// (the "commit backend pushes branch X to host Y" feature). Wraps a plain
/// `git push` to an ssh:// URL — no daemon needed on the far side, just
/// sshd + a bare repo.
#[derive(Clone, Debug)]
pub struct SshDistributor {
    /// e.g. "ssh://build@gpu-box/srv/repos/proj.git"
    pub ssh_url: String,
    /// Branches to mirror automatically (others stay local).
    pub branches: Vec<String>,
}

impl SshDistributor {
    /// argv to push one branch (fast-forward only by default; the policy
    /// layer decides whether to force). Run this from the repo via `git`.
    pub fn push_cmd(&self, branch: &str) -> Vec<String> {
        vec!["push".into(), self.ssh_url.clone(), format!("{branch}:{branch}")]
    }

    /// Which configured branches should fire on this commit's branch.
    pub fn should_distribute(&self, current_branch: &str) -> bool {
        self.branches.iter().any(|b| b == current_branch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rclone_cmd_shape() {
        let c = rclone_copy_cmd("/repo/.git/gitmind", "proton", "backups/proj");
        assert_eq!(c[0], "rclone");
        assert_eq!(c.last().unwrap(), "proton:backups/proj");
    }

    #[test]
    fn rsync_adds_trailing_slash() {
        let c = rsync_push_cmd("/repo/store", "build@host:/srv/mirror");
        assert!(c.contains(&"/repo/store/".to_string()));
        assert_eq!(c.last().unwrap(), "build@host:/srv/mirror");
    }

    #[test]
    fn ssh_distributor_selects_branches() {
        let d = SshDistributor {
            ssh_url: "ssh://b@box/srv/p.git".into(),
            branches: vec!["main".into(), "experiment/gpu".into()],
        };
        assert!(d.should_distribute("main"));
        assert!(!d.should_distribute("scratch"));
        assert_eq!(
            d.push_cmd("main"),
            vec!["push", "ssh://b@box/srv/p.git", "main:main"]
        );
    }
}
