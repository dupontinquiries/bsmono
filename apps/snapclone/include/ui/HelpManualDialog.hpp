#pragma once

#include <QDialog>
#include <QString>

class QTextBrowser;
class QTimer;

namespace SnapClone::UI {

/// A single-page, scrollable markdown manual shown over Settings.
///
/// It mirrors the app's calm, ephemeral feel: the dialog auto-dismisses after
/// a stretch of no interaction, but the idle countdown RESETS whenever the user
/// scrolls (or otherwise interacts), so an active reader is never interrupted.
/// A Close button dismisses it manually as well.
class HelpManualDialog : public QDialog
{
    Q_OBJECT
public:
    explicit HelpManualDialog(QWidget *parent = nullptr);

protected:
    bool eventFilter(QObject *obj, QEvent *event) override;

private:
    /// (Re)start the single-shot idle countdown — called on show and on every
    /// scroll / interaction so reading keeps the manual open.
    void resetIdleTimer();

    QTextBrowser *m_view = nullptr;
    QTimer       *m_idle = nullptr;
};

/// Build the manual's markdown text. Folder paths are resolved at call time
/// from QStandardPaths so the "where your files live" section always shows
/// this device's real locations.
QString helpManualMarkdown();

} // namespace SnapClone::UI
