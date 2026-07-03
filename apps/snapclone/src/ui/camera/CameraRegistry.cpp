#include "ui/camera/CameraRegistry.hpp"

#include <QProcess>
#include <QRegularExpression>
#include <QElapsedTimer>
#include <QMutex>
#include <QLoggingCategory>
#include <QByteArray>
#include <QSettings>
#include <QStandardPaths>
#include <QFile>
#include <QSet>
#include <QMap>
#include <algorithm>

namespace SnapClone::UI::Camera {

namespace {

Q_LOGGING_CATEGORY(lcCamReg, "snapclone.camera.registry")

// V32.32: the canonical camera table. ONE place to edit when sensors
// change. Adding a sensor means:
//   1. Adding an entry here with correct facing + capability flags
//   2. Wiring the bridge to publish the corresponding virtual-cam-<name>
//      node (camera-bridge/src/main.rs run_rear/run_front pattern)
//   3. That's it — GstLaunchBackend, CameraWidget, etc. all read through
//      this table and pick up the new sensor automatically.
//
// Ordering matters: it's the dropdown order. Rear first, ultrawide
// next-to-rear (intuitive scrolling), front below, tracking last.
struct StaticEntry {
    const char *id;
    const char *label;
    Facing      facing;
    bool        supportsFocus;
    bool        mirrorPreview;
    bool        baseline;
};
constexpr StaticEntry kTable[] = {
    // virtual node id, display label, facing, focus?, mirror-preview?, baseline?
    //
    // baseline=true: bridge always spawns this if it starts at all
    //   → safe to list speculatively during bridge-startup grace window
    // baseline=false: bridge only spawns conditionally (ultrawide needs
    //   libcamera to report a second back sensor on FP5)
    //   → NEVER list speculatively, only when pw-dump confirms presence
    { "virtual-cam-rear",            "Rear Camera",             Facing::Rear,     true,  false, true  },
    // FP5 ultrawide has no VCM — fixed focus. Don't expose tap-to-focus.
    { "virtual-cam-rear-ultrawide",  "Rear Camera (Ultrawide)", Facing::Rear,     false, false, false },
    // Front sensor is fixed-focus on FP5 (no dw9719 on that i2c bus).
    // Mirror=true: selfie convention.
    { "virtual-cam-front",           "Front Camera",            Facing::Front,    false, true,  true  },
    // Tracking is the front sensor with a face-detection overlay
    // pipeline; still selfie-mirrored for preview.
    { "virtual-cam-tracking",        "Face Tracking",           Facing::Tracking, false, true,  true  },
};

constexpr qint64 kPwDumpCacheMs = 500;  // half-second cache

// V32.33+: heuristic classification for libcamera_input.* node names.
// FP5 (from observation in /mnt/transcripts journal):
//   libcamera_input._base_soc_0_cci_ac4a000_i2c-bus_1_camera_29 → rear (imx858)
//   libcamera_input._base_soc_0_cci_ac4a000_i2c-bus_3_camera_10 → front (s5kjn1)
// The camera_NN number is the i2c slave address (decimal). On FP5 these
// happen to be 29 and 10. On other devices the numbers will differ.
//
// We don't try to identify rear-vs-front by hardware probe — that requires
// reading /sys/class/video4linux/v4l-subdevX/name or similar which depends
// on uaccess permissions. Heuristic instead: when there are exactly TWO
// libcamera nodes, the one with the LARGER i2c address is rear (this holds
// on FP5; will need adjustment if it doesn't on another device — at which
// point the env-var override kicks in). For three+ nodes we still emit
// them all but the user has to identify them by trial.
//
// Per-device override file ~/.config/snapclone/camera_overrides.ini
// makes the mapping explicit and bypasses heuristics. Format:
//   [overrides]
//   rear=libcamera_input._base_soc_0_cci_ac4a000_i2c-bus_1_camera_29
//   front=libcamera_input._base_soc_0_cci_ac4a000_i2c-bus_3_camera_10

struct LibcameraNode {
    QString nodeName;
    int     i2cBus      = -1;   // parsed i2c-bus-NN
    int     cameraAddr  = -1;   // parsed camera_NN
};

QList<LibcameraNode> parseLibcameraNodes(const QByteArray &pwOutput)
{
    QList<LibcameraNode> out;
    // Regex: libcamera_input.<anything>_i2c-bus_<num>_camera_<num>
    // Also accept libcamera_input.<anything> without the i2c-bus / camera
    // suffix as a fallback — some devices may not use that naming.
    static const QRegularExpression rxFull(
        QStringLiteral(R"RX("node\.name"\s*[:=]\s*"(libcamera_input\.[A-Za-z0-9_\-]+_i2c-bus_(\d+)_camera_(\d+))")RX"));
    static const QRegularExpression rxAny(
        QStringLiteral(R"RX("node\.name"\s*[:=]\s*"(libcamera_input\.[A-Za-z0-9_\-\.]+)")RX"));

    const QString outStr = QString::fromUtf8(pwOutput);
    QSet<QString> seen;

    auto itFull = rxFull.globalMatch(outStr);
    while (itFull.hasNext()) {
        const auto m = itFull.next();
        const QString name = m.captured(1);
        if (seen.contains(name)) continue;
        seen.insert(name);
        LibcameraNode n;
        n.nodeName   = name;
        n.i2cBus     = m.captured(2).toInt();
        n.cameraAddr = m.captured(3).toInt();
        out.append(n);
    }

    auto itAny = rxAny.globalMatch(outStr);
    while (itAny.hasNext()) {
        const auto m = itAny.next();
        const QString name = m.captured(1);
        if (seen.contains(name)) continue;
        seen.insert(name);
        LibcameraNode n;
        n.nodeName = name;
        // i2cBus/cameraAddr stay -1 — heuristic classification falls
        // back to position-in-list ordering for these.
        out.append(n);
    }

    // V32.37+ USB-webcam discovery aid. A UVC webcam (e.g. OBSBOT) appears in
    // pipewire under a DIFFERENT node.name than libcamera_input.* — often a
    // v4l2/uvc-derived name or the product string — so the regexes above miss it
    // and it never enumerates (observed: frames-seen:0, "no luck on non-desktop").
    // We can't guess the exact pattern without seeing the device, so log the
    // node.names we didn't match. This function is called MANY times per second
    // (every connect attempt / device poll), so we must NOT log every call —
    // that floods the log (observed: dozens of identical lines/sec). Log only
    // when the unmatched SET changes vs. the last time we logged, so the names
    // surface once and re-surface only if the hardware list actually changes.
    static const QRegularExpression rxAnyNode(
        QStringLiteral(R"RX("node\.name"\s*[:=]\s*"([^"]+)")RX"));
    auto itAll = rxAnyNode.globalMatch(outStr);
    QStringList unmatched;
    while (itAll.hasNext()) {
        const QString name = itAll.next().captured(1);
        if (!seen.contains(name)) unmatched.append(name);
    }
    if (!unmatched.isEmpty()) {
        unmatched.sort();   // stable comparison key regardless of scan order
        const QString key = unmatched.join(QStringLiteral(","));
        static QString s_lastUnmatchedLogged;
        if (key != s_lastUnmatchedLogged) {
            s_lastUnmatchedLogged = key;
            qCInfo(lcCamReg).noquote()
                << "Unmatched pipewire node.name(s) (NOT enumerated as cameras — if "
                   "your USB webcam is among these, its node.name shows the pattern "
                   "to add):" << unmatched.join(QStringLiteral(", "));
        }
    }

    return out;
}

/// Load per-device overrides from
/// ~/.config/snapclone/camera_overrides.ini if it exists.
/// Returns a map of "rear" / "front" / "tracking" / "ultrawide" → node-name.
QMap<QString, QString> loadOverrides()
{
    QMap<QString, QString> out;
    const QString dir = QStandardPaths::writableLocation(
                            QStandardPaths::AppConfigLocation);
    const QString path = dir + QStringLiteral("/camera_overrides.ini");
    if (!QFile::exists(path)) return out;
    QSettings ini(path, QSettings::IniFormat);
    ini.beginGroup(QStringLiteral("overrides"));
    for (const QString &key : ini.childKeys()) {
        const QString val = ini.value(key).toString().trimmed();
        if (!val.isEmpty()) out.insert(key.toLower(), val);
    }
    ini.endGroup();
    return out;
}

/// Env var override: SNAPCLONE_CAMERA_<facing>=<node>. Easier than
/// editing a config file for debug / one-off sessions.
QString envOverride(const char *facing)
{
    const QByteArray var = QByteArray("SNAPCLONE_CAMERA_") + facing;
    const QByteArray val = qgetenv(var.constData());
    return QString::fromUtf8(val).trimmed();
}

// ─── V32.34+: cam --list parsing (ported from camera-bridge/src/capture.rs) ──
//
// On postmarketOS / Alpine, libcamera's CLI tool `cam --list` produces a
// structured dump that's strictly better than guessing from PipeWire node
// names:
//
//   Available cameras:
//   1: 'imx858 9-001a' (/base/soc/cci@ac4a000/i2c-bus@1/camera@29)
//       Location: Back
//   2: 's5kjn1 8-001a' (/base/soc/cci@ac4a000/i2c-bus@1/camera@1a)
//       Location: Back
//   3: 's5kjn1 8-002d' (/base/soc/cci@ac4b000/i2c-bus@1/camera@10)
//       Location: Front
//
// The advantages:
//   • Authoritative Location: Back/Front/External labels — no i2c
//     address heuristics
//   • Sensor model strings (imx858, s5kjn1) for diagnostics
//   • i2c path fragments we can match against PipeWire node names with
//     suffix matching (avoids the "camera_1 matches camera_10" pitfall)
//
// We try `cam --list` first. If it succeeds (libcamera-tools installed,
// any cameras found), we use its classifications. If it fails — cam
// isn't installed, returns no entries, etc. — we fall back to the
// i2c-sort heuristic.

struct CamListEntry {
    QString model;     // "imx858 9-001a"
    QString pathId;    // "camera_29" (after normalizing @/ → _)
    QString location;  // "back", "front", "external" (lowercase)
};

/// Run `cam --list` and parse it. Returns empty list if cam is missing
/// or output doesn't parse.
QList<CamListEntry> parseCamList()
{
    QProcess p;
    // Most libcamera-tools packages name the binary `cam`. Some distros
    // (rare) install it as `libcamera-cam`. We could try both, but cam
    // is the canonical name — if it's not in PATH the caller falls back
    // to the heuristic anyway.
    p.start(QStringLiteral("cam"), {QStringLiteral("--list")});
    if (!p.waitForFinished(1000)) {
        p.kill();
        return {};
    }
    if (p.exitCode() != 0) return {};

    const QByteArray rawOut = p.readAllStandardOutput();
    const QString text = QString::fromUtf8(rawOut);

    // State machine: when we see a sensor model line ("1: 'model' (path)"),
    // stash model and pathId. When we then see "Location: <loc>", emit
    // a CamListEntry and reset stash.
    QString currentPath, currentModel;
    QList<CamListEntry> entries;

    const QStringList lines = text.split(QChar('\n'));
    for (const QString &raw : lines) {
        const QString line = raw.trimmed();

        // Try to parse a sensor model line: contains 'model' and (path).
        // Single quotes around the model, parens around the path.
        const int firstQuote = line.indexOf(QChar('\''));
        if (firstQuote >= 0) {
            const int secondQuote = line.indexOf(QChar('\''), firstQuote + 1);
            if (secondQuote > firstQuote) {
                const QString model = line.mid(firstQuote + 1, secondQuote - firstQuote - 1);
                const int openParen = line.indexOf(QChar('('), secondQuote);
                if (openParen > 0) {
                    const int closeParen = line.indexOf(QChar(')'), openParen);
                    if (closeParen > openParen) {
                        QString path = line.mid(openParen + 1, closeParen - openParen - 1);
                        // Normalize: '@' and '/' both become '_' so the
                        // result matches PipeWire's node-name encoding.
                        path.replace(QChar('@'), QChar('_'))
                            .replace(QChar('/'), QChar('_'));
                        // Strip everything before the last "camera_" so
                        // we end up with just "camera_NN".
                        const int lastCam = path.lastIndexOf(QStringLiteral("camera_"));
                        if (lastCam >= 0) {
                            currentPath = path.mid(lastCam);
                            currentModel = model;
                        }
                    }
                }
            }
        }

        // Location line — emit the stashed entry.
        if (line.startsWith(QStringLiteral("Location:"))) {
            if (!currentPath.isEmpty() && !currentModel.isEmpty()) {
                CamListEntry e;
                e.model    = currentModel;
                e.pathId   = currentPath;
                e.location = line.mid(QStringLiteral("Location:").size())
                                 .trimmed().toLower();
                entries.append(e);
            }
            currentPath.clear();
            currentModel.clear();
        }
    }

    return entries;
}

/// Result of cam --list-based classification. Authoritative facing labels
/// for each node, distinguishing multiple back-facing cameras.
struct CamListClassification {
    QString rearNode;             // primary back camera
    QString frontNode;            // primary front camera
    QStringList extraRearNodes;   // additional back cameras (e.g. ultrawide)
};

/// Match cam --list entries against PipeWire libcamera_input.* nodes.
/// Returns std::nullopt if the match fails (no cam output, no nodes,
/// no overlap).
///
/// Critical: the match uses SUFFIX matching, then longest-substring
/// fallback. This is the camera_1-vs-camera_10 fix — naive substring
/// match would have path_id "camera_1" matching node name ending in
/// "camera_10" and silently routing the wrong sensor.
std::optional<CamListClassification> resolveCamListNodes(
    const QList<CamListEntry> &entries,
    const QList<LibcameraNode> &probed)
{
    if (entries.isEmpty() || probed.isEmpty()) return std::nullopt;

    auto resolveNode = [&probed](const QString &pathId) -> QString {
        const QString target = pathId.toLower();
        // First pass: suffix match. PipeWire libcamera node names always
        // end with the camera_NN fragment, so this matches correctly even
        // when one path is a numeric prefix of another (camera_1 ⊂ camera_10).
        for (const auto &n : probed) {
            if (n.nodeName.toLower().endsWith(target)) {
                return n.nodeName;
            }
        }
        // Second pass: longest containing match. Some PipeWire versions
        // may append extra suffixes we don't anticipate; take the longest
        // name that contains the target to bias toward more specific matches.
        QString best;
        for (const auto &n : probed) {
            if (n.nodeName.toLower().contains(target)) {
                if (best.isEmpty() || n.nodeName.size() > best.size()) {
                    best = n.nodeName;
                }
            }
        }
        return best;
    };

    CamListClassification out;
    QStringList backNodes;
    for (const auto &e : entries) {
        const QString node = resolveNode(e.pathId);
        if (node.isEmpty()) {
            qCWarning(lcCamReg) << "cam --list entry" << e.model
                                << "(" << e.pathId << "/" << e.location
                                << ") has no matching PipeWire node";
            continue;
        }
        if (e.location == QLatin1String("back")) {
            if (!backNodes.contains(node)) {
                backNodes.append(node);
                qCInfo(lcCamReg) << "cam --list:" << e.model
                                 << "(" << e.pathId << ") → BACK →" << node;
            }
        } else if (e.location == QLatin1String("front")) {
            if (out.frontNode.isEmpty()) {
                out.frontNode = node;
                qCInfo(lcCamReg) << "cam --list:" << e.model
                                 << "(" << e.pathId << ") → FRONT →" << node;
            }
        } else if (out.frontNode.isEmpty()
                   && CameraRegistry::shouldBeFrontFacing(e.model, e.location)) {
            // V32.38+ FaceTime fix, generalized in V32.42+ to the front-facing
            // rule list: libcamera reports USB/UVC user-facing webcams (FaceTime
            // HD, iSight, "Integrated Camera", generic webcams, ov5640) with
            // Location: External, so they fell through here and were dropped —
            // and without a front node such a webcam never got Facing::Front, so
            // its self-view preview was shown un-mirrored. The rule list now
            // promotes any external (USB) camera or name-matched webcam to front.
            // (Resolution isn't known at this point, so the ≤720p rule is a no-op
            // here — but every external webcam, ≤720p or not, is already caught.)
            out.frontNode = node;
            qCInfo(lcCamReg) << "cam --list:" << e.model
                             << "(" << e.pathId
                             << ") → front-facing rule (" << e.location
                             << ") → FRONT →" << node;
        }
        // Locations that are neither back/front/external (empty or "unknown")
        // are ignored — no stable facing, and the rule list found no reason to
        // promote them.
    }

    if (backNodes.isEmpty() && out.frontNode.isEmpty()) {
        // cam --list returned entries but none of them resolved to a
        // PipeWire node. Could happen if libcamera and pipewire-libcamera
        // versions are out of sync. Caller falls back to heuristic.
        return std::nullopt;
    }
    if (!backNodes.isEmpty()) {
        out.rearNode = backNodes.first();
        // Skip the first; the rest are extra back cameras (ultrawide etc.).
        for (int i = 1; i < backNodes.size(); ++i) {
            out.extraRearNodes.append(backNodes.at(i));
        }
    }
    return out;
}

} // namespace

CameraRegistry &CameraRegistry::instance()
{
    static CameraRegistry s;
    return s;
}

CameraRegistry::CameraRegistry() = default;

bool CameraRegistry::isUserFacingWebcamModel(const QString &modelOrLabel)
{
    const QString s = modelOrLabel.toLower();
    // Substrings that identify a built-in / user-facing webcam across the
    // platforms we run on: Apple's built-ins, the generic Linux/UVC/USB labels,
    // and the ov5640 UVC sensor used by several dev boards. Branded USB webcams
    // (Logitech etc.) don't need to be listed — they report location "external"
    // and are caught by the USB rule in shouldBeFrontFacing().
    static const char *const kNeedles[] = {
        "facetime", "isight", "integrated camera",
        "webcam", "uvc", "usb camera", "user-facing", "ov5640",
    };
    for (const char *needle : kNeedles) {
        if (s.contains(QLatin1String(needle)))
            return true;
    }
    return false;
}

bool CameraRegistry::shouldBeFrontFacing(const QString &model,
                                         const QString &location,
                                         int maxResolutionHeightPx)
{
    // Rule 1 — known user-facing webcam by name (FaceTime, iSight, generic
    // webcam/UVC/USB camera, ov5640).
    if (isUserFacingWebcamModel(model))
        return true;
    // Rule 2 — USB/UVC webcam: libcamera reports these with location "external",
    // and an external camera is almost always the user-facing one.
    if (location.trimmed().toLower() == QLatin1String("external"))
        return true;
    // Rule 3 — a ≤720p camera (0 = resolution unknown → skip). Low-res sensors
    // are overwhelmingly user-facing webcams rather than phone rear modules.
    if (maxResolutionHeightPx > 0 && maxResolutionHeightPx <= 720)
        return true;
    return false;
}

QString CameraRegistry::queryPwDumpRawCached() const
{
    // V32.33+: keep the raw output cached so both the virtual-cam-* scan
    // and the libcamera_input.* fallback can share one subprocess spawn.
    // V32.34+: cache moved off function-local statics onto instance
    // members so invalidateCache() can reset it from outside (after
    // the user edits camera_overrides.ini in Settings).
    QMutexLocker lock(&m_pwDumpMu);

    if (m_pwDumpTimer.isValid() && m_pwDumpTimer.elapsed() < kPwDumpCacheMs
        && !m_pwDumpRaw.isEmpty()) {
        return QString::fromUtf8(m_pwDumpRaw);
    }

    QProcess p;
    p.start(QStringLiteral("pw-dump"), {QStringLiteral("--no-colors")});
    if (!p.waitForFinished(500)) {
        p.kill();
        return QString::fromUtf8(m_pwDumpRaw);  // stale; don't update
    }
    m_pwDumpRaw = p.readAllStandardOutput();
    m_pwDumpTimer.start();
    return QString::fromUtf8(m_pwDumpRaw);
}

void CameraRegistry::invalidateCache()
{
    // V32.34+: external invalidation. Settings UI calls this after the
    // user edits camera_overrides.ini so the next available() returns
    // the new mapping immediately (instead of waiting up to 500ms for
    // the cache to age out).
    QMutexLocker lock(&m_pwDumpMu);
    m_pwDumpRaw.clear();
    m_pwDumpTimer.invalidate();
    m_classifiedValid = false;          // V32.34+: drop classified result too
    m_classifiedTimer.invalidate();
    m_classifiedCache.clear();
    m_lastClassifiedSig.clear();        // force a fresh log on next resolve
    qCInfo(lcCamReg) << "pw-dump cache invalidated";
}

QStringList CameraRegistry::queryPwDumpCached() const
{
    const QString out = queryPwDumpRawCached();
    static const QRegularExpression rx(
        QStringLiteral(R"RX("node\.name"\s*[:=]\s*"(virtual-cam-[a-z0-9-]+)")RX"));
    QStringList found;
    auto it = rx.globalMatch(out);
    while (it.hasNext()) {
        const auto m = it.next();
        const QString name = m.captured(1);
        if (!found.contains(name)) found.append(name);
    }
    return found;
}

QList<CameraInfo> CameraRegistry::libcameraFallback() const
{
    // V32.33+: when the virtual-cam-* bridge isn't running (or doesn't
    // exist on this device — FP5 with pmOS sees the bridge in this
    // project but it's not always functional), fall back to whatever
    // libcamera_input.* nodes PipeWire is publishing directly.
    //
    // V32.34+: classification strategy is layered:
    //   1. Per-device overrides (env vars, config file) — explicit wins
    //   2. `cam --list` from libcamera-tools — authoritative facing labels
    //      and i2c-path matching (suffix-safe, multi-rear aware)
    //   3. Pure pw-dump heuristic — sort by i2c address (FP5 convention),
    //      fallback for systems without `cam`
    //
    // Step 2 is the major V32.34+ improvement: instead of guessing that
    // "the larger i2c address is rear", we ask libcamera directly.

    // V32.34+ CLASSIFIED-RESULT CACHE. Serve a recently-computed device list
    // without re-spawning `cam --list` or re-parsing. This is the fix for the
    // FP5 re-enumeration storm: callers hit available() ~4×/sec, and without
    // this guard each call re-ran the full subprocess + classification +
    // verbose logging. Keyed on the same window as the raw pw-dump cache.
    {
        QMutexLocker lock(&m_pwDumpMu);
        if (m_classifiedValid && m_classifiedTimer.isValid()
            && m_classifiedTimer.elapsed() < kPwDumpCacheMs) {
            return m_classifiedCache;  // hot path — no spawn, no parse, no log
        }
    }

    const QString raw = queryPwDumpRawCached();
    const QList<LibcameraNode> nodes = parseLibcameraNodes(raw.toUtf8());
    if (nodes.isEmpty()) return {};

    // Build a cheap signature of the raw node set so we only emit the verbose
    // classification logs when the actual device topology CHANGES, not on
    // every cache-miss recompute.
    QString rawSig;
    for (const auto &n : nodes) rawSig += n.nodeName + QLatin1Char('|');
    const bool topologyChanged = (rawSig != m_lastClassifiedSig);

    if (topologyChanged) {
        qCInfo(lcCamReg) << "libcamera fallback: found" << nodes.size()
                         << "raw libcamera_input.* nodes";
    }

    QMap<QString, QString> overrides = loadOverrides();
    // Env overrides win over file overrides.
    for (const char *f : { "REAR", "FRONT", "TRACKING", "ULTRAWIDE" }) {
        const QString val = envOverride(f);
        if (!val.isEmpty()) {
            overrides.insert(QString::fromLatin1(f).toLower(), val);
        }
    }

    auto findNodeForFacing = [&](const QString &facing) -> QString {
        if (overrides.contains(facing)) return overrides.value(facing);
        return {};
    };

    QList<CameraInfo> out;

    // Resolve from overrides first.
    QString rearNode      = findNodeForFacing(QStringLiteral("rear"));
    QString frontNode     = findNodeForFacing(QStringLiteral("front"));
    QString ultrawideNode = findNodeForFacing(QStringLiteral("ultrawide"));

    // V32.34+: ask libcamera directly via `cam --list` if we still need
    // facing info. This is strictly better than the i2c-address heuristic
    // (gets the actual Location: Back/Front from the kernel driver,
    // handles multi-rear devices correctly, uses suffix matching for
    // path → node resolution).
    //
    // Caveat: this requires libcamera-tools to be installed in the system
    // image. On Alpine/postmarketOS: `apk add libcamera-tools`. The
    // bridge image already ships with it. If absent, we fall through to
    // the heuristic below.
    bool usedCamList = false;
    if (rearNode.isEmpty() || frontNode.isEmpty()) {
        const QList<CamListEntry> entries = parseCamList();
        if (!entries.isEmpty()) {
            const auto cls = resolveCamListNodes(entries, nodes);
            if (cls) {
                if (rearNode.isEmpty()    && !cls->rearNode.isEmpty())
                    rearNode = cls->rearNode;
                if (frontNode.isEmpty()   && !cls->frontNode.isEmpty())
                    frontNode = cls->frontNode;
                if (ultrawideNode.isEmpty() && !cls->extraRearNodes.isEmpty())
                    ultrawideNode = cls->extraRearNodes.first();
                usedCamList = true;
                if (topologyChanged) {
                    qCInfo(lcCamReg) << "Classification source: cam --list"
                                     << "rear=" << rearNode
                                     << "front=" << frontNode
                                     << "ultrawide=" << ultrawideNode;
                }
            }
        }
    }

    // Heuristic fallback for everything still unresolved.
    if (!usedCamList && rearNode.isEmpty() && frontNode.isEmpty()) {
        if (nodes.size() == 1) {
            rearNode = nodes.first().nodeName;
        } else if (nodes.size() >= 2) {
            QList<LibcameraNode> sorted = nodes;
            std::sort(sorted.begin(), sorted.end(),
                      [](const LibcameraNode &a, const LibcameraNode &b) {
                          return a.cameraAddr > b.cameraAddr;
                      });
            rearNode  = sorted[0].nodeName;
            frontNode = sorted[1].nodeName;
        }
        if (topologyChanged) {
            qCInfo(lcCamReg) << "Classification source: i2c-address heuristic"
                             << "rear=" << rearNode << "front=" << frontNode;
        }
    } else if (rearNode.isEmpty() && !frontNode.isEmpty()) {
        // Front overridden or cam-list-only-found-front, pick any other as rear.
        for (const auto &n : nodes) {
            if (n.nodeName != frontNode && n.nodeName != ultrawideNode) {
                rearNode = n.nodeName; break;
            }
        }
    } else if (!rearNode.isEmpty() && frontNode.isEmpty()) {
        for (const auto &n : nodes) {
            if (n.nodeName != rearNode && n.nodeName != ultrawideNode) {
                frontNode = n.nodeName; break;
            }
        }
    }

    // Emit in dropdown order: primary rear, ultrawide, front, then any
    // leftover nodes the user can pick by trial.
    if (!rearNode.isEmpty()) {
        out.append({
            rearNode,
            QStringLiteral("Rear Camera"),
            Facing::Rear,
            false,  // libcamera path has no VCM control wired
            false,
            false   // not baseline — it's a runtime-discovered fallback
        });
    }
    if (!ultrawideNode.isEmpty() && ultrawideNode != rearNode) {
        out.append({
            ultrawideNode,
            QStringLiteral("Rear Camera (Ultrawide)"),
            Facing::Rear,
            false,
            false,
            false
        });
    }
    if (!frontNode.isEmpty() && frontNode != rearNode && frontNode != ultrawideNode) {
        out.append({
            frontNode,
            QStringLiteral("Front Camera"),
            Facing::Front,
            false,
            true,   // selfie mirror
            false
        });
    }

    // Surface any remaining nodes as "Camera N" — user can pick via trial.
    QSet<QString> placed;
    placed.insert(rearNode);
    placed.insert(ultrawideNode);
    placed.insert(frontNode);
    int extraIdx = 1;
    for (const auto &n : nodes) {
        if (placed.contains(n.nodeName)) continue;
        out.append({
            n.nodeName,
            QStringLiteral("Camera %1").arg(extraIdx++),
            Facing::Unknown,
            false,
            false,
            false
        });
    }

    if (topologyChanged) {
        qCInfo(lcCamReg) << "libcamera fallback resolved to:" << out.size() << "devices";
        for (const auto &c : out) {
            qCInfo(lcCamReg) << "  " << c.displayName << "→" << c.virtualNodeId
                             << "facing:" << static_cast<int>(c.facing);
        }
    }

    // V32.34+: store the classified result so subsequent calls within the
    // cache window skip the spawn/parse/classify entirely (see the guard at
    // the top of this function).
    {
        QMutexLocker lock(&m_pwDumpMu);
        m_classifiedCache  = out;
        m_classifiedValid  = true;
        m_classifiedTimer.start();
        m_lastClassifiedSig = rawSig;
    }

    return out;
}

QList<CameraInfo> CameraRegistry::allKnown() const
{
    QList<CameraInfo> out;
    out.reserve(std::size(kTable));
    for (const auto &e : kTable) {
        out.append({
            QString::fromLatin1(e.id),
            QString::fromLatin1(e.label),
            e.facing,
            e.supportsFocus,
            e.mirrorPreview,
            e.baseline,
        });
    }
    return out;
}

QList<CameraInfo> CameraRegistry::available() const
{
    // V32.33+: layered discovery. First check for virtual-cam-* (the
    // bridge model). If empty, fall back to libcamera_input.* direct
    // node discovery. This lets the camera work on devices where the
    // bridge isn't running, isn't compiled, or never existed — without
    // requiring code changes per-device.
    const QStringList present = queryPwDumpCached();
    QList<CameraInfo> out;
    if (!present.isEmpty()) {
        for (const auto &e : kTable) {
            if (present.contains(QLatin1String(e.id))) {
                out.append({
                    QString::fromLatin1(e.id),
                    QString::fromLatin1(e.label),
                    e.facing,
                    e.supportsFocus,
                    e.mirrorPreview,
                    e.baseline,
                });
            }
        }
        if (!out.isEmpty()) return out;
    }
    // Fall back to libcamera direct nodes — devices like FP5 where the
    // virtual-cam-* bridge isn't operational.
    return libcameraFallback();
}

std::optional<CameraInfo> CameraRegistry::byId(const QString &id) const
{
    // V32.33+: check static table first (virtual-cam-*), then libcamera
    // fallback (which is dynamically discovered each time we call it,
    // but cached at the pw-dump level — see queryPwDumpRawCached).
    for (const auto &e : kTable) {
        if (id == QLatin1String(e.id)) {
            return CameraInfo{
                QString::fromLatin1(e.id),
                QString::fromLatin1(e.label),
                e.facing,
                e.supportsFocus,
                e.mirrorPreview,
                e.baseline,
            };
        }
    }
    for (const auto &c : libcameraFallback()) {
        if (c.virtualNodeId == id) return c;
    }
    return std::nullopt;
}

bool CameraRegistry::mirrorForDevice(const QString &id, const QString &displayName) const
{
    // Typed metadata wins when the id is a known virtual node (FP5 libcamera path).
    if (const auto info = byId(id))
        return info->mirrorPreview;
    // Otherwise the id is a Qt/USB device handle with no table entry. Decide by
    // name via the front-facing rule list (location/resolution unknown here, so
    // only the name rule applies — which is what catches "FaceTime HD Camera",
    // "Integrated Camera", generic webcams, etc.).
    return shouldBeFrontFacing(displayName, QString());
}

std::optional<CameraInfo> CameraRegistry::primaryRear() const
{
    const auto avail = available();
    // First pass: prefer the canonical virtual-cam-rear if present.
    for (const auto &c : avail) {
        if (c.virtualNodeId == QLatin1String("virtual-cam-rear")) return c;
    }
    // Second pass: any Rear-facing camera, including libcamera fallback.
    for (const auto &c : avail) {
        if (c.facing == Facing::Rear) return c;
    }
    return std::nullopt;
}

QString CameraRegistry::dumpDiagnostics() const
{
    // V32.34+: human-readable dump of every signal we look at. Goal:
    // user pastes this into a bug report and we can immediately tell
    // whether the issue is bridge-not-running, libcamera-not-installed,
    // wrong classification, or something else.
    QString out;
    out += QStringLiteral("─── SnapClone CameraRegistry diagnostics ───\n");

    // 1. Virtual-cam-* nodes (the bridge's output).
    const QStringList virtNodes = queryPwDumpCached();
    out += QStringLiteral("\n[virtual-cam-* nodes in pw-dump]\n");
    if (virtNodes.isEmpty()) {
        out += QStringLiteral("  (none — bridge not running, or pw-dump unavailable)\n");
    } else {
        for (const auto &n : virtNodes) {
            out += QStringLiteral("  - %1\n").arg(n);
        }
    }

    // 2. Raw libcamera_input.* nodes (what's actually live in PipeWire).
    const QString rawDump = queryPwDumpRawCached();
    const QList<LibcameraNode> libNodes = parseLibcameraNodes(rawDump.toUtf8());
    out += QStringLiteral("\n[libcamera_input.* nodes in pw-dump]\n");
    if (libNodes.isEmpty()) {
        out += QStringLiteral("  (none — pipewire-libcamera plugin not loaded?)\n");
    } else {
        for (const auto &n : libNodes) {
            out += QStringLiteral("  - %1 (i2c-bus=%2 camera-addr=%3)\n")
                    .arg(n.nodeName)
                    .arg(n.i2cBus).arg(n.cameraAddr);
        }
    }

    // 3. cam --list entries (libcamera's own view).
    const QList<CamListEntry> camEntries = parseCamList();
    out += QStringLiteral("\n[cam --list entries]\n");
    if (camEntries.isEmpty()) {
        out += QStringLiteral("  (none — `cam` not installed, or no cameras)\n");
        out += QStringLiteral("  Install: apk add libcamera-tools (Alpine/pmOS)\n");
    } else {
        for (const auto &e : camEntries) {
            out += QStringLiteral("  - sensor: %1  path: %2  location: %3\n")
                    .arg(e.model, e.pathId, e.location);
        }
    }

    // 4. Active overrides.
    out += QStringLiteral("\n[overrides — env vars + config file]\n");
    QMap<QString, QString> overrides = loadOverrides();
    for (const char *f : { "REAR", "FRONT", "TRACKING", "ULTRAWIDE" }) {
        const QString val = envOverride(f);
        if (!val.isEmpty()) {
            overrides.insert(QString::fromLatin1(f).toLower(), val);
        }
    }
    if (overrides.isEmpty()) {
        out += QStringLiteral("  (none set)\n");
    } else {
        for (auto it = overrides.constBegin(); it != overrides.constEnd(); ++it) {
            out += QStringLiteral("  - %1 → %2\n").arg(it.key(), it.value());
        }
    }

    // 5. Final classification.
    out += QStringLiteral("\n[resolved classification (via available())]\n");
    const auto avail = available();
    if (avail.isEmpty()) {
        out += QStringLiteral("  (empty — no cameras visible at all)\n");
    } else {
        for (const auto &c : avail) {
            const char *facingStr = "Unknown";
            switch (c.facing) {
                case Facing::Rear:     facingStr = "Rear";     break;
                case Facing::Front:    facingStr = "Front";    break;
                case Facing::Tracking: facingStr = "Tracking"; break;
                case Facing::Unknown:  facingStr = "Unknown";  break;
            }
            out += QStringLiteral("  - %1 (%2)  facing=%3  vcm=%4  mirror=%5\n")
                    .arg(c.displayName, c.virtualNodeId,
                         QString::fromLatin1(facingStr))
                    .arg(c.supportsFocus ? QStringLiteral("yes") : QStringLiteral("no"))
                    .arg(c.mirrorPreview ? QStringLiteral("yes") : QStringLiteral("no"));
        }
    }

    return out;
}

} // namespace SnapClone::UI::Camera
