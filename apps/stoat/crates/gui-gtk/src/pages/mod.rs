pub mod caches;
pub mod launch;
pub mod stack;
pub mod vitals;
pub mod wire;

use gtk::prelude::*;

/// Page body: scrolls vertically, clamps width on wide windows, pads on narrow ones.
pub fn scrolled_clamp(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let clamp = adw::Clamp::builder().maximum_size(720).child(child).margin_top(12).margin_bottom(12).margin_start(12).margin_end(12).build();
    gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&clamp).vexpand(true).build()
}
