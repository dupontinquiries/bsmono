//! mail_rfc822 — email-shaped text into the mail view's model.
//!
//! The mail views render `MailView`, and `MailView::set_messages` has never
//! had a caller: `mail_messages` in the binary is `Vec::new()` at
//! construction and read in two places. So the inbox, the thread list and
//! the message pane are all correct renderings of nothing.
//!
//! The obvious data source is the Proton bridge, and it is not available
//! here for a reason the frontend's own header already records:
//! `mail_proton` drags the HTTP client and the scraper stack into a widget
//! crate, and none of it can be exercised without an account. What IS
//! available is text — and the terminal is full of it.
//!
//! ## Where the mail comes from: the terminal
//!
//! `term_blocks::BlockKind::Email` exists because people read mail in a
//! terminal: `mail`, `mutt -e display`, `cat ~/Maildir/…`, an `mbsync` log.
//! The classifier has been there the whole time with nothing downstream of
//! it. This module is the downstream: a block whose output is email-shaped
//! becomes messages in the mail view, so the two halves of the app that
//! both know about email stop being strangers.
//!
//! That also makes the whole path testable and offline, which the bridge
//! path is not.
//!
//! ## Deliberately a SUBSET of RFC 822
//!
//! Unfolding, `From`/`To`/`Subject`/`Date`/`Message-ID`, the blank line that
//! ends the headers, and mbox's `From ` separator. Not implemented: MIME
//! multipart, transfer encodings, RFC 2047 encoded words, address group
//! syntax. Each of those is a real thing a real mail file contains, and
//! every one of them degrades to "shown as written" rather than to a parse
//! failure — an encoded subject displays as `=?utf-8?B?…?=`, which is ugly
//! and readable, where refusing the message would lose it.
//!
//! ## Dates
//!
//! RFC 2822 dates are parsed to a unix timestamp because the view sorts on
//! one. Only the numeric fields are read — day, month name, year, time,
//! zone offset — with no calendar library: this is a sort key, and being a
//! second out matters less than a dependency. An unparseable date becomes
//! 0, which sorts oldest, rather than "now", which would put every
//! malformed message at the top of the list forever.

use crate::mail_view::{Address, Message};

/// One parsed message: headers we care about, plus the body.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Parsed {
    pub message_id: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: String,
    /// `In-Reply-To`/`References` root, when present — what makes two
    /// messages one thread.
    pub in_reply_to: String,
    pub body: String,
}

/// Split headers from body and unfold continuation lines.
///
/// A header value may continue on the next line if that line starts with
/// whitespace, and long `Subject:` lines routinely do. Not unfolding is how
/// half a subject ends up in the list and the other half nowhere.
fn headers_and_body(text: &str) -> (Vec<(String, String)>, String) {
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut lines = text.lines();
    let mut body = String::new();
    for line in lines.by_ref() {
        if line.trim().is_empty() {
            break; // the blank line ends the headers
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = headers.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
                continue;
            }
            // A continuation with nothing to continue: malformed, and
            // dropping it is better than inventing a header named "".
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            // A non-header line before the blank one means this was never a
            // header block. Everything from here is body.
            body.push_str(line);
            body.push('\n');
            break;
        };
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    for line in lines {
        body.push_str(line);
        body.push('\n');
    }
    (headers, body)
}

fn header<'a>(h: &'a [(String, String)], name: &str) -> &'a str {
    h.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()).unwrap_or("")
}

/// Parse one message.
pub fn parse_message(text: &str) -> Parsed {
    let (h, body) = headers_and_body(text);
    // `References` before `In-Reply-To`: the first entry of References is
    // the thread ROOT, while In-Reply-To is the immediate parent. Threading
    // on the parent builds a chain that splits whenever one message in the
    // middle is missing, which is common.
    let refs = header(&h, "references");
    let root = refs.split_whitespace().next().unwrap_or("").to_string();
    Parsed {
        message_id: header(&h, "message-id").to_string(),
        from: header(&h, "from").to_string(),
        to: header(&h, "to").to_string(),
        subject: header(&h, "subject").to_string(),
        date: header(&h, "date").to_string(),
        in_reply_to: if root.is_empty() {
            header(&h, "in-reply-to").to_string()
        } else {
            root
        },
        body,
    }
}

