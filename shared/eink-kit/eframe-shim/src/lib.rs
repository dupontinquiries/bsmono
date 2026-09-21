//! eframe's API on eink-kit. See Cargo.toml for how it is swapped in.
//!
//! Frame order matches eframe 0.34's `epi_integration`: inside `run_ui` with
//! the root `Ui`, `App::logic`, then the deprecated `App::update`, then
//! `App::ui`. Not supported (and so absent, a compile error rather than a
//! silent no-op): `glow` / `wgpu` access, paint callbacks, winit window and
//! event-loop hooks. `raw_input_hook` is not called (eink-kit builds the
//! raw input itself). Viewport commands are accepted and ignored: there is
//! one full-screen surface.

pub use egui;
pub use egui::{emath, epaint};

use std::collections::HashMap;
use std::time::{Duration, Instant};

pub type DynError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug)]
pub enum Error {
    AppCreation(DynError),
    /// The e-ink display could not be opened, or went away.
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::AppCreation(e) => write!(f, "app creation error: {e}"),
            Error::Io(e) => write!(f, "e-ink display: {e}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T = (), E = Error> = std::result::Result<T, E>;

/// What eframe calls its window options. Only what the apps set; window
/// geometry is meaningless on a fixed full-screen panel and is ignored.
pub struct NativeOptions {
    pub viewport: egui::ViewportBuilder,
    pub vsync: bool,
    pub multisampling: u16,
    pub depth_buffer: u8,
    pub stencil_buffer: u8,
    pub hardware_acceleration: HardwareAcceleration,
    pub renderer: Renderer,
    pub run_and_return: bool,
    pub centered: bool,
    pub persist_window: bool,
    pub persistence_path: Option<std::path::PathBuf>,
    pub dithering: bool,
}

impl Default for NativeOptions {
    fn default() -> Self {
        NativeOptions {
            viewport: Default::default(),
            vsync: true,
            multisampling: 0,
            depth_buffer: 0,
            stencil_buffer: 0,
            hardware_acceleration: HardwareAcceleration::Preferred,
            renderer: Renderer::default(),
            run_and_return: true,
            centered: false,
            persist_window: true,
            persistence_path: None,
            dithering: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Renderer {
    #[default]
    Glow,
    Wgpu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HardwareAcceleration {
    Required,
    Preferred,
    Off,
}

#[derive(Clone, Debug, Default)]
pub struct IntegrationInfo {
    pub cpu_usage: Option<f32>,
}

/// Key-value persistence, as eframe's.
pub trait Storage {
    fn get_string(&self, key: &str) -> Option<String>;
    fn set_string(&mut self, key: &str, value: String);
    fn flush(&mut self);
}

pub const APP_KEY: &str = "app";

pub fn get_value<T: serde::de::DeserializeOwned>(storage: &dyn Storage, key: &str) -> Option<T> {
    storage.get_string(key).and_then(|s| serde_json::from_str(&s).ok())
}

pub fn set_value<T: serde::Serialize>(storage: &mut dyn Storage, key: &str, value: &T) {
    if let Ok(s) = serde_json::to_string(value) {
        storage.set_string(key, s);
    }
}

/// A JSON file per app under the data dir (persistent `/home` on a tablet).
pub struct FileStorage {
    path: std::path::PathBuf,
    map: HashMap<String, String>,
    dirty: bool,
}

impl FileStorage {
    pub fn for_app(app_name: &str) -> FileStorage {
        let base = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let safe: String = app_name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        let path = base.join(safe).join("eframe.json");
        let map = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        FileStorage { path, map, dirty: false }
    }
}

impl Storage for FileStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.map.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        if self.map.get(key) != Some(&value) {
            self.map.insert(key.to_string(), value);
            self.dirty = true;
        }
    }
    fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string(&self.map) {
            // write-then-rename: a flash write interrupted by power loss
            // must not leave a truncated file
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, s).is_ok() && std::fs::rename(&tmp, &self.path).is_ok() {
                self.dirty = false;
            }
        }
    }
}

pub struct Frame {
    info: IntegrationInfo,
    storage: Option<Box<dyn Storage>>,
}

impl Frame {
    pub fn is_web(&self) -> bool {
        false
    }
    pub fn info(&self) -> &IntegrationInfo {
        &self.info
    }
    pub fn storage(&self) -> Option<&dyn Storage> {
        self.storage.as_deref()
    }
    pub fn storage_mut(&mut self) -> Option<&mut (dyn Storage + 'static)> {
        self.storage.as_deref_mut()
    }
}

pub struct CreationContext<'s> {
    pub egui_ctx: egui::Context,
    pub integration_info: IntegrationInfo,
    pub storage: Option<&'s dyn Storage>,
}

pub trait App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut Frame) {
        _ = (ctx, frame);
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame);
    #[deprecated = "Use Self::ui instead"]
    fn update(&mut self, ctx: &egui::Context, frame: &mut Frame) {
        _ = (ctx, frame);
    }
    fn save(&mut self, _storage: &mut dyn Storage) {}
    fn on_exit(&mut self) {}
    fn auto_save_interval(&self) -> Duration {
        Duration::from_secs(30)
    }
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
    fn persist_egui_memory(&self) -> bool {
        true
    }
    fn raw_input_hook(&mut self, _ctx: &egui::Context, _raw_input: &mut egui::RawInput) {}
}

