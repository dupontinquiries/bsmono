#include "ui/SnapComposerDialog.hpp"
#include "ui/Theme.hpp"

#include <QPainter>
#include <QPointer>
#include <QMouseEvent>
#include <QKeyEvent>
#include <QPaintEvent>
#include <QTextEdit>
#include <QPushButton>
#include <QToolButton>
#include <QSlider>
#include <QLabel>
#include <QVBoxLayout>
#include <QHBoxLayout>

#include "ui/VirtualKeyboardWatcher.hpp"
#include <QFontMetrics>
#include <QScreen>
#include <QGuiApplication>
#include <QTimer>
// V32.32: needed for "Save to Pictures" feature.
#include <QStandardPaths>
#include <QDir>
#include <QDateTime>
#include <QFileInfo>

namespace SnapClone::UI {

// Colour cycle: white → yellow → cyan → orange → black
const QList<QColor> SnapComposerDialog::kColors = {
    Qt::white,
    QColor(0xFF, 0xEB, 0x3B),   // yellow
    QColor(0x00, 0xE5, 0xFF),   // cyan
    QColor(0xFF, 0x6D, 0x00),   // orange
    Qt::black,
};

// ─── Construction ─────────────────────────────────────────────────────────────

// caption() stub — Snap struct has no caption field yet.
QString SnapComposerDialog::caption() const { return {}; }

SnapComposerDialog::SnapComposerDialog(const QImage &image, QWidget *parent)
    : MobileDialog(parent)
    , m_background(image)
    , m_textMode(false)
{
    // Inspecting a captured photo before sending it legitimately takes a while
    // with no taps — give the preview the longer photo idle window.
    setDismissContext(DismissContext::Photo);
    m_textPos = { 0.5, 0.5 };
    buildUi();
}


SnapComposerDialog::SnapComposerDialog(QWidget *parent)
    : MobileDialog(parent)
    , m_textMode(true)
{
    m_textPos = { 0.5, 0.5 };
    buildUi();
}

// ─── UI ───────────────────────────────────────────────────────────────────────

void SnapComposerDialog::buildUi()
{
    setModal(true);
    // Fill the whole screen on mobile.
    if (const QScreen *s = QGuiApplication::primaryScreen())
        setGeometry(s->geometry());

    // Transparent background so our paintEvent controls everything.
    setAttribute(Qt::WA_TranslucentBackground);
    setAutoFillBackground(false);

    // ── Top toolbar ───────────────────────────────────────────────────────
    auto *topBar = new QHBoxLayout;
    topBar->setContentsMargins(Theme::SPACE_SM, Theme::SPACE_SM,
                               Theme::SPACE_SM, Theme::SPACE_SM);
    topBar->setSpacing(Theme::SPACE_SM);

    m_cancelBtn = new QPushButton(QStringLiteral("✕"), this);
    m_cancelBtn->setFixedSize(Theme::TOUCH_ICON, Theme::TOUCH_ICON);
    m_cancelBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background: %2; color: %3;"
        "  border-radius: %1px; font-size: 18px; font-weight: bold; }"
        "QPushButton:pressed { background: %4; }")
        .arg(Theme::TOUCH_ICON / 2)
        .arg(QLatin1String(Theme::BG_DEEP()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::ACCENT_DIM())));
    connect(m_cancelBtn, &QPushButton::clicked, this, &QDialog::reject);
    topBar->addWidget(m_cancelBtn);

    topBar->addStretch();

    // Colour cycle button
    m_colorBtn = new QToolButton(this);
    m_colorBtn->setFixedSize(Theme::TOUCH_ICON, Theme::TOUCH_ICON);
    m_colorBtn->setText(QStringLiteral("A"));
    m_colorBtn->setStyleSheet(QStringLiteral(
        "QToolButton { background: %2;"
        "  border-radius: %1px; font-size: 20px; font-weight: bold;"
        "  color: %3; }"
        "QToolButton:pressed { background: %4; }")
        .arg(Theme::TOUCH_ICON / 2)
        .arg(QLatin1String(Theme::BG_DEEP()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::ACCENT_DIM())));
    connect(m_colorBtn, &QToolButton::clicked, this, [this]() {
        cycleColor();
        update(); // repaint preview
    });
    topBar->addWidget(m_colorBtn);

    // ── Text input ────────────────────────────────────────────────────────
    // Text editor — kept compact (one line) so the focus stays on the live
    // text overlay drawn directly on the photo via paintEvent. The user
    // sees their text at the chosen font size right on the image; this
    // input is just where they type/edit it.
    //
    // Theme-aware styling via Theme::BG_DEEP/TEXT_PRIMARY/ACCENT.
    m_editor = new QTextEdit(this);
    m_editor->setPlaceholderText(tr("Type your message…"));
    m_editor->setAcceptRichText(false);
    m_editor->setAlignment(Qt::AlignCenter);
    m_editor->setFixedHeight(56);  // one line + padding, was 120px
    m_editor->setVerticalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    m_editor->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    m_editor->setStyleSheet(QStringLiteral(
        "QTextEdit {"
        "  background: %3;"
        "  color: %4;"
        "  border: 1px solid %5;"
        "  border-radius: %1px;"
        "  font-size: 16px;"
        "  padding: 8px 12px;"
        "}"
        "QTextEdit:focus { border: 2px solid %2; }")
        .arg(Theme::RADIUS_MD)
        .arg(QLatin1String(Theme::ACCENT()))
        .arg(QLatin1String(Theme::BG_DEEP()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::ACCENT_DIM())));
    connect(m_editor, &QTextEdit::textChanged, this, [this]() { update(); });

    // ── Font size slider ──────────────────────────────────────────────────
    auto *sizeRow = new QHBoxLayout;
    auto *sizeSmall = new QLabel(QStringLiteral("A"), this);
    sizeSmall->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 12px; background: transparent;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())));
    auto *sizeLarge = new QLabel(QStringLiteral("A"), this);
    sizeLarge->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 24px; background: transparent;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())));

    m_sizeSlider = new QSlider(Qt::Horizontal, this);
    m_sizeSlider->setRange(14, 72);
    m_sizeSlider->setValue(m_fontSize);
    m_sizeSlider->setStyleSheet(QStringLiteral(
        "QSlider::groove:horizontal { height: 4px; background: rgba(255,255,255,0.3);"
        "  border-radius: 2px; }"
        "QSlider::handle:horizontal { width: 20px; height: 20px; margin: -8px 0;"
        "  background: %1; border-radius: 10px; }")
        .arg(QLatin1String(Theme::ACCENT())));
    connect(m_sizeSlider, &QSlider::valueChanged, this, [this](int v) {
        m_fontSize = v;
        update();
    });

    sizeRow->addWidget(sizeSmall);
    sizeRow->addWidget(m_sizeSlider, 1);
    sizeRow->addWidget(sizeLarge);

    // Hint label
    m_hint = new QLabel(tr("Tap image to reposition text"), this);
    m_hint->setAlignment(Qt::AlignCenter);
    m_hint->setStyleSheet(
        QStringLiteral("color: %1; font-size: 11px; background: transparent;")
            .arg(QLatin1String(Theme::TEXT_PRIMARY())));

    // ── Send button ───────────────────────────────────────────────────────
    // ── Duration selector ─────────────────────────────────────────────────
    auto *durationRow = new QHBoxLayout;
    auto *durLabel = new QLabel(tr("Timer:"), this);
    durLabel->setStyleSheet(QStringLiteral("color: rgba(255,255,255,0.7); font-size:13px; background:transparent;"));
    durationRow->addWidget(durLabel);

    const struct { const char *label; int secs; } kDurations[] = {
        {"3s", 3}, {"5s", 5}, {"10s", 10}, {"∞", 0}
    };
    m_durationSecs = 5; // default

    auto *durGroup = new QHBoxLayout;
    durGroup->setSpacing(6);
    for (const auto &d : kDurations) {
        auto *btn = new QPushButton(QString::fromLatin1(d.label), this);
        const int secs = d.secs;
        btn->setCheckable(true);
        btn->setChecked(secs == 5);
        btn->setFixedHeight(32);
        btn->setStyleSheet(QStringLiteral(
            "QPushButton { background: %2; color: %3;"
            "  border: 1px solid %4; border-radius: 6px;"
            "  font-size: 13px; font-weight: 600; padding: 0 10px; }"
            "QPushButton:checked { background: %1; color: %5; border-color: %1; }"
            "QPushButton:pressed { background: %4; }")
            .arg(QLatin1String(Theme::ACCENT()))
            .arg(QLatin1String(Theme::BG_DEEP()))
            .arg(QLatin1String(Theme::TEXT_PRIMARY()))
            .arg(QLatin1String(Theme::ACCENT_DIM()))
            .arg(QLatin1String(Theme::TEXT_INVERSE())));
        connect(btn, &QPushButton::clicked, this, [this, btn, secs]() {
            m_durationSecs = secs;
            // Uncheck all sibling duration buttons, check this one
            const auto siblings = btn->parentWidget()
                ? btn->parentWidget()->findChildren<QPushButton *>()
                : QList<QPushButton *>{};
            for (auto *s : siblings)
                if (s != btn) s->setChecked(false);
            btn->setChecked(true);
        });
        durGroup->addWidget(btn);
    }
    durationRow->addLayout(durGroup);
    durationRow->addStretch();

    m_sendBtn = new QPushButton(tr("Send  ▶"), this);
    m_sendBtn->setMinimumHeight(Theme::TOUCH_LARGE);
    m_sendBtn->setStyleSheet(QStringLiteral(
        "QPushButton {"
        "  background: %1; color: %2;"
        "  border-radius: %3px; font-size: 17px; font-weight: bold; }"
        "QPushButton:pressed { background: %4; }")
        .arg(QLatin1String(Theme::ACCENT()))
        .arg(QLatin1String(Theme::TEXT_INVERSE()))
        .arg(Theme::RADIUS_LG)
        .arg(QLatin1String(Theme::ACCENT_SOFT())));
    connect(m_sendBtn, &QPushButton::clicked, this, [this]() {
        renderComposed();
        emit snapReady(m_composed, m_durationSecs);
        accept();
    });

    // V32.32: "Save to Pictures" button. Sits to the LEFT of Send,
    // smaller and less visually prominent (Send is the primary action).
    // Saves the composed image (with text overlay) to
    // ~/Pictures/snapclone_YYYY-MM-DD_HH-MM-SS.jpg. Doesn't close the
    // dialog — typical workflow is "save a copy locally, THEN send to a
    // friend" so the user keeps composing after saving. Feedback is
    // shown by reusing the existing m_hint label at the top of the
    // bottom panel.
    //
    // Reasoning on layout: a row of [Save | Send] with Send taking
    // 70-80% of the width keeps the send action dominant. A separate
    // row would chew vertical space and push the bottom panel higher
    // on smaller screens.
    m_saveBtn = new QPushButton(tr("💾 Save"), this);
    m_saveBtn->setMinimumHeight(Theme::TOUCH_LARGE);
    m_saveBtn->setStyleSheet(QStringLiteral(
        "QPushButton {"
        "  background: %1; color: %2;"
        "  border: 1px solid %3; border-radius: %4px;"
        "  font-size: 14px; padding: 0 12px; }"
        "QPushButton:pressed { background: %5; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::ACCENT_DIM()))
        .arg(Theme::RADIUS_LG)
        .arg(QLatin1String(Theme::BG_HOVER())));
    connect(m_saveBtn, &QPushButton::clicked, this, [this]() {
        // Render the composed image WITH text overlay so the saved file
        // matches what the user is looking at. renderComposed populates
        // m_composed in place; safe to call multiple times.
        renderComposed();

        // Resolve ~/Pictures via Qt's portable standard paths. Falls
        // back to /tmp if Pictures is somehow unavailable (shouldn't
        // happen on Linux desktop but defensive).
        QString picturesDir = QStandardPaths::writableLocation(
            QStandardPaths::PicturesLocation);
        if (picturesDir.isEmpty()) {
            picturesDir = QStandardPaths::writableLocation(
                QStandardPaths::TempLocation);
        }
        // Ensure the directory exists. PicturesLocation usually does
        // on Linux but a fresh user account may not have it yet.
        QDir().mkpath(picturesDir);

        const QString timestamp = QDateTime::currentDateTime()
            .toString(QStringLiteral("yyyy-MM-dd_HH-mm-ss"));
        const QString filename = QStringLiteral("snapclone_%1.jpg").arg(timestamp);
        const QString fullPath = QDir(picturesDir).filePath(filename);

        // JPEG quality 92 — high quality but not max (max produces
        // bigger files for marginal visual gain). PNG would be lossless
        // but ~5-10x larger; JPEG is the right default for photos.
        const bool ok = m_composed.save(fullPath, "JPG", 92);

        if (ok) {
            // Feedback via existing m_hint label. Show full filename
            // (basename + parent dir) so user knows where it went on
            // first save — they can find it without checking the
            // file manager.
            const QString parentDir = QFileInfo(fullPath).absoluteDir().dirName();
            m_hint->setText(tr("✓ Saved to %1/%2").arg(parentDir, filename));
            m_hint->setStyleSheet(QStringLiteral(
                "color: %1; font-size: 11px; font-weight: 600;")
                .arg(QLatin1String(Theme::SUCCESS())));
            // Reset hint after 4s so it goes back to the
            // "Tap image to reposition text" guidance.
            QPointer<QLabel> hintPtr(m_hint);
            QTimer::singleShot(4000, m_hint, [hintPtr]() {
                if (!hintPtr) return;
                hintPtr->setText(tr("Tap image to reposition text"));
                hintPtr->setStyleSheet(QString());  // back to inherited style
            });
        } else {
            m_hint->setText(tr("⚠ Could not save to %1").arg(fullPath));
            m_hint->setStyleSheet(QStringLiteral(
                "color: %1; font-size: 11px; font-weight: 600;")
                .arg(QLatin1String(Theme::DANGER())));
        }
    });

    // ── Layout assembly ───────────────────────────────────────────────────
    m_root = new QVBoxLayout(this);
    auto *root = m_root;
    root->setContentsMargins(0, 0, 0, 0);
    root->setSpacing(0);

    // Top bar sits over the image
    auto *topWidget = new QWidget(this);
    topWidget->setLayout(topBar);
    topWidget->setStyleSheet(QStringLiteral("background: transparent;"));
    root->addWidget(topWidget);

    root->addStretch(1); // push controls to bottom

    // Bottom controls panel
    auto *bottomPanel = new QWidget(this);
    bottomPanel->setStyleSheet(QStringLiteral(
        "background: %1; border-top: 1px solid %2;")
        .arg(QLatin1String(Theme::BG_CARD()))
        .arg(QLatin1String(Theme::ACCENT_DIM())));
    auto *bottomLayout = new QVBoxLayout(bottomPanel);
    bottomLayout->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_SM,
                                     Theme::SPACE_MD, Theme::SPACE_MD);
    bottomLayout->setSpacing(Theme::SPACE_SM);
    bottomLayout->addWidget(m_hint);
    bottomLayout->addWidget(m_editor);
    bottomLayout->addLayout(sizeRow);
    bottomLayout->addLayout(durationRow);
    // V32.32: [Save] [Send──────] row. Save takes natural width, Send
    // stretches to fill the rest so it stays the visually dominant
    // primary action. Spacing between matches the panel's default
    // SPACE_SM so it doesn't look crammed.
    auto *actionRow = new QHBoxLayout();
    actionRow->setSpacing(Theme::SPACE_SM);
    actionRow->addWidget(m_saveBtn, /*stretch=*/0);
    actionRow->addWidget(m_sendBtn, /*stretch=*/1);
    bottomLayout->addLayout(actionRow);
    root->addWidget(bottomPanel);

    // Keyboard avoidance: when the on-screen keyboard appears it would cover
    // the bottom panel (and the Send button). Lift the panel above it by
    // insetting the root layout bottom margin by the overlap. Uses the shared
    // VirtualKeyboardWatcher (already driving MainWindow).
    connect(&VirtualKeyboardWatcher::instance(),
            &VirtualKeyboardWatcher::visibilityChanged,
            this, [this](bool visible, const QRect &rect) {
        applyKeyboardInset(visible, rect);
    });

    // Set focus to the text editor so the virtual keyboard appears immediately
    // on touchscreen devices when the composer opens. Capture m_editor by
    // value (not this) so if the dialog is destroyed during the 50ms window
    // we don't deref a half-destroyed this. The receiver is m_editor itself
    // — Qt auto-disconnects when m_editor is destroyed, so this is doubly
    // safe.
    QPointer<QWidget> editor = m_editor;
    QTimer::singleShot(50, m_editor, [editor]() {
        if (editor) editor->setFocus(Qt::OtherFocusReason);
    });
}

