//! fao-codec-egui — the egui-primary port of fao_app (GTK4/libadwaita), begun per
//! UI_STRATEGY.md: uikit-egui shell + pages calling the codec crates directly; the GTK
//! app stays the parity reference until every surface lands. First slice: the shell
//! (adaptive nav, system palette) and the **Encode Lab** — the page that exercises the
//! REAL encoder (`fao_codec_lib::encode_iframe_recon`) on a generated test frame at the
//! user's quality / dead-zone / chroma settings and reports compressed size, bits per
//! pixel, and PSNR. Numbers over pictures for v1: the codec math is the thing being
//! ported around, and a headless-testable report keeps this crate green in CI the way
//! the other -egui ports are. Remaining fao_app surfaces (sequence encode, block-match
//! viz, v1/v2 comparison) are the follow-up pages — the pattern is set here.

pub mod part_fit;
pub mod reproject;
pub mod seq_lab;
pub mod seq_page;
pub mod lab;
pub mod library;
pub mod player;
pub mod decode;
/// Write frames as a `.fao`. Uses the v1 DCT path because the V2 encoder's
/// `FrameResult` returns no bitstream -- see the module notes.
/// Every codec version by codename -- original names, not version numbers --
/// with what each can actually encode and decode in this frontend.
pub mod codec_catalog;
pub mod export;
/// Seek-based .fao reading in constant memory: 600-hour and 8K files, and
/// timestamps past the 596.5-hour signed-32-bit overflow.
pub mod stream;
/// Reading a remote `.fao` over HTTP byte ranges: a pure block cache plus a
/// ureq transport, so the streaming player can play files on a dufs server.
pub mod http_range;
/// Quiltstone: block-patch frames (marker M) -- only the 16x16 blocks that
/// changed, over the Keystone DCT codec.
pub mod patch;
/// Shiftquilt: patch frames with motion-predicted blocks (marker Q).
pub mod motion;
/// The media collection: video files from local folders and dufs servers, with
/// glob patterns and individual selection. Distinct from the mesh Web Library.
/// Fast sampled estimate of what a SHARED block dictionary would save across a
/// selection of files, versus one dictionary per file.
pub mod analyze;
pub mod media;
/// The Media tab: the egui shell over `media`.
pub mod media_page;
pub mod transcode;
pub mod encode_page;
pub mod archive;

#[derive(Clone, Copy, PartialEq, Eq)]
// ── PARITY STATUS, measured 2026-09-16 against fao_app/src/main.rs ──────────
//
// Tester report: "very little of the gtk viewer visually ... I can't use the
// encode page or web view load libraries etc. I can only do a test frame that
// doesn't even have feedback."
//
// Measured rather than guessed. fao_app is 12,957 lines in one file; this
// frontend is ~2,700 across eleven modules. The gap is NOT that the ported
// modules are thin -- `library.rs` calls the same `Library::init/open`,
// `stats`, `prune_unused`, `rollback_manifest` backend the GTK handlers do,
// and records its one divergence (folder chooser becomes a path field). What
// is missing is WHOLE SURFACES:
//
//   * The GTK "Encode" page -- encoding a real file end to end. `lab.rs` runs
//     the real encoder but only over `Source::pattern`, a synthetic test
//     image, which is exactly the "test frame" in the report. `Source::load`
//     for PNG/JPEG is written and works; what has no counterpart is encoding
//     VIDEO, which is what the Encode page is for.
//   * Any web view. `grep -ric webview` across this frontend returns 0 in
//     every module. The GTK app's web surface has no port at all, not a thin
//     one.
//   * "Open Archive" / "Export" -- present in the GTK label set, absent here.
//
// So the honest framing for whoever picks this up: the five tabs below are
// reachable and their backends are real, and the app is still not usable for
// the task a tester sits down to do, because the task lives on a page that was
// never ported. Adding feedback to the Encode Lab would not fix the report;
// the Encode page would.
//
// UPDATE (c20unified): the Encode page exists now (`encode_page.rs`, the
// default tab): a real video in, a .fao out, streamed one frame at a time,
// cancellable, with live progress / per-frame cost / decoded preview, and
// "Open in Player". It encodes with the one codec the port has (intra DCT);
// the GTK dialog's v2.0-v8 / v9 codec choices are still not ported and the
// page says so. Since then: T-frames (v2.1) on that page, and the Archive
// tab (New / Open Archive, Add Video, finished encodes added -- the GTK
// Export flow). The web view remains unported.
pub enum Tab {
    Encode,
    Lab,
    SeqLab,
    Library,
    Media,
    Player,
    Archive,
    Settings,
}

pub struct App {
    /// The accessibility scene widgets register with.
    ///
    /// Parity reported `a11y=0` for this app. Held on `App` rather than
    /// built per frame because `A11yScene` remembers focus BY ID between
    /// frames — a per-frame scene forgets, which presents as focus
    /// jumping rather than as a missing scene.
    pub a11y: uikit_egui::a11y::A11yScene,
    pub tab: Tab,
    pub prefs: settings_kit::Prefs,
    pub lab: lab::State,
    pub seq: seq_page::SeqState,
    pub library: library::State,
    pub media: media_page::State,
    pub player: player::State,
    pub encode: encode_page::State,
    pub archive: archive::State,
}

impl Default for App {
    fn default() -> Self {
        let mut prefs = settings_kit::Prefs::default();
        prefs.palette = "system".to_string();
        App { a11y: uikit_egui::a11y::A11yScene::new(), tab: Tab::Encode, prefs, lab: lab::State::default(), seq: seq_page::SeqState::default(), library: library::State::default(), media: media_page::State::default(), player: player::State::default(), encode: encode_page::State::default(), archive: archive::State::default() }
    }
}

impl App {
    #[allow(deprecated)] // Panel::show(ctx): same eframe root-Ui limitation as gitmind-egui
    pub fn ui(&mut self, ctx: &egui::Context) {
        uikit_egui::apply_prefs(ctx, &self.prefs);
        let adaptive = uikit_egui::adaptive::Adaptive::detect(ctx);
        let compact = adaptive.is_compact();
        if compact {
            egui::Panel::bottom("nav").show(ctx, |ui| {
                ui.horizontal(|ui| self.nav(ui));
            });
        } else {
            egui::Panel::left("nav").resizable(false).show(ctx, |ui| {
                ui.add_space(6.0);
                ui.heading("fao codec");
                ui.add_space(6.0);
                self.nav(ui);
            });
        }
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Encode => encode_page::ui(self, ui),
            Tab::Lab => lab::ui(self, ui),
            Tab::SeqLab => seq_page::ui(self, ui),
            Tab::Library => library::ui(self, ui),
            Tab::Media => media_page::ui(self, ui),
            Tab::Player => player::ui(self, ui),
            Tab::Archive => archive::ui(self, ui),
            Tab::Settings => {
                ui.heading("Settings");
                ui.label(egui::RichText::new("palette / text follow the shared prefs").weak().small());
            }
        });
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        for (t, label) in [(Tab::Encode, "⏺ Encode"), (Tab::Lab, "🎞 Encode Lab"), (Tab::SeqLab, "⏱ Seq Lab"),
                           (Tab::Library, "🔗 Library"), (Tab::Media, "🗂 Media"),
                           (Tab::Player, "▶ Player"), (Tab::Archive, "🗄 Archive"), (Tab::Settings, "⚙ Settings")] {
            if ui.selectable_label(self.tab == t, label).clicked() {
                self.tab = t;
            }
        }
    }
}
