//! host — the missing call site, expressed as data.
//!
//! `Snapclone` has around ninety-five public methods. Before this module
//! the deployable binary called **two** of them (`apply_settings` and
//! `ui`) and constructed the app with three hardcoded peers, so
//! `answer_call`, `voice_frame`, `qr_frame`, `on_gossip`, `classify_nat`,
//! `peer_learned` and roughly thirty more were ported, tested, and never
//! once reached in a running process. That is what this exists to fix, and
//! the count is a snapshot of the problem rather than a claim about today
//! — the list shrinks every time something here grows an arm. `wiring_check`
//! calls that class "tested but never called in production" and it is the
//! whole gap between this port and the Qt app: not missing code, missing
//! wiring.
//!
//! ## Why an enum and not a trait
//!
//! A `Host` trait with `poll()`/`execute()` would be the reflex, and it would
//! be a layer for a future that may not come — one real implementation, one
//! fake, and an abstraction boundary drawn before anything varied. What
//! actually varies is the TRANSPORT: on a phone the events arrive from a
//! capture thread, a UDP socket and an audio callback; in a test they arrive
//! from a `Vec`. Both want the same thing — a value they can hand to the app
//! from wherever they happen to be running — and that is an enum, not a trait.
//!
//! So this module adds no policy and no indirection. [`HostEvent`] is one
//! variant per thing the world can tell the app, [`HostCommand`] is one
//! variant per thing the app wants done about it, and
//! [`Snapclone::apply`](crate::Snapclone::apply) is the match that connects
//! them to the methods that already exist. Every arm is a call that was
//! previously written by nobody.
//!
//! ## apply() drains, deliberately
//!
//! Several results the app wants are not returns from the method that caused
//! them — they are flags set on `Snapclone` during the UI frame
//! (`export_requested`, `shutter_pressed`, `snaps_saved`, `dictated`). A host
//! that has to remember to read a dozen public fields AND clear them will
//! eventually forget one, and a forgotten flag is either an export that never
//! runs or one that runs on every frame forever. So `apply` ends by draining
//! them into the same command list. One call, nothing to remember.
//!
//! [`Snapclone::drain_commands`](crate::Snapclone::drain_commands) is public
//! because a host that ran a UI frame with no event still needs to collect what
//! the frame requested — but note that `apply` already calls it, so calling
//! both for one event yields the second an empty list rather than duplicates.
//!
//! ## What is NOT here, and why the list matters more than the code
//!
//! Events exist only for entry points that the WORLD drives. The ones a finger
//! drives — `answer_call`, `open_qr_scan`, `begin_swipe`/`end_swipe`,
//! `apply_menu`, `close_video`, `open_snap_menu` — are equally unreached, and
//! an event for them would be this module pretending to fix a problem that
//! lives in `ui()`. They need a button, not a channel.
//!
//! And a caution about measurement: once `apply` exists, `wiring_check` counts
//! every method it dispatches to as "called in production", because it is —
//! from `apply`. That is only true reachability to the extent that `apply`
//! itself is reached with that variant. The binary today can genuinely produce
//! `SettingsLoaded`, `CamerasFound`, `Tick`, `KeyboardChanged` and
//! `WindowFocus`; the rest wait on a capture thread, a socket and an audio
//! device that do not exist yet. Do not read a green wiring column as a
//! working voice call.

use crate::{
    ambient_inbox, ambient_palette, camera_select, contact_book, frame_pump, gaze, gossip,
    identity, keyboard_inset, nat_punch, settings_page, snap_export, snap_state, snap_wire,
    snaps, voice_bridge, voice_send, Snapclone,
};

/// Something the world is telling the app.
///
/// One variant per outside cause, named for the cause rather than for the
/// method it happens to call — a variant called `NoteCameraWorked` would tie
/// the wire format to an internal name, and the point of the seam is that the
/// two can move apart.
/// How much time one level report stands for, for the floor's hold timer.
///
/// These events carry no timestamp, and the mic pump runs at roughly 50Hz.
/// Being slightly out only shifts how long a silent talker keeps the
/// floor, which is a feel setting rather than a correctness one — but the
/// number has to exist somewhere, and inventing it at each call site is
/// how two of them end up disagreeing.
pub const MIC_TICK_MS: f32 = 20.0;

#[derive(Debug, Clone)]
pub enum HostEvent {
    /// Wall clock advanced. Carries seconds because that is what the app's
    /// relative-time rendering uses, and is deliberately NOT a general
    /// per-frame tick: `ui()` already advances gossip, sends, posture, help and
    /// touch feedback every frame, and a second caller doing the same would
    /// double their rates.
    Tick { now_secs: i64 },

    /// Settings came off disk. Replaces the page's contents AND its
    /// last-saved baseline, so a freshly loaded config is not immediately
    /// "unsaved".
    SettingsLoaded(Box<settings_page::Settings>),

    /// Auto-login succeeded, or the login form was satisfied.
    LoggedIn,

    /// Who this installation is, read from disk or freshly minted.
    ///
    /// Carries the whole `Loaded` rather than a bare String because
    /// `Unavailable` is a real outcome the app must not paper over: `local_id`
    /// was the empty string for the entire life of this port, and an empty id
    /// draws an empty QR code, fails to filter your own gossip, and goes on
    /// the wire as a blank sender — all without an error.
    IdentityLoaded(identity::Loaded),

    /// The address book, off disk. Replaces both the book and the display rows,
    /// so the roster is what was saved rather than what was hardcoded.
    ContactsLoaded(Box<contact_book::ContactBook>),

    /// Which snaps are already finished, off disk. Sent at start-up, before
    /// anything can populate the feed — a snap filtered after it has been
    /// shown has already been shown.
    SnapStateLoaded(Box<snap_state::SnapState>),
    /// A snap arrived from a peer, already attributed by the transport.
    /// `from` is the AUTHENTICATED sender — the connection's, not the
    /// frame's claim — because the two can differ and only one of them was
    /// established by anything.
    SnapArrived { from: String, frame: Box<snap_wire::SnapFrame> },
    /// Snaps the host received and wants displayed. Filtered against the
    /// finished set on the way in, which is the one place that filter can
    /// live without every producer having to remember it.
    SnapsForFeed(Vec<snaps::FeedEntry>),

    /// The devices this machine offers, from a probe.
    CamerasFound(Vec<camera_select::Device>),
    /// A capture actually delivered frames. Feeds the health record that
    /// decides which camera to open next time.
    CameraWorked { id: String, now_ms: i64 },
    /// A frame arrived from the capture thread.
    CameraFrame(frame_pump::RawFrame),
    /// Where the capture engine stands. Edge-triggered by the host (sent on
    /// change, not per frame), and shown on the camera page — the person
    /// aiming the camera is looking at the viewfinder, not at stderr.
    CaptureStatus(crate::capture::Status),
    /// A QR payload was decoded from the viewfinder or pasted in.
    QrPayload(String),
    /// Average colour of the scene, for the ambient palette.
    AmbientPixels { pixels: Vec<ambient_palette::Rgb>, side: u32 },

    /// A discovery record arrived on the subnet. `datagram_len` is carried
    /// because the app's own admission check uses it — gossip arrives from
    /// anyone, and an oversized record is one of the things that gets refused.
    GossipHeard {
        record: gossip::PeerRecord,
        datagram_len: usize,
        /// The verifying key the datagram carried, when it carried one. Kept
        /// beside the record rather than inside it: this is something the
        /// decode learned, not part of the peer view's currency.
        public: Option<[u8; 32]>,
    },
    /// Presence changed for a known peer.
    PeerPresence { id: String, online: bool },
    /// A peer's address was observed, for hole punching.
    PeerAddr { id: String, ip: [u8; 4], port: u16, now_ms: u64 },
    /// A peer acknowledged holding a payload, so the next frame can skip the
    /// body.
    PeerHasPayload { peer: String, hash: String },
    /// A peer announced the highest wire version it speaks.
    WireOffered { peer: String, theirs_max: u8 },
    /// Two STUN responses from the SAME local socket. Both are required
    /// together because the classification is a comparison, and one probe
    /// yields Unknown.
    NatProbed { local_port: u16, a: nat_punch::Reflexive, b: nat_punch::Reflexive },
    /// Punch addresses older than the window should go.
    ExpirePunchAddrs { now_ms: u64 },

    /// A vCard file was read (or could not be). The app imports it.
    VCardLoaded(Result<String, String>),
    /// The voice transport came up or went down.
    VoiceLink(bool),
    /// Voice frames are arriving from a contact while this side is not in
    /// a call: someone is calling. Fired by the host once per burst; the
    /// app decides whether to ring (it will not ring twice, and never for
    /// a stranger).
    CallRinging { from: String },
    /// A voice payload arrived from a peer.
    VoiceReceived { sender: String, payload: Vec<u8> },
    /// The mic produced a frame interval; ask the sender whether anything may
    /// leave.
    MicFrame,
    /// How loud that interval was, 0..1. Feeds the transmit gate's activity
    /// score AND the on-screen meter from ONE measurement — two sources
    /// would eventually disagree about whether you are speaking, and the
    /// meter is how a person checks that the mic works at all.
    MicActivity(f32),
    /// Whether the host can turn samples into payloads. False is a real
    /// state, not an error: with no encoder the call joins, the floor works,
    /// and nothing leaves — silence the user cannot otherwise account for.
    EncoderReady(bool),
    /// A packet is ready to go out to everyone in the call.
    VoiceOutbound(voice_bridge::Packet),
    /// A decoded frame for the play-out cushion.
    /// Decoded PCM, and WHOSE it is.
    ///
    /// The peer used to be dropped at the decode boundary: it was known at
    /// receive and neither this nor `DecodeAudio` carried it, so decoded audio
    /// could not say whose it was and nothing per-contact -- leveling, EQ,
    /// individual volume -- could be applied.
    DecodedAudio { peer: String, samples: Vec<f32> },
    /// The audio device reports how much it still holds; answer with what to
    /// write.
    SinkDepth(usize),
    /// Measured speaking level for a participant, and for the room's own
    /// speaker meter.
    SpeakerLevel { peer: String, level: f32 },

