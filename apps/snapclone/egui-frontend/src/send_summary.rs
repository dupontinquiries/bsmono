//! send_summary — `MediaSendSummary`, ported.
//!
//! The Qt header calls it *"a quiet send receipt for group media — three plain
//! facts, one line each, that make the snapclone-mediashare guarantees visible
//! before you send"*:
//!
//! * **privacy** — metadata stripped
//! * **size** — only what is not already shared goes out
//! * **encryption** — sealed to everyone in the thread
//!
//! ## Why a receipt and not a reassurance
//!
//! Every one of these is a claim the app already makes somewhere in its
//! marketing or its settings. What makes this screen worth having is that the
//! numbers come from the send path itself — the strip report, the manifest
//! diffed against the peers' chunk store, the actual key count — so it says what
//! *happened* rather than what the app generally does. A summary that showed
//! "encrypted ✓" without counting recipients would be a logo, not a receipt.
//!
//! ## Nothing animates
//!
//! The header is explicit: *"this is a moment of reassurance, not motion."* A
//! progress-style flourish here would suggest work in progress on a screen whose
//! whole job is to say the work is already accounted for.
//!
//! ## Zero is a real answer for every facet, and a different one each time
//!
//! Zero tags stripped means the file was already clean. Zero bytes to upload
//! means the recipients already have it. Zero recipients means there is nobody
//! to send to — and that one is a problem while the other two are good news.
//! Rendering them with a shared "0" template would flatten a warning into a
//! reassurance.

/// Which fact a row states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facet { Privacy, Size, Encryption }

impl Facet {
    /// The glyph from the Qt header: ✓ privacy, ↑ size, 🔒 encryption.
    pub fn glyph(self) -> &'static str {
        match self {
            Facet::Privacy => "✓",
            Facet::Size => "↑",
            Facet::Encryption => "🔒",
        }
    }

    pub fn all() -> [Facet; 3] { [Facet::Privacy, Facet::Size, Facet::Encryption] }
}

/// Human byte sizes, matching the Qt formatter's precision rules.
///
/// One decimal below 10 KB and none above, because "9.4 KB" is a meaningful
/// distinction while "847.3 KB" is noise — the reader is deciding whether this
/// is small or large, not auditing it.
pub fn human_bytes(n: i64) -> String {
    const KB: f64 = 1024.0;
    let n = n.max(0);
    if n < 1024 { return format!("{n} B"); }
    let k = n as f64 / KB;
    if k < 1024.0 {
        return if n < 10 * 1024 { format!("{k:.1} KB") } else { format!("{k:.0} KB") };
    }
    format!("{:.1} MB", k / KB)
}

/// The receipt. A pure view: the controller supplies what it already computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SendSummary {
    pub format: i32,
    pub segments_removed: i32,
    pub upload_bytes: i64,
    pub total_bytes: i64,
    pub recipients: i32,
}

impl SendSummary {
    /// From the metadata strip report.
    pub fn set_strip_result(&mut self, format: i32, segments_removed: i32) {
        self.format = format;
        self.segments_removed = segments_removed.max(0);
    }

    /// From the manifest diffed against the peers' chunk store.
    pub fn set_dedup(&mut self, upload_bytes: i64, total_bytes: i64) {
        self.total_bytes = total_bytes.max(0);
        // Uploading more than the file contains is impossible; clamping stops a
        // miscomputed manifest from rendering "Sending 4.2 MB" for a 1 MB photo
        // and making the receipt the least trustworthy screen in the app.
        self.upload_bytes = upload_bytes.clamp(0, self.total_bytes);
    }

    pub fn set_recipients(&mut self, count: i32) { self.recipients = count.max(0); }

    /// Bytes the recipients already have.
    pub fn saved_bytes(&self) -> i64 { self.total_bytes - self.upload_bytes }

    /// The headline for a row.
    pub fn primary_text(&self, f: Facet) -> String {
        match f {
            Facet::Privacy => "Metadata stripped".into(),
            Facet::Size => format!("Sending {}", human_bytes(self.upload_bytes)),
            Facet::Encryption => match self.recipients {
                // Named separately: "Encrypted to 0 people" is technically true
                // and reads as a failure, which it is — there is nobody to send
                // to.
                0 => "No recipients".into(),
                1 => "Encrypted to 1 person".into(),
                n => format!("Encrypted to {n} people"),
            },
        }
    }

    /// The supporting line.
    pub fn detail_text(&self, f: Facet) -> String {
        match f {
            Facet::Privacy => match self.segments_removed {
                // Already clean is good news, not an absence of news.
                0 => "Nothing to strip".into(),
                1 => "1 tag stripped".into(),
                n => format!("{n} tags stripped"),
            },
            Facet::Size => {
                let saved = self.saved_bytes();
                if self.total_bytes == 0 { return String::new(); }
                if saved <= 0 { return "Nothing already shared".into(); }
                format!("{} already shared", human_bytes(saved))
            }
            Facet::Encryption => {
                if self.recipients == 0 { "Add someone to this thread".into() }
                else { "Sealed to everyone in the thread".into() }
            }
        }
    }

    /// Is anything here a problem rather than a reassurance?
    ///
    /// Only the recipient count. The other two facets cannot fail — zero tags
    /// and zero upload are both good outcomes — so a caller that greys out Send
    /// needs to know which zero it is looking at.
    pub fn blocks_send(&self) -> bool { self.recipients == 0 }
}