// ─── Paint ────────────────────────────────────────────────────────────────────

void SnapComposerDialog::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing);
    p.setRenderHint(QPainter::TextAntialiasing);

    // Draw background — scale to fill while keeping aspect ratio
    if (!m_background.isNull()) {
        const QRect r   = rect();
        const QImage bg = m_background.scaled(
            r.size(), Qt::KeepAspectRatioByExpanding, Qt::SmoothTransformation);
        const int ox = (bg.width()  - r.width())  / 2;
        const int oy = (bg.height() - r.height()) / 2;
        p.drawImage(-ox, -oy, bg);
    } else {
        // Text mode — solid black
        p.fillRect(rect(), Qt::black);
    }

    // Draw text preview at the current fractional position
    const QString text = m_editor ? m_editor->toPlainText().trimmed() : QString{};
    if (text.isEmpty()) return;

    QFont font = p.font();
    font.setPixelSize(m_fontSize);
    font.setBold(true);
    p.setFont(font);

    const QFontMetrics fm(font);
    const QRect textBound = fm.boundingRect(
        QRect(0, 0, width() * 3 / 4, height()),
        Qt::AlignCenter | Qt::TextWordWrap, text);

    const int cx = static_cast<int>(m_textPos.x() * width());
    const int cy = static_cast<int>(m_textPos.y() * height());
    const QRect drawRect(cx - textBound.width() / 2,
                         cy - textBound.height() / 2,
                         textBound.width() + 2,
                         textBound.height() + 2);

    // Soft drop shadow for readability on any background
    p.setPen(QColor(0, 0, 0, 160));
    p.drawText(drawRect.adjusted(2, 2, 2, 2),
               Qt::AlignCenter | Qt::TextWordWrap, text);

    // Main text
    p.setPen(m_textColor);
    p.drawText(drawRect, Qt::AlignCenter | Qt::TextWordWrap, text);
}

