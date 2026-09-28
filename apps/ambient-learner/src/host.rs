//! host — the engine's side of the egui frontend.
//!
//! `main.rs` (GTK) builds its window and its engine in one 7,000-line
//! function; the engine half is now `engine.rs` plus the `systems` worker
//! threads, and this file is the second thing to stand on it. It does what
//! the GTK `build_ui` does before any widget exists — load the locations,
//! the latest model and neural field, open (or create) the database, spawn
//! the sensor/model/environment/screen-temp workers — and then, every
//! frame, turns the `WorldState` those workers fill into the page state the
//! `Shell` draws, and turns the actions the pages raise back into engine
//! calls.
//!
//! ## Three threads, one direction each
//!
//! * The **workers** (`systems::spawn_all`) write the world: sensor
//!   readings at 10 Hz, predictions when they change, the environment
//!   sensors, the published screen-temperature frames.
//! * The **store** thread owns the SQLite read connection. Every query the
//!   pages need — the week of daily averages, the calibration log, the
//!   counts — is asked for over a channel and answered over another, so a
//!   slow disk never stalls a frame. Writes go through `DbWriter`, the
//!   engine's own batched writer, on its own thread.
//! * The **UI** thread calls [`Host::tick`] once per frame. It takes a read
//!   lock on the world for a few microseconds, drains the store's replies,
//!   and copies into the shell. It never blocks on either.
//!
//! ## Visible, on purpose
//!
//! Everything the host does is appended to [`Host::log`] and the shell
//! draws it on the Debug page: which database it opened, how many sensors
//! it found, each calibration and where it was saved, each refusal and why.
//! A reading that looks wrong can be traced without a debugger.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::db_writer::DbWriter;
use crate::engine::{self, Database, Location, LocationType, Model, DATA_DIR};
use crate::forecast::{ForecastModel, TimeSeriesPoint};
use crate::systems::{self, CalibrationQueue, CalibrationRequest, SystemHandles};
use crate::temperature_field::TemperatureField;
use crate::ui::egui::calibrate::CalibrateAction;
use crate::ui::egui::forecast_card::{Band, ForecastCard};
use crate::ui::egui::history::{History, HistoryAction, Point};
use crate::ui::egui::locations::{self as loc_page, LocationAction};
use crate::ui::egui::shell::{Action, Shell};
use crate::ui::egui::sync::{SyncAction, TransferBackend};
use crate::ui::egui::{live, model as model_page, pressure, screen_temps, sync as sync_page, LiveSnapshot};
use crate::world_state::{self, SystemState, WorldState};

/// How often the store is asked for fresh counts and the calibration log
/// when nothing else prompted it.
const STORE_REFRESH: Duration = Duration::from_secs(15);
/// How often the forecast model is fed the current prediction. The GTK
/// tick does it once a minute; the same here.
const FORECAST_EVERY: Duration = Duration::from_secs(60);
/// How often the published screen-temperature frame is re-read.
const FRAME_EVERY: Duration = Duration::from_millis(1000);
/// How many host log lines to keep.
const LOG_CAP: usize = 200;
/// The chosen location's id, beside `locations.json`.
const CURRENT_LOCATION_FILE: &str = "current_location";
/// The sparkline's length, in samples from the prediction history.
const SPARK_POINTS: usize = 60;

/// Questions for the store thread.
enum Ask {
    /// The daily averages for a week: 0 is this week, 1 last week…
    Week(usize),
    /// Counts and the calibration log.
    Refresh,
    ExportCsv(PathBuf),
    ExportDb(PathBuf),
    ImportDb(PathBuf),
}

/// Answers from it.
enum Reply {
    Week { offset: usize, points: Vec<Point> },
    Refresh { calibrations: usize, readings: usize, log: Vec<(i64, f32)> },
    /// One line for the page that asked, and the log.
    Note { for_history: bool, text: String },
}

/// A calibration the host is waiting on the model worker to apply.
struct PendingCal {
    target: f32,
    since: Instant,
    /// The world's calibration count before the request, so "applied" can
    /// be seen rather than assumed.
    count_before: usize,
}

