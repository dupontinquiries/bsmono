// ─── UI/ConversationDetailDialog.cpp ─────────────────────────────────────────
//
// See header for design philosophy.
//
// Implementation notes:
//   • Each snap renders as a small "card" widget — rounded rect, color
//     depending on direction, single-line label inside. The whole card
//     intercepts mouse-press to emit snapTapped(); we don't use any
//     fancy delegate model, just stacked QWidgets in a QVBoxLayout.
//     The thread is short enough (≤ 1000 snaps from
//     StorageManager::loadConversationSnaps) that this is acceptable.
//   • If the thread grew large enough that this became slow (~5000+
//     snaps), the right move would be a QListView with a custom
//     delegate. Filed in comments not TODO — premature for now.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/ConversationDetailDialog.hpp"
#include "ui/Theme.hpp"

#include <QLabel>
#include <QScrollArea>
#include <QScrollBar>
#include <QVBoxLayout>
// V32.39+: group-voice surface
#include "ui/ConversationSecurityBar.hpp"
#include "ui/GroupVoiceRoster.hpp"
#include "ui/GroupVoiceBar.hpp"
#include "ui/GroupVoiceCoordinator.hpp"
#include "ui/VoiceNetworkBridge.hpp"
#include "core/VoiceNoteRecorder.hpp"
#include "core/VoiceNotePlayer.hpp"
#include <QShowEvent>
#include <QHideEvent>
#include <QHBoxLayout>
#include <QPushButton>
#include <QLineEdit>
#include <QTimer>
#include <QColor>
#include <cmath>
#include <QFrame>
#include <QMouseEvent>
#include <QDateTime>
#include <QSettings>
#include <QLoggingCategory>

Q_LOGGING_CATEGORY(lcConvDet, "snapclone.ui.conv")

