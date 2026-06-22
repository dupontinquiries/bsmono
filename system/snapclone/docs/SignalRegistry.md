# Signal Registry

> **For new contributors:** This document is the authoritative map of every
> Qt signal that flows through SnapClone.  Read this before adding a new
> signal or debugging a missing UI update.

---

## How to read this document

Each entry lists:

```
EmitterClass::signalName(ArgTypes)
  → [ConnectPoint]  ReceiverClass::slotOrLambda
  → [ConnectPoint]  ...
```

`ConnectPoint` is the file and function where `QObject::connect()` is called.

Arrows represent one hop.  If a signal re-emits another signal, both hops are shown.

---

## Layer diagram

```
┌──────────────────────────────────────────────────────────────┐
│  UI Layer                                                    │
│  LoginWidget · CameraWidget · InboxWidget · FriendListWidget │
│  SwipeNavigator · DiagnosticsDialog · SettingsPage           │
└────────────────────────────┬─────────────────────────────────┘
     signals (value types only, no pointers)
┌────────────────────────────▼─────────────────────────────────┐
│  Core Layer                                                  │
│  AppController · SessionManager · SnapRouter                 │
└──────────────────┬───────────────────────────────────────────┘
    signals                      signals
┌─────────▼────────┐    ┌────────▼────────┐    ┌──────────────┐
│ Network Layer     │    │ Storage Layer   │    │ Crypto Layer │
│ NetworkManager    │    │ StorageManager  │    │ CryptoService│
│ GossipDiscovery   │    └─────────────────┘    └──────────────┘
│ WebSocketTransport│
│ JamiAdapter       │
└───────────────────┘
```

**Rules enforced by the architecture:**
1. UI may only connect to Core signals — never directly to Network/Storage/Crypto.
2. All signal parameters are **value types** (defined in `include/core/Types.hpp`).
   No raw pointers cross signal boundaries.
3. Cross-thread connections use `Qt::QueuedConnection` explicitly.

---

## Core Layer Signals

### `AppController`
*Source: `include/core/AppController.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `snapReceived` | `Core::Snap` | A new inbound snap has been decrypted and stored. |
| `snapDelivered` | `Core::SnapId` | An outbound snap was successfully sent to the network. |
| `peerConnected` | `Core::UserId` | A peer's WebSocket transport connected. |
| `peerDisconnected` | `Core::UserId` | A peer's transport disconnected (may retry). |
| `errorOccurred` | `QString` | A non-fatal error that the user should see in the status bar. |

**Connection points:**
```
AppController::snapReceived       → [MainWindow::connectSignals]  InboxWidget::addSnap
AppController::peerConnected      → [MainWindow::connectSignals]  statusBar::showMessage
AppController::peerDisconnected   → [MainWindow::connectSignals]  statusBar::showMessage
AppController::errorOccurred      → [MainWindow::connectSignals]  statusBar::showMessage
AppController::snapDelivered      → (not yet connected to UI — future: delivery receipt)
```

---

### `SessionManager`
*Source: `include/core/SessionManager.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `stateChanged` | `SessionState` | Login state machine transitioned. |
| `loginSucceeded` | `UserProfile` | Auto-login or manual login succeeded. |
| `loginFailed` | `QString` | Login attempt failed (reason string). |

**Connection points:**
```
SessionManager::loginSucceeded  → [MainWindow::connectSignals]  MainWindow::hideLoginOverlay
                                → [MainWindow::connectSignals]  statusBar::showMessage
SessionManager::loginFailed     → [MainWindow::connectSignals]  LoginWidget::showError
```

---

### `SnapRouter`
*Source: `include/core/SnapRouter.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `outboundDelivered` | `Core::SnapId` | Wire send succeeded (no ACK — best-effort). |
| `inboundReady` | `Core::Snap` | Decrypted snap is ready for display. |
| `routingFailed` | `Core::SnapId, QString` | Routing failed (encryption error, etc). |

**Connection points:**
```
SnapRouter::inboundReady      → [AppController::connectSubsystems]  AppController ──re-emits──► snapReceived
SnapRouter::outboundDelivered → [AppController::connectSubsystems]  AppController ──re-emits──► snapDelivered
SnapRouter::routingFailed     → (logged only — not yet surfaced to UI)
```

---

## Network Layer Signals

### `NetworkManager`
*Source: `include/network/NetworkManager.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `payloadReceived` | `UserId, QByteArray` | Encrypted payload arrived from a peer. |
| `peerConnected` | `UserId` | Transport connected to a peer. |
| `peerDisconnected` | `UserId` | Transport disconnected from a peer. |
| `networkError` | `QString` | Non-fatal transport error. |

