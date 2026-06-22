// ─── UI/AddLandmarkDialog.cpp ────────────────────────────────────────────────
// V32.31 — see header.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/AddLandmarkDialog.hpp"
#include "ui/Theme.hpp"
#include "core/LandmarkHash.hpp"   // V32.34+: photo fingerprints

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include <QUuid>
#include <QDateTime>
#include <QtMath>

namespace SnapClone::UI {

AddLandmarkDialog::AddLandmarkDialog(const Core::UserId &selfId,
                                      double currentLat, double currentLon,
                                      double currentAccM,
                                      QWidget *parent)
    : MobileDialog(parent),
      m_selfId(selfId),
      m_lat(currentLat),
      m_lon(currentLon),
      m_acc(currentAccM)
{
    m_haveFix = !qIsNaN(currentLat) && !qIsNaN(currentLon);
    setWindowTitle(tr("Add landmark"));
    buildUi();
}

void AddLandmarkDialog::setCapturedFrame(const QImage &frame)
{
    // Store a refcounted copy. QImage uses implicit sharing so this is
    // O(1) — the actual pixel data isn't duplicated unless either side
    // tries to mutate. The frame is held until the dialog closes; we
    // never persist the raw pixels anywhere, only the lossy hashes.
    m_capturedFrame = frame;
    // If the photo badge widget already exists (buildUi has run), show
    // it now. Otherwise buildUi will read m_capturedFrame.isNull() and
    // create the badge in the right state on construction.
    if (m_photoBadge) {
        m_photoBadge->setVisible(!frame.isNull());
    }
}

void AddLandmarkDialog::buildUi()
{
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                              Theme::SPACE_MD, Theme::SPACE_MD);
    root->setSpacing(Theme::SPACE_SM);

