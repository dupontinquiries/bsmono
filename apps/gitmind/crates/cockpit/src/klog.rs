//! klog — kernel output, turned into something worth analysing.
//!
//! [`kerneltry`](crate::kerneltry) says *when* to read `/sys/fs/pstore`; this
//! says what the bytes mean. It sits here rather than in logscope because it
//! is device knowledge — the same knowledge that knows pstore survives a
//! reboot — and logscope consumes the result as text.
//!
//! ## Why raw dmesg is bad input for a language model
//!
//! logscope's `Corpus::ingest` builds a word-context graph: every distinct
//! token is a vocabulary entry. Feed it raw kernel output and the vocabulary
//! fills with things that are unique by construction —
//!
//! ```text
//! [   12.345678] wlan0: authenticate with 3c:37:86:1f:22:0e
//! [   12.501201] Call trace: __schedule+0x2f4/0x8b0
//! ```
//!
//! — timestamps, MACs, addresses, offsets. Every line is novel, nothing
//! recurs, and the graph learns that kernel logs are random. The *shapes*
//! recur, so normalisation is what makes the corpus mean anything:
//! `__schedule+<off>/<off>` appears in a thousand traces and
//! `+0x2f4/0x8b0` appears once.
//!
//! Normalisation is destructive, which is why [`Record`] keeps the original
//! line. The model reads the normalised form; a human reading a specific
//! crash needs the real offsets.

/// One parsed line.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// Seconds since boot, when the line carried a `[ 12.345 ]` stamp.
    pub monotonic: Option<f64>,
    /// Syslog level from a `<4>` prefix, when present.
    pub level: Option<u8>,
    /// The message, stamp and prefix removed.
    pub message: String,
    /// The line exactly as it arrived. Kept because normalisation throws away
    /// the offsets a person needs to read one specific crash.
    pub raw: String,
}

impl Record {
    /// Level 0-3: emerg, alert, crit, err. What is worth waking someone for.
    pub fn is_error(&self) -> bool {
        self.level.map(|l| l <= 3).unwrap_or(false)
    }
}

/// Strip a leading `<N>` syslog prefix.
fn take_level(s: &str) -> (Option<u8>, &str) {
    let t = s.strip_prefix('<').unwrap_or(s);
    if t.len() == s.len() {
        return (None, s);
    }
    match t.find('>') {
        Some(i) => match t[..i].parse::<u8>() {
            Ok(l) => (Some(l), &t[i + 1..]),
            Err(_) => (None, s),
        },
        None => (None, s),
    }
}

/// Strip a leading `[ 12.345678]` monotonic stamp.
fn take_stamp(s: &str) -> (Option<f64>, &str) {
    let t = s.trim_start();
    if !t.starts_with('[') {
        return (None, s);
    }
    let close = match t.find(']') {
        Some(i) => i,
        None => return (None, s),
    };
    match t[1..close].trim().parse::<f64>() {
        Ok(v) => (Some(v), t[close + 1..].trim_start()),
        // `[drm]` and friends are message text, not stamps. Parsing failure is
        // the discriminator, which is why this is tried rather than matched on
        // a shape.
        Err(_) => (None, s),
    }
}

/// Parse one line.
pub fn parse_line(line: &str) -> Record {
    let raw = line.to_string();
    let (level, rest) = take_level(line);
    let (monotonic, rest) = take_stamp(rest);
    Record { monotonic, level, message: rest.trim().to_string(), raw }
}

/// Parse a whole buffer, skipping blank lines.
pub fn parse(text: &str) -> Vec<Record> {
    text.lines().filter(|l| !l.trim().is_empty()).map(parse_line).collect()
}

