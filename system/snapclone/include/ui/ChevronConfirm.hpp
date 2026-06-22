#pragma once
// ─── ChevronConfirm.hpp ───────────────────────────────────────────────────────
//
// Animated confirmation panel for potentially accidental actions.
//
// Appears anchored below (or above) a button, slides in with a chevron
// pointing back at the button, auto-dismisses after 4 seconds.
//
// Two confirmation modes (chosen based on UxGuide familiarity):
//
//   TapConfirm   — shown to new users (familiarity < 40)
//     A visible panel with "Are you sure?" + Cancel/Confirm buttons.
//     Requires an explicit tap on Confirm.
//
//   HoldConfirm  — shown to intermediate users (familiarity 40–80)
//     A hold-to-confirm button: press and hold for 1.2s with a progress ring.
//     Familiar gesture — quicker than TapConfirm but still intentional.
//
//   NoConfirm    — expert users (familiarity >= 80)
//     Action proceeds immediately with a brief haptic pulse animation.
//
// Usage:
//   ChevronConfirm::show(exportBtn, tr("Export all contacts?"),
//                        Feature::kExportVCard, [this]() { doExport(); });

#include <QWidget>
#include <functional>

namespace SnapClone::UI {

class ChevronConfirm : public QWidget
{
    Q_OBJECT
public:
    enum class Mode { Tap, Hold, None };

    /// Show the appropriate confirmation for the given feature key.
    /// anchor   — the button that was pressed (panel appears near it)
    /// label    — action description shown in the panel
    /// feature  — UxGuide feature key for familiarity lookup
    /// onConfirm — called when/if the user confirms
    static void present(QWidget              *anchor,
                        const QString        &label,
                        const QString        &featureKey,
                        std::function<void()> onConfirm);

    /// Immediately animate out and destroy self.
    void dismiss();

protected:
    void paintEvent(QPaintEvent *e) override;

private:
    explicit ChevronConfirm(QWidget *parent, Mode mode,
                            const QString &label,
                            std::function<void()> onConfirm);

    void buildTapUi(const QString &label);
    void buildHoldUi(const QString &label);
    void animateIn();
    void startAutoDismiss();

    Mode                    m_mode;
    std::function<void()>   m_onConfirm;
    QTimer                 *m_dismissTimer = nullptr;
    QTimer                 *m_holdTimer    = nullptr;  // for hold mode
    int                     m_holdProgress = 0;        // 0–100
    bool                    m_holdActive   = false;

    // Chevron pointing direction
    bool m_pointUp = true;  // true = chevron points up toward the button above
};

} // namespace SnapClone::UI
