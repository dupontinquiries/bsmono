#pragma once

// ─── ui/wayfinder/WayfinderPage.hpp ──────────────────────────────────────────
//
// Personal cartography. Your contacts ARE the geography — not a basemap.
//
// Layout philosophy: this is NOT a road map. It's a wayfinder/dashboard in
// the spirit of a hiking GPS or astrolabe. The user is the center of the
// world; everyone else is positioned relative to them by bearing and
// (log-scaled) distance. No tiles, no ads, no commercial cartographer
// deciding what matters.
//
// What renders:
//   • Center dot (self) with subtle pulse animation
//   • Concentric distance rings (100m, 1km, 10km, 100km — log scale)
//   • Cardinal direction marks (N/E/S/W) at the edge
//   • Each contact: positioned by bearing & log(distance), shown as a circle
//     with their most recent snap as the avatar (or initials if no snap),
//     with online-status dot, name label, and "1.2km NW" distance text.
//   • Floating photos: snaps from contact X appear midway between (you, X)
//     when no third contact is nearby, OR between (X, Y) when X is roughly
//     on the path from you to Y. Capped at 5 total to avoid clutter.
//
// Repaints every 30s to refresh online state and locations. Tap on a contact
// to navigate to the friends page with them selected.
//
// All colors come from Theme:: so the page follows the active palette
// (works on minimal-dark grayscale, ocean, dracula, all 19 palettes).
//
// ─────────────────────────────────────────────────────────────────────────────

#include "core/Types.hpp"

#include <QWidget>
#include <QHash>
#include <QSet>
#include <QImage>
#include <QDateTime>

QT_BEGIN_NAMESPACE
class QTimer;
QT_END_NAMESPACE

namespace SnapClone::Core { class AppController; }

namespace SnapClone::UI {

class WayfinderPage : public QWidget
{
    Q_OBJECT
public:
    explicit WayfinderPage(SnapClone::Core::AppController *controller,
                           QWidget *parent = nullptr);
    ~WayfinderPage() override;

signals:
    /// User tapped a contact node — useful for "go to friends page with this
    /// peer selected" navigation. MainWindow wires this up.
    void contactTapped(SnapClone::Core::UserId id);

protected:
    void paintEvent(QPaintEvent *e) override;
    void mouseReleaseEvent(QMouseEvent *e) override;
    void showEvent(QShowEvent *e) override;
    void hideEvent(QHideEvent *e) override;
    void resizeEvent(QResizeEvent *e) override;
    bool event(QEvent *e) override;          ///< Translate touch → mouse for taps

private:
    /// Per-contact node: position, avatar, status, lifetime stats.
    /// Recomputed on refresh.
    struct ContactNode {
        SnapClone::Core::UserId id;
        QString  displayName;
        QPointF  pos;            ///< widget coordinates (after layout)
        double   bearingDeg;     ///< 0 = north, 90 = east
        double   distanceM;      ///< from self in meters
        QImage   avatar;         ///< thumb of recent snap, 96x96 cached (saliency-cropped)
        bool     online;
        QString  distanceLabel;  ///< "1.2km NW" — pre-formatted
        /// Distance fraction (0..1) relative to most-distant peer.
        /// Drives Hollywood scaling: avatar size, alpha, border width.
        qreal    distFraction = 0.0;
        /// Lifetime conversation stats. Drawn as a small "↑N ↓N" line under
        /// the distance label for nodes that aren't tiny (i.e. close enough
        /// to be readable). Builds the sense of relationship history at
        /// a glance.
        int      sentCount = 0;  ///< snaps WE sent to this peer
        int      recvCount = 0;  ///< snaps THEY sent to us
        /// "last seen" = most recent peer-location update OR most recent
        /// inbox snap from them. Used for offline contacts to show "2h ago".
        QDateTime lastSeen;
        /// Time of most recent inbound snap from this peer. Drives a brief
        /// "fresh snap" pulse animation on the node border. Independent of
        /// lastSeen because lastSeen also includes location updates which
        /// shouldn't pulse the avatar.
        qint64 freshSnapMs = 0;
        /// V32.34+: true when we have no peer-location fix for this contact
        /// but we still want to render them — typically because they're
        /// online over LAN right now. distanceM / bearingDeg / distFraction
        /// are unused for these; computeNodePositions places them in a
        /// "nearby" cluster near the center of the canvas. distanceLabel
        /// for these reads "Nearby" rather than "1.2km NW".
        bool noLocation = false;
    };

