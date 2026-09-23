//! codec_catalog — every codec version, by codename, with what it can do HERE.
//!
//! ## Why codenames
//!
//! "v8" meant three serialisation variants, "v2" meant "v1 underneath", and the
//! marker table in `docs/CODEC_FORMATS.md` described two markers the code does
//! not produce (see `DOC_DRIFT`). A version number is not a name you can hold a
//! conversation about. These codenames are ORIGINAL -- evocative of mythic
//! pantheons, evolving creatures and block-built worlds, but coined words or
//! plain English, not names from any game.
//!
//! ## Why the capabilities are the point
//!
//! Following bscale's model picker: a catalogue where each entry carries a
//! status, and the picker renders every entry but lets you choose only the ones
//! that can do what you asked. bscale HIDES unusable entries; this SHOWS them,
//! greyed out with the reason, because the ask was to expose as many options as
//! possible -- and "this exists but cannot encode yet" is itself worth seeing.
//!
//! Every capability below was checked against the code, not the doc:
//!
//!   * ENCODE: only Keystone writes real bytes from this frontend
//!     (`export::write_dct_fao`). Driftwing's temporal frames are never
//!     serialised -- `seq_lab::T_BITS_ARE_ESTIMATED` -- so it cannot be offered
//!     as an encoder even though it runs.
//!   * DECODE: exactly the arms in `decode::FaoFile::decode_rgb`.

/// Where a codec is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Produces files people use today.
    Current,
    /// Measured and real, but not the default path.
    Research,
    /// Structure present, the real work not landed.
    Scaffold,
    /// Superseded; kept so old files still identify.
    Retired,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Current => "current",
            Status::Research => "research",
            Status::Scaffold => "scaffold",
            Status::Retired => "retired",
        }
    }
}

/// Can this frontend decode it?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decode {
    Yes,
    /// Decodes, but only when the file carries a companion block.
    NeedsCompanion(&'static str),
    /// Recognised, not rendered -- with the reason.
    No(&'static str),
}

/// One codec version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Codec {
    pub codename: &'static str,
    pub version: &'static str,
    /// The frame marker byte, if this version writes frames into the container.
    pub marker: Option<u8>,
    pub what: &'static str,
    pub status: Status,
    /// Can this frontend WRITE real bytes with it?
    pub encode: Option<&'static str>,
    pub decode: Decode,
}

/// What the user is choosing a codec FOR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Encode,
    Decode,
}

impl Codec {
    /// Why this codec cannot serve `purpose`, or None if it can.
    pub fn unavailable(&self, purpose: Purpose) -> Option<&'static str> {
        match purpose {
            Purpose::Encode => match self.encode {
                Some(_) => None,
                None => Some(self.no_encode_reason()),
            },
            Purpose::Decode => match self.decode {
                Decode::Yes | Decode::NeedsCompanion(_) => None,
                Decode::No(why) => Some(why),
            },
        }
    }

    fn no_encode_reason(&self) -> &'static str {
        match self.codename {
            "Driftwing" => "temporal frames are never serialised -- sizes are estimated",
            _ => match self.status {
                Status::Retired => "retired",
                Status::Scaffold => "scaffold -- no encoder yet",
                _ => "no encoder in this frontend yet",
            },
        }
    }

    /// One line for a picker row: codename, version, marker, status.
    pub fn row(&self) -> String {
        let m = self.marker.map(|b| format!(" '{}'", b as char)).unwrap_or_default();
        format!("{} · {}{} · {}", self.codename, self.version, m, self.status.label())
    }
}

/// Every version, oldest first.
pub const CATALOG: &[Codec] = &[
    Codec { codename: "Keystone", version: "v1 DCT", marker: Some(b'D'),
        what: "DCT keyframe, 4:2:0 -- the foundation everything else stands on",
        status: Status::Current, encode: Some("export::write_dct_fao"), decode: Decode::Yes },
    Codec { codename: "Splitstone", version: "v1 plane-split", marker: Some(b'P'),
        what: "luma and chroma planes coded separately",
        status: Status::Current, encode: None, decode: Decode::Yes },
    Codec { codename: "Dimglass", version: "v1 lossy-chroma", marker: Some(b'L'),
        what: "full luma, heavily quantised chroma",
        status: Status::Current, encode: None, decode: Decode::Yes },
    Codec { codename: "Runehoard", version: "block dictionary", marker: Some(b'K'),
        what: "frames rebuilt from a hoard of reusable blocks",
        status: Status::Research, encode: None,
        decode: Decode::NeedsCompanion("FAOBKD01 block dictionary") },
    Codec { codename: "Driftwing", version: "v2.1 translate", marker: Some(b'T'),
        what: "translation residual -- follows motion from the frame before",
        status: Status::Current, encode: None, decode: Decode::Yes },
    Codec { codename: "Moltshell", version: "v3-v5", marker: Some(b'V'),
        what: "intermediate attempts, shed as the codec evolved",
        status: Status::Retired, encode: None,
        decode: Decode::No("fao_app renders nothing for these either") },
    Codec { codename: "Worldloom", version: "v8 reconstruction", marker: Some(b'Z'),
        what: "weaves a 3D mesh from camera pose and features",
        status: Status::Scaffold, encode: None,
        decode: Decode::NeedsCompanion("V8MX mesh trailer") },
    Codec { codename: "Glowlattice", version: "v9 volumetric", marker: None,
        what: "sparse voxel grid in place of v8's triangle mesh",
        status: Status::Research, encode: None,
        decode: Decode::No("no container marker -- research module only") },
    Codec { codename: "Conclave", version: "v10 orchestrator", marker: Some(b'X'),
        what: "a council of prediction strategies, one chosen per frame",
        status: Status::Current, encode: None, decode: Decode::Yes },
    Codec { codename: "Tectonis", version: "v11 plate", marker: None,
        what: "the web thesis in its strongest 2D form",
        status: Status::Research, encode: None,
        decode: Decode::No("no container marker -- research module only") },
    // The one entry authored in THIS frontend rather than ported: the reference
    // app and CLI do not know marker M and will show these frames as gaps.
    Codec { codename: "Quiltstone", version: "egui block patch", marker: Some(b'M'),
        what: "only the 16x16 blocks that changed, over the previous frame -- Keystone DCT underneath",
        status: Status::Current, encode: Some("transcode (patch frames between keyframes)"), decode: Decode::Yes },
    Codec { codename: "Shiftquilt", version: "egui motion patch", marker: Some(b'Q'),
        what: "Quiltstone with motion: changed blocks predicted from the previous frame where that is cheaper",
        status: Status::Current, encode: Some("transcode (compact mode)"), decode: Decode::Yes },
];

