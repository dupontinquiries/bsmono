//! rotation_tab.rs — rotation policy editor + logrotate config viewer
//!
//! On load_file():
//!   1. Probe file ownership/tier
//!   2. Search for existing logrotate configs that mention this file
//!   3. Show probe results, existing config (if found), editable policy,
//!      token drop rules, and a generated config preview with Save button.

use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, DropDown, Entry, Label, ListBox,
    Orientation, PolicyType, ScrolledWindow, SelectionMode, Separator,
    StringList, TextBuffer, TextView,
};
use libadwaita as adw;
use adw::prelude::*;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::probe::{probe, RotationBackend, Tier};
use crate::rotation::{
    save_policy, write_logrotate_conf, write_systemd_user_units, RotationPolicy,
};

use regex;

// ── public struct ─────────────────────────────────────────────────────────────

pub struct RotationTab {
    pub root: GtkBox,
    token_rules_box:    Rc<RefCell<Option<ListBox>>>,
    token_rules_scroll: Rc<RefCell<Option<ScrolledWindow>>>,
    /// Current policy — updated as controls change, read on Apply
    policy: Rc<RefCell<Option<Rc<RefCell<crate::rotation::RotationPolicy>>>>>,
}

impl RotationTab {
    pub fn new() -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.set_margin_start(16);
        root.set_margin_end(16);
        root.set_margin_top(12);
        root.set_margin_bottom(12);

        let placeholder = Label::builder()
            .label("Open a file to configure rotation")
            .halign(gtk4::Align::Center)
            .valign(gtk4::Align::Center)
            .vexpand(true)
            .build();
        placeholder.add_css_class("dim-label");
        root.append(&placeholder);

