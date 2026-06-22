// src/ui/iced/mod.rs
//
// Iced UI for ambient-learner. Designed with mobile/touch primarily
// in mind: large hit targets, bottom-tab navigation, vertically-
// scrollable content. The desktop experience is fine too; we're
// just optimizing for the harder case.
//
// What's here:
//   * 4-page bottom-tab nav: Live / Model / History / About
//   * Live page: big temperature, confidence, calibrate button
//   * Model page: per-sensor breakdown
//   * History page: recent calibrations
//   * About page: app info + sensor metadata
//   * Calibrate dialog as a modal-style overlay
//
// What's deliberately NOT here:
//   * Custom per-widget styling. Iced 0.12 uses the `StyleSheet`
//     trait pattern for that, which means implementing a custom
//     trait per widget type per style — substantial boilerplate
//     for what's already the minimal backend. We stick with the
//     built-in `theme::Button` variants to indicate the active
//     tab and skip per-temperature color highlighting. The
//     temperature reading is the focal point through size and
//     position alone.
//   * Swipe gestures between pages. Building a swipe widget needs
//     custom `iced::Widget` impl on internals that change between
//     Iced versions; not worth the lift for what's the smaller
//     backend.
//   * Animations / mood theming. Iced has its own theming idioms;
//     we use the built-in Dark theme.
//   * Map / Locations / Sync pages. Qt remains the way to access
//     those.
//
// The `iced` crate is declared as `iced_dep` in Cargo.toml to avoid
// colliding with this local module's name. We alias it back here so
// the file reads naturally.

use ::iced_dep as iced;

use iced::mouse;
use iced::widget::{
    button, canvas, column, container, mouse_area, row, scrollable, text,
    text_input, Space,
};
use iced::{
    keyboard, theme, Alignment, Application, Color, Command, Element, Length,
    Padding, Point, Rectangle, Renderer, Settings, Subscription, Theme,
};
use std::time::{Duration, Instant};

/// How long a press must be held to count as a long-press
/// (vs. an accidental tap-and-release). 500 ms is the value used
/// by most mobile UI guidelines and matches what feels right when
/// you intentionally hold a finger down to "open this thing."
const LONG_PRESS_MS: u128 = 500;

use crate::systems::{CalibrationQueue, CalibrationRequest};
use crate::world_state::{self, WorldState};

/// One tab at the bottom of the screen. Adding a variant here is
/// also the way to add a page — the tab bar enumerates this enum
/// and the `view` dispatches on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Live,
    Model,
    Locations,
    Chart,
    About,
}

impl Tab {
    const ALL: &'static [Tab] = &[
        Tab::Live,
        Tab::Model,
        Tab::Locations,
        Tab::Chart,
        Tab::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Tab::Live      => "Live",
            Tab::Model     => "Model",
            Tab::Locations => "Places",
            Tab::Chart     => "Chart",
            Tab::About     => "About",
        }
    }

    fn icon(self) -> &'static str {
        // Emoji glyphs work on most Linux text rasterizers and let
        // us avoid bundling an icon font. Falls back gracefully on
        // older systems (the label is always visible).
        match self {
            Tab::Live      => "🌡",
            Tab::Model     => "🧠",
            Tab::Locations => "📍",
            Tab::Chart     => "📈",
            Tab::About     => "ℹ",
        }
    }

    /// Keyboard shortcut character for this tab. Listed in the
    /// About page so the user can discover them. Returns `None`
    /// for tabs without a shortcut (none currently — all tabs have
    /// one).
    fn shortcut(self) -> Option<char> {
        match self {
            Tab::Live      => Some('1'),
            Tab::Model     => Some('2'),
            Tab::Locations => Some('3'),
            Tab::Chart     => Some('4'),
            Tab::About     => Some('5'),
        }
    }
}

/// One row in the audit log list. Lightweight (just 4 fields), so
/// pulling 20 of these from WorldState per tick doesn't allocate
/// much.
#[derive(Clone, Debug)]
struct AuditEntry {
    target: f32,
    mean_sensors: f32,
    age_seconds: u64,
    source: Option<String>,
}

/// Local copy of the parts of WorldState we render. We pull these
/// once per tick under the read lock and stash them on the app so
/// the `view` function doesn't have to touch the lock — view runs
/// frequently and we want it to be allocation-light.
#[derive(Default, Clone)]
struct LocalSnapshot {
    temperature: f32,
    confidence: f32,
    model_type: String,
    calibration_count: usize,
    sensor_names: Vec<String>,
    sensor_readings: Vec<f32>,
    // (id, name, is_current)
    locations: Vec<(String, String, bool)>,
    /// Last N points of ambient temp. Sub-sampled to ~300 points
    /// max so we don't pass huge vecs to the Canvas every paint.
    ambient_series: Vec<f32>,
    /// Per-sensor recent series. Parallel to sensor_names; each
    /// vector is up to 300 points of recent readings. Used by
    /// the Model page's multi-sensor strip chart.
    sensor_series: Vec<Vec<f32>>,
    /// Recent calibration samples for the scatter plot: each pair
    /// is `(mean_sensor_reading, user_target)`. Iced renders these
    /// as dots on a 2D plot to show how well the sensor readings
    /// cluster around what the user says ambient should be.
    ///
    /// We use mean-of-sensors as the x-axis stand-in for "what
    /// the device thought" because the model's actual prediction
    /// at calibration time isn't stored (the model keeps
    /// learning, so its current prediction for an old sample is
    /// no longer the prediction that calibration was correcting).
    /// Mean-of-sensors is honest about what data is available.
    calibration_samples: Vec<(f32, f32)>,

    /// Audit-log projection of recent calibrations: each entry is
    /// `(target, mean_sensors, age_seconds, source)`. Same data
    /// source as `calibration_samples` above but shaped for the
    /// list view rather than the scatter. Newest first. Capped at
    /// 20 entries since the Iced list isn't virtualized — a longer
    /// list would just be wasted layout work below the fold.
    calibration_log: Vec<AuditEntry>,
}

pub struct AmbientLearnerApp {
    world: WorldState,
    calibration_queue: CalibrationQueue,

    snapshot: LocalSnapshot,

    current_tab: Tab,

    // Calibration dialog state
    calibration_input: String,
    show_calibration_dialog: bool,
    calibration_error: Option<String>,

    // Add-location dialog state
    location_input: String,
    show_location_dialog: bool,
    location_error: Option<String>,

    // Long-press tracking for the Model page sensor rows. We
    // record the press time per sensor index; if a release comes
    // in after ≥ LONG_PRESS_MS, we treat it as a long-press and
    // open the per-sensor detail card. Shorter presses are
    // ignored (the rest of the row isn't interactive on tap —
    // tap is reserved for the long-press gesture).
    sensor_press_started: Option<(usize, Instant)>,
    /// Index of the sensor whose detail card is currently
    /// expanded, if any. `None` means no detail open.
    expanded_sensor: Option<usize>,

    /// Substring filter applied to the sensor list on the Model
    /// tab. Empty string shows everything. Case-insensitive.
    /// We keep this on the app state (not a transient widget
    /// value) so it survives tab switches — useful if a user
    /// types a filter, hops to Live, and comes back.
    sensor_filter: String,

    /// User-selected theme. Iced ships a generous set of built-in
    /// themes; letting the user pick is two clicks in the About
    /// tab. We default to Dark because the rest of the app's
    /// color choices assume a dark canvas, but the picker covers
    /// the realistic light/dark/colorful range.
    selected_theme: Theme,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    SelectTab(Tab),
    OpenCalibrationDialog,
    CloseCalibrationDialog,
    CalibrationInputChanged(String),
    Calibrate,
    OpenLocationDialog,
    CloseLocationDialog,
    LocationInputChanged(String),
    AddLocation,
    SelectLocation(String),
    RemoveLocation(String),
    /// User pressed (mouse down or finger down) on a sensor row.
    /// We record the timestamp; the matching release decides if
    /// this was a tap or a long-press.
    SensorPressed(usize),
    /// User released the press on a sensor row. If the press was
    /// long enough, this expands the per-sensor detail card.
    SensorReleased(usize),
    /// User tapped outside the expanded sensor; collapse it.
    CollapseSensorDetail,
    /// User edited the sensor filter input on the Model tab.
    SensorFilterChanged(String),
    /// User picked a different theme from the About tab.
    ThemeSelected(NamedTheme),
}

