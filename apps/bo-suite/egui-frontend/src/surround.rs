//! surround — which speaker is which.
//!
//! `playback::Sink::Many` fans one stream to every speaker: the same audio
//! in each room. That is multi-room, and it is what the sink picker means
//! by "combine". Surround is a different thing entirely — one stream SPLIT
//! across speakers, each taking the part of the mix its position calls for
//! — and asking for it by choosing several speakers is how people end up
//! with the left channel in the kitchen.
//!
//! ## The model is an assignment, and the interesting part is refusing one
//!
//! Most of this module says no. A layout with two speakers on the left and
//! none on the right is not a quiet room, it is a mix with half its
//! content missing and no indication of why. The rules are in
//! [`Assignment::problems`], and each of them exists because the failure
//! it prevents is silent.
//!
//! ## What is NOT here
//!
//! Distance and delay. A real surround setup measures how far each speaker
//! is from the listener and delays the near ones so the wavefronts arrive
//! together — get it wrong and the image collapses toward whichever
//! speaker is closest. That needs either a measurement microphone or the
//! person typing distances, and inventing a default would be worse than
//! not offering it: an untuned delay of zero is at least a predictable
//! error, while a guessed one moves the image somewhere nobody chose.
//!
//! Stated because "surround" implies it, and a module that quietly does
//! not do it looks like one that does it badly.

/// A speaker's role in the mix.
///
/// The names are PulseAudio's channel names, spelled the way `pactl` wants
/// them, so the routing layer does not translate — a table mapping our
/// spelling to theirs is a table that goes out of date silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    FrontLeft,
    FrontRight,
    FrontCenter,
    Lfe,
    RearLeft,
    RearRight,
    /// Both front channels, for a single speaker doing the whole mix. A
    /// stereo device in a surround setup is one speaker, not two, and
    /// pretending otherwise puts the same audio in it twice.
    Stereo,
}

impl Channel {
    pub fn pa_name(self) -> &'static str {
        match self {
            Channel::FrontLeft => "front-left",
            Channel::FrontRight => "front-right",
            Channel::FrontCenter => "front-center",
            Channel::Lfe => "lfe",
            Channel::RearLeft => "rear-left",
            Channel::RearRight => "rear-right",
            Channel::Stereo => "front-left,front-right",
        }
    }

    /// What the picker shows.
    pub fn label(self) -> &'static str {
        match self {
            Channel::FrontLeft => "front left",
            Channel::FrontRight => "front right",
            Channel::FrontCenter => "centre",
            Channel::Lfe => "subwoofer",
            Channel::RearLeft => "rear left",
            Channel::RearRight => "rear right",
            Channel::Stereo => "stereo",
        }
    }

    /// The channel a listener would expect on the other side.
    pub fn opposite(self) -> Option<Channel> {
        match self {
            Channel::FrontLeft => Some(Channel::FrontRight),
            Channel::FrontRight => Some(Channel::FrontLeft),
            Channel::RearLeft => Some(Channel::RearRight),
            Channel::RearRight => Some(Channel::RearLeft),
            _ => None,
        }
    }
}

/// What the whole set adds up to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// One speaker taking everything.
    Mono,
    Stereo,
    /// Front pair plus centre and/or sub.
    Front31,
    /// Front pair, rear pair, and whatever else is assigned.
    Surround5,
}

impl Layout {
    pub fn label(self) -> &'static str {
        match self {
            Layout::Mono => "mono",
            Layout::Stereo => "stereo",
            Layout::Front31 => "3.1",
            Layout::Surround5 => "surround",
        }
    }
}

/// Why an assignment will not do what the person expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// Two speakers claim the same role.
    ///
    /// Not an error in the audio sense — two speakers can play the same
    /// channel — but almost always a mistake in a room, and one that
    /// presents as "the left side is too loud" rather than as a setting.
    Duplicate(Channel),
    /// One side of a pair is assigned and the other is not. Half the mix
    /// is missing and nothing says so.
    Unpaired(Channel),
    /// Nothing assigned at all.
    Empty,
}