        RotationTab {
            root,
            token_rules_box:    Rc::new(RefCell::new(None)),
            token_rules_scroll: Rc::new(RefCell::new(None)),
            policy:             Rc::new(RefCell::new(None)),
        }
    }

    /// Called from tab 2 "→ Filter" button.
    pub fn add_token_filter(&self, token: &str) {
        let rules_box = self.token_rules_box.borrow();
        let scroll    = self.token_rules_scroll.borrow();
        if let (Some(lb), Some(sw)) = (rules_box.as_ref(), scroll.as_ref()) {
            if lb.observe_children().n_items() == 1 {
                if let Some(placeholder) = lb.row_at_index(0) {
                    lb.remove(&placeholder);
                }
            }
            lb.append(&build_token_rule_row(token));
            let adj = sw.vadjustment();
            glib::idle_add_local_once(move || {
                adj.set_value(adj.upper() - adj.page_size());
            });
        }

        // Also push a Drop rule into the policy's pipeline config
        if let Some(policy_rc) = self.policy.borrow().as_ref() {
            use crate::pipeline::{FilterAction, FilterRule, PipelineConfig};
            let mut pol = policy_rc.borrow_mut();
            let log_path = pol.log_path.clone();
            let cfg = pol.pipeline.get_or_insert_with(|| PipelineConfig {
                log_path: log_path.clone(),
                ..Default::default()
            });
            // Only add if not already present
            if !cfg.rules.iter().any(|r| r.pattern == token) {
                if let Ok(rule) = FilterRule::new(
                    &format!("drop-token:{token}"),
                    &regex::escape(token),
                    FilterAction::Drop,
                ) {
                    cfg.rules.push(rule);
                }
            }
        }
    }

    pub fn load_file(&self, path: PathBuf) {
        while let Some(child) = self.root.first_child() {
            self.root.remove(&child);
        }
        *self.token_rules_box.borrow_mut()    = None;
        *self.token_rules_scroll.borrow_mut() = None;
        *self.policy.borrow_mut()             = None;

        let result = probe(&path);
        let policy: Rc<RefCell<RotationPolicy>> = Rc::new(RefCell::new(RotationPolicy {
            log_path: path.clone(),
            tier:     result.tier.clone(),
            backend:  result.rotation_backend.clone(),
            ..Default::default()
        }));
        // Store for add_token_filter() to reach
        *self.policy.borrow_mut() = Some(Rc::clone(&policy));

        // ── 1. Probe banner ───────────────────────────────────────────────
        let probe_group = adw::PreferencesGroup::new();
        probe_group.set_title("File ownership");
        probe_group.set_description(Some(&format!(
            "{}  ·  {}  ·  perms {}",
            result.owner_name.as_deref().unwrap_or("?"),
            result.group_name.as_deref().unwrap_or("?"),
            result.permissions_octal,
        )));

        probe_group.add(&action_row("Tier", &tier_label(&result.tier)));
        probe_group.add(&action_row("Rotation backend",
            &backend_label(&result.rotation_backend)));
        probe_group.add(&action_row("Writable by current user",
            if result.is_writable_by_us { "yes" }
            else { "no — will need privilege" }));
        for note in &result.notes {
            probe_group.add(&action_row("Note", note));
        }
        self.root.append(&probe_group);

        // ── 2. Existing logrotate config search ───────────────────────────
        self.root.append(&hsep(12));

        let existing = find_existing_configs(&path);
        let existing_group = adw::PreferencesGroup::new();
        existing_group.set_title("Existing logrotate config");

        if existing.is_empty() {
            existing_group.set_description(Some(
                "No existing logrotate config found for this file.\n\
                 Common locations searched: /etc/logrotate.d/, /etc/logrotate.conf, \
                 ~/.config/logrotate/"
            ));
        } else {
            existing_group.set_description(Some(&format!(
                "{} config(s) found — showing first match",
                existing.len()
            )));

            // Show the found config path as an action row
            for found_path in &existing {
                let row = adw::ActionRow::new();
                row.set_title(&found_path.display().to_string());
                row.set_subtitle("Click to load into editor below");
                row.set_activatable(true);
                // "open in editor" icon
                let icon = gtk4::Image::from_icon_name("document-edit-symbolic");
                row.add_suffix(&icon);
                row.set_activatable_widget(Some(&icon));

                let path2 = found_path.clone();
                let policy_ref = Rc::clone(&policy);
                let _self_root = self.root.clone();
                row.connect_activate(move |_| {
                    if let Ok(text) = std::fs::read_to_string(&path2) {
                        // Find the editor TextView by traversing the widget tree
                        // (simpler: we store the buffer in policy later)
                        // For now: show a toast via window — ideally pass toast_overlay in
                        // Just update the preview buffer if we can reach it.
                        // We'll push into the Rc<policy> raw_existing field.
                        policy_ref.borrow_mut().raw_existing = Some(text);
                    }
                });
                existing_group.add(&row);
            }
        }
        self.root.append(&existing_group);

        // Editable config text area (loads existing config if found, else empty)
        self.root.append(&hsep(4));

        let existing_label = Label::builder()
            .label("Config editor")
            .halign(gtk4::Align::Start)
            .build();
        existing_label.add_css_class("heading");
        self.root.append(&existing_label);

        let existing_hint = Label::builder()
            .label("Edit an existing config or start from the generated preview below. \
                   Changes are saved when you click Save.")
            .halign(gtk4::Align::Start)
            .wrap(true)
            .build();
        existing_hint.add_css_class("dim-label");
        self.root.append(&existing_hint);

        let editor_buf = TextBuffer::new(None);
        // Pre-populate with first found config, or a starter comment
        if let Some(found_path) = existing.first() {
            if let Ok(text) = std::fs::read_to_string(found_path) {
                editor_buf.set_text(&text);
            }
        } else {
            editor_buf.set_text(&format!(
                "# No existing config found.\n# Edit below or use the generated preview.\n"
            ));
        }

        let editor_view = TextView::builder()
            .buffer(&editor_buf)
            .editable(true)
            .monospace(true)
            .left_margin(8).right_margin(8)
            .top_margin(8).bottom_margin(8)
            .build();
        editor_view.add_css_class("rot-editor");

        let editor_scroll = ScrolledWindow::builder()
            .height_request(160)
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .child(&editor_view)
            .build();
        self.root.append(&editor_scroll);

        // Save editor button
        let save_row = GtkBox::new(Orientation::Horizontal, 8);
        save_row.set_margin_top(4);
        save_row.set_halign(gtk4::Align::End);

        let save_target_label = Label::new(Some(
            &existing.first().map(|p| p.display().to_string())
                .unwrap_or_else(|| "— no target path —".to_owned())
        ));
        save_target_label.add_css_class("dim-label");
        save_target_label.set_ellipsize(gtk4::pango::EllipsizeMode::Start);

        let save_editor_btn = Button::with_label("Save config");
        save_editor_btn.add_css_class("suggested-action");

        let save_path = existing.first().cloned();
        let buf_ref   = editor_buf.clone();
        let backend   = result.rotation_backend.clone();
        save_editor_btn.connect_clicked(move |btn| {
            let text = buf_ref.text(&buf_ref.start_iter(), &buf_ref.end_iter(), false);
            let target = match &save_path {
                Some(p) => p.clone(),
                None => {
                    btn.set_label("No path — use Apply below");
                    return;
                }
            };
            match write_raw_config(&text, &target, &backend) {
                Ok(_)  => { btn.set_label("Saved ✓"); btn.set_sensitive(false); }
                Err(e) => { btn.set_label(&format!("Error: {e}")); }
            }
        });

        save_row.append(&save_target_label);
        save_row.append(&save_editor_btn);
        self.root.append(&save_row);

        // ── 3. Pipeline config ────────────────────────────────────────────
        self.root.append(&hsep(12));

        let pipeline_group = adw::PreferencesGroup::new();
        pipeline_group.set_title("Filter pipeline");
        pipeline_group.set_description(Some(
            "Applied before rotation — drop noisy lines, collapse repeats",
        ));

        let level_row = adw::ActionRow::new();
        level_row.set_title("Minimum log level to keep");
        level_row.set_subtitle("Lines below this level are dropped");
        let level_items = StringList::new(&["all (no filter)", "debug", "info", "warn", "error"]);
        let level_combo = DropDown::new(Some(level_items), gtk4::Expression::NONE);
        level_combo.set_selected(0);
        level_combo.set_valign(gtk4::Align::Center);
        level_row.add_suffix(&level_combo);
        pipeline_group.add(&level_row);

        let disp_row = adw::ActionRow::new();
        disp_row.set_title("After filtering");
        let disp_items = StringList::new(&[
            "keep filtered (in-place)",
            "archive filtered copy",
            "summarize + archive",
            "delete after summarizing",
        ]);
        let disp_combo = DropDown::new(Some(disp_items), gtk4::Expression::NONE);
        disp_combo.set_selected(0);
        disp_combo.set_valign(gtk4::Align::Center);
        disp_row.add_suffix(&disp_combo);
        pipeline_group.add(&disp_row);

        let collapse_row = adw::SwitchRow::new();
        collapse_row.set_title("Collapse repeated lines");
        collapse_row.set_subtitle("'N× <line>' instead of N identical lines");
        collapse_row.set_active(true);
        pipeline_group.add(&collapse_row);

        self.root.append(&pipeline_group);

        // ── 4. Token drop rules ───────────────────────────────────────────
        self.root.append(&hsep(12));

        let rules_header = Label::builder()
            .label("Token drop rules")
            .halign(gtk4::Align::Start)
            .build();
        rules_header.add_css_class("heading");
        self.root.append(&rules_header);

        let rules_hint = Label::builder()
            .label("Lines containing these tokens are dropped before rotation. \
                   Add from Token Frequencies tab.")
            .halign(gtk4::Align::Start)
            .wrap(true)
            .build();
        rules_hint.add_css_class("dim-label");
        self.root.append(&rules_hint);

        let token_rules_lb = ListBox::new();
        token_rules_lb.set_selection_mode(SelectionMode::None);
        token_rules_lb.add_css_class("boxed-list");
        token_rules_lb.set_margin_top(8);

        let empty_row = adw::ActionRow::new();
        empty_row.set_title("No token rules yet");
        empty_row.set_subtitle("Click the funnel icon on a token in Token Frequencies");
        token_rules_lb.append(&empty_row);

        let rules_scroll = ScrolledWindow::builder()
            .vscrollbar_policy(PolicyType::Automatic)
            .hscrollbar_policy(PolicyType::Never)
            .max_content_height(220)
            .propagate_natural_height(true)
            .child(&token_rules_lb)
            .build();
        self.root.append(&rules_scroll);

        *self.token_rules_box.borrow_mut()    = Some(token_rules_lb.clone());
        *self.token_rules_scroll.borrow_mut() = Some(rules_scroll);

        // ── 5. Rotation policy ────────────────────────────────────────────
        self.root.append(&hsep(12));

        let rot_group = adw::PreferencesGroup::new();
        rot_group.set_title("Rotation policy");

        let freq_row = adw::ActionRow::new();
        freq_row.set_title("Frequency");
        let freq_combo = DropDown::new(
            Some(StringList::new(&["daily", "weekly", "monthly", "hourly", "on size"])),
            gtk4::Expression::NONE,
        );
        freq_combo.set_selected(0);
        freq_combo.set_valign(gtk4::Align::Center);
        freq_row.add_suffix(&freq_combo);
        rot_group.add(&freq_row);

        let keep_row = adw::ActionRow::new();
        keep_row.set_title("Keep rotated files");
        let keep_entry = Entry::builder()
            .text("7").width_chars(4).valign(gtk4::Align::Center).build();
        keep_row.add_suffix(&keep_entry);
        rot_group.add(&keep_row);

        let compress_row = adw::ActionRow::new();
        compress_row.set_title("Compression");
        let compress_combo = DropDown::new(
            Some(StringList::new(&["auto-detect best", "pigz (parallel gzip)", "zstd", "gzip", "xz", "none"])),
            gtk4::Expression::NONE,
        );
        compress_combo.set_selected(0);
        compress_combo.set_valign(gtk4::Align::Center);
        compress_row.add_suffix(&compress_combo);
        rot_group.add(&compress_row);

        let delay_row = adw::SwitchRow::new();
        delay_row.set_title("Delay compress");
        delay_row.set_subtitle("Don't compress the most recent rotated file");
        delay_row.set_active(true);
        rot_group.add(&delay_row);

        self.root.append(&rot_group);

        // ── 6. Generated config preview ───────────────────────────────────
        self.root.append(&hsep(12));

        let preview_label = Label::builder()
            .label("Generated config preview")
            .halign(gtk4::Align::Start)
            .build();
        preview_label.add_css_class("heading");
        self.root.append(&preview_label);

        let preview_hint = Label::builder()
            .label("Read-only. Click Apply to write this to disk \
                   (or copy into the editor above).")
            .halign(gtk4::Align::Start)
            .wrap(true)
            .build();
        preview_hint.add_css_class("dim-label");
        self.root.append(&preview_hint);

        let preview_scroll = ScrolledWindow::builder()
            .height_request(140)
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .build();
        let preview_text = TextView::builder()
            .editable(false)
            .monospace(true)
            .left_margin(8).top_margin(8)
            .build();
        preview_text.add_css_class("rot-editor");
        preview_scroll.set_child(Some(&preview_text));
        self.root.append(&preview_scroll);

        update_preview(&preview_text, &result.rotation_backend, &policy.borrow());

        // ── wire all controls → policy mutation + preview refresh ─────────
        let backend = result.rotation_backend.clone();

        // Frequency
        {
            let p = Rc::clone(&policy); let pv = preview_text.clone(); let b = backend.clone();
            freq_combo.connect_notify_local(Some("selected"), move |dd, _| {
                p.borrow_mut().frequency = match dd.selected() {
                    0 => crate::rotation::RotationFrequency::Daily,
                    1 => crate::rotation::RotationFrequency::Weekly,
                    2 => crate::rotation::RotationFrequency::Monthly,
                    3 => crate::rotation::RotationFrequency::Hourly,
                    _ => crate::rotation::RotationFrequency::OnSize { bytes: 100 * 1024 * 1024 },
                };
                update_preview(&pv, &b, &p.borrow());
            });
        }

        // Keep count
        {
            let p = Rc::clone(&policy); let pv = preview_text.clone(); let b = backend.clone();
            keep_entry.connect_changed(move |entry| {
                if let Ok(n) = entry.text().parse::<u32>() {
                    p.borrow_mut().keep_count = n.max(1);
                    update_preview(&pv, &b, &p.borrow());
                }
            });
        }

        // Compression format
        {
            let p = Rc::clone(&policy); let pv = preview_text.clone(); let b = backend.clone();
            compress_combo.connect_notify_local(Some("selected"), move |dd, _| {
                p.borrow_mut().compress = match dd.selected() {
                    0 => crate::rotation::CompressFormat::detect_best(),
                    1 => crate::rotation::CompressFormat::Pigz,
                    2 => crate::rotation::CompressFormat::Zst,
                    3 => crate::rotation::CompressFormat::Gz,
                    4 => crate::rotation::CompressFormat::Xz,
                    _ => crate::rotation::CompressFormat::None,
                };
                update_preview(&pv, &b, &p.borrow());
            });
        }

        // Delay compress
        {
            let p = Rc::clone(&policy); let pv = preview_text.clone(); let b = backend.clone();
            delay_row.connect_active_notify(move |sw| {
                p.borrow_mut().delay_compress = sw.is_active();
                update_preview(&pv, &b, &p.borrow());
            });
        }

        // Min log level → PipelineConfig
        {
            let p = Rc::clone(&policy); let pv = preview_text.clone(); let b = backend.clone();
            level_combo.connect_notify_local(Some("selected"), move |dd, _| {
                use crate::pipeline::{LogLevel, PipelineConfig};
                let min_level = match dd.selected() {
                    0 => None,
                    1 => Some(LogLevel::Debug),
                    2 => Some(LogLevel::Info),
                    3 => Some(LogLevel::Warn),
                    _ => Some(LogLevel::Error),
                };
                // Extract log_path before borrow_mut to avoid simultaneous borrows
                let log_path = p.borrow().log_path.clone();
                let mut pol = p.borrow_mut();
                if min_level.is_none() {
                    pol.pipeline = None;
                } else {
                    let cfg = pol.pipeline.get_or_insert_with(|| PipelineConfig {
                        log_path: log_path.clone(),
                        ..Default::default()
                    });
                    cfg.min_level = min_level;
                }
                drop(pol);
                update_preview(&pv, &b, &p.borrow());
            });
        }

        // Disposition
        {
            let p = Rc::clone(&policy); let pv = preview_text.clone(); let b = backend.clone();
            disp_combo.connect_notify_local(Some("selected"), move |dd, _| {
                use crate::pipeline::{OutputDisposition, PipelineConfig};
                let log_path = p.borrow().log_path.clone();
                let disposition = match dd.selected() {
                    0 => OutputDisposition::InPlace,
                    1 => OutputDisposition::Archive {
                        path: log_path.with_extension("log.archive"),
                    },
                    2 => OutputDisposition::SummaryAndArchive {
                        archive: log_path.with_extension("log.archive"),
                        summary: log_path.with_extension("log.summary"),
                    },
                    _ => OutputDisposition::Delete,
                };
                let mut pol = p.borrow_mut();
                let cfg = pol.pipeline.get_or_insert_with(|| PipelineConfig {
                    log_path: log_path.clone(),
                    ..Default::default()
                });
                cfg.disposition = disposition;
                drop(pol);
                update_preview(&pv, &b, &p.borrow());
            });
        }

        // Collapse repeats
        {
            let p = Rc::clone(&policy); let pv = preview_text.clone(); let b = backend.clone();
            collapse_row.connect_active_notify(move |sw| {
                use crate::pipeline::PipelineConfig;
                let log_path = p.borrow().log_path.clone();
                let mut pol = p.borrow_mut();
                let cfg = pol.pipeline.get_or_insert_with(|| PipelineConfig {
                    log_path: log_path.clone(),
                    ..Default::default()
                });
                cfg.collapse_repeats = sw.is_active();
                drop(pol);
                update_preview(&pv, &b, &p.borrow());
            });
        }

        // ── 7. Apply + Enable timer ───────────────────────────────────────
        self.root.append(&hsep(12));

        // Status row showing where units will land
        let unit_hint = {
            let unit_dir = dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("~"))
                .join(".config/systemd/user");
            let safe = policy.borrow().log_path.to_string_lossy()
                .replace(['/', ' '], "-").trim_matches('-').to_owned();
            Label::builder()
                .label(&format!(
                    "Timer: {}/logscope-{safe}.timer\n\
                     Service: {}/logscope-{safe}.service",
                    unit_dir.display(), unit_dir.display()
                ))
                .halign(gtk4::Align::Start)
                .wrap(true)
                .build()
        };
        unit_hint.add_css_class("dim-label");
        self.root.append(&unit_hint);

        let is_user_tier = matches!(policy.borrow().tier, crate::probe::Tier::User);
        let is_native    = matches!(policy.borrow().backend, RotationBackend::Native);

        let button_row = GtkBox::new(Orientation::Horizontal, 8);
        button_row.set_halign(gtk4::Align::End);
        button_row.set_margin_top(8);
        button_row.set_margin_bottom(16);

        // Copy to editor
        let copy_btn = Button::with_label("Copy to editor");
        copy_btn.set_tooltip_text(Some("Copy generated config into the editor above"));
        let preview_buf = preview_text.buffer();
        let editor_buf2 = editor_buf.clone();
        copy_btn.connect_clicked(move |_| {
            let text = preview_buf.text(&preview_buf.start_iter(), &preview_buf.end_iter(), false);
            editor_buf2.set_text(&text);
        });

        // Apply
        let apply_btn = Button::with_label("Apply config");
        apply_btn.add_css_class("suggested-action");
        if matches!(result.rotation_backend, RotationBackend::LogrotatePrivileged { .. }) {
            apply_btn.set_label("Apply (needs auth)");
            apply_btn.set_tooltip_text(Some("Will use pkexec to write to /etc/logrotate.d/"));
        }

        let policy_apply = Rc::clone(&policy);
        let enable_btn_ref: Rc<RefCell<Option<Button>>> = Rc::new(RefCell::new(None));

        apply_btn.connect_clicked({
            let enable_btn_ref = Rc::clone(&enable_btn_ref);
            move |btn| {
                let pol = policy_apply.borrow();
                match apply_config(&pol) {
                    Ok(_)  => {
                        btn.set_label("Applied ✓");
                        btn.set_sensitive(false);
                        // Reveal the enable-timer button now that units exist on disk
                        if let Some(eb) = enable_btn_ref.borrow().as_ref() {
                            eb.set_visible(true);
                        }
                    }
                    Err(e) => { btn.set_label(&format!("Error: {e}")); }
                }
            }
        });

        // Enable timer — only meaningful for user-tier native rotation
        // Hidden until Apply succeeds (units must exist first)
        let enable_btn = Button::with_label("Enable timer");
        enable_btn.set_tooltip_text(Some(
            "Run: systemctl --user enable --now logscope-<name>.timer\n\
             After this the rotation runs on schedule with no UI needed."
        ));
        enable_btn.set_visible(false);

        if !is_user_tier || !is_native {
            // For logrotate backends the system's own cron/timer handles scheduling.
            // For system-tier, the unit goes in /etc/systemd/system (not --user).
            // Show a hint instead.
            let sched_note = if is_native && !is_user_tier {
                "systemd system timer (not --user) — enable via pkexec"
            } else {
                "logrotate backend — scheduling via /etc/cron.d or systemd-logrotate"
            };
            enable_btn.set_label(sched_note);
            enable_btn.set_sensitive(false);
            enable_btn.set_visible(true);
        } else {
            let policy_enable = Rc::clone(&policy);
            enable_btn.connect_clicked(move |btn| {
                let pol = policy_enable.borrow();
                let safe = pol.log_path.to_string_lossy()
                    .replace(['/', ' '], "-").trim_matches('-').to_owned();
                let timer = format!("logscope-{safe}.timer");

                let status = std::process::Command::new("systemctl")
                    .args(["--user", "enable", "--now", &timer])
                    .status();

                match status {
                    Ok(s) if s.success() => {
                        btn.set_label("Timer enabled ✓");
                        btn.set_sensitive(false);
                    }
                    Ok(s) => {
                        btn.set_label(&format!("systemctl exited {}", s.code().unwrap_or(-1)));
                    }
                    Err(e) => {
                        btn.set_label(&format!("Error: {e}"));
                    }
                }
            });
        }

        *enable_btn_ref.borrow_mut() = Some(enable_btn.clone());

        button_row.append(&copy_btn);
        button_row.append(&apply_btn);
        button_row.append(&enable_btn);
        self.root.append(&button_row);

        // Status: check if timer is already active
        if is_user_tier && is_native {
            let pol = policy.borrow();
            let safe = pol.log_path.to_string_lossy()
                .replace(['/', ' '], "-").trim_matches('-').to_owned();
            let timer = format!("logscope-{safe}.timer");
            let active = std::process::Command::new("systemctl")
                .args(["--user", "is-active", "--quiet", &timer])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            if active {
                let status_lbl = Label::builder()
                    .label("● Timer is active and running")
                    .halign(gtk4::Align::End)
                    .build();
                status_lbl.add_css_class("success-label");
                self.root.append(&status_lbl);
            }
        }
    }
}

