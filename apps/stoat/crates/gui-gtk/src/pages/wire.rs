use gtk::prelude::*;

pub struct WirePage {
    pub root: gtk::Box,
    pub entry: gtk::Entry,
    buffer: gtk::TextBuffer,
    view: gtk::TextView,
}

impl WirePage {
    pub fn new() -> Self {
        let view = gtk::TextView::builder().editable(false).cursor_visible(false).monospace(true).wrap_mode(gtk::WrapMode::WordChar).left_margin(8).right_margin(8).top_margin(8).build();
        view.add_css_class("caption");
        let buffer = view.buffer();
        let scroll = gtk::ScrolledWindow::builder().vexpand(true).child(&view).build();
        let entry = gtk::Entry::builder().placeholder_text("gdb command, e.g. info sharedlibrary").margin_start(8).margin_end(8).margin_top(6).margin_bottom(6).build();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&scroll);
        root.append(&entry);
        WirePage { root, entry, buffer, view }
    }

    pub fn push(&self, prefix: &str, text: &str) {
        let mut end = self.buffer.end_iter();
        for line in text.trim_end_matches('\n').split('\n') {
            self.buffer.insert(&mut end, &format!("{prefix}{line}\n"));
        }
        // An MI session is thousands of lines an hour; keep the tail.
        if self.buffer.line_count() > 3000 {
            let mut start = self.buffer.start_iter();
            if let Some(mut cut) = self.buffer.iter_at_line(1000) {
                self.buffer.delete(&mut start, &mut cut);
            }
        }
        let mark = self.buffer.create_mark(None, &self.buffer.end_iter(), false);
        self.view.scroll_mark_onscreen(&mark);
        self.buffer.delete_mark(&mark);
    }
}