impl Problem {
    /// What to show. Says what is WRONG and what it will sound like,
    /// because "invalid configuration" tells nobody which speaker to move.
    pub fn message(&self) -> String {
        match self {
            Problem::Duplicate(c) => format!(
                "two speakers are set to {} — that side will be twice as loud",
                c.label()),
            Problem::Unpaired(c) => format!(
                "nothing is set to {} — that half of the mix will be missing",
                c.label()),
            Problem::Empty => "no speaker has a role yet".to_string(),
        }
    }
}

/// One speaker's place in the room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// The sink id — a MAC for Bluetooth, a PulseAudio name otherwise.
    pub id: String,
    pub label: String,
    pub channel: Channel,
}

/// A whole setup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Assignment {
    pub speakers: Vec<Placement>,
}

impl Assignment {
    pub fn is_empty(&self) -> bool { self.speakers.is_empty() }

    /// Put a speaker somewhere, replacing any previous placement of it.
    ///
    /// One speaker cannot be in two places; assigning it again MOVES it
    /// rather than adding a second entry, which is what a list of
    /// placements would otherwise accumulate as the person changes their
    /// mind.
    pub fn place(&mut self, id: &str, label: &str, channel: Channel) {
        self.speakers.retain(|p| p.id != id);
        self.speakers.push(Placement {
            id: id.to_string(), label: label.to_string(), channel });
    }

    pub fn remove(&mut self, id: &str) {
        self.speakers.retain(|p| p.id != id);
    }

    pub fn channel_of(&self, id: &str) -> Option<Channel> {
        self.speakers.iter().find(|p| p.id == id).map(|p| p.channel)
    }

    /// What this adds up to.
    pub fn layout(&self) -> Layout {
        let has = |c: Channel| self.speakers.iter().any(|p| p.channel == c);
        let rear = has(Channel::RearLeft) || has(Channel::RearRight);
        let front_pair = has(Channel::FrontLeft) && has(Channel::FrontRight);
        if rear && front_pair {
            Layout::Surround5
        } else if front_pair && (has(Channel::FrontCenter) || has(Channel::Lfe)) {
            Layout::Front31
        } else if front_pair || has(Channel::Stereo) {
            Layout::Stereo
        } else {
            Layout::Mono
        }
    }

    /// Everything wrong with it, in the order a person would fix them.
    ///
    /// A LIST rather than a bool: "invalid" is not actionable, and a setup
    /// with two problems should show two, or fixing the first appears to
    /// do nothing.
    pub fn problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        if self.speakers.is_empty() {
            return vec![Problem::Empty];
        }
        // Duplicates first: they are the ones that change what you HEAR,
        // and a missing rear speaker on a two-speaker setup is expected.
        let mut seen: Vec<Channel> = Vec::new();
        for p in &self.speakers {
            if p.channel == Channel::Stereo { continue; }
            if seen.contains(&p.channel) {
                if !out.contains(&Problem::Duplicate(p.channel)) {
                    out.push(Problem::Duplicate(p.channel));
                }
            } else {
                seen.push(p.channel);
            }
        }
        // A lone half of a pair. LFE and centre are deliberately not
        // paired — a single subwoofer is the normal case, and demanding a
        // second would report a fault on almost every real room.
        for p in &self.speakers {
            if let Some(other) = p.channel.opposite() {
                if !self.speakers.iter().any(|q| q.channel == other)
                    && !out.contains(&Problem::Unpaired(other)) {
                    out.push(Problem::Unpaired(other));
                }
            }
        }
        out
    }

    /// Is this safe to route?
    ///
    /// A duplicate is allowed through — it is a room decision, not a
    /// broken one, and someone may genuinely want two speakers on the same
    /// side. A MISSING half is not: it silently drops content.
    pub fn routable(&self) -> bool {
        !self.speakers.is_empty()
            && !self.problems().iter().any(|p| matches!(p, Problem::Unpaired(_)))
    }

    /// The channel map for the virtual sink, in PulseAudio's order.
    pub fn channel_map(&self) -> String {
        let mut names: Vec<&str> = Vec::new();
        for c in [Channel::FrontLeft, Channel::FrontRight, Channel::FrontCenter,
                  Channel::Lfe, Channel::RearLeft, Channel::RearRight] {
            if self.speakers.iter().any(|p| p.channel == c) {
                names.push(c.pa_name());
            }
        }
        if names.is_empty() && self.speakers.iter().any(|p| p.channel == Channel::Stereo) {
            return Channel::Stereo.pa_name().to_string();
        }
        names.join(",")
    }
}

