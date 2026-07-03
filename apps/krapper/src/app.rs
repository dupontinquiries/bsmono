//! Iced application — Scratchpad view (Path B / Ship B).
//!
//! The Beatbox / Calibrate / Vibe-capture stages from the workflow diagram are
//! stubbed for now (see `docs/path-a-plan.md`). This view assumes a manual
//! `VibeSummary` typed into the Vibe inputs panel.
//!
//! What you get:
//!   * a multi-line lyrics editor with live syllable counts
//!   * a Blurbs panel that produces stress-matched phrase fragments matching
//!     the current vibe, optionally rhyme-anchored
//!   * the hot-mic indicator from Path A's audio thread (already wired)
//!   * a collapsible debug panel surfacing audio + dict + vibe state

use crate::audio::{self, AudioState, OnsetEvent, SharedAudio, ONSET_HIGH, ONSET_LOW};
use crate::dict::Dict;
use crate::scale::{self, PitchHistogram, ScaleGuess};
use crate::session::{self, Session};
use crate::vibe::{self, VibeSource, VibeSummary};
use crate::vocab::{suggest_blurbs, syllable_count, Blurb, Vocab};

use std::path::PathBuf;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use iced::widget::{
    button, column, container, pick_list, row, scrollable, text, text_editor,
    text_input, Canvas, Space,
};
use iced::{mouse, Color, Element, Length, Point, Rectangle, Renderer, Size, Subscription, Theme};

// --- embedded data ----------------------------------------------------------

const WRLD_VOCAB: &str = include_str!("../vocab/words_wrld.txt");
const WEEKND_VOCAB: &str = include_str!("../vocab/words_theweeknd.txt");
const COMMON_VOCAB: &str = include_str!("../vocab/words_common.txt");
const STARTER_CMUDICT: &str = include_str!("../data/cmudict_starter.txt");

// --- messages ---------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Message {
    Tick,

    // Vibe inputs
    BpmChanged(String),
    SyllChanged(String),
    StressChanged(String),
    SeedChanged(String),
    VocabSelected(String),

    // Lyrics editor
    Lyrics(text_editor::Action),

    // Blurbs
    RefreshBlurbs,
    InsertBlurb(usize),

    // Hot mic (existing audio thread)
    ToggleRecord,
    ResetMic,

    // Vocab management
    AppendFileToVocab,
    NewVocabFromFile,

    // Misc
    ToggleDebug,
    BackToBeatsStub,
}

// --- state ------------------------------------------------------------------

pub struct App {
    // --- audio + reference data (carried from previous build) ---
    audio: SharedAudio,
    dict: Dict,
    dict_path: Option<String>,
    vocabs: Vec<Vocab>,
    vocab_names: Vec<String>,

    // --- vibe inputs (Ship B: typed manually) ---
    vibe: VibeSummary,
    bpm_input: String,
    syll_input: String,
    stress_input: String,
    seed_input: String,

    // --- lyrics editor state ---
    lyrics: text_editor::Content,

    // --- blurb suggestions ---
    blurbs: Vec<Blurb>,
    blurb_status: String,

    // --- audio thread mirror (Tick reads these) ---
    detected_syllables: u8,
    is_recording: bool,
    levels: Vec<f32>,
    smoothed_level: f32,
    armed: bool,
    onsets: Vec<OnsetEvent>,
    current_pitch_hz: Option<f32>,
    pitch_history: PitchHistogram,
    scale_guess: Option<ScaleGuess>,
    sample_rate: u32,

    // --- vocab management ---
    vocabs_dir: PathBuf,

    // --- chrome ---
    show_debug: bool,
    status: String,

    // --- save debouncing ---
    session_dirty: bool,
    last_edit_at: std::time::Instant,
}

impl App {
    pub fn new() -> (Self, iced::Task<Message>) {
        let audio = Arc::new(Mutex::new(AudioState::new()));
        {
            let st = Arc::clone(&audio);
            std::thread::spawn(move || {
                let stream = audio::start_capture(st);
                if stream.is_none() {
                    eprintln!("kRapper: no input device — running in silent demo mode.");
                }
                loop {
                    std::thread::park();
                }
            });
        }

        let (dict, dict_path) = match std::fs::read_to_string("data/cmudict-0.7b.txt") {
            Ok(raw) => (Dict::load(&raw), Some("data/cmudict-0.7b.txt".to_string())),
            Err(_) => (Dict::load(STARTER_CMUDICT), None),
        };

        // Vocabs live on disk in ./vocabs/. On first run, write the embedded
        // defaults so the user has something to edit. Then scan the directory
        // and load whatever is there — including any new profiles the user
        // has added since.
        let vocabs_dir = std::path::PathBuf::from("vocabs");
        let _ = std::fs::create_dir_all(&vocabs_dir);
        for (name, embedded) in [
            ("Juice WRLD", WRLD_VOCAB),
            ("The Weeknd", WEEKND_VOCAB),
            ("Common", COMMON_VOCAB),
        ] {
            let path = vocabs_dir.join(format!("{name}.txt"));
            if !path.exists() {
                let _ = std::fs::write(&path, embedded);
            }
        }
        let vocabs = load_vocabs_from_dir(&vocabs_dir, &dict);
        let vocab_names: Vec<String> = vocabs.iter().map(|v| v.name.clone()).collect();

        // Try to restore from disk. Failure means first run — fall through to
        // defaults and seed the editor with a tiny welcome line so the
        // scansion chips have something to render against immediately.
        let restored = session::load();
        let is_first_run = restored.is_none();
        let (mut vibe, bpm_input, syll_input, stress_input, seed_input, initial_lyrics) =
            match restored {
                Some(s) => (
                    s.vibe,
                    s.bpm_input,
                    s.syll_input,
                    s.stress_input,
                    s.seed_input,
                    s.lyrics,
                ),
                None => {
                    let vibe = VibeSummary::default();
                    let bpm_input = format!("{}", vibe.bpm as u32);
                    let syll_input = format!("{}", vibe.syllables_per_bar);
                    let stress_input = vibe::render_stress_pattern(&vibe.stress_pattern);
                    (
                        vibe,
                        bpm_input,
                        syll_input,
                        stress_input,
                        String::new(),
                        // First-run welcome — short enough to read at a glance,
                        // long enough to show the scansion strip and counts.
                        "running from her love".to_string(),
                    )
                }
            };

        // Saved sessions can reference a vocab that no longer exists (renamed
        // / deleted on disk). Fall back to the first available so blurbs work.
        if !vocab_names.contains(&vibe.vocab_name) {
            if let Some(first) = vocab_names.first() {
                vibe.vocab_name = first.clone();
            }
        }

        let show_debug = std::env::var("KRAPPER_DEBUG")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);

