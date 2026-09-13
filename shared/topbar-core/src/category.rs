//! category — what *kind* of thing a tile is, independent of where it's
//! drawn. Exists so `placement` can answer "does this form factor show
//! system monitoring" as a data lookup instead of a hardcoded filter
//! buried in a shell somewhere by the time there are three shells (laptop
//! topbar, mobile sidebar, desktop bottom bar) each wanting a different
//! subset.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TileCategory {
    /// A pinned/favorited launcher entry.
    Launcher,
    /// A running window (from `systemcompkit::ToplevelInfo`).
    Taskbar,
    /// CPU/memory/battery/network-style readouts. No source produces these
    /// yet anywhere in this crate — `unify()` never emits this category —
    /// this variant exists now so `Placement`'s visibility table doesn't
    /// need reshaping when that source shows up (the "full power bottom
    /// bar with extra system monitoring" shape).
    SystemMonitor,
}