/// The running engine, as the frontend sees it.
pub struct Host {
    world: WorldState,
    model: Arc<Mutex<Model>>,
    neural: Arc<Mutex<TemperatureField>>,
    writer: Option<DbWriter>,
    /// Held for its `Drop`: dropping it signals every worker to stop.
    _handles: SystemHandles,
    queue: CalibrationQueue,
    ask: Sender<Ask>,
    reply: Receiver<Reply>,
    locations: Vec<Location>,
    current_location: usize,
    forecast: ForecastModel,
    last_forecast: Option<Instant>,
    last_refresh: Option<Instant>,
    last_frame: Option<Instant>,
    /// The week the History page is showing, and whether the store has
    /// answered for it yet.
    week_offset: usize,
    week_loaded: Option<usize>,
    pending_cal: Option<PendingCal>,
    /// Calibrations this session, by unix time, for attributing the log's
    /// rows to this frontend.
    session_cals: Vec<i64>,
    /// Counts from the store, for the Sync and Model pages.
    db_calibrations: usize,
    db_readings: usize,
    cal_log: Vec<(i64, f32)>,
    /// Where the database lives, for the log and the Sync page.
    db_path: Option<PathBuf>,
    /// Which optional environment sensors have EVER reported, sticky:
    /// humidity, pressure, light. A sensor that reported once and stopped
    /// is a degraded one; a sensor that never existed is not.
    env_seen: [bool; 3],
    /// What happened, newest last.
    pub log: Vec<String>,
}

impl Host {
    /// Bring the engine up the way the GTK build does: locations, models,
    /// database, workers. Never fails — a missing database is created, a
    /// missing model is a new one, missing sensors are reported — because
    /// an app that cannot open is indistinguishable from one that crashed.
    pub fn open() -> Host {
        let mut log = Vec::new();
        let mut note = |s: String| { println!("host: {s}"); log.push(s); };

        let data_dir = std::env::current_dir()
            .map(|d| d.join(DATA_DIR))
            .unwrap_or_else(|_| PathBuf::from(DATA_DIR));
        std::fs::create_dir_all(&data_dir).ok();
        note(format!("data dir {}", data_dir.display()));

        // Locations, synchronously: the file is a few hundred bytes and the
        // Live page needs the current name on its first frame.
        // The GTK build's WiFi-based detection is commented out there
        // ("causes UI freezes"); the same default it falls back to is used
        // here: the Unknown location if there is one, else the first.
        let locations = engine::load_locations();
        let current_location = current_location_file(&data_dir)
            .and_then(|id| locations.iter().position(|l| l.id == id))
            .or_else(|| locations.iter().position(|l| matches!(l.location_type, LocationType::Unknown)))
            .unwrap_or(0);
        note(format!("{} location(s), current: {}", locations.len(),
            locations.get(current_location).map(|l| l.name.as_str()).unwrap_or("—")));

        // Sensors, to size the models. The poller here is thrown away; the
        // sensor worker opens its own.
        let poller = engine::ThermalPoller::new();
        let names = poller.sensor_names();
        note(if names.is_empty() {
            "no thermal sensors found (set AMBIENT_SYSFS_ROOT to point at a fake sysfs)".into()
        } else {
            format!("{} sensor(s): {}", names.len(), names.join(", "))
        });

        let model = match engine::get_latest_model().and_then(|p| {
            Model::load(&p).map_err(|e| note(format!("model {} unreadable: {e}", p.display()))).ok()
                .map(|m| (p, m))
        }) {
            Some((p, m)) => {
                note(format!("model {} ({} calibrations)", p.display(), m.calibration_count));
                m
            }
            None => { note("new linear model".into()); Model::new(names.clone()) }
        };
        let neural = match engine::get_latest_neural_field().and_then(|p| {
            TemperatureField::load(&p).map_err(|e| note(format!("neural field {} unreadable: {e}", p.display()))).ok()
                .map(|f| (p, f))
        }) {
            Some((p, f)) => {
                note(format!("neural field {} ({} calibrations)", p.display(), f.get_calibration_count()));
                f
            }
            None => { note("new neural field".into()); TemperatureField::new(names.len()) }
        };
        let model = Arc::new(Mutex::new(model));
        let neural = Arc::new(Mutex::new(neural));

        // The database: the newest in the data dir, or a new one. Opened
        // once here for its checks, then handed to the store thread.
        let db = match engine::get_latest_database() {
            Some(p) => match Database::open(&p) {
                Ok(d) => { note(format!("database {}", p.display())); Some(d) }
                Err(e) => {
                    note(format!("database {} failed to open: {e}; creating a new one", p.display()));
                    Database::new(&engine::get_timestamp()).ok()
                }
            },
            None => {
                let d = Database::new(&engine::get_timestamp()).ok();
                if let Some(d) = &d { note(format!("new database {}", d.path.display())); }
                d
            }
        };
        let db_path = db.as_ref().map(|d| d.path.clone());
        let writer = db_path.as_ref().and_then(|p| match DbWriter::spawn(p.clone()) {
            Ok(w) => Some(w),
            Err(e) => { note(format!("db writer failed to start: {e}")); None }
        });
        // GTK passes no writer and persists hourly from its UI tick; here
        // the sensor worker persists one reading a minute through the same
        // writer (a clone shares the thread), which is the newer of the two
        // paths and the one the History page reads back.
        let writer_for_workers = writer.clone();

        let world: WorldState = Arc::new(std::sync::RwLock::new(SystemState::new()));
        {
            let mut w = world_state::write(&world);
            w.sensor_names = names.clone();
            w.sensor_count = names.len();
            w.locations = locations.iter().map(|l| world_state::Location {
                id: l.id.clone(), name: l.name.clone(), latitude: None, longitude: None }).collect();
            w.current_location_idx = current_location;
            w.current_location = locations.get(current_location).map(|l| l.name.clone()).unwrap_or_default();
        }
        let (handles, queue) = systems::spawn_all(world.clone(), model.clone(), neural.clone(), writer_for_workers);
        note("workers running: sensor, model, environment, screen-temp".into());

        let (ask_tx, ask_rx) = channel::<Ask>();
        let (reply_tx, reply_rx) = channel::<Reply>();
        std::thread::Builder::new().name("ambient-store".into())
            .spawn(move || store_loop(db, ask_rx, reply_tx)).ok();
        let _ = ask_tx.send(Ask::Refresh);
        let _ = ask_tx.send(Ask::Week(0));

        Host {
            world, model, neural, writer, _handles: handles, queue,
            ask: ask_tx, reply: reply_rx,
            locations, current_location,
            forecast: ForecastModel::new(),
            last_forecast: None, last_refresh: Some(Instant::now()), last_frame: None,
            week_offset: 0, week_loaded: None,
            pending_cal: None, session_cals: Vec::new(),
            db_calibrations: 0, db_readings: 0, cal_log: Vec::new(),
            db_path, env_seen: [false; 3], log,
        }
    }

