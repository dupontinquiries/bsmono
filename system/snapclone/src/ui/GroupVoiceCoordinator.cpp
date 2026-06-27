#include "ui/GroupVoiceCoordinator.hpp"

#include "ui/GroupVoiceBar.hpp"
#include "core/GroupVoiceCapture.hpp"
#include "core/ActivityGate.hpp"
#include "core/GroupVoiceSender.hpp"
#include "core/GroupVoiceReceiver.hpp"

#include <QLoggingCategory>
#include <utility>

// The backend objects are CMake-guarded. Mirror those guards here so the
// coordinator is ALWAYS compilable + linkable (its signals/slots are generated
// unconditionally) and simply runs inert when the backend isn't built — that
// keeps the conversation dialog free of feature #ifdefs.
//   SC_VOICE_CORE  : GroupRadioSession (floor) + GroupVoiceCapture (mic)
//   SC_VOICE_CODEC : + GroupVoiceSender / GroupVoiceReceiver (Opus encode/decode)
#if defined(SNAPCLONE_RUST_GROUP) && defined(SNAPCLONE_RUST_AUDIO)
#  define SC_VOICE_CORE 1
#endif
#if defined(SC_VOICE_CORE) && defined(SNAPCLONE_OPUS_CODEC)
#  define SC_VOICE_CODEC 1
#endif

Q_LOGGING_CATEGORY(lcVoiceCoord, "snapclone.ui.voicecoord")