    /// Snaps arrived. `focused` is the window's own state and travels with the
    /// event rather than being read from the app, because whether to notify is
    /// a question about the moment the snaps landed.
    SnapsArrived { from: Vec<String>, focused: bool },
    /// A send was acknowledged.
    /// A send did not get there. Distinct from never having tried: the row
    /// the sender watches must be able to reach Failed, or a send that
    /// broke is indistinguishable from one still in flight and the retry
    /// button never lights.
    SnapSendFailed { id: String, now_ms: u64 },
    /// A snap reached its recipient.
    ///
    /// Carries `now_ms` like `CameraWorked` and `PeerAddr` do, because
    /// two outboxes need marking and one of them records WHEN delivery
    /// happened. The variant had no timestamp while only the storage
    /// outbox was updated, and that one does not need one.
    SnapDelivered { id: String, now_ms: u64 },
    /// Someone asked to connect.
    ConnectionRequest { id: u64, peer: String, at: i64 },
    /// The host finished (or failed) an export it was asked to run.
    ExportFinished(snap_export::Outcome),
    /// The host acted on `CaptureSnap`: the file it wrote, or why it could
    /// not. Routed to the shutter note rather than to stderr — the person who
    /// pressed the shutter is looking at the camera page, and a photo that
    /// silently did not save is the failure they find out about last.
    SnapSaved(Result<String, String>),
    /// The host wrote (or could not write) my code's SVG. Its own event, NOT `SnapSaved`:
    /// that one moves the send flow to Compose, and a saved SVG is not a captured photo.
    CodeSvgSaved(Result<String, String>),
    /// The host wrote (or could not write) a RECEIVED snap the user chose to
    /// keep. Not `SnapSaved`, which is the capture flow's step one and moves it
    /// to Compose -- keeping a friend's snap must not open the composer.
    SnapKept(Result<String, String>),
    /// The host encoded the shutter's frame for the wire: the bytes that will
    /// go out. Feeds the pre-send receipt's Size facet, which showed
    /// "Sending 0 B" for every snap because nothing told the app what the
    /// encoder produced — the number was in the host's log and nowhere else.
    SnapEncoded { bytes: u64 },
    /// A DHT lookup produced a value.
    DhtValue(Vec<u8>),
    /// A DHT lookup finished. The bool command reports whether this was the
    /// first completion — a second `done` on the same lookup is a
    /// use-after-free in the C++ original and is counted rather than obeyed.
    DhtDone,

    /// The on-screen keyboard moved. A rectangle in screen coordinates, not a
    /// height — see `keyboard_inset` for why the difference is load-bearing.
    KeyboardChanged(keyboard_inset::Rect),
    /// Window focus changed.
    WindowFocus(bool),
    /// A gaze sample from the face pipeline.
    Gaze { sample: gaze::GazeSample, dt_ms: f32 },
    /// The face pipeline lost the subject.
    GazeLost,
}

/// Something the app wants the world to do.
///
/// Returned rather than performed, for the same reason the events are data:
/// the app has no business knowing whether "export" means a zip, an rsync or a
/// test double.
#[derive(Debug, Clone, PartialEq)]
pub enum HostCommand {
    /// Capture a frame — the shutter was pressed. `crop` is the zoom
    /// window (`[u0, v0, u1, v1]`, frame fractions) the viewfinder was
    /// showing, `None` at 1×: the host crops the frame to it before
    /// encoding, so the photo is what was on screen. It travels with the
    /// command for the same reason the export folder does — the host does
    /// not read the viewfinder's state behind the app's back.
    CaptureSnap { crop: Option<[f32; 4]> },
    /// Run the export the user asked for, into `dest_dir` (Settings >
    /// Storage > export folder; empty means the host's default, the home
    /// directory). The destination travels with the command so the host does
    /// not read the settings page's state behind the app's back.
    ExportSnaps { dest_dir: String },
    /// Retry the sends that failed.
    RetryFailedSends,
    /// Write these snaps out; the user chose to keep them.
    KeepSnap(String),
    /// Write my code as an SVG — the kit's `qr::svg`, which draws itself in — somewhere
    /// shareable, and report the path back through `CodeSvgSaved` so the note says where.
    SaveCodeSvg(String),
    /// Mark as seen in storage.
    MarkViewed(String),
    /// The view window elapsed. Distinct from `MarkViewed`: one is "did they
    /// see it", the other is "is the window over", and storage does different
    /// things with each.
    MarkExpired(String),
    /// Put dictated text into a field.
    ApplyDictation { field: String, text: String },
    /// Write these frames to the audio device.
    PlayAudio(Vec<Vec<f32>>),
    /// Send these payloads, one per peer.
    SendVoice(Vec<(String, Vec<u8>)>),
    /// What the transmit half decided about this mic interval.
    VoiceEmit(voice_send::Emit),
    /// A voice payload arrived and was routed. Decode it and hand the
    /// samples back as `DecodedAudio`.
    ///
    /// The codec belongs to the host, which is why this is a round trip
    /// rather than the app calling a decoder: the app knows about frames of
    /// f32 and nothing about how they were packed. Before this the decoded
    /// packet was matched on and DROPPED — inbound audio reached the bridge
    /// and stopped there, so a call was silent in both directions.
    /// `codec` says what the bytes are: raw S16LE or one Opus packet.
    DecodeAudio { peer: String, codec: crate::voice_codec::Codec, pcm: Vec<u8> },
    /// An Opus frame with its sequence number and talk-spurt flag, for the
    /// receive path (`voice_rx`): it is played in sequence order at the next
    /// play-out tick, and a missing one is concealed.
    OpusArrived { peer: String, seq: u32, spurt_start: bool, data: Vec<u8> },
    /// Show a notification. Only produced when the app decided one is
    /// warranted — an unfocused window and at least one sender.
    Notify(uikit_egui::notify::Rendered),
    /// Discovery admitted (true) or refused (false) a gossip record. Surfaced
    /// because a refused record is a security event worth logging, and the
    /// return value of `on_gossip` was otherwise dropped on the floor.
    GossipAdmitted(bool),
    /// The NAT classification succeeded (true) or the two probes disagreed
    /// about their local port (false).
    NatClassified(bool),
    /// A QR payload latched onto an open scanner.
    QrLatched(bool),
    /// Send the most recent capture to these people, with what was composed
    /// over it.
    ///
    /// WHO, and the composition — not the pixels. The app chose the
    /// recipients and the caption, colour and TIMER; the host knows which
    /// bytes the last shutter press produced and how to put them on a
    /// socket. Passing the payload through the app would mean the
    /// controller holding a photo in memory for no reason other than to
    /// hand it straight back — but the composition has to come from here,
    /// because the host has no other source for it: when the command was
    /// `{ to }` alone the bin wrote `duration_secs = 0` onto the wire, and on
    /// this wire zero means the snap never vanishes.
    ///
    /// `snap` is `None` for the palette's bare "send to" (no composer behind
    /// it); the host then sends with the composer's defaults.
    SendSnap { to: Vec<String>, snap: Option<crate::composer::Snap> },
    /// Write the finished-snap set out; it changed. Carries the value for
    /// the same reason `SaveContacts` does — a host writing from another
    /// thread must not have to borrow the app.
    SaveSnapState(Box<snap_state::SnapState>),
    /// Write the address book out; it changed. Carries the book rather than
    /// signalling "go read the app's field", so a host can write it from
    /// another thread without borrowing the app.
    SaveContacts(Box<contact_book::ContactBook>),
    /// A contact's announcement was signed by a key that is not the one on
    /// file. Either they reinstalled or somebody ground a keypair to collide
    /// with their sc-ID, and those are indistinguishable from here — so this
    /// reaches the user rather than being resolved automatically.
    PeerKeyChanged(String),
    /// No identity could be established. The app runs, but nobody can add this
    /// device, and the QR code it shows is empty — the host should say so
    /// rather than let that be discovered socially.
    IdentityUnavailable,
    /// A DHT lookup completed for the first time (true), or `done` arrived
    /// twice for the same lookup (false) — which the C++ original treats as a
    /// use-after-free, so it is surfaced rather than swallowed.
    DhtFinished(bool),
}

