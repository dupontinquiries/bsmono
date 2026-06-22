//! SnapApp — the top-level application trait.
//!
//! Implement this to wire any UI backend to the SnapClone core.
//! The Qt backend implements this as MainWindow + AppController.

use crate::{TabBar, GestureHost, ContactList};
use snapclone_core::{AppCore, AppConfig, AppEvent, AppCommand};

/// The tabs shown in the bottom navigation bar.
/// Order matches kPage* constants in C++ MainWindow.hpp.
pub const TABS: &[crate::TabDef] = &[
    crate::TabDef { id: "camera",   label: "Camera",  icon: "📷" },
    crate::TabDef { id: "inbox",    label: "Inbox",   icon: "💬" },
    crate::TabDef { id: "contacts", label: "Friends", icon: "👥" },
    crate::TabDef { id: "settings", label: "Settings",icon: "⚙" },
];

/// Tab indices (match C++ kPageCamera etc.)
pub const TAB_CAMERA:   usize = 0;
pub const TAB_INBOX:    usize = 1;
pub const TAB_CONTACTS: usize = 2;
pub const TAB_SETTINGS: usize = 3;

/// Top-level application contract.
///
/// Implementing this trait gives you a fully functional SnapClone application.
/// The trait separates "wiring" (done once, here) from "rendering" (done by
/// the framework-specific implementation of the sub-traits).
pub trait SnapApp {
    type Tabs:     TabBar;
    type Gestures: GestureHost;
    type Contacts: ContactList;

    fn tab_bar(&mut self)      -> &mut Self::Tabs;
    fn gesture_host(&mut self) -> &mut Self::Gestures;
    fn contact_list(&mut self) -> &mut Self::Contacts;

    /// Called with each AppEvent from the core. Dispatch to sub-components.
    ///
    /// Default implementation covers the common cases; override for custom handling.
    fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::ContactsLoaded { contacts } => {
                let items = contacts.iter().map(Into::into).collect();
                self.contact_list().set_contacts(items);
            }
            AppEvent::ContactUpdated { contact } => {
                self.contact_list().upsert_contact((&contact).into());
            }
            AppEvent::ContactRemoved { id } => {
                self.contact_list().remove_contact(&id);
            }
            AppEvent::PeerConnected { peer } => {
                self.contact_list().set_online(&peer.id, true);
            }
            AppEvent::PeerDisconnected { id } => {
                self.contact_list().set_online(&id, false);
            }
            AppEvent::SnapReceived { .. } => {
                // Increment inbox badge
                let current = self.tab_bar().current();
                if current != TAB_INBOX {
                    // TODO: read actual unread count from core
                    self.tab_bar().set_badge(TAB_INBOX, crate::BadgeCount(1));
                }
            }
            AppEvent::NetworkChanged { online } => {
                if !online {
                    // Visual feedback handled by concrete UI
                }
            }
            _ => {}
        }
    }

    /// Run the app event loop. Poll `core` for events and call `handle_event`.
    /// For Qt: this is replaced by the signal/slot system.
    /// For egui: call this every frame from `update()`.
    fn poll_events(&mut self, core: &AppCore) {
        // Drain the event channel without blocking
        // (in practice each framework has its own polling mechanism)
        let _ = core; // core.evt_rx is private — real impl uses an Arc<Mutex<>> or channel
    }
}
