//! mail_store — mail off disk, without a terminal in the way.
//!
//! `mail_rfc822` gave the mail views a data source: email-shaped command
//! output. That works and it is indirect — you have to `cat` something
//! before the inbox has anything in it. This reads the same files the
//! terminal would have printed.
//!
//! Two formats, because those are the two things on a Linux machine that
//! hold mail:
//!
//! * **Maildir** — a directory with `cur/`, `new/` and `tmp/`, one message
//!   per file. `tmp/` is SKIPPED: it holds messages mid-delivery, and
//!   reading one gives you half a mail that a redraw later replaces with
//!   the whole thing.
//! * **mbox** — a single file of messages separated by `From ` lines.
//!
//! ## Where it looks, and why it asks first
//!
//! `$MAIL`, then `~/Maildir`, then `~/mail`, then `/var/mail/$USER`. That
//! order is `$MAIL` first because it is the variable that exists to answer
//! exactly this question, and a machine that sets it has an opinion worth
//! respecting over any guess.
//!
//! ## Bounded, because a mail spool is not a small file
//!
//! A Maildir with forty thousand messages in it is ordinary, and reading
//! all of them on the frame loop would stall the UI for seconds and hold
//! the lot in memory. [`MAX_MESSAGES`] caps it and the NEWEST are kept —
//! by file mtime, which is what a mail reader shows first and the only
//! ordering available before parsing. A cap that kept the oldest would
//! show a decade-old inbox.
//!
//! ## What this does not do
//!
//! No watching, no incremental read, no writing. A rescan is a call, and
//! the caller decides when — on start, and when the person asks. An inotify
//! watch is the right answer for a spool that changes while you look at it,
//! and it is a thread and a platform dependency for a refresh button's
//! worth of value.

use std::path::{Path, PathBuf};

use crate::mail_view::Message;

/// The most messages read in one scan. Ten thousand parses is well under a
/// frame's budget on a phone; forty thousand is not, and neither is holding
/// them.
pub const MAX_MESSAGES: usize = 10_000;

/// Refuse a single file larger than this. A mail with a 200 MB attachment
/// inlined is not something this reader should pull into memory to display
/// a subject line for.
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// What was found at a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Maildir,
    Mbox,
    /// Exists but is neither — a directory with no `cur`, an empty file.
    Unknown,
}

/// Classify a path without reading it.
///
/// A Maildir is a directory containing `cur` (the spec also wants `new` and
/// `tmp`, and plenty of real ones are missing `tmp`). Requiring all three
/// would reject working Maildirs; requiring `cur` alone would call any
/// directory with a `cur` subdirectory a mail store, and that is rare
/// enough to accept.
pub fn classify(path: &Path) -> Kind {
    if path.is_dir() {
        if path.join("cur").is_dir() { Kind::Maildir } else { Kind::Unknown }
    } else if path.is_file() {
        Kind::Mbox
    } else {
        Kind::Unknown
    }
}

/// The candidates, in order.
///
/// `$MAIL` first: it is the variable that exists to answer this question,
/// and a machine that sets it has an opinion worth more than a guess.
pub fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(m) = std::env::var("MAIL") {
        if !m.trim().is_empty() { out.push(PathBuf::from(m)); }
    }
    if let Ok(home) = std::env::var("HOME") {
        out.push(Path::new(&home).join("Maildir"));
        out.push(Path::new(&home).join("mail"));
    }
    if let Ok(user) = std::env::var("USER") {
        out.push(Path::new("/var/mail").join(user));
    }
    out
}

/// Why the mail pane is empty, in the user's terms.
///
/// "no mail" is what the list said, and it is indistinguishable from "this feature is
/// broken": nothing on screen said that a maildir was looked for, where, or that a REMOTE
/// account is a different thing this build cannot reach. Someone with mail in
/// `~/.local/share/mail` had no way to learn that only four paths are tried.
///
/// `None` when a store WAS found — then an empty list means an empty inbox, which needs no
/// explanation.
pub fn empty_reason() -> Option<String> {
    if discover().is_some() {
        return None;
    }
    let looked = candidates();
    if looked.is_empty() {
        // No HOME, no USER, no MAIL. Almost certainly a service or a container.
        return Some(
            "No mail store: neither $MAIL nor $HOME is set, so there was nowhere to look."
                .to_string(),
        );
    }
    let list: Vec<String> = looked.iter().map(|p| p.display().to_string()).collect();
    Some(format!(
        "No local mail store. Looked in: {}.\nSet $MAIL to a maildir or mbox to read it here, \
         or sign in to Proton above.",
        list.join(", ")
    ))
}

