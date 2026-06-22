//! sources_tab.rs — live log source picker
//!
//! Streams from system sources (dmesg, journalctl, custom commands) directly
//! into the Peek view buffer, line by line, without writing a temp file.
//!
//! Layout:
//!   ┌─ Preset sources ──────────────────────────────┐
//!   │  [dmesg]  [journal]  [syslog]  [kernel]        │
//!   ├─ Custom command ──────────────────────────────┐│
//!   │  $ [_________________________] [▶ Run]  [■]   ││
//!   ├─ Filters ─────────────────────────────────────┐│
//!   │  grep: [_______________]  [✓ regex]  [✗ invert]││
//!   ├─ Output preview (last N lines) ───────────────┐│
//!   │  (scrolling live tail shown here)             ││
//!   └───────────────────────────────────────────────┘│

use gtk4::prelude::*;
use gtk4::glib;
use gtk4::{
    Box as GtkBox, Button, CheckButton, Entry, Label,
    Orientation, PolicyType, ScrolledWindow, TextView, TextBuffer,
    WrapMode,
};
use libadwaita as adw;
use adw::prelude::*;

use std::cell::RefCell;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossbeam_channel::unbounded;

// ── public struct ─────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub struct SourcesTab {
    pub root: GtkBox,
    /// Lines collected from the current source — caller reads these into PeekView
    pub lines: Rc<RefCell<Vec<String>>>,
    /// Set to true to stop the current stream
    pub cancel: Arc<AtomicBool>,
    /// Callback invoked when lines are ready to load into PeekView
    on_lines: Rc<RefCell<Option<Box<dyn Fn(Vec<String>)>>>>,
}

impl SourcesTab {
    pub fn new() -> Self {
        let root        = GtkBox::new(Orientation::Vertical, 0);
        let lines       = Rc::new(RefCell::new(Vec::<String>::new()));
        let cancel      = Arc::new(AtomicBool::new(false));
        let on_lines: Rc<RefCell<Option<Box<dyn Fn(Vec<String>)>>>> =
            Rc::new(RefCell::new(None));

        // ── Preset sources ────────────────────────────────────────────────
        let preset_group = adw::PreferencesGroup::new();
        preset_group.set_title("System sources");
        preset_group.set_description(Some("Tap to stream into Peek view"));
        preset_group.set_margin_start(12);
        preset_group.set_margin_end(12);
        preset_group.set_margin_top(12);

        let presets: &[(&str, &str, &str)] = &[
            ("dmesg",        "computer-symbolic",         "dmesg --color=never -T"),
            ("journalctl",   "text-x-log-symbolic",       "journalctl -n 2000 --no-pager -o short-iso"),
            ("journal -f",   "media-record-symbolic",     "journalctl -f --no-pager -o short-iso"),
            ("kernel",       "emblem-system-symbolic",    "journalctl -k -n 1000 --no-pager -o short-iso"),
            ("errors",       "dialog-error-symbolic",     "journalctl -p err -n 500 --no-pager -o short-iso"),
            ("syslog",       "document-open-symbolic",    "tail -n 2000 /var/log/syslog 2>/dev/null || tail -n 2000 /var/log/messages 2>/dev/null || echo 'syslog not found'"),
            ("boot",         "system-restart-symbolic",   "journalctl -b --no-pager -o short-iso"),
            ("auth",         "system-lock-screen-symbolic","journalctl -u ssh -u sshd -n 500 --no-pager -o short-iso"),
        ];

        let btn_grid = GtkBox::new(Orientation::Horizontal, 6);
        btn_grid.set_margin_start(12); btn_grid.set_margin_end(12);
        btn_grid.set_margin_top(8);    btn_grid.set_margin_bottom(4);
        btn_grid.set_homogeneous(false);

        // Wrap in a ScrolledWindow so presets don't break layout on narrow
        let preset_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Never)
            .child(&btn_grid)
            .build();
        preset_scroll.set_margin_start(12);
        preset_scroll.set_margin_end(12);
        preset_scroll.set_margin_top(8);

        root.append(&preset_scroll);
        root.append(&gtk4::Separator::new(Orientation::Horizontal));

