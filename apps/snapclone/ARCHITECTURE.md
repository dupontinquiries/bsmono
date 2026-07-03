# Architecture Decision Records

This document captures the key design decisions made in SnapClone and the
reasoning behind each one.  New contributors should read this before making
structural changes.

---

## ADR-001: Gossip Protocol for Peer Discovery

**Status:** Accepted

**Context:**  
We need peers to find each other without a central registry server.

**Decision:**  
Use an epidemic gossip protocol (SWIM-lite) over UDP multicast for LAN
discovery, with manual seed addresses for cross-network scenarios.

**Consequences:**  
+ Truly serverless — the system works with zero infrastructure.  
+ O(log N) convergence — all nodes learn about a new peer within a few rounds.  
- Multicast does not work across routers without IGMP snooping configured.  
- Gossip messages must be authenticated to prevent Sybil attacks.

---

## ADR-002: WebSockets for Data Transport

**Status:** Accepted

**Context:**  
We need a reliable, bidirectional channel between peers.

**Decision:**  
Use RFC 6455 WebSockets (Qt6::WebSockets) over port 9876.

**Consequences:**  
+ Firewall-friendly — HTTP upgrade passes through most corporate firewalls.  
+ Qt provides a well-tested implementation with TLS support.  
+ Single binary frame = no reassembly buffer needed.  
- Higher overhead than raw TCP for small messages.  
- Does not support multiplexing natively (unlike QUIC/HTTP3).

---

## ADR-003: IDiscovery / ITransport Interfaces

**Status:** Accepted

**Context:**  
The networking strategy (gossip vs DHT, WebSocket vs QUIC) will likely change.

**Decision:**  
Define `IDiscovery` and `ITransport` as pure-virtual Qt interfaces.
`NetworkManager` depends only on these interfaces, not their implementations.

**Consequences:**  
+ Swapping backends requires zero changes outside the implementation file.  
+ Each backend can be unit-tested in isolation with mock implementations.  
- Adds one level of indirection.

---

## ADR-004: Pimpl in AppController

**Status:** Accepted

**Context:**  
`AppController` owns all subsystems. Including its full member types in the
header would force every UI translation unit to compile all subsystem headers.

**Decision:**  
Use the Pimpl (pointer-to-implementation) idiom: `struct Impl` is defined only
in `AppController.cpp`.

**Consequences:**  
+ UI compile times are independent of subsystem headers.  
+ ABI stability — adding a subsystem member doesn't break the header.  
- Slightly more boilerplate.

---

## ADR-005: std::expected for CryptoService Errors

**Status:** Accepted

**Context:**  
Crypto operations can fail (bad key, MAC mismatch, …). We need a way to
propagate errors without exceptions.

**Decision:**  
Use C++23 `std::expected<Result, QString>` (available in C++23 or via a
backport in C++20).

**Consequences:**  
+ Error path is visible in the function signature.  
+ No exception overhead.  
+ Forces callers to handle the error case.  
- `std::expected` is C++23; requires `__cpp_lib_expected` support or a shim.

---

## ADR-006: Append-Only Schema Migrations

**Status:** Accepted

**Context:**  
The SQLite schema will evolve over the lifetime of the app.  Existing
installations must be upgraded automatically.

**Decision:**  
Store `kMigrations` as a compile-time `QList<QPair<int, QString>>`.  The
current schema version is stored in SQLite's `user_version` PRAGMA.  At
startup, every migration with `version > current` is applied in order.
**Existing entries are never modified.**

**Consequences:**  
+ Safe for live upgrades — no data loss.  
+ Easy to audit — the full history is in one place.  
- Migration scripts accumulate over time (acceptable for this scale).

---

## ADR-007: Value-Semantic Cross-Layer Types

**Status:** Accepted

**Context:**  
Signal/slot connections that cross thread boundaries require that types are
registered with the Qt meta-object system and are safely copyable.

**Decision:**  
All cross-layer data uses the value types in `include/core/Types.hpp`.
Every type satisfies:
- Copyable and movable (C++ rule-of-zero)
- Registered via `Q_DECLARE_METATYPE`
- No raw pointers or references

**Consequences:**  
+ Thread safety by construction — no shared mutable state.  
+ Types can be safely queued in signal/slot connections.  
- Copies instead of references (negligible for our payload sizes).

---

## ADR-008: No UI Logic in Non-UI Classes

**Status:** Accepted

**Context:**  
Qt widgets are not thread-safe and cannot be instantiated outside the main
thread.

**Decision:**  
No class outside `src/ui/` and `include/ui/` may include a Qt widget header
(`QWidget`, `QLabel`, etc.).  This is enforced by CMake link dependencies —
only `snapclone_ui` links `Qt6::Widgets`.

**Consequences:**  
+ Core and network logic can be unit-tested without a display.  
+ Worker thread safety is guaranteed by construction.  
- The UI must be driven purely by signals from the core.

---

## ADR-009: Named Logging Categories per Module

**Status:** Accepted

**Context:**  
In a multi-threaded app with several subsystems, unstructured log output is
impossible to filter.

**Decision:**  
Every `.cpp` file declares `Q_LOGGING_CATEGORY(lcXxx, "snapclone.xxx")` and
uses `qCDebug(lcXxx)` / `qCInfo(lcXxx)` / `qCWarning(lcXxx)`.

**Consequences:**  
+ Log verbosity is controllable per-subsystem at runtime via
  `QT_LOGGING_RULES` with no recompilation.  
+ Log messages always carry their origin.  
- Slightly more boilerplate per file.

---

## ADR-010: Encrypt-then-MAC

**Status:** Accepted

**Context:**  
We must guarantee both confidentiality and integrity of snap payloads.

**Decision:**  
Encrypt the plaintext, then authenticate the *ciphertext* with HMAC-SHA256
(Encrypt-then-MAC, not MAC-then-Encrypt).  The MAC is verified *before*
decryption begins so tampered ciphertexts are rejected immediately.

**Consequences:**  
+ Immune to padding oracle and chosen-ciphertext attacks.  
+ Standard recommendation (TLS 1.3, Signal Protocol).  
- Adds `kHmacBytes` (32 bytes) to every payload.

---

*Last updated: project initial release.*