/// Split an mbox into messages.
///
/// The separator is a line starting `From ` (with a space, not a colon) at
/// the start of a message. A `From:` header is NOT a separator and the
/// space is the only thing distinguishing them, which is mbox's oldest and
/// most quoted flaw — worth handling exactly rather than approximately,
/// because getting it wrong splits a message in half at its own From header.
pub fn split_mbox(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut idx = 0usize;
    for line in text.split_inclusive('\n') {
        let is_sep = line.starts_with("From ");
        if is_sep {
            if let Some(s) = start {
                out.push(text[s..idx].trim_end_matches('\n'));
            }
            start = Some(idx + line.len());
        }
        idx += line.len();
    }
    if let Some(s) = start {
        let tail = text[s..].trim_end_matches('\n');
        if !tail.trim().is_empty() {
            out.push(tail);
        }
    } else if !text.trim().is_empty() {
        // No separator at all: a single message pasted or catted on its
        // own, which is the common case from a terminal block.
        out.push(text.trim_end_matches('\n'));
    }
    out
}

/// `Ada Lovelace <ada@example.com>` or a bare address.
pub fn parse_address(raw: &str) -> Address {
    let raw = raw.trim();
    if let Some(open) = raw.rfind('<') {
        if let Some(close) = raw[open..].find('>') {
            let addr = raw[open + 1..open + close].trim().to_string();
            let name = raw[..open].trim().trim_matches('"').trim().to_string();
            return Address { name, address: addr };
        }
    }
    Address { name: String::new(), address: raw.to_string() }
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun",
                            "jul", "aug", "sep", "oct", "nov", "dec"];

/// An RFC 2822 date to a unix timestamp, for sorting.
///
/// 0 on anything unparseable. Deliberately not "now": a malformed date
/// stamped with the current time puts that message at the top of the list
/// and keeps it there, where 0 puts it at the bottom and is obviously wrong
/// when you look.
pub fn parse_date(raw: &str) -> i64 {
    // "Tue, 4 Mar 2025 09:15:00 +0000" — the weekday is optional.
    let s = raw.trim();
    let s = s.split_once(',').map(|(_, r)| r.trim()).unwrap_or(s);
    let mut it = s.split_whitespace();
    let day: i64 = it.next().and_then(|d| d.parse().ok()).unwrap_or(0);
    let mon = it.next().unwrap_or("").to_ascii_lowercase();
    let mon = match MONTHS.iter().position(|m| mon.starts_with(m)) {
        Some(i) => i as i64,
        None => return 0,
    };
    let year: i64 = it.next().and_then(|y| y.parse().ok()).unwrap_or(0);
    if day == 0 || year < 1970 {
        return 0;
    }
    let time = it.next().unwrap_or("00:00:00");
    let mut t = time.split(':');
    let hh: i64 = t.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let mm: i64 = t.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let ss: i64 = t.next().and_then(|x| x.parse().ok()).unwrap_or(0);

    // Days since the epoch, by civil-date arithmetic (Howard Hinnant's
    // days_from_civil). No calendar crate, and correct across leap years
    // and centuries — which a naive 365.25 would not be, drifting a day per
    // century in a value used to sort a list.
    let (y, m) = (year, mon + 1);
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    let mut secs = days * 86_400 + hh * 3_600 + mm * 60 + ss;
    // Zone offset, subtracted to reach UTC. Absent means the local zone,
    // which we cannot know here; treated as UTC, which is a bounded error
    // in a sort key.
    if let Some(z) = it.next() {
        if let Some(rest) = z.strip_prefix('+').or_else(|| z.strip_prefix('-')) {
            if rest.len() == 4 {
                if let (Ok(h), Ok(m)) = (rest[..2].parse::<i64>(), rest[2..].parse::<i64>()) {
                    let off = h * 3_600 + m * 60;
                    secs += if z.starts_with('-') { off } else { -off };
                }
            }
        }
    }
    secs
}

