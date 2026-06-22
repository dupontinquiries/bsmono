#pragma once

// ─── ui/SubTabbed.hpp ─────────────────────────────────────────────────────────
//
// Interface a page implements when it has in-page sub-tabs that participate in
// the unified two-level navigation (V32.34+ directive). SwipeNavigator and the
// unified bottom bar (SmartRibbon) talk to a page through this interface so the
// navigation logic stays generic — it doesn't need to know whether the page is
// the Inbox (All/Unread/Sent), Contacts (All/Online/Pending), or Settings
// (General/Camera/Network/Storage/About).
//
// The two-level model (see docs/TODO.md "unified bar + two-level horizontal
// swipe"):
//   • HORIZONTAL swipe scrubs sub-tabs (subTabCount / currentSubTab /
//     setCurrentSubTab); at the cycle boundary a friction detent breaks to the
//     adjacent PAGE.
//   • VERTICAL content scroll, with an end-of-content detent that advances the
//     sub-tab (down = next, up = prev), symmetric, after the scroll hits its
//     limit and a ~1s armed window.
//   • TAP jumps directly to a sub-tab.
//
// A page with no sub-tabs simply reports subTabCount() == 0; the navigator then
// treats it as a single flat page (horizontal swipe goes straight to page
// paging, vertical is plain scroll with no sub-tab detent).
//
// This is a pure interface (no Q_OBJECT) — pages multiply-inherit it alongside
// QWidget. Implementations should be cheap, synchronous accessors.
// ─────────────────────────────────────────────────────────────────────────────

#include <QString>

namespace SnapClone::UI {

class SubTabbed
{
public:
    virtual ~SubTabbed() = default;

    /// Number of sub-tabs this page exposes. 0 = flat page (no sub-tabs).
    [[nodiscard]] virtual int subTabCount() const = 0;

    /// Index of the currently-active sub-tab in [0, subTabCount). Undefined
    /// (return 0) when subTabCount()==0.
    [[nodiscard]] virtual int currentSubTab() const = 0;

    /// Switch to a sub-tab. Implementations clamp out-of-range indices and
    /// should be idempotent (selecting the current sub-tab is a no-op beyond
    /// any normal refresh). This is what a TAP, a horizontal scrub commit, or
    /// a vertical end-of-content detent all funnel through.
    virtual void setCurrentSubTab(int index) = 0;

    /// Short label for a sub-tab (e.g. "Unread"), used by the unified bar to
    /// render the inline sub-tab list. Empty string for out-of-range.
    [[nodiscard]] virtual QString subTabLabel(int index) const = 0;

    /// Optional unread/attention badge count for a sub-tab (0 = none). Default
    /// returns 0 so pages that don't badge needn't implement it.
    [[nodiscard]] virtual int subTabBadge(int /*index*/) const { return 0; }

    /// V32.34+ optional icon NAME for a sub-tab (an SvgIcon asset name like
    /// "camera", "settings"), used by the unified bar's responsive collapse to
    /// shrink an inactive sub-tab to an icon on narrow screens. Default empty:
    /// most sub-tabs (All/Unread/Sent, Online/Pending) have no dedicated icon,
    /// in which case the bar falls back to a short text form instead. Pages
    /// whose sub-tabs DO map to an existing icon (e.g. Settings "Camera" →
    /// "camera") can override this for those indices only.
    [[nodiscard]] virtual QString subTabIcon(int /*index*/) const { return {}; }
};

} // namespace SnapClone::UI
