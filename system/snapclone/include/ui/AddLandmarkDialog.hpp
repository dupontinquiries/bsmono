#pragma once
// ─── UI/AddLandmarkDialog.hpp ────────────────────────────────────────────────
//
// V32.31: dialog for creating a new landmark.
//
// Minimal V1 inputs:
//   • Name (free-text, required)
//   • Current location — read from AppController::lastKnownLocation(self).
//     If none, the dialog shows a clear "Set your location first" hint
//     pointing the user at Settings → Privacy → Manual location.
//
// Optional V1 inputs:
//   • Photo: NOT included in V1. A future revision will allow either
//     "take photo now" (camera quick-capture) or "use a recent snap"
//     (pick from inbox). Photo gets hashed via LandmarkHash, the
//     hashes go on the landmark record, the photo itself does NOT.
//   • Scope: NOT included in V1. All landmarks are LandmarkScope::Private.
//     Phase 2 adds a scope picker.
//
// Result: emits saved(Landmark) on success; the parent (ContactsPage)
// is responsible for persisting via StorageManager. The dialog stays
// stateless — no direct storage access.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/MobileDialog.hpp"
#include "core/Types.hpp"

#include <QImage>   // V32.34+: m_capturedFrame

QT_BEGIN_NAMESPACE
class QLineEdit;
class QLabel;
class QPushButton;
QT_END_NAMESPACE

namespace SnapClone::UI {

class AddLandmarkDialog : public MobileDialog {
    Q_OBJECT
public:
    /// `selfId` is the creator-id we'll stamp on the produced Landmark.
    /// `currentLat`, `currentLon`, `currentAccM` are pre-resolved by the
    /// caller (typically via AppController::lastKnownLocation) — if NaN,
    /// the dialog refuses to save and shows the "set location first" hint.
    explicit AddLandmarkDialog(const SnapClone::Core::UserId &selfId,
                                double currentLat, double currentLon,
                                double currentAccM,
                                QWidget *parent = nullptr);

    /// V32.34+: attach a captured viewfinder frame. When set, the
    /// dialog computes `LandmarkHash::{blurHash, edgeHash}` on Save
    /// and stamps the hashes on the produced Landmark so the
    /// recipient (or future me on this device) can match against the
    /// same scene later. The frame itself is NEVER stored on the
    /// landmark or persisted — only its lossy fingerprints. Safe to
    /// call multiple times; only the most recent frame matters.
    ///
    /// Call BEFORE exec()/open(); buildUi() reads m_haveFrame to
    /// decide whether to show the "Photo attached" badge.
    void setCapturedFrame(const QImage &frame);

signals:
    /// Fired exactly once when the user taps Save with valid input.
    void saved(SnapClone::Core::Landmark landmark);

private:
    void buildUi();
    void onSaveClicked();

    SnapClone::Core::UserId m_selfId;
    double m_lat;
    double m_lon;
    double m_acc;
    bool   m_haveFix;
    /// V32.34+: optional captured frame. Used to compute photoBlurHash
    /// and photoEdgeHash at save time. The QImage is held until the
    /// dialog closes; not persisted anywhere outside this dialog.
    QImage m_capturedFrame;

    QLineEdit *m_nameEdit = nullptr;
    QLabel    *m_locLabel = nullptr;
    QLabel    *m_photoBadge = nullptr;  ///< V32.34+: "Photo attached" indicator
    QPushButton *m_saveBtn = nullptr;
};

} // namespace SnapClone::UI