// ── existing config discovery ─────────────────────────────────────────────────

/// Search common logrotate config locations for any stanza that mentions `path`.
fn find_existing_configs(path: &Path) -> Vec<PathBuf> {
    let path_str = path.to_string_lossy();
    let mut found = Vec::new();

    let search_dirs: Vec<PathBuf> = {
        let mut dirs = vec![
            PathBuf::from("/etc/logrotate.d"),
            PathBuf::from("/etc/logrotate.conf"),
        ];
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".config/logrotate"));
            dirs.push(home.join(".config/logscope/rotate"));
        }
        dirs
    };

    for dir in &search_dirs {
        if dir.is_file() {
            if file_mentions(dir, &path_str) {
                found.push(dir.clone());
            }
        } else if dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() && file_mentions(&p, &path_str) {
                        found.push(p);
                    }
                }
            }
        }
    }
    found
}

fn file_mentions(file: &Path, needle: &str) -> bool {
    std::fs::read_to_string(file)
        .map(|s| s.contains(needle))
        .unwrap_or(false)
}

// ── raw config write (for editor Save button) ─────────────────────────────────

fn write_raw_config(
    text: &str,
    target: &Path,
    backend: &RotationBackend,
) -> Result<(), String> {
    match backend {
        RotationBackend::LogrotatePrivileged { .. } => {
            // Need privilege — write to temp then pkexec/sudo cp
            let tmp = std::env::temp_dir().join("logscope_raw_edit.conf.tmp");
            std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
            let ok = std::process::Command::new("pkexec")
                .args(["install", "-m", "644",
                    tmp.to_str().unwrap(), target.to_str().unwrap()])
                .status().map(|s| s.success()).unwrap_or(false);
            if ok { return Ok(()); }
            let ok2 = std::process::Command::new("sudo")
                .args(["cp", tmp.to_str().unwrap(), target.to_str().unwrap()])
                .status().map(|s| s.success()).unwrap_or(false);
            if ok2 { Ok(()) } else { Err("pkexec and sudo both failed".into()) }
        }
        _ => {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(target, text).map_err(|e| e.to_string())
        }
    }
}

