#pragma once

// ─── SnapComposerDialog ───────────────────────────────────────────────────────
//
// Full-screen text-overlay editor shown between capture and send.
// Works in two modes:
//   • Photo mode  — the captured image fills the background; user adds text.
//   • Text mode   — solid black background for text-only snaps (cameraless).
//
// The user types text, repositions it by tapping (centres on tap), picks a
// colour, and presses Send. The composed image (background + text rendered in)
// is returned via snapReady(QImage).
//
// Text is rendered directly into the QImage before sending — no SVG or
// separate overlay layer needed. The dialog can be dismissed with the X button.

#include "ui/MobileDialog.hpp"
#include <QImage>
#include <QPoint>

QT_BEGIN_NAMESPACE
class QTextEdit;
class QPushButton;
class QLabel;
class QToolButton;
class QSlider;
class QVBoxLayout;
QT_END_NAMESPACE

namespace SnapClone::UI {

class SnapComposerDialog : public MobileDialog
{
    Q_OBJECT

public:
    /// Photo mode — image is the captured frame.
    [[nodiscard]] QString caption() const;
    explicit SnapComposerDialog(const QImage &image, QWidget *parent = nullptr);

    /// Text mode — black background (for cameraless devices or text-only snaps).
    explicit SnapComposerDialog(QWidget *parent = nullptr);

    /// Composed image with text baked in, ready to encode and send.
    [[nodiscard]] QImage composedImage() const { return m_composed; }

signals:
    void snapReady(QImage image, int durationSecs);

protected:
    void paintEvent(QPaintEvent *) override;
    void mousePressEvent(QMouseEvent *) override;
    void mouseReleaseEvent(QMouseEvent *) override;
    void keyPressEvent(QKeyEvent *) override;

private:
    void buildUi();
    void renderComposed(); ///< Bake text onto background into m_composed.
    void cycleColor();
    /// Lift the bottom panel above the on-screen keyboard (keyboard avoidance).
    void applyKeyboardInset(bool visible, const QRect &kbRect);

    QImage      m_background;      // source image or solid black
    QImage      m_composed;        // rendered output
    QString     m_text;
    QPointF     m_textPos;         // fractional position [0,1]x[0,1]
    QColor      m_textColor  { Qt::white };
    int         m_fontSize   { 28 };
    bool        m_textMode{false}; // true = cameraless black background

    // UI widgets
    QTextEdit   *m_editor    = nullptr;
    QPushButton *m_sendBtn   = nullptr;
    /// V32.32: "Save to Pictures" button — writes the composed image
    /// to ~/Pictures/. Sits to the left of Send; doesn't close the
    /// dialog so user can save THEN send. See ctor for the save path.
    QPushButton *m_saveBtn   = nullptr;
    QPushButton *m_cancelBtn = nullptr;
    QToolButton *m_colorBtn  = nullptr;
    QSlider     *m_sizeSlider= nullptr;
    QLabel      *m_hint      = nullptr;

    QVBoxLayout *m_root      = nullptr;  ///< root layout; bottom margin = keyboard inset
    QPoint       m_pressStart;           ///< background press origin (swipe-up-to-send)
    bool         m_pressOnBackground = false;

    static const QList<QColor> kColors;
    int m_colorIndex   = 0;
    int m_durationSecs = 5;  ///< snap timer: 0 = no limit
};

} // namespace SnapClone::UI
