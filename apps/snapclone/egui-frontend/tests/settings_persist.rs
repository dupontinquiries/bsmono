//! The settings page's Save, end to end: tap the button, then read the FILE.
//!
//! One test in its own binary because it points XDG_CONFIG_HOME at a scratch directory,
//! and an environment variable is process-wide -- inside the lib's test binary it would
//! race every other test that constructs the app.
use snapclone_egui::Snapclone;
use uikit_egui::Adaptive;

type Glyphs = Vec<(String, egui::Rect)>;

fn frame(ctx: &egui::Context, app: &mut Snapclone, events: Vec<egui::Event>) -> Glyphs {
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 4000.0));
    let out = ctx.run(egui::RawInput { events, screen_rect: Some(screen), ..Default::default() }, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| app.ui(ui, &Adaptive::default()));
    });
    uikit_egui::layout_probe::texts(&out.shapes).into_iter().map(|t| (t.text.clone(), t.rect)).collect()
}

fn click(ctx: &egui::Context, app: &mut Snapclone, at: egui::Pos2) {
    let b = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame(ctx, app, vec![egui::Event::PointerMoved(at), b(true)]);
    frame(ctx, app, vec![b(false)]);
    frame(ctx, app, vec![]);
}

#[test]
fn the_pages_save_button_writes_the_file_and_the_theme_applies() {
    let dir = std::env::temp_dir().join(format!("snapclone-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY-ish: this binary has exactly one test, so nothing else reads the env.
    std::env::set_var("XDG_CONFIG_HOME", &dir);
    let file = dir.join("snapclone").join("settings.conf");

    let ctx = egui::Context::default();
    let mut app = Snapclone::new(Vec::new());
    app.nav.go_page(4);
    let glyphs = frame(&ctx, &mut app, vec![]);

    // ONE save control. The second button ("Save to disk") was the one that wrote; the
    // first only committed in memory and greyed the second out.
    let saves: Vec<&(String, egui::Rect)> = glyphs.iter().filter(|(t, _)| t.starts_with("Save")).collect();
    assert_eq!(saves.len(), 1, "save controls on the page: {:?}", saves.iter().map(|s| &s.0).collect::<Vec<_>>());

    // Edit: a name with the characters the flat file used to cut at, and a theme.
    app.settings.current.general.display_name = "Ada #1; the one".into();
    let theme = snapclone_theme::PaletteId::all().iter().find(|p| p.as_str() != app.settings.current.general.palette).unwrap().as_str();
    app.settings.current.general.palette = theme.to_string();
    assert_eq!(app.active_palette(), theme, "the picker previews the theme before Save");
    let glyphs = frame(&ctx, &mut app, vec![]);
    let save = glyphs.iter().find(|(t, _)| t == "Save").expect("the Save button").1;
    click(&ctx, &mut app, save.center());

    assert!(!app.settings.is_dirty(), "Save left the page dirty");
    let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("Save wrote no file at {}: {e}", file.display()));
    let back = snapclone_egui::settings_store::parse(&text);
    assert_eq!(back.general.display_name, "Ada #1; the one", "the name was cut on the way through the file");
    assert_eq!(back.general.palette, theme);

    // The theme survives the per-frame cross-app prefs pass (which used to reset it)...
    app.apply_settings(&ctx, &settings_kit::Prefs::default());
    assert_eq!(app.active_palette(), theme, "the default prefs pass overwrote the saved theme");
    // ...and a restart: a fresh app reads it back and draws with it.
    let fresh = Snapclone::new(Vec::new());
    assert_eq!(fresh.active_palette(), theme, "a restarted app does not use the saved theme");

    // The camera tuning persists too, written once the pointer is up.
    app.camera_policy.startup_timeout_ms = 4_321;
    frame(&ctx, &mut app, vec![]);
    let tuning = std::fs::read_to_string(dir.join("snapclone").join("camera_policy.conf")).expect("tuning file");
    assert_eq!(snapclone_egui::camera_policy::store::parse(&tuning).startup_timeout_ms, 4_321);
    assert_eq!(Snapclone::new(Vec::new()).camera_policy.startup_timeout_ms, 4_321, "tuning not loaded at startup");

    // A persist-on-change control saves ITSELF and leaves another pending edit pending.
    app.settings.current.general.display_name = "half typed".into();
    app.set_capture_flash_bright(!app.settings.saved().camera.capture_flash_bright);
    assert!(app.settings.is_dirty(), "the flash toggle committed an unrelated pending edit");
    let back = snapclone_egui::settings_store::parse(&std::fs::read_to_string(&file).unwrap());
    assert_eq!(back.general.display_name, "Ada #1; the one", "the flash toggle wrote a half-typed name");
    assert_eq!(back.camera.capture_flash_bright, app.settings.saved().camera.capture_flash_bright);

    let _ = std::fs::remove_dir_all(&dir);
}