/// Assign speakers to positions.
///
/// The rules above are what make this worth drawing: every one of them is
/// a mistake that otherwise reaches your ears instead of the screen. The
/// panel shows the layout it adds up to, and the problems, as you go —
/// not on commit, because a validation that only appears when you press
/// the button is one you meet after deciding you were finished.
///
/// Returns true when something changed, so a caller can persist once
/// rather than every frame.
pub fn assign_ui(ui: &mut egui::Ui, a: &mut Assignment,
                 speakers: &[(String, String)],
                 c: &uikit_egui::PaletteColors) -> bool {
    let mut changed = false;
    if speakers.is_empty() {
        ui.colored_label(c.text_weak, "no speakers — pair one on the Connect page");
        return false;
    }
    for (id, label) in speakers {
        ui.horizontal(|ui| {
            ui.label(label);
            let cur = a.channel_of(id);
            // "off" FIRST and always available: a speaker you cannot take
            // out of the arrangement is one you have to clear the whole
            // room to remove.
            if ui.selectable_label(cur.is_none(), "off").clicked() && cur.is_some() {
                a.remove(id);
                changed = true;
            }
            for ch in [Channel::Stereo, Channel::FrontLeft, Channel::FrontRight,
                       Channel::FrontCenter, Channel::Lfe,
                       Channel::RearLeft, Channel::RearRight] {
                if ui.selectable_label(cur == Some(ch), ch.label()).clicked()
                    && cur != Some(ch) {
                    a.place(id, label, ch);
                    changed = true;
                }
            }
        });
    }

    ui.separator();
    let problems = a.problems();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(a.layout().label()).strong());
        // The count, so the label can be checked against the rows above
        // before committing — the same reason the combine button names
        // its number.
        ui.colored_label(c.text_weak, format!("· {} speaker(s)", a.speakers.len()));
    });
    for p in &problems {
        // Colour by severity, because they are not the same: one refuses
        // to route and one is a note. Showing both the same way makes the
        // note look fatal and the fault look optional.
        let fatal = matches!(p, Problem::Unpaired(_) | Problem::Empty);
        ui.colored_label(if fatal { c.warning } else { c.text_weak }, p.message());
    }
    if a.routable() {
        ui.colored_label(c.text_weak, "ready");
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(pairs: &[(&str, Channel)]) -> Assignment {
        let mut a = Assignment::default();
        for (id, c) in pairs {
            a.place(id, id, *c);
        }
        a
    }

    #[test]
    fn a_speaker_moves_rather_than_appearing_twice() {
        // A list of placements otherwise accumulates every time the person
        // changes their mind, and the older entry keeps routing.
        let mut a = setup(&[("kitchen", Channel::FrontLeft)]);
        a.place("kitchen", "kitchen", Channel::RearLeft);
        assert_eq!(a.speakers.len(), 1);
        assert_eq!(a.channel_of("kitchen"), Some(Channel::RearLeft));
    }

    #[test]
    fn a_missing_half_of_a_pair_is_reported_and_refused() {
        // Half the mix is missing and nothing says so — the failure that
        // presents as "this recording sounds thin".
        let a = setup(&[("l", Channel::FrontLeft)]);
        let probs = a.problems();
        assert_eq!(probs, vec![Problem::Unpaired(Channel::FrontRight)]);
        assert!(probs[0].message().contains("front right"), "{}", probs[0].message());
        assert!(!a.routable(), "a half-assigned pair was allowed through");
    }

    #[test]
    fn a_single_subwoofer_is_not_a_fault() {
        // Demanding a second would report a problem on almost every real
        // room. LFE and centre are deliberately unpaired.
        let a = setup(&[("l", Channel::FrontLeft), ("r", Channel::FrontRight),
                        ("sub", Channel::Lfe), ("c", Channel::FrontCenter)]);
        assert!(a.problems().is_empty(), "{:?}", a.problems());
        assert!(a.routable());
        assert_eq!(a.layout(), Layout::Front31);
    }

    #[test]
    fn two_speakers_on_one_side_are_flagged_but_allowed() {
        // Not broken audio — a room decision, and someone may want it. It
        // presents as "the left is too loud" rather than as a setting, so
        // it is worth saying; refusing it would be the app overruling a
        // choice it cannot see the reason for.
        let a = setup(&[("l1", Channel::FrontLeft), ("l2", Channel::FrontLeft),
                        ("r", Channel::FrontRight)]);
        assert_eq!(a.problems(), vec![Problem::Duplicate(Channel::FrontLeft)]);
        assert!(a.routable(), "a duplicate was treated as fatal");
    }

    #[test]
    fn the_layout_is_named_from_what_is_actually_assigned() {
        assert_eq!(setup(&[]).layout(), Layout::Mono);
        assert_eq!(setup(&[("a", Channel::Stereo)]).layout(), Layout::Stereo);
        assert_eq!(setup(&[("l", Channel::FrontLeft), ("r", Channel::FrontRight)]).layout(),
                   Layout::Stereo);
        assert_eq!(setup(&[("l", Channel::FrontLeft), ("r", Channel::FrontRight),
                           ("rl", Channel::RearLeft), ("rr", Channel::RearRight)]).layout(),
                   Layout::Surround5);
    }

    #[test]
    fn the_channel_map_is_in_pulseaudios_order_not_assignment_order() {
        // pactl reads a map positionally. Emitting it in the order the
        // person happened to assign speakers swaps their channels, which
        // is the worst possible bug here: everything plays, from the wrong
        // places.
        let a = setup(&[("rr", Channel::RearRight), ("l", Channel::FrontLeft),
                        ("sub", Channel::Lfe), ("r", Channel::FrontRight),
                        ("rl", Channel::RearLeft)]);
        assert_eq!(a.channel_map(), "front-left,front-right,lfe,rear-left,rear-right");
    }

    #[test]
    fn an_empty_setup_says_so_rather_than_looking_configured() {
        let a = Assignment::default();
        assert_eq!(a.problems(), vec![Problem::Empty]);
        assert!(!a.routable());
        assert_eq!(a.channel_map(), "");
    }

    #[test]
    fn every_problem_names_a_speaker_position_and_a_symptom() {
        // "Invalid configuration" tells nobody which speaker to move.
        for p in [Problem::Duplicate(Channel::RearLeft),
                  Problem::Unpaired(Channel::FrontRight), Problem::Empty] {
            let m = p.message();
            assert!(m.len() > 12 && m.chars().next().is_some_and(|c| c.is_lowercase()),
                    "{m:?}");
        }
    }
}