        let status = match (&dict_path, is_first_run) {
            (Some(p), false) => format!("Loaded {} phoneme entries from {p}.", dict.len()),
            (None, false) => format!(
                "Loaded {} phoneme entries (starter set). Drop data/cmudict-0.7b.txt for the full ~134k.",
                dict.len()
            ),
            (_, true) => format!(
                "Welcome. Edit the sample line, set a rhyme seed, or just type. {} phoneme entries loaded.",
                dict.len()
            ),
        };

        let mut app = Self {
            audio,
            dict,
            dict_path,
            vocabs,
            vocab_names,
            vibe,
            bpm_input,
            syll_input,
            stress_input,
            seed_input,
            lyrics: text_editor::Content::with_text(&initial_lyrics),
            blurbs: Vec::new(),
            blurb_status: String::new(),
            detected_syllables: 0,
            is_recording: false,
            levels: vec![0.0; audio::HISTORY_BARS],
            smoothed_level: 0.0,
            armed: true,
            onsets: Vec::new(),
            current_pitch_hz: None,
            pitch_history: PitchHistogram::new(),
            scale_guess: None,
            sample_rate: 0,
            vocabs_dir,
            show_debug,
            status,
            session_dirty: false,
            last_edit_at: std::time::Instant::now(),
        };
        app.regenerate_blurbs();
        (app, iced::Task::none())
    }

    pub fn theme(&self) -> Theme {
        Theme::TokyoNight
    }

    pub fn subscription(&self) -> Subscription<Message> {
        iced::time::every(Duration::from_millis(50)).map(|_| Message::Tick)
    }

    pub fn update(&mut self, msg: Message) -> iced::Task<Message> {
        match msg {
            Message::Tick => {
                if let Ok(guard) = self.audio.lock() {
                    self.detected_syllables = guard.syllables.min(255) as u8;
                    self.is_recording = guard.is_recording;
                    self.levels = guard.levels.clone();
                    self.smoothed_level = guard.smoothed_level;
                    self.armed = guard.armed;
                    self.onsets = guard.onsets.clone();
                    self.current_pitch_hz = guard.current_pitch_hz;
                    self.sample_rate = guard.sample_rate;
                }
                // Feed any new pitch observation into the histogram. This is
                // sampled at the UI rate (~20 Hz), which is plenty since YIN
                // itself runs at ~10 Hz on the audio thread.
                if self.is_recording {
                    if let Some(hz) = self.current_pitch_hz {
                        self.pitch_history.observe(hz);
                    }
                    self.scale_guess = scale::guess_scale(&self.pitch_history);
                }
                // Debounced session save: flush only after the user has stopped
                // editing for >300ms.
                if self.session_dirty
                    && self.last_edit_at.elapsed() > Duration::from_millis(300)
                {
                    self.persist_session();
                    self.session_dirty = false;
                }
            }
            Message::BpmChanged(s) => {
                self.bpm_input = s;
                if let Ok(n) = self.bpm_input.trim().parse::<f32>() {
                    if (20.0..=300.0).contains(&n) {
                        self.vibe.bpm = n;
                    }
                }
                self.mark_dirty();
                // BPM doesn't affect blurbs — skip regen.
            }
            Message::SyllChanged(s) => {
                if s.chars().all(|c| c.is_ascii_digit()) && s.len() <= 2 {
                    self.syll_input = s;
                    if let Ok(n) = self.syll_input.trim().parse::<u8>() {
                        if (1..=32).contains(&n) {
                            self.vibe.syllables_per_bar = n;
                            self.regenerate_blurbs();
                        }
                    }
                    self.mark_dirty();
                }
            }
            Message::StressChanged(s) => {
                self.stress_input = s;
                let parsed = vibe::parse_stress_pattern(&self.stress_input);
                if !parsed.is_empty() {
                    self.vibe.stress_pattern = parsed;
                    self.regenerate_blurbs();
                }
                self.mark_dirty();
            }
            Message::SeedChanged(s) => {
                self.seed_input = s;
                let seed = self.seed_input.trim();
                self.vibe.rhyme_seeds = if seed.is_empty() {
                    vec![]
                } else {
                    vec![seed.to_string()]
                };
                self.regenerate_blurbs();
                self.mark_dirty();
            }
            Message::VocabSelected(name) => {
                self.vibe.vocab_name = name;
                self.regenerate_blurbs();
                self.mark_dirty();
            }
            Message::Lyrics(action) => {
                let is_edit = matches!(action, text_editor::Action::Edit(_));
                // Scroll doesn't move the cursor or change the text, so the
                // auto-rhyme / line-completion targets are unchanged. Regenerating
                // would just re-roll the random sampler — looks like a UI glitch.
                let is_scroll = matches!(action, text_editor::Action::Scroll { .. });
                self.lyrics.perform(action);
                if !is_scroll {
                    self.regenerate_blurbs();
                }
                if is_edit {
                    self.mark_dirty();
                }
            }
            Message::RefreshBlurbs => {
                self.regenerate_blurbs();
            }
            Message::InsertBlurb(idx) => {
                if let Some(b) = self.blurbs.get(idx) {
                    let mut to_insert = b.as_text();
                    // Add a trailing space so successive inserts don't smash together.
                    to_insert.push(' ');
                    self.lyrics.perform(text_editor::Action::Edit(
                        text_editor::Edit::Paste(Arc::new(to_insert)),
                    ));
                    self.regenerate_blurbs();
                    self.mark_dirty();
                }
            }
            Message::ToggleRecord => {
                if let Ok(mut guard) = self.audio.lock() {
                    if guard.is_recording {
                        guard.is_recording = false;
                        self.status = format!("Mic off. Captured {} onsets.", guard.syllables);
                    } else {
                        guard.reset();
                        guard.is_recording = true;
                        self.status = "Mic hot — catching fragments.".into();
                    }
                }
            }
            Message::ResetMic => {
                if let Ok(mut guard) = self.audio.lock() {
                    guard.reset();
                }
                self.pitch_history.clear();
                self.scale_guess = None;
                self.status = "Mic counter and pitch history cleared.".into();
            }
            Message::AppendFileToVocab => {
                self.handle_append_file();
            }
            Message::NewVocabFromFile => {
                self.handle_new_vocab_from_file();
            }
            Message::ToggleDebug => {
                self.show_debug = !self.show_debug;
            }
            Message::BackToBeatsStub => {
                self.status = "Back to beats: Path A — see docs/path-a-plan.md.".into();
            }
        }
        iced::Task::none()
    }

    fn current_vocab(&self) -> Option<&Vocab> {
        self.vocabs.iter().find(|v| v.name == self.vibe.vocab_name)
    }

    /// File picker → append the file's contents into the currently selected
    /// vocab, save the merged result to disk, and refresh blurbs.
    fn handle_append_file(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Text / lyrics", &["txt"])
            .pick_file()
        else {
            self.status = "Append cancelled.".into();
            return;
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                self.status = format!("Couldn't read {}: {e}", path.display());
                return;
            }
        };
        let dict = self.dict.clone();
        let vocab_name = self.vibe.vocab_name.clone();
        let vocabs_dir = self.vocabs_dir.clone();

        // Do the vocab mutation inside a tight scope so the &mut borrow is
        // released before we touch self.status / self.regenerate_blurbs.
        let result: Option<(usize, usize, std::io::Result<()>)> = {
            if let Some(vocab) = self.vocabs.iter_mut().find(|v| v.name == vocab_name) {
                let before = vocab.words.len();
                vocab.extend_with_text(&raw, &dict);
                let total = vocab.words.len();
                let added = total - before;
                if vocab.path.is_none() {
                    vocab.path = Some(vocabs_dir.join(format!("{vocab_name}.txt")));
                }
                Some((added, total, vocab.save_to_disk()))
            } else {
                None
            }
        };

        self.status = match result {
            None => "No active vocab to append into.".into(),
            Some((added, total, Ok(()))) => format!(
                "Appended {added} new word{} into \"{vocab_name}\" ({total} total).",
                if added == 1 { "" } else { "s" }
            ),
            Some((_, _, Err(e))) => format!(
                "Loaded into memory but couldn't save \"{vocab_name}\" to disk: {e}"
            ),
        };
        self.regenerate_blurbs();
    }

    /// File picker → create a new vocab whose name is the file's stem, save it
    /// to `./vocabs/<stem>.txt`, add to the dropdown, and switch to it.
    fn handle_new_vocab_from_file(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Text / lyrics", &["txt"])
            .pick_file()
        else {
            self.status = "New profile cancelled.".into();
            return;
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("New profile")
            .to_string();

        // Reject names that collide with an existing vocab — avoid surprise
        // overwrites. User can rename the file (or use Append) instead.
        if self.vocabs.iter().any(|v| v.name == stem) {
            self.status = format!(
                "A vocab named \"{stem}\" already exists. Rename the file (or use Append) instead."
            );
            return;
        }

        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                self.status = format!("Couldn't read {}: {e}", path.display());
                return;
            }
        };

        let target = self.vocabs_dir.join(format!("{stem}.txt"));
        let mut new_vocab = Vocab::load_from_str(&stem, &raw, &self.dict);
        new_vocab.path = Some(target.clone());
        let save_result = new_vocab.save_to_disk();
        let count = new_vocab.words.len();
        self.vocabs.push(new_vocab);
        self.vocab_names.push(stem.clone());
        self.vibe.vocab_name = stem.clone();
        self.status = match save_result {
            Ok(()) => format!(
                "Created \"{stem}\" with {count} words. Saved to {}.",
                target.display()
            ),
            Err(e) => format!("Created \"{stem}\" in memory but disk save failed: {e}"),
        };
        self.regenerate_blurbs();
        self.mark_dirty();
    }

    /// Mark the session dirty for the debounced flush. Updates `last_edit_at`
    /// so the flush waits another idle window before writing.
    fn mark_dirty(&mut self) {
        self.session_dirty = true;
        self.last_edit_at = std::time::Instant::now();
    }

    /// Write the current session to disk. Best-effort — log on failure, never
    /// crash the app over it.
    fn persist_session(&self) {
        let snapshot = Session::from_vibe(self.vibe.clone(), self.lyrics.text());
        if let Err(e) = session::save(&snapshot) {
            eprintln!("kRapper: session save failed: {e}");
        }
    }

    /// Compute the effective rhyme seed: explicit manual input wins; otherwise
    /// auto-rhyme with the last word of the line *above* the cursor.
    /// Returns the seed plus a label describing where it came from.
    fn effective_rhyme_seed(&self) -> (String, &'static str) {
        let manual = self.seed_input.trim().to_string();
        if !manual.is_empty() {
            return (manual, "manual");
        }
        let text = self.lyrics.text();
        let lines: Vec<&str> = text.lines().collect();
        let (cursor_line, _) = self.lyrics.cursor_position();
        if cursor_line == 0 {
            return (String::new(), "none");
        }
        let prev = lines.get(cursor_line - 1).copied().unwrap_or("");
        let last = prev
            .split_whitespace()
            .last()
            .map(|w| w.trim_matches(|c: char| !c.is_alphabetic()).to_lowercase())
            .filter(|w| !w.is_empty())
            .unwrap_or_default();
        if last.is_empty() {
            (String::new(), "none")
        } else {
            (last, "auto")
        }
    }

    /// The stress pattern to target on the next blurb run.
    /// If the current line has unfilled syllables, target *just* those —
    /// completing the line. Otherwise target the whole bar (starting fresh).
    /// Returns `(pattern, descriptor)` where descriptor explains in the panel.
    fn target_pattern(&self) -> (Vec<u8>, String) {
        let full = self.vibe.full_stress();
        let text = self.lyrics.text();
        let lines: Vec<&str> = text.lines().collect();
        let (cursor_line, _) = self.lyrics.cursor_position();
        let current = lines.get(cursor_line).copied().unwrap_or("");
        let current_syll = count_syllables_in_line(current, &self.dict);
        let per_bar = self.vibe.syllables_per_bar as u32;

        if current_syll > 0 && current_syll < per_bar {
            // Mid-line: target the tail of the full pattern that hasn't been
            // covered yet, so the suggested fragment lands the bar correctly.
            let remaining = (per_bar - current_syll) as usize;
            let start = full.len().saturating_sub(remaining);
            let pat = full[start..].to_vec();
            let desc = format!(
                "completing line — {} more syll, {}",
                pat.len(),
                vibe::render_stress_pattern(&pat)
            );
            (pat, desc)
        } else {
            let desc = format!(
                "full bar — {} syll, {}",
                full.len(),
                vibe::render_stress_pattern(&full)
            );
            (full, desc)
        }
    }

    fn regenerate_blurbs(&mut self) {
        let Some(vocab) = self.current_vocab() else {
            self.blurbs.clear();
            return;
        };
        let (target, target_desc) = self.target_pattern();
        let (seed, seed_source) = self.effective_rhyme_seed();
        self.blurbs = suggest_blurbs(vocab, &self.dict, &target, &seed, 6);

        let seed_part = match (seed.is_empty(), seed_source) {
            (true, _) => "no rhyme".to_string(),
            (_, "manual") => format!("rhyming \"{seed}\" (manual)"),
            (_, "auto") => format!("rhyming \"{seed}\" (auto from line above)"),
            _ => format!("rhyming \"{seed}\""),
        };
        self.blurb_status = if self.blurbs.is_empty() {
            // The user is more likely confused than they are wrong. Surface
            // the actual cause: low dict coverage, seed not in dict, or just
            // an unlucky stress pattern.
            let mut why = format!("No blurbs for {target_desc}, {seed_part}.");
            if self.dict_path.is_none() {
                why.push_str(
                    "  Likely the starter dict is too small — see data/README.md to install full CMUdict.",
                );
            } else if !seed.is_empty() && !self.dict.contains(&seed) {
                why.push_str(&format!("  \"{seed}\" isn't in the dict — no phoneme rhymes possible."));
            }
            why
        } else {
            format!(
                "{} blurb{} · {target_desc} · {seed_part}",
                self.blurbs.len(),
                if self.blurbs.len() == 1 { "" } else { "s" }
            )
        };
    }

    pub fn view(&self) -> Element<'_, Message> {
        let header = self.view_header();
        let vibe_panel = self.view_vibe_inputs();
        let main = self.view_main_area();
        let mic_strip = self.view_mic_strip();
        let pitch_strip = self.view_pitch_strip();
        let status_line = container(
            text(self.status.as_str())
                .size(12)
                .color(Color::from_rgb(0.6, 0.6, 0.7)),
        )
        .padding(4);

        let mut body = column![
            header,
            Space::with_height(Length::Fixed(10.0)),
            vibe_panel,
            Space::with_height(Length::Fixed(10.0)),
            main,
            Space::with_height(Length::Fixed(8.0)),
            mic_strip,
            Space::with_height(Length::Fixed(6.0)),
            pitch_strip,
        ]
        .spacing(0);
        if self.show_debug {
            body = body.push(Space::with_height(Length::Fixed(8.0)));
            body = body.push(self.view_debug_panel());
        }
        body = body.push(status_line);

        container(body).padding(22).into()
    }

    // -------- view sections -------------------------------------------------

    fn view_header(&self) -> Element<'_, Message> {
        let title = text("kRapper · Scratchpad")
            .size(28)
            .color(Color::from_rgb(0.92, 0.78, 0.42));
        let tagline = text("write to your vibe. mic stays hot.")
            .size(12)
            .color(Color::from_rgb(0.6, 0.6, 0.7));

        let back_btn = button(text("Back to beats (Path A)").size(12))
            .on_press(Message::BackToBeatsStub)
            .padding(8);
        let debug_btn = button(
            text(if self.show_debug { "Debug ▴" } else { "Debug ▾" }).size(12),
        )
        .on_press(Message::ToggleDebug)
        .padding(8);

        row![
            column![title, tagline].spacing(2),
            Space::with_width(Length::Fill),
            back_btn,
            Space::with_width(Length::Fixed(6.0)),
            debug_btn,
        ]
        .align_y(iced::Alignment::Center)
        .into()
    }

    fn view_vibe_inputs(&self) -> Element<'_, Message> {
        let bpm = labeled_input(
            "BPM",
            text_input("80", &self.bpm_input)
                .on_input(Message::BpmChanged)
                .padding(6)
                .width(Length::Fixed(70.0))
                .into(),
        );
        let syll = labeled_input(
            "Syll / bar",
            text_input("8", &self.syll_input)
                .on_input(Message::SyllChanged)
                .padding(6)
                .width(Length::Fixed(58.0))
                .into(),
        );
        let stress = labeled_input(
            "Stress (x . repeats)",
            text_input("x.x.x.x.", &self.stress_input)
                .on_input(Message::StressChanged)
                .padding(6)
                .width(Length::Fixed(160.0))
                .into(),
        );
        let seed = labeled_input(
            "Rhyme seed",
            text_input("e.g. curse", &self.seed_input)
                .on_input(Message::SeedChanged)
                .padding(6)
                .width(Length::Fixed(140.0))
                .into(),
        );
        let vocab_picker = pick_list(
            self.vocab_names.clone(),
            Some(self.vibe.vocab_name.clone()),
            Message::VocabSelected,
        )
        .width(Length::Fixed(140.0));
        let vocab_with_actions = column![
            vocab_picker,
            Space::with_height(Length::Fixed(4.0)),
            row![
                button(text("+ Append").size(10))
                    .on_press(Message::AppendFileToVocab)
                    .padding(4),
                Space::with_width(Length::Fixed(4.0)),
                button(text("+ New").size(10))
                    .on_press(Message::NewVocabFromFile)
                    .padding(4),
            ]
            .spacing(0),
        ]
        .spacing(0);
        let vocab = labeled_input("Vocab", vocab_with_actions.into());

        container(
            column![
                text("Vibe inputs")
                    .size(13)
                    .color(Color::from_rgb(0.85, 0.85, 0.95)),
                Space::with_height(Length::Fixed(8.0)),
                row![bpm, syll, stress, seed, vocab]
                    .spacing(14)
                    .align_y(iced::Alignment::End),
                Space::with_height(Length::Fixed(4.0)),
                text("(Filled in by hand for Path B — Path A will produce this from your hum.)")
                    .size(10)
                    .color(Color::from_rgb(0.5, 0.5, 0.6)),
            ]
            .spacing(0),
        )
        .padding(14)
        .style(panel_style)
        .width(Length::Fill)
        .into()
    }

    fn view_main_area(&self) -> Element<'_, Message> {
        let editor = text_editor(&self.lyrics)
            .on_action(Message::Lyrics)
            .padding(10)
            .height(Length::Fill);

        let (line_idx, col_idx) = self.lyrics.cursor_position();
        let text_str = self.lyrics.text();
        let lines: Vec<&str> = text_str.lines().collect();
        let total_syll: u32 = lines
            .iter()
            .map(|l| count_syllables_in_line(l, &self.dict))
            .sum();
        let current_line_text = lines.get(line_idx).copied().unwrap_or("");
        let current_line_syll = count_syllables_in_line(current_line_text, &self.dict);
        let target_per_bar = self.vibe.syllables_per_bar as u32;
        let line_chip_color = if current_line_syll == target_per_bar {
            Color::from_rgb(0.55, 0.85, 0.55)
        } else if current_line_syll > target_per_bar {
            Color::from_rgb(0.95, 0.65, 0.35)
        } else {
            Color::from_rgb(0.6, 0.6, 0.7)
        };

        // Cursor indicator — "blurbs will paste here." The accent color matches
        // the title so it reads as the same active-anchor concept.
        let cursor_indicator = text(format!("▌ L{}·C{}", line_idx + 1, col_idx + 1))
            .size(11)
            .color(Color::from_rgb(0.92, 0.78, 0.42));

        let counts_line = row![
            cursor_indicator,
            Space::with_width(Length::Fixed(12.0)),
            text(format!("{current_line_syll}/{target_per_bar} syll"))
                .size(11)
                .color(line_chip_color),
            Space::with_width(Length::Fixed(12.0)),
            text(format!("Total: {total_syll} syll · {} lines", lines.len()))
                .size(11)
                .color(Color::from_rgb(0.6, 0.6, 0.7)),
        ]
        .align_y(iced::Alignment::Center);

        // Scansion strip: per-word stress for the current line, colored by
        // alignment with the target bar pattern.
        let scansion = self.view_scansion_strip(current_line_text);

        let lyrics_box = container(
            column![
                text("Lyrics").size(13).color(Color::from_rgb(0.85, 0.85, 0.95)),
                Space::with_height(Length::Fixed(6.0)),
                editor,
                Space::with_height(Length::Fixed(8.0)),
                scansion,
                Space::with_height(Length::Fixed(4.0)),
                counts_line,
            ]
            .spacing(0),
        )
        .padding(14)
        .style(panel_style)
        .height(Length::Fixed(360.0))
        .width(Length::FillPortion(60));

        // --- blurbs panel ---
        let mut blurbs_col = column![].spacing(6);
        if self.blurbs.is_empty() {
            blurbs_col = blurbs_col.push(
                text("No blurbs yet. Try a shorter stress pattern, or drop the rhyme seed.")
                    .size(12)
                    .color(Color::from_rgb(0.55, 0.55, 0.65)),
            );
        } else {
            for (i, b) in self.blurbs.iter().enumerate() {
                blurbs_col = blurbs_col.push(view_blurb_row(i, b, self.show_debug));
            }
        }
        let blurbs_box = container(
            column![
                row![
                    text("Blurbs")
                        .size(13)
                        .color(Color::from_rgb(0.85, 0.85, 0.95)),
                    Space::with_width(Length::Fill),
                    button(text("Refresh").size(11))
                        .on_press(Message::RefreshBlurbs)
                        .padding(6),
                ]
                .align_y(iced::Alignment::Center),
                Space::with_height(Length::Fixed(4.0)),
                text(self.blurb_status.as_str())
                    .size(10)
                    .color(Color::from_rgb(0.55, 0.55, 0.65)),
                Space::with_height(Length::Fixed(8.0)),
                scrollable(blurbs_col).height(Length::Fill),
            ]
            .spacing(0),
        )
        .padding(14)
        .style(panel_style)
        .height(Length::Fixed(360.0))
        .width(Length::FillPortion(40));

        row![lyrics_box, Space::with_width(Length::Fixed(12.0)), blurbs_box]
            .height(Length::Fixed(360.0))
            .into()
    }

    /// Render the current line as a row of word chips, each annotated with its
    /// per-syllable stress and colored by alignment with the target bar pattern.
    /// Words not in the dict fall back to a "first syllable stressed" heuristic
    /// and are marked with a `?` so the user knows the scansion is a guess.
    fn view_scansion_strip(&self, line: &str) -> Element<'_, Message> {
        let target = self.vibe.full_stress();
        let words: Vec<&str> = line.split_whitespace().collect();

        if words.is_empty() {
            return text(format!(
                "scansion: empty line · target {}",
                vibe::render_stress_pattern(&target)
            ))
            .size(10)
            .color(Color::from_rgb(0.55, 0.55, 0.65))
            .into();
        }

        let mut chip_row: Vec<Element<'_, Message>> = Vec::new();
        let mut pos = 0usize;
        for w in &words {
            // Clean the token for dict lookup.
            let key: String = w
                .chars()
                .filter(|c| c.is_alphabetic() || *c == '\'')
                .collect::<String>()
                .to_lowercase();
            let (stress, in_dict): (Vec<u8>, bool) = match self.dict.stress_pattern(&key) {
                Some(s) if !s.is_empty() => (s, true),
                _ => {
                    let n = syllable_count(&self.dict, &key) as usize;
                    let mut p = vec![0u8; n.max(1)];
                    p[0] = 1; // heuristic: first syllable carries the stress
                    (p, false)
                }
            };

            let mut dot_row: Vec<Element<'_, Message>> = Vec::new();
            for (i, &s) in stress.iter().enumerate() {
                let target_at = target.get(pos + i).copied();
                let aligned = target_at == Some(s);
                let glyph = if s == 1 { "●" } else { "○" };
                let color = match (target_at, aligned) {
                    (None, _) => Color::from_rgb(0.55, 0.55, 0.65),       // past end of bar
                    (Some(_), true) => Color::from_rgb(0.40, 0.80, 0.55), // aligned: green
                    (Some(_), false) => Color::from_rgb(0.95, 0.55, 0.35), // misaligned: orange
                };
                dot_row.push(text(glyph).size(12).color(color).into());
                if i + 1 < stress.len() {
                    dot_row.push(Space::with_width(Length::Fixed(2.0)).into());
                }
            }
            let chip_inner = column![
                text(if in_dict {
                    w.to_string()
                } else {
                    format!("{w}?")
                })
                .size(11),
                Space::with_height(Length::Fixed(2.0)),
                iced::widget::Row::with_children(dot_row).align_y(iced::Alignment::Center),
            ]
            .align_x(iced::Alignment::Center)
            .spacing(0);
            chip_row.push(
                container(chip_inner)
                    .padding(4)
                    .style(chip_style)
                    .into(),
            );
            chip_row.push(Space::with_width(Length::Fixed(4.0)).into());
            pos += stress.len();
        }

        let target_line = text(format!(
            "target: {}{}",
            vibe::render_stress_pattern(&target),
            if pos > target.len() {
                format!("  · over by {}", pos - target.len())
            } else if pos < target.len() {
                format!("  · {} syll to go", target.len() - pos)
            } else {
                "  · fits the bar".to_string()
            }
        ))
        .size(10)
        .color(Color::from_rgb(0.55, 0.55, 0.65));

        column![
            scrollable(
                iced::widget::Row::with_children(chip_row)
                    .align_y(iced::Alignment::Center)
            )
            .direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::default()
            )),
            Space::with_height(Length::Fixed(2.0)),
            target_line,
        ]
        .spacing(0)
        .into()
    }

    fn view_pitch_strip(&self) -> Element<'_, Message> {
        let pitch_text = match self.current_pitch_hz {
            Some(hz) => {
                let (note, octave, cents) = scale::freq_to_note(hz);
                let cents_str = if cents == 0 {
                    "±0¢".to_string()
                } else if cents > 0 {
                    format!("+{cents}¢")
                } else {
                    format!("{cents}¢")
                };
                format!("♪ {note}{octave}  {cents_str}  ·  {hz:.1} Hz")
            }
            None => {
                if self.is_recording {
                    "♪ — listening".to_string()
                } else {
                    "♪ — arm mic to listen".to_string()
                }
            }
        };

        let scale_text = match &self.scale_guess {
            Some(g) if g.score > 0.2 => {
                let confidence = if g.margin > 0.15 {
                    "strong"
                } else if g.margin > 0.05 {
                    "ok"
                } else {
                    "fuzzy"
                };
                format!(
                    "scale: {} {}  ({confidence}, score {:.2})",
                    g.root_name, g.scale_name, g.score
                )
            }
            Some(_) => "scale: undetermined".into(),
            None => "scale: need more notes".into(),
        };

        // 12-cell pitch class strip — current pitch class gets the accent;
        // in-scale classes get a brighter histogram-weighted background; out-
        // of-scale stay dim.
        let current_class = self
            .current_pitch_hz
            .map(|hz| {
                let midi = scale::freq_to_midi(hz);
                ((midi.round() as i32 % 12) + 12) % 12
            })
            .unwrap_or(-1);
        let max_count = self
            .pitch_history
            .counts
            .iter()
            .cloned()
            .fold(0.0_f32, f32::max)
            .max(1.0);

        let in_scale_mask: [bool; 12] = match &self.scale_guess {
            Some(g) => {
                let root_idx = scale::NOTE_NAMES
                    .iter()
                    .position(|n| *n == g.root_name)
                    .unwrap_or(0);
                let pattern = scale::SCALES
                    .iter()
                    .find(|(name, _)| *name == g.scale_name)
                    .map(|(_, p)| *p)
                    .unwrap_or(&scale::MAJOR);
                let mut mask = [false; 12];
                for i in 0..12 {
                    let pos = (i + 12 - root_idx) % 12;
                    mask[i] = pattern[pos] == 1;
                }
                mask
            }
            None => [false; 12],
        };

        let mut cells: Vec<Element<'_, Message>> = Vec::new();
        for i in 0..12 {
            let count = self.pitch_history.counts[i];
            let strength = (count / max_count).clamp(0.0, 1.0);
            let is_current = current_class as usize == i;
            let in_scale = in_scale_mask[i];

            let bg_color = if is_current {
                Color::from_rgb(0.95, 0.40, 0.40)
            } else if in_scale {
                Color {
                    r: 0.40,
                    g: 0.80,
                    b: 0.55,
                    a: 0.25 + 0.75 * strength,
                }
            } else {
                Color {
                    r: 0.55,
                    g: 0.55,
                    b: 0.65,
                    a: 0.18 + 0.40 * strength,
                }
            };

            let text_color = if is_current {
                Color::from_rgb(1.0, 1.0, 1.0)
            } else {
                Color::from_rgb(0.85, 0.85, 0.95)
            };

            cells.push(
                container(text(scale::NOTE_NAMES[i]).size(10).color(text_color))
                    .padding(4)
                    .style(move |_theme: &Theme| container::Style {
                        background: Some(bg_color.into()),
                        border: iced::Border {
                            radius: 4.0.into(),
                            ..Default::default()
                        },
                        ..container::Style::default()
                    })
                    .width(Length::Fixed(28.0))
                    .into(),
            );
            cells.push(Space::with_width(Length::Fixed(2.0)).into());
        }
        let pitch_classes =
            iced::widget::Row::with_children(cells).align_y(iced::Alignment::Center);

        container(
            column![
                row![
                    text(pitch_text)
                        .size(13)
                        .color(Color::from_rgb(0.92, 0.78, 0.42)),
                    Space::with_width(Length::Fixed(20.0)),
                    text(scale_text)
                        .size(11)
                        .color(Color::from_rgb(0.75, 0.75, 0.85)),
                ]
                .align_y(iced::Alignment::Center),
                Space::with_height(Length::Fixed(6.0)),
                pitch_classes,
            ]
            .spacing(0),
        )
        .padding(10)
        .style(panel_style)
        .width(Length::Fill)
        .into()
    }

    fn view_mic_strip(&self) -> Element<'_, Message> {
        let dot_color = if self.is_recording {
            Color::from_rgb(0.95, 0.40, 0.40)
        } else {
            Color::from_rgb(0.45, 0.45, 0.55)
        };
        let dot = container(text("●").size(14).color(dot_color)).padding(4);

        let label = text(if self.is_recording {
            "MIC HOT"
        } else {
            "mic off"
        })
        .size(11)
        .color(dot_color);

        let toggle_btn = button(
            text(if self.is_recording {
                "■ Stop"
            } else {
                "● Arm mic"
            })
            .size(11),
        )
        .on_press(Message::ToggleRecord)
        .padding(6);
        let reset_btn = button(text("Reset").size(11))
            .on_press(Message::ResetMic)
            .padding(6);

        let bars = Canvas::new(Bars {
            levels: self.levels.clone(),
            recording: self.is_recording,
        })
        .width(Length::Fill)
        .height(Length::Fixed(34.0));

        let onsets_label = text(format!(
            "{} onset{} · {} syll",
            self.onsets.len(),
            if self.onsets.len() == 1 { "" } else { "s" },
            self.detected_syllables
        ))
        .size(11)
        .color(Color::from_rgb(0.6, 0.6, 0.7));

        container(
            row![
                dot,
                label,
                Space::with_width(Length::Fixed(12.0)),
                toggle_btn,
                Space::with_width(Length::Fixed(6.0)),
                reset_btn,
                Space::with_width(Length::Fixed(14.0)),
                container(bars).width(Length::FillPortion(70)),
                Space::with_width(Length::Fixed(12.0)),
                onsets_label,
            ]
            .align_y(iced::Alignment::Center)
            .spacing(0),
        )
        .padding(10)
        .style(panel_style)
        .width(Length::Fill)
        .into()
    }

    fn view_debug_panel(&self) -> Element<'_, Message> {
        let armed_color = if self.armed {
            Color::from_rgb(0.40, 0.85, 0.55)
        } else {
            Color::from_rgb(0.95, 0.65, 0.35)
        };

        let level_meter = Canvas::new(LevelMeter {
            level: self.smoothed_level,
            low: ONSET_LOW,
            high: ONSET_HIGH,
            armed: self.armed,
        })
        .width(Length::Fill)
        .height(Length::Fixed(22.0));

        let audio_block = column![
            text("audio thread").size(12).color(Color::from_rgb(0.75, 0.75, 0.85)),
            row![
                text(format!("smoothed RMS: {:.4}", self.smoothed_level)).size(10),
                Space::with_width(Length::Fixed(12.0)),
                text(format!("HIGH={ONSET_HIGH:.2}  LOW={ONSET_LOW:.2}")).size(10),
                Space::with_width(Length::Fixed(12.0)),
                text(if self.armed { "armed ✓" } else { "armed ✗" })
                    .size(10)
                    .color(armed_color),
            ]
            .align_y(iced::Alignment::Center),
            Space::with_height(Length::Fixed(3.0)),
            level_meter,
        ]
        .spacing(2);

        let dict_line = match &self.dict_path {
            Some(p) => format!("dict: {p} ({} entries)", self.dict.len()),
            None => format!(
                "dict: embedded starter ({} entries)",
                self.dict.len()
            ),
        };

        let vibe_line = format!(
            "vibe: {} BPM, {} syll/bar, stress {}, seed {}",
            self.vibe.bpm as u32,
            self.vibe.syllables_per_bar,
            vibe::render_stress_pattern(&self.vibe.stress_pattern),
            if self.vibe.rhyme_seeds.is_empty() {
                "(none)".to_string()
            } else {
                format!("\"{}\"", self.vibe.rhyme_seeds[0])
            }
        );
        let source_line = match &self.vibe.source {
            VibeSource::Manual => "source: manual (Path B)".to_string(),
            VibeSource::Recording { audio_path } => format!("source: recording @ {audio_path}"),
        };

        let vocab_line = match self.current_vocab() {
            None => "vocab: none".to_string(),
            Some(v) => format!(
                "vocab: {} — {} words, {} in dict, {} distinct stress patterns",
                v.name,
                v.words.len(),
                v.dict_coverage,
                v.by_stress.len()
            ),
        };

        container(
            column![
                text("debug").size(13).color(Color::from_rgb(0.85, 0.85, 0.95)),
                Space::with_height(Length::Fixed(6.0)),
                audio_block,
                Space::with_height(Length::Fixed(8.0)),
                text(dict_line).size(10).color(Color::from_rgb(0.7, 0.7, 0.8)),
                text(vibe_line).size(10).color(Color::from_rgb(0.7, 0.7, 0.8)),
                text(source_line).size(10).color(Color::from_rgb(0.7, 0.7, 0.8)),
                text(vocab_line).size(10).color(Color::from_rgb(0.7, 0.7, 0.8)),
            ]
            .spacing(2),
        )
        .padding(12)
        .style(debug_panel_style)
        .width(Length::Fill)
        .into()
    }
}