**Connection points:**
```
NetworkManager::payloadReceived  → [AppController::connectSubsystems]
                                     → SnapRouter::routeInbound()
NetworkManager::peerConnected    → [AppController::connectSubsystems]
                                     → AppController ──re-emits──► peerConnected
NetworkManager::peerDisconnected → [AppController::connectSubsystems]
                                     → AppController ──re-emits──► peerDisconnected
NetworkManager::networkError     → [AppController::connectSubsystems]
                                     → AppController ──re-emits──► errorOccurred
```

---

### `IDiscovery` (GossipDiscovery / JamiDiscovery)
*Source: `include/network/NetworkManager.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `peerDiscovered` | `UserId, PeerAddress` | A new peer was found in the gossip view or DHT. |
| `peerLost` | `UserId` | A peer timed out and was removed from the view. |

**Connection points:**
```
IDiscovery::peerDiscovered  → [NetworkManager constructor]  NetworkManager::onPeerDiscovered
IDiscovery::peerLost        → [NetworkManager constructor]  NetworkManager ──re-emits──► peerDisconnected
```

---

### `ITransport` (WebSocketTransport / JamiTransport)
*Source: `include/network/NetworkManager.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `connected` | — | Handshake complete, transport ready. |
| `disconnected` | — | Connection lost. |
| `messageReceived` | `QByteArray` | A complete binary frame arrived. |
| `errorOccurred` | `QString` | Transport-level error. |

**Connection points (established in `NetworkManager::connectToPeer`):**
```
ITransport::connected        → lambda [captures id] → session.connected = true
                                                    → emit peerConnected(id)
ITransport::disconnected     → lambda [captures id] → NetworkManager::onTransportDisconnected(id)
ITransport::messageReceived  → lambda [captures id] → emit payloadReceived(id, data)
ITransport::errorOccurred    → lambda [captures id] → emit networkError(...)
```

---

### `NetworkDiagnostics`
*Source: `include/network/NetworkDiagnostics.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `checkCompleted` | `DiagnosticResult` | One check finished (streamed to dialog). |
| `diagnosisReady` | `QList<DiagnosticResult>` | All checks finished. |

**Connection points:**
```
NetworkDiagnostics::checkCompleted → [DiagnosticsDialog constructor]  addResultRow
NetworkDiagnostics::diagnosisReady → [DiagnosticsDialog constructor]  update status label
```

---

### `FirewallPortManager`
*Source: `include/network/FirewallPortManager.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `portOpened` | `PortResult` | One port request completed (success or failure). |
| `allPortsProcessed` | `bool` | All requested ports have been processed. |
| `backendDetected` | `FirewallBackend, QString` | Detected which firewall backend is available. |

**Connection points:**
```
FirewallPortManager::portOpened        → [AppController::initialize]  qCInfo log only
FirewallPortManager::allPortsProcessed → [AppController::initialize]  emit errorOccurred if !ok
```

---

## UI Layer Signals

### `SwipeNavigator`
*Source: `include/ui/SwipeNavigator.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `pageChanged` | `int` | Animation complete; new page index is active. |

**Connection points:**
```
SwipeNavigator::pageChanged → [MainWindow::connectSignals]
    index == kPageCamera → CameraWidget::startCamera
    other               → CameraWidget::stopCamera
```

---

### `CameraWidget`
*Source: `include/ui/CameraWidget.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `snapCaptured` | `QImage` | A photo was taken; raw image ready for routing. |
| `backRequested` | — | User pressed the back button. |

**Connection points:**
```
CameraWidget::snapCaptured  → [MainWindow::connectSignals]  builds Snap, calls AppController::sendSnap
CameraWidget::backRequested → [MainWindow::connectSignals]  SwipeNavigator::goToPage(kPageInbox)
```

---