        for (label, icon, cmd) in presets {
            let btn = Button::new();
            btn.add_css_class("sources-preset-btn");

            let btn_inner = GtkBox::new(Orientation::Horizontal, 4);
            let ico = gtk4::Image::from_icon_name(icon);
            ico.set_pixel_size(16);
            let lbl = Label::new(Some(label));
            lbl.add_css_class("sources-btn-label");
            btn_inner.append(&ico);
            btn_inner.append(&lbl);
            btn.set_child(Some(&btn_inner));
            btn.set_tooltip_text(Some(cmd));

            let cmd_str    = cmd.to_string();
            let cancel_ref = Arc::clone(&cancel);
            let lines_ref  = Rc::clone(&lines);
            let cb_ref     = Rc::clone(&on_lines);
            btn.connect_clicked(move |_| {
                cancel_ref.store(true, Ordering::Relaxed); // stop previous
                std::thread::sleep(std::time::Duration::from_millis(50));
                cancel_ref.store(false, Ordering::Relaxed);
                lines_ref.borrow_mut().clear();
                run_command(&cmd_str, "", false, false,
                    Arc::clone(&cancel_ref), Rc::clone(&lines_ref), Rc::clone(&cb_ref));
            });

            btn_grid.append(&btn);
        }

        // ── Custom command — stacked vertically so entry gets full width ──
        let cmd_group = GtkBox::new(Orientation::Vertical, 4);
        cmd_group.set_margin_start(12); cmd_group.set_margin_end(12);
        cmd_group.set_margin_top(10);   cmd_group.set_margin_bottom(4);

        let cmd_lbl = Label::builder()
            .label("Custom command").halign(gtk4::Align::Start).build();
        cmd_lbl.add_css_class("heading");
        cmd_group.append(&cmd_lbl);

        let cmd_hint = Label::builder()
            .label("Pipes, redirects, sudo all work  •  e.g. sudo dmesg | grep usb")
            .halign(gtk4::Align::Start).wrap(true).build();
        cmd_hint.add_css_class("dim-label");
        cmd_group.append(&cmd_hint);

        // Entry gets its own full-width row
        let cmd_entry = Entry::builder()
            .hexpand(true)
            .placeholder_text("journalctl -u nginx -f | grep ERROR")
            .build();
        cmd_entry.add_css_class("sources-cmd-entry");
        cmd_group.append(&cmd_entry);

        // Run/Stop on separate row so entry isn't squished
        let run_row = GtkBox::new(Orientation::Horizontal, 6);
        let run_btn  = Button::with_label("▶ Run");
        run_btn.add_css_class("suggested-action");
        run_btn.set_hexpand(true);
        let stop_btn = Button::with_label("■ Stop");
        stop_btn.add_css_class("flat");
        run_row.append(&run_btn);
        run_row.append(&stop_btn);
        cmd_group.append(&run_row);
        root.append(&cmd_group);

        // ── Filters — stacked vertically ─────────────────────────────────
        let filter_group = GtkBox::new(Orientation::Vertical, 4);
        filter_group.set_margin_start(12); filter_group.set_margin_end(12);
        filter_group.set_margin_top(4);    filter_group.set_margin_bottom(4);

        let filter_entry = Entry::builder()
            .hexpand(true)
            .placeholder_text("Filter pattern (blank = show all)")
            .build();

        let filter_opts = GtkBox::new(Orientation::Horizontal, 12);
        let filter_lbl  = Label::builder().label("Filter:").halign(gtk4::Align::Start).hexpand(true).build();
        filter_lbl.add_css_class("dim-label");
        let regex_chk  = CheckButton::with_label("regex");
        let invert_chk = CheckButton::with_label("invert");
        filter_opts.append(&filter_lbl);
        filter_opts.append(&regex_chk);
        filter_opts.append(&invert_chk);
        filter_group.append(&filter_opts);
        filter_group.append(&filter_entry);
        root.append(&filter_group);
        root.append(&gtk4::Separator::new(Orientation::Horizontal));

        // ── Live output preview ───────────────────────────────────────────
        let out_lbl = Label::builder()
            .label("Live output (last 500 lines)").halign(gtk4::Align::Start)
            .margin_start(12).margin_top(6).build();
        out_lbl.add_css_class("dim-label");
        root.append(&out_lbl);