    /// How many of the Live panel's four readings this device produces.
    fn expected_readings(&mut self, env: &crate::environment::EnvSnapshot, thermal: bool) -> usize {
        for (i, present) in [env.humidity_pct.is_some(), env.pressure_hpa.is_some(), env.ambient_light.is_some()]
            .into_iter().enumerate() {
            if present { self.env_seen[i] = true; }
        }
        usize::from(thermal) + self.env_seen.iter().filter(|s| **s).count()
    }

    fn note(&mut self, s: impl Into<String>) {
        let s = s.into();
        println!("host: {s}");
        self.log.push(s);
        if self.log.len() > LOG_CAP { self.log.drain(..self.log.len() - LOG_CAP); }
    }

    fn current_location_id(&self) -> Option<String> {
        self.locations.get(self.current_location).map(|l| l.id.clone())
    }

    /// Once per frame: pull the world into the shell.
    pub fn tick(&mut self, shell: &mut Shell) {
        self.drain_replies(shell);
        self.feed_forecast(shell);
        self.read_frame(shell);
        self.settle_calibration(shell);

        // Periodic store refresh, so a calibration written by the writer
        // shows up in the counts without a restart.
        if self.last_refresh.map_or(true, |t| t.elapsed() >= STORE_REFRESH) {
            let _ = self.ask.send(Ask::Refresh);
            self.last_refresh = Some(Instant::now());
        }
        if self.week_loaded != Some(self.week_offset) {
            let _ = self.ask.send(Ask::Week(self.week_offset));
            self.week_loaded = Some(self.week_offset);
        }

        // The world, under a read lock for as long as the copies take.
        let w = world_state::read(&self.world);
        let health = w.sensor_health;
        let have_sensors = !w.sensors.is_empty() && health.is_ok();
        let temp = if have_sensors { Some(w.ambient_temp) } else { None };
        let conf = if have_sensors { Some(w.confidence) } else { None };
        let env = w.env.clone();
        let model_type = w.model_type.as_str().to_string();
        let series: Vec<f32> = {
            let n = w.temperature_history.len();
            let take = n.min(SPARK_POINTS);
            w.temperature_history.iter().skip(n - take).map(|(_, t)| *t).collect()
        };
        let pressure_trend = crate::environment::pressure_trend_hpa(
            w.pressure_history.iter().cloned().collect::<Vec<_>>().as_slice())
            .or(env.pressure_trend_hpa_per_30m);
        let calibrations = w.calibration_count;
        let (lin, neu) = (w.linear_calibrations, w.neural_calibrations);
        let sensor_count = w.sensor_count;
        let device = w.device_state.clone();
        drop(w);

        // Live: the sensor panel.
        shell.live = live::Live {
            temp_c: temp.map_or(live::Reading::missing(), |t| live::Reading::new(Some(t), 1)),
            humidity_pct: env.humidity_pct.map_or(live::Reading::missing(), |h| live::Reading::new(Some(h), 0)),
            pressure_hpa: env.pressure_hpa.map_or(live::Reading::missing(), |p| live::Reading::new(Some(p), 0)),
            light: env.ambient_light.map_or(live::Reading::missing(), |l| live::Reading::new(Some(l), 0)),
            confidence: conf,
            expected: Some(self.expected_readings(&env, sensor_count > 0)),
        };
        // Live: the headline. The eased display keeps its own state; only
        // the snapshot it eases toward is replaced.
        let (min_temp, max_temp) = shell.forecast.one_hour
            .map(|b| (b.low, b.high))
            .or_else(|| {
                let (lo, hi) = series.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
                if series.is_empty() { None } else { Some((lo, hi)) }
            })
            .unwrap_or((temp.unwrap_or(0.0), temp.unwrap_or(0.0)));
        let location = self.locations.get(self.current_location).map(|l| l.name.clone()).unwrap_or_default();
        let condition = if have_sensors && calibrations == 0 {
            // The linear model's prediction before any calibration is its
            // zero bias — the GTK build shows the same 0.0°C. Said plainly
            // rather than left for the user to work out.
            "uncalibrated — tap Calibrate".into()
        } else if have_sensors {
            format!("{} model", model_type.to_lowercase())
        } else {
            match health {
                world_state::SensorHealth::Unknown => "waiting for sensors".into(),
                _ => "no sensors".into(),
            }
        };
        let snapshot = LiveSnapshot {
            location,
            temperature: temp.unwrap_or(shell.live_state.snapshot.temperature),
            confidence: conf.unwrap_or(0.0),
            min_temp, max_temp,
            series,
            condition,
            updated: now_secs() as u64,
            pressure: env.pressure_hpa.map(|hpa| pressure::Pressure { hpa, trend_hpa_per_30m: pressure_trend }),
        };
        shell.live_state.update(snapshot);
        shell.sparkles.humidity_pct = env.humidity_pct.unwrap_or(0.0);
        shell.ambience = ambience_now(&env, pressure_trend);

        // The GTK status lines.
        shell.status = vec![
            format!("Active sensors: {sensor_count} | Calibrations: {calibrations} | Model: {model_type}"),
            match health {
                world_state::SensorHealth::Ok => "● Sensors locked".into(),
                world_state::SensorHealth::Degraded => "◐ Sensors degraded".into(),
                world_state::SensorHealth::Unavailable => "⚠ Sensors unavailable".into(),
                world_state::SensorHealth::Unknown => "○ Waiting for the first poll".into(),
            },
        ];

        // Model page.
        shell.model.stats = model_page::ModelStats {
            model_type: model_type.clone(),
            linear: lin as u32,
            neural: neu as u32,
            confidence: conf.unwrap_or(0.0),
        };
        shell.model.device = model_page::DeviceState {
            cpu_load: device.cpu_load, battery: device.battery_level, charging: device.charging,
        };
        let log: Vec<model_page::CalEntry> = self.cal_log.iter().rev().map(|(at, target)| {
            let source = if self.session_cals.contains(at) {
                model_page::Source::Egui
            } else {
                model_page::Source::Unknown(String::new())
            };
            model_page::CalEntry { source, target_c: *target, at: *at }
        }).collect();
        shell.model.set_log(log);

        // Sync page.
        shell.sync.db = sync_page::DbSummary {
            calibrations: self.db_calibrations as u32,
            linear: lin as u32,
            neural: neu as u32,
            model_type: model_type.clone(),
        };
        shell.sync.backend = Some(TransferBackend {
            can_export: self.db_path.is_some(),
            can_import: true,
            exports_locations: false,
        });
        shell.sync.export_dir = self.export_dir().display().to_string();

        // Places.
        shell.locations.set(self.locations.iter().enumerate().map(|(i, l)| loc_page::Location {
            id: i as i64,
            name: l.name.clone(),
            coords: None,
            current: i == self.current_location,
        }).collect());

        shell.host_log = self.log.clone();
    }