// ─── Interactions ─────────────────────────────────────────────────────────────

void SnapComposerDialog::applyKeyboardInset(bool visible, const QRect &kbRect)
{
    int inset = 0;
    if (visible && !kbRect.isEmpty()) {
        // kbRect is in global (screen) coordinates — find how much of our
        // bottom edge it covers, and inset the root layout by that amount so
        // the bottom panel (and Send button) ride above the keyboard.
        const QRect dlgGlobal(mapToGlobal(QPoint(0, 0)), size());
        inset = qMax(0, dlgGlobal.bottom() - kbRect.top());
    }
    if (m_root)
        m_root->setContentsMargins(0, 0, 0, inset);
}

void SnapComposerDialog::mousePressEvent(QMouseEvent *e)
{
    // Record a press that lands on the image background (no child widget hit).
    // We defer acting until release so we can tell a tap (reposition text)
    // from an upward swipe (send) — handling both on press would conflict.
    m_pressOnBackground = (childAt(e->position().toPoint()) == nullptr);
    m_pressStart = e->position().toPoint();
    QDialog::mousePressEvent(e);
}

void SnapComposerDialog::mouseReleaseEvent(QMouseEvent *e)
{
    if (m_pressOnBackground) {
        const QPoint end = e->position().toPoint();
        const int dyUp = m_pressStart.y() - end.y();        // positive = upward
        const int dx   = qAbs(end.x() - m_pressStart.x());

        // A deliberate upward swipe on the background sends the snap. Tuned
        // conservatively (sending is significant) and gated to the background,
        // so swiping inside the editor to select text never sends. The keyboard
        // inset already keeps the Send button reachable — this is a shortcut for
        // when the keyboard is up.
        constexpr int kSwipeUpToSendPx    = 80;
        constexpr int kSwipeDxTolerancePx = 70;
        constexpr int kTapTolerancePx     = 12;

        if (dyUp >= kSwipeUpToSendPx && dx <= kSwipeDxTolerancePx) {
            m_pressOnBackground = false;
            if (m_sendBtn)
                m_sendBtn->click();           // identical path to the Send button
            return;
        }
        // Otherwise a tap (negligible motion) repositions the text overlay —
        // the original press behaviour, now gated on it actually being a tap.
        if (qAbs(dyUp) <= kTapTolerancePx && dx <= kTapTolerancePx) {
            m_textPos = {
                end.x() / static_cast<qreal>(width()),
                end.y() / static_cast<qreal>(height())
            };
            m_textPos.setX(qBound(0.15, m_textPos.x(), 0.85));
            m_textPos.setY(qBound(0.1,  m_textPos.y(), 0.6));
            update();
        }
    }
    m_pressOnBackground = false;
    QDialog::mouseReleaseEvent(e);
}