/// The codec that wrote a frame, from its marker byte.
pub fn by_marker(marker: u8) -> Option<&'static Codec> {
    CATALOG.iter().find(|c| c.marker == Some(marker))
}

/// Markers `docs/CODEC_FORMATS.md` describes as something the code does not
/// write. Found while building this catalogue: `P` is documented as a v7
/// Perception frame but is only ever written as plane-split (lib.rs:3829,
/// :7323); `K` is documented as a v2.1 keyframe but is only written by
/// block_dict.rs:196 as a dictionary frame. The catalogue follows the CODE.
pub const DOC_DRIFT: &[(u8, &str, &str)] = &[
    (b'P', "v7 Perception frame", "v1 plane-split"),
    (b'K', "v2.1 keyframe", "block-dictionary frame"),
];

/// A picker over the whole catalogue, bscale-style: every codec shown, only the
/// ones that serve `purpose` selectable, the rest greyed with the reason.
pub fn picker(ui: &mut egui::Ui, id: &str, purpose: Purpose, selected: &mut &'static str) {
    let current = CATALOG.iter().find(|c| c.codename == *selected);
    let text = current.map(|c| c.row()).unwrap_or_else(|| "(choose a codec)".into());
    egui::ComboBox::from_id_salt(id).selected_text(text).width(320.0).show_ui(ui, |ui| {
        for c in CATALOG {
            match c.unavailable(purpose) {
                None => {
                    ui.selectable_value(selected, c.codename, c.row()).on_hover_text(c.what);
                }
                Some(why) => {
                    ui.add_enabled(false, egui::SelectableLabel::new(false, c.row()))
                        .on_disabled_hover_text(format!("{}\n\nUnavailable: {why}", c.what));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_least_eight_versions_with_unique_codenames() {
        assert!(CATALOG.len() >= 8, "{}", CATALOG.len());
        let mut names: Vec<_> = CATALOG.iter().map(|c| c.codename).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), CATALOG.len(), "a codename is used twice");
    }

    #[test]
    fn no_two_codecs_claim_the_same_marker() {
        let mut m: Vec<u8> = CATALOG.iter().filter_map(|c| c.marker).collect();
        let n = m.len();
        m.sort();
        m.dedup();
        assert_eq!(m.len(), n, "two codecs share a marker -- the drift this exists to stop");
    }

    /// EVERY MARKER THE DECODER HANDLES HAS A CODENAME. The list is the arms of
    /// `decode::FaoFile::decode_rgb`; a new arm with no entry here is a frame the
    /// UI would have to show as a bare byte, which is the confusion this ends.
    /// ('R' is a repeat marker, not a codec, and resolves to the frame it repeats.)
    #[test]
    fn every_decoded_marker_has_a_codename() {
        for m in [b'D', b'P', b'L', b'K', b'T', b'V', b'Z', b'X', b'M', b'Q'] {
            assert!(by_marker(m).is_some(), "marker '{}' has no codename", m as char);
        }
    }

    /// Only codecs that really write bytes are offered as encoders. Driftwing
    /// RUNS, but writes nothing -- offering it would promise a file it cannot
    /// make.
    #[test]
    fn only_real_encoders_are_selectable_for_encoding() {
        let enc: Vec<_> = CATALOG.iter().filter(|c| c.unavailable(Purpose::Encode).is_none())
            .map(|c| c.codename).collect();
        // Quiltstone joined when the transcoder began writing patch frames --
        // a REAL encoder: its output is decoded and checked in transcode's tests.
        assert_eq!(enc, vec!["Keystone", "Quiltstone", "Shiftquilt"]);
        let drift = CATALOG.iter().find(|c| c.codename == "Driftwing").unwrap();
        assert!(drift.unavailable(Purpose::Encode).unwrap().contains("estimated"));
    }

    /// The catalogue's decode claim for Keystone is TRUE: a file it writes, the
    /// decoder reads.
    #[test]
    fn keystones_decode_claim_holds() {
        let (w, h) = (16usize, 16usize);
        let bytes = crate::export::write_dct_fao(&[vec![128u8; w * h * 3]], w, h, 30.0, 80).unwrap();
        let f = crate::decode::FaoFile::parse(bytes).unwrap();
        assert_eq!(f.markers(), vec![Some(b'D')]);
        assert_eq!(by_marker(b'D').unwrap().codename, "Keystone");
        assert!(f.decode_rgb(0).is_ok());
    }

    #[test]
    fn every_codec_is_visible_in_the_picker_even_when_unavailable() {
        // Exposed, not hidden: every entry renders a row, available or not.
        for c in CATALOG {
            assert!(!c.row().is_empty());
            if let Some(why) = c.unavailable(Purpose::Encode) {
                assert!(!why.is_empty(), "{} greyed with no reason", c.codename);
            }
        }
    }
}
