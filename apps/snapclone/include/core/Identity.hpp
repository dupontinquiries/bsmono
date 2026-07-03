#pragma once
#include <QString>

namespace SnapClone::Core {

/// Loads or generates a stable node identity string.
/// Format: "sc-XXXXXXXX" (sc- prefix + 8 hex chars).
/// Persisted to AppDataLocation/identity.txt so it survives restarts
/// and IP/network changes.
struct Identity {
    static QString loadOrGenerate();
};

} // namespace SnapClone::Core