impl Snapclone {
    /// Apply one event from the world, and collect everything the app wants
    /// done as a result — including whatever the UI frame requested before it.
    ///
    /// This is the only function a host needs to call. See the module header
    /// for why it drains rather than leaving that to the caller.
    pub fn apply(&mut self, ev: HostEvent) -> Vec<HostCommand> {
        let mut out = Vec::new();
        match ev {
            HostEvent::Tick { now_secs } => {
                self.now_secs = now_secs;
                // The five-minute arming window for the proximity screensaver.
                // Its own tick because it depends on when voice last arrived,
                // not on frame time.
                self.refresh_posture_arming(now_secs);
            }
            HostEvent::SettingsLoaded(s) => {
                // Both halves: a load that set only `current` would leave the
                // page reporting unsaved changes against a default it never
                // showed, and the Save button lit on a fresh start.
                self.settings = settings_page::SettingsPage::new(*s);
                self.apply_live_settings();
            }
            HostEvent::LoggedIn => self.login_succeeded(),
            HostEvent::IdentityLoaded(loaded) => {
                if let identity::Loaded::Existing(i) | identity::Loaded::Created(i) = &loaded {
                    self.identity = Some(i.clone());
                }
                match loaded.sc_id() {
                    Some(id) => self.local_id = id.to_string(),
                    // Left empty deliberately and reported, rather than
                    // substituted. Everything downstream already behaves badly
                    // with an empty id; the difference is that the host now
                    // knows and can say so instead of the user finding out when
                    // nobody can add them.
                    None => out.push(HostCommand::IdentityUnavailable),
                }
            }
            HostEvent::ContactsLoaded(book) => {
                self.peers = book.to_peers();
                self.contact_book = *book;
                self.sync_roster();
                // Loading is not a change: writing the file straight back on
                // every launch would rewrite it for nothing and lose the
                // ability to tell a real edit from a start-up.
                self.contacts_dirty = false;
            }

            HostEvent::CamerasFound(list) => {
                self.cameras = list;
                // The list without a choice is what kept the page on "no
                // camera found" while the picker showed two — everything
                // downstream (describe line, capture plan) hangs off
                // `camera`, and nothing set it.
                self.camera = crate::camera_source::choose_from_enumerated(
                    &self.cameras, &self.settings.current.camera.device);
            }
            HostEvent::CameraWorked { id, now_ms } => self.note_camera_worked(&id, now_ms),
            HostEvent::CameraFrame(f) => {
                // Into the recording FIRST, while the frame is still owned
                // here: `frames.offer` moves it into the pump.
                //
                // The sink discards for now — there is no encoder linked (§15
                // step 2) — but the admission rules, the frame count and the
                // measured rate are all real, so what the app reports about a
                // recording is measured rather than assumed. Dropping the
                // pixels is also the correct shape: `video_record` keeps none
                // either, and a sink that buffered them would be the voice-kit
                // leak again.
                if let Some(r) = self.recorder.as_mut() {
                    let _ = r.offer(&f, &mut |_| {});
                }
                self.frames.offer(f)
            }
            HostEvent::CaptureStatus(st) => self.capture_status = st,
            HostEvent::SnapStateLoaded(st) => {
                self.snap_state = *st;
                // Loading is not a change. Marking it dirty here would write
                // the file straight back on every launch, which is a disk
                // write and a chance to truncate it for no reason at all.
                self.snap_state_dirty = false;
            }
            HostEvent::SnapArrived { from, frame } => {
                // THE SEQUENCE IS OBSERVED. `observe_seq` was written,
                // tested and never called, so the ack this device sends
                // back carried nothing and a peer had no way to learn what
                // had arrived. Advancing only — reordered frames are
                // normal, and letting a late low sequence move the ack
                // backwards makes the peer re-send everything after it.
                self.sessions.entry(&from).observe_seq(frame.header.seq);
                // The bytes, kept, or nothing can ever be opened.
                self.keep_received(frame.to_snap(&from));
                // And the ambient palette is told: it decides itself whether
                // this arrival is worth a derivation.
                let _ = self.ambient_snap_arrived(self.now_secs.max(0) as u64 * 1000);
                let mut row = frame.to_feed_entry(&from);
                // The LOCAL name for a contact beats what the sender calls
                // itself: the tile read "sc-8fd30abd" for a friend the
                // book knows as Ada, and a nickname the sender picks is
                // not a fact this side has to accept.
                if let Some(name) = self.contact_book.display_name(&from) { row.sender_name = name; }
                // The finished set is consulted for a SINGLE arrival too,
                // not only at a bulk feed load. A peer that re-sends a snap
                // you already viewed must not put it back — and re-sending
                // is the ordinary consequence of a retry, not an attack.
                if !self.snap_state.is_finished(&row.id) {
                    let mut rows = self.feed.entries().to_vec();
                    if !rows.iter().any(|e| e.id == row.id) {
                        // Newest first, matching what the inbox supplies —
                        // `set_entries` renders the order it is given.
                        rows.insert(0, row);
                        self.feed.set_entries(rows);
                    }
                }
            }
            HostEvent::SnapsForFeed(entries) => {
                self.feed.set_entries(self.snap_state.filter_unfinished(entries));
            }
            HostEvent::SnapEncoded { bytes } => {
                // JPEG (format 1 in the strip report's vocabulary), nothing
                // stripped — a fresh encode carries no camera metadata —
                // nothing already shared (the dedup line is a mediashare
                // fact this path does not have), and the recipient count is
                // whatever the picker holds now.
                let recipients = self.picker.selected().len() as i32;
                self.prepare_send(1, 0, bytes as i64, bytes as i64, recipients);
            }
            HostEvent::CodeSvgSaved(r) => {
                self.feed_note = match r {
                    Ok(p) => format!("my code saved: {}", p.rsplit('/').next().unwrap_or(&p)),
                    Err(e) => e,
                };
            }
            HostEvent::SnapKept(r) => {
                self.feed_note = match r {
                    Ok(p) => format!("kept: {}", p.rsplit('/').next().unwrap_or(&p)),
                    Err(e) => format!("could not keep that snap: {e}"),
                };
            }
            HostEvent::SnapSaved(r) => {
                // Flash only on SUCCESS. The universal camera confirmation
                // should not fire for a capture that produced no file — that
                // would be a worse lie than the silence it replaces.
                if let Ok(path) = &r {
                    self.snap_flash = Some(0.0);
                    // STEP 1 IS DONE: the flow moves to Compose, and the
                    // composer's controls appear under the frame.
                    self.send_flow.on_captured(crate::send_flow::Captured {
                        path: path.clone(),
                        at_ms: self.now_secs as u64 * 1000,
                    });
                }
                self.shutter_note = match r {
                    // The basename, not the path: a phone's data dir is
                    // sixty characters of XDG and the filename is the part
                    // that identifies the shot. The host prints the full
                    // path for anyone who needs it.
                    Ok(path) => format!("saved {}", path.rsplit('/').next().unwrap_or(&path)),
                    Err(why) => format!("not saved — {why}"),
                };
            }
            HostEvent::QrPayload(p) => out.push(HostCommand::QrLatched(self.qr_frame(&p))),
            HostEvent::AmbientPixels { pixels, side } => self.sample_ambient(&pixels, side),

            HostEvent::GossipHeard { record, datagram_len, public } => {
                out.push(HostCommand::GossipAdmitted(
                    self.on_gossip(record, datagram_len, public)));
            }
            HostEvent::PeerPresence { id, online } => {
                self.roster.mark_online(&id, online);
            }
            HostEvent::PeerAddr { id, ip, port, now_ms } => {
                self.note_peer_addr(&id, ip, port, now_ms)
            }
            HostEvent::PeerHasPayload { peer, hash } => self.peer_learned(&peer, &hash),
            HostEvent::WireOffered { peer, theirs_max } => {
                self.negotiate_wire(&peer, theirs_max);
            }
            HostEvent::NatProbed { local_port, a, b } => {
                out.push(HostCommand::NatClassified(self.classify_nat(local_port, &a, &b)));
            }
            HostEvent::ExpirePunchAddrs { now_ms } => {
                self.punch.expire(now_ms);
                // Gossip too, or the heard table grows for the life of the
                // app. A day: a peer not heard from since yesterday is a
                // bootstrap entry worth trying, not a live one.
                self.heard.purge_stale(now_ms, 24 * 60 * 60 * 1000);
            }

            HostEvent::VCardLoaded(r) => match r {
                Ok(text) => self.import_vcard_text(&text),
                Err(e) => self.import_report = Some(format!("vCard: {e}")),
            },
            HostEvent::VoiceLink(on) => self.voice_bridge.set_connected(on),
            HostEvent::CallRinging { from } => {
                // One sheet. Voice frames arrive fifty times a second and
                // each is "a call" from the socket's point of view; a
                // sheet re-presented per frame could never be answered.
                // And only someone in the book can ring: a stranger's
                // datagram at the voice port is a punch or a probe.
                if self.incoming_call.is_none() && !self.group_voice.joined
                    && self.contact_book.get(&from).is_some() {
                    self.ring(&from);
                }
            }
            HostEvent::VoiceReceived { sender, payload } => {
                // The return carries the routed packet; the app has already
                // applied any floor change to the coordinator by the time it
                // comes back, so there is nothing for the host to do with it.
                // `from`, not the raw `sender`: it is the bridge's resolved
                // identity, the same one its security rule rests on ("the
                // asserting peer IS the sender"). Per-contact processing keys on
                // who the bridge says this is, not on a transport address.
                // Both voice kinds, through the SAME bridge rules: an Opus
                // frame gets exactly the checks a raw one does.
                match self.voice_receive(&sender, &payload) {
                    crate::voice_bridge::Inbound::Handled { from, packet: crate::voice_bridge::Packet::VoiceFrame(f) } => {
                        out.push(HostCommand::DecodeAudio { peer: from, codec: crate::voice_codec::Codec::RawS16, pcm: f });
                    }
                    crate::voice_bridge::Inbound::Handled { from, packet: crate::voice_bridge::Packet::VoiceOpus { seq, spurt_start, data } } => {
                        out.push(HostCommand::OpusArrived { peer: from, seq, spurt_start, data });
                    }
                    _ => {}
                }
                // Arriving voice is what arms the proximity screensaver's
                // window, and the posture engine is told separately from the
                // codec path on purpose: one is about audio, the other about
                // whether the phone is at an ear.
                self.last_voice_at = Some(self.now_secs);
            }
            HostEvent::MicFrame => out.push(HostCommand::VoiceEmit(self.voice_frame())),
            // The mic reports at the frame pump's rate, and the floor's
            // hold is in milliseconds — so the tracker needs a step. A
            // constant rather than a measured dt because these events
            // carry no timestamp; being slightly out only shifts how long
            // a silent talker keeps the floor, which is a feel setting.
            HostEvent::MicActivity(level) => {
                let l = level.clamp(0.0, 1.0);
                self.sender.on_activity(l);
                self.voicebar.set_mic_level(l);
                // YOU ONLY HOLD THE FLOOR WHILE TRANSMITTING. The mic is
                // live whenever the call is, so without this gate the bar
                // says "You" for a room the other end cannot hear —
                // which is worse than saying nothing, because it looks
                // like confirmation that you are being heard.
                let mine = if self.sender.is_transmitting() { l } else { 0.0 };
                self.voicebar.note_level("You", true, mine, MIC_TICK_MS);
                // AND INTO THE GROUP, the way every REMOTE speaker already
                // goes (`SpeakerLevel` below calls `group_voice.set_level`).
                // The local one was simply left out, so in Open mode -- the
                // default, where the active speaker is whoever is loudest in
                // `levels` -- you could never be the active speaker and your
                // own ring never lit.
                //
                // `mine`, not `l`: the same gate as the voicebar line above
                // and for the same reason. Feeding the raw level would make
                // you "loudest" while muted or not transmitting, and the
                // roster would say you are speaking to a room that cannot hear
                // you -- the exact false confirmation that comment warns of.
                //
                // Guarded on a known id: before the identity resolves,
                // `local_id` is empty, and `set_level` ignores ids that are not
                // participants anyway -- but naming the case beats relying on
                // a lookup to fail.
                if !self.local_id.is_empty() {
                    let me = self.local_id.clone();
                    self.group_voice.set_level(&me, mine);
                }
            }
            HostEvent::EncoderReady(ready) => {
                self.sender.set_encoder_ready(ready);
                // `note_no_encoder` reports ONCE by design — the bool is
                // whether this is the first time — because saying it every
                // frame at fifty hertz would bury the log it belongs in.
                if !ready && self.sender.is_transmitting() && self.sender.note_no_encoder() {
                    out.push(HostCommand::VoiceEmit(voice_send::Emit::NoEncoder));
                }
            }
            HostEvent::VoiceOutbound(p) => out.push(HostCommand::SendVoice(self.voice_send(&p))),
            HostEvent::DecodedAudio { peer, mut samples } => {
                // Processed BEFORE the sink, per contact, so each speaker's
                // leveler and EQ see only their own audio.
                self.voice_fx.process(&peer, &mut samples);
                self.sink_push(samples);
            }
            HostEvent::SinkDepth(depth) => {
                let frames = self.sink_tick(depth);
                if !frames.is_empty() { out.push(HostCommand::PlayAudio(frames)); }
            }
            HostEvent::SpeakerLevel { peer, level } => {
                self.group_voice.set_level(&peer, level);
                self.voice_room.set_speaker_level(level);
                // The peer's DISPLAY name, not its id: the centre line is
                // read at arm's length by someone holding the phone, and a
                // 52-character key there is the same as no label.
                // The nickname when there is one, the id otherwise. A
                // 52-character key on the centre line is the same as no
                // label to someone holding the phone at arm's length.
                let name = self.contact_book.get(&peer)
                    .map(|c| c.nickname.clone())
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| peer.clone());
                self.voicebar.note_level(&name, false, level, MIC_TICK_MS);
            }

            HostEvent::SnapsArrived { from, focused } => {
                if let Some(n) = self.on_snaps_arrived(&from, focused) {
                    out.push(HostCommand::Notify(n));
                }
                // Wake the screensaver whether or not a notification was sent:
                // a snap that arrives while the phone is face-down is exactly
                // when the screen should come back.
                self.posture.notify_snap_arrived();
            }
            HostEvent::SnapSendFailed { id, now_ms } => {
                self.send_flow.on_failed(&id, now_ms);
                for r in self.sends.rows_mut() {
                    if r.id == id { r.failed(now_ms); }
                }
                // THE ADDRESS IS NOTED, not just the send. Without this a
                // cached address that no longer answers is retried
                // forever, and every attempt costs the person a timeout.
                if let Some(a) = self.punch.get(&id) {
                    self.addr_health.failed(&id, a, now_ms as i64);
                }
            }
            HostEvent::SnapDelivered { id, now_ms } => {
                self.addr_health.worked(&id, now_ms as i64);
                // BOTH outboxes, and the two are not redundant — see the
                // note on `Snapclone::sends`. `crate::outbox` owns a
                // snap's lifetime in storage; `send_state::Outbox` owns
                // what the SENDER SEES.
                //
                // Only the first was marked, so a row in `sends` could
                // never reach delivered: it could only age out. That is
                // the difference between "sent, confirmed" and "sent,
                // gave up waiting", and the sender was always shown the
                // second one.
                //
                // Found by `testonly_check`: `send_state::ack` was
                // defined, documented, tested, and called by nothing.
                self.outbox.mark_delivered(&id);
                self.sends.ack(&id, now_ms);
                self.send_flow.on_delivered(&id, now_ms);
            }
            HostEvent::ConnectionRequest { id, peer, at } => {
                self.inbox.connection_request(id, &peer, at)
            }
            HostEvent::ExportFinished(o) => self.export_finished(o),
            HostEvent::DhtValue(v) => self.dht_lookup.on_value(v),
            HostEvent::DhtDone => {
                out.push(HostCommand::DhtFinished(self.dht_lookup.on_done()));
            }

            HostEvent::KeyboardChanged(r) => { self.on_keyboard(r); }
            HostEvent::WindowFocus(_) => {
                // Focus is carried on SnapsArrived, where the decision is
                // actually made. Accepted as an event anyway so a host can
                // report it in one place rather than caching it to attach
                // later — and so this arm is the one obvious place to hang
                // focus-dependent behaviour when there is some.
            }
            HostEvent::Gaze { sample, dt_ms } => self.feed_gaze(sample, dt_ms),
            HostEvent::GazeLost => self.gaze_lost(),
        }
        out.extend(self.drain_commands());
        out
    }

    /// Collect what the UI frame asked for, clearing it as it goes.
    ///
    /// `apply` already calls this, so a host using `apply` does not need it —
    /// calling both for one event gives the second an empty list rather than a
    /// duplicate. It is public for a host that ran a frame with no event to
    /// deliver.
    ///
    /// Order is fixed and capture comes first: a frame in which the user hit
    /// the shutter and then asked to export should capture before the export
    /// runs, or the new snap misses the archive it was clearly meant to be in.
    pub fn drain_commands(&mut self) -> Vec<HostCommand> {
        let mut out = Vec::new();
        if std::mem::take(&mut self.shutter_pressed) {
            let z = &self.viewfinder.zoom;
            out.push(HostCommand::CaptureSnap { crop: z.is_zoomed().then(|| z.window()) });
        }
        for id in std::mem::take(&mut self.snaps_saved) { out.push(HostCommand::KeepSnap(id)); }
        if let Some(svg) = self.code_svg_requested.take() { out.push(HostCommand::SaveCodeSvg(svg)); }
        for id in std::mem::take(&mut self.snaps_viewed) { out.push(HostCommand::MarkViewed(id)); }
        for id in std::mem::take(&mut self.snaps_expired) { out.push(HostCommand::MarkExpired(id)); }
        for (field, text) in std::mem::take(&mut self.dictated) {
            out.push(HostCommand::ApplyDictation { field, text });
        }
        for id in std::mem::take(&mut self.key_conflicts) {
            out.push(HostCommand::PeerKeyChanged(id));
        }
        if let Some(req) = self.send_request.take() {
            out.push(HostCommand::SendSnap { to: req.to, snap: Some(req.snap) });
        }
        let to = std::mem::take(&mut self.send_requested);
        if !to.is_empty() {
            out.push(HostCommand::SendSnap { to, snap: None });
        }
        if std::mem::take(&mut self.snap_state_dirty) {
            out.push(HostCommand::SaveSnapState(Box::new(self.snap_state.clone())));
        }
        if std::mem::take(&mut self.contacts_dirty) {
            out.push(HostCommand::SaveContacts(Box::new(self.contact_book.clone())));
        }
        if std::mem::take(&mut self.export_requested) {
            out.push(HostCommand::ExportSnaps { dest_dir: self.settings.saved().storage.export_dir.clone() });
        }
        if std::mem::take(&mut self.retry_requested) { out.push(HostCommand::RetryFailedSends); }
        out
    }
}