/// Replace the parts of a message that are unique per-occurrence.
///
/// Order matters: the longest, most specific patterns go first, or a MAC
/// address gets eaten piecewise by the hex-number rule and becomes six
/// separate `<hex>` tokens.
pub fn normalize(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let b: Vec<char> = msg.chars().collect();
    let mut i = 0;

    // A MAC is six hex pairs joined by colons — recognised as a unit.
    let is_mac_at = |i: usize| -> bool {
        let mut j = i;
        for pair in 0..6 {
            if j + 1 >= b.len() || !b[j].is_ascii_hexdigit() || !b[j + 1].is_ascii_hexdigit() {
                return false;
            }
            j += 2;
            if pair < 5 {
                if j >= b.len() || b[j] != ':' {
                    return false;
                }
                j += 1;
            }
        }
        true
    };

    while i < b.len() {
        if is_mac_at(i) {
            out.push_str("<mac>");
            i += 17;
            continue;
        }
        // 0x… — an address or an offset.
        if b[i] == '0' && i + 1 < b.len() && (b[i + 1] == 'x' || b[i + 1] == 'X') {
            let mut j = i + 2;
            while j < b.len() && b[j].is_ascii_hexdigit() {
                j += 1;
            }
            if j > i + 2 {
                out.push_str("<addr>");
                i = j;
                continue;
            }
        }
        // A bare run of digits. Kept as one token so a pid and a byte count
        // both become <n> rather than differing by length.
        if b[i].is_ascii_digit() {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == '.') {
                j += 1;
            }
            out.push_str("<n>");
            i = j;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// The text to hand a corpus: normalised messages, one per line.
///
/// Timestamps are dropped entirely rather than normalised — a `<n>` at the
/// start of every single line teaches the graph nothing except that lines
/// begin.
pub fn corpus_text(records: &[Record]) -> String {
    records
        .iter()
        .map(|r| normalize(&r.message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The lines that say a kernel died.
///
/// Matched on the phrases the kernel actually prints. Deliberately a short
/// list: a long one drifts into matching ordinary driver complaints, and a
/// crash detector that fires on every boot is one nobody reads.
const CRASH_MARKERS: [&str; 6] = [
    "kernel panic",
    "unable to handle kernel",
    "oops:",
    "bug: kernel null pointer",
    "internal error:",
    "watchdog: bug: soft lockup",
];

/// What killed it, if anything did.
#[derive(Debug, Clone, PartialEq)]
pub struct Crash {
    /// The line that announced it, normalised — the signature two crashes are
    /// "the same" by.
    pub signature: String,
    /// Where in the record list it was, so a caller can show the run-up.
    pub at: usize,
    /// Frames from the call trace that followed, normalised and in order.
    pub trace: Vec<String>,
}

/// Find the first crash in a log.
///
/// First rather than last: a panic cascades, and the later messages are
/// consequences of the first one. Reporting the last line of a cascade points
/// the reader at the wreckage instead of the cause.
pub fn find_crash(records: &[Record]) -> Option<Crash> {
    let at = records.iter().position(|r| {
        let m = r.message.to_ascii_lowercase();
        CRASH_MARKERS.iter().any(|k| m.contains(k))
    })?;

    let mut trace = Vec::new();
    let mut in_trace = false;
    for r in records.iter().skip(at + 1) {
        let m = r.message.trim();
        let lower = m.to_ascii_lowercase();
        if lower.starts_with("call trace") {
            in_trace = true;
            continue;
        }
        if in_trace {
            // A trace frame is `symbol+0x…/0x…`, possibly indented. Anything
            // else ends it — the trace is contiguous and stopping at the first
            // non-frame avoids swallowing the rest of the log.
            if m.contains('+') && m.contains("0x") {
                trace.push(normalize(m));
            } else if !m.is_empty() {
                break;
            }
        }
    }
    Some(Crash { signature: normalize(&records[at].message), at, trace })
}

/// Collapse `message repeated N times` runs and exact duplicates.
///
/// A driver retrying in a loop can fill a buffer with one line. Left in, that
/// line dominates a corpus by sheer count and the model learns the retry, not
/// the failure.
pub fn dedupe(records: &[Record]) -> Vec<Record> {
    let mut out: Vec<Record> = Vec::new();
    for r in records {
        if out.last().map(|p: &Record| p.message == r.message).unwrap_or(false) {
            continue;
        }
        out.push(r.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const OOPS: &str = "\
[   12.345678] wlan0: authenticate with 3c:37:86:1f:22:0e
[   12.501201] Unable to handle kernel NULL pointer dereference at virtual address 0000000000000018
[   12.501300] Call trace:
[   12.501400]  __schedule+0x2f4/0x8b0
[   12.501500]  schedule+0x4c/0xc0
[   12.501600] ---[ end trace 0000000000000000 ]---
";

    #[test]
    fn a_stamped_line_splits_into_time_and_message() {
        let r = parse_line("[   12.345678] wlan0: link is not ready");
        assert_eq!(r.monotonic, Some(12.345678));
        assert_eq!(r.message, "wlan0: link is not ready");
        // The original survives: normalisation is destructive and a person
        // reading one crash needs the real text.
        assert!(r.raw.contains("12.345678"));
    }

    #[test]
    fn a_bracketed_word_is_not_mistaken_for_a_timestamp() {
        // "[drm]" and "[ 1.23]" have the same shape; only one parses as a
        // number, which is why the parse attempt is the discriminator.
        let r = parse_line("[drm] Initialized");
        assert_eq!(r.monotonic, None);
        assert_eq!(r.message, "[drm] Initialized");
    }

    #[test]
    fn a_syslog_level_is_read_and_errors_are_identifiable() {
        let r = parse_line("<3>[   1.000000] i2c: transfer failed");
        assert_eq!(r.level, Some(3));
        assert_eq!(r.monotonic, Some(1.0));
        assert_eq!(r.message, "i2c: transfer failed");
        assert!(r.is_error());
        assert!(!parse_line("<6>[ 1.0] ok").is_error());
        // No prefix means no claim either way, not "fine".
        assert!(!parse_line("[ 1.0] something").is_error());
    }

    #[test]
    fn a_mac_survives_as_one_token_rather_than_six() {
        // Order matters: the hex-number rule would otherwise eat a MAC
        // piecewise and produce six tokens where the shape is one thing.
        let n = normalize("authenticate with 3c:37:86:1f:22:0e");
        assert_eq!(n, "authenticate with <mac>");
    }

    #[test]
    fn addresses_and_offsets_collapse_so_the_shape_recurs() {
        // The whole point: __schedule+<addr>/<addr> appears in a thousand
        // traces; +0x2f4/0x8b0 appears once. A corpus needs the former.
        assert_eq!(normalize("__schedule+0x2f4/0x8b0"), "__schedule+<addr>/<addr>");
        assert_eq!(
            normalize("NULL pointer dereference at virtual address 0000000000000018"),
            "NULL pointer dereference at virtual address <n>"
        );
        // Two different crashes at different offsets normalise identically,
        // which is what makes them countable as the same crash.
        assert_eq!(normalize("foo+0x10/0x20"), normalize("foo+0x999/0xaaa"));
    }

    #[test]
    fn the_crash_signature_is_the_first_line_not_the_last() {
        // A panic cascades. The last line of a cascade is wreckage; the first
        // is the cause, and pointing a reader at the wreckage wastes the trip.
        let recs = parse(OOPS);
        let c = find_crash(&recs).unwrap();
        assert!(c.signature.starts_with("Unable to handle kernel NULL pointer"));
        assert_eq!(c.at, 1, "the wlan line came first but is not a crash");
    }

    #[test]
    fn the_call_trace_is_captured_and_stops_where_it_ends() {
        let recs = parse(OOPS);
        let c = find_crash(&recs).unwrap();
        assert_eq!(c.trace, vec!["__schedule+<addr>/<addr>", "schedule+<addr>/<addr>"]);
        // "---[ end trace ]---" is not a frame, and the trace stops there
        // rather than swallowing the rest of the log.
        assert!(!c.trace.iter().any(|f| f.contains("end trace")));
    }

    #[test]
    fn a_clean_log_reports_no_crash() {
        // A detector that fires on ordinary driver complaints is one nobody
        // reads, so the marker list stays short.
        let clean = parse("[ 1.0] usb 1-1: new high-speed USB device\n[ 2.0] EXT4-fs: mounted");
        assert!(find_crash(&clean).is_none());
    }

    #[test]
    fn a_retrying_driver_does_not_drown_the_corpus() {
        // One line repeated five hundred times dominates a word-context graph
        // by count, and the model learns the retry instead of the failure.
        let spam: String = std::iter::repeat("[ 1.0] mmc0: tuning failed\n").take(500).collect();
        let recs = parse(&spam);
        assert_eq!(recs.len(), 500);
        assert_eq!(dedupe(&recs).len(), 1);
    }

    #[test]
    fn corpus_text_drops_timestamps_entirely() {
        // Normalising them to <n> would put the same token at the start of
        // every line, which teaches the graph only that lines begin.
        let recs = parse(OOPS);
        let text = corpus_text(&recs);
        assert!(!text.contains("12.345678"));
        assert!(!text.starts_with("<n>"));
        assert!(text.contains("<mac>"));
        assert_eq!(text.lines().count(), recs.len());
    }
}