    /// Apply what a page asked for.
    pub fn apply(&mut self, action: Action, shell: &mut Shell) {
        match action {
            Action::Calibrate(CalibrateAction::Calibrate(target)) => self.calibrate(target, shell),
            Action::History(a) => {
                if a == HistoryAction::ExportCsv {
                    let path = self.export_dir().join(format!("ambient-history-{}.csv", engine::get_timestamp()));
                    let _ = self.ask.send(Ask::ExportCsv(path));
                } else if shell.history_nav.apply(a) {
                    self.week_offset = shell.history_nav.week_offset;
                    shell.history_nav.range = week_range(self.week_offset);
                }
            }
            Action::Location(a) => self.location(a, shell),
            Action::Sync(SyncAction::Export) => {
                let path = self.export_dir().join(format!("sensors_export_{}.db", engine::get_timestamp()));
                shell.sync.note = Some(format!("exporting to {}…", path.display()));
                let _ = self.ask.send(Ask::ExportDb(path));
            }
            Action::Sync(SyncAction::Import(p)) => {
                shell.sync.note = Some(format!("importing {p}…"));
                let _ = self.ask.send(Ask::ImportDb(PathBuf::from(p)));
            }
        }
    }

    fn export_dir(&self) -> PathBuf {
        self.db_path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from(DATA_DIR))
            .join("exports")
    }

    fn calibrate(&mut self, target: f32, shell: &mut Shell) {
        let (sensors, count_before) = {
            let w = world_state::read(&self.world);
            (w.sensors.clone(), w.calibration_count)
        };
        if sensors.is_empty() {
            self.note(format!("calibration to {target:.1}°C refused: no sensor readings yet"));
            shell.calibrate.settle(None, now_secs(), "no sensor readings yet — nothing to calibrate against".into());
            return;
        }
        let loc = self.current_location_id();
        let mut req = CalibrationRequest::new(target).with_source("egui-ui");
        if let Some(id) = &loc { req = req.with_location(id.clone()); }
        self.queue.push(req);
        // The database row goes through the writer now; the model files are
        // saved once the worker reports the calibration applied.
        let stored = self.writer.as_ref().map_or(false, |w| w.submit_calibration(&sensors, target, loc.as_deref()));
        self.session_cals.push(now_secs());
        self.note(format!("calibrating to {target:.1}°C from {} sensor(s){}", sensors.len(),
            if stored { ", row queued for the database" } else { " (no database — not stored)" }));
        self.pending_cal = Some(PendingCal { target, since: Instant::now(), count_before });
    }

    /// Has the model worker applied the pending calibration? Then save the
    /// model files, as the GTK dialog's thread does after its burst.
    fn settle_calibration(&mut self, shell: &mut Shell) {
        let Some(p) = &self.pending_cal else { return };
        let count = world_state::read(&self.world).calibration_count;
        let applied = count > p.count_before && self.queue.is_empty();
        let timed_out = p.since.elapsed() > Duration::from_secs(5);
        if !applied && !timed_out { return; }
        let target = p.target;
        self.pending_cal = None;
        if !applied {
            self.note(format!("calibration to {target:.1}°C: the model worker did not report it within 5s"));
            shell.calibrate.settle(None, now_secs(), "the model worker did not apply it — see the host log".into());
            return;
        }
        let dir = std::env::current_dir().map(|d| d.join(DATA_DIR)).unwrap_or_else(|_| PathBuf::from(DATA_DIR));
        let stamp = engine::get_timestamp();
        let model_path = dir.join(format!("model_{stamp}.json"));
        let saved_model = match self.model.lock() {
            Ok(mut m) => { m.prune_weak_sensors(); m.save(&model_path).map_err(|e| e.to_string()) }
            Err(_) => Err("model lock poisoned".into()),
        };
        let neural_path = dir.join(format!("neural_field_{stamp}.json"));
        let saved_neural = match self.neural.lock() {
            Ok(nf) => nf.save(&neural_path),
            Err(_) => Err("neural field lock poisoned".into()),
        };
        match (&saved_model, &saved_neural) {
            (Ok(()), Ok(())) => self.note(format!("calibrated to {target:.1}°C; saved {} and {}",
                model_path.display(), neural_path.display())),
            (m, n) => self.note(format!("calibrated to {target:.1}°C; model save: {:?}; neural save: {:?}", m, n)),
        }
        let msg = format!("calibrated to {target:.1}°C ({count} total)");
        shell.calibrate.settle(Some(target), now_secs(), msg);
        // The counts on the Sync page come from the store.
        let _ = self.ask.send(Ask::Refresh);
    }

    fn location(&mut self, a: LocationAction, shell: &mut Shell) {
        match a {
            LocationAction::SetCurrent(i) => {
                let i = i as usize;
                if i < self.locations.len() {
                    self.current_location = i;
                    let name = self.locations[i].name.clone();
                    let mut w = world_state::write(&self.world);
                    w.current_location_idx = i;
                    w.current_location = name.clone();
                    drop(w);
                    // Remembered across launches. The GTK build re-detects
                    // by WiFi on each start (when that is enabled); without
                    // detection the choice has to be kept somewhere.
                    let dir = std::env::current_dir().map(|d| d.join(DATA_DIR)).unwrap_or_else(|_| PathBuf::from(DATA_DIR));
                    if let Err(e) = std::fs::write(dir.join(CURRENT_LOCATION_FILE), &self.locations[i].id) {
                        self.note(format!("could not remember the location: {e}"));
                    }
                    self.note(format!("current location: {name}"));
                }
            }
            LocationAction::Add(name) => {
                let name = name.trim().to_string();
                if name.is_empty() { return; }
                self.locations.push(Location::new(name.clone(), LocationType::Unknown));
                self.save_locations();
                self.note(format!("added location {name}"));
            }
            LocationAction::Remove(i) => {
                let i = i as usize;
                if i < self.locations.len() {
                    let removed = self.locations.remove(i);
                    if self.current_location >= self.locations.len() {
                        self.current_location = self.locations.len().saturating_sub(1);
                    } else if i < self.current_location {
                        self.current_location -= 1;
                    }
                    self.save_locations();
                    self.note(format!("removed location {}", removed.name));
                }
            }
        }
        // The world's copy, for the sensor worker's location_id column.
        let mut w = world_state::write(&self.world);
        w.locations = self.locations.iter().map(|l| world_state::Location {
            id: l.id.clone(), name: l.name.clone(), latitude: None, longitude: None }).collect();
        w.current_location_idx = self.current_location;
        drop(w);
        let _ = shell;
    }

    fn save_locations(&mut self) {
        if let Err(e) = engine::save_locations(&self.locations) {
            self.note(format!("could not save locations: {e}"));
        }
    }

    fn drain_replies(&mut self, shell: &mut Shell) {
        while let Ok(r) = self.reply.try_recv() {
            match r {
                Reply::Week { offset, points } => {
                    if offset == self.week_offset {
                        shell.history = History::new(points);
                        shell.history_nav.range = week_range(offset);
                    }
                }
                Reply::Refresh { calibrations, readings, log } => {
                    self.db_calibrations = calibrations;
                    self.db_readings = readings;
                    self.cal_log = log;
                }
                Reply::Note { for_history, text } => {
                    self.note(text.clone());
                    if for_history { shell.history_nav.note = Some(text); }
                    else { shell.sync.note = Some(text); }
                }
            }
        }
    }

    fn feed_forecast(&mut self, shell: &mut Shell) {
        let due = self.last_forecast.map_or(true, |t| t.elapsed() >= FORECAST_EVERY);
        if !due { return; }
        // Only a prediction the model worker has actually made: before the
        // first one `ambient_temp` is the struct's 20.0 default, and a
        // forecast seeded from that is a forecast of nothing.
        let (temp, ok) = {
            let w = world_state::read(&self.world);
            (w.ambient_temp, !w.temperature_history.is_empty() && w.sensor_health.is_ok())
        };
        if !ok { return; }
        self.last_forecast = Some(Instant::now());
        self.forecast.add_observation(TimeSeriesPoint {
            timestamp: now_secs(), temperature: temp,
            location_id: self.current_location_id().unwrap_or_else(|| "current".into()),
        });
        let band = |f: Option<crate::forecast::Forecast>| f.map(|f| Band { temp: f.temperature, low: f.min_temp, high: f.max_temp });
        shell.forecast = ForecastCard {
            one_hour: band(self.forecast.forecast(1.0)),
            one_day: band(self.forecast.forecast(24.0)),
            observations: self.forecast.data_points(),
        };
    }

    /// The screen-temperature frame the screen-temp worker publishes, read
    /// back the way any other app would — from the file, not from memory.
    fn read_frame(&mut self, shell: &mut Shell) {
        if self.last_frame.map_or(false, |t| t.elapsed() < FRAME_EVERY) { return; }
        self.last_frame = Some(Instant::now());
        let path = crate::screen_temps::default_output_dir().join("screen-temps.bin");
        shell.screen_temps.state = match std::fs::read(&path) {
            Ok(bytes) => screen_temps::Frame::parse(&bytes),
            Err(_) => Err(screen_temps::FrameError::Missing),
        };
    }
}

