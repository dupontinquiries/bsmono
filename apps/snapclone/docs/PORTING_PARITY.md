# Porting parity — snapclone & voiceterm → egui

The faithful-port ledger: every Qt/QML surface, its egui status, and where the port lives.
"Faithful" means the Qt behavior contract is mirrored — page sets, sub-tab tables, ordering
rules, animation semantics — with headless tests pinning each contract (the gpu_menu pattern).

## snapclone (Qt6 C++, 59 UI widgets)

Shell contract (MainWindow.cpp), PORTED + tested in `egui-frontend`:
five pages Camera·Inbox·Friends·Wayfinder·Settings; UnifiedNavBar two-level nav with
verbatim sub-tab tables (Inbox All/Unread/Sent, Friends All/Online/Pending, Settings
General/Camera/Network/Storage/About), centered-name form for non-SubTabbed pages,
prev/next enable states, morphing underline (eases + settles), page dots; login overlay
starts hidden with the auto-login ordering rule.

| widget | status | notes |
|---|---|---|
| AddLandmarkDialog | pending |  |
| AmbientBlobs | ported | uikit_egui::livebg (Aurora/Waves live background) |
| AmbientCameraDriver | pending |  |
| AmbientInboxDriver | pending |  |
| AmbientPaletteDeriver | pending |  |
| CameraWidget | pending |  |
| ChevronConfirm | pending |  |
| ComponentTestWindow | pending |  |
| ContactCard | pending |  |
| ContactEditorDialog | pending |  |
| ContactPickerDialog | pending |  |
| ContactsPage | pending |  |
| ConversationDetailDialog | pending |  |
| ConversationSecurityBar | pending |  |
| DiagnosticsDialog | pending |  |
| ExportReminderCard | pending |  |
| FriendListWidget | pending |  |
| GroupVoiceBar | pending |  |
| GroupVoiceCoordinator | pending |  |
| GroupVoiceRoster | pending |  |
| HelpManualDialog | pending |  |
| InboxWidget | pending |  |
| IncomingCallDialog | pending |  |
| LoginWidget | pending |  |
| MainWindow | partial | 5-page shell + login-overlay ordering rule in lib.rs; status bar/export gate pending |
| MediaSendSummary | pending |  |
| MobileDialog | pending |  |
| ModalLayerTestPanel | pending |  |
| ModalSurface | pending |  |
| MonthlyExportReminder | pending |  |
| OutboxWidget | pending |  |
| PagePeelHint | pending |  |
| PressGesture | pending |  |
| QrCodeRenderer | pending |  |
| QrScanDialog | pending |  |
| RingtoneSynth | pending |  |
| ScrollAreas | pending |  |
| SegmentedTabStrip | pending |  |
| SettingsPage | partial | sub-tab table only; 4680-line page body pending |
| SmartRibbon | pending |  |
| SnapComposerDialog | pending |  |
| SnapExporter | pending |  |
| SnapFeedWidget | pending |  |
| SnapViewerDialog | pending |  |
| SvgIcon | pending |  |
| SwipeNavigator | ported | uikit_egui::nav + GestureLayer (page swipe, subpage drill, clamp) |
| TabBar | pending |  |
| ThumbnailService | pending |  |
| TouchFeedback | pending |  |
| UiDebugFlags | pending |  |
| UnifiedNavBar | ported | unified_nav.rs — segments/arrows/centered-name/underline-morph/dots, tables verbatim |
| UxGuide | pending |  |
| VirtualKeyboardWatcher | pending |  |
| VoiceNetworkBridge | pending |  |

## voiceterm (QML, 8 files)

| QML | egui module | status |
|---|---|---|
| main.qml | shell.rs | ported (pane system, earlier session) |
| ChatPanel.qml | chat_panel.rs | ported |
| ConfirmDialog.qml | confirm_dialog.rs | ported |
| GestureHost.qml + SlidePopup.qml | gesture_menus.rs + action_compass.rs | ported (host-owns-input model, menu variants) |
| ModelPicker.qml | model_picker.rs | ported |
| ReplVoiceGlow.qml | repl_voice_glow.rs | ported |
| SetupWizard.qml | setup_wizard.rs | ported |

Next faithful increments, in value order: snapclone ContactsPage (1854 lines: roster +
Nearby inline panel + pending flow) on the Friends page; InboxWidget + SnapFeedWidget on
Inbox; SnapViewerDialog (auto-close after durationSecs, save-any-time) + SnapComposerDialog;
then the SettingsPage body. voiceterm: behavior-diff audit of each ported module against its
QML for drift.


## snapclone DHT — native port of OpenDHT (`thirdparty/opendht` reference)

The `snapclone-dht` crate's presence/announce path was a "via OpenDHT FFI / JamiAdapter" stub;
it is now a native sans-io DHT faithful to the OpenDHT reference, with constants and semantics
cited per item:

| OpenDHT reference | snapclone-dht | fidelity |
|---|---|---|
| `InfoHash` 160-bit + `commonBits` (infohash.h) | `core::InfoHash` XOR + `common_bits` | exact |
| k-buckets, `TARGET_NODES=8`, split-own-bucket (routing_table.h) | `core::RoutingTable` | exact constants + split rule |
| `SEARCH_NODES=14`, `MAX_REQUESTED_SEARCH_NODES=4` (dht.h) | `engine` search state | exact; iterative lookup ends when closest set stops improving |
| `MessageType` ping/find/get/put/listen + reply/error (net.h) | `wire::Msg` / `Q` | full set incl. ValueData push |
| msgpack maps keyed t/y/q/a/r (net.h KEY_*) | `wire` codec | subset codec, strict round-trip |
| write token on announce (anti-spoof) | `engine` token_for + 401 on missing | enforced |
| value expiry (storage.h) | `core::Storage::expire`, 10 min | matched |
| `virtual_network_builder` sim testing (python/tools) | `sim::Sim` deterministic net | same strategy |

Sans-io throughout: `Engine::handle_packet(from, bytes, now)` and `tick(now)` return packets to
send; nothing touches a socket, so the whole DHT is a pure function under test. Seven simulation
tests cover wire round-trips, XOR/common-bits, bucket split + K-cap, 30-node convergence, a
put resolved by a distant get, token-less announce rejected with no store, listener push on a
later put, and value expiry. Presence bridges sc-IDs to the DHT (`presence::key_for_scid` +
`Presence` value); `announce::announce_record` is the pure (key,value) the socket loop feeds to
`Engine::put`.

Honest divergences (documented, not hidden): the msgpack codec is a hand-rolled subset (only
the cases these messages use); crypto-signed values and the proxy/TCP transport paths are out
of scope for this pass; ids in tests are deterministic seeds, not identity keys.

Incidental in-tree fixes (the crate hadn't built since June): `snapclone-nat` needed tokio's
`sync` feature, `snapclone-dht` was missing its `rand` dep, and an identity test called
`.as_deref()` on a non-Deref newtype. All unrelated to the DHT design; fixed so the crate
compiles and its tests run.