namespace SnapClone::UI {

namespace {

QString relativeTime(const QDateTime &dt)
{
    const qint64 secs = dt.secsTo(QDateTime::currentDateTimeUtc());
    if (secs < 60)    return QObject::tr("just now");
    if (secs < 3600)  return QObject::tr("%1m ago").arg(secs / 60);
    if (secs < 86400) return QObject::tr("%1h ago").arg(secs / 3600);
    return QObject::tr("%1d ago").arg(secs / 86400);
}

QString mediaGlyph(Core::MediaType m)
{
    switch (m) {
    case Core::MediaType::Image:    return QStringLiteral("📷");
    case Core::MediaType::Video:    return QStringLiteral("🎥");
    case Core::MediaType::Link:     return QStringLiteral("🔗");
    case Core::MediaType::Location: return QStringLiteral("📍");
    case Core::MediaType::Text:     return QStringLiteral("💬");
    case Core::MediaType::Audio:    return QStringLiteral("🎤");
    }
    return QStringLiteral("📩");
}

QString mediaLabel(Core::MediaType m)
{
    switch (m) {
    case Core::MediaType::Image:    return QObject::tr("Photo");
    case Core::MediaType::Video:    return QObject::tr("Video");
    case Core::MediaType::Link:     return QObject::tr("Link");
    case Core::MediaType::Location: return QObject::tr("Location");
    case Core::MediaType::Text:     return QObject::tr("Text");
    case Core::MediaType::Audio:    return QObject::tr("Voice");
    }
    return QObject::tr("Snap");
}

// ─── Tappable card widget ────────────────────────────────────────────────────
//
// QFrame that swallows the mouse-press and emits snapTapped via a callback.
// We don't subclass into a full Q_OBJECT because the dialog owns the card-
// to-snapId mapping and we need only one signal per dialog, not per card.
// Storing the snap id in a dynamic property + a single event filter would
// also work but the explicit subclass reads cleaner here.

class SnapCard : public QFrame
{
public:
    SnapCard(const Core::SnapId &id,
             std::function<void(Core::SnapId)> onTap,
             QWidget *parent)
        : QFrame(parent), m_id(id), m_onTap(std::move(onTap))
    {
        setCursor(Qt::PointingHandCursor);
    }
protected:
    void mousePressEvent(QMouseEvent *e) override {
        if (e->button() == Qt::LeftButton && m_onTap) m_onTap(m_id);
        QFrame::mousePressEvent(e);
    }
private:
    Core::SnapId                       m_id;
    std::function<void(Core::SnapId)>  m_onTap;
};

} // anon

ConversationDetailDialog::ConversationDetailDialog(
    const Core::UserId &selfId,
    const Core::UserId &otherParty,
    const QString      &otherDisplayName,
    const QList<Core::Snap> &snaps,
    QWidget *parent)
    : MobileDialog(parent)
    , m_selfId(selfId)
    , m_otherParty(otherParty)
    , m_otherDisplayName(otherDisplayName)
    , m_snaps(snaps)
{
    buildUi();
}

void ConversationDetailDialog::buildUi()
{
    setWindowTitle(m_otherDisplayName);
    // Mobile-friendly default sizing; MobileDialog auto-adjusts for
    // bottom-sheet mode on phones.
    resize(420, 600);

    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(0, 0, 0, 0);
    root->setSpacing(0);

    // ── Header bar ────────────────────────────────────────────────────────
    // V32.26: header now has TWO rows — a small drag-handle pill on top,
    // and the title/close row below. The pill is a visual cue that this
    // sheet is draggable. The entire header (pill row + title row, minus
    // the Close button which is a child interactive widget) is wired up
    // as the drag-down-to-close target via eventFilter on m_header.
    m_header = new QFrame(this);
    m_header->setStyleSheet(QStringLiteral(
        "background: %1; border-bottom: 1px solid %2;")
        .arg(QLatin1String(Theme::BG_RAISED()),
             QLatin1String(Theme::BORDER())));
    auto *headerVLay = new QVBoxLayout(m_header);
    headerVLay->setContentsMargins(0, 6, 0, 0);
    headerVLay->setSpacing(0);

    // Drag-handle pill: a narrow rounded rectangle centred at the top
    // of the header. Pure visual affordance — the gesture works on any
    // header area, not just the pill itself.
    {
        auto *pillRow = new QHBoxLayout();
        pillRow->setContentsMargins(0, 0, 0, 4);
        pillRow->addStretch(1);
        auto *pill = new QFrame(m_header);
        pill->setFixedSize(48, 4);
        pill->setStyleSheet(QStringLiteral(
            "background: %1; border-radius: 2px;")
            .arg(QLatin1String(Theme::TEXT_DISABLED())));
        pillRow->addWidget(pill);
        pillRow->addStretch(1);
        headerVLay->addLayout(pillRow);
    }

    // Title + Close button row (the original "header" content).
    auto *headerRow = new QWidget(m_header);
    headerRow->setStyleSheet(QStringLiteral("background: transparent;"));
    auto *headerLay = new QHBoxLayout(headerRow);
    headerLay->setContentsMargins(12, 6, 12, 10);

    auto *title = new QLabel(m_otherDisplayName, headerRow);
    title->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 16px; font-weight: 600;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())));
    headerLay->addWidget(title, 1);

    // V32.39+: voice-call toggle. Checkable; drives setVoiceCallActive, which
    // joins/leaves the coordinator and reveals/hides the voice surface. Sits
    // left of Close.
    m_callBtn = new QPushButton(tr("Call"), headerRow);
    m_callBtn->setCheckable(true);
    m_callBtn->setFlat(true);
    m_callBtn->setStyleSheet(QStringLiteral(
        "QPushButton { color: %1; padding: 4px 12px; }"
        "QPushButton:hover { color: %2; }"
        "QPushButton:checked { color: %2; font-weight: 600; }")
        .arg(QLatin1String(Theme::TEXT_SECONDARY()),
             QLatin1String(Theme::ACCENT())));
    connect(m_callBtn, &QPushButton::toggled,
            this, &ConversationDetailDialog::setVoiceCallActive);
    headerLay->addWidget(m_callBtn);

    auto *closeBtn = new QPushButton(tr("Close"), headerRow);
    closeBtn->setFlat(true);
    closeBtn->setStyleSheet(QStringLiteral(
        "QPushButton { color: %1; padding: 4px 12px; }"
        "QPushButton:hover { color: %2; }")
        .arg(QLatin1String(Theme::TEXT_SECONDARY()),
             QLatin1String(Theme::ACCENT())));
    connect(closeBtn, &QPushButton::clicked, this, &QDialog::reject);
    headerLay->addWidget(closeBtn);

    headerVLay->addWidget(headerRow);

    root->addWidget(m_header);

    // V32.39+: conversation security-tier banner, directly under the header.
    // Defaults to Standard (end-to-end encrypted); the coordinator drives it to
    // Securing/Private as the user requests private mode and the crypto layer
    // negotiates.
    m_securityBar = new ConversationSecurityBar(this);
    root->addWidget(m_securityBar);

    // Install event filter on the header to enable drag-down-to-close.
    // We filter on m_header (the outer frame) rather than installing on
    // the Close button or the title — those are interactive children
    // and shouldn't capture the drag. Mouse-press inside the close button
    // is naturally consumed by the button's own handler before our filter
    // sees it on the header.
    m_header->installEventFilter(this);

    // ── Scrollable thread ─────────────────────────────────────────────────
    m_scroll = new QScrollArea(this);
    m_scroll->setWidgetResizable(true);
    m_scroll->setFrameShape(QFrame::NoFrame);
    m_scroll->setStyleSheet(QStringLiteral("background: %1;")
        .arg(QLatin1String(Theme::BG_BASE())));

    auto *threadHost = new QWidget(m_scroll);
    threadHost->setStyleSheet(QStringLiteral("background: transparent;"));
    m_threadLayout = new QVBoxLayout(threadHost);
    m_threadLayout->setContentsMargins(12, 12, 12, 12);
    m_threadLayout->setSpacing(8);

    if (m_snaps.isEmpty()) {
        auto *empty = new QLabel(
            tr("No snaps in this conversation yet."), threadHost);
        empty->setStyleSheet(QStringLiteral(
            "color: %1; font-style: italic; padding: 24px;")
            .arg(QLatin1String(Theme::TEXT_SECONDARY())));
        empty->setAlignment(Qt::AlignCenter);
        m_threadLayout->addWidget(empty);
    } else {
        for (const auto &snap : m_snaps) appendSnapCard(snap);
    }

    m_threadLayout->addStretch(1);
    m_scroll->setWidget(threadHost);
    root->addWidget(m_scroll, 1);

    // ── V32.39+: group-voice surface beneath the thread ──────────────────────
    // Roster (who's in the call / who's speaking) above the push-to-talk bar.
    m_voiceRoster = new GroupVoiceRoster(this);
    m_voiceBar    = new GroupVoiceBar(this);
    // Hidden until the user starts a call (setVoiceCallActive). The security
    // banner above stays visible as a passive tier indicator.
    m_voiceRoster->setVisible(false);
    m_voiceBar->setVisible(false);
    root->addWidget(m_voiceRoster);

    // V32.43+: bottom-left "+" to pull more people into the live call. It rides
    // in a row with the push-to-talk bar; the lock/security indicator is the
    // banner up top, so this corner is free. Hidden with the bar until a call
    // starts (see setVoiceCallActive). Tapping it asks the owning InboxWidget to
    // open the people picker — the dialog itself doesn't hold the contact list.
    m_addPeopleButton = new QPushButton(QStringLiteral("+"), this);
    m_addPeopleButton->setObjectName(QStringLiteral("addPeopleBtn"));
    m_addPeopleButton->setFixedSize(40, 40);
    m_addPeopleButton->setToolTip(tr("Add people to the call"));
    m_addPeopleButton->setVisible(false);
    m_addPeopleButton->setStyleSheet(QStringLiteral(
        "QPushButton { background: %1; color: %2; border: 1px solid %3;"
        "  border-radius: 20px; font-size: 22px; font-weight: bold; }"
        "QPushButton:pressed { background: %3; }")
        .arg(QLatin1String(Theme::BG_RAISED()),
             QLatin1String(Theme::TEXT_PRIMARY()),
             QLatin1String(Theme::BORDER())));
    connect(m_addPeopleButton, &QPushButton::clicked,
            this, &ConversationDetailDialog::addPeopleRequested);

    auto *voiceBarRow = new QHBoxLayout();
    voiceBarRow->setContentsMargins(0, 0, 0, 0);
    voiceBarRow->setSpacing(8);
    voiceBarRow->addWidget(m_addPeopleButton, 0, Qt::AlignBottom);
    voiceBarRow->addWidget(m_voiceBar, 1);
    root->addLayout(voiceBarRow);

    // V32.45+: ephemeral text composer — always at the very bottom (the voice
    // controls, when a call is live, sit above it). Sends a Text snap that
    // disappears after kTextTtlSecs via the normal retention/purge path.
    constexpr qint64 kTextTtlSecs = 24 * 60 * 60;   // 24h; tune for shorter ephemerality
    auto *composerRow = new QHBoxLayout();
    composerRow->setContentsMargins(8, 6, 8, 8);
    composerRow->setSpacing(8);
    m_composer = new QLineEdit(this);
    m_composer->setPlaceholderText(tr("Message — disappears after a while"));
    m_composer->setStyleSheet(QStringLiteral(
        "QLineEdit { background: %1; color: %2; border: 1px solid %3;"
        "  border-radius: 18px; padding: 8px 14px; font-size: 14px; }")
        .arg(QLatin1String(Theme::BG_RAISED()),
             QLatin1String(Theme::TEXT_PRIMARY()),
             QLatin1String(Theme::BORDER())));
    auto *sendBtn = new QPushButton(tr("Send"), this);
    sendBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background: %1; color: %2; border: none;"
        "  border-radius: 18px; padding: 8px 18px; font-weight: 600; }"
        "QPushButton:pressed { background: %3; }")
        .arg(QLatin1String(Theme::ACCENT()),
             QLatin1String(Theme::TEXT_INVERSE()),
             QLatin1String(Theme::ACCENT_DIM())));
    composerRow->addWidget(m_composer, 1);

    // V32.46+: voice-note mic. Tap to start, tap to send (checkable toggle).
    auto *micBtn = new QPushButton(QStringLiteral("\U0001F3A4"), this);   // 🎤
    micBtn->setFixedSize(40, 36);
    micBtn->setCheckable(true);
    micBtn->setToolTip(tr("Record a voice note — tap to start, tap again to send"));
    micBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background: %1; color: %2; border: none; border-radius: 8px;"
        " font-size: 16px; }"
        "QPushButton:checked { background: %3; color: %4; }")
        .arg(QLatin1String(Theme::BG_HOVER()), QLatin1String(Theme::TEXT_PRIMARY()),
             QLatin1String(Theme::ACCENT()),   QLatin1String(Theme::TEXT_INVERSE())));
    // V32.48+: discard-recording button, shown only while recording.
    auto *recCancel = new QPushButton(QStringLiteral("\u2715"), this);   // ✕
    recCancel->setFixedSize(40, 36);
    recCancel->setVisible(false);
    recCancel->setToolTip(tr("Discard voice note"));
    recCancel->setStyleSheet(QStringLiteral(
        "QPushButton { background: %1; color: %2; border: none; border-radius: 8px;"
        " font-size: 15px; }")
        .arg(QLatin1String(Theme::BG_HOVER()), QLatin1String(Theme::TEXT_SECONDARY())));

    composerRow->addWidget(recCancel);
    composerRow->addWidget(micBtn);
    composerRow->addWidget(sendBtn);
    root->addLayout(composerRow);

    // Live recording duration shown in the (read-only) composer placeholder.
    auto *recTimer = new QTimer(this);
    recTimer->setInterval(200);
    connect(recTimer, &QTimer::timeout, this, [this]() {
        if (!m_recorder) return;
        const int ms = m_recorder->frameCount() * 20;
        m_composer->setPlaceholderText(
            tr("\u25CF  Recording  %1:%2")
                .arg(ms / 60000)
                .arg((ms / 1000) % 60, 2, 10, QLatin1Char('0')));
    });
    const QString idlePlaceholder = m_composer->placeholderText();

    // Restore the composer out of recording mode (shared by send + discard).
    auto endRecordingUi = [this, micBtn, recCancel, recTimer, idlePlaceholder]() {
        recTimer->stop();
        recCancel->setVisible(false);
        m_composer->setReadOnly(false);
        m_composer->setPlaceholderText(idlePlaceholder);
        micBtn->setText(QStringLiteral("\U0001F3A4"));   // 🎤
    };

    auto sendText = [this]() {
        const QString text = m_composer->text().trimmed();
        if (text.isEmpty()) return;
        Core::Snap snap;
        snap.id            = Core::SnapId::generate();
        snap.senderId      = m_selfId;
        snap.recipientId   = m_otherParty;
        snap.mediaType     = Core::MediaType::Text;
        snap.caption       = text;
        snap.encryptedBlob = text.toUtf8();   // MediaType::Text: the bytes carry the message
        snap.sentAt        = QDateTime::currentDateTimeUtc();
        snap.expiresAt     = snap.sentAt.addSecs(kTextTtlSecs);  // per-type TTL → auto-wipe
        appendSnapCard(snap);                 // optimistic: show it immediately
        if (auto *bar = m_scroll->verticalScrollBar())
            bar->setValue(bar->maximum());
        emit sendSnapRequested(snap);
        m_composer->clear();
    };
    connect(sendBtn, &QPushButton::clicked, this, sendText);
    connect(m_composer, &QLineEdit::returnPressed, this, sendText);

    // Voice-note record toggle: first tap starts capture, second tap stops and
    // sends the encoded note as an ephemeral MediaType::Audio snap (same send
    // path as text). Inert when the build has no voice codec (empty blob).
    connect(micBtn, &QPushButton::toggled, this,
            [this, micBtn, recCancel, recTimer, endRecordingUi](bool on) {
        if (on) {
            if (!m_recorder) m_recorder = new Core::VoiceNoteRecorder(this);
            m_recorder->start();
            m_composer->clear();
            m_composer->setReadOnly(true);
            recCancel->setVisible(true);
            recTimer->start();
            micBtn->setText(QStringLiteral("\u25A0"));   // ■ (stop/send)
            return;
        }
        endRecordingUi();
        if (!m_recorder) return;
        int dur = 0;
        const QByteArray blob = m_recorder->stop(&dur);
        if (blob.isEmpty()) return;   // codec off, or nothing captured
        Core::Snap snap;
        snap.id            = Core::SnapId::generate();
        snap.senderId      = m_selfId;
        snap.recipientId   = m_otherParty;
        snap.mediaType     = Core::MediaType::Audio;
        snap.durationSecs  = dur;
        snap.encryptedBlob = blob;                       // Opus frames (self-delimited)
        snap.sentAt        = QDateTime::currentDateTimeUtc();
        snap.expiresAt     = snap.sentAt.addSecs(kTextTtlSecs);
        appendSnapCard(snap);                            // optimistic
        if (auto *bar = m_scroll->verticalScrollBar())
            bar->setValue(bar->maximum());
        emit sendSnapRequested(snap);
    });

    // Discard: stop capture without building/sending, and reset the mic toggle
    // silently (blockSignals so the off-branch's send path doesn't run).
    connect(recCancel, &QPushButton::clicked, this,
            [this, micBtn, endRecordingUi]() {
        if (m_recorder) m_recorder->stop(nullptr);   // drop the recording
        endRecordingUi();
        micBtn->blockSignals(true);
        micBtn->setChecked(false);
        micBtn->blockSignals(false);
    });

    // The coordinator owns the backend (floor session / mic / encoder / play-out)
    // and wires it to the three views above in one place. Participants for this
    // 1:1 thread are self + the other party; the same coordinator scales to N
    // for a future group screen. Its transport seams (voicePacketToSend,
    // floorAssertToSend, floorReleaseToSend, privateModeRequested) are exposed
    // for the network layer to bridge — until that's connected, the local floor,
    // on-air, mic-level and security-tier UI are fully live for testing.
    m_voiceCoord = new GroupVoiceCoordinator(m_selfId.value, this);
    m_voiceCoord->setViews(m_voiceBar, m_voiceRoster, m_securityBar);

    // Bridge the coordinator's transport seams onto the network's per-peer pipe
    // via the VoiceWire envelope. Peers are filled by applyParticipantsToVoice()
    // below; the bridge fans outbound voice to every peer and accepts inbound
    // from any of them. The bridge's outbound packets surface as
    // sendVoicePayload(); the owner (InboxWidget/AppController) connects that to
    // NetworkManager::sendToPeer and forwards inbound payloads to
    // deliverNetworkPayload().
    m_voiceBridge = new VoiceNetworkBridge(m_voiceCoord, {}, this);
    connect(m_voiceBridge, &VoiceNetworkBridge::sendPayload,
            this, &ConversationDetailDialog::sendVoicePayload);

    // Default participants for a 1:1 thread = the other party. The same
    // coordinator/bridge scale to N; setCallParticipants / addCallParticipant
    // replace or extend this set to form a group call.
    if (m_participants.isEmpty())
        m_participants = { { m_otherParty, m_otherDisplayName } };
    applyParticipantsToVoice();

    // No voice backend compiled in → hide the call control entirely (the
    // security banner stays as a passive indicator). Keeps this dialog free of
    // feature #ifdefs; the coordinator reports its own availability.
    if (m_callBtn) m_callBtn->setVisible(m_voiceCoord->isAvailable());

    // Auto-scroll to the bottom (most recent) after the layout settles —
    // mirrors typical chat UX. Use a queued single-shot to land after
    // the scroll area has measured its content height.
    QMetaObject::invokeMethod(this, [this]() {
        if (auto *bar = m_scroll->verticalScrollBar()) {
            bar->setValue(bar->maximum());
        }
    }, Qt::QueuedConnection);
}