/// A subset of Iced's built-in themes that we expose in the
/// picker. We don't enumerate every theme (Iced 0.12 ships about
/// 20); a curated handful covers the realistic light/dark/colorful
/// space without overwhelming the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedTheme {
    Dark,
    Light,
    Dracula,
    Nord,
    SolarizedDark,
    SolarizedLight,
    GruvboxDark,
    TokyoNight,
}

impl NamedTheme {
    const ALL: &'static [NamedTheme] = &[
        NamedTheme::Dark,
        NamedTheme::Light,
        NamedTheme::Dracula,
        NamedTheme::Nord,
        NamedTheme::SolarizedDark,
        NamedTheme::SolarizedLight,
        NamedTheme::GruvboxDark,
        NamedTheme::TokyoNight,
    ];

    fn label(self) -> &'static str {
        match self {
            NamedTheme::Dark           => "Dark",
            NamedTheme::Light          => "Light",
            NamedTheme::Dracula        => "Dracula",
            NamedTheme::Nord           => "Nord",
            NamedTheme::SolarizedDark  => "Solarized Dark",
            NamedTheme::SolarizedLight => "Solarized Light",
            NamedTheme::GruvboxDark    => "Gruvbox Dark",
            NamedTheme::TokyoNight     => "Tokyo Night",
        }
    }

    fn to_theme(self) -> Theme {
        match self {
            NamedTheme::Dark           => Theme::Dark,
            NamedTheme::Light          => Theme::Light,
            NamedTheme::Dracula        => Theme::Dracula,
            NamedTheme::Nord           => Theme::Nord,
            NamedTheme::SolarizedDark  => Theme::SolarizedDark,
            NamedTheme::SolarizedLight => Theme::SolarizedLight,
            NamedTheme::GruvboxDark    => Theme::GruvboxDark,
            NamedTheme::TokyoNight     => Theme::TokyoNight,
        }
    }
}

/// Initialization flags. Iced's `Application` trait takes one
/// `Flags` type so we bundle both inputs into a struct.
///
/// We have to impl `Default` so iced's `Settings::default()` works
/// (Iced requires `Flags: Default`). The default instance produces
/// a non-functional app — it's only there to satisfy the type
/// system; production callers always pass real values via
/// `Settings { flags: ..., .. }`.
pub struct InitFlags {
    pub world: WorldState,
    pub calibration_queue: CalibrationQueue,
}

impl Default for InitFlags {
    fn default() -> Self {
        // The defaults here are never observed in production — see
        // module-level note above. They exist only to satisfy
        // Settings::default()'s `Flags: Default` requirement.
        use std::sync::{Arc, RwLock};
        use crate::world_state::SystemState;
        Self {
            world: Arc::new(RwLock::new(SystemState::new())),
            calibration_queue: CalibrationQueue::new(),
        }
    }
}

impl Application for AmbientLearnerApp {
    type Executor = iced::executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = InitFlags;

    fn new(flags: InitFlags) -> (Self, Command<Message>) {
        let mut app = Self {
            world: flags.world,
            calibration_queue: flags.calibration_queue,
            snapshot: LocalSnapshot::default(),
            current_tab: Tab::Live,
            calibration_input: String::new(),
            show_calibration_dialog: false,
            calibration_error: None,
            location_input: String::new(),
            show_location_dialog: false,
            location_error: None,
            sensor_press_started: None,
            expanded_sensor: None,
            sensor_filter: String::new(),
            selected_theme: Theme::Dark,
        };
        // Seed the snapshot once so the first frame isn't blank.
        app.refresh_snapshot();
        (app, Command::none())
    }

    fn title(&self) -> String {
        String::from("Ambient Learner")
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Tick => {
                self.refresh_snapshot();
            }
            Message::SelectTab(tab) => {
                self.current_tab = tab;
            }
            Message::OpenCalibrationDialog => {
                self.show_calibration_dialog = true;
                // Pre-fill with the current temperature reading;
                // users typically calibrate to "what my reference
                // thermometer says is close to" the current display.
                self.calibration_input = format!("{:.1}", self.snapshot.temperature);
                self.calibration_error = None;
            }
            Message::CloseCalibrationDialog => {
                self.show_calibration_dialog = false;
                self.calibration_error = None;
            }
            Message::CalibrationInputChanged(value) => {
                self.calibration_input = value;
                // Clear the error message as soon as the user
                // starts editing; we'll revalidate on Calibrate.
                self.calibration_error = None;
            }
            Message::Calibrate => match self.calibration_input.trim().parse::<f32>() {
                Ok(target) if (-50.0..=70.0).contains(&target) => {
                    // Push onto the queue so the model worker picks
                    // it up on its next tick. This is what GTK and
                    // Qt do too; previously the Iced UI only set a
                    // flag and lost the value.
                    self.calibration_queue
                        .push(CalibrationRequest::new(target).with_source("iced-ui"));
                    self.show_calibration_dialog = false;
                    self.calibration_error = None;
                }
                Ok(_) => {
                    self.calibration_error =
                        Some("Temperature out of plausible range (-50 to 70 °C)".to_string());
                }
                Err(_) => {
                    self.calibration_error =
                        Some("Enter a number, e.g. 22.5".to_string());
                }
            },
            Message::OpenLocationDialog => {
                self.show_location_dialog = true;
                self.location_input.clear();
                self.location_error = None;
            }
            Message::CloseLocationDialog => {
                self.show_location_dialog = false;
                self.location_error = None;
            }
            Message::LocationInputChanged(value) => {
                self.location_input = value;
                self.location_error = None;
            }
            Message::AddLocation => {
                let name = self.location_input.trim();
                if name.is_empty() {
                    self.location_error =
                        Some("Enter a name for the place.".to_string());
                } else if name.len() > 64 {
                    self.location_error =
                        Some("Name must be 64 characters or fewer.".to_string());
                } else {
                    // Mutate world state directly — same pattern
                    // the Qt backend's `add_location` uses. The
                    // model worker re-reads on its next tick.
                    let mut state = world_state::write(&self.world);
                    let new_loc = crate::world_state::Location::new(name);
                    state.locations.push(new_loc);
                    // Promote-on-add: select the new one so users
                    // don't have to do a second tap.
                    state.current_location_idx = state.locations.len() - 1;
                    drop(state);
                    self.show_location_dialog = false;
                    self.location_input.clear();
                    self.location_error = None;
                    self.refresh_snapshot();
                }
            }
            Message::SelectLocation(id) => {
                let mut state = world_state::write(&self.world);
                if let Some(idx) = state.locations.iter().position(|l| l.id == id) {
                    state.current_location_idx = idx;
                    state.current_location = state.locations[idx].name.clone();
                }
                drop(state);
                self.refresh_snapshot();
            }
            Message::RemoveLocation(id) => {
                let mut state = world_state::write(&self.world);
                if let Some(idx) = state.locations.iter().position(|l| l.id == id) {
                    state.locations.remove(idx);
                    // Re-anchor current_location_idx so we don't
                    // dangle past the end.
                    if state.current_location_idx >= state.locations.len() {
                        state.current_location_idx =
                            state.locations.len().saturating_sub(1);
                    }
                    if let Some(loc) = state.locations.get(state.current_location_idx) {
                        state.current_location = loc.name.clone();
                    } else {
                        state.current_location.clear();
                    }
                }
                drop(state);
                self.refresh_snapshot();
            }
            Message::SensorPressed(idx) => {
                self.sensor_press_started = Some((idx, Instant::now()));
            }
            Message::SensorReleased(idx) => {
                if let Some((press_idx, started)) = self.sensor_press_started {
                    if press_idx == idx
                        && started.elapsed().as_millis() >= LONG_PRESS_MS
                    {
                        // Toggle: if the same sensor is already
                        // expanded, collapse; otherwise expand.
                        self.expanded_sensor = if self.expanded_sensor == Some(idx) {
                            None
                        } else {
                            Some(idx)
                        };
                    }
                }
                self.sensor_press_started = None;
            }
            Message::CollapseSensorDetail => {
                self.expanded_sensor = None;
            }
            Message::SensorFilterChanged(s) => {
                self.sensor_filter = s;
                // Clear the expanded sensor if it's no longer in
                // the filtered view — otherwise the user would see
                // an "orphan" expanded card without context.
                if let Some(idx) = self.expanded_sensor {
                    let name = self.snapshot.sensor_names.get(idx);
                    let still_matches = name.map(|n| {
                        self.sensor_filter.is_empty()
                            || n.to_lowercase()
                                .contains(&self.sensor_filter.to_lowercase())
                    }).unwrap_or(false);
                    if !still_matches {
                        self.expanded_sensor = None;
                    }
                }
            }
            Message::ThemeSelected(t) => {
                self.selected_theme = t.to_theme();
            }
        }
        Command::none()
    }

    fn view(&self) -> Element<Message> {
        // Per-tab content; everything else (tabs, modal) wraps it.
        let page: Element<Message> = match self.current_tab {
            Tab::Live      => self.view_live(),
            Tab::Model     => self.view_model(),
            Tab::Locations => self.view_locations(),
            Tab::Chart     => self.view_chart(),
            Tab::About     => self.view_about(),
        };

        let main = column![
            container(page)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(20),
            self.view_tab_bar(),
        ]
        .width(Length::Fill)
        .height(Length::Fill);

        // Overlay one of the dialogs on top when active. Iced 0.12
        // doesn't have a proper modal stack so we render the dialog
        // as the only visible content; cancel/confirm dismisses.
        if self.show_calibration_dialog {
            self.view_calibration_dialog().into()
        } else if self.show_location_dialog {
            self.view_location_dialog().into()
        } else {
            main.into()
        }
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        // 5 Hz UI refresh + keyboard shortcuts. The keyboard
        // subscription is a feature Iced does well that QML
        // doesn't get for free — we use it for power-user
        // navigation (number keys jump tabs, c calibrates,
        // l adds a place, ESC closes dialogs).
        //
        // We batch the two sources; Iced merges them into one
        // stream of `Message`. The time::every requires the
        // `smol` feature on iced, which we enable in Cargo.toml.
        Subscription::batch([
            iced::time::every(Duration::from_millis(200)).map(|_| Message::Tick),
            keyboard::on_key_press(handle_hotkey),
        ])
    }

    fn theme(&self) -> Theme {
        self.selected_theme.clone()
    }
}