/// The store thread: one SQLite read connection, questions in, answers out.
fn store_loop(db: Option<Database>, ask: Receiver<Ask>, reply: Sender<Reply>) {
    while let Ok(q) = ask.recv() {
        let Some(db) = db.as_ref() else {
            let _ = reply.send(Reply::Note { for_history: matches!(q, Ask::ExportCsv(_)),
                text: "no database".into() });
            continue;
        };
        let r = match q {
            Ask::Week(offset) => Reply::Week { offset, points: week_points(db, offset) },
            Ask::Refresh => {
                let (calibrations, readings, _, _) = db.get_database_stats().unwrap_or((0, 0, None, None));
                let log = db.get_calibration_history(365).unwrap_or_default();
                Reply::Refresh { calibrations, readings, log }
            }
            Ask::ExportCsv(path) => {
                std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).ok();
                Reply::Note { for_history: true, text: match db.export_to_csv(&path) {
                    Ok(()) => format!("exported {}", path.display()),
                    Err(e) => format!("CSV export failed: {e}"),
                } }
            }
            Ask::ExportDb(path) => Reply::Note { for_history: false, text: match engine::export_database_cli(&path) {
                Ok(()) => format!("exported {}", path.display()),
                Err(e) => format!("export failed: {e}"),
            } },
            Ask::ImportDb(src) => Reply::Note { for_history: false, text: match engine::import_database_cli(&src) {
                Ok(dst) => format!("imported as {} — it becomes the active database on the next start", dst.display()),
                Err(e) => format!("import failed: {e}"),
            } },
        };
        if reply.send(r).is_err() { return; }
    }
}