void ConversationDetailDialog::setVoiceCallActive(bool on)
{
    // Idle is legitimate during a call (you listen/talk via the PTT button,
    // which is a child widget so its taps don't reset the dialog's idle timer):
    // widen the auto-dismiss window while in-call, and restore the normal window
    // when it ends. Inbound voice frames also poke the timer (see
    // deliverNetworkPayload). With the "enable long calls" advanced setting on,
    // use the effectively-unbounded window so a long silent call is never
    // auto-dismissed (the user exits via End/Close).
    if (on) {
        QSettings s;
        const bool longCalls =
            s.value(QStringLiteral("calls/enableLongCalls"), false).toBool();
        setDismissContext(longCalls ? DismissContext::PhoneCallLong
                                     : DismissContext::PhoneCall);
    } else {
        setDismissContext(DismissContext::Normal);
    }

    // Keep the header button in sync without re-entering this slot.
    if (m_callBtn) {
        const bool was = m_callBtn->blockSignals(true);
        m_callBtn->setChecked(on);
        m_callBtn->setText(on ? tr("End") : tr("Call"));
        m_callBtn->blockSignals(was);
    }
    if (m_voiceRoster) m_voiceRoster->setVisible(on);
    if (m_voiceBar)    m_voiceBar->setVisible(on);
    if (m_addPeopleButton) m_addPeopleButton->setVisible(on);
    if (!m_voiceCoord) return;
    if (on && !m_voiceCoord->inCall())       m_voiceCoord->joinCall();
    else if (!on && m_voiceCoord->inCall())  m_voiceCoord->leaveCall();
}