/// The outcome of an ambient-inbox arrival, re-exported so a host does not
/// have to name the module to match on what `apply` did.
pub use ambient_inbox::Outcome as AmbientOutcome;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Peer;

    fn app() -> Snapclone {
        Snapclone::new(vec![
            Peer { name: "kitchen".into(), online: false },
            Peer { name: "studio".into(), online: true },
        ])
    }

    #[test]
    fn a_tick_reaches_the_clock_and_the_posture_arming() {
        // refresh_posture_arming was one of the ~35 methods nothing called. If
        // this arm is ever dropped, the proximity screensaver stops arming and
        // no other test in the crate notices, because every posture test drives
        // the engine directly.
        let mut s = app();
        s.apply(HostEvent::Tick { now_secs: 1_700_000_000 });
        assert_eq!(s.now_secs, 1_700_000_000);
    }

    #[test]
    fn loading_settings_replaces_the_baseline_and_not_only_the_values() {
        // A load that set `current` alone leaves the page reporting unsaved
        // changes against a default it never showed — Save lit on a fresh
        // start, which trains people to press it.
        let mut s = app();
        let mut cfg = settings_page::Settings::default();
        cfg.network.listen_port = 5555;
        s.apply(HostEvent::SettingsLoaded(Box::new(cfg)));
        assert_eq!(s.settings.current.network.listen_port, 5555);
        assert!(!s.settings.is_dirty(), "a freshly loaded config is not unsaved work");
    }

    #[test]
    fn login_dismisses_the_overlay() {
        let mut s = Snapclone::new_with_login(vec![], false);
        assert!(s.login_visible, "no auto-login, so the overlay is up");
        s.apply(HostEvent::LoggedIn);
        assert!(!s.login_visible);
    }

    #[test]
    fn presence_reaches_the_roster_rather_than_a_second_list() {
        let mut s = app();
        s.apply(HostEvent::PeerPresence { id: "kitchen".into(), online: true });
        assert!(s.roster.rows().iter().any(|r| r.id == "kitchen" && r.online),
                "presence did not reach the roster");
    }

    #[test]
    fn a_qr_payload_only_latches_onto_an_open_scanner() {
        let mut s = app();
        // Nothing open: the payload is reported as not latched rather than
        // silently building a contact from a code nobody asked to scan.
        let out = s.apply(HostEvent::QrPayload("snapclone://sc-deadbeef:9000".into()));
        assert!(out.contains(&HostCommand::QrLatched(false)));
        s.open_qr_scan(true);
        let out = s.apply(HostEvent::QrPayload("snapclone://sc-deadbeef:9000".into()));
        assert!(out.contains(&HostCommand::QrLatched(true)), "{out:?}");
    }

    #[test]
    fn a_gossip_record_is_admitted_or_refused_and_the_answer_is_not_dropped() {
        // `on_gossip` returns the admission decision and every previous caller
        // — there were none — would have thrown it away. A refused record is a
        // security event: gossip arrives from anyone on the subnet.
        let mut s = app();
        let rec = crate::gossip::PeerRecord {
            id: "studio".into(), host: "10.0.0.9".into(), port: 9876,
            last_seen_ms: 1_000, verified: true,
        };
        let out = s.apply(HostEvent::GossipHeard {
            record: rec, datagram_len: 64, public: None });
        assert!(out.iter().any(|c| matches!(c, HostCommand::GossipAdmitted(_))),
                "the decision was dropped: {out:?}");
    }

    #[test]
    fn arriving_voice_arms_the_screensaver_window() {
        // The posture engine's arming window keys off when voice last arrived,
        // and nothing was setting that — so the flip to gated talk could never
        // fire in a running app however long the call went on.
        let mut s = app();
        s.apply(HostEvent::Tick { now_secs: 5_000 });
        assert_eq!(s.last_voice_at, None);
        s.apply(HostEvent::VoiceReceived { sender: "studio".into(), payload: vec![0u8; 8] });
        assert_eq!(s.last_voice_at, Some(5_000));
    }

    #[test]
    fn an_empty_sink_tick_produces_no_command_rather_than_an_empty_one() {
        // A PlayAudio carrying nothing would have the host write zero frames
        // to the device every tick, which is a wakeup for no reason at fifty
        // hertz for the length of a call.
        let mut s = app();
        let out = s.apply(HostEvent::SinkDepth(9_999));
        assert!(!out.iter().any(|c| matches!(c, HostCommand::PlayAudio(_))), "{out:?}");
    }

    #[test]
    fn snaps_arriving_while_the_window_is_focused_do_not_notify() {
        // An app that notifies about something already on screen is one people
        // mute, and then the real notification is gone too.
        let mut s = app();
        s.notify_enabled = true;
        let from = vec!["studio".to_string()];
        let out = s.apply(HostEvent::SnapsArrived { from: from.clone(), focused: true });
        assert!(!out.iter().any(|c| matches!(c, HostCommand::Notify(_))), "{out:?}");
        let out = s.apply(HostEvent::SnapsArrived { from, focused: false });
        assert!(out.iter().any(|c| matches!(c, HostCommand::Notify(_))), "{out:?}");
    }

    #[test]
    fn the_ui_frames_requests_come_back_through_apply_without_a_second_call() {
        // The whole reason apply drains: a host that has to remember to read a
        // dozen public fields will forget one, and a forgotten flag is either
        // an export that never runs or one that runs every frame forever.
        let mut s = app();
        s.shutter_pressed = true;
        s.export_requested = true;
        s.snaps_saved.push("snap-1".into());
        let out = s.apply(HostEvent::Tick { now_secs: 1 });
        assert!(out.iter().any(|c| matches!(c, HostCommand::CaptureSnap { .. })));
        assert!(out.iter().any(|c| matches!(c, HostCommand::ExportSnaps { .. })));
        assert!(out.contains(&HostCommand::KeepSnap("snap-1".into())));
    }

    /// The export goes where Settings > Storage says -- the saved value, which
    /// is what the user confirmed.
    #[test]
    fn the_export_carries_the_folder_the_settings_chose() {
        let mut s = app();
        s.settings.persist_field(|x| x.storage.export_dir = "/media/sd/backup".into());
        s.export_requested = true;
        let out = s.drain_commands();
        assert!(out.contains(&HostCommand::ExportSnaps { dest_dir: "/media/sd/backup".into() }), "{out:?}");
    }

    #[test]
    fn capture_is_ordered_before_export_within_one_frame() {
        // A frame in which the user hit the shutter and then asked to export
        // should capture first, or the new snap misses the archive it was
        // obviously meant to be in.
        let mut s = app();
        s.shutter_pressed = true;
        s.export_requested = true;
        let out = s.drain_commands();
        let cap = out.iter().position(|c| matches!(c, HostCommand::CaptureSnap { .. })).unwrap();
        let exp = out.iter().position(|c| matches!(c, HostCommand::ExportSnaps { .. })).unwrap();
        assert!(cap < exp, "{out:?}");
    }

    #[test]
    fn a_drained_request_does_not_come_back_a_second_time() {
        // The failure this prevents is an export that runs on every frame for
        // the life of the process.
        let mut s = app();
        s.export_requested = true;
        assert!(!s.drain_commands().is_empty());
        assert!(s.drain_commands().is_empty(), "the flag was not cleared");
        // and apply agrees, rather than having its own copy of the logic
        s.retry_requested = true;
        assert!(s.apply(HostEvent::Tick { now_secs: 2 })
                 .contains(&HostCommand::RetryFailedSends));
        assert!(s.apply(HostEvent::Tick { now_secs: 3 })
                 .iter().all(|c| *c != HostCommand::RetryFailedSends));
    }

    #[test]
    fn loading_an_identity_fills_the_id_that_was_empty_for_the_whole_port() {
        // local_id was String::new() at construction and nothing ever set it,
        // so my_code_ui drew a QR for the empty string, the self-gossip filter
        // never matched, and the handshake carried a blank sender. None of the
        // three failed loudly.
        let mut s = app();
        assert_eq!(s.local_id, "");
        let id = identity::create().expect("entropy");
        let want = id.sc_id.clone();
        s.apply(HostEvent::IdentityLoaded(identity::Loaded::Existing(id)));
        assert_eq!(s.local_id, want);
    }

    #[test]
    fn no_identity_is_reported_rather_than_faked() {
        // The app still runs — refusing to start over identity would be worse —
        // but the host has to be told, or the user discovers it socially when
        // someone tries to scan a blank code.
        let mut s = app();
        let out = s.apply(HostEvent::IdentityLoaded(identity::Loaded::Unavailable));
        assert!(out.contains(&HostCommand::IdentityUnavailable));
        assert_eq!(s.local_id, "", "and nothing plausible was substituted");
    }

    #[test]
    fn loaded_contacts_become_the_roster_and_do_not_look_like_an_edit() {
        // Marking the book dirty on load would rewrite the file on every
        // launch and destroy the app's ability to tell a real change from a
        // start-up.
        let mut s = app();
        let mut book = contact_book::ContactBook::default();
        book.upsert("sc-aabbccdd", 9000, 1_000);
        let out = s.apply(HostEvent::ContactsLoaded(Box::new(book)));
        assert_eq!(s.contact_book.len(), 1);
        assert!(s.peers.iter().any(|p| p.name == "sc-aabbccdd"),
                "the display rows were not rebuilt: {:?}",
                s.peers.iter().map(|p| &p.name).collect::<Vec<_>>());
        assert!(!out.iter().any(|c| matches!(c, HostCommand::SaveContacts(_))),
                "loading wrote the file back: {out:?}");
    }

    #[test]
    fn adding_a_contact_reaches_both_the_book_and_the_display_row() {
        // The parallel-state risk contact_book's header admits to. apply_friends
        // is the only writer of either and this is what fails if a change
        // updates one and forgets the other.
        let mut s = app();
        s.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: "sc-aabbccdd".into(), port: 9000 }));
        assert!(s.peers.iter().any(|p| p.name == "sc-aabbccdd"), "no display row");
        assert_eq!(s.contact_book.get("sc-aabbccdd").map(|c| c.port), Some(9000),
                   "not in the book, so it vanishes on restart");
    }

    #[test]
    fn standing_tells_the_row_what_the_book_knows() {
        // The badge logic behind the folded Friends cards: a booked contact
        // shows its trust, a discovered sc-ID offers Keep with the observed
        // port, and a hostname row offers nothing — a book keyed by id has
        // nowhere to put "192.168.1.5".
        use crate::friends::Standing;
        let mut s = app();
        s.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: "sc-aabbccdd".into(), port: 9000 }));
        match s.standing_of("sc-aabbccdd") {
            Standing::Known { danger, .. } => assert!(!danger, "a fresh add is not a conflict"),
            other => panic!("booked contact reads as {other:?}"),
        }
        // Discovered but not kept: a valid id absent from the book. With no
        // observed address the Keep falls back to the protocol default port
        // rather than inventing one.
        assert_eq!(s.standing_of("sc-11223344"),
                   Standing::Discovered { port: crate::qr::DEFAULT_PORT });
        assert_eq!(s.standing_of("192.168.1.5"), Standing::Plain);
    }

    #[test]
    fn an_arriving_snap_reaches_the_feed_once() {
        // The app has never received a snap. This is the arm that makes one
        // appear — and the same snap arriving twice (a retry, which is
        // ordinary) must not appear twice.
        let mut s = app();
        let frame = crate::snap_wire::SnapFrame::announce(
            1, 0, "sc-aabbccdd", "kit", crate::snaps::Media::Image, 1_700_000_000, 5,
            b"a photo".to_vec());
        for _ in 0..2 {
            s.apply(HostEvent::SnapArrived {
                from: "sc-aabbccdd".into(), frame: Box::new(frame.clone()) });
        }
        assert_eq!(s.feed.count(), 1, "a re-delivered snap appeared twice");
        assert_eq!(s.feed.entries()[0].sender_name, "kit");
    }

    #[test]
    fn a_vcard_with_snapclone_ids_lands_in_the_book_and_the_rest_are_counted() {
        let mut s = app();
        let text = "BEGIN:VCARD\nVERSION:4.0\nFN:Ada\nX-SNAPCLONE-ID:sc-aabbccdd\nEND:VCARD\n\
                    BEGIN:VCARD\nVERSION:4.0\nFN:No Id\nEND:VCARD\n\
                    BEGIN:VCARD\nVERSION:4.0\nEND:VCARD\n";
        let out = s.apply(HostEvent::VCardLoaded(Ok(text.into())));
        assert_eq!(s.contact_book.len(), 1);
        assert_eq!(s.contact_book.display_name("sc-aabbccdd").as_deref(), Some("Ada"));
        assert!(s.peers.iter().any(|p| p.name == "sc-aabbccdd"), "a row for the import");
        assert!(out.iter().any(|c| matches!(c, HostCommand::SaveContacts(_))), "and it is saved: {out:?}");
        let r = s.import_report.clone().unwrap_or_default();
        assert!(r.contains("1 added") && r.contains("1 without"), "{r}");
        // a read failure is reported, not swallowed
        s.apply(HostEvent::VCardLoaded(Err("no such file".into())));
        assert!(s.import_report.as_deref().unwrap_or("").contains("no such file"));
    }

    #[test]
    fn frames_from_a_contact_while_idle_ring_once_and_answering_joins_the_call() {
        // `ring()` had no caller outside its tests, and an idle socket was
        // polled with an empty peer table, so a caller's first frames were
        // dropped as a stranger's and nothing rang.
        let mut s = app();
        let mut book = crate::contact_book::ContactBook::default();
        book.upsert("sc-aabbccdd", 41234, 1);
        book.rename("sc-aabbccdd", "Ada");
        s.apply(HostEvent::ContactsLoaded(Box::new(book)));
        // A stranger cannot ring.
        s.apply(HostEvent::CallRinging { from: "sc-00000000".into() });
        assert!(s.incoming_call.is_none(), "a stranger rang");
        // A contact can, once: the sheet shows the nickname.
        s.apply(HostEvent::CallRinging { from: "sc-aabbccdd".into() });
        s.apply(HostEvent::CallRinging { from: "sc-aabbccdd".into() });
        let call = s.incoming_call.as_ref().expect("ringing");
        assert_eq!(call.caller_name, "Ada");
        assert!(s.modal.topmost(crate::modal::Layer::Confirm).is_some_and(|p| p.id == "call:sc-aabbccdd"));
        // Answering joins with the caller as a participant, through
        // set_in_call, so play-out starts and the peer table admits them.
        s.answer_call(crate::ring::Answer::Accepted);
        assert!(s.incoming_call.is_none());
        assert!(s.group_voice.joined);
        assert!(s.group_voice.participants.iter().any(|p| p.id == "sc-aabbccdd" && !p.is_self));
        assert!(s.modal.topmost(crate::modal::Layer::Confirm).is_none(), "the sheet is gone");
        // In a call, a ring is ignored rather than stacked.
        s.apply(HostEvent::CallRinging { from: "sc-aabbccdd".into() });
        assert!(s.incoming_call.is_none());
    }

    #[test]
    fn a_snap_already_viewed_is_not_put_back_by_a_re_send() {
        // The finished set is consulted for a single arrival too, not only
        // at a bulk feed load. Otherwise a sender's retry undoes the one
        // promise this app makes.
        let mut s = app();
        let frame = crate::snap_wire::SnapFrame::announce(
            1, 0, "sc-aabbccdd", "kit", crate::snaps::Media::Image, 0, 5,
            b"a photo".to_vec());
        let id = crate::snap_wire::content_id("sc-aabbccdd", b"a photo");
        s.snap_state.mark(&id, crate::snap_state::Mark::Viewed, 1_000);
        s.apply(HostEvent::SnapArrived {
            from: "sc-aabbccdd".into(), frame: Box::new(frame) });
        assert_eq!(s.feed.count(), 0, "a viewed snap came back on a re-send");
    }

    #[test]
    fn pressing_send_asks_the_host_to_send_rather_than_only_drawing_a_row() {
        // The button drew a progress row for a transfer that never started,
        // and the row aged out looking like a timeout.
        let mut s = app();
        s.send_requested = vec!["sc-aabbccdd".into()];
        let out = s.drain_commands();
        assert!(out.iter().any(|c| matches!(c,
            HostCommand::SendSnap { to, .. } if to == &vec!["sc-aabbccdd".to_string()])),
            "{out:?}");
        // Drained, not repeated: a flag left set would re-send the photo
        // every frame for the rest of the session.
        assert!(!s.drain_commands().iter().any(|c| matches!(c, HostCommand::SendSnap { .. })));
    }

    #[test]
    fn a_send_row_can_reach_failed_and_not_only_age_out() {
        // Both terminal states, or a broken send is indistinguishable from
        // one still in flight and the retry button never lights.
        let mut s = app();
        s.sends.push("sc-aabbccdd", 1_000);
        s.apply(HostEvent::SnapSendFailed { id: "sc-aabbccdd".into(), now_ms: 2_000 });
        assert!(s.sends.rows().iter().any(|r| r.state.can_retry()),
                "no row reached Failed: {:?}",
                s.sends.rows().iter().map(|r| r.state).collect::<Vec<_>>());
    }

    #[test]
    fn an_inbound_voice_frame_reaches_the_host_instead_of_being_dropped() {
        // The arm was `Packet::VoiceFrame(_) => {}`. A call could connect,
        // negotiate the floor, and play nothing — the payload reached the
        // bridge and stopped there.
        let mut s = app();
        // The link first: `voice_bridge::inbound` refuses everything while
        // disconnected, which is correct — a voice packet arriving before
        // the transport is up has no session to belong to.
        s.apply(HostEvent::VoiceLink(true));
        let payload = crate::voice_bridge::encode(
            &crate::voice_bridge::Packet::VoiceFrame(vec![1, 2, 3, 4]));
        let out = s.apply(HostEvent::VoiceReceived {
            sender: "sc-aabbccdd".into(), payload });
        let decoded = out.iter().find_map(|c| match c {
            HostCommand::DecodeAudio { pcm, .. } => Some(pcm.clone()), _ => None });
        assert_eq!(decoded, Some(vec![1, 2, 3, 4]),
                   "the frame never left the bridge: {out:?}");

        // A floor assert is NOT audio and must not become a decode request.
        let floor = crate::voice_bridge::encode(
            &crate::voice_bridge::Packet::FloorAssert { claim_ts: 1 });
        let out = s.apply(HostEvent::VoiceReceived {
            sender: "sc-aabbccdd".into(), payload: floor });
        assert!(!out.iter().any(|c| matches!(c, HostCommand::DecodeAudio { .. })), "{out:?}");
    }

    /// THE BOUNDARY THAT DROPPED THE PEER. Decoded audio must reach the
    /// processing for the contact who sent it, and ONLY that contact.
    #[test]
    fn decoded_audio_is_processed_for_the_contact_who_sent_it() {
        let mut s = app();
        let loud: Vec<f32> = (0..16_000 * 5)
            .map(|i| 0.6 * (std::f32::consts::TAU * 500.0 * i as f32 / 16_000.0).sin())
            .collect();
        let quiet: Vec<f32> = loud.iter().map(|x| x * 0.05).collect();
        // Many frames, as the decoder would deliver them.
        for chunk in loud.chunks(320) {
            s.apply(HostEvent::DecodedAudio { peer: "sc-loud".into(), samples: chunk.to_vec() });
        }
        for chunk in quiet.chunks(320) {
            s.apply(HostEvent::DecodedAudio { peer: "sc-quiet".into(), samples: chunk.to_vec() });
        }
        let lr = s.voice_fx.channel("sc-loud").level_reduction_db();
        let qr = s.voice_fx.channel("sc-quiet").level_reduction_db();
        assert!(lr < -6.0, "the loud contact should be turned down: {lr:.1} dB");
        assert!(qr > -1.0, "and the quiet one left alone -- routed by peer: {qr:.1} dB");
    }

    /// ...and the other half: the peer leaves `VoiceReceived` attached to the
    /// decode request, as the bridge's AUTHENTICATED sender.
    #[test]
    fn a_decode_request_carries_the_sender() {
        let mut s = app();
        // CONNECTED, or the bridge refuses the frame outright. My first version
        // skipped this, the frame was refused, `peers` came back EMPTY -- and
        // the assertion "every peer is the sender" passed VACUOUSLY over an
        // empty list. It tested nothing. The non-empty check below is what
        // stops that recurring.
        s.apply(HostEvent::VoiceLink(true));
        let frame = crate::voice_bridge::encode(
            &crate::voice_bridge::Packet::VoiceFrame(vec![0, 0, 1, 0]));
        let out = s.apply(HostEvent::VoiceReceived { sender: "sc-aabbccdd".into(), payload: frame });
        let peers: Vec<_> = out.iter().filter_map(|c| match c {
            HostCommand::DecodeAudio { peer, .. } => Some(peer.clone()), _ => None }).collect();
        assert_eq!(peers, vec!["sc-aabbccdd".to_string()],
                   "exactly one decode request, carrying its sender");
    }

    /// A snapclone in an Open-mode call with us as a participant.
    fn in_open_call(me: &str) -> Snapclone {
        let mut s = app();
        s.local_id = me.to_string();
        s.group_voice.set_participants(vec![
            crate::group_voice::Participant { id: me.into(), name: "Me".into(), is_self: true },
            crate::group_voice::Participant { id: "sc-peer".into(), name: "Peer".into(), is_self: false },
        ]);
        s.group_voice.mode = crate::group_voice::TalkMode::Open;
        s
    }

    /// THE REPORTED GAP. Open is the default mode, where the active speaker is
    /// whoever is loudest in `levels`. Remote speakers entered `levels` via
    /// `SpeakerLevel`; the local one never did, so you could never be the
    /// active speaker and your own ring never lit.
    #[test]
    fn in_open_mode_the_local_speaker_can_be_the_active_one() {
        let mut s = in_open_call("sc-me");
        s.sender.set_transmitting(true);
        s.apply(HostEvent::MicActivity(0.8));
        assert_eq!(s.group_voice.active_speaker(), Some("sc-me"),
                   "you must be able to be the loudest in an open room");
    }

    /// ...BUT ONLY WHILE TRANSMITTING. Feeding the raw level would make you
    /// "loudest" while muted, and the roster would say you are speaking to a
    /// room that cannot hear you -- the false confirmation host.rs warns of.
    #[test]
    fn a_muted_local_speaker_is_not_reported_as_speaking() {
        let mut s = in_open_call("sc-me");
        s.sender.set_transmitting(false);
        s.apply(HostEvent::MicActivity(0.9));
        assert_ne!(s.group_voice.active_speaker(), Some("sc-me"),
                   "a loud mic that is not transmitting must not light your ring");
    }

    /// The local and remote paths now agree: whoever is actually loudest wins,
    /// whether that is you or them.
    #[test]
    fn a_louder_peer_still_takes_the_ring_from_you() {
        let mut s = in_open_call("sc-me");
        s.sender.set_transmitting(true);
        s.apply(HostEvent::MicActivity(0.4));
        s.apply(HostEvent::SpeakerLevel { peer: "sc-peer".into(), level: 0.9 });
        assert_eq!(s.group_voice.active_speaker(), Some("sc-peer"));
    }

    /// Before identity resolves `local_id` is empty; that must not insert a
    /// phantom "" speaker.
    #[test]
    fn no_id_means_no_local_level() {
        let mut s = in_open_call("sc-me");
        s.local_id = String::new();
        s.sender.set_transmitting(true);
        s.apply(HostEvent::MicActivity(0.9));
        assert_ne!(s.group_voice.active_speaker(), Some(""));
    }

    #[test]
    fn one_measurement_drives_both_the_gate_and_the_meter() {
        // Two sources would eventually disagree about whether you are
        // speaking, and the meter is how a person checks the mic works.
        let mut s = app();
        s.apply(HostEvent::MicActivity(0.42));
        assert!((s.sender.activity() - 0.42).abs() < 1e-6);
        assert!((s.voicebar.mic_level() - 0.42).abs() < 1e-6,
                "the meter did not move with the gate");
        // Out of range is clamped, not trusted: a level above 1 would pin
        // the meter and skew the gate's own thresholds.
        s.apply(HostEvent::MicActivity(9.0));
        assert!((s.sender.activity() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_missing_encoder_is_said_once_and_only_while_transmitting() {
        // Silence with no explanation is the failure this exists to
        // prevent; a line per frame at fifty hertz is the failure it must
        // not become.
        let mut s = app();
        s.set_transmitting(true);
        let out = s.apply(HostEvent::EncoderReady(false));
        assert!(out.iter().any(|c| matches!(
            c, HostCommand::VoiceEmit(crate::voice_send::Emit::NoEncoder))), "{out:?}");
        let again = s.apply(HostEvent::EncoderReady(false));
        assert!(!again.iter().any(|c| matches!(
            c, HostCommand::VoiceEmit(crate::voice_send::Emit::NoEncoder))),
            "reported twice: {again:?}");
    }

    #[test]
    fn a_viewed_snap_is_not_offered_again_after_a_restart() {
        // End to end through the seam for the last command that had no
        // store. Before this, a snap was viewed until you quit and unviewed
        // afterwards — and the sender believes it is gone.
        let mut s = app();
        let feed = vec![
            crate::snaps::FeedEntry { id: "snap-1".into(), sender_name: "kit".into(),
                sent_at: 0, media: crate::snaps::Media::Image, thumb_path: String::new() },
            crate::snaps::FeedEntry { id: "snap-2".into(), sender_name: "kit".into(),
                sent_at: 0, media: crate::snaps::Media::Image, thumb_path: String::new() },
        ];
        s.apply(HostEvent::SnapsForFeed(feed.clone()));
        assert_eq!(s.feed.count(), 2, "a fresh install shows both");

        s.snaps_viewed.push("snap-1".into());
        s.snap_state.mark("snap-1", crate::snap_state::Mark::Viewed, 1_000);
        s.snap_state_dirty = true;
        let out = s.drain_commands();
        let saved = out.iter().find_map(|c| match c {
            HostCommand::SaveSnapState(st) => Some(st.clone()), _ => None });
        let saved = saved.expect(&format!("the mark never reached a save: {out:?}"));

        // Next launch: the state comes off disk and the same feed arrives.
        let mut next = app();
        next.apply(HostEvent::SnapStateLoaded(saved));
        next.apply(HostEvent::SnapsForFeed(feed));
        assert_eq!(next.feed.count(), 1, "the viewed snap came back");
        assert_eq!(next.feed.entries()[0].id, "snap-2");
    }

    #[test]
    fn loading_the_snap_state_does_not_ask_to_write_it_back() {
        // A launch that rewrites the file it just read is a disk write and a
        // chance to truncate it, for nothing.
        let mut s = app();
        let mut st = crate::snap_state::SnapState::default();
        st.mark("snap-1", crate::snap_state::Mark::Viewed, 1);
        let out = s.apply(HostEvent::SnapStateLoaded(Box::new(st)));
        assert!(!out.iter().any(|c| matches!(c, HostCommand::SaveSnapState(_))),
                "loading wrote the file back: {out:?}");
    }

    #[test]
    fn the_message_and_call_buttons_do_something() {
        // Both were `_ => {}` behind buttons drawn exactly like the two that
        // worked. This is what fails if either falls back through again.
        let mut s = app();
        s.local_id = "sc-11111111".into();
        s.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: "sc-aabbccdd".into(), port: 9876 }));

        s.apply_friends(crate::friends::Action::Invoke(
            "sc-aabbccdd".into(), "message".into()));
        assert_eq!(s.conversation_peer, "sc-aabbccdd", "Message opened nothing");

        s.apply_friends(crate::friends::Action::Invoke(
            "sc-aabbccdd".into(), "call".into()));
        assert!(s.group_voice.joined, "Call did not enter the room");
        assert!(s.group_voice.participants.iter().any(|p| p.id == "sc-aabbccdd"),
                "the person being called is not in the call");
        // No address is known, so the note says so NOW rather than after
        // thirty seconds of silence the user reads as a broken microphone.
        assert!(s.call_note.contains("no known address"), "{}", s.call_note);

        // With an address, the same action reports a call being placed.
        s.note_peer_addr("sc-aabbccdd", [10, 0, 0, 3], 9876, 1_000);
        s.apply_friends(crate::friends::Action::Invoke(
            "sc-aabbccdd".into(), "call".into()));
        assert_eq!(s.call_note, "calling sc-aabbccdd");
    }

    #[test]
    fn calling_twice_does_not_duplicate_anyone_in_the_room() {
        let mut s = app();
        s.local_id = "sc-11111111".into();
        s.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: "sc-aabbccdd".into(), port: 9876 }));
        for _ in 0..3 {
            s.apply_friends(crate::friends::Action::Invoke(
                "sc-aabbccdd".into(), "call".into()));
        }
        let n = s.group_voice.participants.iter()
            .filter(|p| p.id == "sc-aabbccdd").count();
        assert_eq!(n, 1, "three taps put them in the room {n} times");
        assert_eq!(s.group_voice.participants.iter().filter(|p| p.is_self).count(), 1,
                   "and we joined ourselves more than once");
    }

    #[test]
    fn a_device_with_no_identity_refuses_the_call_rather_than_half_placing_it() {
        // `group_voice::leave` releases the floor through `me()`, so a room
        // we are not a participant of cannot be left cleanly — and the far
        // end would get packets from a sender it cannot name. Gossip already
        // refuses to announce without a key; this is the same rule.
        let mut s = app();
        assert!(s.local_id.is_empty(), "the fixture is supposed to have no identity");
        s.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: "sc-aabbccdd".into(), port: 9876 }));
        s.apply_friends(crate::friends::Action::Invoke(
            "sc-aabbccdd".into(), "call".into()));
        assert!(!s.group_voice.joined, "we entered a call as nobody");
        assert!(s.call_note.contains("no identity"), "{}", s.call_note);
    }

    #[test]
    fn the_dht_only_asks_about_contacts_the_lan_cannot_see() {
        // Gossip is better evidence than a DHT record — signed, and it means
        // they are on this subnet right now. Asking the DHT about someone
        // two metres away spends traffic to learn something weaker, and on a
        // phone that traffic is a radio wakeup.
        let mut s = app();
        for id in ["sc-aaaaaaaa", "sc-bbbbbbbb"] {
            s.apply_friends(crate::friends::Action::Add(
                crate::friends::AddPeer { host: id.into(), port: 9876 }));
        }
        // One of them is right here.
        s.apply(HostEvent::PeerPresence { id: "sc-aaaaaaaa".into(), online: true });
        for p in s.peers.iter_mut() {
            if p.name == "sc-aaaaaaaa" { p.online = true; }
        }
        let offline: Vec<String> = s.contact_book.iter().map(|c| c.sc_id.clone())
            .filter(|id| !s.peers.iter().any(|p| p.name == *id && p.online))
            .collect();
        assert_eq!(offline, vec!["sc-bbbbbbbb".to_string()],
                   "the visible peer is being looked up anyway");
    }

    #[test]
    fn a_dht_record_becomes_a_dialable_address_not_just_a_note() {
        // It used to set `feed_note` and evaporate — nothing recorded the
        // address, so a peer "found via DHT" still could not be reached and
        // the message was a tease.
        let mut s = app();
        s.note_peer_addr("sc-aabbccdd", [203, 0, 113, 9], 9876, 5_000);
        let a = s.punch.get("sc-aabbccdd").expect("the DHT result went nowhere");
        assert_eq!((a.ip, a.port), ([203, 0, 113, 9], 9876));
    }

    #[test]
    fn a_learned_address_is_what_makes_a_peer_dialable() {
        // `PunchAddrs` was empty for the whole life of this port — nothing
        // called `note_peer_addr`, so no hole punch had a target and voice
        // had nowhere to send. This is the arm the binary now feeds from
        // ADMITTED gossip.
        let mut s = app();
        assert!(s.punch.is_empty(), "nothing should be known before any gossip");
        s.apply(HostEvent::PeerAddr {
            id: "sc-aabbccdd".into(), ip: [10, 0, 0, 7], port: 9876, now_ms: 1_000 });
        let a = s.punch.get("sc-aabbccdd").expect("the address was not recorded");
        assert_eq!((a.ip, a.port), ([10, 0, 0, 7], 9876));

        // And it does not last forever: a peer that moved networks must stop
        // authorising an ex-neighbour's address for voice admission.
        s.apply(HostEvent::ExpirePunchAddrs { now_ms: 1_000 + 60 * 60 * 1000 });
        assert!(s.punch.get("sc-aabbccdd").is_none(),
                "an hour-old address is still being offered as current");
    }

    #[test]
    fn the_shutter_says_whether_the_photo_was_saved() {
        // The failure this closes: `CaptureSnap` was answered on stderr, and
        // the person who pressed the shutter is looking at the camera page.
        // A photo that silently did not save is the one they find out about
        // last — usually when it is not there later.
        let mut s = app();
        s.apply(HostEvent::SnapSaved(Ok("/home/u/.local/share/snapclone/snaps/snap-42.ppm".into())));
        assert_eq!(s.shutter_note, "saved snap-42.ppm",
                   "the note shows the filename, not sixty characters of XDG path");
        s.apply(HostEvent::SnapSaved(Err("no live frame".into())));
        assert_eq!(s.shutter_note, "not saved — no live frame");
    }

    #[test]
    fn the_pick_toggle_is_not_the_card_toggle() {
        // These were one action for a while and it meant opening someone's
        // action menu silently added them to the next send. Separate
        // variants, and this is what fails if they are merged again.
        let mut s = app();
        s.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: "sc-aabbccdd".into(), port: 9000 }));
        s.apply_friends(crate::friends::Action::Selected("sc-aabbccdd".into()));
        assert!(!s.picker.selected().iter().any(|p| p == "sc-aabbccdd"),
                "opening the card added a send recipient");
        s.apply_friends(crate::friends::Action::TogglePick("sc-aabbccdd".into()));
        assert!(s.picker.selected().iter().any(|p| p == "sc-aabbccdd"),
                "the checkbox did not reach the picker");
    }

    #[test]
    fn reach_is_matched_by_name_not_by_a_synthesised_id() {
        // `peer_roster` zero-pads a name into a PeerId. Matching rows on
        // those bytes works today and breaks silently — every row reading
        // "no reach" — the moment real keys arrive.
        let mut s = app();
        s.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: "sc-aabbccdd".into(), port: 9000 }));
        let reach = vec![crate::peer_source::RosterPeer {
            id: [7u8; 32],           // deliberately NOT the padded name
            name: "sc-aabbccdd".into(),
            reach: crate::peer_source::Reach::Connected,
        }];
        let info = s.row_info("sc-aabbccdd", &reach);
        assert_eq!(info.reach, Some(crate::friends::Reach::Connected));
        assert_eq!(s.row_info("sc-99999999", &reach).reach, None,
                   "a row with no roster entry must not borrow another's reach");
    }

    #[test]
    fn a_scanned_contact_produces_a_save_and_keeps_its_port() {
        // End to end for the thing that made the app a demo: scan a code, and
        // the contact is still there next launch. The port travelling with it
        // is the second half — apply_friends used to read AddPeer.host and
        // ignore AddPeer.port entirely.
        let mut s = app();
        s.open_qr_scan(true);
        // The commands come back from `apply`, which drains as part of its
        // contract — draining again afterwards yields nothing, which is what
        // the first version of this test did and why it failed.
        let out = s.apply(HostEvent::QrPayload("snapclone://sc-aabbccdd:9000".into()));
        assert!(out.contains(&HostCommand::QrLatched(true)));
        // Latching is not applying. The scanner holds the success state for
        // SUCCESS_HOLD_MS so the user can see WHICH code was read before the
        // dialog closes, and `tick` is what turns that into a contact. I wrote
        // this test expecting the save on the latch and it was the test that
        // was wrong: reporting "added" before the confirmation the user is
        // being shown has finished would make the hold a lie.
        for _ in 0..10 { s.tick(0.1); }
        let out = s.drain_commands();
        let saved = out.iter().find_map(|c| match c {
            HostCommand::SaveContacts(b) => Some(b.clone()), _ => None });
        let saved = saved.expect(&format!("a scan produced no save: {out:?}"));
        assert_eq!(saved.get("sc-aabbccdd").map(|c| c.port), Some(9000));
    }

    #[test]
    fn a_manual_host_that_is_not_an_sc_id_still_shows_but_is_not_booked() {
        // The manual field accepts a hostname, and a book keyed by
        // "192.168.1.5" could never be matched against a peer that later
        // announces itself by id. The row appearing without being durable is
        // the honest state; a half-record keyed by an address is not.
        let mut s = app();
        s.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: "192.168.1.5".into(), port: 9876 }));
        assert!(s.peers.iter().any(|p| p.name == "192.168.1.5"));
        assert!(s.contact_book.is_empty());
        assert!(s.drain_commands().is_empty(), "and no pointless write");
    }

    #[test]
    fn deleting_a_contact_writes_the_book_so_it_stays_deleted() {
        // A delete that survives until you quit is the most confusing kind of
        // not working, because it looks like it worked.
        let mut s = app();
        s.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: "sc-aabbccdd".into(), port: 9876 }));
        let _ = s.drain_commands();
        s.confirm_delete("sc-aabbccdd");
        let out = s.drain_commands();
        let saved = out.iter().find_map(|c| match c {
            HostCommand::SaveContacts(b) => Some(b.clone()), _ => None })
            .expect(&format!("a delete produced no save: {out:?}"));
        assert!(saved.is_empty());
    }

    #[test]
    fn a_verified_record_brings_a_saved_contact_online_and_a_prune_takes_it_back_off() {
        // THE asymmetry that made discovery pointless. on_gossip added a row
        // for an unknown peer and did nothing at all for a known one, while
        // tick_gossip happily set online=false on prune — so a contact loaded
        // from the address book was permanently grey however loudly it was
        // broadcasting, and the Connect button never meant anything.
        let mut s = Snapclone::new(vec![]);
        let id = identity::create().expect("entropy");
        let mut book = contact_book::ContactBook::default();
        book.upsert(&id.sc_id, 9876, 1_000);
        s.apply(HostEvent::ContactsLoaded(Box::new(book)));
        assert!(s.peers.iter().all(|p| !p.online), "loaded contacts start offline");

        // Through the wire, not by hand: a record with verified set by fiat
        // would prove nothing about whether a real datagram can do this.
        let a = crate::gossip_wire::Announce {
            id: id.sc_id.clone(), host: "10.0.0.7".into(), port: 9876, ts: 5_000,
            peers: Vec::new(), public: None, verified: false,
        };
        let bytes = crate::gossip_wire::encode_signed(&a, &id.secret, &id.public).unwrap();
        let heard = crate::gossip_wire::decode(&bytes).unwrap();
        assert!(heard.verified);

        let out = s.apply(HostEvent::GossipHeard {
            record: heard.sender_record(), datagram_len: bytes.len(),
            public: heard.public });
        assert!(out.contains(&HostCommand::GossipAdmitted(true)), "{out:?}");
        assert!(s.peers.iter().any(|p| p.name == id.sc_id && p.online),
                "an admitted record did not bring the contact online");
        assert_eq!(s.peers.len(), 1, "and it did not add a duplicate row");

        // ...and it goes back off when the peer stops talking. Marked offline,
        // not removed: a device that stopped gossiping may just be asleep.
        s.tick_gossip(5_000 + crate::gossip::FAILURE_MS + crate::gossip::PRUNE_MS + 1);
        assert!(s.peers.iter().all(|p| !p.online), "a pruned peer stayed green");
    }

    #[test]
    fn a_first_signed_sighting_pins_the_key_and_saves_it() {
        let mut s = app();
        let id = identity::create().expect("entropy");
        let mut book = contact_book::ContactBook::default();
        book.upsert(&id.sc_id, 9876, 1_000);
        s.apply(HostEvent::ContactsLoaded(Box::new(book)));

        let a = crate::gossip_wire::Announce {
            id: id.sc_id.clone(), host: "10.0.0.7".into(), port: 9876, ts: 5_000,
            peers: Vec::new(), public: None, verified: false,
        };
        let bytes = crate::gossip_wire::encode_signed(&a, &id.secret, &id.public).unwrap();
        let heard = crate::gossip_wire::decode(&bytes).unwrap();
        let out = s.apply(HostEvent::GossipHeard {
            record: heard.sender_record(), datagram_len: bytes.len(), public: heard.public });

        assert!(out.contains(&HostCommand::GossipAdmitted(true)));
        assert_eq!(s.contact_book.get(&id.sc_id).unwrap().public, Some(id.public));
        assert!(out.iter().any(|c| matches!(c, HostCommand::SaveContacts(_))),
                "the pin was not written, so it is forgotten on restart: {out:?}");
    }

    #[test]
    fn a_second_key_for_a_pinned_contact_is_refused_and_reaches_the_user() {
        // A ground keypair colliding with a chosen sc-ID is ~2^32 work, so I am
        // not generating a real collision — I am supplying the two values the
        // decoder WOULD produce for one: the victim's id, and the attacker's
        // key. That is exactly the pair the pin exists to reject.
        let mut s = app();
        let victim = identity::create().unwrap();
        let attacker = identity::create().unwrap();
        let mut book = contact_book::ContactBook::default();
        book.upsert(&victim.sc_id, 9876, 1_000);
        s.apply(HostEvent::ContactsLoaded(Box::new(book)));
        s.contact_book.pin(&victim.sc_id, victim.public);

        let rec = crate::gossip::PeerRecord {
            id: victim.sc_id.clone(), host: "10.0.0.9".into(), port: 9876,
            last_seen_ms: 5_000, verified: true,
        };
        let out = s.apply(HostEvent::GossipHeard {
            record: rec, datagram_len: 200, public: Some(attacker.public) });

        assert!(out.contains(&HostCommand::GossipAdmitted(false)), "{out:?}");
        assert!(out.contains(&HostCommand::PeerKeyChanged(victim.sc_id.clone())),
                "refused silently, which makes a legitimate reinstall look like a broken \
                 network: {out:?}");
        assert_eq!(s.contact_book.get(&victim.sc_id).unwrap().public, Some(victim.public),
                   "the pinned key was replaced by the challenger");
        assert!(s.peers.iter().all(|p| !p.online), "the impostor brought the contact online");
    }

    #[test]
    fn an_event_with_nothing_to_report_yields_no_commands() {
        // Otherwise every host loop spins on a list it has to filter, and the
        // no-op case is the common one.
        let mut s = app();
        assert!(s.apply(HostEvent::WindowFocus(true)).is_empty());
        assert!(s.apply(HostEvent::GazeLost).is_empty());
    }
}

