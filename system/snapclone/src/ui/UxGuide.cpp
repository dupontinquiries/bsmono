#include "ui/UxGuide.hpp"
#include "ui/ChevronConfirm.hpp"
#include <QSettings>
#include <QWidget>
#include <QTimer>
#include <QDateTime>
#include <QApplication>
#include <QLoggingCategory>
#include <QtMath>

Q_LOGGING_CATEGORY(lcUx, "snapclone.ux")

namespace SnapClone::UI {

// ── FamiliarityScore ──────────────────────────────────────────────────────────

int FamiliarityScore::decayed() const
{
    if (lastUsedMs == 0) return 0;
    const qint64 ageDays = (QDateTime::currentMSecsSinceEpoch() - lastUsedMs)
                           / (86400LL * 1000LL);
    double decay = qPow(0.5, static_cast<double>(ageDays) / 90.0);
    // Users who fail more than they succeed get extra decay — they need more help
    if (isStruggling()) decay *= 0.7;
    return qBound(0, qRound(score * decay), 100);
}

FeatureState FamiliarityScore::state() const
{
    if (firstUsedMs == 0) return FeatureState::Unknown;
    if (useCount == 0)    return FeatureState::Introduced;
    if (useCount < 3)     return FeatureState::Attempted;  // struggled to get here
    const int d = decayed();
    if (d < 20)  return FeatureState::Succeeded;
    if (d < 60)  return FeatureState::Practiced;
    return FeatureState::Fluent;
}

// ── Singleton ─────────────────────────────────────────────────────────────────

UxGuide &UxGuide::instance()
{
    static UxGuide g;
    return g;
}

UxGuide::UxGuide(QObject *parent) : QObject(parent)
{
    load();
    registerBuiltinChains();
    registerBuiltinHints();
    registerBuiltinMilestones();
    updatePhase();

    // Periodic phase re-check — phases change based on calendar days
    m_phaseTimer = new QTimer(this);
    m_phaseTimer->setInterval(30 * 60 * 1000); // every 30 min
    connect(m_phaseTimer, &QTimer::timeout, this, &UxGuide::updatePhase);
    m_phaseTimer->start();

    // V32.34+: save-coalescing timer. recordUse(), recordFail(),
    // recordIntroduction() all flagged the model dirty and need a
    // QSettings write — but previously each called save() inline,
    // which writes ~30 keys to QSettings synchronously. Across a
    // few page swipes that was hundreds of writes, visible in the
    // gesture's tail latency on slow phones.
    //
    // Now: those callers set m_pendingSave + start this 2-second
    // single-shot. Subsequent recordUse() calls within the 2s window
    // just reset the timer (the data is already captured in m_scores).
    // The final save() runs once at the 2s deadline (or sooner if
    // flush() is called at app exit).
    m_saveTimer = new QTimer(this);
    m_saveTimer->setSingleShot(true);
    m_saveTimer->setInterval(2000);
    connect(m_saveTimer, &QTimer::timeout, this, [this]() {
        if (m_pendingSave) {
            m_pendingSave = false;
            save();
        }
    });
}

void UxGuide::flush()
{
    // Called from app shutdown / wherever the caller wants guaranteed
    // persistence of any pending save. Cancels the debounce timer so
    // we don't double-write.
    if (!m_pendingSave) return;
    if (m_saveTimer) m_saveTimer->stop();
    m_pendingSave = false;
    save();
}

// ── Persistence ───────────────────────────────────────────────────────────────

void UxGuide::load()
{
    QSettings s(QStringLiteral("SnapCloneProject"), QStringLiteral("SnapClone"));
    s.beginGroup(QStringLiteral("ux_familiarity"));

    m_firstLaunchMs = s.value(QStringLiteral("firstLaunch"), 0LL).toLongLong();
    m_totalSessions = s.value(QStringLiteral("sessions"), 0).toInt();

    for (const QString &key : s.childGroups()) {
        s.beginGroup(key);
        FamiliarityScore fs;
        fs.score       = s.value(QStringLiteral("score"),    0).toInt();
        fs.useCount    = s.value(QStringLiteral("uses"),     0).toInt();
        fs.failCount   = s.value(QStringLiteral("fails"),    0).toInt();
        fs.streak      = s.value(QStringLiteral("streak"),   0).toInt();
        fs.lastUsedMs  = s.value(QStringLiteral("last"),     0LL).toLongLong();
        fs.firstUsedMs = s.value(QStringLiteral("first"),    0LL).toLongLong();
        fs.lastFailMs  = s.value(QStringLiteral("lastFail"), 0LL).toLongLong();
        m_scores[key] = fs;
        s.endGroup();
    }
    s.endGroup();

    // Load seen hints
    s.beginGroup(QStringLiteral("ux_hints_seen"));
    for (const QString &k : s.childKeys())
        if (s.value(k, false).toBool()) m_seenHints.insert(k);
    s.endGroup();

    // Load achieved milestones
    s.beginGroup(QStringLiteral("ux_milestones"));
    for (const QString &k : s.childKeys())
        if (m_milestones.contains(k))
            m_milestones[k].achieved = s.value(k, false).toBool();
    s.endGroup();
}

void UxGuide::save()
{
    QSettings s(QStringLiteral("SnapCloneProject"), QStringLiteral("SnapClone"));
    s.beginGroup(QStringLiteral("ux_familiarity"));
    s.setValue(QStringLiteral("firstLaunch"), m_firstLaunchMs);
    s.setValue(QStringLiteral("sessions"),    m_totalSessions);
    for (auto it = m_scores.constBegin(); it != m_scores.constEnd(); ++it) {
        s.beginGroup(it.key());
        const auto &fs = it.value();
        s.setValue(QStringLiteral("score"),    fs.score);
        s.setValue(QStringLiteral("uses"),     fs.useCount);
        s.setValue(QStringLiteral("fails"),    fs.failCount);
        s.setValue(QStringLiteral("streak"),   fs.streak);
        s.setValue(QStringLiteral("last"),     fs.lastUsedMs);
        s.setValue(QStringLiteral("first"),    fs.firstUsedMs);
        s.setValue(QStringLiteral("lastFail"), fs.lastFailMs);
        s.endGroup();
    }
    s.endGroup();

    s.beginGroup(QStringLiteral("ux_hints_seen"));
    for (const QString &k : m_seenHints)
        s.setValue(k, true);
    s.endGroup();

    s.beginGroup(QStringLiteral("ux_milestones"));
    for (auto it = m_milestones.constBegin(); it != m_milestones.constEnd(); ++it)
        s.setValue(it.key(), it.value().achieved);
    s.endGroup();
}

// ── Hint registry ─────────────────────────────────────────────────────────────

void UxGuide::registerBuiltinHints()
{
    auto add = [this](HintDef h) { m_hints[h.key] = std::move(h); };

    // Day 0 — Discovery phase
    add({ Hint::kWelcome,
          tr("Welcome to SnapClone"),
          tr("SnapClone lets you send disappearing photos to friends — directly, "
             "without any servers or accounts. Everything stays between you and "
             "your friends.\n\nSwipe left and right to explore the four pages."),
          QStringLiteral("beacon"), 0, 1, false, false });

    add({ Hint::kSwipeNav,
          tr("Swipe to switch pages"),
          tr("Slide your finger left or right across the screen to move between "
             "Camera, Inbox, Friends and Settings. The dots at the top of the "
             "bottom bar show where you are."),
          QStringLiteral("send"), 0, 3, false, false });

    add({ Hint::kCameraFirst,
          tr("Take a photo to get started"),
          tr("Tap the circle button to capture a photo. You can then send it to "
             "a friend — it will disappear after they view it."),
          QStringLiteral("camera"), 0, 7, false, false });

    add({ Hint::kFriendsEmpty,
          tr("No friends yet"),
          tr("Go to the Friends page and tap the + button to add someone. "
             "You'll need their sc-ID — it's in their Settings page. "
             "Or scan their QR code!"),
          QStringLiteral("friends"), 0, 7, false, false });

    add({ Hint::kAddFirst,
          tr("Add your first contact"),
          tr("Tap the camera icon in the Friends page to scan someone's QR code, "
             "or ask them for their sc-ID and type it in the box below."),
          QStringLiteral("camera"), 0, 14, false, false });

    // Day 1-3 — First use phase
    add({ Hint::kQrExplain,
          tr("What is a QR code?"),
          tr("A QR code is like a barcode you can scan with your camera. "
             "Your SnapClone QR code lets friends add you just by pointing "
             "their camera at it — no typing needed."),
          QStringLiteral("camera"), 1, 14, false, false });

    add({ Hint::kShareId,
          tr("Share your sc-ID with friends"),
          tr("Your sc-ID looks like 'sc-a1b2c3d4'. Share it by showing your "
             "QR code (in Settings), copying it to a message, or just reading "
             "it out loud — it's only 12 characters."),
          QStringLiteral("export"), 0, 30, false, false });

    add({ Hint::kSnapSent,
          tr("Snap sent! 🎉"),
          tr("Your friend will see a notification when your snap arrives. "
             "Once they open it, it disappears — that's the whole idea."),
          QStringLiteral("send"), 1, 30, false, true }); // celebratory

    add({ Hint::kSnapExpiry,
          tr("Snaps disappear after viewing"),
          tr("Once your friend opens a snap, it's gone. They can't save it, "
             "screenshot it without you knowing, or send it to anyone else. "
             "That's what makes this private."),
          QStringLiteral("inbox"), 1, 60, false, false });

    add({ Hint::kInboxEmpty,
          tr("Inbox is empty"),
          tr("When a friend sends you a snap, it will appear here. "
             "Snaps disappear after you view them — no history is kept."),
          QStringLiteral("inbox"), 0, 14, false, false });

    // Day 4-14 — Habit phase
    add({ Hint::kPollinator,
          tr("Find friends nearby"),
          tr("The Beacon feature lets you discover other SnapClone users on "
             "the same Wi-Fi. Great for adding friends at school, work, or "
             "a café without exchanging IDs manually."),
          QStringLiteral("beacon"), 0, 60, false, false });

    add({ Hint::kThemeHint,
          tr("Make it yours"),
          tr("SnapClone has 22 colour themes — from Dracula to Lavender Mist. "
             "Find them in Settings → Appearance. Your theme choice stays "
             "private and doesn't affect how snaps look to your friends."),
          QStringLiteral("settings"), 0, 30, false, false });

    add({ Hint::kVcardExport,
          tr("Back up your contacts"),
          tr("You can export your friends list as a vCard (.vcf) file and save "
             "it to your device. Useful if you reinstall the app or switch "
             "phones — your friends' sc-IDs are preserved."),
          QStringLiteral("export"), 1, 90, false, false });

    add({ Hint::kVcardImport,
          tr("Restore contacts from a backup"),
          tr("If you have a .vcf backup from a previous installation, tap "
             "Import in the Friends page to restore all your contacts at once."),
          QStringLiteral("import"), 0, 90, false, false });

    add({ Hint::kStreakStart,
          tr("Day 2 — you're building a habit"),
          tr("You've opened SnapClone two days in a row. The more you use it, "
             "the more SnapClone steps back and lets you work uninterrupted."),
          QString{}, 0, 30, false, true }); // celebratory

    // Day 15-30 — Comfort phase
    add({ Hint::kPrivacyRemind,
          tr("Your privacy at a glance"),
          tr("• No account required — your sc-ID is generated on your device\n"
             "• No servers store your messages\n"
             "• Snaps are encrypted end-to-end\n"
             "• Location is only shared when you choose to send a snap"),
          QStringLiteral("settings"), 0, 60, false, false });

    add({ Hint::kNoCloudRemind,
          tr("No cloud, no history"),
          tr("SnapClone doesn't upload anything to a server. Your snaps "
             "travel directly from your phone to your friend's — or through "
             "an anonymous relay if you're on different networks. "
             "Nothing is stored after delivery."),
          QString{}, 0, 60, false, false });

    add({ Hint::kP2PExplain,
          tr("How SnapClone finds your friends"),
          tr("When you and a friend are on the same Wi-Fi, snaps travel "
             "directly. On different networks, SnapClone uses hole-punching "
             "to connect directly. If that fails, an anonymous relay "
             "forwards the snap — then immediately forgets it."),
          QStringLiteral("beacon"), 0, 90, false, false });

    // Day 60+ — Ambassador phase
    add({ Hint::kAmbassador,
          tr("Know someone who'd love this?"),
          tr("SnapClone works best with people you actually know. Your QR "
             "code in Settings makes it easy to add friends in seconds — "
             "show it to someone and they can add you instantly."),
          QStringLiteral("camera"), 10, 0, false, false });

    add({ Hint::kQrForFriends,
          tr("Share SnapClone with a friend"),
          tr("SnapClone is free and open source. If a friend wants to try it, "
             "they can get it at snapclone.org. Once installed, scan each "
             "other's QR codes and you're connected."),
          QStringLiteral("export"), 5, 0, false, false });
}

// ── Milestone registry ────────────────────────────────────────────────────────

void UxGuide::registerBuiltinMilestones()
{
    auto add = [this](Milestone m) { m_milestones[m.key] = std::move(m); };

    add({ MilestoneKey::kFirstSnap,
          tr("First snap sent! 📸"),
          tr("You just sent your first snap. It will disappear after your "
             "friend views it — that's what makes it special.") });

    add({ MilestoneKey::kFirstContact,
          tr("First friend added! 🤝"),
          tr("You've added your first contact. Now you can send them a snap!") });

    add({ MilestoneKey::kFirstReceived,
          tr("First snap received! 📬"),
          tr("A friend sent you a snap. Open it in your Inbox — "
             "once you close it, it's gone forever.") });

    add({ MilestoneKey::kWeekStreak,
          tr("7-day streak! 🔥"),
          tr("You've used SnapClone every day this week. The hints will "
             "start backing off now — you've got this.") });

    add({ MilestoneKey::kTenSnaps,
          tr("10 snaps sent! 🚀"),
          tr("You're a natural. SnapClone is yours now.") });

    add({ MilestoneKey::kFiveContacts,
          tr("5 friends connected! 👥"),
          tr("Your private circle is growing. Every one of them communicates "
             "with you without any company reading the messages.") });

    add({ MilestoneKey::kOneMonth,
          tr("One month of private snaps 🗓️"),
          tr("30 days of SnapClone. Not one message was stored on any server. "
             "Not one snap was read by anyone but its recipient.") });

    add({ MilestoneKey::kExplorer,
          tr("Explorer 🗺️"),
          tr("You've visited all four pages. Now you know your way around.") });

    add({ MilestoneKey::kThemer,
          tr("Themer 🎨"),
          tr("You found the themes! Try Dracula or Forest Stone for a "
             "different mood.") });

    add({ MilestoneKey::kNetworker,
          tr("Networker 📡"),
          tr("You used the Beacon to find a nearby friend. Direct connections "
             "are the fastest — no relay needed.") });
}

// ── Chain registry ────────────────────────────────────────────────────────────

void UxGuide::registerBuiltinChains()
{
    auto add = [this](ActionChain c) { m_chains[c.id] = std::move(c); };

    add({ QStringLiteral("send_snap"), tr("Send a snap"),
        {
          { QStringLiteral("open_camera"),   tr("Tap the round camera button"),
            tr("It's the large circle at the bottom of the Camera page."), 8000, 15000 },
          { QStringLiteral("take_photo"),    tr("Tap the shutter to capture"),
            tr("Hold steady — the shutter button is the large white circle."), 6000, 12000 },
          { QStringLiteral("pick_recipient"),tr("Choose who to send to"),
            tr("Tap a friend's name, then tap Send."), 8000, 16000 },
          { QStringLiteral("sent"),          QString{}, QString{}, 0 },
        }, 5 });

    add({ QStringLiteral("add_contact"), tr("Add a contact"),
        {
          { QStringLiteral("open_friends"),  tr("Swipe to the Friends page"),
            tr("Slide right from any page, or tap Friends in the bottom bar."), 6000, 12000 },
          { QStringLiteral("tap_add"),       tr("Tap the + or camera button"),
            tr("The camera button scans a QR code. The + button lets you type an sc-ID."), 5000, 10000 },
          { QStringLiteral("enter_id"),      tr("Enter your friend's sc-ID"),
            tr("It looks like 'sc-a1b2c3d4'. They can find it in their Settings page."), 6000, 12000 },
          { QStringLiteral("connected"),     QString{}, QString{}, 0 },
        }, 10 });

    add({ QStringLiteral("view_snap"), tr("View a snap"),
        {
          { QStringLiteral("open_inbox"),    tr("Go to your Inbox"),
            tr("Swipe left from Camera, or tap Inbox in the bottom bar."), 8000, 0 },
          { QStringLiteral("tap_snap"),      tr("Tap the snap to open it"),
            tr("Once you open it, it will disappear after you close it."), 5000, 10000 },
          { QStringLiteral("viewed"),        QString{}, QString{}, 0 },
        }, 5 });

    add({ QStringLiteral("qr_add"), tr("Add via QR code"),
        {
          { QStringLiteral("open_scanner"),  tr("Tap the camera icon in Friends"),
            tr("This opens the QR scanner so you can scan a friend's code."), 5000, 0 },
          { QStringLiteral("scan_qr"),       tr("Point your camera at their QR code"),
            tr("Hold steady — the code will scan automatically."), 8000, 15000 },
          { QStringLiteral("scanned"),       QString{}, QString{}, 0 },
        }, 5 });
}

// ── Lifecycle ─────────────────────────────────────────────────────────────────

void UxGuide::onAppLaunched()
{
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    if (m_firstLaunchMs == 0) {
        m_firstLaunchMs = now;
        qCInfo(lcUx) << "First launch recorded";
    }
    m_totalSessions++;
    save();
    updatePhase();

    // Update daily streaks for all features used today
    for (auto it = m_scores.begin(); it != m_scores.end(); ++it) {
        auto &fs = it.value();
        if (fs.lastUsedMs == 0) continue;
        const qint64 lastDayMs = (now - fs.lastUsedMs) / (86400LL * 1000LL);
        if (lastDayMs == 1) {
            fs.streak++;
            qCDebug(lcUx) << it.key() << "streak:" << fs.streak;
        } else if (lastDayMs > 1) {
            fs.streak = 0; // streak broken
        }
    }

    // Schedule day-0 welcome hint on first ever launch
    if (m_totalSessions == 1) {
        QTimer::singleShot(1500, this, [this]() {
            if (!m_seenHints.contains(QLatin1String(Hint::kWelcome)))
                scheduleHint(QLatin1String(Hint::kWelcome), nullptr, 0);
        });
    }
    // Day 2 streak hint
    if (m_totalSessions == 2) {
        QTimer::singleShot(3000, this, [this]() {
            if (!m_seenHints.contains(QLatin1String(Hint::kStreakStart)))
                scheduleHint(QLatin1String(Hint::kStreakStart), nullptr, 0);
        });
    }
}

void UxGuide::onSessionStarted()
{
    updatePhase();

    // Phase-gated session hints — one per session max, with random selection
    const int day = daysSinceFirstLaunch();
    const OnboardingPhase phase = currentPhase();

    if (phase == OnboardingPhase::Discovery) {
        // Show camera hint after swipe hint has been seen
        if (m_seenHints.contains(QLatin1String(Hint::kSwipeNav)) &&
            !m_seenHints.contains(QLatin1String(Hint::kCameraFirst))) {
            QTimer::singleShot(2000, this, [this]() {
                scheduleHint(QLatin1String(Hint::kCameraFirst), nullptr, 0);
            });
        }
    }

    if (phase == OnboardingPhase::Habit && day <= 14) {
        // Introduce advanced features one per session
        if (!m_seenHints.contains(QLatin1String(Hint::kPollinator))) {
            QTimer::singleShot(4000, this, [this]() {
                scheduleHint(QLatin1String(Hint::kPollinator), nullptr, 0);
            });
        } else if (!m_seenHints.contains(QLatin1String(Hint::kThemeHint))) {
            QTimer::singleShot(4000, this, [this]() {
                scheduleHint(QLatin1String(Hint::kThemeHint), nullptr, 0);
            });
        }
    }

    if (phase == OnboardingPhase::Comfort) {
        // Privacy reminders in comfort phase — once per phase
        if (!m_seenHints.contains(QLatin1String(Hint::kPrivacyRemind))) {
            QTimer::singleShot(5000, this, [this]() {
                scheduleHint(QLatin1String(Hint::kPrivacyRemind), nullptr, 0);
            });
        }
    }

    if (phase == OnboardingPhase::Ambassador) {
        // Occasional sharing nudge (not more than once a week)
        const qint64 lastAmbMs = QSettings(
            QStringLiteral("SnapCloneProject"), QStringLiteral("SnapClone"))
            .value(QStringLiteral("ux_hints_seen/last_ambassador"), 0LL).toLongLong();
        const qint64 weekMs = 7LL * 86400LL * 1000LL;
        if (QDateTime::currentMSecsSinceEpoch() - lastAmbMs > weekMs) {
            QTimer::singleShot(5000, this, [this]() {
                scheduleHint(QLatin1String(Hint::kAmbassador), nullptr, 0);
            });
        }
    }
}

void UxGuide::onSessionEnded()
{
    save();
}

// ── Phase ─────────────────────────────────────────────────────────────────────

void UxGuide::updatePhase()
{
    const int day = daysSinceFirstLaunch();
    OnboardingPhase newPhase;
    if      (day == 0)    newPhase = OnboardingPhase::Discovery;
    else if (day <= 3)    newPhase = OnboardingPhase::FirstUse;
    else if (day <= 14)   newPhase = OnboardingPhase::Habit;
    else if (day <= 30)   newPhase = OnboardingPhase::Comfort;
    else if (day <= 60)   newPhase = OnboardingPhase::Fluent;
    else                  newPhase = OnboardingPhase::Ambassador;

    if (newPhase != m_phase) {
        m_phase = newPhase;
        qCInfo(lcUx) << "Phase →" << static_cast<int>(m_phase) << "(day" << day << ")";
        emit phaseChanged(m_phase);
    }
    emit globalScoreChanged(computeGlobalScore());
}

int UxGuide::daysSinceFirstLaunch() const
{
    if (m_firstLaunchMs == 0) return 0;
    return static_cast<int>(
        (QDateTime::currentMSecsSinceEpoch() - m_firstLaunchMs) / (86400LL * 1000LL));
}

int UxGuide::totalSessions() const { return m_totalSessions; }

OnboardingPhase UxGuide::currentPhase() const { return m_phase; }

int UxGuide::computeGlobalScore() const
{
    if (m_scores.isEmpty()) return 0;
    int total = 0;
    for (const auto &fs : m_scores) total += fs.decayed();
    // Normalise to 0–1000
    return qMin(1000, total * 10 / qMax(1, static_cast<int>(m_scores.size())));
}

int UxGuide::globalScore() const { return computeGlobalScore(); }

// ── Familiarity ───────────────────────────────────────────────────────────────

void UxGuide::recordUse(const QString &featureKey)
{
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    auto &fs = m_scores[featureKey];
    if (fs.firstUsedMs == 0) fs.firstUsedMs = now;
    fs.useCount++;
    fs.lastUsedMs = now;

    // Gain curve: steep early, tapers
    const int gain = fs.useCount == 1  ? 15
                   : fs.useCount <= 5  ?  8
                   : fs.useCount <= 20 ?  5 : 2;
    fs.score = qMin(100, fs.score + gain);

    qCDebug(lcUx) << "recordUse:" << featureKey
                  << "score=" << fs.score << "uses=" << fs.useCount;
    // V32.34+: debounce. Save coalesces multiple updates within a 2s
    // window into one QSettings write. flush() forces immediate write
    // for shutdown paths.
    m_pendingSave = true;
    if (m_saveTimer) m_saveTimer->start();
    checkMilestones(featureKey);
    emit familiarityChanged(featureKey, fs.decayed());
    emit globalScoreChanged(computeGlobalScore());
}

void UxGuide::recordFail(const QString &featureKey)
{
    auto &fs = m_scores[featureKey];
    fs.failCount++;
    fs.lastFailMs = QDateTime::currentMSecsSinceEpoch();
    // Don't increase score on fail — and add a note for the hint system
    qCDebug(lcUx) << "recordFail:" << featureKey
                  << "fails=" << fs.failCount << "uses=" << fs.useCount;
    m_pendingSave = true;
    if (m_saveTimer) m_saveTimer->start();

    // If struggling, schedule a contextual hint
    if (fs.isStruggling()) {
        const auto &hint = m_hints.value(featureKey + QStringLiteral("_fail"));
        if (!hint.key.isEmpty())
            scheduleHint(hint.key, nullptr, 500);
    }
}

void UxGuide::recordIntroduction(const QString &key)
{
    auto &fs = m_scores[key];
    if (fs.firstUsedMs == 0)
        fs.firstUsedMs = QDateTime::currentMSecsSinceEpoch();
    // Seeing an intro hint about a feature counts as "Introduced" state
    m_pendingSave = true;
    if (m_saveTimer) m_saveTimer->start();
}

FamiliarityScore UxGuide::familiarityFor(const QString &key) const
{
    return m_scores.value(key);
}

FeatureState UxGuide::stateFor(const QString &key) const
{
    return m_scores.value(key).state();
}

bool UxGuide::needsGuidance(const QString &key) const
{
    return !m_scores.contains(key) || m_scores[key].decayed() < 40;
}

bool UxGuide::shouldShowHint(const QString &hintKey) const
{
    if (m_seenHints.contains(hintKey)) return false;
    const auto &h = m_hints.value(hintKey);
    if (h.key.isEmpty()) return true; // unknown hint — show it
    const int day = daysSinceFirstLaunch();
    if (h.showUntilDay > 0 && day > h.showUntilDay) return false;
    return true;
}

void UxGuide::markHintSeen(const QString &hintKey)
{
    m_seenHints.insert(hintKey);
    // Track ambassador nudge time separately for weekly throttle
    if (hintKey == QLatin1String(Hint::kAmbassador)) {
        QSettings(QStringLiteral("SnapCloneProject"), QStringLiteral("SnapClone"))
            .setValue(QStringLiteral("ux_hints_seen/last_ambassador"),
                      QDateTime::currentMSecsSinceEpoch());
    }
    // V32.34+: debounced save (see recordUse for rationale).
    m_pendingSave = true;
    if (m_saveTimer) m_saveTimer->start();
}

// ── Hint scheduling ───────────────────────────────────────────────────────────

void UxGuide::scheduleHint(const QString &hintKey, QWidget *anchor, int delayMs)
{
    if (!shouldShowHint(hintKey)) return;

    // Global cooldown — don't nag
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    if (now - m_lastHintMs < kHintCooldownMs && delayMs == 0) return;

    QTimer::singleShot(qMax(0, delayMs), this, [this, hintKey, anchor]() {
        if (!shouldShowHint(hintKey)) return;
        const auto &h = m_hints.value(hintKey);
        const QString title = h.key.isEmpty() ? hintKey : h.title;
        const QString body  = h.body;
        m_lastHintMs = QDateTime::currentMSecsSinceEpoch();
        markHintSeen(hintKey);
        recordIntroduction(hintKey);
        qCInfo(lcUx) << "Hint shown:" << hintKey;
        emit hintReady(hintKey, title, body, anchor);
    });
}

// ── Milestones ────────────────────────────────────────────────────────────────

void UxGuide::checkMilestones(const QString &featureKey)
{
    auto check = [this](const QString &key, bool condition) {
        if (!condition) return;
        auto &ms = m_milestones[key];
        if (ms.achieved) return;
        ms.achieved = true;
        m_pendingMilestones.append(ms);
        qCInfo(lcUx) << "Milestone:" << key;
        emit milestoneAchieved(key, ms.title, ms.body);
    };

    const auto &fs = m_scores[featureKey];

    check(MilestoneKey::kFirstSnap,
          featureKey == Feature::kSendSnap && fs.useCount == 1);
    check(MilestoneKey::kFirstContact,
          featureKey == Feature::kAddContact && fs.useCount == 1);
    check(MilestoneKey::kFirstReceived,
          featureKey == Feature::kSnapReceived && fs.useCount == 1);
    check(MilestoneKey::kTenSnaps,
          featureKey == Feature::kSendSnap && fs.useCount == 10);
    check(MilestoneKey::kFiveContacts,
          featureKey == Feature::kAddContact && fs.useCount == 5);
    check(MilestoneKey::kThemer,
          featureKey == Feature::kThemeChange && fs.useCount == 1);
    check(MilestoneKey::kNetworker,
          featureKey == Feature::kPollinator && fs.useCount == 1);

    // One-month milestone based on calendar days
    check(MilestoneKey::kOneMonth, daysSinceFirstLaunch() >= 30 &&
          !m_milestones[MilestoneKey::kOneMonth].achieved);

    // Week streak: check send_snap or any core feature streak
    check(MilestoneKey::kWeekStreak, m_scores.value(Feature::kSendSnap).streak >= 7);

    // Explorer: visited all four pages
    const bool exploredAll =
        m_scores.contains(Feature::kCamera) &&
        m_scores.contains(Feature::kInbox)  &&
        m_scores.contains(Feature::kAddContact) &&
        m_scores.contains(Feature::kSettings);
    check(MilestoneKey::kExplorer, exploredAll);
}

QList<Milestone> UxGuide::pendingMilestones()
{
    return m_pendingMilestones;
}

void UxGuide::clearPendingMilestones()
{
    m_pendingMilestones.clear();
}

// ── Celebration ───────────────────────────────────────────────────────────────

void UxGuide::celebrate(const QString &milestoneKey, QWidget *parent)
{
    const auto &ms = m_milestones.value(milestoneKey);
    if (ms.key.isEmpty() || !parent) return;
    // Delegate to the hint signal — MainWindow shows a celebration panel
    emit hintReady(milestoneKey, ms.title, ms.body, parent);
    qCInfo(lcUx) << "Celebration shown:" << milestoneKey;
}

// ── Action chains ─────────────────────────────────────────────────────────────

void UxGuide::startChain(const QString &chainId)
{
    if (!m_chains.contains(chainId)) return;
    m_chainSteps[chainId] = 0;

    auto *timer = new QElapsedTimer;
    timer->start();
    delete m_chainTimers.take(chainId);
    m_chainTimers[chainId] = timer;

    // Set an abandon timer based on chain maxMinutes
    const int maxMs = m_chains[chainId].maxMinutes * 60 * 1000;
    QTimer::singleShot(maxMs, this, [this, chainId]() {
        if (m_chainTimers.contains(chainId))
            abandonChain(chainId);
    });

    const auto &step0 = m_chains[chainId].steps.value(0);
    if (step0.hintAfterMs > 0) {
        QTimer::singleShot(step0.hintAfterMs, this, [this, chainId]() {
            if (m_chainSteps.value(chainId, -1) != 0) return;
            const auto &ch = m_chains.value(chainId);
            if (!ch.steps.isEmpty())
                emit chainStepHint(chainId, ch.steps[0].hint, ch.steps[0].detailHint, 0);
        });
    }
}

void UxGuide::advanceChain(const QString &chainId, const QString &stepKey)
{
    if (!m_chains.contains(chainId)) return;
    const auto &chain = m_chains[chainId];
    int &cur = m_chainSteps[chainId];

    for (int i = 0; i < chain.steps.size(); ++i) {
        if (chain.steps[i].key != stepKey) continue;
        cur = i + 1;
        if (cur >= chain.steps.size()) break;
        const auto &next = chain.steps[cur];
        if (next.hintAfterMs > 0) {
            const int snap = cur;
            QTimer::singleShot(next.hintAfterMs, this, [this, chainId, snap]() {
                if (m_chainSteps.value(chainId) != snap) return;
                const auto &ch = m_chains.value(chainId);
                emit chainStepHint(chainId, ch.steps[snap].hint,
                                   ch.steps[snap].detailHint, snap);
            });
        }
        break;
    }
}

void UxGuide::completeChain(const QString &chainId)
{
    if (!m_chainTimers.contains(chainId)) return;
    const qint64 ms = m_chainTimers[chainId]->elapsed();
    qCInfo(lcUx) << "Chain complete:" << chainId << "in" << ms << "ms";
    recordUse(chainId);
    delete m_chainTimers.take(chainId);
    m_chainSteps.remove(chainId);
}

void UxGuide::abandonChain(const QString &chainId)
{
    if (m_chainTimers.contains(chainId))
        recordFail(chainId);
    delete m_chainTimers.take(chainId);
    m_chainSteps.remove(chainId);
    qCDebug(lcUx) << "Chain abandoned:" << chainId;
}

// ── Confirmation gate ─────────────────────────────────────────────────────────

bool UxGuide::confirmOrGate(const QString &featureKey, QWidget *anchor,
                             const QString &label, std::function<void()> onConfirm)
{
    if (!needsGuidance(featureKey)) { onConfirm(); return true; }
    ChevronConfirm::present(anchor, label, featureKey, std::move(onConfirm));
    return false;
}

} // namespace SnapClone::UI
