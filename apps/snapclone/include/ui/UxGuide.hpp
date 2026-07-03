#pragma once
// ─── UxGuide.hpp ─────────────────────────────────────────────────────────────
//
// Adaptive UX guidance system. Tracks user familiarity across months of use
// and surfaces contextual hints, milestone celebrations, and confirmations
// exactly calibrated to each user's actual experience level.
//
// # Philosophy
//
// A new user on day 1 and a returning user on day 45 need completely different
// help. This system measures both dimensions: how many times someone has done
// something (depth) and how long ago they last did it (recency). A user who
// sent 50 snaps six months ago and just reinstalled is treated as lightly
// experienced, not an expert — their muscle memory may have faded.
//
// The system is also culturally aware: users in regions with lower smartphone
// penetration, users identified as elderly (via font-size settings), and users
// whose first language isn't the app language all receive extended guidance
// automatically without being asked intrusive questions.
//
// # Onboarding phases (months-long arc)
//
// Week 1    — Day 0   DISCOVERY:  What does this app do? Where do I go?
//           — Day 1-3 FIRST_USE:  Successfully send first snap, add first contact
//           — Day 4-7 HABIT:      Building routine — showing tips at natural moments
//
// Month 1   — Day 8-14 COMFORT:   User is confident, hints back off
//           — Day 15-30 FLUENT:   Expert flows, no confirmation gates
//
// Month 2+  — Day 31+ AMBASSADOR: Gentle nudges to share with friends
//
// # Feature familiarity state machine
//
// Each feature has states:
//   UNKNOWN → INTRODUCED → ATTEMPTED → SUCCEEDED → PRACTICED → FLUENT
//
// Hints appear at UNKNOWN and INTRODUCED; confirmation gates at UNKNOWN through
// ATTEMPTED; celebrations at SUCCEEDED (first success); silence from PRACTICED on.
//
// # Scoring
//
// FamiliarityScore {
//   score:        0–100  (raw accumulated score)
//   useCount:     total successful uses
//   failCount:    failed attempts (user struggled — give more help)
//   streak:       consecutive days used (habit formation metric)
//   lastUsedMs:   for decay calculation
//   firstUsedMs:  day-0 timestamp
// }
//
// Score gains: +15 first use, +8 next 4 uses, +5 uses 6-20, +2 thereafter
// Score decay: 90-day half-life (score * 0.5^(days/90))
// Extra decay: ×0.7 if failCount > useCount (user is struggling)

#include <QObject>
#include <QHash>
#include <QSet>
#include <QElapsedTimer>
#include <QTimer>
#include <QString>
#include <QSettings>
#include <QDateTime>
#include <functional>

QT_BEGIN_NAMESPACE
class QWidget;
QT_END_NAMESPACE

namespace SnapClone::UI {

// ── Onboarding phase ──────────────────────────────────────────────────────────

enum class OnboardingPhase {
    Discovery,    // Day 0: first launch, orientation
    FirstUse,     // Day 1-3: building first successful flows
    Habit,        // Day 4-14: learning by doing, contextual tips
    Comfort,      // Day 15-30: confident, lighter touches
    Fluent,       // Day 31+: mostly silent
    Ambassador,   // Day 60+: sharing nudges
};

// ── Feature state machine ─────────────────────────────────────────────────────

enum class FeatureState {
    Unknown,      // never seen this feature
    Introduced,   // saw a hint but hasn't tried it
    Attempted,    // tried but didn't fully succeed
    Succeeded,    // completed successfully at least once
    Practiced,    // completed 5+ times
    Fluent,       // 20+ completions, expert
};

// ── Familiarity score (extended) ──────────────────────────────────────────────

struct FamiliarityScore {
    int    score        = 0;    // 0–100 accumulated
    int    useCount     = 0;    // successful completions
    int    failCount    = 0;    // failed attempts
    int    streak       = 0;    // consecutive days with at least one use
    qint64 lastUsedMs   = 0;
    qint64 firstUsedMs  = 0;
    qint64 lastFailMs   = 0;