// ── token rule row ────────────────────────────────────────────────────────────

pub fn build_token_rule_row(token: &str) -> gtk4::ListBoxRow {
    let row_box = GtkBox::new(Orientation::Horizontal, 8);
    row_box.set_margin_start(8); row_box.set_margin_end(8);
    row_box.set_margin_top(6);   row_box.set_margin_bottom(6);

    let lbl = Label::builder()
        .label(&format!("drop: {token}"))
        .halign(gtk4::Align::Start)
        .hexpand(true)
        .selectable(true)
        .build();
    lbl.add_css_class("file-info");

    let del_btn = Button::from_icon_name("edit-delete-symbolic");
    del_btn.add_css_class("flat");
    del_btn.set_tooltip_text(Some("Remove this rule"));
    del_btn.set_valign(gtk4::Align::Center);

    row_box.append(&lbl);
    row_box.append(&del_btn);

    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(&row_box));

    {
        let row_ref = row.clone();
        del_btn.connect_clicked(move |_| {
            if let Some(lb) = row_ref.parent().and_then(|p| p.downcast::<ListBox>().ok()) {
                lb.remove(&row_ref);
                if lb.observe_children().n_items() == 0 {
                    let empty = adw::ActionRow::new();
                    empty.set_title("No token rules yet");
                    empty.set_subtitle("Click the funnel icon on a token in Token Frequencies");
                    lb.append(&empty);
                }
            }
        });
    }
    row
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn action_row(title: &str, subtitle: &str) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_title(title);
    row.set_subtitle(subtitle);
    row
}