/// Turn email-shaped text into messages the view can hold.
///
/// `origin` names where it came from (a block id, a path) and is folded
/// into any synthesised id, so two `cat`s of different files cannot collide
/// on `<no-id>` and appear as one thread.
pub fn messages_from_text(origin: &str, text: &str) -> Vec<Message> {
    split_mbox(text).into_iter().enumerate().map(|(i, raw)| {
        let p = parse_message(raw);
        let id = if p.message_id.is_empty() {
            format!("{origin}#{i}")
        } else {
            p.message_id.clone()
        };
        // Threading, in the order that makes a root and its replies agree.
        // My first version was: References root, else subject. It looked
        // right and split every thread it was given — the ROOT has no
        // References, so it threaded by subject, while its reply threaded
        // by the root's Message-ID, and the two never met.
        //
        // The rule that works is that a thread is named by its ROOT's
        // Message-ID: a reply says which that is, and a root IS one.
        // Subject is the fallback for mail with no ids at all, which is
        // what a terminal dump often is, and it is why "Re: Re: Fwd:" has
        // to come off.
        //
        // Known gap, left deliberately: a root that HAS a Message-ID and a
        // reply that omits References thread apart. Closing that means
        // matching a subject key against an id key, which merges unrelated
        // mail that happens to share a subject — "Re: hello" is not a
        // conversation.
        let conversation_id = if !p.in_reply_to.is_empty() {
            p.in_reply_to.clone()
        } else if !p.message_id.is_empty() {
            p.message_id.clone()
        } else {
            let s = strip_reply_prefixes(&p.subject);
            if s.is_empty() { id.clone() } else { format!("subj:{}", s.to_lowercase()) }
        };
        Message {
            id,
            conversation_id,
            subject: p.subject.clone(),
            // Everything read out of a terminal has, by definition, been
            // read. Marking it unread would put a badge on the inbox for
            // mail the person is looking at.
            unread: false,
            sender: Some(parse_address(&p.from)),
            time: parse_date(&p.date),
            num_attachments: 0,
            is_encrypted: 0,
        }
    }).collect()
}