void ConversationDetailDialog::startVoiceCall()
{
    if (m_voiceCoord && m_voiceCoord->isAvailable() && !m_voiceCoord->inCall())
        setVoiceCallActive(true);
}

void ConversationDetailDialog::setCallGated(bool gated)
{
    // Only meaningful while a call is live; otherwise leave the user's chosen
    // talk mode alone.
    if (m_voiceCoord && m_voiceCoord->inCall())
        m_voiceCoord->setTalkMode(gated ? Core::TalkMode::VoiceActivated
                                        : Core::TalkMode::PushToTalk);
}

void ConversationDetailDialog::applyParticipantsToVoice()
{
    if (!m_voiceCoord) return;
    QList<GroupVoiceCoordinator::Participant> roster;
    roster.append({ m_selfId.value, tr("You"), true });
    QList<Core::UserId> peers;
    peers.reserve(m_participants.size());
    for (const auto &m : m_participants) {
        roster.append({ m.id.value, m.name, false });
        peers.append(m.id);
    }
    m_voiceCoord->setParticipants(roster);
    if (m_voiceBridge) m_voiceBridge->setPeers(peers);
}

void ConversationDetailDialog::setCallParticipants(const QList<CallParticipant> &others)
{
    m_participants = others;
    applyParticipantsToVoice();
}