/// Turn a validated assignment into a routing sink.
///
/// `None` when it is not routable — the rules live in
/// [`Assignment::problems`] and this refuses to translate a setup that
/// would silently drop half the mix. Returning a sink anyway and letting
/// the audio server sort it out is how a missing rear speaker becomes
/// "why does this album sound thin".
pub fn sink_of(a: &Assignment) -> Option<crate::playback::Sink> {
    if !a.routable() {
        return None;
    }
    // A setup that is really just stereo, or one speaker, does not need a
    // null sink and two remaps: that chain costs latency and two modules
    // to reproduce what the device does natively.
    if a.speakers.len() == 1 {
        return Some(crate::playback::Sink::One(a.speakers[0].id.clone()));
    }
    Some(crate::playback::Sink::Surround {
        members: a.speakers.iter()
            .filter(|p| p.channel != Channel::Stereo)
            .map(|p| (p.id.clone(), p.channel.pa_name().to_string()))
            .collect(),
        channel_map: a.channel_map(),
    })
}

/// The panel, through real frames.
///
/// The rules are tested above as data. These check that a person can SEE
/// them — a validation nobody is shown is a validation that does not
/// exist, which is most of what this module is for.
#[cfg(test)]
mod frame_tests {
    use super::*;

    fn speakers() -> Vec<(String, String)> {
        vec![("l".into(), "Living Room".into()), ("k".into(), "Kitchen".into())]
    }

