//! Live-tile notices: short messages other apps would hand to hatch (a snapclone call, a new photo,
//! a game invite, a voiceterm note). Nothing publishes them yet, so [`demo`] stands in with fixed
//! dummy messages -- shown so the strip can be designed against real-looking content, and labelled
//! as demo data wherever they are drawn.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A snapclone call.
    Call,
    /// A photo notification (snapclone photos).
    Photo,
    /// A game invite (clawbound lobby).
    GameInvite,
    /// A note from voiceterm.
    Note,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Call, Kind::Photo, Kind::GameInvite, Kind::Note];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Call => "snapclone",
            Kind::Photo => "photos",
            Kind::GameInvite => "clawbound",
            Kind::Note => "voiceterm",
        }
    }

    /// A glyph in the symbol fonts uikit-egui installs.
    pub fn glyph(self) -> &'static str {
        match self {
            Kind::Call => "✆",
            Kind::Photo => "▣",
            Kind::GameInvite => "⚔",
            Kind::Note => "✎",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub id: u32,
    pub kind: Kind,
    pub title: String,
    pub body: String,
    /// "2 min", "now": already formatted, because the demo has no clock.
    pub when: String,
}

/// Dummy messages, one or two per kind, newest first.
pub fn demo() -> Vec<Notice> {
    let n = |id, kind, title: &str, body: &str, when: &str| Notice { id, kind, title: title.into(), body: body.into(), when: when.into() };
    vec![
        n(1, Kind::Call, "Incoming call", "Mira is calling over snapclone", "now"),
        n(2, Kind::GameInvite, "Game invite", "Dunes, 4 players: join Tomas's lobby", "2 min"),
        n(3, Kind::Photo, "3 new photos", "Garden walk, shared by Mira", "14 min"),
        n(4, Kind::Note, "Note", "\"buy oat milk, call the landlord about the heater\"", "1 h"),
        n(5, Kind::Call, "Missed call", "Tomas, 07:42", "3 h"),
        n(6, Kind::Note, "Note", "\"idea: the drawer tiles could show their app's battery cost\"", "yesterday"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_covers_every_kind_with_unique_ids() {
        let d = demo();
        for k in Kind::ALL {
            assert!(d.iter().any(|n| n.kind == k), "{k:?} has a message");
        }
        let mut ids: Vec<u32> = d.iter().map(|n| n.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), d.len(), "ids are unique");
        assert!(d.iter().all(|n| !n.title.is_empty() && !n.body.is_empty()));
    }
}