### `InboxWidget`
*Source: `include/ui/InboxWidget.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `snapOpened` | `Core::SnapId` | User double-tapped a snap to view it. |
| `cameraRequested` | — | User pressed the camera button. |
| `diagnosticsRequested` | — | User pressed the network diagnostics button. |

**Connection points:**
```
InboxWidget::cameraRequested      → [MainWindow::connectSignals]  SwipeNavigator::goToPage(kPageCamera)
InboxWidget::diagnosticsRequested → [MainWindow::connectSignals]  DiagnosticsDialog::exec()
InboxWidget::snapOpened           → [MainWindow::connectSignals]  AppController::markSnapViewed
```

---

### `LoginWidget`
*Source: `include/ui/LoginWidget.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `loginRequested` | `QString username, QString passwordHash` | User submitted credentials. The password is already hashed (SHA-256) — raw password never leaves this widget. |

**Connection points:**
```
LoginWidget::loginRequested → [MainWindow::connectSignals]  SessionManager::beginLogin
```

---

### `FriendListWidget`
*Source: `include/ui/FriendListWidget.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `friendSelected` | `Core::UserId` | User tapped a friend row. |
| `addFriendRequested` | `QString host, quint16 port` | User submitted the Add Friend form. |

**Connection points:**
```
FriendListWidget::addFriendRequested → [MainWindow::connectSignals]  AppController::addFriend
FriendListWidget::friendSelected     → (stored in m_selectedFriend for snap routing)
```

---

## Adding a New Signal — Checklist

1. Declare the signal in the appropriate header under `signals:`.
2. Add `Q_DECLARE_METATYPE(YourType)` in `include/core/Types.hpp` if the
   parameter type is new.
3. Connect it in the appropriate `connectSignals()` / `connectSubsystems()`
   method — never in constructors or business logic functions.
4. **Update this file** with the new row and connection point.
5. Write a test in `tests/` that verifies the signal fires under the correct
   condition (use `QSignalSpy`).

---

## Security notes on signals

- **Never pass raw pointers through signals.** Qt's queued connections copy
  parameters — a raw pointer would be a dangling pointer after the emitting
  object is deleted.
- **Encrypt before emitting.** `SnapRouter` never emits plaintext over
  `payloadReceived` — only `encryptedBlob` and metadata cross the signal boundary.
- **Hash before emitting login credentials.** `LoginWidget::loginRequested`
  carries `passwordHash`, never the raw password.

---

### `SnapViewerDialog`
*Source: `include/ui/SnapViewerDialog.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `imageSaved` | `QString path` | User saved the snap image; path is the absolute file written to `~/Pictures/SnapClone/`. |
| `viewTimerExpired` | `Core::SnapId` | Auto-close countdown reached zero. |

**Connection points:**
```
SnapViewerDialog::imageSaved      → [InboxWidget::onItemDoubleClicked]
                                       InboxWidget ──re-emits──► snapSaved(path)
                                   → [MainWindow::connectSignals]
                                       statusBar::showMessage("Saved: <filename>")
SnapViewerDialog::viewTimerExpired → (no current connection — AppController::snapViewExpired
                                       handles the storage side via markSnapViewed timer)
```

---

### `InboxWidget` (updated)
*Source: `include/ui/InboxWidget.hpp`*

| Signal | Parameters | Description |
|--------|-----------|-------------|
| `snapOpened` | `Core::SnapId` | User opened a snap → triggers storage mark-viewed + expiry timer. |
| `snapSaved`  | `QString path` | User saved snap image to disk via the viewer. |
| `cameraRequested` | — | User pressed camera icon. |
| `diagnosticsRequested` | — | User pressed diagnostics icon. |

---

## Key Exchange Protocol

Added in connectSubsystems() to make crypto functional without a full PKI:

```
Peer A connects to Peer B via WebSocket
  → NetworkManager::peerConnected(B)
    → AppController::sendHandshake(B)           [SHAK magic + A's pubKey]
  → Peer B receives payload
    → magic == 0x5348414B ("SHAK")
    → crypto->registerPeerPublicKey(A, pubKey)   [derives session key]
    → AppController::sendHandshake(A)            [responds with B's pubKey]
  → Peer A receives response
    → crypto->registerPeerPublicKey(B, pubKey)   [derives matching session key]
  → Both peers now have symmetric session keys → snaps can flow
```

Wire format for handshake messages:
```
[4 bytes: 0x5348414B "SHAK"] [32 bytes: Ed25519/HMAC public key]
```

Regular snap payloads begin with `0x534E4150` ("SNAP") and are dispatched
to `SnapRouter::routeInbound()`. The magic byte check in `payloadReceived`
discriminates between the two message types before any decryption is attempted.