impl AmbientLearnerApp {
    fn refresh_snapshot(&mut self) {
        let state = world_state::read(&self.world);
        self.snapshot.temperature = state.ambient_temp;
        self.snapshot.confidence = state.confidence;
        self.snapshot.model_type = state.model_type.as_str().to_string();
        self.snapshot.calibration_count = state.calibration_count;
        self.snapshot.sensor_names = state.sensor_names.clone();
        self.snapshot.sensor_readings = state.sensors.clone();
        self.snapshot.locations = state
            .locations
            .iter()
            .enumerate()
            .map(|(idx, loc)| {
                (
                    loc.id.clone(),
                    loc.name.clone(),
                    idx == state.current_location_idx,
                )
            })
            .collect();

        // Ambient temperature series. The raw history can be up to
        // 1000 points; we sub-sample to ~300 because pixel-perfect
        // resolution on a 400px-wide chart maxes out around there
        // and we save a lot of work in the Canvas paint path.
        let raw_ambient: Vec<f32> = state
            .temperature_history
            .iter()
            .map(|(_, t)| *t)
            .collect();
        self.snapshot.ambient_series = sub_sample(&raw_ambient, 300);

        // Per-sensor series, parallel to sensor_names. Each sensor
        // gets a vector of its recent values.
        let n_sensors = state.sensor_names.len();
        let mut per_sensor: Vec<Vec<f32>> = vec![Vec::with_capacity(300); n_sensors];
        for (_, sample) in state.sensor_history.iter() {
            for (i, val) in sample.iter().enumerate() {
                if i < per_sensor.len() {
                    per_sensor[i].push(*val);
                }
            }
        }
        for s in per_sensor.iter_mut() {
            *s = sub_sample(s, 300);
        }
        self.snapshot.sensor_series = per_sensor;

        // Calibration projections: both the scatter plot data and
        // the audit log come from `state.calibration_samples`, just
        // shaped differently. We compute the mean-of-sensors once
        // and use it for both.
        //
        // The scatter projection drops samples with no plausible
        // mean (empty sensors or all-NaN); the audit log keeps
        // them but renders mean=0 so the user can still see the
        // attribution. The two projections diverge on this because
        // the scatter plot is "what does the data look like" and a
        // NaN dot is meaningless, while the audit log is "who
        // calibrated me" and the attribution is still meaningful
        // even if the sensor snapshot was bad.
        let now = std::time::Instant::now();
        let mut samples: Vec<(f32, f32)> = Vec::new();
        let mut log: Vec<AuditEntry> = Vec::new();
        // Iterate newest-first by reversing the deque (which grows
        // append-only with oldest at the front).
        for sample in state.calibration_samples.iter().rev() {
            let (mean, has_mean) = if sample.sensors.is_empty() {
                (0.0, false)
            } else {
                let mut sum = 0.0f32;
                let mut count = 0u32;
                for &v in &sample.sensors {
                    if v.is_finite() {
                        sum += v;
                        count += 1;
                    }
                }
                if count == 0 {
                    (0.0, false)
                } else {
                    (sum / count as f32, true)
                }
            };

            if log.len() < 20 {
                log.push(AuditEntry {
                    target: sample.target,
                    mean_sensors: mean,
                    age_seconds: now
                        .saturating_duration_since(sample.when)
                        .as_secs(),
                    source: sample.source.clone(),
                });
            }
            if has_mean && sample.target.is_finite() {
                samples.push((mean, sample.target));
            }
        }
        // The scatter expects oldest-first (drawing order doesn't
        // matter much for dots, but consistency with prior
        // behavior is helpful). Reverse back.
        samples.reverse();
        self.snapshot.calibration_samples = samples;
        self.snapshot.calibration_log = log;
    }

    // ─── Pages ────────────────────────────────────────────────────

    fn view_live(&self) -> Element<Message> {
        // The big temperature reading. Centered, large, with the
        // confidence underneath. Sized generously for visibility on
        // a phone in sunlight. Color is left to the default theme
        // because Iced 0.12 doesn't have a clean per-instance text
        // color override without a custom StyleSheet impl, and the
        // size + position already make it the focal point.
        let temp_block = column![
            text("AMBIENT TEMPERATURE").size(13),
            Space::new(Length::Shrink, Length::Fixed(8.0)),
            text(format!("{:.1}°C", self.snapshot.temperature)).size(96),
            text(format!(
                "{:.0}% confidence",
                self.snapshot.confidence * 100.0,
            ))
            .size(18),
        ]
        .spacing(4)
        .align_items(Alignment::Center);

        // Status row: small KV columns for model, calibrations,
        // sensors. Each pair is `LABEL` (small) above `value`
        // (medium). Touch users rarely need to tap these numbers;
        // they're informational.
        let status_row = row![
            stat_block("MODEL", &self.snapshot.model_type),
            stat_block(
                "CALIBRATIONS",
                &format!("{}", self.snapshot.calibration_count),
            ),
            stat_block(
                "SENSORS",
                &format!("{}", self.snapshot.sensor_names.len()),
            ),
        ]
        .spacing(32)
        .align_items(Alignment::Center);

        // Calibrate button — big touch target (vertical padding
        // ~18 ensures a 48dp+ tap zone on typical mobile DPI).
        let calibrate_btn = button(
            text("🎯  Calibrate Model")
                .size(18)
                .horizontal_alignment(iced::alignment::Horizontal::Center),
        )
        .padding(Padding::from([18.0, 32.0]))
        .width(Length::Fixed(260.0))
        .on_press(Message::OpenCalibrationDialog);

        column![
            Space::new(Length::Shrink, Length::Fixed(40.0)),
            temp_block,
            Space::new(Length::Shrink, Length::Fixed(40.0)),
            status_row,
            Space::new(Length::Shrink, Length::Fixed(40.0)),
            calibrate_btn,
        ]
        .align_items(Alignment::Center)
        .width(Length::Fill)
        .into()
    }

