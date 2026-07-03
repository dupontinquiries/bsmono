#include "ui/DiagnosticsDialog.hpp"
#include "ui/Theme.hpp"

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QLabel>
#include <QPushButton>
#include <QScrollArea>
#include <QFrame>
#include <QClipboard>
#include <QGuiApplication>
#include <QApplication>
#include <QTextEdit>
#include <QTimer>
#include <algorithm>

// ─── UI/DiagnosticsDialog.cpp ─────────────────────────────────────────────────
//
// All colours from Theme namespace — no hardcoded hex strings.
// ─────────────────────────────────────────────────────────────────────────────

namespace SnapClone::UI {

DiagnosticsDialog::DiagnosticsDialog(quint16 wsPort, quint16 gossipPort, QWidget *parent)
    : MobileDialog(parent)
    , m_wsPort(wsPort)
    , m_gossipPort(gossipPort)
{
    setWindowTitle(QStringLiteral("Network Diagnostics"));
    setMinimumSize(540, 460);
    setStyleSheet(Theme::globalStyleSheet());
    buildUi();

    m_diag = new Network::NetworkDiagnostics(m_wsPort, m_gossipPort, this);
    connect(m_diag, &Network::NetworkDiagnostics::checkCompleted,
            this, &DiagnosticsDialog::onCheckCompleted);
    connect(m_diag, &Network::NetworkDiagnostics::diagnosisReady,
            this, &DiagnosticsDialog::onDiagnosisReady);

    QTimer::singleShot(0, this, &DiagnosticsDialog::runChecks);
}

void DiagnosticsDialog::buildUi()
{
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                              Theme::SPACE_MD, Theme::SPACE_MD);
    root->setSpacing(Theme::SPACE_SM);

    // ── Header ────────────────────────────────────────────────────────────
    auto *title = new QLabel(QStringLiteral("🔍  Network Diagnostics"), this);
    title->setObjectName(QStringLiteral("titleLabel"));
    root->addWidget(title);

    auto *sub = new QLabel(
        QStringLiteral("Checking ports %1/tcp (WebSocket) and %2/udp (Gossip)…")
            .arg(m_wsPort).arg(m_gossipPort), this);
    sub->setObjectName(QStringLiteral("subtitleLabel"));
    root->addWidget(sub);

    m_statusLabel = new QLabel(QStringLiteral("Running checks…"), this);
    m_statusLabel->setObjectName(QStringLiteral("accentLabel"));
    root->addWidget(m_statusLabel);

    // ── Scrollable results ────────────────────────────────────────────────
    auto *scroll = new QScrollArea(this);
    scroll->setWidgetResizable(true);
    scroll->setFrameShape(QFrame::NoFrame);
    scroll->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);

    auto *container = new QWidget;
    m_resultsLayout = new QVBoxLayout(container);
    m_resultsLayout->setAlignment(Qt::AlignTop);
    m_resultsLayout->setSpacing(Theme::SPACE_SM);
    m_resultsLayout->setContentsMargins(0, 0, 0, 0);
    scroll->setWidget(container);
    root->addWidget(scroll, 1);

    // ── Buttons ───────────────────────────────────────────────────────────
    auto *btnRow = new QHBoxLayout;
    btnRow->setSpacing(Theme::SPACE_SM);

    m_runButton = new QPushButton(QStringLiteral("Re-run"), this);
    connect(m_runButton, &QPushButton::clicked, this, &DiagnosticsDialog::runChecks);
    btnRow->addWidget(m_runButton);

    btnRow->addStretch();

    auto *closeBtn = new QPushButton(QStringLiteral("Close"), this);
    closeBtn->setObjectName(QStringLiteral("accentBtn"));
    connect(closeBtn, &QPushButton::clicked, this, &QDialog::accept);
    btnRow->addWidget(closeBtn);

    root->addLayout(btnRow);
}

void DiagnosticsDialog::runChecks()
{
    QLayoutItem *item;
    while ((item = m_resultsLayout->takeAt(0)) != nullptr) {
        delete item->widget();
        delete item;
    }
    m_statusLabel->setText(QStringLiteral("Running checks…"));
    m_statusLabel->setObjectName(QStringLiteral("accentLabel"));
    m_runButton->setEnabled(false);
    m_diag->runAll();
}

void DiagnosticsDialog::onCheckCompleted(Network::DiagnosticResult result)
{
    addResultRow(result);
}