void ConversationDetailDialog::addCallParticipant(const Core::UserId &id, const QString &name)
{
    if (id.value.isEmpty() || id.value == m_selfId.value) return;
    for (const auto &m : m_participants)
        if (m.id.value == id.value) return;   // already in the call
    m_participants.append({ id, name });
    applyParticipantsToVoice();
}

bool ConversationDetailDialog::isCallPeer(const Core::UserId &id) const
{
    for (const auto &m : m_participants)
        if (m.id.value == id.value) return true;
    return false;
}

bool ConversationDetailDialog::isCallActive() const
{
    return m_voiceCoord && m_voiceCoord->inCall();
}

void ConversationDetailDialog::deliverNetworkPayload(const Core::UserId &sender,
                                                     const QByteArray &payload)
{
    // Route voice/floor packets into the live call; non-voice payloads are
    // ignored by the bridge (returns false), so forwarding everything is safe.
    if (m_voiceBridge) m_voiceBridge->onPayloadFromPeer(sender, payload);
    // Inbound traffic during a call counts as activity — keep the dialog from
    // auto-dismissing while the peer is talking and we're not touching the screen.
    pokeAutoDismiss();
}

void ConversationDetailDialog::primeVoicePreroll(const Core::UserId &sender,
                                                 const QList<QByteArray> &payloads)
{
    // Ordered pre-roll: the receiver plays these ahead of the live jitter, so
    // the head of an accepted message isn't lost to the jitter's drop-oldest cap.
    if (m_voiceBridge) m_voiceBridge->flushPreroll(sender, payloads);
}