    fn view_model(&self) -> Element<Message> {
        // Per-sensor list. Each row shows the sensor name on the
        // left, its current reading on the right. Wrapped in a
        // mouse_area so a long-press (≥500 ms hold) expands a
        // detail card with min/max/mean over the visible history.
        // This is the kind of touch gesture Iced needed to be
        // first-class for mobile use — short taps do nothing here
        // (the row isn't a button), but a deliberate hold reveals
        // more info without cluttering the default view.
        let mut sensor_rows = column![]
            .spacing(8)
            .width(Length::Fill);

        // Substring filter. Case-insensitive, empty matches all.
        // On a 53-sensor device this is the difference between
        // "scroll through everything" and "find the battery
        // sensor in 2 seconds." Iced makes this trivial — we
        // filter the iterator and use the original index for
        // long-press tracking so it's still correct.
        let filter_lower = self.sensor_filter.to_lowercase();
        let mut visible_count = 0usize;

        for (idx, name) in self.snapshot.sensor_names.iter().enumerate() {
            if !filter_lower.is_empty() && !name.to_lowercase().contains(&filter_lower) {
                continue;
            }
            visible_count += 1;

            let reading = self
                .snapshot
                .sensor_readings
                .get(idx)
                .copied()
                .unwrap_or(0.0);
            let is_expanded = self.expanded_sensor == Some(idx);

            let summary_row = row![
                text(name).size(14).width(Length::Fill),
                text(format!("{:.1}°C", reading)).size(16),
            ]
            .align_items(Alignment::Center)
            .padding(Padding::from([12.0, 8.0]));

            // The detail card. Only present when this row is
            // expanded; computes min/max/mean over the recent
            // history so the user can see whether this sensor has
            // been drifting, spiking, or stable.
            let row_el: Element<Message> = if is_expanded {
                let series = self
                    .snapshot
                    .sensor_series
                    .get(idx)
                    .cloned()
                    .unwrap_or_default();
                let (lo, hi) = series_min_max(&series);
                let mean = if series.is_empty() {
                    0.0
                } else {
                    series.iter().sum::<f32>() / series.len() as f32
                };
                let detail = column![
                    summary_row,
                    Space::new(Length::Shrink, Length::Fixed(4.0)),
                    row![
                        stat_block("MIN", &format!("{:.1}°C", lo)),
                        Space::new(Length::Fixed(16.0), Length::Shrink),
                        stat_block("MEAN", &format!("{:.1}°C", mean)),
                        Space::new(Length::Fixed(16.0), Length::Shrink),
                        stat_block("MAX", &format!("{:.1}°C", hi)),
                        Space::new(Length::Fill, Length::Shrink),
                    ]
                    .align_items(Alignment::Center)
                    .padding(Padding::from([4.0, 12.0])),
                    Space::new(Length::Shrink, Length::Fixed(4.0)),
                    container(
                        canvas(SparkLine {
                            data: series,
                            color: palette_color(idx),
                            min: lo,
                            max: hi,
                        })
                        .width(Length::Fill)
                        .height(Length::Fixed(80.0)),
                    )
                    .padding(Padding::from([0.0, 12.0])),
                    Space::new(Length::Shrink, Length::Fixed(8.0)),
                    container(
                        text("Tap and hold again to collapse").size(11),
                    )
                    .padding(Padding::from([0.0, 12.0])),
                    Space::new(Length::Shrink, Length::Fixed(4.0)),
                ];
                container(detail)
                    .style(theme::Container::Box)
                    .width(Length::Fill)
                    .into()
            } else {
                container(summary_row)
                    .width(Length::Fill)
                    .into()
            };

            // Wrap in a mouse_area to capture press/release for the
            // long-press detection. The threshold check happens in
            // `update` on the matching SensorReleased message.
            let interactive = mouse_area(row_el)
                .on_press(Message::SensorPressed(idx))
                .on_release(Message::SensorReleased(idx));

            sensor_rows = sensor_rows.push(interactive);
        }

        // Two distinct empty-states: zero sensors at all, vs. all
        // sensors filtered out by the search box. Different
        // messages tell the user what they're looking at.
        if self.snapshot.sensor_names.is_empty() {
            sensor_rows = sensor_rows.push(
                text("No sensors detected yet — give the sensor system a moment.")
                    .size(14),
            );
        } else if visible_count == 0 && !filter_lower.is_empty() {
            sensor_rows = sensor_rows.push(
                text(format!(
                    "No sensors match \"{}\".\nClear the filter to see all {}.",
                    self.sensor_filter,
                    self.snapshot.sensor_names.len(),
                ))
                .size(14),
            );
        }

        let filter_input = text_input(
            "Filter sensors (e.g. cpu, battery, gpu)",
            &self.sensor_filter,
        )
        .on_input(Message::SensorFilterChanged)
        .padding(Padding::from([10.0, 12.0]))
        .size(14);

        // Status line that shows the count of currently-visible
        // sensors. With a 53-sensor mobile device, this is useful
        // feedback — "filter narrowed 53 → 4" tells you the
        // filter worked.
        let count_line = if self.sensor_filter.is_empty() {
            text(format!(
                "{} sensor{}",
                self.snapshot.sensor_names.len(),
                if self.snapshot.sensor_names.len() == 1 { "" } else { "s" },
            ))
            .size(12)
        } else {
            text(format!(
                "{} of {} sensor{} match",
                visible_count,
                self.snapshot.sensor_names.len(),
                if self.snapshot.sensor_names.len() == 1 { "" } else { "s" },
            ))
            .size(12)
        };

        column![
            text("Per-sensor readings").size(20),
            Space::new(Length::Shrink, Length::Fixed(4.0)),
            text("Polled at ~10 Hz from /sys/class/thermal and /sys/class/hwmon.")
                .size(12),
            Space::new(Length::Shrink, Length::Fixed(4.0)),
            text("Tap and hold a sensor to see its recent history.")
                .size(11),
            Space::new(Length::Shrink, Length::Fixed(10.0)),
            filter_input,
            Space::new(Length::Shrink, Length::Fixed(4.0)),
            count_line,
            Space::new(Length::Shrink, Length::Fixed(10.0)),
            scrollable(sensor_rows).height(Length::Fill),
        ]
        .width(Length::Fill)
        .into()
    }

    fn view_locations(&self) -> Element<Message> {
        // Locations page: a list of saved places, with a tap-to-
        // select interaction and a Remove button per row. An
        // "Add place" button at the top opens the location dialog.
        let mut list = column![].spacing(8).width(Length::Fill);

        if self.snapshot.locations.is_empty() {
            list = list.push(
                text(
                    "No places saved yet.\n\
                    Tap “Add place” to save the current spot.",
                )
                .size(14)
                .horizontal_alignment(iced::alignment::Horizontal::Center),
            );
        } else {
            for (id, name, is_current) in &self.snapshot.locations {
                // Each row: name (with a star if current) + Remove.
                // The whole name area is tap-to-select; the remove
                // button is a separate target so accidental
                // mistaps don't delete a place.
                let display_name = if *is_current {
                    format!("★ {}", name)
                } else {
                    name.clone()
                };

                let row_el = row![
                    button(text(display_name).size(16))
                        .padding(Padding::from([12.0, 16.0]))
                        .width(Length::Fill)
                        .style(if *is_current {
                            theme::Button::Primary
                        } else {
                            theme::Button::Secondary
                        })
                        .on_press(Message::SelectLocation(id.clone())),
                    Space::new(Length::Fixed(8.0), Length::Shrink),
                    button(text("✕").size(16))
                        .padding(Padding::from([12.0, 16.0]))
                        .style(theme::Button::Destructive)
                        .on_press(Message::RemoveLocation(id.clone())),
                ]
                .align_items(Alignment::Center);

                list = list.push(row_el);
            }
        }

        let add_btn = button(
            text("➕  Add place")
                .size(16)
                .horizontal_alignment(iced::alignment::Horizontal::Center),
        )
        .padding(Padding::from([14.0, 28.0]))
        .width(Length::Fixed(200.0))
        .style(theme::Button::Primary)
        .on_press(Message::OpenLocationDialog);

        column![
            text("Saved places").size(20),
            Space::new(Length::Shrink, Length::Fixed(8.0)),
            text(
                "Tap a place to set it as current. \
                ★ marks the current selection.",
            )
            .size(12),
            Space::new(Length::Shrink, Length::Fixed(12.0)),
            container(add_btn).center_x().width(Length::Fill),
            Space::new(Length::Shrink, Length::Fixed(16.0)),
            scrollable(list).height(Length::Fill),
        ]
        .width(Length::Fill)
        .into()
    }