// --- helpers ----------------------------------------------------------------

fn labeled_input<'a>(label: &'static str, widget: Element<'a, Message>) -> Element<'a, Message> {
    column![
        text(label).size(10).color(Color::from_rgb(0.55, 0.55, 0.65)),
        widget,
    ]
    .spacing(3)
    .into()
}

/// Scan `dir` for `*.txt` files and load each as a `Vocab`. Returns them
/// sorted by name for stable dropdown ordering.
fn load_vocabs_from_dir(dir: &std::path::Path, dict: &Dict) -> Vec<Vocab> {
    let mut out: Vec<Vocab> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Ok(v) = Vocab::load_from_file(stem, &path, dict) {
            out.push(v);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn count_syllables_in_line(line: &str, dict: &Dict) -> u32 {
    line.split_whitespace()
        .map(|w| syllable_count(dict, w) as u32)
        .sum()
}

fn view_blurb_row(idx: usize, b: &Blurb, show_debug: bool) -> Element<'_, Message> {
    let mut items: Vec<Element<'_, Message>> = vec![
        button(text(b.as_text()).size(13))
            .on_press(Message::InsertBlurb(idx))
            .padding(6)
            .into(),
    ];
    if show_debug {
        items.push(Space::with_width(Length::Fixed(8.0)).into());
        for (w, s) in &b.words {
            items.push(
                container(text(format!("{w}·{s}")).size(9))
                    .padding(2)
                    .style(chip_style)
                    .into(),
            );
            items.push(Space::with_width(Length::Fixed(3.0)).into());
        }
        if b.rhymes_seed {
            items.push(
                container(text("rhymes").size(9))
                    .padding(2)
                    .style(tag_style)
                    .into(),
            );
        }
    }
    container(
        iced::widget::Row::with_children(items)
            .align_y(iced::Alignment::Center)
            .spacing(0),
    )
    .padding(2)
    .into()
}

