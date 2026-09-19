use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use stoat_core::diag::{Diagnostic, Severity};
use stoat_core::model::{Frame, RunState, Variable};

struct FramePage {
    root: gtk::ScrolledWindow,
    source: gtk::Label,
    locals: gtk::ListBox,
    filled: Cell<bool>,
    rows: RefCell<HashMap<String, adw::ActionRow>>,
}

pub struct StackPage {
    pub root: gtk::Box,
    pub carousel: adw::Carousel,
    pub threads: gtk::DropDown,
    pub toward_callee: gtk::Button,
    pub toward_caller: gtk::Button,
    pub go: gtk::Button,
    pub step_over: gtk::Button,
    pub step_into: gtk::Button,
    pub step_out: gtk::Button,
    /// True only while this page rebuilds its own widgets (pages removed, thread
    /// model replaced), which makes GTK emit change signals nobody asked for.
    /// Echoes of scroll_to are handled statelessly by the listener instead: a
    /// page-changed to the already-selected frame is not a user action.
    pub programmatic: Cell<bool>,
    /// A deferred first scroll is outstanding; see `select`.
    pub pending: Cell<Option<usize>>,
    fresh: Cell<bool>,
    switcher: gtk::Stack,
    diag_box: gtk::Box,
    diag_title: gtk::Label,
    diag_notes: gtk::Label,
    position: gtk::Label,
    pages: RefCell<Vec<FramePage>>,
    frames: RefCell<Vec<Frame>>,
}

/// Icon names here are all from the Adwaita core set. There is no standard
/// step-over/into/out icon; these are stand-ins, and the real fix is shipping
/// three symbolic SVGs in a gresource rather than hoping a theme has them.
fn icon_button(icon: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::builder().icon_name(icon).tooltip_text(tip).build();
    b.add_css_class("flat");
    b
}