    fn view_chart(&self) -> Element<Message> {
        // Chart page: Canvas-based viz that's natural to Iced.
        // Renders ambient temperature as a sparkline over the
        // visible history range. The Canvas's draw method is pure
        // over state, so no QML-style imperative `requestPaint()`
        // is needed — Iced re-derives the geometry whenever the
        // snapshot changes.

        let ambient = self.snapshot.ambient_series.clone();
        let n = ambient.len();
        let (min, max) = series_min_max(&ambient);

        // Range labels above the chart.
        let header = row![
            text(format!("Ambient · {} points", n)).size(13),
            Space::new(Length::Fill, Length::Shrink),
            text(format!("range: {:.1}…{:.1}°C", min, max)).size(13),
        ];

        // The Canvas itself. We make it fixed-height so it doesn't
        // fight the Column for vertical space; the page below
        // shows summary text and the per-sensor strip chart.
        let ambient_chart = canvas(SparkLine {
            data: ambient.clone(),
            color: Color::from_rgb(0.45, 0.78, 0.42),
            min,
            max,
        })
        .width(Length::Fill)
        .height(Length::Fixed(180.0));

        // Multi-sensor strip chart. Stacks the per-sensor lines on
        // the same axes — a classic dense-data view that Canvas
        // makes trivial. Sensor count can be 50+ which would be
        // visual mush in QML's per-line-Item approach; here it's
        // just iterating the slice.
        let strip_chart = canvas(MultiLine {
            series: self.snapshot.sensor_series.clone(),
        })
        .width(Length::Fill)
        .height(Length::Fixed(220.0));

        let summary_line = if n == 0 {
            text("History is empty — give the sensors a moment to populate.")
                .size(13)
        } else {
            let delta = ambient.last().unwrap() - ambient.first().unwrap();
            let arrow = if delta > 0.05 {
                "▲"
            } else if delta < -0.05 {
                "▼"
            } else {
                "→"
            };
            text(format!(
                "{}  {:+.2}°C since the oldest point in the window",
                arrow, delta
            ))
            .size(14)
        };

        let scatter_chart = canvas(Scatter {
            samples: self.snapshot.calibration_samples.clone(),
        })
        .width(Length::Fill)
        .height(Length::Fixed(240.0));

        let scatter_header = if self.snapshot.calibration_samples.is_empty() {
            text(
                "No calibrations yet. Tap Calibrate on the Live tab\n\
                 to record samples; they'll appear here as dots.",
            )
            .size(13)
        } else {
            text(format!(
                "{} calibration sample{} — each dot is (mean sensor reading, your target).",
                self.snapshot.calibration_samples.len(),
                if self.snapshot.calibration_samples.len() == 1 { "" } else { "s" },
            ))
            .size(13)
        };

        // Audit log. A simple vertical column of rows; not
        // virtualized because 20 entries fits in screen-budget
        // even on a phone. Each row shows source pill, target,
        // delta from device, and age. The source pill colors are
        // the security-meaningful part: muted blue for "known UI
        // backends", amber for anything else — that's what makes
        // a rogue D-Bus caller visible.
        let log_section: Element<Message> = {
            let mut col = column![]
                .spacing(6)
                .width(Length::Fill);

            if self.snapshot.calibration_log.is_empty() {
                col = col.push(
                    text(
                        "No calibrations recorded yet. They'll appear here\n\
                         when you tap Calibrate on the Live tab.",
                    )
                    .size(13),
                );
            } else {
                for entry in &self.snapshot.calibration_log {
                    col = col.push(audit_log_row(entry));
                }
            }
            col.into()
        };

        scrollable(
            column![
                text("Ambient temperature").size(20),
                Space::new(Length::Shrink, Length::Fixed(8.0)),
                header,
                Space::new(Length::Shrink, Length::Fixed(8.0)),
                ambient_chart,
                Space::new(Length::Shrink, Length::Fixed(8.0)),
                summary_line,
                Space::new(Length::Shrink, Length::Fixed(24.0)),
                text("All sensors, recent history").size(20),
                Space::new(Length::Shrink, Length::Fixed(8.0)),
                text(format!(
                    "{} sensor line{} — stacked on shared axes",
                    self.snapshot.sensor_series.len(),
                    if self.snapshot.sensor_series.len() == 1 { "" } else { "s" },
                ))
                .size(13),
                Space::new(Length::Shrink, Length::Fixed(8.0)),
                strip_chart,
                Space::new(Length::Shrink, Length::Fixed(24.0)),
                text("Calibration scatter").size(20),
                Space::new(Length::Shrink, Length::Fixed(8.0)),
                scatter_header,
                Space::new(Length::Shrink, Length::Fixed(8.0)),
                scatter_chart,
                Space::new(Length::Shrink, Length::Fixed(24.0)),
                text("Calibration audit log").size(20),
                Space::new(Length::Shrink, Length::Fixed(4.0)),
                text(
                    "Who has calibrated this model. \"iced-ui\" is this\n\
                     app; other sources are external apps via IPC.",
                )
                .size(11),
                Space::new(Length::Shrink, Length::Fixed(8.0)),
                log_section,
            ]
            .spacing(0)
            .width(Length::Fill),
        )
        .into()
    }

    fn view_about(&self) -> Element<Message> {
        // Build the shortcuts hint string from the Tab enum, so it
        // stays in sync if we add tabs later.
        let mut shortcuts = String::from("Keyboard shortcuts:\n");
        for &tab in Tab::ALL {
            if let Some(c) = tab.shortcut() {
                shortcuts.push_str(&format!("  {} → {}\n", c, tab.label()));
            }
        }
        shortcuts.push_str("  c → Calibrate model\n");
        shortcuts.push_str("  l → Add place\n");
        shortcuts.push_str("  Esc → Close dialog\n");
        shortcuts.push_str("\nOn the Model tab: tap-and-hold a sensor row\n\
                            (≥0.5s) to see its recent history.");

        // Theme picker. Iced ships a generous set of built-in
        // themes; letting the user pick is two clicks here and
        // changes nothing else in the code. This is the kind of
        // "the entire UI rederives from state" benefit that's
        // genuinely Iced-flavored — in QML we'd need to refactor
        // every Theme.js reference into a runtime lookup.
        let current = self.selected_theme.clone();
        let mut theme_grid = column![]
            .spacing(6)
            .align_items(Alignment::Center);
        // Two-per-row to fit on a phone width without scrolling.
        let mut chunk_iter = NamedTheme::ALL.chunks(2);
        while let Some(chunk) = chunk_iter.next() {
            let mut row_el = row![].spacing(8);
            for &t in chunk {
                let is_active = current == t.to_theme();
                row_el = row_el.push(
                    button(text(t.label()).size(13))
                        .padding(Padding::from([10.0, 14.0]))
                        .width(Length::Fixed(140.0))
                        .style(if is_active {
                            theme::Button::Primary
                        } else {
                            theme::Button::Secondary
                        })
                        .on_press(Message::ThemeSelected(t)),
                );
            }
            theme_grid = theme_grid.push(row_el);
        }

        column![
            text("Ambient Learner").size(28),
            text(format!("v{}", env!("CARGO_PKG_VERSION"))).size(14),
            Space::new(Length::Shrink, Length::Fixed(20.0)),
            text("Local-only sensor-fusion + ML temperature prediction.")
                .size(14)
                .horizontal_alignment(iced::alignment::Horizontal::Center),
            Space::new(Length::Shrink, Length::Fixed(20.0)),
            stat_block("UI", "Iced"),
            Space::new(Length::Shrink, Length::Fixed(20.0)),
            text("Theme").size(14),
            Space::new(Length::Shrink, Length::Fixed(8.0)),
            theme_grid,
            Space::new(Length::Shrink, Length::Fixed(20.0)),
            text(shortcuts).size(13),
            Space::new(Length::Shrink, Length::Fixed(20.0)),
            text(
                "For map view, mood theming, and other rich UI features,\n\
                 use the Qt UI:  `--features qt --ui qt`",
            )
            .size(12)
            .horizontal_alignment(iced::alignment::Horizontal::Center),
        ]
        .align_items(Alignment::Center)
        .width(Length::Fill)
        .into()
    }

    // ─── Tab bar ──────────────────────────────────────────────────