#[cfg(test)]
mod recording_wiring_tests {
    use super::*;

    fn frame(seq: u64, ms: u64) -> frame_pump::RawFrame {
        frame_pump::RawFrame {
            width: 32, height: 32, captured_ms: ms, seq,
            rgba: vec![0u8; 32 * 32 * 4].into(),
        }
    }

    #[test]
    fn camera_frames_reach_the_recorder_only_while_recording() {
        // The WIRING, which `video_record`'s own tests cannot see: they prove
        // the recorder counts correctly and pass perfectly with nothing
        // feeding it. Same split that hid the IME hook and the JPEG quality
        // clamp.
        let mut s = Snapclone::new(Vec::new());
        s.apply(HostEvent::CameraFrame(frame(1, 0)));
        assert!(s.recorder.is_none(), "a recorder appeared without a gesture");

        s.recorder = Some(crate::video_record::Recorder::new(
            crate::video_record::Limits::default()));
        s.apply(HostEvent::CameraFrame(frame(2, 100)));
        s.apply(HostEvent::CameraFrame(frame(3, 200)));
        let r = s.recorder.as_ref().expect("still recording");
        assert_eq!(r.frames(), 2, "camera frames are not reaching the recorder");
        assert!((r.duration_secs() - 0.1).abs() < 1e-3);
    }