/// Draw the receipt.
pub fn send_summary_ui(ui: &mut egui::Ui, s: &SendSummary,
                       c: &uikit_egui::PaletteColors) {
    for f in Facet::all() {
        ui.horizontal(|ui| {
            let problem = f == Facet::Encryption && s.blocks_send();
            ui.colored_label(if problem { c.warning } else { c.success }, f.glyph());
            ui.vertical(|ui| {
                ui.colored_label(c.text, s.primary_text(f));
                let d = s.detail_text(f);
                if !d.is_empty() {
                    ui.colored_label(c.text_weak, egui::RichText::new(d).small());
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> SendSummary {
        let mut s = SendSummary::default();
        s.set_strip_result(1, 4);
        s.set_dedup(300_000, 1_200_000);
        s.set_recipients(3);
        s
    }

    #[test]
    fn the_three_facets_each_state_a_different_guarantee() {
        let s = summary();
        assert_eq!(s.primary_text(Facet::Privacy), "Metadata stripped");
        assert!(s.primary_text(Facet::Size).starts_with("Sending"));
        assert_eq!(s.primary_text(Facet::Encryption), "Encrypted to 3 people");
    }

    #[test]
    fn nothing_to_strip_is_good_news_rather_than_an_absence_of_news() {
        // The file was already clean, and a blank line there would read as the
        // check not having run.
        let mut s = summary();
        s.set_strip_result(1, 0);
        assert_eq!(s.detail_text(Facet::Privacy), "Nothing to strip");
    }

    #[test]
    fn one_tag_is_singular() {
        let mut s = summary();
        s.set_strip_result(1, 1);
        assert_eq!(s.detail_text(Facet::Privacy), "1 tag stripped");
    }

    #[test]
    fn dedup_reports_what_the_recipients_already_have() {
        // The claim this row exists to make visible: only what is not already
        // shared goes out.
        let s = summary();
        assert_eq!(s.saved_bytes(), 900_000);
        assert!(s.detail_text(Facet::Size).contains("already shared"));
    }

    #[test]
    fn a_fully_deduplicated_send_uploads_nothing_and_says_so() {
        // Zero bytes is the BEST outcome here, and must not read as a failure.
        let mut s = summary();
        s.set_dedup(0, 1_200_000);
        assert_eq!(s.primary_text(Facet::Size), "Sending 0 B");
        assert!(s.detail_text(Facet::Size).contains("already shared"));
    }

    #[test]
    fn a_send_with_nothing_already_shared_says_that_too() {
        let mut s = summary();
        s.set_dedup(1_200_000, 1_200_000);
        assert_eq!(s.detail_text(Facet::Size), "Nothing already shared");
    }

    #[test]
    fn uploading_more_than_the_file_contains_is_impossible() {
        // A miscomputed manifest rendering "Sending 4.2 MB" for a 1 MB photo
        // would make the receipt the least trustworthy screen in the app.
        let mut s = SendSummary::default();
        s.set_dedup(9_000_000, 1_000_000);
        assert_eq!(s.upload_bytes, 1_000_000);
        assert_eq!(s.saved_bytes(), 0);
    }

    #[test]
    fn negative_inputs_cannot_reach_the_screen() {
        let mut s = SendSummary::default();
        s.set_dedup(-5, -9);
        s.set_strip_result(0, -3);
        s.set_recipients(-2);
        assert_eq!(s.upload_bytes, 0);
        assert_eq!(s.total_bytes, 0);
        assert_eq!(s.segments_removed, 0);
        assert_eq!(s.recipients, 0);
    }

    #[test]
    fn zero_recipients_is_a_problem_where_the_other_zeroes_are_not() {
        // Rendering all three with a shared "0" template would flatten a
        // warning into a reassurance.
        let mut s = summary();
        s.set_recipients(0);
        assert_eq!(s.primary_text(Facet::Encryption), "No recipients");
        assert_eq!(s.detail_text(Facet::Encryption), "Add someone to this thread");
        assert!(s.blocks_send());

        s.set_strip_result(1, 0);
        s.set_dedup(0, 100);
        s.set_recipients(2);
        assert!(!s.blocks_send(), "clean file and full dedup do not block");
    }

    #[test]
    fn one_recipient_is_singular() {
        let mut s = summary();
        s.set_recipients(1);
        assert_eq!(s.primary_text(Facet::Encryption), "Encrypted to 1 person");
    }

    #[test]
    fn byte_sizes_keep_a_decimal_only_where_it_means_something() {
        // "9.4 KB" is a meaningful distinction; "847.3 KB" is noise — the
        // reader is deciding small or large, not auditing.
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(9_600), "9.4 KB");
        assert_eq!(human_bytes(867_000), "847 KB");
        assert_eq!(human_bytes(5_242_880), "5.0 MB");
    }

    #[test]
    fn zero_bytes_renders_as_bytes_rather_than_as_a_fraction_of_a_kilobyte() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(-4), "0 B");
    }

    #[test]
    fn an_empty_send_shows_no_size_detail_rather_than_a_misleading_one() {
        // With no file there is nothing shared and nothing to share, and
        // claiming either would be inventing a fact.
        let s = SendSummary::default();
        assert_eq!(s.detail_text(Facet::Size), "");
    }

    #[test]
    fn each_facet_has_its_own_glyph() {
        // ✓ privacy, ↑ size, 🔒 encryption — three facts read at a glance, and
        // a shared glyph would make them one blurred claim.
        let g: Vec<&str> = Facet::all().iter().map(|f| f.glyph()).collect();
        let mut sorted = g.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), g.len());
    }
}
