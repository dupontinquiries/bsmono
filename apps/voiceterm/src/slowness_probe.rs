//! v0.1.663: "why so slow" wait classifier (R3) — pure core.
//!
//! Hector's literal complaint: "Why is it so slow? I don't know why." When a
//! foreground command sits there, the kernel knows exactly what it's blocked
//! on — /proc/<pid>/wchan is the kernel function it's sleeping in, and
//! /proc/<pid>/stat gives the process state (R/S/D/Z). This maps those into
//! a human cause ("waiting on disk", "waiting on network", "waiting on a
//! lock"…) so an assist bubble can answer the question instead of leaving
//! him guessing.
//!
//! PURE + testable: the classification is here; the /proc sampling (Linux
//! I/O) is the wiring layer. Surfacing goes through assist_bubble (only when
//! genuinely stuck — see `is_stuck`), never as ambient furniture.

/// What a blocked process is waiting on, derived from its wait-channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitCause {
    /// uninterruptible/interruptible sleep in block I/O — disk/filesystem
    DiskIo,
    /// socket / TCP / network wait
    Network,
    /// futex / mutex — blocked on a lock
    Lock,
    /// reading from a pipe — waiting on another process's output
    Pipe,
    /// waiting on a child process to finish (wait/waitpid)
    ChildProcess,
    /// poll/select/epoll with no timeout pressure — waiting for input/events
    /// (usually NOT "stuck", just idle at a prompt)
    IdleWaiting,
    /// state R — actually running, CPU-bound (not blocked)
    RunningCpu,
    /// not blocked / wchan is 0 / sleeping normally
    NotBlocked,
    /// blocked but the wchan symbol isn't recognized
    UnknownBlocked,
}

/// Classify from the raw wchan symbol and the single-char process state
/// (R running, S sleep, D uninterruptible, Z zombie, T stopped, I idle).
/// `wchan` is the contents of /proc/<pid>/wchan (e.g. "io_schedule",
/// "sk_wait_data", "0", "-").
pub fn classify_wait(wchan: &str, state: char) -> WaitCause {
    let w = wchan.trim();
    // Running on CPU: not waiting on anything.
    if state == 'R' { return WaitCause::RunningCpu; }
    // No wait channel → not meaningfully blocked.
    if w.is_empty() || w == "0" || w == "-" {
        return WaitCause::NotBlocked;
    }
    let lw = w.to_lowercase();
    // Match on substrings of common kernel wait symbols.
    let any = |needles: &[&str]| needles.iter().any(|n| lw.contains(n));

    if any(&["io_schedule", "wait_on_page", "blkdev", "wait_on_buffer",
             "balance_dirty", "wbt_wait", "submit_bio", "jbd2", "ext4",
             "filemap_fault", "folio_wait"]) {
        return WaitCause::DiskIo;
    }
    if any(&["sk_wait", "sock_", "tcp_", "inet_", "skb_", "netlink",
             "recvmsg", "datagram", "unix_stream_read"]) {
        return WaitCause::Network;
    }
    if any(&["futex", "mutex", "rwsem", "down_", "rt_mutex", "sem_",
             "completion"]) {
        return WaitCause::Lock;
    }
    if any(&["pipe_read", "pipe_wait", "pipe_write", "anon_pipe"]) {
        return WaitCause::Pipe;
    }
    if any(&["do_wait", "wait_woken_child", "wait4", "do_signal_stop"]) {
        return WaitCause::ChildProcess;
    }
    // timer/interval sleeps are INTENTIONAL waits, not stalls — treat as
    // idle (v0.1.663: real /proc probe showed `sleep` → hrtimer_nanosleep,
    // which must not look "stuck").
    if any(&["hrtimer", "nanosleep", "schedule_timeout", "msleep",
             "do_nanosleep", "clock_nanosleep"]) {
        return WaitCause::IdleWaiting;
    }
    if any(&["poll_schedule", "do_select", "ep_poll", "do_poll",
             "read_events", "wait_woken", "pause"]) {
        return WaitCause::IdleWaiting;
    }
    WaitCause::UnknownBlocked
}

/// A short, plain-language cause for a bubble. No kernel jargon.
pub fn cause_label(c: WaitCause) -> &'static str {
    match c {
        WaitCause::DiskIo => "waiting on disk",
        WaitCause::Network => "waiting on the network",
        WaitCause::Lock => "waiting on a lock",
        WaitCause::Pipe => "waiting on another process",
        WaitCause::ChildProcess => "waiting on a child process",
        WaitCause::IdleWaiting => "waiting for input",
        WaitCause::RunningCpu => "busy on the CPU",
        WaitCause::NotBlocked => "running normally",
        WaitCause::UnknownBlocked => "blocked (cause unknown)",
    }
}