    fn view_tab_bar(&self) -> Element<Message> {
        // The tab bar lives at the bottom of the screen — that's
        // the thumb-friendly zone on a phone held one-handed.
        // Each tab is a fixed-height button so reaching them is
        // predictable; the row stretches to fill width.
        //
        // We indicate the active tab using Iced's built-in
        // `theme::Button::Primary` variant; inactive tabs use
        // `theme::Button::Text` for a subtler look. This avoids
        // implementing a custom StyleSheet just for tab state.
        let mut bar = row![].width(Length::Fill);

        for &tab in Tab::ALL {
            let active = tab == self.current_tab;
            let style = if active {
                theme::Button::Primary
            } else {
                theme::Button::Text
            };

            let tab_button = button(
                column![
                    text(tab.icon()).size(20),
                    text(tab.label()).size(11),
                ]
                .align_items(Alignment::Center)
                .spacing(2),
            )
            .padding(Padding::from([10.0, 0.0]))
            .width(Length::Fill)
            .height(Length::Fixed(56.0))
            .style(style)
            .on_press(Message::SelectTab(tab));

            bar = bar.push(tab_button);
        }

        container(bar).width(Length::Fill).into()
    }

    // ─── Calibration dialog ───────────────────────────────────────

    fn view_location_dialog(&self) -> Element<Message> {
        // Add-place dialog. Mirrors the calibration dialog shape:
        // text input, validation error inline, Cancel/Confirm pair.
        let input = text_input("e.g. Living room", &self.location_input)
            .on_input(Message::LocationInputChanged)
            .on_submit(Message::AddLocation)
            .padding(Padding::from([14.0, 12.0]))
            .size(18);

        let mut content = column![
            text("Add a place").size(22),
            Space::new(Length::Shrink, Length::Fixed(8.0)),
            text(
                "Give the spot a short name. You'll be able to\n\
                 tap it later to set it as your current place.",
            )
            .size(13)
            .horizontal_alignment(iced::alignment::Horizontal::Center),
            Space::new(Length::Shrink, Length::Fixed(16.0)),
            input,
        ]
        .spacing(4)
        .align_items(Alignment::Center);

        if let Some(err) = &self.location_error {
            content = content.push(Space::new(Length::Shrink, Length::Fixed(8.0)));
            content = content.push(text(format!("⚠  {}", err)).size(13));
        }

        let buttons = row![
            button(text("Cancel").size(16))
                .padding(Padding::from([14.0, 28.0]))
                .style(theme::Button::Secondary)
                .on_press(Message::CloseLocationDialog),
            Space::new(Length::Fixed(12.0), Length::Shrink),
            button(text("Add").size(16))
                .padding(Padding::from([14.0, 28.0]))
                .style(theme::Button::Primary)
                .on_press(Message::AddLocation),
        ]
        .align_items(Alignment::Center);

        content = content.push(Space::new(Length::Shrink, Length::Fixed(24.0)));
        content = content.push(buttons);

        container(
            container(content)
                .padding(28)
                .max_width(420.0)
                .style(theme::Container::Box),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x()
        .center_y()
        .into()
    }

    fn view_calibration_dialog(&self) -> Element<Message> {
        // Inputs need extra padding here — the text_input widget's
        // hit area is otherwise quite small for a finger.
        let input = text_input("e.g. 22.5", &self.calibration_input)
            .on_input(Message::CalibrationInputChanged)
            .on_submit(Message::Calibrate)
            .padding(Padding::from([14.0, 12.0]))
            .size(20);

        let mut content = column![
            text("Calibrate the model").size(22),
            Space::new(Length::Shrink, Length::Fixed(8.0)),
            text(
                "Enter the actual ambient temperature you measured\n\
                 with a reference thermometer.",
            )
            .size(13)
            .horizontal_alignment(iced::alignment::Horizontal::Center),
            Space::new(Length::Shrink, Length::Fixed(16.0)),
            input,
        ]
        .spacing(4)
        .align_items(Alignment::Center);

        // Inline validation error. Shows up below the input field
        // when the user hits Calibrate with bad input. We don't
        // apply a custom color (would need a StyleSheet impl); the
        // error prefix carries the meaning.
        if let Some(err) = &self.calibration_error {
            content = content.push(Space::new(Length::Shrink, Length::Fixed(8.0)));
            content = content.push(text(format!("⚠  {}", err)).size(13));
        }

        // Buttons row. Order matches platform convention: cancel on
        // the left, confirm on the right. Both are big enough to
        // tap with a finger. The confirm button uses the Primary
        // style so the user's eye lands on it.
        let buttons = row![
            button(text("Cancel").size(16))
                .padding(Padding::from([14.0, 28.0]))
                .style(theme::Button::Secondary)
                .on_press(Message::CloseCalibrationDialog),
            Space::new(Length::Fixed(12.0), Length::Shrink),
            button(text("Calibrate").size(16))
                .padding(Padding::from([14.0, 28.0]))
                .style(theme::Button::Primary)
                .on_press(Message::Calibrate),
        ]
        .align_items(Alignment::Center);

        content = content.push(Space::new(Length::Shrink, Length::Fixed(24.0)));
        content = content.push(buttons);

        // Outer container — center on the screen, max width so it
        // doesn't span the full width on a desktop run. We use a
        // box-style container variant from Iced's built-in theme.
        container(
            container(content)
                .padding(28)
                .max_width(420.0)
                .style(theme::Container::Box),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x()
        .center_y()
        .into()
    }
}

// ─── Helpers ──────────────────────────────────────────────────────

/// Small "STAT NAME / value" block used across pages. Returns an
/// owned `Element` so callers can drop it into any column or row.
fn stat_block<'a>(label: &str, value: &str) -> Element<'a, Message> {
    column![
        text(label.to_string()).size(11),
        Space::new(Length::Shrink, Length::Fixed(4.0)),
        text(value.to_string()).size(18),
    ]
    .align_items(Alignment::Center)
    .into()
}

/// One row of the calibration audit log. Shows source pill,
/// target temp, device delta, and age. Source pill styling is
/// the security-meaningful part: the three known-UI sources get
/// the Secondary (muted) button variant; anything else gets
/// Destructive (amber/red) to draw attention to "an external app
/// did this."
///
/// Using `button` widgets for the pills instead of styled
/// containers is a deliberate Iced 0.12 choice — `theme::Button`
/// has the variants we want (Primary/Secondary/Destructive) but
/// `theme::Container` doesn't. The button is `on_press(Tick)`
/// because Iced 0.12 requires an `on_press` handler for the
/// button to render styled; we route to a harmless message.
fn audit_log_row(entry: &AuditEntry) -> Element<'_, Message> {
    let label = match entry.source.as_deref() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "(unknown)".to_string(),
    };
    let is_known_ui = matches!(
        entry.source.as_deref(),
        Some("qt-ui") | Some("gtk-ui") | Some("iced-ui"),
    );
    let is_external = entry
        .source
        .as_deref()
        .map(|s| !s.is_empty() && !is_known_ui)
        .unwrap_or(false);

    let pill_style = if is_known_ui {
        theme::Button::Secondary
    } else if is_external {
        // Destructive variant is red/amber in most Iced themes —
        // matches the Qt audit log's amber pill for external
        // sources.
        theme::Button::Destructive
    } else {
        // Source missing entirely — use Text (faintest) since
        // there's nothing actionable here, just a record.
        theme::Button::Text
    };

    let pill = button(text(label).size(11))
        .padding(Padding::from([4.0, 10.0]))
        .style(pill_style)
        // Tap on the pill is a no-op; we just need a handler so
        // Iced renders the button with its theme. Tick is
        // already the per-frame refresh message — re-firing it
        // mid-frame is safe.
        .on_press(Message::Tick);

    let delta = entry.target - entry.mean_sensors;
    let delta_str = format!(
        "(device {:.1}°C; {}{:.1})",
        entry.mean_sensors,
        if delta >= 0.0 { "+" } else { "" },
        delta,
    );

    row![
        pill,
        Space::new(Length::Fixed(8.0), Length::Shrink),
        text(format!("{:.1}°C", entry.target)).size(14),
        Space::new(Length::Fixed(8.0), Length::Shrink),
        text(delta_str).size(11).width(Length::Fill),
        text(format_age_short(entry.age_seconds)).size(11),
    ]
    .align_items(Alignment::Center)
    .padding(Padding::from([4.0, 4.0]))
    .into()
}

/// "5m ago", "2h ago", etc. Short form for the audit log age
/// column where space is tight.
fn format_age_short(seconds: u64) -> String {
    if seconds < 60 {
        format!("{}s ago", seconds)
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86400)
    }
}