    /// Floating photo overlay: positioned somewhere on the canvas to express
    /// the spatial relationship between contacts.
    struct FloatingPhoto {
        QImage  thumb;           ///< 80x80 thumb cached
        QPointF pos;             ///< widget coordinates
        QString caption;         ///< short text, e.g. "alice"
        /// Distance fraction (0..1) of the OWNING contact relative to the
        /// most-distant contact on the canvas. Used to scale the photo's
        /// border thickness — close = thick border (alive), far = thin
        /// (ghostly). border = base * (1 - sqrt(distFrac)).
        qreal   distFraction = 0.0;
    };

    void refreshLayout();
    void loadContactsAndLocations();
    void computeNodePositions();
    void computeFloatingPhotos();
    QImage avatarForContact(const SnapClone::Core::UserId &id);
    QPointF projectToCanvas(double bearingDeg, double distanceM) const;
    static QString cardinalDirection(double bearingDeg);
    static QString formatDistance(double distanceM);
    /// Humanize a UTC timestamp into "just now", "5m ago", "2h ago", "3d ago".
    /// Returns empty for invalid timestamps so callers can omit the line.
    static QString humanizeAgo(const QDateTime &when);

    SnapClone::Core::AppController *m_controller = nullptr;
    QPointF        m_selfCenter;
    qreal          m_canvasRadius = 0;       ///< usable radius for projection
    QTimer        *m_refreshTimer = nullptr;
    /// V32.32: pulse-animation timer for online-indicator dots. Was a
    /// local-lambda-captured timer in the ctor; promoted to a member so
    /// hideEvent can stop() it, ending background CPU wakeups when the
    /// page isn't visible.
    QTimer        *m_pulseTimer   = nullptr;
    /// V32.34+ battery: app-foreground state. Stops the 30fps pulse
    /// timer when the user backgrounds the app even though our hideEvent
    /// won't fire (Qt doesn't issue hide on app-state change for child
    /// widgets of a top-level window that just lost focus). Subscribed
    /// to QGuiApplication::applicationStateChanged in the ctor.
    bool          m_appActive    = true;
    QHash<QString, ContactNode> m_nodes;     ///< keyed by contact id
    QList<FloatingPhoto>        m_photos;
    /// Self position. Without a fix, all peers are "unknown distance" and
    /// the page shows a friendly empty state.
    bool   m_haveSelfFix = false;
    double m_selfLat = 0.0;
    double m_selfLon = 0.0;
    /// Cache of avatar thumbnails (key = contact id) so we don't re-load
    /// JPEGs on every repaint.
    QHash<QString, QImage> m_avatarCache;
    /// Pulse phase (0..1) for self-dot animation
    qreal m_pulsePhase = 0.0;

    /// V32.34+ crash guard: track where a touch BEGAN on this page and whether
    /// it began here at all, so a TouchEnd that's actually the tail of a PAGE
    /// SWIPE (began elsewhere, or travelled far) is NOT mis-handled as a node
    /// tap. The fake mouseReleaseEvent synthesized from a swipe's TouchEnd was
    /// the suspected trigger for the swipe-onto-Wayfinder segfault; only
    /// firing it for a genuine in-page tap avoids that path entirely.
    bool   m_touchBeganHere = false;
    QPointF m_touchBeginPos;
    static constexpr qreal kTapMaxTravelPx = 16.0;  ///< beyond this = a swipe, not a tap

    /// Set of currently-online peer IDs. Updated reactively from
    /// AppController::peerConnected/peerDisconnected. The wayfinder repaints
    /// when this changes so the green online dot reflects real-time presence,
    /// not just stored ContactState::Known.
    QSet<QString> m_onlinePeers;

private slots:
    void onPeerConnected(SnapClone::Core::UserId id);
    void onPeerDisconnected(SnapClone::Core::UserId id);
    /// React to inbound snap: invalidate avatar cache for that contact
    /// so the next refresh shows the just-arrived snap, AND start a brief
    /// pulse animation on their node so the user notices.
    void onSnapReceived(SnapClone::Core::Snap snap);
};

} // namespace SnapClone::UI