    int          decayed()  const;  // score adjusted for time decay
    FeatureState state()    const;  // derived state from score + counts
    bool         isExpert() const { return decayed() >= 80; }
    bool         isFamiliar() const { return decayed() >= 40; }
    bool         isNew()    const { return decayed() < 20; }
    bool         isStruggling() const { return failCount > useCount && useCount < 5; }
};

// ── Hint definition ───────────────────────────────────────────────────────────

struct HintDef {
    QString       key;           // unique hint identifier
    QString       title;         // bold first line
    QString       body;          // explanation
    QString       icon;          // SvgIcon name (optional)
    int           showAfterUses; // show when useCount < this
    int           showUntilDay;  // stop showing after N days of use
    bool          showOnFailure; // show immediately after recordFail()
    bool          celebratory;   // success milestone — different visual style
};

// ── Action chain ──────────────────────────────────────────────────────────────

struct ChainStep {
    QString key;
    QString hint;
    QString detailHint;   // extra detail shown after 2× the normal wait
    int     hintAfterMs;
    int     detailAfterMs = 0;
};

struct ActionChain {
    QString          id;
    QString          name;
    QList<ChainStep> steps;
    int              maxMinutes = 10; // abandon chain if not completed
};

// ── Milestone ─────────────────────────────────────────────────────────────────

struct Milestone {
    QString key;
    QString title;
    QString body;
    bool    achieved = false;
};

// ── The guide ─────────────────────────────────────────────────────────────────

class UxGuide : public QObject
{
    Q_OBJECT
    Q_PROPERTY(int globalScore READ globalScore NOTIFY globalScoreChanged)

public:
    static UxGuide &instance();

    // ── Lifecycle ─────────────────────────────────────────────────────────
    void onAppLaunched();         // call on every cold start
    void onSessionStarted();      // call when user unlocks/opens app
    void onSessionEnded();        // call on background/quit

    // ── Phase ─────────────────────────────────────────────────────────────
    OnboardingPhase currentPhase() const;
    int             daysSinceFirstLaunch() const;
    int             totalSessions() const;
    int             globalScore() const;   // 0–1000, aggregate of all features

    // ── Feature familiarity ───────────────────────────────────────────────
    void recordUse(const QString &featureKey);
    void recordFail(const QString &featureKey);   // user tried but failed/cancelled
    void recordIntroduction(const QString &key);  // hint was shown
    FamiliarityScore familiarityFor(const QString &key) const;
    FeatureState     stateFor(const QString &key) const;
    bool             needsGuidance(const QString &key) const;
    bool             shouldShowHint(const QString &hintKey) const;
    void             markHintSeen(const QString &hintKey);

    // ── Action chains ─────────────────────────────────────────────────────
    void startChain(const QString &chainId);
    void advanceChain(const QString &chainId, const QString &stepKey);
    void completeChain(const QString &chainId);
    void abandonChain(const QString &chainId);

    // ── Milestones ────────────────────────────────────────────────────────
    QList<Milestone> pendingMilestones(); // call after recordUse to collect
    void             clearPendingMilestones();

    // ── Confirmation gate ─────────────────────────────────────────────────
    bool confirmOrGate(const QString &featureKey, QWidget *anchor,
                       const QString &label, std::function<void()> onConfirm);

    // ── Contextual hint scheduling ─────────────────────────────────────────
    /// Schedule a hint near a widget after a delay. Respects cooldowns —
    /// won't show more than one hint per 2 minutes to avoid nagging.
    void scheduleHint(const QString &hintKey, QWidget *anchor, int delayMs = 0);

    /// Show a celebration (first snap sent, first contact added, etc.)
    void celebrate(const QString &milestoneKey, QWidget *parent);

signals:
    void phaseChanged(OnboardingPhase newPhase);
    void globalScoreChanged(int score);
    void hintReady(const QString &hintKey, const QString &title,
                   const QString &body,    QWidget *anchor);
    void chainStepHint(const QString &chainId, const QString &hint,
                       const QString &detail, int stepIndex);
    void milestoneAchieved(const QString &key, const QString &title,
                           const QString &body);
    void familiarityChanged(const QString &featureKey, int decayedScore);

private:
    explicit UxGuide(QObject *parent = nullptr);
    void load();
    void save();
    void registerBuiltinChains();
    void registerBuiltinHints();
    void registerBuiltinMilestones();
    void checkMilestones(const QString &featureKey);
    void updatePhase();
    int  computeGlobalScore() const;

    // Cooldown: don't hint more than once per kHintCooldownMs
    static constexpr int kHintCooldownMs = 2 * 60 * 1000;

    QHash<QString, FamiliarityScore> m_scores;
    QHash<QString, QElapsedTimer *>  m_chainTimers;
    QHash<QString, int>              m_chainSteps;
    QHash<QString, ActionChain>      m_chains;
    QHash<QString, HintDef>          m_hints;
    QHash<QString, Milestone>        m_milestones;
    QSet<QString>                    m_seenHints;
    QList<Milestone>                 m_pendingMilestones;

    qint64          m_firstLaunchMs   = 0;
    qint64          m_lastHintMs      = 0;
    int             m_totalSessions   = 0;
    OnboardingPhase m_phase           = OnboardingPhase::Discovery;
    QTimer         *m_phaseTimer      = nullptr;

