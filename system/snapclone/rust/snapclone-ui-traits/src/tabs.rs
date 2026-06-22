//! Tab bar contract — defines what a tab bar needs to expose.
//!
//! The Qt implementation is TabBar.cpp + SwipeNavigator.cpp.
//! An egui implementation would be a top/bottom button row.
//! A web implementation would be a nav element.
//! All implement `TabBar<T>` where T is the page content type.

/// Definition of a single tab. Passed to `TabBar::set_tabs()`.
#[derive(Debug, Clone)]
pub struct TabDef {
    /// Short identifier (used as key, e.g. "camera", "inbox")
    pub id:    &'static str,
    /// Display label below the icon
    pub label: &'static str,
    /// Icon — either a unicode emoji or a named icon key for the framework
    pub icon:  &'static str,
}

/// Badge count shown on a tab (e.g. unread snap count on Inbox).
#[derive(Debug, Clone, Copy, Default)]
pub struct BadgeCount(pub u32);

impl BadgeCount {
    pub fn none() -> Self { Self(0) }
    pub fn is_visible(&self) -> bool { self.0 > 0 }
}

/// The tab bar contract. Implement for your UI framework.
///
/// # Qt mapping
///
/// ```text
/// TabBar::addTab()          → TabBar::set_tabs()
/// TabBar::setCurrentIndex() → TabBar::select()
/// TabBar::setBadge()        → TabBar::set_badge()
/// TabBar::tabSelected       → TabBar::on_select callback
/// ```
pub trait TabBar {
    /// Replace the tab list. Called once at startup.
    fn set_tabs(&mut self, tabs: &[TabDef]);

    /// Programmatically switch to a tab by index.
    fn select(&mut self, index: usize);

    /// Current active tab index.
    fn current(&self) -> usize;

    /// Set (or clear) the badge on a tab.
    fn set_badge(&mut self, tab_index: usize, count: BadgeCount);

    /// Register a callback invoked when the user taps a tab.
    /// The callback receives the new tab index.
    fn on_select(&mut self, f: Box<dyn Fn(usize) + Send + 'static>);
}