        let out_buf = TextBuffer::new(None);
        let out_view = TextView::with_buffer(&out_buf);
        out_view.set_monospace(true);
        out_view.set_editable(false);
        out_view.set_wrap_mode(WrapMode::None);
        out_view.add_css_class("sources-output");

        let out_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .child(&out_view)
            .build();
        out_scroll.set_margin_start(12);
        out_scroll.set_margin_end(12);
        out_scroll.set_margin_bottom(8);
        root.append(&out_scroll);

        // Status bar
        let status_bar = gtk4::ActionBar::new();
        let status_lbl = Label::new(Some("No source running"));
        status_lbl.add_css_class("dim-label");
        status_lbl.set_hexpand(true);
        let clear_btn = Button::with_label("Clear");
        clear_btn.add_css_class("flat");
        status_bar.set_center_widget(Some(&status_lbl));
        status_bar.pack_end(&clear_btn);
        root.append(&status_bar);

        // Clear button
        {
            let buf = out_buf.clone();
            let lr  = Rc::clone(&lines);
            clear_btn.connect_clicked(move |_| {
                buf.set_text("");
                lr.borrow_mut().clear();
            });
        }

        // Stop button
        {
            let c = Arc::clone(&cancel);
            let sl = status_lbl.clone();
            stop_btn.connect_clicked(move |_| {
                c.store(true, Ordering::Relaxed);
                sl.set_text("Stopped");
            });
        }

        // Run button — shell-exec with filters
        {
            let cancel_ref  = Arc::clone(&cancel);
            let lines_ref   = Rc::clone(&lines);
            let cb_ref      = Rc::clone(&on_lines);
            let cmd_entry2  = cmd_entry.clone();
            let filter_e    = filter_entry.clone();
            let regex_c     = regex_chk.clone();
            let invert_c    = invert_chk.clone();
            let status_ref  = status_lbl.clone();
            let buf_ref     = out_buf.clone();
            let out_view2   = out_view.clone();

            run_btn.connect_clicked(move |_| {
                let cmd = cmd_entry2.text().to_string();
                if cmd.trim().is_empty() { return; }

                cancel_ref.store(true, Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(50));
                cancel_ref.store(false, Ordering::Relaxed);
                lines_ref.borrow_mut().clear();
                buf_ref.set_text("");

                let filter   = filter_e.text().to_string();
                let is_regex = regex_c.is_active();
                let invert   = invert_c.is_active();
                status_ref.set_text(&format!("Running: {cmd}"));

                // Clone cb_ref so the inner FnOnce closure captures a fresh Rc,
                // leaving cb_ref available for the next button click.
                let outer_cb = Rc::clone(&cb_ref);
                let buf_live = buf_ref.clone();
                let ov       = out_view2.clone();
                let lines_cb = Rc::clone(&lines_ref);

                let cb_live: Rc<RefCell<Option<Box<dyn Fn(Vec<String>)>>>> =
                    Rc::new(RefCell::new(Some(Box::new(move |_new_lines: Vec<String>| {
                        // Update preview buffer with accumulated lines
                        let all = lines_cb.borrow().join("");
                        buf_live.set_text(&all);

                        // Trim to last 500 lines if needed
                        let lc = buf_live.line_count();
                        if lc > 500 {
                            let trim_to = lc - 500;
                            if let Some(trim_iter) = buf_live.iter_at_line(trim_to) {
                                buf_live.delete(&mut buf_live.start_iter(), &mut trim_iter.clone());
                            }
                        }

                        // Scroll to bottom
                        ov.scroll_to_iter(&mut buf_live.end_iter(), 0.0, false, 0.0, 1.0);

                        // Forward to the peek-view callback
                        if let Some(cb) = outer_cb.borrow().as_ref() {
                            cb(lines_cb.borrow().clone());
                        }
                    }))));

                run_command(&cmd, &filter, is_regex, invert,
                    Arc::clone(&cancel_ref), Rc::clone(&lines_ref), cb_live);
            });
        }