/// The week's daily averages as chart points, oldest first.
fn week_points(db: &Database, offset: usize) -> Vec<Point> {
    let mut days = db.get_daily_averages(offset * 7, 7, None);
    days.reverse();
    days.into_iter().filter_map(|(day, avg)| {
        let d = chrono::NaiveDate::parse_from_str(&day, "%Y-%m-%d").ok()?;
        let at = d.and_hms_opt(12, 0, 0)?.and_utc().timestamp();
        Some(Point { at, temp_c: avg })
    }).collect()
}

/// "Sep 21 – Sep 27" for a week offset, matching the query's window.
fn week_range(offset: usize) -> String {
    let today = chrono::Local::now().date_naive();
    let end = today - chrono::Duration::days((offset * 7) as i64);
    let start = end - chrono::Duration::days(6);
    format!("{} – {}", start.format("%b %-d"), end.format("%b %-d"))
}

/// The remembered location id, if any.
fn current_location_file(data_dir: &Path) -> Option<String> {
    let id = std::fs::read_to_string(data_dir.join(CURRENT_LOCATION_FILE)).ok()?;
    let id = id.trim();
    if id.is_empty() { None } else { Some(id.to_string()) }
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The mood background's inputs: the season and time of day from the
/// clock, tilt and light from the environment sensors.
fn ambience_now(env: &crate::environment::EnvSnapshot, pressure_trend: Option<f32>)
    -> crate::ui::egui::ambience::Ambience {
    use crate::ui::egui::ambience::{Ambience, Phase, Season};
    let mood = crate::seasons::Mood::now_local();
    let season = match mood.season {
        crate::seasons::Season::Winter => Season::Winter,
        crate::seasons::Season::Spring => Season::Spring,
        crate::seasons::Season::Summer => Season::Summer,
        crate::seasons::Season::Autumn => Season::Autumn,
    };
    let phase = match mood.phase {
        crate::seasons::DayPhase::Dawn => Phase::Dawn,
        crate::seasons::DayPhase::Morning | crate::seasons::DayPhase::Afternoon => Phase::Day,
        crate::seasons::DayPhase::Dusk | crate::seasons::DayPhase::Evening => Phase::Evening,
        crate::seasons::DayPhase::Night => Phase::Night,
    };
    let tilt = match env.accel_tilt.as_deref().or(env.accel_orientation.as_deref()) {
        Some("left-up") => (-0.5, 0.0),
        Some("right-up") => (0.5, 0.0),
        Some("bottom-up") => (0.0, 0.5),
        _ => (0.0, 0.0),
    };
    // Lux to 0..=1 on a log scale: 1 lx is dark, 1000 lx is a bright room.
    let light_norm = env.ambient_light.map(|l| (l.max(1.0).log10() / 3.0).clamp(0.0, 1.0)).unwrap_or(0.5);
    Ambience { season, phase, tilt, light_norm, pressure_trend: pressure_trend.unwrap_or(0.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create_at(&dir.path().join("sensors_20260101_000000.db")).unwrap();
        (dir, db)
    }

    fn store_reading_at(db: &Database, at: i64, temp: f32) {
        db.conn.execute(
            "INSERT INTO readings (timestamp, data, location_id) VALUES (?1, ?2, NULL)",
            rusqlite::params![at, format!("{{\"time\":\"{at}\",\"sensors\":[{temp}],\"cpu_load\":0.0,\"als_lux\":0,\"charging\":false,\"screen\":100}}")],
        ).unwrap();
    }

    #[test]
    fn a_week_of_readings_becomes_ascending_daily_points() {
        // The engine's query returns days newest-first; the chart wants
        // oldest-first, one point per day at noon.
        let (_d, db) = temp_db();
        let now = now_secs();
        for day in 0..3 {
            for hour in [9, 15] {
                store_reading_at(&db, now - day * 86_400 - hour * 3_600 - 1, 20.0 + day as f32);
            }
        }
        let pts = week_points(&db, 0);
        assert!(pts.len() >= 2 && pts.len() <= 4, "three days, maybe split by UTC midnight: {pts:?}");
        assert!(pts.windows(2).all(|w| w[0].at < w[1].at), "ascending: {pts:?}");
        // Today's mean is 20.0, the oldest day's 22.0: the line falls
        // toward now.
        assert!(pts.first().unwrap().temp_c >= pts.last().unwrap().temp_c);
    }

    #[test]
    fn the_calibration_log_reads_the_column_the_table_has() {
        // `get_calibration_history` selected `target_temp`, which the
        // table never had; the Model page's log was empty forever.
        let (_d, db) = temp_db();
        db.store_calibration(&[30.0, 31.0], 21.5, None).unwrap();
        let log = db.get_calibration_history(1).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].1, 21.5);
    }

    #[test]
    fn the_store_answers_over_the_channel_and_exports_csv() {
        let (dir, db) = temp_db();
        store_reading_at(&db, now_secs() - 100, 21.0);
        store_reading_at(&db, now_secs() - 200, 23.0);
        let (ask_tx, ask_rx) = channel();
        let (reply_tx, reply_rx) = channel();
        std::thread::spawn(move || store_loop(Some(db), ask_rx, reply_tx));
        ask_tx.send(Ask::Refresh).unwrap();
        match reply_rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Reply::Refresh { readings, calibrations, .. } => { assert_eq!(readings, 2); assert_eq!(calibrations, 0); }
            _ => panic!("expected a refresh"),
        }
        let csv = dir.path().join("out").join("h.csv");
        ask_tx.send(Ask::ExportCsv(csv.clone())).unwrap();
        match reply_rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Reply::Note { for_history: true, text } => assert!(text.starts_with("exported"), "{text}"),
            _ => panic!("expected a note"),
        }
        let body = std::fs::read_to_string(&csv).unwrap();
        assert!(body.starts_with("Date,Average Temperature"), "{body}");
        assert!(body.lines().count() >= 2, "a header and at least one day: {body}");
    }

    #[test]
    fn the_week_label_covers_seven_days_ending_on_the_offsets_last_day() {
        let this = week_range(0);
        assert!(this.contains('–'));
        assert_ne!(week_range(1), this);
    }

    #[test]
    fn ambience_maps_lux_on_a_log_scale_and_defaults_to_half_without_a_sensor() {
        let mut env = crate::environment::EnvSnapshot::default();
        assert!((ambience_now(&env, None).light_norm - 0.5).abs() < 1e-6);
        env.ambient_light = Some(1.0);
        assert_eq!(ambience_now(&env, None).light_norm, 0.0, "1 lx is dark");
        env.ambient_light = Some(1000.0);
        assert!((ambience_now(&env, None).light_norm - 1.0).abs() < 1e-6, "1000 lx is bright");
        env.ambient_light = Some(1e9);
        assert_eq!(ambience_now(&env, None).light_norm, 1.0, "clamped");
    }
}
