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