impl StackPage {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let diag_title = gtk::Label::builder().xalign(0.0).wrap(true).build();
        diag_title.add_css_class("heading");
        let diag_notes = gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).build();
        diag_notes.add_css_class("caption");
        let diag_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).margin_top(8).margin_bottom(4).margin_start(12).margin_end(12).visible(false).build();
        diag_box.add_css_class("card");
        diag_box.add_css_class("stoat-diag");
        diag_box.append(&diag_title);
        diag_box.append(&diag_notes);
        root.append(&diag_box);

        let carousel = adw::Carousel::builder().vexpand(true).spacing(12).allow_long_swipes(true).build();
        let lines = adw::CarouselIndicatorLines::builder().carousel(&carousel).margin_top(2).margin_bottom(2).build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&carousel);
        content.append(&lines);

        let empty = adw::StatusPage::builder().icon_name("media-playback-pause-symbolic").title("Not stopped").description("Frames appear here when the program stops. Swipe sideways, or use the arrows, to walk the stack.").vexpand(true).build();
        let switcher = gtk::Stack::new();
        switcher.add_named(&empty, Some("empty"));
        switcher.add_named(&content, Some("frames"));
        root.append(&switcher);

        let bar = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(2).margin_start(6).margin_end(6).margin_top(4).margin_bottom(4).build();
        let toward_callee = icon_button("go-previous-symbolic", "Toward the callee (gdb: down)");
        let toward_caller = icon_button("go-next-symbolic", "Toward the caller (gdb: up)");
        let position = gtk::Label::builder().hexpand(true).ellipsize(gtk::pango::EllipsizeMode::Middle).build();
        position.add_css_class("caption");
        let go = icon_button("media-playback-start-symbolic", "Continue");
        let step_over = icon_button("media-skip-forward-symbolic", "Step over");
        let step_into = icon_button("go-bottom-symbolic", "Step into");
        let step_out = icon_button("go-top-symbolic", "Step out");
        let threads = gtk::DropDown::from_strings(&[]);
        threads.set_visible(false);
        for w in [toward_callee.upcast_ref::<gtk::Widget>(), position.upcast_ref(), toward_caller.upcast_ref(), threads.upcast_ref(), go.upcast_ref(), step_over.upcast_ref(), step_into.upcast_ref(), step_out.upcast_ref()] {
            bar.append(w);
        }
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&bar);

        StackPage {
            root,
            carousel,
            threads,
            toward_callee,
            toward_caller,
            go,
            step_over,
            step_into,
            step_out,
            programmatic: Cell::new(false),
            pending: Cell::new(None),
            fresh: Cell::new(false),
            switcher,
            diag_box,
            diag_title,
            diag_notes,
            position,
            pages: RefCell::default(),
            frames: RefCell::default(),
        }
    }

    pub fn set_state(&self, state: RunState) {
        let stopped = state == RunState::Stopped;
        for b in [&self.step_over, &self.step_into, &self.step_out, &self.toward_callee, &self.toward_caller] {
            b.set_sensitive(stopped);
        }
        self.go.set_sensitive(matches!(state, RunState::Stopped | RunState::Running));
        let (icon, tip) = if state == RunState::Running { ("media-playback-pause-symbolic", "Interrupt") } else { ("media-playback-start-symbolic", "Continue") };
        self.go.set_icon_name(icon);
        self.go.set_tooltip_text(Some(tip));
        if state == RunState::Running {
            self.switcher.set_visible_child_name("empty");
            self.diag_box.set_visible(false);
        }
    }

    pub fn set_diagnostic(&self, d: &Diagnostic, extra_notes: &[String]) {
        let sev = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        };
        self.diag_title.set_text(&format!("{sev}[{}]: {}", d.code, d.title));
        for c in ["error", "warning"] {
            self.diag_title.remove_css_class(c);
        }
        if d.severity != Severity::Info {
            self.diag_title.add_css_class(sev);
        }
        let mut lines: Vec<String> = d.notes.iter().map(|n| format!("note: {n}")).collect();
        lines.extend(extra_notes.iter().map(|n| format!("note: {n}")));
        lines.extend(d.help.iter().map(|h| format!("help: {h}")));
        self.diag_notes.set_text(&lines.join("\n"));
        self.diag_notes.set_visible(!lines.is_empty());
        self.diag_box.set_visible(true);
    }

    pub fn set_frames(&self, frames: &[Frame]) {
        self.programmatic.set(true);
        for p in self.pages.borrow_mut().drain(..) {
            self.carousel.remove(&p.root);
        }
        for f in frames {
            let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).margin_top(8).margin_bottom(8).margin_start(12).margin_end(12).build();
            let func = gtk::Label::builder().label(f.func.as_deref().unwrap_or("??")).xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).build();
            func.add_css_class("title-4");
            let loc = gtk::Label::builder().label(format!("#{} · {}", f.level, f.location())).xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).selectable(true).build();
            loc.add_css_class("caption");
            loc.add_css_class("dim-label");
            // Wrapped rather than horizontally scrollable: a sideways-scrolling
            // child would fight the carousel for the same swipe.
            let source = gtk::Label::builder().xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).use_markup(true).selectable(false).build();
            source.add_css_class("monospace");
            source.add_css_class("stoat-source");
            let locals = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
            locals.add_css_class("boxed-list");
            body.append(&func);
            body.append(&loc);
            body.append(&source);
            body.append(&locals);
            let root = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&body).hexpand(true).build();
            self.carousel.append(&root);
            self.pages.borrow_mut().push(FramePage { root, source, locals, filled: Cell::new(false), rows: RefCell::default() });
        }
        *self.frames.borrow_mut() = frames.to_vec();
        self.fresh.set(true);
        self.pending.set(None);
        self.switcher.set_visible_child_name(if frames.is_empty() { "empty" } else { "frames" });
        self.programmatic.set(false);
    }

    fn fill(&self, idx: usize) {
        let pages = self.pages.borrow();
        let (Some(p), Some(f)) = (pages.get(idx), self.frames.borrow().get(idx).cloned()) else { return };
        if p.filled.replace(true) {
            return;
        }
        match source_markup(&f, 6) {
            Some(m) => p.source.set_markup(&m),
            None => {
                p.source.set_text(if f.line.is_some() { "source file is not on this machine" } else { "no line info for this frame" });
                p.source.add_css_class("dim-label");
            }
        }
    }

    /// Bring frame `idx` into view. Source is read lazily for it and its two
    /// neighbours, so a 250-frame recursion does not read 250 files up front.
    fn show_label_and_fill(&self, idx: usize) {
        for i in idx.saturating_sub(1)..=idx + 1 {
            self.fill(i);
        }
        let n = self.pages.borrow().len();
        if let Some(f) = self.frames.borrow().get(idx) {
            self.position.set_text(&format!("frame {} of {n} · {}", idx + 1, f.func.as_deref().unwrap_or("??")));
        }
    }

    pub fn show(&self, idx: usize, animate: bool) {
        self.show_label_and_fill(idx);
        let Some(target) = self.pages.borrow().get(idx).map(|p| p.root.clone()) else { return };
        if self.carousel.position().round() as usize == idx {
            return;
        }
        self.carousel.scroll_to(&target, animate);
    }

    /// Selection changed in the session. The first selection after a rebuild is
    /// special: the pages were appended in this main-loop iteration and have no
    /// allocation, and AdwCarousel::scroll_to is a silent no-op until they do.
    /// (An idle callback is not enough; it can run before the first layout, and
    /// the carousel then reports page 0 as if the user had swiped there.) So wait
    /// on the frame clock until the target page has a width, and have the
    /// page-changed listener ignore everything while `pending` is set.
    pub fn select(self: &std::rc::Rc<Self>, idx: usize) {
        if !self.fresh.replace(false) || idx == 0 {
            return self.show(idx, true);
        }
        self.show_label_and_fill(idx);
        self.pending.set(Some(idx));
        let me = std::rc::Rc::downgrade(self);
        self.carousel.add_tick_callback(move |carousel, _| {
            let Some(me) = me.upgrade() else { return glib::ControlFlow::Break };
            let target = me.pending.get().and_then(|i| me.pages.borrow().get(i).map(|p| p.root.clone()));
            match target {
                Some(t) if t.width() > 0 => {
                    carousel.scroll_to(&t, false);
                    me.pending.set(None);
                    glib::ControlFlow::Break
                }
                Some(_) => glib::ControlFlow::Continue,
                None => {
                    me.pending.set(None);
                    glib::ControlFlow::Break
                }
            }
        });
    }

    pub fn set_locals(&self, idx: usize, vars: &[Variable], on_expand: impl Fn(String) + Clone + 'static) {
        let pages = self.pages.borrow();
        let Some(p) = pages.get(idx) else { return };
        p.locals.remove_all();
        p.rows.borrow_mut().clear();
        for v in vars {
            let title = if v.is_arg { format!("{}  (arg)", v.name) } else { v.name.clone() };
            let row = adw::ActionRow::builder().title(glib::markup_escape_text(&title)).subtitle_lines(6).build();
            row.add_css_class("property");
            match &v.value {
                Some(val) => row.set_subtitle(&glib::markup_escape_text(val)),
                None => {
                    row.set_subtitle(&glib::markup_escape_text(&format!("{} — tap to evaluate", v.ty.as_deref().unwrap_or("aggregate"))));
                    row.set_activatable(true);
                    let (name, f) = (v.name.clone(), on_expand.clone());
                    row.connect_activated(move |_| f(name.clone()));
                }
            }
            p.locals.append(&row);
            p.rows.borrow_mut().insert(v.name.clone(), row);
        }
    }

    pub fn set_value(&self, idx: usize, expr: &str, value: &str) -> bool {
        let pages = self.pages.borrow();
        let Some(row) = pages.get(idx).and_then(|p| p.rows.borrow().get(expr).cloned()) else { return false };
        row.set_subtitle(&glib::markup_escape_text(value));
        true
    }

    pub fn set_threads(&self, labels: &[String], selected: u32) {
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.programmatic.set(true);
        self.threads.set_model(Some(&gtk::StringList::new(&refs)));
        self.threads.set_selected(selected);
        self.threads.set_visible(labels.len() > 1);
        self.programmatic.set(false);
    }
}

fn source_markup(f: &Frame, context: u32) -> Option<String> {
    let (file, line) = (f.file.as_ref()?, f.line?);
    let text = std::fs::read_to_string(file).ok()?;
    let lo = line.saturating_sub(context).max(1);
    let hi = line + context;
    let mut out = String::new();
    for (n, l) in text.lines().enumerate().map(|(i, l)| (i as u32 + 1, l)).filter(|(n, _)| (lo..=hi).contains(n)) {
        let code = glib::markup_escape_text(&l.replace('\t', "    "));
        if n == line {
            out += &format!("<b><span alpha=\"60%\">{n:>4}</span> ▸ {code}</b>\n");
        } else {
            out += &format!("<span alpha=\"60%\">{n:>4}</span>   {code}\n");
        }
    }
    Some(out.trim_end().to_string())
}