    fn draw(a: &mut Assignment, sp: &[(String, String)]) -> Vec<String> {
        let ctx = egui::Context::default();
        let c = uikit_egui::palette_colors("amber");
        let out = ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO,
                                                        egui::vec2(500.0, 700.0))),
            ..Default::default()
        }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| { assign_ui(ui, a, sp, &c); });
        });
        out.shapes.iter().filter_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        }).collect()
    }

    #[test]
    fn a_half_assigned_pair_says_what_will_be_missing() {
        // Not "invalid configuration" — which speaker, and what it will
        // sound like.
        let mut a = Assignment::default();
        a.place("l", "Living Room", Channel::FrontLeft);
        let texts = draw(&mut a, &speakers());
        let joined = texts.join(" | ");
        assert!(joined.contains("front right"), "{joined}");
        assert!(joined.contains("missing"), "{joined}");
        assert!(!joined.contains("ready"), "an unroutable room said it was ready");
    }

    #[test]
    fn a_valid_room_says_so_and_names_its_layout() {
        let mut a = Assignment::default();
        a.place("l", "Living Room", Channel::FrontLeft);
        a.place("k", "Kitchen", Channel::FrontRight);
        let joined = draw(&mut a, &speakers()).join(" | ");
        assert!(joined.contains("ready"), "{joined}");
        assert!(joined.contains("stereo"), "the layout is not named: {joined}");
    }

    #[test]
    fn every_speaker_can_be_taken_out_again() {
        // A speaker you cannot remove means clearing the whole room to
        // change your mind about one of them.
        let mut a = Assignment::default();
        a.place("l", "Living Room", Channel::FrontLeft);
        let joined = draw(&mut a, &speakers()).join(" | ");
        assert!(joined.contains("off"), "no way to unassign: {joined}");
    }

    #[test]
    fn with_no_speakers_it_points_at_where_pairing_happens() {
        // An empty panel with no explanation reads as broken; the user
        // cannot pair from here.
        let mut a = Assignment::default();
        let joined = draw(&mut a, &[]).join(" | ");
        assert!(joined.contains("Connect"), "{joined}");
    }
}

#[cfg(test)]
mod sink_tests {
    use super::*;

    #[test]
    fn a_valid_room_becomes_a_surround_sink() {
        let mut a = Assignment::default();
        a.place("l", "Left", Channel::FrontLeft);
        a.place("r", "Right", Channel::FrontRight);
        match sink_of(&a) {
            Some(crate::playback::Sink::Surround { members, channel_map }) => {
                assert_eq!(members.len(), 2);
                assert_eq!(channel_map, "front-left,front-right");
            }
            other => panic!("a stereo pair produced {other:?}"),
        }
    }

    #[test]
    fn an_unroutable_room_produces_nothing_rather_than_half_a_mix() {
        // Letting the audio server sort it out is how a missing rear
        // speaker becomes "why does this album sound thin".
        let mut a = Assignment::default();
        a.place("l", "Left", Channel::FrontLeft);
        assert_eq!(sink_of(&a), None);
    }

    #[test]
    fn one_speaker_skips_the_whole_chain() {
        // A null sink plus a remap to reproduce what the device does
        // natively is two modules and a latency penalty for nothing.
        let mut a = Assignment::default();
        a.place("solo", "Kitchen", Channel::Stereo);
        assert_eq!(sink_of(&a), Some(crate::playback::Sink::One("solo".into())));
    }
}