pub type AppCreator<'app> = Box<dyn 'app + FnOnce(&CreationContext<'_>) -> std::result::Result<Box<dyn 'app + App>, DynError>>;

/// The eframe app, driven as eink-kit's app.
struct Shim<'a> {
    creator: Option<AppCreator<'a>>,
    app: Option<Box<dyn App + 'a>>,
    frame: Frame,
    last_save: Instant,
    error: Option<DynError>,
}

impl<'a> Shim<'a> {
    fn save(&mut self) {
        if let (Some(app), Some(storage)) = (self.app.as_mut(), self.frame.storage.as_deref_mut()) {
            app.save(storage);
            storage.flush();
        }
    }
}

impl<'a> eink_kit::EinkApp for Shim<'a> {
    fn ui(&mut self, ui: &mut egui::Ui) {
        if self.app.is_none() {
            let Some(creator) = self.creator.take() else { return };
            let cc = CreationContext {
                egui_ctx: ui.ctx().clone(),
                integration_info: self.frame.info.clone(),
                storage: self.frame.storage.as_deref(),
            };
            match creator(&cc) {
                Ok(app) => self.app = Some(app),
                Err(e) => {
                    eprintln!("eframe(eink): app creation failed: {e}");
                    self.error = Some(e);
                    std::process::exit(1);
                }
            }
        }
        let app = self.app.as_mut().unwrap();
        let ctx = ui.ctx().clone();
        app.logic(&ctx, &mut self.frame);
        #[allow(deprecated)]
        app.update(&ctx, &mut self.frame);
        app.ui(ui, &mut self.frame);
        let interval = app.auto_save_interval();
        if self.last_save.elapsed() >= interval {
            self.save();
            self.last_save = Instant::now();
        }
    }
}

impl<'a> Drop for Shim<'a> {
    fn drop(&mut self) {
        self.save();
        if let Some(app) = self.app.as_mut() {
            app.on_exit();
        }
    }
}

/// Run an eframe app on the e-ink display (eink-kit picks the backend).
pub fn run_native(app_name: &str, native_options: NativeOptions, app_creator: AppCreator<'_>) -> Result {
    let _ = native_options; // geometry/renderer choices do not apply to a fixed panel
    let storage: Box<dyn Storage> = Box::new(FileStorage::for_app(app_name));
    let shim = Shim {
        creator: Some(app_creator),
        app: None,
        frame: Frame { info: IntegrationInfo::default(), storage: Some(storage) },
        last_save: Instant::now(),
        error: None,
    };
    eink_kit::run(shim).map_err(Error::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Counter(u32);
    impl App for Counter {
        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut Frame) {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                if ui.button("tap").clicked() {
                    self.0 += 1;
                }
            });
        }
    }

    /// An unmodified eframe-style app, driven through the shim by eink-kit's
    /// engine on the in-memory panel: it creates, draws, and persists.
    #[test]
    fn an_eframe_app_runs_on_the_engine_and_persists() {
        use eink_kit::present::{mem::MemPresent, Present};
        let dir = std::env::temp_dir().join(format!("efshim_{}", std::process::id()));
        std::env::set_var("XDG_DATA_HOME", &dir);
        let cfg = eink_kit::config::Config::from_env().unwrap();
        let mut p = MemPresent::new(400, 600, eink_kit::format::PixelFormat::Rgb565, true, false);
        let mut e = eink_kit::Engine::new(&cfg, p.caps(), "mem");
        struct Saves(u32);
        impl App for Saves {
            fn ui(&mut self, ui: &mut egui::Ui, _f: &mut Frame) { ui.label("hello"); self.0 += 1; }
            fn save(&mut self, s: &mut dyn Storage) { set_value(s, APP_KEY, &self.0); }
            fn auto_save_interval(&self) -> Duration { Duration::ZERO }
        }
        let mut shim = Shim {
            creator: Some(Box::new(|cc| {
                assert!(cc.storage.is_some());
                Ok(Box::new(Saves(0)))
            })),
            app: None,
            frame: Frame { info: IntegrationInfo::default(), storage: Some(Box::new(FileStorage::for_app("shim test"))) },
            last_save: Instant::now(),
            error: None,
        };
        let r = e.frame(Duration::ZERO, &mut shim, &mut p).unwrap();
        assert!(!r.updates.is_empty(), "drew");
        drop(shim);
        let s = FileStorage::for_app("shim test");
        assert!(get_value::<u32>(&s, APP_KEY).is_some(), "saved through eframe's storage API");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = Counter(0);
    }
}
