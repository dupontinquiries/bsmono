// Identity.cpp — Persistent stable node identity.
//
// Generates a short random ID on first launch and persists it to
// AppDataLocation/identity.txt. Subsequent launches reuse the same ID.
//
// Format: "sc-" + 8 random hex chars, e.g. "sc-a3f7c291"
// This is the ID announced into the DHT and shown in the Settings page.
// It is separate from the auto-login "user@ip" session ID so it stays
// stable across network changes (IP changes, DHCP renewals, etc.).

#include "core/Identity.hpp"
#include "core/AccountManager.hpp"

#include <QLoggingCategory>

Q_LOGGING_CATEGORY(lcIdentity, "snapclone.identity")

namespace SnapClone::Core {

QString Identity::loadOrGenerate()
{
    // The identity is now one of possibly several accounts. Return the active
    // one (AccountManager seeds a first account, or migrates a legacy
    // identity.txt, on first use). Reroll / multi-account switching happen
    // through AccountManager and apply on the next launch.
    return AccountManager::instance().activeId();
}

} // namespace SnapClone::Core