// --- canvas widgets ---------------------------------------------------------

struct Bars {
    levels: Vec<f32>,
    recording: bool,
}

impl<Message> canvas::Program<Message, Theme, Renderer> for Bars {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let w = bounds.width;
        let h = bounds.height;
        if self.levels.is_empty() {
            return vec![frame.into_geometry()];
        }
        let palette = theme.extended_palette();
        let active = if self.recording {
            palette.danger.base.color
        } else {
            palette.primary.base.color
        };
        let baseline_color = Color { a: 0.22, ..active };

        let n = self.levels.len() as f32;
        let slot = w / n;
        let bar_w = (slot * 0.72).max(1.0);
        let pad = (slot - bar_w) * 0.5;

        let baseline = Path::line(Point::new(0.0, h - 1.5), Point::new(w, h - 1.5));
        frame.stroke(
            &baseline,
            Stroke::default().with_color(baseline_color).with_width(1.0),
        );
        for (i, &v) in self.levels.iter().enumerate() {
            let scaled = v.powf(0.65);
            let bh = (scaled * (h - 4.0)).max(2.0);
            let x = i as f32 * slot + pad;
            let y = h - bh - 1.5;
            frame.fill_rectangle(Point::new(x, y), Size::new(bar_w, bh), active);
        }
        vec![frame.into_geometry()]
    }
}