/// Whether a wait of `elapsed_secs` with this cause is worth surfacing as a
/// "why so slow" bubble. The point is to answer a REAL stall — not to nag
/// at idle prompts or brief blocks. Rules:
///  - IdleWaiting / NotBlocked never surface (a prompt waiting for input is
///    not "slow").
///  - RunningCpu surfaces only on a LONG burn (it's expected to be busy).
///  - genuine blocks (disk/network/lock/pipe/child/unknown) surface after a
///    short threshold.
pub fn is_stuck(c: WaitCause, elapsed_secs: u64) -> bool {
    match c {
        WaitCause::IdleWaiting | WaitCause::NotBlocked => false,
        WaitCause::RunningCpu => elapsed_secs >= 30,
        WaitCause::DiskIo | WaitCause::Network | WaitCause::Lock
        | WaitCause::Pipe | WaitCause::ChildProcess
        | WaitCause::UnknownBlocked => elapsed_secs >= 3,
    }
}

/// Build the full bubble text: cause + how long. e.g. "waiting on the
/// network · 4s". The assist-bubble kind for dedup is cause-based so the
/// same stall doesn't re-fire.
pub fn stuck_message(c: WaitCause, elapsed_secs: u64) -> String {
    format!("{} · {}s", cause_label(c), elapsed_secs)
}

/// The assist-bubble `kind` string for a cause (so the scheduler dedups by
/// cause, not by exact message).
pub fn bubble_kind(c: WaitCause) -> &'static str {
    match c {
        WaitCause::DiskIo => "slow:disk",
        WaitCause::Network => "slow:net",
        WaitCause::Lock => "slow:lock",
        WaitCause::Pipe => "slow:pipe",
        WaitCause::ChildProcess => "slow:child",
        WaitCause::RunningCpu => "slow:cpu",
        WaitCause::UnknownBlocked => "slow:unknown",
        WaitCause::IdleWaiting | WaitCause::NotBlocked => "slow:none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_is_cpu() {
        assert_eq!(classify_wait("anything", 'R'), WaitCause::RunningCpu);
    }

    #[test]
    fn zero_wchan_not_blocked() {
        assert_eq!(classify_wait("0", 'S'), WaitCause::NotBlocked);
        assert_eq!(classify_wait("-", 'S'), WaitCause::NotBlocked);
        assert_eq!(classify_wait("", 'S'), WaitCause::NotBlocked);
    }

    #[test]
    fn disk_io_symbols() {
        assert_eq!(classify_wait("io_schedule", 'D'), WaitCause::DiskIo);
        assert_eq!(classify_wait("wait_on_page_bit", 'D'), WaitCause::DiskIo);
        assert_eq!(classify_wait("jbd2_log_wait_commit", 'D'), WaitCause::DiskIo);
    }

    #[test]
    fn network_symbols() {
        assert_eq!(classify_wait("sk_wait_data", 'S'), WaitCause::Network);
        assert_eq!(classify_wait("tcp_recvmsg", 'S'), WaitCause::Network);
        assert_eq!(classify_wait("inet_csk_accept", 'S'), WaitCause::Network);
    }

    #[test]
    fn lock_symbols() {
        assert_eq!(classify_wait("futex_wait_queue_me", 'S'), WaitCause::Lock);
        assert_eq!(classify_wait("rwsem_down_read", 'S'), WaitCause::Lock);
    }

    #[test]
    fn pipe_and_child() {
        assert_eq!(classify_wait("pipe_read", 'S'), WaitCause::Pipe);
        assert_eq!(classify_wait("do_wait", 'S'), WaitCause::ChildProcess);
    }

    #[test]
    fn real_proc_symbols_v663() {
        // found by probing real processes in the sandbox:
        // a timer sleep is intentional, NOT a stall
        assert_eq!(classify_wait("hrtimer_nanosleep", 'S'), WaitCause::IdleWaiting);
        assert!(!is_stuck(classify_wait("hrtimer_nanosleep", 'S'), 9999));
        // modern pipe-read symbol
        assert_eq!(classify_wait("anon_pipe_read", 'S'), WaitCause::Pipe);
    }

    #[test]
    fn idle_poll_is_not_stuck() {
        let c = classify_wait("poll_schedule_timeout", 'S');
        assert_eq!(c, WaitCause::IdleWaiting);
        assert!(!is_stuck(c, 9999), "a prompt polling for input is never 'slow'");
    }

    #[test]
    fn is_stuck_thresholds() {
        assert!(!is_stuck(WaitCause::Network, 1));   // brief block: not yet
        assert!(is_stuck(WaitCause::Network, 3));     // 3s+: surface
        assert!(!is_stuck(WaitCause::RunningCpu, 10)); // short burn: expected
        assert!(is_stuck(WaitCause::RunningCpu, 30));  // long burn: surface
        assert!(!is_stuck(WaitCause::NotBlocked, 9999));
    }

    #[test]
    fn messages_have_no_jargon() {
        // the user sees a cause, never a kernel symbol
        let m = stuck_message(classify_wait("io_schedule", 'D'), 4);
        assert_eq!(m, "waiting on disk · 4s");
        assert!(!m.contains("io_schedule"));
    }

    #[test]
    fn bubble_kind_dedups_by_cause() {
        // two disk stalls share a kind → the scheduler can dedup them
        assert_eq!(bubble_kind(classify_wait("io_schedule", 'D')),
                   bubble_kind(classify_wait("wait_on_page_bit", 'D')));
    }
}