/// The first candidate that holds mail, if any.
pub fn discover() -> Option<(PathBuf, Kind)> {
    candidates().into_iter()
        .map(|p| { let k = classify(&p); (p, k) })
        .find(|(_, k)| *k != Kind::Unknown)
}

/// Newest-first file list for a Maildir's `cur` and `new`.
///
/// mtime, because it is the only ordering available before parsing and it
/// is what the cap has to be applied against — sorting after reading would
/// mean reading everything first, which is the cost the cap exists to
/// avoid.
fn maildir_files(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    // `tmp/` deliberately absent: it holds messages mid-delivery, and
    // reading one gives half a mail that a later scan silently replaces.
    for sub in ["cur", "new"] {
        let Ok(rd) = std::fs::read_dir(root.join(sub)) else { continue };
        for e in rd.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
                continue;
            }
            let t = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            files.push((t, e.path()));
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.truncate(MAX_MESSAGES);
    files.into_iter().map(|(_, p)| p).collect()
}

/// Read a mail store into messages.
///
/// Unreadable files are SKIPPED rather than failing the scan: one message
/// with bad permissions must not empty the inbox, and a spool always has
/// something odd in it.
pub fn load(path: &Path) -> Vec<Message> {
    match classify(path) {
        Kind::Maildir => {
            let mut out = Vec::new();
            // The ids each message claims, collected alongside so threads
            // can be merged once the whole directory is read. One file at
            // a time cannot know that two of them belong together.
            let mut chains: Vec<Vec<String>> = Vec::new();
            for f in maildir_files(path) {
                let Ok(text) = std::fs::read_to_string(&f) else { continue };
                // The file name is the origin, so two messages that both
                // lack a Message-ID stay distinct.
                let origin = f.file_name().map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "maildir".to_string());
                // One file is one message, so mbox splitting must NOT be
                // applied. I wrote this the other way first and the test
                // showed it is worse than the tearing I expected: a body
                // line beginning "From " is a separator, `split_mbox` keeps
                // only what FOLLOWS one, so the headers were dropped and
                // what survived was the quoted tail with no subject and no
                // sender. Quoted mail is full of such lines.
                chains.push({
                    let mut ids = crate::mail_rfc822::thread_ids(&text);
                    if ids.is_empty() { ids.push(origin.clone()); }
                    ids
                });
                out.push(crate::mail_rfc822::message_from_text(&origin, &text));
                if out.len() >= MAX_MESSAGES { break; }
            }
            crate::mail_rfc822::regroup(&mut out, &chains);
            out
        }
        Kind::Mbox => {
            let Ok(meta) = std::fs::metadata(path) else { return Vec::new() };
            if meta.len() > MAX_FILE_BYTES {
                return Vec::new();
            }
            let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
            let origin = path.file_name().map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "mbox".to_string());
            let mut out = crate::mail_rfc822::messages_from_text(&origin, &text);
            // Newest first, matching the Maildir path — a list whose order
            // depends on which format the person happens to use is a list
            // that looks broken on one of them.
            out.sort_by(|a, b| b.time.cmp(&a.time));
            out.truncate(MAX_MESSAGES);
            out
        }
        Kind::Unknown => Vec::new(),
    }
}

/// The same traversal as [`load`], but returns body text keyed by message
/// id instead of metadata — the lazy-fetch half a reader calls on open,
/// not the list a caller shows for every message.
///
/// A genuinely separate pass, not `load` with an extra field bolted on:
/// bodies are the reason this reads every file's FULL text at all
/// (`load` needs only the headers a `Message` carries), and keeping the
/// two apart means listing a mailbox never pays for parsing bodies it
/// isn't about to show. Mirrors `load`'s Maildir/mbox branches exactly,
/// including the tmp/-is-skipped and size-cap rules, so the two never
/// disagree about which files exist to read.
pub fn load_bodies(path: &Path) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    match classify(path) {
        Kind::Maildir => {
            for f in maildir_files(path) {
                let Ok(text) = std::fs::read_to_string(&f) else { continue };
                let origin = f.file_name().map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "maildir".to_string());
                let id = crate::mail_rfc822::message_from_text(&origin, &text).id;
                out.insert(id, crate::mail_rfc822::body_from_text(&text));
                if out.len() >= MAX_MESSAGES { break; }
            }
        }
        Kind::Mbox => {
            let Ok(meta) = std::fs::metadata(path) else { return out };
            if meta.len() > MAX_FILE_BYTES {
                return out;
            }
            let Ok(text) = std::fs::read_to_string(path) else { return out };
            let origin = path.file_name().map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "mbox".to_string());
            // `#{i}`, matching `messages_from_text`'s own origin exactly —
            // that function suffixes each split part with its index for
            // the SAME reason a Maildir file's whole name is the origin:
            // a message with no Message-ID falls back to `origin` as its
            // id, and multiple such messages in one mbox file need to stay
            // distinct. Using a bare, shared origin here (an earlier draft
            // of this function did) would silently collide their bodies —
            // whichever no-id message loaded last would overwrite the rest
            // in the map, and every one of them would then show the WRONG
            // body when opened. Caught before this shipped, not after.
            for (i, raw) in crate::mail_rfc822::split_mbox(&text).iter().enumerate() {
                let part_origin = format!("{origin}#{i}");
                let id = crate::mail_rfc822::message_from_text(&part_origin, raw).id;
                out.insert(id, crate::mail_rfc822::body_from_text(raw));
                if out.len() >= MAX_MESSAGES { break; }
            }
        }
        Kind::Unknown => {}
    }
    out
}