void SnapComposerDialog::keyPressEvent(QKeyEvent *e)
{
    // Don't let Escape close the dialog accidentally — use the ✕ button.
    if (e->key() == Qt::Key_Escape) {
        e->ignore();
        return;
    }
    QDialog::keyPressEvent(e);
}

// ─── Rendering ────────────────────────────────────────────────────────────────

void SnapComposerDialog::renderComposed()
{
    // Produce the final image at the background's native resolution
    // (or 1080×1920 for text-mode snaps — good quality without being huge).
    const QSize outputSize = m_background.isNull()
        ? QSize(1080, 1920)
        : m_background.size();

    m_composed = QImage(outputSize, QImage::Format_RGB32);

    QPainter p(&m_composed);
    p.setRenderHint(QPainter::Antialiasing);
    p.setRenderHint(QPainter::TextAntialiasing);

    // Background
    if (!m_background.isNull()) {
        p.drawImage(QRect(QPoint(0, 0), outputSize), m_background);
    } else {
        p.fillRect(m_composed.rect(), Qt::black);
    }

    const QString text = m_editor ? m_editor->toPlainText().trimmed() : QString{};
    if (text.isEmpty()) return;

    // Scale font to the output resolution proportionally
    const double scale = static_cast<double>(outputSize.height()) / height();
    const int scaledFont = qMax(12, static_cast<int>(m_fontSize * scale));

    QFont font;
    font.setPixelSize(scaledFont);
    font.setBold(true);
    p.setFont(font);

    const QFontMetrics fm(font);
    const QRect textBound = fm.boundingRect(
        QRect(0, 0, outputSize.width() * 3 / 4, outputSize.height()),
        Qt::AlignCenter | Qt::TextWordWrap, text);

    const int cx = static_cast<int>(m_textPos.x() * outputSize.width());
    const int cy = static_cast<int>(m_textPos.y() * outputSize.height());
    const QRect drawRect(cx - textBound.width() / 2,
                         cy - textBound.height() / 2,
                         textBound.width() + 2,
                         textBound.height() + 2);

    // Shadow
    p.setPen(QColor(0, 0, 0, 180));
    p.drawText(drawRect.adjusted(3, 3, 3, 3),
               Qt::AlignCenter | Qt::TextWordWrap, text);

    // Text
    p.setPen(m_textColor);
    p.drawText(drawRect, Qt::AlignCenter | Qt::TextWordWrap, text);
}

// ─── Colour cycle ─────────────────────────────────────────────────────────────

void SnapComposerDialog::cycleColor()
{
    m_colorIndex = (m_colorIndex + 1) % static_cast<int>(kColors.size());
    m_textColor  = kColors[m_colorIndex];

    // Update the button's text colour to preview the current selection
    m_colorBtn->setStyleSheet(QStringLiteral(
        "QToolButton { background: %3;"
        "  border-radius: %1px; font-size: 20px; font-weight: bold;"
        "  color: %2; }"
        "QToolButton:pressed { background: %4; }")
        .arg(Theme::TOUCH_ICON / 2)
        .arg(m_textColor.name())
        .arg(QLatin1String(Theme::BG_DEEP()))
        .arg(QLatin1String(Theme::ACCENT_DIM())));
}

} // namespace SnapClone::UI