struct LevelMeter {
    level: f32,
    low: f32,
    high: f32,
    armed: bool,
}

impl<Message> canvas::Program<Message, Theme, Renderer> for LevelMeter {
    type State = ();
    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let w = bounds.width;
        let h = bounds.height;
        let palette = theme.extended_palette();
        let track = Color {
            a: 0.22,
            ..palette.primary.weak.color
        };
        let fill = if self.armed {
            palette.success.base.color
        } else {
            palette.danger.base.color
        };
        let scale = 0.20_f32;
        let level_x = (self.level / scale).clamp(0.0, 1.0) * w;
        let low_x = (self.low / scale).clamp(0.0, 1.0) * w;
        let high_x = (self.high / scale).clamp(0.0, 1.0) * w;

        frame.fill_rectangle(Point::new(0.0, h * 0.35), Size::new(w, h * 0.3), track);
        frame.fill_rectangle(Point::new(0.0, h * 0.35), Size::new(level_x, h * 0.3), fill);

        let tick = Color::from_rgb(0.9, 0.9, 0.95);
        frame.stroke(
            &Path::line(Point::new(low_x, 2.0), Point::new(low_x, h - 2.0)),
            Stroke::default().with_color(tick).with_width(1.0),
        );
        frame.stroke(
            &Path::line(Point::new(high_x, 2.0), Point::new(high_x, h - 2.0)),
            Stroke::default().with_color(tick).with_width(1.5),
        );
        vec![frame.into_geometry()]
    }
}

// --- container styles -------------------------------------------------------

fn panel_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.weak.color.into()),
        border: iced::Border {
            radius: 8.0.into(),
            width: 1.0,
            color: Color {
                a: 0.12,
                ..palette.primary.weak.color
            },
        },
        ..container::Style::default()
    }
}

fn chip_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.primary.weak.color.into()),
        text_color: Some(palette.primary.weak.text),
        border: iced::Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..container::Style::default()
    }
}

fn tag_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.success.base.color.into()),
        text_color: Some(palette.success.base.text),
        border: iced::Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..container::Style::default()
    }
}

fn debug_panel_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(
            Color {
                a: 0.55,
                ..palette.background.weak.color
            }
            .into(),
        ),
        border: iced::Border {
            radius: 8.0.into(),
            width: 1.0,
            color: Color {
                a: 0.35,
                ..palette.primary.base.color
            },
        },
        ..container::Style::default()
    }
}