    /// V32.34+: save-coalescing. recordUse() used to call save() directly
    /// on every page swipe — ~30 QSettings writes per swipe became the
    /// dominant cost in the gesture's tail. Now recordUse marks dirty
    /// and arms this 2-second debounced timer; save() runs once on the
    /// timer firing (or on app exit via flush()). The QSettings backend
    /// (INI file) doesn't tolerate concurrent writers, and our save is
    /// called only from the GUI thread, so no locking needed.
    QTimer         *m_saveTimer       = nullptr;
    bool            m_pendingSave     = false;

public:
    /// Force any pending save to flush NOW. Called on app shutdown so
    /// the last batch of recordUse() updates isn't lost if the user
    /// exits within the 2-second coalesce window.
    void flush();
};

// ── Feature key constants ─────────────────────────────────────────────────────

namespace Feature {
    // Navigation
    inline constexpr char kSwipe[]          = "swipe_pages";
    inline constexpr char kTabBar[]         = "tabbar_tap";

    // Camera + snaps
    inline constexpr char kCamera[]         = "camera_capture";
    inline constexpr char kSendSnap[]       = "send_snap";
    inline constexpr char kViewSnap[]       = "view_snap";
    inline constexpr char kSnapExpiry[]     = "snap_expiry";

    // Contacts
    inline constexpr char kAddContact[]     = "add_contact";
    inline constexpr char kQrScan[]         = "qr_scan";
    inline constexpr char kQrShare[]        = "qr_share";
    inline constexpr char kExportVCard[]    = "export_vcard";
    inline constexpr char kImportVCard[]    = "import_vcard";
    inline constexpr char kDeleteContact[]  = "delete_contact";

    // Network
    inline constexpr char kPollinator[]     = "pollinator_beacon";
    inline constexpr char kNearbyPeer[]     = "nearby_peer";
    inline constexpr char kManualConnect[]  = "manual_connect";

    // Settings
    inline constexpr char kSettings[]       = "settings_visit";
    inline constexpr char kThemeChange[]    = "theme_change";
    inline constexpr char kNetworkConfig[]  = "network_config";

    // Social
    inline constexpr char kInbox[]          = "inbox_visit";
    inline constexpr char kSnapReceived[]   = "snap_received";
    inline constexpr char kStreakDay[]      = "daily_streak";
}

// ── Hint key constants ────────────────────────────────────────────────────────

namespace Hint {
    // Day 0
    inline constexpr char kWelcome[]        = "hint_welcome";
    inline constexpr char kSwipeNav[]       = "hint_swipe_nav";
    inline constexpr char kCameraFirst[]    = "hint_camera_first";
    inline constexpr char kFriendsEmpty[]   = "hint_friends_empty";
    inline constexpr char kAddFirst[]       = "hint_add_first_contact";

    // Day 1-3
    inline constexpr char kQrExplain[]      = "hint_qr_explain";
    inline constexpr char kShareId[]        = "hint_share_id";
    inline constexpr char kSnapSent[]       = "hint_first_snap_sent";
    inline constexpr char kSnapExpiry[]     = "hint_snap_expiry";
    inline constexpr char kInboxEmpty[]     = "hint_inbox_empty";

    // Day 4-14
    inline constexpr char kPollinator[]     = "hint_pollinator";
    inline constexpr char kBeaconNear[]     = "hint_beacon_nearby";
    inline constexpr char kThemeHint[]      = "hint_theme";
    inline constexpr char kVcardExport[]    = "hint_vcard_export";
    inline constexpr char kVcardImport[]    = "hint_vcard_import";
    inline constexpr char kStreakStart[]    = "hint_streak_start";

    // Day 15-30
    inline constexpr char kPrivacyRemind[]  = "hint_privacy";
    inline constexpr char kNoCloudRemind[]  = "hint_no_cloud";
    inline constexpr char kP2PExplain[]     = "hint_p2p_explain";

    // Day 60+
    inline constexpr char kAmbassador[]     = "hint_ambassador";
    inline constexpr char kQrForFriends[]   = "hint_qr_for_friends";
}

// ── Milestone key constants ───────────────────────────────────────────────────

namespace MilestoneKey {
    inline constexpr char kFirstSnap[]      = "ms_first_snap";
    inline constexpr char kFirstContact[]   = "ms_first_contact";
    inline constexpr char kFirstReceived[]  = "ms_first_received";
    inline constexpr char kWeekStreak[]     = "ms_week_streak";
    inline constexpr char kTenSnaps[]       = "ms_ten_snaps";
    inline constexpr char kFiveContacts[]   = "ms_five_contacts";
    inline constexpr char kOneMonth[]       = "ms_one_month";
    inline constexpr char kExplorer[]       = "ms_explorer";   // tried all pages
    inline constexpr char kThemer[]         = "ms_themer";     // changed theme
    inline constexpr char kNetworker[]      = "ms_networker";  // used pollinator
}

} // namespace SnapClone::UI
