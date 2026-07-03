#pragma once

// ─── UI/DiagnosticsDialog.hpp ─────────────────────────────────────────────────
//
// Modal dialog that runs NetworkDiagnostics and displays a colour-coded report
// with per-check status icons and copy-paste firewall fix commands.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/MobileDialog.hpp"
#include "network/NetworkDiagnostics.hpp"

QT_BEGIN_NAMESPACE
class QVBoxLayout;
class QLabel;
class QPushButton;
class QScrollArea;
QT_END_NAMESPACE

namespace SnapClone::UI {

class DiagnosticsDialog : public MobileDialog
{
    Q_OBJECT

public:
    explicit DiagnosticsDialog(quint16 wsPort    = 9876,
                               quint16 gossipPort = 9877,
                               QWidget *parent    = nullptr);

private slots:
    void onCheckCompleted(SnapClone::Network::DiagnosticResult result);
    void onDiagnosisReady(QList<SnapClone::Network::DiagnosticResult> results);
    void runChecks();

private:
    void buildUi();
    void addResultRow(const Network::DiagnosticResult &r);

    Network::NetworkDiagnostics *m_diag      = nullptr;
    QVBoxLayout                 *m_resultsLayout = nullptr;
    QLabel                      *m_statusLabel   = nullptr;
    QPushButton                 *m_runButton     = nullptr;
    quint16                      m_wsPort;
    quint16                      m_gossipPort;
};

} // namespace SnapClone::UI