fn hsep(margin: i32) -> Separator {
    let s = Separator::new(Orientation::Horizontal);
    s.set_margin_top(margin);
    s.set_margin_bottom(margin);
    s
}

fn tier_label(tier: &Tier) -> String {
    match tier {
        Tier::User  => "User (no privilege needed)".into(),
        Tier::System => "System (root-owned)".into(),
        Tier::SharedGroup { group_name } => format!("Shared group: {group_name}"),
        Tier::Unknown => "Unknown".into(),
    }
}

fn backend_label(backend: &RotationBackend) -> String {
    match backend {
        RotationBackend::Native => "Native Rust rotation".into(),
        RotationBackend::Logrotate { conf_path } =>
            format!("logrotate → {}", conf_path.display()),
        RotationBackend::LogrotatePrivileged { conf_path } =>
            format!("logrotate (privileged) → {}", conf_path.display()),
    }
}

fn update_preview(view: &TextView, backend: &RotationBackend, policy: &RotationPolicy) {
    let text = match backend {
        RotationBackend::Native => generate_toml_preview(policy),
        _                       => generate_logrotate_preview(policy),
    };
    view.buffer().set_text(&text);
}

fn generate_logrotate_preview(policy: &RotationPolicy) -> String {
    let mut out = String::new();
    out.push_str("# logscope — generated logrotate config\n");
    out.push_str(&format!("'{}' {{\n", policy.log_path.display()));
    out.push_str(&format!("    {}\n", policy.frequency.logrotate_directive()));
    out.push_str(&format!("    rotate {}\n", policy.keep_count));
    out.push_str("    compress\n    delaycompress\n    missingok\n    notifempty\n");
    if policy.pipeline.is_some() {
        out.push_str("\n    prerotate\n");
        out.push_str("        logscope-filter --filter --config '<pipeline.toml>' --input $1\n");
        out.push_str("    endscript\n");
    }
    out.push_str("}\n");
    out
}