/// Reduce a series to at most `max_points` evenly-sampled values.
///
/// We sub-sample rather than average because we want to preserve
/// short-lived peaks (a 30-second hot-spot) without averaging them
/// into invisibility. For sparkline-style display, "show the line
/// at every third point" is more honest than "show a smoothed
/// line."
fn sub_sample(values: &[f32], max_points: usize) -> Vec<f32> {
    let n = values.len();
    if n <= max_points || max_points == 0 {
        return values.to_vec();
    }
    let step = n as f32 / max_points as f32;
    (0..max_points)
        .map(|i| values[(i as f32 * step) as usize])
        .collect()
}

/// (min, max) of a series, ignoring NaN. Returns (0, 1) for an
/// empty series so callers can safely divide by `(max - min)`.
fn series_min_max(values: &[f32]) -> (f32, f32) {
    if values.is_empty() {
        return (0.0, 1.0);
    }
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for &v in values {
        if v.is_finite() {
            if v < lo { lo = v; }
            if v > hi { hi = v; }
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        return (0.0, 1.0);
    }
    if (hi - lo).abs() < 1e-3 {
        // Degenerate: all values equal. Inflate by ±0.5 so the line
        // doesn't render as a flat baseline against the canvas edge.
        return (lo - 0.5, hi + 0.5);
    }
    (lo, hi)
}

/// Map keyboard events to app Messages.
///
/// Number keys 1..=5 jump tabs. `c` opens the calibration dialog,
/// `l` opens the add-place dialog, `Esc` closes any open dialog.
/// Other keys are ignored.
///
/// Iced fires this for *every* key press globally — including
/// when a text input is focused. We let those pass through if a
/// dialog is open by returning None for navigation keys when a
/// dialog might be using them. Specifically: when the user is
/// typing into a calibration input, "c" should be a character;
/// when no dialog is open, "c" should open the dialog. We can't
/// know dialog state from a hotkey handler (it's a `fn`, not a
/// closure over `self`), so we default to always-active and trust
/// that text_input swallows typed characters before they reach
/// this layer.
fn handle_hotkey(
    key: keyboard::Key,
    _modifiers: keyboard::Modifiers,
) -> Option<Message> {
    use keyboard::Key;
    match key.as_ref() {
        Key::Character("1") => Some(Message::SelectTab(Tab::Live)),
        Key::Character("2") => Some(Message::SelectTab(Tab::Model)),
        Key::Character("3") => Some(Message::SelectTab(Tab::Locations)),
        Key::Character("4") => Some(Message::SelectTab(Tab::Chart)),
        Key::Character("5") => Some(Message::SelectTab(Tab::About)),
        Key::Character("c") | Key::Character("C") => {
            Some(Message::OpenCalibrationDialog)
        }
        Key::Character("l") | Key::Character("L") => {
            Some(Message::OpenLocationDialog)
        }
        Key::Named(keyboard::key::Named::Escape) => {
            // We don't know which dialog (if any) is open from
            // this layer; emit a close for both. The handlers are
            // idempotent — closing a non-open dialog is a no-op.
            Some(Message::CloseCalibrationDialog)
        }
        _ => None,
    }
}

// ─── Canvas programs ─────────────────────────────────────────────

/// Sparkline: a single line plot of a 1D series. Auto-scales to
/// fit the provided (min, max). Renders a thin axis line at the
/// midpoint for orientation.
///
/// This is the kind of viz Iced makes natural: pure function from
/// a `&[f32]` to a `Path`. No retained widget tree, no per-point
/// QML Item allocations, no model invalidation logic — just data
/// in, geometry out.
struct SparkLine {
    data: Vec<f32>,
    color: Color,
    min: f32,
    max: f32,
}

impl<Message> canvas::Program<Message> for SparkLine {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        // Background fill (subtle, lets us read against any theme).
        let bg = canvas::Path::rectangle(Point::ORIGIN, bounds.size());
        frame.fill(&bg, Color::from_rgba(0.10, 0.10, 0.14, 0.6));

        if self.data.len() < 2 {
            return vec![frame.into_geometry()];
        }

        // Midpoint reference line.
        let mid_y = bounds.size().height / 2.0;
        let axis = canvas::Path::line(
            Point::new(0.0, mid_y),
            Point::new(bounds.size().width, mid_y),
        );
        frame.stroke(
            &axis,
            canvas::Stroke::default()
                .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.08))
                .with_width(1.0),
        );

        // The data line.
        let w = bounds.size().width;
        let h = bounds.size().height;
        let span = (self.max - self.min).max(1e-3);
        let n = self.data.len() as f32;

        let line_path = canvas::Path::new(|p| {
            for (i, &v) in self.data.iter().enumerate() {
                let x = (i as f32 / (n - 1.0)) * w;
                // y inverted: higher temperature → smaller y
                // (closer to top of canvas).
                let y = h - ((v - self.min) / span) * h;
                if i == 0 {
                    p.move_to(Point::new(x, y));
                } else {
                    p.line_to(Point::new(x, y));
                }
            }
        });
        frame.stroke(
            &line_path,
            canvas::Stroke::default().with_color(self.color).with_width(2.0),
        );

        vec![frame.into_geometry()]
    }
}

/// MultiLine: stacked line plot of N series on shared axes. Each
/// series gets a color picked deterministically from its index so
/// colors stay stable across redraws.
///
/// Auto-scales jointly over all series (one shared y-axis) so
/// relative comparisons between sensors are meaningful.
struct MultiLine {
    series: Vec<Vec<f32>>,
}

impl<Message> canvas::Program<Message> for MultiLine {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        let bg = canvas::Path::rectangle(Point::ORIGIN, bounds.size());
        frame.fill(&bg, Color::from_rgba(0.10, 0.10, 0.14, 0.6));

        // Find the joint (min, max) across all series.
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        let mut max_len = 0usize;
        for s in &self.series {
            for &v in s {
                if v.is_finite() {
                    if v < lo { lo = v; }
                    if v > hi { hi = v; }
                }
            }
            max_len = max_len.max(s.len());
        }
        if !lo.is_finite() || !hi.is_finite() || max_len < 2 {
            return vec![frame.into_geometry()];
        }
        if (hi - lo).abs() < 1e-3 {
            lo -= 0.5;
            hi += 0.5;
        }

        let w = bounds.size().width;
        let h = bounds.size().height;
        let span = hi - lo;

        for (idx, s) in self.series.iter().enumerate() {
            if s.len() < 2 {
                continue;
            }
            let color = palette_color(idx);
            let n = s.len() as f32;
            let line_path = canvas::Path::new(|p| {
                for (i, &v) in s.iter().enumerate() {
                    let x = (i as f32 / (n - 1.0)) * w;
                    let y = h - ((v - lo) / span) * h;
                    if i == 0 {
                        p.move_to(Point::new(x, y));
                    } else {
                        p.line_to(Point::new(x, y));
                    }
                }
            });
            frame.stroke(
                &line_path,
                canvas::Stroke::default()
                    .with_color(color)
                    .with_width(1.2),
            );
        }

        vec![frame.into_geometry()]
    }
}

/// Scatter plot of calibration samples. Each dot is one
/// (mean_sensor_reading, user_target) sample. Renders a diagonal
/// y=x reference line so the user can see at a glance how far off
/// the sensors are from the user's reference.
///
/// Why this matters: a perfect device would have the diagonal
/// passing through every dot — sensors and reality agree. A
/// device with thermal offset would show a parallel-but-shifted
/// cloud. A device with poor sensor placement would have a
/// scattered cloud. This is the visual you can't easily get in
/// QML without third-party charting libraries, and it's *exactly*
/// what's diagnostically useful about looking at the data.
struct Scatter {
    samples: Vec<(f32, f32)>,
}

impl<Message> canvas::Program<Message> for Scatter {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        // Background.
        let bg = canvas::Path::rectangle(Point::ORIGIN, bounds.size());
        frame.fill(&bg, Color::from_rgba(0.10, 0.10, 0.14, 0.6));

        if self.samples.is_empty() {
            return vec![frame.into_geometry()];
        }

