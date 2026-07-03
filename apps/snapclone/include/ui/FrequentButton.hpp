#pragma once
// ─── ui/FrequentButton.hpp ────────────────────────────────────────────────────
//
// A "frequent button" is a control that grows when it's the PRIMARY, frequently-
// used action in the current context, and relaxes to a comfortable size when it's
// present but not the focus. The talk button is the first adopter: grand while in
// push-to-talk (you're hammering it), comfortable in the auto modes (VOX /
// continuous) where you rarely touch it.
//
// Buttons opt in with the flags below and ask scale() for their multiplier given
// whether they're currently the primary action. Centralizing the scale factors
// here keeps every frequent control consistent and tunable in one place.
// ─────────────────────────────────────────────────────────────────────────────

namespace SnapClone::UI::FrequentButton {

enum Flag {
    None              = 0,
    ExpandWhenPrimary = 1 << 0, ///< grow to the grand size while this is primary
};

constexpr double kGrandScale       = 1.50; ///< primary, high-frequency (e.g. PTT hold)
constexpr double kComfortableScale = 1.25; ///< present but not the focus (auto modes)

/// Multiplier over a base touch size for a button with the given flags. When the
/// button isn't flagged to expand, it always sits at the comfortable size.
inline double scale(int flags, bool primary)
{
    if ((flags & ExpandWhenPrimary) && primary) return kGrandScale;
    return kComfortableScale;
}

} // namespace SnapClone::UI::FrequentButton
