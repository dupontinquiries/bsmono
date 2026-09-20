use adw::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use stoat_core::cache::{self, CacheKind, PrunePlan, Usage};

pub struct CachePage {
    pub root: gtk::ScrolledWindow,
    pub days: gtk::SpinButton,
    pub plan: gtk::Button,
    pub apply: gtk::Button,
    pub debuginfod: adw::SwitchRow,
    group: adw::PreferencesGroup,
    rows: RefCell<Vec<(PathBuf, adw::ActionRow, gtk::Label)>>,
}

impl CachePage {
    pub fn new() -> Self {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
        let net = adw::PreferencesGroup::builder().title("Fetching").build();
        let debuginfod = adw::SwitchRow::builder().title("Download debug info (debuginfod)").subtitle("Off by default: one stop inside a toolkit can fetch hundreds of MB. Applies to the next session.").build();
        net.add(&debuginfod);
        body.append(&net);

        let group = adw::PreferencesGroup::builder().title("On disk").build();
        body.append(&group);

        // A spin *row* plus two equal-width buttons: a single horizontal strip of
        // label + spin + two buttons needs ~530 px and would set the minimum width
        // of the whole window, since AdwViewStack is homogeneous.
        let prune = adw::PreferencesGroup::builder().title("Prune").description("Planning only lists what would go; nothing is deleted until you confirm.").build();
        let days = gtk::SpinButton::with_range(1.0, 365.0, 1.0);
        days.set_value(30.0);
        days.set_valign(gtk::Align::Center);
        let days_row = adw::ActionRow::builder().title("Unused for (days)").build();
        days_row.add_suffix(&days);
        prune.add(&days_row);
        body.append(&prune);
        let controls = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(8).homogeneous(true).build();
        let plan = gtk::Button::with_label("Plan");
        let apply = gtk::Button::builder().label("Delete").sensitive(false).build();
        apply.add_css_class("destructive-action");
        controls.append(&plan);
        controls.append(&apply);
        body.append(&controls);

        CachePage { root: super::scrolled_clamp(&body), days, plan, apply, debuginfod, group, rows: RefCell::default() }
    }

    pub fn set_usage(&self, items: &[(CacheKind, PathBuf, Usage)]) {
        for (_, row, _) in self.rows.borrow_mut().drain(..) {
            self.group.remove(&row);
        }
        for (kind, path, u) in items {
            let size = gtk::Label::new(Some(&cache::human_bytes(u.bytes)));
            size.add_css_class("numeric");
            let note = if u.unreadable { " · not readable as this user" } else { "" };
            let row = adw::ActionRow::builder().title(kind.label()).subtitle(format!("{} · {} files{note}", path.display(), u.files)).subtitle_lines(3).build();
            row.add_suffix(&size);
            self.group.add(&row);
            self.rows.borrow_mut().push((path.clone(), row, size));
        }
        if items.is_empty() {
            let row = adw::ActionRow::builder().title("No debug caches found").build();
            self.group.add(&row);
            self.rows.borrow_mut().push((PathBuf::new(), row, gtk::Label::new(None)));
        }
    }

    pub fn show_plans(&self, plans: &[(PathBuf, PrunePlan)]) {
        for (path, _, size) in self.rows.borrow().iter() {
            if let Some((_, p)) = plans.iter().find(|(pp, _)| pp == path) {
                size.set_text(&format!("−{} ({} files)", cache::human_bytes(p.bytes), p.remove.len()));
            }
        }
        self.apply.set_sensitive(plans.iter().any(|(_, p)| !p.remove.is_empty()));
    }
}