#[cfg(test)]
mod tests {

    /// "no mail" is indistinguishable from "this feature is broken". Nothing said that a
    /// maildir was looked for, where, or that a remote account is a different thing this
    /// build cannot reach.
    #[test]
    fn an_empty_pane_says_where_it_looked() {
        // See lib.rs's ENV_TEST_LOCK doc comment: HOME/MAIL are process-global
        // and editor_open_env.rs's tests mutate HOME too.
        let _env_guard = crate::ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("vt-mail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev_home = std::env::var_os("HOME");
        let prev_mail = std::env::var_os("MAIL");
        // USER too, and this is the whole bug. `candidates()` probes
        // /var/mail/$USER, which EXISTS on a real build host -- so discover()
        // found the tester's actual mailbox, empty_reason() correctly returned
        // None, and the test blamed the code. Controlling HOME and MAIL is not
        // enough; every input candidates() reads has to be controlled or the
        // test is really asking whether the machine has mail.
        let prev_user = std::env::var_os("USER");
        // SAFETY: single-threaded test; restored below.
        unsafe {
            std::env::set_var("HOME", &dir);
            std::env::remove_var("MAIL");
            std::env::remove_var("USER");
        }

        let why = empty_reason().expect("no store, so a reason");
        assert!(why.contains("Looked in"), "{why}");
        assert!(why.contains(&dir.join("Maildir").display().to_string()), "{why}");
        // The remote case is named, because "no mail" reads as "your mail is missing" to
        // someone who has an account somewhere and no local store at all — and it is
        // named as the thing to DO, since the pane can sign in to Proton now.
        assert!(why.contains("Proton"), "{why}");
        assert!(why.contains("$MAIL"), "the way out should be in the message: {why}");

        unsafe {
            match prev_home { Some(v) => std::env::set_var("HOME", v), None => std::env::remove_var("HOME") }
            if let Some(v) = prev_mail { std::env::set_var("MAIL", v) }
            if let Some(v) = prev_user { std::env::set_var("USER", v) }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir()
            .join(format!("voiceterm-mail-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        let _ = std::fs::remove_file(&p);
        p
    }

    const MSG: &str = "\
From: Ada <ada@example.com>
Subject: Engine
Date: Tue, 4 Mar 2025 09:15:00 +0000
Message-ID: <a@x>

body
";

    #[test]
    fn a_maildir_is_read_from_cur_and_new_but_never_tmp() {
        // tmp/ holds messages mid-delivery. Reading one shows half a mail
        // that a later scan silently replaces with the whole thing.
        let root = tmp("maildir");
        for sub in ["cur", "new", "tmp"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        std::fs::write(root.join("cur").join("1"), MSG).unwrap();
        std::fs::write(root.join("new").join("2"),
                       MSG.replace("<a@x>", "<b@x>").replace("Engine", "Second")).unwrap();
        std::fs::write(root.join("tmp").join("3"),
                       MSG.replace("<a@x>", "<c@x>").replace("Engine", "Partial")).unwrap();

        assert_eq!(classify(&root), Kind::Maildir);
        let msgs = load(&root);
        assert_eq!(msgs.len(), 2, "{:?}", msgs.iter().map(|m| &m.subject).collect::<Vec<_>>());
        assert!(!msgs.iter().any(|m| m.subject == "Partial"), "a tmp/ message was read");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_maildir_message_whose_body_mentions_a_from_line_is_not_split() {
        // One file is one message. Applying mbox splitting here tears a
        // message in two at any body line starting "From ", and quoted mail
        // is full of them.
        let root = tmp("quoted");
        std::fs::create_dir_all(root.join("cur")).unwrap();
        let quoted = format!("{MSG}From here on it is quoted text\nmore body\n");
        std::fs::write(root.join("cur").join("1"), quoted).unwrap();
        let msgs = load(&root);
        assert_eq!(msgs.len(), 1, "the body split the message");
        assert_eq!(msgs[0].subject, "Engine");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_mbox_yields_its_messages_newest_first() {
        // A list whose order depends on which format the person happens to
        // use is a list that looks broken on one of them.
        let path = tmp("inbox");
        let older = MSG.replace("<a@x>", "<old@x>").replace("Engine", "Older");
        let newer = MSG.replace("<a@x>", "<new@x>").replace("Engine", "Newer")
                       .replace("09:15:00", "18:00:00");
        std::fs::write(&path, format!("From a\n{older}From b\n{newer}")).unwrap();
        assert_eq!(classify(&path), Kind::Mbox);
        let msgs = load(&path);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].subject, "Newer", "not newest first");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_bodies_keys_match_loads_ids_for_a_maildir() {
        // The lazy-fetch map is only useful if its keys are the SAME ids
        // `load` hands the reader — a body map that can't be looked up by
        // the id the inbox list actually uses is a body map for nothing.
        let root = tmp("bodies-maildir");
        std::fs::create_dir_all(root.join("cur")).unwrap();
        std::fs::write(root.join("cur").join("1"), MSG).unwrap();
        let msgs = load(&root);
        let bodies = load_bodies(&root);
        assert_eq!(msgs.len(), 1);
        assert_eq!(bodies.get(&msgs[0].id).map(|s| s.as_str()), Some("body\n"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The exact bug caught in review before this shipped: two messages in
    /// one mbox file, NEITHER with a Message-ID, must not collide onto the
    /// same fallback key. An earlier draft used a bare, shared `origin` as
    /// the fallback id for every part of the file — this pins the fix
    /// (`format!("{origin}#{i}")`, matching `messages_from_text`'s own
    /// per-part origin exactly) rather than trusting it silently.
    #[test]
    fn load_bodies_does_not_collide_two_no_id_messages_in_one_mbox() {
        let path = tmp("bodies-mbox-no-id");
        // Both messages omit Message-ID on purpose.
        let a = "From: a@x\nSubject: First\nDate: Tue, 4 Mar 2025 09:15:00 +0000\n\nfirst body\n";
        let b = "From: b@x\nSubject: Second\nDate: Tue, 4 Mar 2025 10:00:00 +0000\n\nsecond body\n";
        std::fs::write(&path, format!("From x\n{a}From y\n{b}")).unwrap();
        let msgs = load(&path);
        let bodies = load_bodies(&path);
        assert_eq!(msgs.len(), 2);
        assert_ne!(msgs[0].id, msgs[1].id, "the two messages must not share a fallback id");
        let body_for = |subject: &str| {
            let id = &msgs.iter().find(|m| m.subject == subject).unwrap().id;
            bodies.get(id).cloned()
        };
        assert_eq!(body_for("First"), Some("first body\n".to_string()));
        assert_eq!(body_for("Second"), Some("second body\n".to_string()));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_unreadable_file_costs_that_message_and_not_the_inbox() {
        // A spool always has something odd in it, and one bad file must not
        // empty the list.
        let root = tmp("mixed");
        std::fs::create_dir_all(root.join("cur")).unwrap();
        std::fs::write(root.join("cur").join("good"), MSG).unwrap();
        // Invalid UTF-8: `read_to_string` refuses it.
        std::fs::write(root.join("cur").join("bad"), [0xff, 0xfe, 0x00]).unwrap();
        let msgs = load(&root);
        assert_eq!(msgs.len(), 1, "one bad file emptied the inbox");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_directory_that_is_not_a_maildir_reads_as_nothing_rather_than_guessing() {
        let root = tmp("notmail");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("notes.txt"), MSG).unwrap();
        assert_eq!(classify(&root), Kind::Unknown);
        assert!(load(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_path_is_empty_and_not_a_panic() {
        let p = tmp("absent");
        assert_eq!(classify(&p), Kind::Unknown);
        assert!(load(&p).is_empty());
    }

    #[test]
    fn disk_and_terminal_sources_merge_without_showing_a_mail_twice() {
        // `cat`-ing a file that is also in the Maildir is the ordinary way
        // to read mail in a terminal. Showing it twice makes the inbox
        // wrong exactly for the person using both halves of this app.
        let root = tmp("merge");
        std::fs::create_dir_all(root.join("cur")).unwrap();
        std::fs::write(root.join("cur").join("1"), MSG).unwrap();
        let from_disk = load(&root);
        assert_eq!(from_disk.len(), 1);

        // The same message, as it would arrive from a terminal block.
        let from_term = crate::mail_rfc822::messages_from_text("block:1", MSG);
        assert_eq!(from_term.len(), 1);
        assert_eq!(from_disk[0].id, from_term[0].id,
                   "the Message-ID is what makes de-duplication possible");

        // Through the real function now, not a copy of its loop.
        let all = merge(&from_disk, &from_term);
        let mut view = crate::mail_view::MailView::default();
        view.set_messages(&all);
        assert_eq!(view.visible().len(), 1, "the same mail appeared twice");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_terminal_message_with_no_id_is_not_merged_away() {
        // De-duplication is by Message-ID. A message that has none gets a
        // synthesised id from its origin, so two different unheadered
        // dumps stay two rows rather than collapsing into one.
        let a = crate::mail_rfc822::messages_from_text("block:1", "no headers here
");
        let b = crate::mail_rfc822::messages_from_text("block:2", "different text
");
        assert_ne!(a[0].id, b[0].id);
    }

    #[test]
    fn the_mail_variable_is_consulted_before_any_guess() {
        // A machine that sets $MAIL has an opinion, and it is worth more
        // than the order of our guesses.
        let c = candidates();
        if let Ok(m) = std::env::var("MAIL") {
            if !m.trim().is_empty() {
                assert_eq!(c.first(), Some(&PathBuf::from(m)));
                return;
            }
        }
        // Unset here, which is the ordinary sandbox case: the guesses are
        // still in the documented order.
        if std::env::var("HOME").is_ok() {
            assert!(c.first().is_some_and(|p| p.ends_with("Maildir")), "{c:?}");
        }
    }
}

/// Combine mail from disk with mail harvested from the terminal.
///
/// This lived in the binary, and the test for it in `mail_store` COPIED
/// the loop rather than calling it — so the test proved that a
/// reimplementation of the rule worked, which is the one thing it could
/// not usefully prove. A rule with two implementations has two
/// behaviours the first time either is edited.
///
/// Disk first, then anything from the terminal not already there.
/// De-duplicated by Message-ID, because `cat`-ing a file that is also in
/// the Maildir is the ordinary way to read mail in a terminal — showing
/// it twice makes the inbox wrong precisely for the person using both
/// halves of the app.
pub fn merge(from_disk: &[Message], from_terminal: &[Message]) -> Vec<Message> {
    let mut all = from_disk.to_vec();
    for m in from_terminal {
        if !all.iter().any(|x| x.id == m.id) {
            all.push(m.clone());
        }
    }
    all
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    fn msg(id: &str, subject: &str) -> Message {
        Message {
            id: id.into(),
            conversation_id: id.into(),
            subject: subject.into(),
            unread: false,
            sender: None,
            time: 0,
            num_attachments: 0,
            is_encrypted: 0,
        }
    }

    #[test]
    fn the_same_mail_from_both_sources_appears_once() {
        let disk = vec![msg("<a@x>", "One")];
        let term = vec![msg("<a@x>", "One"), msg("<b@x>", "Two")];
        let all = merge(&disk, &term);
        assert_eq!(all.len(), 2, "{:?}", all.iter().map(|m| &m.id).collect::<Vec<_>>());
    }

    #[test]
    fn disk_wins_on_a_collision() {
        // The version on disk has been parsed from the whole file; the
        // terminal's came from whatever was printed, which may be a
        // fragment. Keeping the disk copy is keeping the more complete one.
        let disk = vec![msg("<a@x>", "Full subject")];
        let term = vec![msg("<a@x>", "trunc")];
        assert_eq!(merge(&disk, &term)[0].subject, "Full subject");
    }

    #[test]
    fn terminal_mail_with_no_counterpart_is_kept() {
        let all = merge(&[], &[msg("<t@x>", "From a cat")]);
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn order_is_disk_then_terminal_and_stable() {
        // The list is shown in this order before any sort, and a merge
        // that reshuffled would move rows under the reader on every
        // rescan.
        let disk = vec![msg("<1@x>", "a"), msg("<2@x>", "b")];
        let term = vec![msg("<3@x>", "c")];
        let ids: Vec<String> = merge(&disk, &term).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec!["<1@x>".to_string(), "<2@x>".into(), "<3@x>".into()]);
    }
}