void ConversationDetailDialog::hideEvent(QHideEvent *e)
{
    // Leaving the screen ends an in-progress call (releases the floor + closes
    // the mic). No-op if no call is active.
    if (m_voiceCoord && m_voiceCoord->inCall()) setVoiceCallActive(false);
    MobileDialog::hideEvent(e);
}

namespace {
// WCAG-style relative luminance + contrast ratio, used to keep chat-bubble text
// legible in every theme regardless of how the accent/card tokens are set.
double srgbLuminance(const QColor &c)
{
    auto lin = [](int v) {
        const double s = v / 255.0;
        return s <= 0.03928 ? s / 12.92 : std::pow((s + 0.055) / 1.055, 2.4);
    };
    return 0.2126 * lin(c.red()) + 0.7152 * lin(c.green()) + 0.0722 * lin(c.blue());
}
double contrastRatio(const QColor &a, const QColor &b)
{
    const double la = srgbLuminance(a), lb = srgbLuminance(b);
    const double hi = std::max(la, lb), lo = std::min(la, lb);
    return (hi + 0.05) / (lo + 0.05);
}
// Pick whichever palette text token actually contrasts with this bubble's bg.
QString contrastingText(const QString &bgHex)
{
    const QColor bg(bgHex);
    const QColor primary(QString::fromLatin1(Theme::TEXT_PRIMARY()));
    const QColor inverse(QString::fromLatin1(Theme::TEXT_INVERSE()));
    return contrastRatio(bg, primary) >= contrastRatio(bg, inverse)
        ? QString::fromLatin1(Theme::TEXT_PRIMARY())
        : QString::fromLatin1(Theme::TEXT_INVERSE());
}
} // namespace

