use adw::prelude::*;
use std::cell::RefCell;
use stoat_core::discover::Candidate;
use stoat_core::history::Invocation;

pub struct LaunchPage {
    pub root: gtk::ScrolledWindow,
    pub dir: adw::EntryRow,
    pub build_row: adw::ActionRow,
    pub build_button: gtk::Button,
    pub build_log: gtk::Label,
    pub binaries: adw::PreferencesGroup,
    pub args: adw::EntryRow,
    pub env: adw::EntryRow,
    pub marker: adw::EntryRow,
    pub history_query: adw::EntryRow,
    pub history_group: adw::PreferencesGroup,
    pub debug: gtk::Button,
    binary_rows: RefCell<Vec<adw::ActionRow>>,
    history_rows: RefCell<Vec<adw::ActionRow>>,
}

impl LaunchPage {
    pub fn new() -> Self {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 18);

        let project = adw::PreferencesGroup::builder().title("Project").build();
        let dir = adw::EntryRow::builder().title("Directory").show_apply_button(true).build();
        let build_button = gtk::Button::builder().label("Build").valign(gtk::Align::Center).build();
        let build_row = adw::ActionRow::builder().title("Build").subtitle_lines(2).build();
        build_row.add_suffix(&build_button);
        project.add(&dir);
        project.add(&build_row);
        let build_log = gtk::Label::builder().xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).selectable(true).visible(false).build();
        build_log.add_css_class("monospace");
        build_log.add_css_class("caption");
        body.append(&project);
        body.append(&build_log);

        let binaries = adw::PreferencesGroup::builder().title("Binary").description("Ranked; every row says why it is where it is.").build();
        body.append(&binaries);

        let run = adw::PreferencesGroup::builder().title("Run with").build();
        let args = adw::EntryRow::builder().title("Arguments").build();
        let env = adw::EntryRow::builder().title("Environment (KEY=value …)").build();
        let marker = adw::EntryRow::builder().title("Frame marker (function called once per frame)").build();
        run.add(&args);
        run.add(&env);
        run.add(&marker);
        body.append(&run);

        let history_group = adw::PreferencesGroup::builder().title("Shell history").build();
        let history_query = adw::EntryRow::builder().title("Search").build();
        history_group.add(&history_query);
        body.append(&history_group);

        let debug = gtk::Button::builder().label("Debug").halign(gtk::Align::Center).build();
        debug.add_css_class("pill");
        debug.add_css_class("suggested-action");
        body.append(&debug);

        LaunchPage {
            root: super::scrolled_clamp(&body),
            dir,
            build_row,
            build_button,
            build_log,
            binaries,
            args,
            env,
            marker,
            history_query,
            history_group,
            debug,
            binary_rows: RefCell::default(),
            history_rows: RefCell::default(),
        }
    }

    /// `on_pick(index)` fires when a row's radio becomes active.
    pub fn set_candidates(&self, c: &[Candidate], on_pick: impl Fn(usize) + Clone + 'static) {
        for r in self.binary_rows.borrow_mut().drain(..) {
            self.binaries.remove(&r);
        }
        let mut first: Option<gtk::CheckButton> = None;
        for (i, cand) in c.iter().enumerate().take(6) {
            let reasons: Vec<String> = cand.reasons.iter().map(|r| r.describe()).collect();
            let row = adw::ActionRow::builder()
                .title(cand.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                .subtitle(format!("{} · score {}\n{}", reasons.join(" · "), cand.score(), cand.path.display()))
                .subtitle_lines(4)
                .build();
            let radio = gtk::CheckButton::builder().valign(gtk::Align::Center).build();
            match &first {
                Some(f) => radio.set_group(Some(f)),
                None => first = Some(radio.clone()),
            }
            let pick = on_pick.clone();
            radio.connect_toggled(move |r| {
                if r.is_active() {
                    pick(i);
                }
            });
            row.add_prefix(&radio);
            row.set_activatable_widget(Some(&radio));
            self.binaries.add(&row);
            self.binary_rows.borrow_mut().push(row);
            if i == 0 {
                radio.set_active(true);
            }
        }
        if c.is_empty() {
            let row = adw::ActionRow::builder().title("No runnable ELF found").subtitle("Looked in the directory and the usual build trees. Build first, or pick another directory.").build();
            self.binaries.add(&row);
            self.binary_rows.borrow_mut().push(row);
        }
    }

    pub fn set_history_hits(&self, hits: &[(String, Option<Invocation>)], on_adopt: impl Fn(Invocation) + Clone + 'static) {
        for r in self.history_rows.borrow_mut().drain(..) {
            self.history_group.remove(&r);
        }
        for (cmd, inv) in hits {
            let row = adw::ActionRow::builder().title(gtk::glib::markup_escape_text(cmd)).title_lines(3).build();
            row.add_css_class("monospace");
            if let Some(inv) = inv.clone() {
                row.set_subtitle("tap to use these arguments");
                row.set_activatable(true);
                let adopt = on_adopt.clone();
                row.connect_activated(move |_| adopt(inv.clone()));
            }
            self.history_group.add(&row);
            self.history_rows.borrow_mut().push(row);
        }
    }

    pub fn set_invocation(&self, inv: &Invocation) {
        self.args.set_text(&inv.args.iter().map(|a| if a.contains(' ') { format!("'{a}'") } else { a.clone() }).collect::<Vec<_>>().join(" "));
        self.env.set_text(&inv.env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "));
    }
}