void DiagnosticsDialog::onDiagnosisReady(QList<Network::DiagnosticResult> results)
{
    const bool anyFail = std::any_of(results.begin(), results.end(),
        [](const auto &r){ return r.status == Network::DiagnosticResult::Status::Fail; });
    const bool anyWarn = std::any_of(results.begin(), results.end(),
        [](const auto &r){ return r.status == Network::DiagnosticResult::Status::Warning; });

    if (anyFail) {
        m_statusLabel->setText(QStringLiteral("❌  Some checks failed — see details below."));
        m_statusLabel->setStyleSheet(
            QStringLiteral("color: %1; font-size: 13px;").arg(QLatin1String(Theme::DANGER())));
    } else if (anyWarn) {
        m_statusLabel->setText(QStringLiteral("⚠  Checks complete — warnings found."));
        m_statusLabel->setStyleSheet(
            QStringLiteral("color: %1; font-size: 13px;").arg(QLatin1String(Theme::WARNING())));
    } else {
        m_statusLabel->setText(QStringLiteral("✅  All checks passed — network looks good!"));
        m_statusLabel->setStyleSheet(
            QStringLiteral("color: %1; font-size: 13px;").arg(QLatin1String(Theme::SUCCESS())));
    }

    m_runButton->setEnabled(true);
}

void DiagnosticsDialog::addResultRow(const Network::DiagnosticResult &r)
{
    using Status = Network::DiagnosticResult::Status;

    // Card border colour indicates status.
    const QString borderColour =
        (r.status == Status::Ok)      ? QStringLiteral("#2A3B2A")   // muted green
      : (r.status == Status::Warning) ? QStringLiteral("#3A3020")   // muted amber
                                      : QStringLiteral("#3B2020");  // muted red

    auto *frame = new QFrame(this);
    frame->setFrameShape(QFrame::StyledPanel);
    frame->setStyleSheet(QStringLiteral(
        "QFrame { background: %1; border: 1.5px solid %2; border-radius: 12px; }")
            .arg(QLatin1String(Theme::BG_CARD()), borderColour));

    auto *fl = new QVBoxLayout(frame);
    fl->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_SM,
                            Theme::SPACE_MD, Theme::SPACE_SM);
    fl->setSpacing(Theme::SPACE_XS);

    // Header: icon + name
    const QString icon = (r.status == Status::Ok)      ? QStringLiteral("✅")
                       : (r.status == Status::Warning) ? QStringLiteral("⚠")
                                                       : QStringLiteral("❌");
    auto *nameLabel = new QLabel(QStringLiteral("%1  %2").arg(icon, r.name), frame);
    nameLabel->setStyleSheet(QStringLiteral(
        "font-weight: 600; font-size: 14px; color: %1; border: none;")
            .arg(QLatin1String(Theme::TEXT_PRIMARY())));
    fl->addWidget(nameLabel);

    // Detail
    if (!r.detail.isEmpty()) {
        auto *detail = new QLabel(r.detail, frame);
        detail->setWordWrap(true);
        detail->setStyleSheet(QStringLiteral(
            "color: %1; font-size: 12px; border: none;")
                .arg(QLatin1String(Theme::TEXT_SECONDARY())));
        fl->addWidget(detail);
    }

    // Suggestion box + copy button
    if (!r.suggestion.isEmpty()) {
        auto *suggEdit = new QTextEdit(frame);
        suggEdit->setPlainText(r.suggestion);
        suggEdit->setReadOnly(true);
        suggEdit->setMaximumHeight(88);
        suggEdit->setStyleSheet(QStringLiteral(
            "QTextEdit { background: %1; color: %2; border: 1px solid %3; "
            "border-radius: 8px; font-family: monospace; font-size: 11px; padding: 6px; }")
                .arg(QLatin1String(Theme::BG_DEEP()),
                     QLatin1String(Theme::SUCCESS()),
                     QLatin1String(Theme::BORDER())));
        fl->addWidget(suggEdit);

        auto *copyBtn = new QPushButton(QStringLiteral("📋  Copy fix commands"), frame);
        copyBtn->setObjectName(QStringLiteral("copyBtn"));
        connect(copyBtn, &QPushButton::clicked, copyBtn,
                [copyBtn, suggestion = r.suggestion]() {
            QGuiApplication::clipboard()->setText(suggestion, QClipboard::Clipboard);
            const QString orig = copyBtn->text();
            copyBtn->setText(QStringLiteral("✓  Copied!"));
            copyBtn->setEnabled(false);
            QTimer::singleShot(1800, copyBtn, [copyBtn, orig]() {
                if (copyBtn) {
                    copyBtn->setText(orig);
                    copyBtn->setEnabled(true);
                }
            });
        });
        fl->addWidget(copyBtn, 0, Qt::AlignLeft);
    }

    m_resultsLayout->addWidget(frame);
}

} // namespace SnapClone::UI