void ConversationDetailDialog::appendSnapCard(const Core::Snap &snap)
{
    const bool outbound = (snap.senderId.value == m_selfId.value);

    auto *row = new QWidget(m_scroll->widget());
    row->setStyleSheet(QStringLiteral("background: transparent;"));
    auto *rowLay = new QHBoxLayout(row);
    rowLay->setContentsMargins(0, 0, 0, 0);

    auto *card = new SnapCard(
        snap.id,
        [this](Core::SnapId id) { emit snapTapped(id); },
        row);
    // Card visual styling: outbound = accent bg, right-aligned;
    // inbound = raised bg, left-aligned. Border-radius gives the
    // familiar chat-bubble appearance even though the content is
    // a single-line snap descriptor.
    const QString bg = outbound
        ? QLatin1String(Theme::ACCENT_SOFT())
        : QLatin1String(Theme::BG_CARD());
    // Text colour is chosen per-bubble for legible contrast against its own
    // background, so neither outbound (accent) nor inbound (card) washes out in
    // any theme.
    const QString fg = contrastingText(bg);
    card->setStyleSheet(QStringLiteral(
        "background: %1; color: %2;"
        "border-radius: 14px; padding: 10px 14px;")
        .arg(bg, fg));
    card->setMaximumWidth(280);

    auto *cardLay = new QVBoxLayout(card);
    cardLay->setContentsMargins(0, 0, 0, 0);
    cardLay->setSpacing(2);

    // Primary line. Text snaps render the full message as a wrapped bubble;
    // media snaps show a glyph + label + a short caption preview.
    const bool isText  = (snap.mediaType == Core::MediaType::Text);
    const bool isAudio = (snap.mediaType == Core::MediaType::Audio);
    if (isAudio) {
        // Voice note: a play control + duration. Tapping enqueues it for
        // strictly sequential (non-overlapping) playback via VoiceNotePlayer.
        auto *playBtn = new QPushButton(
            tr("\u25B6  Voice  \u00B7  %1s").arg(qMax(1, snap.durationSecs)), card);
        playBtn->setCursor(Qt::PointingHandCursor);
        playBtn->setStyleSheet(QStringLiteral(
            "QPushButton { color: %1; font-size: 13px; background: transparent;"
            " border: none; text-align: left; padding: 0; }").arg(fg));
        const QByteArray blob = snap.encryptedBlob;
        const QString    path = snap.localPath;
        const QString    tag  = snap.id.value;
        connect(playBtn, &QPushButton::clicked, this, [blob, path, tag]() {
            Core::VoiceNotePlayer::instance().enqueue(
                Core::VoiceNotePlayer::resolveBytes(blob, path), tag);
        });
        cardLay->addWidget(playBtn);
    } else {
    QString primary;
    if (isText) {
        primary = snap.caption.isEmpty()
            ? QString::fromUtf8(snap.encryptedBlob) : snap.caption;
    } else {
        primary = QStringLiteral("%1  %2")
            .arg(mediaGlyph(snap.mediaType), mediaLabel(snap.mediaType));
        if (!snap.caption.isEmpty()) {
            // Truncate captions to keep the row a single visual line.
            const int kMaxCap = 40;
            const QString cap = snap.caption.size() > kMaxCap
                ? snap.caption.left(kMaxCap - 1) + QStringLiteral("…")
                : snap.caption;
            primary += QStringLiteral("  ·  ") + cap;
        }
    }
    auto *primaryLbl = new QLabel(primary, card);
    primaryLbl->setWordWrap(isText);   // full message wraps; media descriptor stays one line
    primaryLbl->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 13px; background: transparent;")
        .arg(fg));
    cardLay->addWidget(primaryLbl);
    }

    // Secondary line: relative time + delivery indicator (outbound only).
    QString secondary = relativeTime(snap.sentAt);
    if (outbound) {
        // ✓ = sent (we have it persisted, by definition); ✓✓ = delivered
        // (peer ACKed). View-once doesn't get a separate indicator here —
        // the snap stays in the thread either way until expiry.
        secondary += snap.deliveredAt.isValid()
            ? QStringLiteral("  ✓✓")
            : QStringLiteral("  ✓");
    }
    auto *secondaryLbl = new QLabel(secondary, card);
    secondaryLbl->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 11px; background: transparent;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    cardLay->addWidget(secondaryLbl);

    if (outbound) {
        rowLay->addStretch(1);
        rowLay->addWidget(card);
    } else {
        rowLay->addWidget(card);
        rowLay->addStretch(1);
    }

    m_threadLayout->addWidget(row);
}

