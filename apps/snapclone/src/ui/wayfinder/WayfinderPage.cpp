#include "ui/wayfinder/WayfinderPage.hpp"
#include "ui/Theme.hpp"
#include "core/AppController.hpp"
#include "storage/StorageManager.hpp"

#ifdef SNAPCLONE_RUST_SALIENCY
#  include "saliency/snapclone_saliency_ffi.h"
#endif

#include <QPainter>
#include <QPainterPath>
#include <QTimer>
#include <QMouseEvent>
#include <QTouchEvent>
#include <QPaintEvent>
#include <QFile>
#include <QtMath>
#include <QRandomGenerator>
#include <QLoggingCategory>
#include <QGuiApplication>  // V32.34+: app-state for battery gating
#include <algorithm>

namespace SnapClone::UI {

Q_LOGGING_CATEGORY(lcWay, "snapclone.wayfinder")

namespace {

/// Haversine bearing from (lat1,lon1) to (lat2,lon2), 0 = north, clockwise.
double bearingDeg(double lat1, double lon1, double lat2, double lon2)
{
    const double phi1 = qDegreesToRadians(lat1);
    const double phi2 = qDegreesToRadians(lat2);
    const double dLon = qDegreesToRadians(lon2 - lon1);
    const double y = std::sin(dLon) * std::cos(phi2);
    const double x = std::cos(phi1) * std::sin(phi2)
                   - std::sin(phi1) * std::cos(phi2) * std::cos(dLon);
    double brg = qRadiansToDegrees(std::atan2(y, x));
    if (brg < 0) brg += 360.0;
    return brg;
}

/// Haversine distance in meters between two WGS-84 points.
double haversineM(double lat1, double lon1, double lat2, double lon2)
{
    constexpr double kEarthRadiusM = 6'371'000.0;
    const double phi1 = qDegreesToRadians(lat1);
    const double phi2 = qDegreesToRadians(lat2);
    const double dPhi = qDegreesToRadians(lat2 - lat1);
    const double dLam = qDegreesToRadians(lon2 - lon1);
    const double a = std::sin(dPhi / 2) * std::sin(dPhi / 2)
                   + std::cos(phi1) * std::cos(phi2)
                   * std::sin(dLam / 2) * std::sin(dLam / 2);
    return 2 * kEarthRadiusM * std::asin(std::sqrt(a));
}

} // anon

// ─── Construction ────────────────────────────────────────────────────────────

WayfinderPage::WayfinderPage(SnapClone::Core::AppController *controller, QWidget *parent)
    : QWidget(parent)
    , m_controller(controller)
{
    // Page must accept touch directly so a tap on a contact node works on
    // mobile. SwipeNavigator's interactive-widget check (lines added earlier)
    // doesn't help here because we're not a QAbstractButton — we're a custom
    // canvas with hit-testing inside paintEvent.
    setAttribute(Qt::WA_AcceptTouchEvents);

    m_refreshTimer = new QTimer(this);
    m_refreshTimer->setInterval(30 * 1000);  // 30s — locations don't change fast
    connect(m_refreshTimer, &QTimer::timeout, this, &WayfinderPage::refreshLayout);

    // Pulse animation timer — 30 fps for the center dot
    m_pulseTimer = new QTimer(this);
    m_pulseTimer->setInterval(33);
    connect(m_pulseTimer, &QTimer::timeout, this, [this]() {
        // V32.32: defensive isVisible check. Belt-and-braces with the
        // stop()-on-hide below — if a signal cascade ever triggers the
        // timer between hideEvent and the actual stop() taking effect,
        // we still don't waste work on a hidden widget.
        if (!isVisible()) return;
        m_pulsePhase += 0.01;
        if (m_pulsePhase > 1.0) m_pulsePhase -= 1.0;
        update();
    });
    // V32.34+ battery: stop the 30fps pulse when the app is backgrounded.
    // hideEvent does NOT fire for child widgets when the top-level
    // window loses focus to another app — Qt keeps the page "visible"
    // in widget terms, so without this signal the timer would keep
    // running while the phone is in the user's pocket.
    if (auto *gui = qobject_cast<QGuiApplication *>(QGuiApplication::instance())) {
        m_appActive = (gui->applicationState() == Qt::ApplicationActive
                    || gui->applicationState() == Qt::ApplicationInactive);
        connect(gui, &QGuiApplication::applicationStateChanged,
                this, [this](Qt::ApplicationState s) {
            const bool nowActive = (s == Qt::ApplicationActive
                                  || s == Qt::ApplicationInactive);
            if (nowActive == m_appActive) return;
            m_appActive = nowActive;
            if (!m_pulseTimer) return;
            if (m_appActive && isVisible()) m_pulseTimer->start();
            else                            m_pulseTimer->stop();
        });
    }
    // Note: timer is started in showEvent (not here) so it only runs
    // while the page is actually visible. See V32.32 perf optimization
    // — previously this timer fired at 30Hz forever, generating
    // background CPU wakeups even when the user was on Camera or Inbox.

    // Real-time presence: forward AppController signals to our slot. When a
    // peer connects/disconnects we update m_onlinePeers and repaint so the
    // green dot reflects actual reachability rather than stored state.
    if (m_controller) {
        connect(m_controller, &Core::AppController::peerConnected,
                this, &WayfinderPage::onPeerConnected);
        connect(m_controller, &Core::AppController::peerDisconnected,
                this, &WayfinderPage::onPeerDisconnected);
        connect(m_controller, &Core::AppController::snapReceived,
                this, &WayfinderPage::onSnapReceived);
    }
}

void WayfinderPage::onSnapReceived(SnapClone::Core::Snap snap)
{
    // Invalidate avatar cache so the next refresh picks up the new snap
    // (50/50 logic in avatarForContact may swap to it).
    m_avatarCache.remove(snap.senderId.value);

    // Mark the node as "fresh" — paintEvent uses this to pulse the border.
    // The pulse is timestamp-based (decays over 2.5s) so we don't need a
    // separate timer per peer; the existing pulseTimer's repaint loop
    // animates whatever is currently fresh.
    if (auto it = m_nodes.find(snap.senderId.value); it != m_nodes.end()) {
        it->freshSnapMs = QDateTime::currentMSecsSinceEpoch();
        ++it->recvCount;  // optimistic — refresh will reconcile from storage
        it->lastSeen = QDateTime::currentDateTimeUtc();
    } else if (snap.hasLocation()) {
        // V32.28: snap from a peer NOT currently on the map AND it carries
        // a location fix. SnapRouter::cacheSnapSenderLocation has already
        // written to peer_locations, but our in-memory m_nodes won't see
        // it until the next 30s refresh tick. Force a refreshLayout so
        // the new pin appears immediately — matches user expectation
        // that "they just snapped you, you should see where they are."
        //
        // Filed in the TODO as "🧪 Wayfinder shows nothing after
        // receiving snaps from peers." The cache write WAS happening;
        // the gap was the rebuild trigger. Two cases naturally produce
        // this: first contact who never previously shared location, or
        // a peer who moved networks since last refresh and we'd skipped
        // them as no-location.
        qCInfo(lcWay) << "Snap from new-on-map peer"
                      << snap.senderId.value
                      << "with location — refreshing layout";
        refreshLayout();
    }
    // No update() call for the existing-node path — the 30fps pulseTimer
    // already triggers paints.
}

void WayfinderPage::onPeerConnected(SnapClone::Core::UserId id)
{
    m_onlinePeers.insert(id.value);
    // Update the matching ContactNode's online flag if it exists; the next
    // repaint will pick it up. We don't refreshLayout() because that would
    // re-load every avatar from disk.
    if (m_nodes.contains(id.value)) {
        m_nodes[id.value].online = true;
        update();
        return;
    }
    // V32.28: peer not yet on the map. If we have a stored location for
    // them (from a prior session — peer_locations persists across
    // restarts), they SHOULD appear. Force a refresh so the pin shows
    // up immediately rather than waiting up to 30s for the timer.
    // Avatars are cached after first paint so this is cheap on repeat.
    if (m_controller && m_controller->storageManager()) {
        if (m_controller->storageManager()->loadPeerLocation(id.value).has_value()) {
            qCInfo(lcWay) << "Peer" << id.value
                          << "came online with stored location — refreshing layout";
            refreshLayout();
        }
    }
}

void WayfinderPage::onPeerDisconnected(SnapClone::Core::UserId id)
{
    m_onlinePeers.remove(id.value);
    if (m_nodes.contains(id.value)) {
        m_nodes[id.value].online = false;
        update();
    }
}

WayfinderPage::~WayfinderPage() = default;

void WayfinderPage::showEvent(QShowEvent *e)
{
    QWidget::showEvent(e);
    refreshLayout();
    m_refreshTimer->start();
    // V32.32: only run the 30Hz pulse animation while the page is visible.
    // Saves ~30 CPU wakeups/sec on every other page (Camera, Inbox, etc).
    // V32.34+ battery: also gate on appActive — no point pulsing pixels
    // when the user can't see them.
    if (m_pulseTimer && m_appActive) m_pulseTimer->start();
}

void WayfinderPage::hideEvent(QHideEvent *e)
{
    QWidget::hideEvent(e);
    m_refreshTimer->stop();
    if (m_pulseTimer) m_pulseTimer->stop();
}

void WayfinderPage::resizeEvent(QResizeEvent *e)
{
    QWidget::resizeEvent(e);
    computeNodePositions();
    computeFloatingPhotos();
}

// ─── Touch routing ───────────────────────────────────────────────────────────

bool WayfinderPage::event(QEvent *e)
{
    // We accept touch events directly so SwipeNavigator (the ancestor with
    // WA_AcceptTouchEvents) doesn't claim them. Translate to mouse semantics
    // for our existing mouseReleaseEvent handler — simpler than handling
    // touch state machine ourselves.
    //
    // V32.34+ crash guard: only synthesize a node-tap from a TouchEnd that was
    // a GENUINE in-page tap — i.e. the touch BEGAN on this page AND barely
    // moved (kTapMaxTravelPx). A page SWIPE that lands on Wayfinder also ends
    // with a TouchEnd delivered here, and turning that into a fake
    // mouseReleaseEvent during the slide-in was the suspected trigger for the
    // swipe-onto-Wayfinder segfault. A swipe travels far, so the travel check
    // rejects it; a tap that began on a different page (m_touchBeganHere false)
    // is also rejected.
    if (e->type() == QEvent::TouchBegin) {
        auto *te = static_cast<QTouchEvent *>(e);
        m_touchBeganHere = !te->points().isEmpty();
        if (m_touchBeganHere)
            m_touchBeginPos = te->points().first().position();
        // Accept so we get TouchEnd. Don't translate to mousePress because
        // we only act on release (tap = press+release within node bounds).
        e->accept();
        return true;
    }
    if (e->type() == QEvent::TouchEnd || e->type() == QEvent::TouchCancel) {
        auto *te = static_cast<QTouchEvent *>(e);
        const bool wasTap =
            (e->type() == QEvent::TouchEnd)
            && m_touchBeganHere
            && !te->points().isEmpty()
            && QLineF(m_touchBeginPos,
                      te->points().first().position()).length() <= kTapMaxTravelPx;
        m_touchBeganHere = false;   // reset for the next sequence
        if (wasTap) {
            const QPointF pos = te->points().first().position();
            QMouseEvent fake(QEvent::MouseButtonRelease, pos,
                             mapToGlobal(pos.toPoint()),
                             Qt::LeftButton, Qt::NoButton, Qt::NoModifier);
            mouseReleaseEvent(&fake);
            e->accept();
            return true;
        }
        // Not a tap (a swipe tail, or began off-page) — let it propagate so
        // SwipeNavigator's gesture handling completes normally.
        return QWidget::event(e);
    }
    return QWidget::event(e);
}

void WayfinderPage::mouseReleaseEvent(QMouseEvent *e)
{
    // V32.34+: hit test against contact nodes with size-aware radius.
    // The old fixed 40px was tuned for 80px circles. After the
    // kAvatarMax reduction (96→76) AND the new "nearby" cluster where
    // noLocation nodes are rendered at the middle size band, a fixed
    // radius makes near-by nodes hard to hit (small radius vs node)
    // OR makes far-away nodes ambiguous (large radius overlaps
    // neighbors). The avatar's effective rendered size IS available
    // — we computed distFraction for each node — so we use the same
    // formula here to derive a per-node hit radius.
    constexpr qreal kAvatarMax = 72.0;
    constexpr qreal kAvatarMin = 32.0;
    for (const auto &node : m_nodes) {
        const qreal avatarSize = kAvatarMax + (kAvatarMin - kAvatarMax) * node.distFraction;
        // Hit radius = half the rendered avatar size, +6px slop for
        // fingertip touch — easier to hit on phones than the literal
        // circle would suggest.
        const qreal hitRadius = (avatarSize / 2.0) + 6.0;
        if (QLineF(e->pos(), node.pos).length() <= hitRadius) {
            qCInfo(lcWay) << "Tapped contact node:" << node.displayName;
            emit contactTapped(node.id);
            return;
        }
    }
    QWidget::mouseReleaseEvent(e);
}

// ─── Data refresh ────────────────────────────────────────────────────────────

void WayfinderPage::refreshLayout()
{
    loadContactsAndLocations();
    computeNodePositions();
    computeFloatingPhotos();
    update();
}

void WayfinderPage::loadContactsAndLocations()
{
    if (!m_controller || !m_controller->storageManager()) return;
    auto *storage = m_controller->storageManager();

    // Self position
    const Core::UserId selfId{m_controller->stableId()};
    if (const auto loc = m_controller->lastKnownLocation(selfId)) {
        m_haveSelfFix = true;
        m_selfLat = loc->lat;
        m_selfLon = loc->lon;
    } else {
        m_haveSelfFix = false;
    }

    // Contacts
    const auto contacts = storage->loadContacts(selfId);
    m_nodes.clear();

    // V32.34+: previously this method returned early when m_haveSelfFix
    // was false — leaving Wayfinder empty for users who haven't enabled
    // location or are indoors. That was correct for the geographic-map
    // view but defeated the wayfinder's role as a presence panel. Now
    // we keep going: contacts without GPS data still render in a
    // "nearby" cluster (positioned by computeNodePositions) so the user
    // sees who's reachable even without geolocation.
    for (const auto &c : contacts) {
        const auto loc = storage->loadPeerLocation(c.id.value);

        ContactNode node;
        node.id           = c.id;
        node.displayName  = c.displayName.isEmpty() ? c.id.value : c.displayName;
        node.online       = m_onlinePeers.contains(c.id.value);
        node.avatar       = avatarForContact(c.id);

        // V32.34+: branch on whether we have BOTH our own fix AND a
        // location for this contact. If either is missing, mark
        // noLocation=true and the layout will place this node in the
        // "nearby" zone. Otherwise compute the geographic distance/bearing
        // as before.
        if (m_haveSelfFix && loc.has_value()) {
            node.bearingDeg    = bearingDeg(m_selfLat, m_selfLon, loc->lat, loc->lon);
            node.distanceM     = haversineM(m_selfLat, m_selfLon, loc->lat, loc->lon);
            node.distanceLabel = formatDistance(node.distanceM)
                               + QStringLiteral(" ") + cardinalDirection(node.bearingDeg);
        } else {
            // No GPS-derived position. Still surface the contact so the
            // user can SEE them on the page. computeNodePositions reads
            // noLocation and places these in a center-ish cluster.
            node.noLocation    = true;
            node.bearingDeg    = 0.0;
            node.distanceM     = 0.0;
            node.distanceLabel = node.online ? tr("Nearby") : tr("Offline");
        }
        m_nodes.insert(c.id.value, node);
    }

    // ── Compute per-contact lifetime stats ──────────────────────────────
    // Single pass over inbox + outbox snaps. O(N) but inbox is small.
    // Builds the "↑12  ↓7" relationship summary shown under each node.
    {
        const auto inbox  = storage->loadInboxSnaps(selfId);
        const auto outbox = storage->loadOutboxSnaps(selfId);

        for (const auto &s : inbox) {
            if (auto it = m_nodes.find(s.senderId.value); it != m_nodes.end()) {
                ++it->recvCount;
                if (s.sentAt > it->lastSeen) it->lastSeen = s.sentAt;
            }
        }
        for (const auto &s : outbox) {
            if (auto it = m_nodes.find(s.recipientId.value); it != m_nodes.end()) {
                ++it->sentCount;
            }
        }

        // Also use peer-location recordedAt as a "last seen" candidate. A
        // peer who shared their location 5min ago is "more recent" than
        // a peer whose last snap was an hour ago.
        for (auto &node : m_nodes) {
            const auto loc = storage->loadPeerLocation(node.id.value);
            if (loc.has_value() && loc->recordedAt > node.lastSeen) {
                node.lastSeen = loc->recordedAt;
            }
        }
    }

    // Now compute distFraction for each node — used for Hollywood scaling.
    // distFraction = node.distanceM / maxNodeDistanceM, clamped to [0..1].
    // Falls to 0 (you're "right here") and rises to 1 (most distant peer).
    double maxDist = 0.0;
    for (const auto &n : m_nodes) {
        if (n.distanceM > maxDist) maxDist = n.distanceM;
    }
    if (maxDist > 0.0) {
        for (auto &n : m_nodes) {
            n.distFraction = qBound(0.0, n.distanceM / maxDist, 1.0);
        }
    }
    // V32.34+: noLocation nodes don't have a meaningful distance, but
    // we still want their avatars sized reasonably. Defaulting them to
    // distFraction=0 makes them render at kAvatarMax (96px), which is
    // ridiculously large for the "nearby" cluster — overlapping
    // avatars all the way around the small ring. Set them to a
    // middle-ish value so they render at the "medium" size band
    // (~60px). 0.55 picked to be a tad past the midpoint so they look
    // smaller than any geolocated near-peer.
    for (auto &n : m_nodes) {
        if (n.noLocation) n.distFraction = 0.55;
    }
}

QImage WayfinderPage::avatarForContact(const SnapClone::Core::UserId &id)
{
    if (m_avatarCache.contains(id.value)) return m_avatarCache[id.value];
    if (!m_controller) return {};

    auto *storage = m_controller->storageManager();
    if (!storage) return {};

    // Collect all inbox snaps from this peer that have a local file on disk.
    // 50/50 between most-recent and a random one — the user explicitly wanted
    // some randomness so the wayfinder feels alive over time, but recent
    // snaps still dominate so people who just sent something are recognizable.
    const Core::UserId selfId{m_controller->stableId()};
    const auto inbox = storage->loadInboxSnaps(selfId);

    QList<Core::Snap> mySnaps;
    for (const auto &s : inbox) {
        if (s.senderId.value != id.value) continue;
        if (s.hasVideo()) continue;  // video thumbs aren't usable as static avatars
        if (s.localPath.isEmpty() || !QFile::exists(s.localPath)) continue;
        mySnaps.append(s);
    }
    if (mySnaps.isEmpty()) {
        m_avatarCache.insert(id.value, QImage{});
        return {};
    }

    // Sort newest-first
    std::sort(mySnaps.begin(), mySnaps.end(),
              [](const Core::Snap &a, const Core::Snap &b) {
        return a.sentAt > b.sentAt;
    });

    // 50/50: half the time use the most recent, half the time pick at random
    // from the available pool. With one snap, the choice degenerates to it.
    QString chosenPath;
    if (mySnaps.size() == 1 || QRandomGenerator::global()->bounded(2) == 0) {
        chosenPath = mySnaps.first().localPath;
    } else {
        const int idx = QRandomGenerator::global()->bounded(static_cast<int>(mySnaps.size()));
        chosenPath = mySnaps.at(idx).localPath;
    }

    QImage img(chosenPath);
    if (img.isNull()) {
        m_avatarCache.insert(id.value, QImage{});
        return {};
    }

    // Crop to a square region. We try to use Rust saliency to find the most
    // visually-interesting area; fall back to center crop if that's unavailable
    // (build option SNAPCLONE_RUST_SALIENCY=OFF) or if it returns nothing.
    QRect cropRect;
#ifdef SNAPCLONE_RUST_SALIENCY
    {
        // Saliency expects RGBA8888. Convert if needed.
        const QImage rgba = img.convertToFormat(QImage::Format_RGBA8888);
        const SalRect sb = sc_saliency_bbox(rgba.constBits(),
                                            static_cast<uint32_t>(rgba.width()),
                                            static_cast<uint32_t>(rgba.height()));
        if (sb.w > 0 && sb.h > 0) {
            // Expand the bbox to a square (largest dimension), centered on
            // the salient region's center. This preserves aspect for the
            // 64x64 final scale and adds a bit of context around the subject.
            const int salCx = static_cast<int>(sb.x + sb.w / 2);
            const int salCy = static_cast<int>(sb.y + sb.h / 2);
            int side = static_cast<int>(qMax(sb.w, sb.h));
            // Add 20% context padding around the salient region
            side = static_cast<int>(side * 1.2);
            // Clamp side so the square fits inside the image
            side = qMin(side, qMin(img.width(), img.height()));
            int x0 = salCx - side / 2;
            int y0 = salCy - side / 2;
            // Slide back into image bounds if necessary
            x0 = qBound(0, x0, img.width()  - side);
            y0 = qBound(0, y0, img.height() - side);
            cropRect = QRect(x0, y0, side, side);
        }
    }
#endif
    if (!cropRect.isValid()) {
        // Center crop fallback
        const int side = qMin(img.width(), img.height());
        cropRect = QRect((img.width() - side) / 2,
                          (img.height() - side) / 2,
                          side, side);
    }

    const QImage cropped = img.copy(cropRect);
    const QImage avatar = cropped.scaled(96, 96,
                                          Qt::IgnoreAspectRatio,
                                          Qt::SmoothTransformation);
    m_avatarCache.insert(id.value, avatar);
    return avatar;
}

// ─── Layout math ─────────────────────────────────────────────────────────────

QPointF WayfinderPage::projectToCanvas(double brgDeg, double distM) const
{
    // Log scale: distance 0 → center, distance ∞ → edge, smooth in between.
    // Anchor: 1km maps to ~50% of canvas radius. Clamp so very far peers
    // sit on the edge ring rather than going off-screen.
    constexpr double kAnchorM = 1000.0;     // 1km = midway out
    if (distM < 1.0) return m_selfCenter;   // essentially at self

    const double r = m_canvasRadius;
    // log10(distM/anchor) maps:
    //   100m  → -1   → 0.25 of radius
    //   1km   →  0   → 0.50
    //   10km  →  1   → 0.75
    //   100km →  2   → 1.00 (clamped)
    const double frac = qBound(0.05, 0.5 + 0.25 * std::log10(distM / kAnchorM), 1.0);

    const double brgRad = qDegreesToRadians(brgDeg - 90.0);  // 0deg = north (up)
    return m_selfCenter + QPointF(std::cos(brgRad) * r * frac,
                                   std::sin(brgRad) * r * frac);
}

void WayfinderPage::computeNodePositions()
{
    m_selfCenter = QPointF(width() / 2.0, height() / 2.0);
    // Leave margin for cardinal labels and distance text under nodes.
    m_canvasRadius = qMin(width(), height()) / 2.0 - 56.0;

    // V32.34+: collect noLocation nodes for the "nearby" cluster.
    // We arrange them in a tight ring around the self-center at a
    // small radius (~25% of canvas radius). This keeps them visually
    // distinct from geolocated contacts (which can be anywhere on
    // the canvas) while still letting the user tap them.
    QList<QString> nearbyIds;
    for (auto it = m_nodes.constBegin(); it != m_nodes.constEnd(); ++it) {
        if (it->noLocation) nearbyIds.append(it.key());
    }

    // Place geolocated nodes first via the bearing/distance projection.
    for (auto &node : m_nodes) {
        if (!node.noLocation) {
            node.pos = projectToCanvas(node.bearingDeg, node.distanceM);
        }
    }

    // Now place nearby nodes evenly around a small ring. The ring
    // radius is 25% of canvas radius — close enough to feel "nearby"
    // visually but with room to display each avatar without overlap
    // up to ~8 nodes. With more than 8 nodes we still place them on
    // the same ring (they overlap slightly, which reads as a
    // "cluster" of people right here).
    const qreal nearbyRadius = m_canvasRadius * 0.25;
    const int n = nearbyIds.size();
    for (int i = 0; i < n; ++i) {
        // Distribute evenly from -90° (top) clockwise. Single nearby
        // node sits at the top of the ring (12 o'clock), reading
        // naturally as "right here next to you".
        const qreal angleDeg = -90.0 + (360.0 * i) / qMax(1, n);
        const qreal angleRad = qDegreesToRadians(angleDeg);
        m_nodes[nearbyIds[i]].pos = m_selfCenter
            + QPointF(nearbyRadius * std::cos(angleRad),
                      nearbyRadius * std::sin(angleRad));
    }
}

void WayfinderPage::computeFloatingPhotos()
{
    m_photos.clear();
    if (m_nodes.isEmpty()) return;

    auto *storage = m_controller ? m_controller->storageManager() : nullptr;
    if (!storage) return;

    const Core::UserId selfId{m_controller->stableId()};
    const auto inbox = storage->loadInboxSnaps(selfId);

    // Map sender → list of recent snaps with local files
    QHash<QString, QList<Core::Snap>> bySender;
    for (const auto &s : inbox) {
        if (s.localPath.isEmpty() || !QFile::exists(s.localPath)) continue;
        if (s.hasVideo()) continue;  // video thumbs aren't usable here
        bySender[s.senderId.value].append(s);
    }

    // Pick up to 5 floating photos. For each contact, place a recent snap
    // either at the midpoint between (you, X), OR (when geometry allows)
    // between two contacts X and Y where Y is roughly on path from you to X.
    int placed = 0;
    constexpr int kMaxFloatingPhotos = 5;
    auto nodeList = m_nodes.values();

    for (const auto &node : nodeList) {
        if (placed >= kMaxFloatingPhotos) break;
        const auto &snaps = bySender.value(node.id.value);
        if (snaps.isEmpty()) continue;
        const auto &snap = snaps.first();  // most recent

        // Try to find a contact "between" us and node — same bearing within
        // ±20° AND distance < node's. If found, place photo at that midpoint.
        QPointF placement = (m_selfCenter + node.pos) * 0.5;  // default
        for (const auto &other : nodeList) {
            if (other.id.value == node.id.value) continue;
            const double bearingDiff = std::abs(
                std::fmod(other.bearingDeg - node.bearingDeg + 540.0, 360.0) - 180.0);
            // Bearing close (within 20°) and distance smaller — "between"
            if (bearingDiff < 20.0 && other.distanceM < node.distanceM * 0.8) {
                placement = (other.pos + node.pos) * 0.5;
                break;
            }
        }

        QImage img(snap.localPath);
        if (img.isNull()) continue;
        const int side = qMin(img.width(), img.height());
        FloatingPhoto fp;
        fp.thumb = img.copy((img.width() - side) / 2, (img.height() - side) / 2,
                            side, side)
                      .scaled(56, 56, Qt::IgnoreAspectRatio, Qt::SmoothTransformation);
        fp.pos = placement;
        fp.caption = node.displayName;
        // Inherit distance fraction from the owning contact — drives the
        // half-exponent border-thickness scaling in paintEvent. Close-from-you
        // photos get THICK borders (alive, present); far photos thin/ghostly.
        fp.distFraction = node.distFraction;
        m_photos.append(fp);
        ++placed;
    }
}

// ─── Painting ────────────────────────────────────────────────────────────────

QString WayfinderPage::cardinalDirection(double brgDeg)
{
    static const QStringList dirs = {
        QStringLiteral("N"),  QStringLiteral("NE"),
        QStringLiteral("E"),  QStringLiteral("SE"),
        QStringLiteral("S"),  QStringLiteral("SW"),
        QStringLiteral("W"),  QStringLiteral("NW")
    };
    const int idx = static_cast<int>((brgDeg + 22.5) / 45.0) % 8;
    return dirs.at(idx);
}

QString WayfinderPage::formatDistance(double m)
{
    if (m < 1000.0) return QStringLiteral("%1m").arg(static_cast<int>(m));
    if (m < 100000.0) return QStringLiteral("%1km").arg(m / 1000.0, 0, 'f', 1);
    return QStringLiteral("%1km").arg(static_cast<int>(m / 1000.0));
}

QString WayfinderPage::humanizeAgo(const QDateTime &when)
{
    if (!when.isValid()) return QString();
    const qint64 sec = when.secsTo(QDateTime::currentDateTimeUtc());
    if (sec < 0)         return tr("just now");      // clock skew safety
    if (sec < 90)        return tr("just now");
    if (sec < 3600)      return tr("%1m ago").arg(sec / 60);
    if (sec < 86400)     return tr("%1h ago").arg(sec / 3600);
    if (sec < 86400 * 7) return tr("%1d ago").arg(sec / 86400);
    if (sec < 86400 * 30)return tr("%1w ago").arg(sec / (86400 * 7));
    return tr("long ago");
}

void WayfinderPage::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing);

    // ── Background ──────────────────────────────────────────────────────────
    //
    // V32.32: cached color resolution. Was 7 fresh QColor(QString) constructions
    // per paint event — at 30Hz that's ~210 string-to-color parses per second.
    // The colors only change when the active palette changes (rare; happens on
    // theme switch or ambient-mode tick), so we cache them and refresh only
    // when the underlying palette identity changes.
    //
    // Detection: we hash the active palette name + the first byte of BG_BASE
    // as a cheap fingerprint. False positives (same hash, different palette)
    // are vanishingly rare in practice; false negatives (different hash, same
    // palette) just re-resolve unnecessarily — both modes are safe.
    static QString s_lastPaletteFingerprint;
    static QColor s_bg, s_card, s_accent, s_text, s_text2, s_border, s_success;
    const QString fingerprint =
        QString::fromUtf8(Theme::BG_BASE()) + QChar('|') +
        QString::fromUtf8(Theme::ACCENT());
    if (fingerprint != s_lastPaletteFingerprint) {
        s_lastPaletteFingerprint = fingerprint;
        s_bg      = QColor(QString::fromLatin1(Theme::BG_BASE()));
        s_card    = QColor(QString::fromLatin1(Theme::BG_CARD()));
        s_accent  = QColor(QString::fromLatin1(Theme::ACCENT()));
        s_text    = QColor(QString::fromLatin1(Theme::TEXT_PRIMARY()));
        s_text2   = QColor(QString::fromLatin1(Theme::TEXT_SECONDARY()));
        s_border  = QColor(QString::fromLatin1(Theme::ACCENT_DIM()));
        s_success = QColor(QString::fromLatin1(Theme::SUCCESS()));
    }
    const QColor &bg      = s_bg;
    const QColor &card    = s_card;
    const QColor &accent  = s_accent;
    const QColor &text    = s_text;
    const QColor &text2   = s_text2;
    const QColor &border  = s_border;
    const QColor &success = s_success;

    p.fillRect(rect(), bg);

    // ── Empty-state when no GPS fix ─────────────────────────────────────────
    if (!m_haveSelfFix) {
        p.setPen(text);
        QFont f = p.font();
        f.setPointSize(14);
        p.setFont(f);
        p.drawText(rect(), Qt::AlignCenter,
            tr("No location fix yet.\n\nWayfinder needs your GPS to position friends.\n"
               "Set a manual location in Settings → Privacy if you don't have GPS."));
        return;
    }

    // ── Distance rings (Garmin-style topo grid) ─────────────────────────────
    p.setPen(QPen(border, 1, Qt::DashLine));
    p.setBrush(Qt::NoBrush);
    // Draw rings at 25%, 50%, 75%, 100% of canvas radius
    for (int i = 1; i <= 4; ++i) {
        const qreal r = m_canvasRadius * (i / 4.0);
        p.drawEllipse(m_selfCenter, r, r);
    }
    // Distance ring labels (small text near top of each ring)
    p.setPen(text2);
    QFont smallFont = p.font();
    smallFont.setPointSize(8);
    p.setFont(smallFont);
    // Ring distances correspond to log scale: 25% → ~100m, 50% → 1km, 75% → 10km, 100% → 100km
    const QStringList labels = {
        QStringLiteral("100m"), QStringLiteral("1km"),
        QStringLiteral("10km"), QStringLiteral("100km")
    };
    for (int i = 0; i < 4; ++i) {
        const qreal r = m_canvasRadius * ((i + 1) / 4.0);
        p.drawText(QPointF(m_selfCenter.x() + 4,
                            m_selfCenter.y() - r - 4),
                    labels[i]);
    }

    // ── Cardinal direction marks (N/E/S/W at edge) ──────────────────────────
    p.setPen(text);
    QFont cardinalFont = p.font();
    cardinalFont.setPointSize(11);
    cardinalFont.setBold(true);
    p.setFont(cardinalFont);
    const qreal cr = m_canvasRadius + 32;
    p.drawText(QRectF(m_selfCenter.x() - 12, m_selfCenter.y() - cr - 14, 24, 16),
                Qt::AlignCenter, QStringLiteral("N"));
    p.drawText(QRectF(m_selfCenter.x() - 12, m_selfCenter.y() + cr - 2, 24, 16),
                Qt::AlignCenter, QStringLiteral("S"));
    p.drawText(QRectF(m_selfCenter.x() + cr - 12, m_selfCenter.y() - 8, 24, 16),
                Qt::AlignCenter, QStringLiteral("E"));
    p.drawText(QRectF(m_selfCenter.x() - cr - 12, m_selfCenter.y() - 8, 24, 16),
                Qt::AlignCenter, QStringLiteral("W"));

    // ── Lines from self to each contact (subtle accent) ─────────────────────
    p.setPen(QPen(border, 1));
    for (const auto &node : m_nodes) {
        p.drawLine(m_selfCenter, node.pos);
    }

    // ── Floating photos (drawn first so nodes overlay them) ────────────────
    //
    // Half-exponent border-thickness rule per user spec:
    //   borderWidth = base * (1 - sqrt(distFraction))
    // distFraction is the OWNING contact's distance/maxDistance ratio.
    //
    //   distFraction=0    → border = base       (thick — photo is close to you)
    //   distFraction=0.25 → border = 0.5 * base
    //   distFraction=1.0  → border = 0          (clamped to ≥0.5px so it stays visible)
    //
    // The visual effect: photos from close friends feel solid and present;
    // photos from far friends look ghostly, like they're drifting away.
    constexpr qreal kPhotoBorderBase = 6.0;
    for (const auto &fp : m_photos) {
        const QRectF r(fp.pos.x() - 28, fp.pos.y() - 28, 56, 56);
        QPainterPath clip;
        clip.addRoundedRect(r, 8, 8);
        p.save();
        p.setClipPath(clip);
        p.drawImage(r, fp.thumb);
        p.restore();
        const qreal borderW = qMax(0.5, kPhotoBorderBase * (1.0 - std::sqrt(fp.distFraction)));
        p.setPen(QPen(accent, borderW));
        p.setBrush(Qt::NoBrush);
        p.drawRoundedRect(r, 8, 8);
    }

    // ── Self center dot with pulse ──────────────────────────────────────────
    const qreal pulseR = 12 + 6 * std::sin(m_pulsePhase * 2 * M_PI);
    QColor pulseColor = accent;
    pulseColor.setAlphaF(0.3f);
    p.setPen(Qt::NoPen);
    p.setBrush(pulseColor);
    p.drawEllipse(m_selfCenter, pulseR, pulseR);
    p.setBrush(accent);
    p.drawEllipse(m_selfCenter, 8, 8);
    p.setPen(text);
    QFont selfFont = p.font();
    selfFont.setPointSize(9);
    selfFont.setBold(false);
    p.setFont(selfFont);
    p.drawText(QPointF(m_selfCenter.x() + 12, m_selfCenter.y() + 4),
                tr("you"));

    // ── Contact nodes (Hollywood-scaled by distance) ───────────────────────
    //
    // Garmin GPS for walkers, but with cinematic emphasis: close friends look
    // BIG and PROMINENT (they're in your life right now), distant friends look
    // small and faded (they're on the periphery). The size + alpha scaling on
    // top of log-distance positioning makes the wayfinder feel like a movie
    // POV shot rather than a flat dot map.
    //
    // Sizes:  near (distFraction 0.0)  → 96px avatar, full alpha
    //         mid  (distFraction 0.5)  → 60px,        ~85% alpha
    //         far  (distFraction 1.0)  → 32px,        ~55% alpha
    //
    // Border thickness: same half-exponent rule as floating photos so the
    // visual language is consistent throughout the canvas.
    // V32.34+: dropped from 96 → 76 → 72. Continued device testing on
    // FP5 showed the closer-peer avatars still felt oversized relative
    // to the rest of the UI after the global 5%-twice reductions. 72
    // keeps the "closest peer is prominent" feel without overwhelming
    // the small phone canvas. Min unchanged at 32.
    constexpr qreal kAvatarMax = 72.0;  // closest peer
    constexpr qreal kAvatarMin = 32.0;  // most distant peer
    constexpr qreal kBorderMaxPx = 4.0;
    for (const auto &node : m_nodes) {
        // Linear interpolation on size between max (close) and min (far)
        const qreal df = node.distFraction;
        const qreal avatarSize = kAvatarMax + (kAvatarMin - kAvatarMax) * df;
        // Alpha falloff — never fully transparent; floor at 0.55
        const qreal alpha = 1.0 - 0.45 * df;

        const QRectF avatarR(node.pos.x() - avatarSize / 2,
                              node.pos.y() - avatarSize / 2,
                              avatarSize, avatarSize);

        // Save + apply alpha for everything in this node's draw block
        p.save();
        p.setOpacity(alpha);

        // Avatar circle (snap thumb if available, otherwise initials)
        QPainterPath clip;
        clip.addEllipse(avatarR);
        p.save();
        p.setClipPath(clip);
        if (!node.avatar.isNull()) {
            p.drawImage(avatarR, node.avatar);
        } else {
            p.fillRect(avatarR, card);
            p.setPen(text);
            QFont initFont = p.font();
            initFont.setPointSize(qMax(10, static_cast<int>(avatarSize * 0.4)));
            initFont.setBold(true);
            p.setFont(initFont);
            const QString initial = node.displayName.left(1).toUpper();
            p.drawText(avatarR, Qt::AlignCenter, initial);
        }
        p.restore();

        // Border ring — half-exponent thickness rule (same as floating photos),
        // PLUS a "fresh snap" pulse boost that decays over 2.5s when a new
        // snap arrives. Pulse is computed from elapsed-since-snap so all
        // recent peers animate in sync from the same paint clock.
        qreal borderW = qMax(1.0, kBorderMaxPx * (1.0 - std::sqrt(df)));
        QColor borderCol = node.online ? success : border;
        if (node.freshSnapMs > 0) {
            const qint64 elapsed = QDateTime::currentMSecsSinceEpoch() - node.freshSnapMs;
            constexpr qint64 kPulseMs = 2500;
            if (elapsed < kPulseMs) {
                // 0..1 progress, used for a fading sine pulse
                const qreal t = qreal(elapsed) / qreal(kPulseMs);
                const qreal pulse = (1.0 - t) * (0.5 + 0.5 * std::sin(t * 12.0));
                borderW += 4.0 * pulse;             // up to +4px ring
                borderCol = accent;                 // override to accent during pulse
            }
        }
        p.setPen(QPen(borderCol, borderW));
        p.setBrush(Qt::NoBrush);
        p.drawEllipse(avatarR);

        // Online status dot — only on close-enough contacts (clutter at scale)
        if (node.online && avatarSize > 40.0) {
            p.setBrush(success);
            p.setPen(QPen(bg, 2));
            p.drawEllipse(QPointF(avatarR.right() - 4, avatarR.bottom() - 4),
                          qMax(3.0, avatarSize * 0.07),
                          qMax(3.0, avatarSize * 0.07));
        }

        // Name label below avatar — font scales subtly with distance too
        p.setPen(text);
        QFont nameFont = p.font();
        nameFont.setPointSize(qMax(7, static_cast<int>(11 - 4 * df)));
        nameFont.setBold(true);
        p.setFont(nameFont);
        const QString shortName = node.displayName.length() > 14
            ? node.displayName.left(13) + QStringLiteral("…")
            : node.displayName;
        p.drawText(QRectF(node.pos.x() - 60,
                           node.pos.y() + avatarSize / 2 + 4, 120, 14),
                    Qt::AlignCenter, shortName);

        // Distance label — secondary text, smaller
        p.setPen(text2);
        QFont distFont = p.font();
        distFont.setPointSize(qMax(7, static_cast<int>(9 - 2 * df)));
        distFont.setBold(false);
        p.setFont(distFont);
        p.drawText(QRectF(node.pos.x() - 60,
                           node.pos.y() + avatarSize / 2 + 18, 120, 12),
                    Qt::AlignCenter, node.distanceLabel);

        // Lifetime stats line: "↑12  ↓7  · 2h ago"
        // Skip on tiny avatars (would just be visual noise) and when totally
        // unconversed-with (no signal to show). Compact glyphs ↑↓ are wider
        // than ASCII 'u/d' but read much faster on a busy canvas.
        if (avatarSize > 50.0
         && (node.sentCount > 0 || node.recvCount > 0
             || node.lastSeen.isValid())) {
            QStringList parts;
            if (node.sentCount > 0) parts << QStringLiteral("↑%1").arg(node.sentCount);
            if (node.recvCount > 0) parts << QStringLiteral("↓%1").arg(node.recvCount);
            // Only show "ago" if not currently online (online dot conveys it)
            if (!node.online && node.lastSeen.isValid()) {
                const QString ago = humanizeAgo(node.lastSeen);
                if (!ago.isEmpty()) parts << ago;
            }
            if (!parts.isEmpty()) {
                p.setPen(text2);
                QFont statFont = p.font();
                statFont.setPointSize(qMax(7, static_cast<int>(8 - 2 * df)));
                statFont.setBold(false);
                p.setFont(statFont);
                p.drawText(QRectF(node.pos.x() - 70,
                                   node.pos.y() + avatarSize / 2 + 30, 140, 12),
                            Qt::AlignCenter, parts.join(QStringLiteral("  ·  ")));
            }
        }

        p.restore();  // restore alpha
    }

    // ── Header strip ────────────────────────────────────────────────────────
    // Aggregate network summary: located peers, total sent/received,
    // online-now count. Builds a sense of the social network at a glance.
    int totalSent = 0;
    int totalRecv = 0;
    int onlineNow = 0;
    for (const auto &n : m_nodes) {
        totalSent += n.sentCount;
        totalRecv += n.recvCount;
        if (n.online) ++onlineNow;
    }

    QFont headerFont = p.font();
    headerFont.setPointSize(10);
    headerFont.setBold(true);
    p.setFont(headerFont);
    p.setPen(text2);
    QString headerText = tr("WAYFINDER · %1 located").arg(m_nodes.size());
    if (onlineNow > 0) {
        headerText += tr(" · %1 online").arg(onlineNow);
    }
    if (totalSent > 0 || totalRecv > 0) {
        headerText += tr(" · ↑%1 ↓%2").arg(totalSent).arg(totalRecv);
    }
    p.drawText(QRectF(8, 8, width() - 16, 18),
                Qt::AlignLeft | Qt::AlignVCenter, headerText);
}

} // namespace SnapClone::UI