        // Joint bounding box across both axes — we want the
        // diagonal reference line to actually be y=x, so the two
        // axes share a scale.
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for &(x, y) in &self.samples {
            for v in [x, y] {
                if v.is_finite() {
                    if v < lo { lo = v; }
                    if v > hi { hi = v; }
                }
            }
        }
        if !lo.is_finite() || !hi.is_finite() {
            return vec![frame.into_geometry()];
        }
        // Inflate a bit so dots aren't on the canvas edge.
        let pad = (hi - lo).max(1.0) * 0.10;
        lo -= pad;
        hi += pad;
        let span = (hi - lo).max(1e-3);

        let w = bounds.size().width;
        let h = bounds.size().height;

        // Project (data_x, data_y) → (canvas_x, canvas_y). Y is
        // inverted (screen grows down, temperature grows up).
        let project = |x: f32, y: f32| -> Point {
            let nx = (x - lo) / span;
            let ny = (y - lo) / span;
            Point::new(nx * w, h - ny * h)
        };

        // Diagonal reference line y=x. Drawn first so dots cover it.
        let diag = canvas::Path::line(project(lo, lo), project(hi, hi));
        frame.stroke(
            &diag,
            canvas::Stroke::default()
                .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.20))
                .with_width(1.0),
        );

        // Diagonal label.
        frame.fill_text(canvas::Text {
            content: "y = x  (perfect)".to_string(),
            position: Point::new(w - 110.0, 18.0),
            color: Color::from_rgba(1.0, 1.0, 1.0, 0.40),
            size: 11.0.into(),
            ..canvas::Text::default()
        });

        // Plot points. Use a single color (no per-point coloring
        // — the position carries the meaning).
        let dot_color = Color::from_rgba(0.40, 0.78, 0.95, 0.85);
        for &(x, y) in &self.samples {
            if !x.is_finite() || !y.is_finite() {
                continue;
            }
            let p = project(x, y);
            let dot = canvas::Path::circle(p, 4.0);
            frame.fill(&dot, dot_color);
        }

        // Axis range labels in the corners.
        frame.fill_text(canvas::Text {
            content: format!("{:.1}°C", lo),
            position: Point::new(4.0, h - 16.0),
            color: Color::from_rgba(1.0, 1.0, 1.0, 0.55),
            size: 11.0.into(),
            ..canvas::Text::default()
        });
        frame.fill_text(canvas::Text {
            content: format!("{:.1}°C", hi),
            position: Point::new(4.0, 4.0),
            color: Color::from_rgba(1.0, 1.0, 1.0, 0.55),
            size: 11.0.into(),
            ..canvas::Text::default()
        });

        vec![frame.into_geometry()]
    }
}

/// Deterministic color from index. Picks from a small palette of
/// distinguishable hues; wraps mod-N. We deliberately don't try to
/// generate colors algorithmically (HSL-rotate) because the eye
/// can't easily distinguish two close hues; a hand-picked palette
/// of 8 is more useful than 53 algorithmically-spaced ones.
fn palette_color(idx: usize) -> Color {
    const PALETTE: &[(f32, f32, f32)] = &[
        (0.96, 0.50, 0.50), // coral
        (0.55, 0.78, 0.40), // green
        (0.40, 0.65, 0.92), // blue
        (0.93, 0.74, 0.30), // amber
        (0.76, 0.56, 0.92), // violet
        (0.30, 0.80, 0.78), // teal
        (0.92, 0.45, 0.78), // pink
        (0.65, 0.65, 0.65), // grey
    ];
    let (r, g, b) = PALETTE[idx % PALETTE.len()];
    Color::from_rgba(r, g, b, 0.75)
}

/// Entry point used by `main.rs::run_iced_ui`.
pub fn run(world: WorldState, calibration_queue: CalibrationQueue) -> iced::Result {
    println!("🎨 Starting Iced UI (touch-friendly)…");

    AmbientLearnerApp::run(Settings {
        flags: InitFlags { world, calibration_queue },
        window: iced::window::Settings {
            // Approximate phone aspect ratio so the layout looks
            // right on a desktop preview. The real phone window
            // will be sized by the compositor.
            size: iced::Size::new(420.0, 760.0),
            ..Default::default()
        },
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_iteration_covers_all_variants() {
        // If someone adds a new Tab variant they must add it to
        // Tab::ALL. This test catches the omission.
        assert_eq!(Tab::ALL.len(), 5);
        assert!(Tab::ALL.contains(&Tab::Live));
        assert!(Tab::ALL.contains(&Tab::Model));
        assert!(Tab::ALL.contains(&Tab::Locations));
        assert!(Tab::ALL.contains(&Tab::Chart));
        assert!(Tab::ALL.contains(&Tab::About));
    }

    #[test]
    fn every_tab_has_a_shortcut() {
        // We promote keyboard shortcuts as a first-class Iced
        // feature — every tab should have one or the About-page
        // shortcut listing will be confusing.
        for &t in Tab::ALL {
            assert!(
                t.shortcut().is_some(),
                "Tab {:?} is missing a shortcut",
                t
            );
        }
    }

    #[test]
    fn shortcuts_are_distinct() {
        let mut seen: std::collections::HashSet<char> =
            std::collections::HashSet::new();
        for &t in Tab::ALL {
            if let Some(c) = t.shortcut() {
                assert!(
                    seen.insert(c),
                    "duplicate shortcut character {:?} for {:?}",
                    c, t
                );
            }
        }
    }

    #[test]
    fn sub_sample_preserves_short_series() {
        let v = vec![1.0, 2.0, 3.0];
        assert_eq!(sub_sample(&v, 100), v);
    }

    #[test]
    fn sub_sample_reduces_long_series() {
        let v: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        let out = sub_sample(&v, 50);
        assert_eq!(out.len(), 50);
        // First sampled point is index 0 → value 0.0
        assert_eq!(out[0], 0.0);
        // Last sampled point should be near the end
        assert!(out[49] > 950.0);
    }

    #[test]
    fn series_min_max_handles_empty() {
        let (lo, hi) = series_min_max(&[]);
        assert!(hi > lo, "empty series must return a usable range");
    }

    #[test]
    fn series_min_max_handles_flat() {
        let v = vec![22.0, 22.0, 22.0];
        let (lo, hi) = series_min_max(&v);
        // Degenerate case should be inflated so the chart isn't a
        // flat line on the canvas edge.
        assert!(hi - lo > 0.5);
    }

    #[test]
    fn series_min_max_ignores_nan() {
        let v = vec![20.0, f32::NAN, 25.0, f32::INFINITY, 22.0];
        let (lo, hi) = series_min_max(&v);
        assert!((lo - 20.0).abs() < 1e-3);
        assert!((hi - 25.0).abs() < 1e-3);
    }

    #[test]
    fn tabs_have_distinct_labels() {
        // Mostly a sanity check — duplicate tab labels would be a
        // copy-paste bug and would confuse users.
        let labels: Vec<&str> = Tab::ALL.iter().map(|t| t.label()).collect();
        let unique_count = {
            let mut set: std::collections::HashSet<&&str> = std::collections::HashSet::new();
            for l in &labels {
                set.insert(l);
            }
            set.len()
        };
        assert_eq!(unique_count, labels.len());
    }

    #[test]
    fn tabs_have_distinct_icons() {
        let icons: Vec<&str> = Tab::ALL.iter().map(|t| t.icon()).collect();
        let unique_count = {
            let mut set: std::collections::HashSet<&&str> = std::collections::HashSet::new();
            for i in &icons {
                set.insert(i);
            }
            set.len()
        };
        assert_eq!(unique_count, icons.len());
    }

    #[test]
    fn format_age_short_boundaries() {
        // The boundaries should match: < 60s → seconds, 60s → 1m,
        // 3600s → 1h, 86400s → 1d. Off-by-one in these checks
        // would make the audit log say "60s ago" or "0m ago".
        assert_eq!(format_age_short(0),      "0s ago");
        assert_eq!(format_age_short(59),     "59s ago");
        assert_eq!(format_age_short(60),     "1m ago");
        assert_eq!(format_age_short(3599),   "59m ago");
        assert_eq!(format_age_short(3600),   "1h ago");
        assert_eq!(format_age_short(86_399), "23h ago");
        assert_eq!(format_age_short(86_400), "1d ago");
    }

    #[test]
    fn format_age_short_handles_large_values() {
        // 365 days. Should produce a "365d ago" reading instead of
        // panicking on integer overflow or returning a weird unit.
        assert_eq!(format_age_short(365 * 86_400), "365d ago");
    }
}