// ─── V32.26: drag-down-to-close gesture ──────────────────────────────────────
//
// The user can drag down anywhere on the header bar (outside the Close
// button) to dismiss the dialog. Threshold is intentionally generous —
// 60px — so the gesture doesn't trigger accidentally during a sideways
// swipe or normal scroll attempt that happened to start on the header.
//
// We do NOT instrument the QScrollArea below the header: that area
// already owns vertical drag for scrolling content, and overriding
// it would break native flick-scroll. The header is the empty space
// the user can grab to dismiss.

bool ConversationDetailDialog::eventFilter(QObject *watched, QEvent *event)
{
    if (watched == m_header) {
        if (handleHeaderDragEvent(event)) return true;
    }
    return MobileDialog::eventFilter(watched, event);
}

bool ConversationDetailDialog::handleHeaderDragEvent(QEvent *e)
{
    // We accept mouse-press / mouse-move / mouse-release on the header.
    // Touch events get synthesized to mouse events by Qt when the widget
    // doesn't set WA_AcceptTouchEvents, which is the case here. If we
    // later need real touch handling (e.g. for kinetic close animation),
    // switch to TouchBegin/TouchUpdate/TouchEnd and call
    // setAttribute(Qt::WA_AcceptTouchEvents, true) on m_header.
    switch (e->type()) {
    case QEvent::MouseButtonPress: {
        auto *me = static_cast<QMouseEvent *>(e);
        if (me->button() != Qt::LeftButton) return false;
        m_dragActive = true;
        m_dragStartY = me->globalPosition().toPoint().y();
        // Don't consume — let the press propagate (it'd be needed if we
        // ever wanted ChildAt detection during press). MoveEvent will
        // do the discrimination.
        return false;
    }

    case QEvent::MouseMove: {
        if (!m_dragActive) return false;
        auto *me = static_cast<QMouseEvent *>(e);
        const int delta = me->globalPosition().toPoint().y() - m_dragStartY;
        // We don't visually rubber-band (see header comment for why).
        // Only fire close when the gesture crosses threshold. Threshold
        // is committed-on-cross (no snap-back), so a single decisive
        // drag-down past 60px closes immediately.
        constexpr int kCloseThresholdPx = 60;
        if (delta >= kCloseThresholdPx) {
            m_dragActive = false;
            qCInfo(lcConvDet) << "drag-to-close: threshold crossed (" << delta
                              << "px) — closing dialog";
            // Use QDialog::reject — MobileDialog overrides reject to
            // play the bottom-sheet slide-out animation, so the user
            // gets a coherent "the sheet drops away" effect even
            // without live rubber-band.
            reject();
            return true;
        }
        return false;
    }

    case QEvent::MouseButtonRelease: {
        if (!m_dragActive) return false;
        m_dragActive = false;
        // No threshold cross during the gesture — fall through. We
        // intentionally don't consume the release; if the user pressed
        // the close button it would have already fired its own click,
        // and a release on empty header space is harmless.
        return false;
    }

    default:
        return false;
    }
}

} // namespace SnapClone::UI