/// `Re: Fwd: Re: thing` → `thing`.
pub fn strip_reply_prefixes(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        let cut = ["re:", "fwd:", "fw:", "aw:"].iter()
            .find(|p| lower.starts_with(**p))
            .map(|p| p.len());
        match cut {
            Some(n) => s = s[n..].trim_start(),
            None => break,
        }
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: &str = "\
From: Ada Lovelace <ada@example.com>
To: bo@example.com
Subject: Analytical engine
Date: Tue, 4 Mar 2025 09:15:00 +0000
Message-ID: <abc@example.com>

The engine weaves algebraic patterns.
";

    #[test]
    fn a_single_message_parses_into_the_views_model() {
        let m = messages_from_text("block:1", ONE);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id, "<abc@example.com>");
        assert_eq!(m[0].subject, "Analytical engine");
        let s = m[0].sender.as_ref().unwrap();
        assert_eq!(s.name, "Ada Lovelace");
        assert_eq!(s.address, "ada@example.com");
        assert!(!m[0].unread, "mail read out of a terminal is not unread");
    }

    #[test]
    fn a_folded_subject_arrives_whole() {
        // Long subjects routinely wrap. Not unfolding puts half of one in
        // the list and the other half nowhere.
        let text = "Subject: a subject that is long\n  and continues here\nFrom: a@b\n\nbody\n";
        let p = parse_message(text);
        assert_eq!(p.subject, "a subject that is long and continues here");
        assert_eq!(p.from, "a@b");
    }

    #[test]
    fn an_mbox_splits_on_the_from_line_and_not_on_the_from_header() {
        // mbox's oldest flaw: `From ` and `From:` differ by one character.
        // Splitting on the header cuts every message in half at its own
        // sender line.
        let mbox = format!("From ada@example.com Tue Mar 4 09:15:00 2025\n{ONE}\
From bo@example.com Tue Mar 4 10:00:00 2025\n\
From: Bo <bo@example.com>\nSubject: Re: Analytical engine\n\nagreed\n");
        let parts = split_mbox(&mbox);
        assert_eq!(parts.len(), 2, "{parts:#?}");
        assert!(parts[0].contains("Analytical engine"));
        assert!(parts[1].contains("agreed"));
    }

    #[test]
    fn a_reply_joins_the_thread_it_answers() {
        // Two ways, and both matter: References when the mail carries it,
        // and the stripped subject when it does not. A reply starting its
        // own thread is the failure people notice first.
        let mbox = format!("From a\n{ONE}From b\n\
From: Bo <bo@example.com>\nSubject: Re: Analytical engine\n\
References: <abc@example.com>\n\nagreed\n");
        let m = messages_from_text("block:1", &mbox);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].conversation_id, m[1].conversation_id,
                   "the reply started its own thread");

        let no_refs = "From: Bo <bo@x>\nSubject: Re: Re: Fwd: Analytical engine\n\nhi\n";
        let m2 = messages_from_text("block:2", no_refs);
        assert_eq!(m2[0].conversation_id, "subj:analytical engine");
    }

    #[test]
    fn dates_sort_correctly_across_a_leap_year_and_a_zone() {
        // The value exists to sort a list. A 365.25-day approximation drifts
        // a day per century, and the zone offset is a real difference
        // between two mails sent minutes apart in different places.
        let a = parse_date("Tue, 4 Mar 2025 09:15:00 +0000");
        let b = parse_date("Tue, 4 Mar 2025 09:15:00 +0100");
        assert!(b < a, "the eastern zone should be EARLIER in UTC: {b} vs {a}");
        assert_eq!(a - b, 3_600);
        // 1 Mar in a leap year is one day later in the year than in a
        // common one; the arithmetic must know that.
        let leap = parse_date("1 Mar 2024 00:00:00 +0000");
        let prev = parse_date("29 Feb 2024 00:00:00 +0000");
        assert_eq!(leap - prev, 86_400, "29 February went missing");
        // A known instant, from an independent source
        // (`date -u -d '2025-03-04 09:15:00' +%s` → 1741079700).
        assert_eq!(a, 1_741_079_700);
    }

    #[test]
    fn an_unparseable_date_sorts_oldest_rather_than_newest() {
        // Stamping it "now" puts every malformed message at the top of the
        // list and keeps it there.
        assert_eq!(parse_date("sometime last week"), 0);
        assert_eq!(parse_date(""), 0);
    }

    #[test]
    fn a_message_with_no_id_does_not_collide_with_another_files() {
        // Two `cat`s of different mail files both yielding `<no-id>` would
        // merge into one thread.
        let raw = "From: a@b\nSubject: hello\n\nbody\n";
        let a = messages_from_text("block:1", raw);
        let b = messages_from_text("block:2", raw);
        assert_ne!(a[0].id, b[0].id);
    }

    #[test]
    fn a_terminal_mail_dump_reaches_the_view_as_threads() {
        // The whole path this module exists for: what `cat ~/Maildir/…`
        // puts on a terminal becomes what the mail list draws. Asserted
        // through `MailView` rather than on the Vec, because `set_messages`
        // is the call that had never happened.
        let mbox = format!("From ada\n{ONE}From bo\nFrom: Bo <bo@example.com>\nSubject: Re: Analytical engine\nReferences: <abc@example.com>\nDate: Tue, 4 Mar 2025 10:00:00 +0000\n\nagreed\n");
        let msgs = messages_from_text("block:1", &mbox);
        let mut view = crate::mail_view::MailView::default();
        view.set_messages(&msgs);
        let threads = view.visible();
        assert_eq!(threads.len(), 1, "two messages of one exchange made {} threads",
                   threads.len());
        assert_eq!(threads[0].messages.len(), 2);
        assert_eq!(view.unread_total(), 0, "mail already read in a terminal is not unread");
    }

    #[test]
    fn output_that_is_not_mail_yields_a_message_of_body_and_nothing_else() {
        // The classifier upstream decides what is email-shaped; this must
        // not panic or invent headers when handed something else.
        let m = messages_from_text("block:9", "total 48\ndrwxr-xr-x 2 me me\n");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].subject, "");
        assert_eq!(m[0].time, 0);
    }
}