        // ── AI analysis button ─────────────────────────────────────────────
        let ai_btn = gtk4::Button::with_label("✦ Analyse logs");
        ai_btn.add_css_class("flat");
        ai_btn.set_tooltip_text(Some("Send recent log lines to Ollama for AI analysis"));
        status_bar.pack_start(&ai_btn);

        // AI result revealer (slides in below status bar)
        let ai_revealer = gtk4::Revealer::new();
        ai_revealer.set_transition_type(gtk4::RevealerTransitionType::SlideDown);
        let ai_box = GtkBox::new(gtk4::Orientation::Vertical, 4);
        ai_box.set_margin_start(10); ai_box.set_margin_end(10);
        ai_box.set_margin_top(6);    ai_box.set_margin_bottom(6);
        ai_box.add_css_class("sources-ai-panel");
        let ai_hdr = GtkBox::new(gtk4::Orientation::Horizontal, 8);
        let ai_title = gtk4::Label::new(Some("AI Analysis"));
        ai_title.add_css_class("heading"); ai_title.set_hexpand(true);
        ai_title.set_halign(gtk4::Align::Start);
        let ai_dismiss = gtk4::Button::from_icon_name("window-close-symbolic");
        ai_dismiss.add_css_class("flat");
        ai_hdr.append(&ai_title); ai_hdr.append(&ai_dismiss);
        let ai_lbl = gtk4::Label::new(Some(""));
        ai_lbl.set_halign(gtk4::Align::Start);
        ai_lbl.set_wrap(true); ai_lbl.set_selectable(true);
        ai_lbl.add_css_class("sources-ai-result");
        ai_box.append(&ai_hdr); ai_box.append(&ai_lbl);
        ai_revealer.set_child(Some(&ai_box));
        root.append(&ai_revealer);

        { let rev = ai_revealer.clone();
          ai_dismiss.connect_clicked(move |_| { rev.set_reveal_child(false); }); }

        {
            let lines_ref = Rc::clone(&lines);
            let rl = ai_lbl.clone();
            let rev = ai_revealer.clone();
            let sl = status_lbl.clone();
            let ai_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let ai_lc = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            ai_btn.connect_clicked(move |_| {
                let all = lines_ref.borrow().clone();
                if all.is_empty() { sl.set_text("No log lines to analyse"); return; }
                ai_cancel.store(true, Ordering::Relaxed);
                ai_cancel.store(false, Ordering::Relaxed);
                let sample: String = all.iter().rev().take(200).rev()
                    .cloned().collect::<Vec<_>>().join("
");
                let prompt = format!(
                    "Analyse these system log lines. Respond with:\n                     1. **Summary**: what is happening (1-2 sentences)\n                     2. **Issues**: errors or warnings (bullet list)\n                     3. **Action**: recommended next step (1 sentence)\n                     \nLog:\n```\n{sample}\n```"
                );
                rl.set_text("Analysing…");
                rev.set_reveal_child(true);
                let (tx, rx) = crossbeam_channel::unbounded::<crate::ui::ollama::StreamChunk>();
                let gc2 = std::sync::Arc::clone(&ai_cancel);
                let lc2 = std::sync::Arc::clone(&ai_lc);
                std::thread::spawn(move || {
                    crate::ui::ollama::prompt_streaming(
                        &prompt, crate::ui::ollama::DEFAULT_HOST, "", &tx, &gc2, &lc2,
                    );
                });
                let rl2 = rl.clone();
                let acc = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
                glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
                    let mut batch = String::new(); let mut done = false;
                    loop {
                        match rx.try_recv() {
                            Ok(crate::ui::ollama::StreamChunk::Token(t)) => batch.push_str(&t),
                            Ok(crate::ui::ollama::StreamChunk::Done { .. }) |
                            Ok(crate::ui::ollama::StreamChunk::Cancelled) => { done = true; break; }
                            Ok(crate::ui::ollama::StreamChunk::Error(e)) => {
                                if rl2.parent().is_some() { rl2.set_text(&format!("⚠ {e}")); }
                                done = true; break;
                            }
                            Err(crossbeam_channel::TryRecvError::Empty) => break,
                            Err(_) => { done = true; break; }
                        }
                    }
                    if !batch.is_empty() {
                        let mut a = acc.borrow_mut(); a.push_str(&batch);
                        if rl2.parent().is_some() { rl2.set_text(&*a); }
                    }
                    if done {
                        let ft = acc.borrow().clone();
                        if crate::ui::markdown::has_markdown(&ft) && rl2.parent().is_some() {
                            rl2.set_text(&format!("{ft}\n\n⊞ click to view rendered"));
                            let ft2 = ft.clone(); let rl3 = rl2.clone();
                            let gc = gtk4::GestureClick::new();
                            gc.connect_pressed(move |_, _, _, _| {
                                let pop = gtk4::Popover::new();
                                let rendered = crate::ui::markdown::render(&ft2);
                                let scr = gtk4::ScrolledWindow::builder()
                                    .child(&rendered).min_content_width(400)
                                    .min_content_height(250).max_content_height(550).build();
                                pop.set_child(Some(&scr));
                                pop.set_parent(&rl3);
                                pop.popup();
                            });
                            rl2.add_controller(gc);
                        }
                        return glib::ControlFlow::Break;
                    }
                    glib::ControlFlow::Continue
                });
            });
        }

        SourcesTab { root, lines, cancel, on_lines }
    }
    /// The caller (window.rs) uses this to load into PeekView.
    pub fn set_on_lines<F: Fn(Vec<String>) + 'static>(&self, f: F) {
        *self.on_lines.borrow_mut() = Some(Box::new(f));
    }
}