    #[test]
    fn the_frame_still_reaches_the_pump_after_the_recorder_sees_it() {
        // The recorder borrows the frame before `frames.offer` moves it. If
        // that order were wrong the preview would freeze during every
        // recording, which is a worse bug than not recording at all.
        let mut s = Snapclone::new(Vec::new());
        s.recorder = Some(crate::video_record::Recorder::new(
            crate::video_record::Limits::default()));
        s.apply(HostEvent::CameraFrame(frame(1, 0)));
        assert!(s.frames.has_frame(), "the preview stopped getting frames");
    }
}

#[cfg(test)]
mod addr_health_host_tests {
    use super::*;

    fn app() -> Snapclone { Snapclone::new(Vec::new()) }

    #[test]
    fn repeated_send_failures_take_an_address_out_of_use() {
        // The end-to-end version of the rule: the events the host already
        // emits are what drive it, so nothing extra has to remember.
        let mut a = app();
        a.punch.set("sc-1", crate::punch_addrs::PeerAddr::new([10, 0, 0, 9], 9876), 0);
        assert!(a.addr_health.usable("sc-1", 0));
        for _ in 0..crate::store_privacy::ADDR_FAIL_THRESHOLD {
            a.apply(HostEvent::SnapSendFailed { id: "sc-1".into(), now_ms: 0 });
        }
        assert!(!a.addr_health.usable("sc-1", 0),
                "{} failures recorded", a.addr_health.failures("sc-1"));
    }