namespace SnapClone::UI {

GroupVoiceCoordinator::GroupVoiceCoordinator(QString selfId, QObject *parent)
    : QObject(parent)
    , m_selfId(std::move(selfId))
{
#ifdef SC_VOICE_CORE
    // Owned, parented → destroyed with the coordinator. Created up front so the
    // internal graph wires once; the mic/socket only open on joinCall().
    m_session = new Core::GroupRadioSession(m_selfId, this);
    m_capture = new Core::GroupVoiceCapture(this);
#  ifdef SC_VOICE_CODEC
    m_sender   = new Core::GroupVoiceSender(/*bitrateBitsPerSec=*/16000, this);
    m_receiver = new Core::GroupVoiceReceiver(this);
#  endif
    wireInternal();

    // Privacy + battery: drop the mic the moment the device stops being
    // interactive — screen off, app backgrounded, or battery-saver (ActivityGate
    // unifies these). Resume it when interactivity returns IF still in a call.
    // capture.start() additionally honors the app-wide mic kill-switch.
    connect(Core::ActivityGate::instance(), &Core::ActivityGate::interactiveChanged,
            this, [this](bool interactive) {
        if (!m_capture) return;
        if (!interactive)  m_capture->stop();
        else if (m_inCall) m_capture->start();
    });
#else
    qCInfo(lcVoiceCoord) << "voice backend not built (needs SNAPCLONE_RUST_GROUP "
                            "+ SNAPCLONE_RUST_AUDIO) — coordinator is inert.";
#endif
}

GroupVoiceCoordinator::~GroupVoiceCoordinator()
{
    leaveCall();   // idempotent; releases the floor + stops the owned objects
}

bool GroupVoiceCoordinator::isAvailable() const
{
#ifdef SC_VOICE_CORE
    return true;
#else
    return false;
#endif
}

void GroupVoiceCoordinator::wireInternal()
{
#ifdef SC_VOICE_CORE
    // ── capture → floor VAD / meters ──
    connect(m_capture, &Core::GroupVoiceCapture::voiceActivity,
            m_session, &Core::GroupRadioSession::onVoiceActivity);
    connect(m_capture, &Core::GroupVoiceCapture::micLevel, this, [this](float rms) {
        if (m_bar) m_bar->setMicLevel(rms);
        // The roster's animated level belongs to the ACTIVE speaker; we only have
        // our own mic locally, so feed it only while WE hold the floor. When a
        // peer talks the roster still shows the active ring via setActiveSpeaker.
        if (m_roster && m_holder == m_selfId) m_roster->setSpeakerLevel(rms);
    });
    connect(m_capture, &Core::GroupVoiceCapture::captureError,
            this, &GroupVoiceCoordinator::captureError);

    // ── floor session → UI / transport ──
    connect(m_session, &Core::GroupRadioSession::transmitStateChanged, this,
            [this](bool on) { if (m_bar) m_bar->setOnAir(on); });
    connect(m_session, &Core::GroupRadioSession::floorHolderChanged,
            this, &GroupVoiceCoordinator::applyHolder);
    connect(m_session, &Core::GroupRadioSession::floorAssertRequested,
            this, &GroupVoiceCoordinator::floorAssertToSend);
    connect(m_session, &Core::GroupRadioSession::floorReleaseRequested,
            this, &GroupVoiceCoordinator::floorReleaseToSend);

#  ifdef SC_VOICE_CODEC
    // ── capture → encoder, session → encoder gate, encoder → transport ──
    connect(m_capture, &Core::GroupVoiceCapture::frameReady,
            m_sender,  &Core::GroupVoiceSender::onFrameReady);
    connect(m_session, &Core::GroupRadioSession::transmitStateChanged,
            m_sender,  &Core::GroupVoiceSender::setTransmitting);
    connect(m_sender, &Core::GroupVoiceSender::voicePacketReady,
            this, &GroupVoiceCoordinator::voicePacketToSend);
#  endif
#endif
}

void GroupVoiceCoordinator::setViews(GroupVoiceBar *bar,
                                     GroupVoiceRoster *roster,
                                     ConversationSecurityBar *securityBar)
{
    // Drop prior view→coordinator links (all view signals target `this`).
    if (m_bar)         disconnect(m_bar, nullptr, this, nullptr);
    if (m_roster)      disconnect(m_roster, nullptr, this, nullptr);
    if (m_securityBar) disconnect(m_securityBar, nullptr, this, nullptr);

    m_bar = bar;
    m_roster = roster;
    m_securityBar = securityBar;

    if (m_bar) {
        connect(m_bar, &GroupVoiceBar::talkPressed, this, [this] {
#ifdef SC_VOICE_CORE
            if (m_session) m_session->pressTalk();
#endif
        });
        connect(m_bar, &GroupVoiceBar::talkReleased, this, [this] {
#ifdef SC_VOICE_CORE
            if (m_session) m_session->releaseTalk();
#endif
        });
        connect(m_bar, &GroupVoiceBar::privateModeRequested,
                this, &GroupVoiceCoordinator::requestPrivate);
    }
    if (m_roster) {
        connect(m_roster, &GroupVoiceRoster::participantActivated,
                this, &GroupVoiceCoordinator::participantActivated);
        m_roster->setParticipants(m_participants);
    }
    if (m_securityBar) {
        connect(m_securityBar, &ConversationSecurityBar::privateRequestToggled,
                this, &GroupVoiceCoordinator::requestPrivate);
    }

    applyHolder(m_holder);   // reflect current state onto fresh views

    // Size the talk button for the current mode (grand in PTT, comfortable in
    // the auto modes). Default mode is PushToTalk → grand.
#ifdef SC_VOICE_CORE
    if (m_bar && m_session)
        m_bar->setTalkProminent(m_session->talkMode() == Core::TalkMode::PushToTalk);
#else
    if (m_bar) m_bar->setTalkProminent(true);
#endif
}

void GroupVoiceCoordinator::setParticipants(const QList<Participant> &people)
{
    m_participants = people;
    if (m_roster) m_roster->setParticipants(people);
    if (!m_holder.isEmpty()) applyHolder(m_holder);
}

void GroupVoiceCoordinator::setTalkMode(Core::TalkMode mode)
{
#ifdef SC_VOICE_CORE
    if (m_session) m_session->setTalkMode(mode);
    // PTT is the high-frequency button (you hold it); the auto modes (VOX /
    // continuous) rarely touch it → relax it to the comfortable size.
    if (m_bar) m_bar->setTalkProminent(mode == Core::TalkMode::PushToTalk);
#else
    Q_UNUSED(mode);
#endif
}

void GroupVoiceCoordinator::joinCall()
{
    if (m_inCall) return;
    m_inCall = true;
    qCInfo(lcVoiceCoord) << "joinCall — starting voice subsystem for" << m_selfId;
#ifdef SC_VOICE_CODEC
    if (m_receiver) m_receiver->start();   // open the sink first
#endif
#ifdef SC_VOICE_CORE
    if (m_session) m_session->start();
    // Open the mic only if the device is interactive (screen on / foreground);
    // otherwise the ActivityGate handler opens it once interactivity returns.
    if (m_capture && Core::ActivityGate::instance()->interactive())
        m_capture->start();
#endif
}

void GroupVoiceCoordinator::leaveCall()
{
    if (!m_inCall) return;
    m_inCall = false;
    qCInfo(lcVoiceCoord) << "leaveCall — stopping voice subsystem";
#ifdef SC_VOICE_CORE
    if (m_capture) m_capture->stop();
    if (m_session) m_session->stop();   // releases the floor if held
#endif
#ifdef SC_VOICE_CODEC
    if (m_receiver) m_receiver->stop();
#endif
    if (m_bar) { m_bar->setOnAir(false); m_bar->setTalker(QString(), false); }
    if (m_roster) m_roster->setActiveSpeaker(QString());
    m_holder.clear();
}

void GroupVoiceCoordinator::setSecurityTier(Tier tier)
{
    if (m_securityBar) m_securityBar->setTier(tier);
    if (m_bar) {
        GroupVoiceBar::Security s = GroupVoiceBar::Security::Public;
        switch (tier) {
        case Tier::Standard: s = GroupVoiceBar::Security::Public;   break;
        case Tier::Securing: s = GroupVoiceBar::Security::Securing; break;
        case Tier::Private:  s = GroupVoiceBar::Security::Private;  break;
        }
        m_bar->setSecurityState(s);
    }
}

void GroupVoiceCoordinator::onVoiceFrameReceived(const QString &senderId, const QByteArray &wireFrame)
{
#ifdef SC_VOICE_CODEC
    if (m_receiver) m_receiver->onVoiceFrame(senderId, wireFrame);
#else
    Q_UNUSED(senderId);
    Q_UNUSED(wireFrame);
#endif
}

void GroupVoiceCoordinator::enqueuePreroll(const QString &senderId, const QByteArray &wireFrame)
{
#ifdef SC_VOICE_CODEC
    if (m_receiver) m_receiver->enqueuePreroll(senderId, wireFrame);
#else
    Q_UNUSED(senderId);
    Q_UNUSED(wireFrame);
#endif
}

void GroupVoiceCoordinator::onPeerFloorAssert(const QString &holder, quint64 claimTs)
{
#ifdef SC_VOICE_CORE
    if (m_session) m_session->onPeerFloorAssert(holder, claimTs);
#else
    Q_UNUSED(holder); Q_UNUSED(claimTs);
#endif
}

void GroupVoiceCoordinator::onPeerFloorRelease(const QString &holder)
{
#ifdef SC_VOICE_CORE
    if (m_session) m_session->onPeerFloorRelease(holder);
#else
    Q_UNUSED(holder);
#endif
}

void GroupVoiceCoordinator::applyHolder(const QString &holder)
{
    m_holder = holder;
    if (m_roster) m_roster->setActiveSpeaker(holder);
    if (m_bar) {
        if (holder.isEmpty()) m_bar->setTalker(QString(), false);
        else m_bar->setTalker(nameForId(holder), holder == m_selfId);
    }
}

void GroupVoiceCoordinator::requestPrivate(bool on)
{
    if (m_securityBar) m_securityBar->setPrivateRequested(on);
    setSecurityTier(on ? Tier::Securing : Tier::Standard);
    emit privateModeRequested(on);
}

QString GroupVoiceCoordinator::nameForId(const QString &id) const
{
    for (const auto &p : m_participants)
        if (p.id == id) return p.name.isEmpty() ? id : p.name;
    return id;
}

} // namespace SnapClone::UI