fn generate_toml_preview(policy: &RotationPolicy) -> String {
    toml::to_string_pretty(policy).unwrap_or_else(|e| format!("# error: {e}"))
}

fn apply_config(policy: &RotationPolicy) -> Result<String, String> {
    match &policy.backend {
        RotationBackend::Native => {
            let config_dir = dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("~/.config"))
                .join("logscope/rotate");
            let name = policy.log_path.to_string_lossy()
                .replace(['/', ' '], "-").trim_matches('-').to_owned();
            let conf_path = config_dir.join(format!("{name}.toml"));
            save_policy(policy, &conf_path).map_err(|e: std::io::Error| e.to_string())?;
            let unit_dir = dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("~"))
                .join(".config/systemd/user");
            write_systemd_user_units(policy, &unit_dir)
                .map_err(|e: std::io::Error| e.to_string())?;
            Ok(format!("Config written to {}", conf_path.display()))
        }
        RotationBackend::Logrotate { conf_path } => {
            write_logrotate_conf(policy, conf_path)
                .map_err(|e: std::io::Error| e.to_string())?;
            Ok(format!("logrotate conf written to {}", conf_path.display()))
        }
        RotationBackend::LogrotatePrivileged { conf_path } => {
            let conf_str = generate_logrotate_preview(policy);
            let tmp = std::env::temp_dir().join("logscope_rotate.conf.tmp");
            std::fs::write(&tmp, &conf_str).map_err(|e: std::io::Error| e.to_string())?;
            let ok = std::process::Command::new("pkexec")
                .args(["install", "-m", "644",
                    tmp.to_str().unwrap(), conf_path.to_str().unwrap()])
                .status().map(|s| s.success()).unwrap_or(false);
            if ok { return Ok(format!("Config written to {}", conf_path.display())); }
            let ok2 = std::process::Command::new("sudo")
                .args(["cp", tmp.to_str().unwrap(), conf_path.to_str().unwrap()])
                .status().map(|s| s.success()).unwrap_or(false);
            if ok2 { Ok(format!("Config written via sudo to {}", conf_path.display())) }
            else   { Err("pkexec and sudo both failed".into()) }
        }
    }
}