    // Header
    auto *title = new QLabel(tr("Add landmark"), this);
    title->setStyleSheet(QStringLiteral(
        "font-size: 18px; font-weight: 600; color: %1; background: transparent;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())));
    root->addWidget(title);

    auto *hint = new QLabel(
        tr("Save your current location with a name. Useful for places "
           "you visit often — when the app sees the same WiFi or surroundings "
           "later, it can derive your location even without GPS."), this);
    hint->setWordWrap(true);
    hint->setStyleSheet(QStringLiteral(
        "font-size: 13px; color: %1; background: transparent;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    root->addWidget(hint);

    // Name field
    auto *nameLabel = new QLabel(tr("Name"), this);
    nameLabel->setStyleSheet(QStringLiteral(
        "font-size: 13px; color: %1; background: transparent;"
        " border: none;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    root->addWidget(nameLabel);
    m_nameEdit = new QLineEdit(this);
    m_nameEdit->setPlaceholderText(tr("e.g. My apartment, Café Vista"));
    m_nameEdit->setMaxLength(64);
    root->addWidget(m_nameEdit);

    // Location display
    auto *locTitle = new QLabel(tr("Location"), this);
    locTitle->setStyleSheet(nameLabel->styleSheet());
    root->addWidget(locTitle);
    m_locLabel = new QLabel(this);
    m_locLabel->setWordWrap(true);
    if (m_haveFix) {
        QString accStr;
        if (m_acc > 0.0) {
            accStr = tr(" (±%1 m)").arg(static_cast<int>(qRound(m_acc)));
        }
        m_locLabel->setText(tr("%1, %2%3")
            .arg(m_lat, 0, 'f', 5)
            .arg(m_lon, 0, 'f', 5)
            .arg(accStr));
        m_locLabel->setStyleSheet(QStringLiteral(
            "font-family: monospace; font-size: 14px; color: %1;"
            " background: %2; padding: 8px; border-radius: %3px;")
            .arg(QLatin1String(Theme::ACCENT()))
            .arg(QLatin1String(Theme::BG_RAISED()))
            .arg(Theme::RADIUS_SM));
    } else {
        m_locLabel->setText(tr("No location fix available.\n\n"
                                "Set a manual location in Settings → Privacy, "
                                "or wait for a GPS fix, then try again."));
        m_locLabel->setStyleSheet(QStringLiteral(
            "font-size: 13px; color: %1; background: %2;"
            " padding: 8px; border-radius: %3px;")
            .arg(QLatin1String(Theme::WARNING()))
            .arg(QLatin1String(Theme::BG_RAISED()))
            .arg(Theme::RADIUS_SM));
    }
    root->addWidget(m_locLabel);

    // V32.34+: photo-attached badge. Shown when setCapturedFrame() has
    // been called (typically via the camera viewfinder's landmark hint).
    // The badge is informational — it tells the user "we'll also store
    // a tiny fingerprint of the view, NOT the photo itself" so they
    // understand what's being captured. Hidden by default if no frame.
    m_photoBadge = new QLabel(
        tr("📷 Photo attached — a small privacy-safe fingerprint will be "
           "stored so we can later recognize this view. The photo itself "
           "is never saved or shared."), this);
    m_photoBadge->setWordWrap(true);
    m_photoBadge->setStyleSheet(QStringLiteral(
        "font-size: 12px; color: %1; background: %2;"
        " padding: 8px; border-radius: %3px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(Theme::RADIUS_SM));
    m_photoBadge->setVisible(!m_capturedFrame.isNull());
    root->addWidget(m_photoBadge);

    root->addStretch(1);

    // Buttons
    auto *btnRow = new QHBoxLayout();
    auto *cancelBtn = new QPushButton(tr("Cancel"), this);
    cancelBtn->setMinimumHeight(Theme::TOUCH_MIN);
    cancelBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background:%1; color:%2; border:1px solid %3;"
        " border-radius:%4px; padding: 8px 16px; }"
        "QPushButton:hover { background:%5; }")
        .arg(QLatin1String(Theme::BG_CARD()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(Theme::RADIUS_SM)
        .arg(QLatin1String(Theme::BG_HOVER())));
    connect(cancelBtn, &QPushButton::clicked, this, &QDialog::reject);
    btnRow->addWidget(cancelBtn);

    m_saveBtn = new QPushButton(tr("Save landmark"), this);
    m_saveBtn->setMinimumHeight(Theme::TOUCH_MIN);
    m_saveBtn->setObjectName(QStringLiteral("accentBtn"));
    m_saveBtn->setEnabled(m_haveFix);
    m_saveBtn->setToolTip(m_haveFix
        ? tr("Save this landmark")
        : tr("Set your location first (Settings → Privacy)"));
    connect(m_saveBtn, &QPushButton::clicked,
            this, &AddLandmarkDialog::onSaveClicked);
    btnRow->addWidget(m_saveBtn);
    root->addLayout(btnRow);

    setLayout(root);
}

void AddLandmarkDialog::onSaveClicked()
{
    if (!m_haveFix) return;
    const QString name = m_nameEdit ? m_nameEdit->text().trimmed() : QString();
    if (name.isEmpty()) {
        m_nameEdit->setFocus();
        m_nameEdit->setPlaceholderText(tr("A name is required"));
        return;
    }

    Core::Landmark lm;
    lm.id = QStringLiteral("sc-landmark-") +
            QUuid::createUuid().toString(QUuid::WithoutBraces);
    lm.creatorId   = m_selfId.value;
    lm.displayName = name;
    lm.lat         = m_lat;
    lm.lon         = m_lon;
    lm.accuracyM   = m_acc;
    // geohash7 left empty in V31 — populated by a future helper when the
    // DHT publish path lands; SQL queries don't depend on it for
    // local-only landmarks (the BETWEEN-on-lat-lon pre-filter in
    // loadLandmarksNear does the spatial work).
    lm.scope       = Core::LandmarkScope::Private;
    lm.createdAtMs = QDateTime::currentMSecsSinceEpoch();
    // V32.34+: photo fingerprints. Computed here so the QImage stays
    // local — Storage only sees the 8-byte blurHash + 4-byte edgeHash.
    // The hashes are LOSSY one-way transforms; an attacker who somehow
    // got the bytes cannot reconstruct anything recognizable about the
    // original view. See core/LandmarkHash.hpp for the math.
    //
    // If no frame was attached (e.g. user opened the dialog from the
    // Contacts page header button rather than the viewfinder hint),
    // both stay empty and the dialog behaves exactly as V31 did.
    if (!m_capturedFrame.isNull()) {
        lm.photoBlurHash = Core::LandmarkHash::blurHash(m_capturedFrame);
        lm.photoEdgeHash = Core::LandmarkHash::edgeHash(m_capturedFrame);
    }
    // ssid / bssidHash / signature remain empty — V31 doesn't capture
    // those yet. Phase 2.

    emit saved(lm);
    accept();
}

} // namespace SnapClone::UI
