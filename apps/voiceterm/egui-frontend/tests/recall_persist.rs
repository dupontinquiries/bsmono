//! Recall must survive a restart. The pieces are unit-tested in `recall.rs`; this drives the
//! round trip the app actually performs — serialize what a session saw, read it back in a
//! fresh process's shape, merge with new live blocks, and rank over the union.
//!
//! The failure this guards is the one the feature exists for: after a reboot, "how did I fix
//! that" finding nothing because the ranking only ever saw the current session.

use voiceterm_egui::cmd_sheet::recall;

fn item(cmd: &str, at: u64) -> recall::RecallItem {
    recall::RecallItem {
        command: cmd.to_string(),
        output_snippet: String::new(),
        cwd: Some("/home/k/proj".to_string()),
        started_at: at,
        exit_code: Some(0),
    }
}

#[test]
fn a_command_from_a_previous_session_is_still_recallable() {
    // Session one ran something memorable and wrote its history out.
    let session_one = vec![item("systemctl restart nginx", 1_000), item("ls", 1_010)];
    let on_disk = recall::serialize_history(&session_one);
    assert!(on_disk.contains("nginx"), "{on_disk}");

    // Session two: a fresh process, different live blocks, reads that file back.
    let persisted = recall::parse_history(&on_disk);
    assert_eq!(persisted.len(), 2, "the file did not round trip: {persisted:?}");
    let live = vec![item("cargo build", 2_000)];
    let merged = recall::merge_history(persisted, &live);

    // The old command is in the union and findable.
    let hits = recall::recall("nginx", &merged, 2_100, 40);
    assert!(
        hits.iter().any(|h| h.command.contains("nginx")),
        "a command from the previous session was not recalled: {:?}",
        hits.iter().map(|h| &h.command).collect::<Vec<_>>()
    );
    // And the live one is too — a merge that lost either half would be worse than no merge.
    assert!(recall::recall("cargo", &merged, 2_100, 40).iter().any(|h| h.command.contains("cargo")));
}

#[test]
fn re_running_a_command_does_not_duplicate_it_and_history_stays_bounded() {
    // The same command, same start time, seen live AND on disk: one entry, not two. Without
    // the dedup, every query would grow the file by the size of the live session.
    let one = vec![item("git status", 500)];
    let persisted = recall::parse_history(&recall::serialize_history(&one));
    let merged = recall::merge_history(persisted, &one);
    assert_eq!(merged.len(), 1, "a re-seen command was duplicated: {merged:?}");

    // The ring cap holds, and keeps the NEWEST — a history that dropped recent commands to
    // keep ancient ones would answer "what did I just do" with last month.
    let many: Vec<recall::RecallItem> =
        (0..recall::MAX_PERSISTED as u64 + 50).map(|i| item(&format!("cmd{i}"), i)).collect();
    let capped = recall::merge_history(many, &[]);
    assert_eq!(capped.len(), recall::MAX_PERSISTED);
    assert!(
        capped.iter().any(|i| i.command == format!("cmd{}", recall::MAX_PERSISTED as u64 + 49)),
        "the newest command was dropped"
    );
    assert!(!capped.iter().any(|i| i.command == "cmd0"), "the oldest survived the cap");
}

/// Commands contain tabs and newlines. A history format that loses them silently corrupts
/// the row after the one that had them.
#[test]
fn awkward_commands_survive_the_file() {
    let nasty = vec![
        item("grep -P '\\t' file", 10),
        item("printf 'a\\nb'", 20),
        item("echo done", 30),
    ];
    let back = recall::parse_history(&recall::serialize_history(&nasty));
    assert_eq!(back.len(), 3, "a tab or newline ate a row: {back:?}");
    assert_eq!(back.iter().filter(|i| i.command == "echo done").count(), 1);
}

/// A command started after the shell announced a new directory is stamped with THAT
/// directory — which is the whole point of reading OSC 7, and the thing that was broken
/// while `set_cwd` was a hardcoded demo path.
///
/// This replaces a test I wrote for an ordering bug that does not exist. I had assumed
/// `feed_line` opened blocks; it only CLOSES them, and `submit_command` opens one at Enter,
/// by which time the prompt's announcement has long since arrived. The first version passed
/// while asserting nothing, and the guard I added against exactly that is what caught it.
#[test]
fn a_command_is_stamped_with_the_directory_the_shell_announced() {
    use uikit_term::blocks::BlockTracker;
    use voiceterm_egui::osc7;

    let mut t = BlockTracker::new();
    let mut sc = osc7::Scanner::new();

    // Where we started.
    t.set_cwd("/home/k/old");
    t.submit_command("ls", 0);
    t.finish(Some(0));

    // The shell cds and announces it at the next prompt.
    if let Some(cwd) = sc.feed(b"\x1b]7;file:///srv/work\x07user@host:/srv/work$ ").pop() {
        t.set_cwd(&cwd);
    }
    t.submit_command("make", 1);
    t.finish(Some(0));

    let stamped: Vec<Option<String>> = t.blocks.iter().map(|b| b.cwd.clone()).collect();
    assert_eq!(stamped.len(), 2, "{stamped:?}");
    assert_eq!(stamped[0].as_deref(), Some("/home/k/old"), "the first block moved with us");
    assert_eq!(stamped[1].as_deref(), Some("/srv/work"), "the announcement did not reach the block");

    // And `previous_cwd` tracked the move, which is what smart-cd undo reads.
    assert_eq!(t.previous_cwd.as_deref(), Some("/home/k/old"));
}