// ── command runner ────────────────────────────────────────────────────────────

fn run_command(
    cmd:       &str,
    filter:    &str,
    is_regex:  bool,
    invert:    bool,
    cancel:    Arc<AtomicBool>,
    lines:     Rc<RefCell<Vec<String>>>,
    on_lines:  Rc<RefCell<Option<Box<dyn Fn(Vec<String>)>>>>,
) {
    let (tx, rx) = unbounded::<Vec<String>>();
    let cmd      = cmd.to_string();
    let filter   = filter.to_string();

    // Clone cancel so both the rayon thread and the idle closure can use it
    let cancel_idle = Arc::clone(&cancel);

    std::thread::spawn(move || {
        // Build the shell invocation — always via sh -c so pipes work
        let mut child = match Command::new("sh")
            .args(["-c", &cmd])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c)  => c,
            Err(e) => {
                let _ = tx.send(vec![format!("[logscope error] failed to spawn: {e}\n")]);
                return;
            }
        };

        let stdout = match child.stdout.take() {
            Some(s) => s,
            None    => return,
        };

        let reader     = BufReader::new(stdout);
        let filter_re  = if is_regex && !filter.is_empty() {
            regex::Regex::new(&filter).ok()
        } else { None };

        let mut batch = Vec::<String>::new();

        for line_res in reader.lines() {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                break;
            }

            let line = match line_res {
                Ok(l)  => l + "\n",
                Err(_) => break,
            };

            // Apply filter
            let passes = if filter.is_empty() {
                true
            } else if let Some(re) = &filter_re {
                let m = re.is_match(&line);
                if invert { !m } else { m }
            } else {
                let m = line.contains(&filter);
                if invert { !m } else { m }
            };

            if !passes { continue; }

            batch.push(line);

            // Send in batches of 50 so the UI doesn't redraw every line
            if batch.len() >= 50 {
                let _ = tx.send(std::mem::take(&mut batch));
            }
        }
        if !batch.is_empty() {
            let _ = tx.send(batch);
        }

        let _ = child.wait();
    });

    // Poll on idle — merge into lines vec, fire callback
    glib::idle_add_local(move || {
        if cancel_idle.load(Ordering::Relaxed) {
            return glib::ControlFlow::Break;
        }
        let mut any = false;
        loop {
            match rx.try_recv() {
                Ok(batch) => {
                    lines.borrow_mut().extend(batch);
                    any = true;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(_) => return glib::ControlFlow::Break,
            }
        }
        if any {
            if let Some(cb) = on_lines.borrow().as_ref() {
                cb(lines.borrow().clone());
            }
        }
        glib::ControlFlow::Continue
    });
}