    #[test]
    fn a_delivery_puts_it_back_in_use() {
        let mut a = app();
        a.punch.set("sc-1", crate::punch_addrs::PeerAddr::new([10, 0, 0, 9], 9876), 0);
        for _ in 0..9 {
            a.apply(HostEvent::SnapSendFailed { id: "sc-1".into(), now_ms: 0 });
        }
        a.apply(HostEvent::SnapDelivered { id: "sc-1".into(), now_ms: 1_000 });
        assert!(a.addr_health.usable("sc-1", 1_000));
    }

    #[test]
    fn a_failure_with_no_known_address_records_nothing() {
        // "No known address" is a different problem from "this address
        // does not answer", and counting it as the second would blacklist
        // a peer whose address was simply never learned.
        let mut a = app();
        a.apply(HostEvent::SnapSendFailed { id: "sc-unknown".into(), now_ms: 0 });
        assert_eq!(a.addr_health.failures("sc-unknown"), 0);
        assert!(a.addr_health.usable("sc-unknown", 0));
    }

    /// THE WHOLE SEQUENCE, driven by events rather than a finger: shutter →
    /// the host's SnapSaved moves the flow to Compose → Next composes with the
    /// timer the person set → Send builds ONE command carrying who AND what →
    /// the host's delivered/failed answers settle the receipt. The bug this
    /// pins: the command carried only `to`, and the bin put `duration 0` —
    /// never vanishes — on the wire for every snap.
    #[test]
    fn a_snap_goes_out_with_its_timer_and_the_receipt_settles() {
        use crate::send_flow::Step;
        let mut s = app();
        assert_eq!(s.send_flow.step(), Step::Capture);
        s.shutter_pressed = true;
        assert!(s.drain_commands().iter().any(|c| matches!(c, HostCommand::CaptureSnap { .. })));
        s.apply(HostEvent::Tick { now_secs: 1_000 });
        s.apply(HostEvent::SnapSaved(Ok("/x/snaps/snap-7.jpg".into())));
        assert_eq!(s.send_flow.step(), Step::Compose, "a saved frame is what moves the flow on");
        assert_eq!(s.send_flow.captured().unwrap().path, "/x/snaps/snap-7.jpg");
        s.composer.caption = "  hello  ".into();
        s.composer.duration = crate::composer::Duration::Seconds(10);
        s.advance_to_recipients();
        assert_eq!(s.send_flow.step(), Step::Recipients);
        assert_eq!(s.nav.page(), 2, "Next opens the picker page");
        assert!(s.picker.is_multi(), "with Send armed");
        s.send_to(&["kitchen".into(), "studio".into()], 5_000);
        assert_eq!(s.send_flow.step(), Step::Sending);
        let cmds = s.drain_commands();
        let snap = cmds.iter().find_map(|c| match c {
            HostCommand::SendSnap { to, snap } if to == &vec!["kitchen".to_string(), "studio".to_string()] => snap.clone(),
            _ => None,
        }).expect("one SendSnap for both recipients, with the composed snap riding along");
        assert_eq!(snap.duration.to_secs(), 10, "THE TIMER TRAVELS");
        assert_eq!(snap.caption, "hello", "trimmed by compose()");
        assert!(s.picker.selected().is_empty(), "the selection does not carry to the next snap");
        assert!(!s.drain_commands().iter().any(|c| matches!(c, HostCommand::SendSnap { .. })), "sent once");
        // the rows the sender watches are keyed by recipient, so the host's answers reach them
        s.apply(HostEvent::SnapDelivered { id: "kitchen".into(), now_ms: 6_000 });
        s.apply(HostEvent::SnapSendFailed { id: "studio".into(), now_ms: 6_500 });
        assert_eq!(s.send_flow.step(), Step::Sent);
        assert_eq!(s.send_flow.receipt().headline(), "sent to 1, 1 failed");
        assert_eq!(s.sends.needs_attention().len(), 1, "the failed row is the one to retry");
        assert!(s.sends.rows().iter().any(|r| r.id == "kitchen" && r.state == crate::send_state::SendState::Delivered),
                "the delivered row reached Delivered — it could only age out before");
    }

