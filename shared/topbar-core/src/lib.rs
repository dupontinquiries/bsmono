//! topbar-core — headless state for the cosmic-comp system bar.
//!
//! * [`tile::unify`] — merges pinned launcher entries (`hatch-core`,
//!   filtered to favorites by the caller) with running windows
//!   (`systemcompkit::ToplevelInfo`) into one ordered [`tile::TileItem`]
//!   list, tagged with a [`category::TileCategory`] and folding a
//!   pinned-and-running app into a single tile.
//! * [`placement::Placement`] — the form-factor shapes this bar can take
//!   (laptop top bar, mobile thumb sidebar, desktop bottom bar), each
//!   mapping to a `systemcompkit::LayerSurfaceIntent` and to which
//!   `TileCategory`s it shows. A caller filters `unify`'s output through
//!   `Placement::shows` before handing tiles to `Bar`.
//! * [`bar::Bar`] — collapsed/expanded mode plus the unified
//!   `hatch_core::anim::Board` that animates the (already-filtered) tile
//!   list. Placement-agnostic on purpose, same as `Board` is
//!   tile-content-agnostic.
//!
//! Nothing here touches egui or wayland-client. The shell that paints this
//! into an actual layer-shell surface is a separate crate over this one,
//! same split as `systemcompkit` itself.

pub mod bar;
pub mod category;
pub mod placement;
pub mod runtime;
pub mod tile;

pub use bar::{Bar, BarMode};
pub use category::TileCategory;
pub use placement::Placement;
pub use runtime::{BarSurface, Runtime};
pub use tile::{unify, TileItem, TileSource};
