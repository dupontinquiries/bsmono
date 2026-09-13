//! Everything the Settings tab edits. Deliberately a normal `View` (see
//! `View::Settings`), not a popup — see `ui::settings_view`'s doc for why.

use photosync::DeduplicationConfig;

/// How a tile paints before there's a decoded bitmap to show — which, given
/// there's no thumbnail cache (see `Settings::dedup`'s doc on why one isn't
/// wired up), is most of the time. All three are pure per-frame painter
/// calls: nothing here is ever written to disk, so there's no cache to
/// grow, expire, or need a "clear thumbnails" button for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbStyle {
    /// A flat colour swatch, hashed from the path. Cheapest, plainest.
    Solid,
    /// A soft two-stop radial gradient in the same hashed hue — reads as
    /// "there's a photo here" without pretending to show one.
    Blur,
    /// A small hashed dot lattice over a dark tile, like a contact-sheet
    /// negative. More texture per pixel than `Solid` at the same cost.
    Tileset,
}

impl ThumbStyle {
    pub const ALL: [ThumbStyle; 3] = [ThumbStyle::Solid, ThumbStyle::Blur, ThumbStyle::Tileset];
    pub fn label(self) -> &'static str {
        match self {
            ThumbStyle::Solid => "Solid",
            ThumbStyle::Blur => "Blur",
            ThumbStyle::Tileset => "Tileset",
        }
    }
}

pub struct Settings {
    /// Mirrors `photosync::DeduplicationConfig` — the block store's actual
    /// chunking/hashing/compression knobs. This UI copy is what the
    /// controls below edit; nothing here calls into a running daemon yet
    /// (there isn't one wired to this shell — see `App::demo`'s doc), so
    /// changing these currently only shapes what a *future* sync would be
    /// configured with, not an in-flight one. Kept as the real config type
    /// rather than a shadow struct so the two can't drift apart on their
    /// own — one shape, one source of truth.
    pub dedup: DeduplicationConfig,
    pub thumb_style: ThumbStyle,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { dedup: DeduplicationConfig::default(), thumb_style: ThumbStyle::Blur }
    }
}