    #[test]
    fn a_bare_palette_send_still_goes_out_without_a_composition() {
        let mut s = app();
        s.run_action(crate::palette::Action::SendTo { id: "kitchen".into(), name: "Kitchen".into() });
        let cmds = s.drain_commands();
        assert!(cmds.iter().any(|c| matches!(c, HostCommand::SendSnap { to, snap: None } if to == &vec!["kitchen".to_string()])));
    }

    #[test]
    fn the_encoded_size_reaches_the_receipt() {
        // "Sending 0 B" on every snap: the encoder's size lived in the host's
        // log and the receipt was never told.
        let mut s = app();
        s.apply(HostEvent::SnapEncoded { bytes: 16 * 1024 });
        assert_eq!(s.send_summary.upload_bytes, 16 * 1024);
        assert_eq!(s.send_summary.total_bytes, 16 * 1024, "nothing already shared on this path");
        assert!(!s.can_send(), "and still no recipients");
    }

    #[test]
    fn save_svg_becomes_a_command_and_its_answer_a_note_not_a_capture() {
        let mut s = app();
        s.code_svg_requested = Some("<svg/>".into());
        let cmds = s.drain_commands();
        assert!(cmds.iter().any(|c| matches!(c, HostCommand::SaveCodeSvg(v) if v == "<svg/>")));
        assert!(s.code_svg_requested.is_none(), "drained once");
        let before = s.send_flow.step();
        s.apply(HostEvent::CodeSvgSaved(Ok("/x/snaps/sc-1.svg".into())));
        assert_eq!(s.feed_note, "my code saved: sc-1.svg");
        assert_eq!(s.send_flow.step(), before, "an SVG is not a photo: the send flow does not move");
    }
}
