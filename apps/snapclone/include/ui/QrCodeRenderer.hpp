#pragma once
// ─── QrCodeRenderer.hpp ──────────────────────────────────────────────────────
//
// Theme-aware QR code rendering.
//
// • makeIdQr(id, px) — produces a QPixmap encoding the SnapClone ID
//   in a custom (non-standard) module pattern. Both endpoints of a
//   SnapClone connection use the matching decoder in QrScanDialog,
//   so this works for our use case but is NOT standards-compliant.
//
// • showExpandedQr(parent, id, anchorRect) — opens an animated full-screen
//   overlay showing the same QR larger, themed and dismissable.
//
// Color model:
//   • Background = Theme::BG_CARD (matches surrounding UI)
//   • Foreground = Theme::TEXT_PRIMARY by default
//   • Falls back to pure black/white if WCAG contrast < 4.5:1
//
// Smoothing: at sizes > 200px, applies subpixel smoothing via
// downsample-then-upsample to soften aliasing for phone cameras.

#include <QPixmap>
#include <QString>

class QWidget;

namespace SnapClone::UI {

/// Render a QR-style pixmap encoding the given identifier.
/// Returns a square QPixmap of approximately `px` pixels.
QPixmap makeIdQr(const QString &id, int px);

/// Open a full-screen animated overlay showing the QR code expanded.
/// `parent` is used to find the top-level window. Returns immediately;
/// the overlay self-deletes when dismissed.
void showExpandedQr(QWidget *parent, const QString &id);

} // namespace SnapClone::UI
