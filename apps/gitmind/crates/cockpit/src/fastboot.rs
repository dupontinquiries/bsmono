//! fastboot — the protocol, without the subprocess.
//!
//! Three existing implementations of this workflow agree on its shape, which
//! is the strongest evidence available that the shape is right:
//!
//! * pmbootstrap's `pmb/config/__init__.py` holds `flashers` as
//!   method → action → argv templates and shells out to the `fastboot` binary
//!   from inside a chroot, as root.
//! * postmarketOS's `webflash` carries a device table of
//!   `{cmd | flash, partition, name}` steps and speaks the wire protocol
//!   directly over WebUSB.
//! * `pm_flash_rusty` carries the same `{cmd | flash, partition, name}` script
//!   in JSON and shells out to `fastboot`.
//!
//! So the script model here is deliberately the one already in
//! `spec-fp5-boot.json` — same field names, same semantics — because a fourth
//! incompatible spelling of a settled idea helps nobody.
//!
//! What is different is the transport. Shelling out to `fastboot` inherits
//! whatever that binary does about device selection, permissions and error
//! reporting, and hands back an exit code. Speaking the protocol means the
//! failure is a value: "claim failed, permission denied" instead of "exit 1".
//! Given how much time the udev question has already cost, that distinction is
//! the point of doing this at all.
//!
//! ## The protocol is small
//!
//! Commands are ASCII, at most 64 bytes, written to the bulk OUT endpoint.
//! Every reply is a 4-byte tag and up to 60 bytes of payload:
//!
//! * `OKAY` — done, payload is the result.
//! * `FAIL` — refused, payload is why.
//! * `DATA` — ready for `payload` bytes, sent as hex. This is the handshake
//!   before an upload.
//! * `INFO` — progress chatter. **Not** terminal: the device may send many,
//!   and the next read continues the same exchange. Treating INFO as an answer
//!   is the classic way to desynchronise a fastboot session.

use std::fmt;

/// The maximum a fastboot command may be. Longer is a protocol error, not a
/// truncation — a silently cut command runs something the caller did not ask
/// for.
pub const MAX_COMMAND: usize = 64;

/// One reply from the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Okay(String),
    Fail(String),
    /// The device will accept this many bytes next.
    Data(u32),
    /// Progress text. The exchange is not over.
    Info(String),
}

/// Why a reply could not be understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// Fewer than the four tag bytes.
    Short(usize),
    /// A tag that is not one of the four.
    UnknownTag(String),
    /// `DATA` whose length was not valid hex.
    BadDataLength(String),
    /// A command that will not fit in 64 bytes.
    CommandTooLong(usize),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::Short(n) => write!(f, "reply was {n} bytes, need at least 4"),
            ProtocolError::UnknownTag(t) => write!(f, "unknown reply tag {t:?}"),
            ProtocolError::BadDataLength(s) => write!(f, "DATA length {s:?} is not hex"),
            ProtocolError::CommandTooLong(n) => {
                write!(f, "command is {n} bytes, limit is {MAX_COMMAND}")
            }
        }
    }
}

/// Parse one reply off the wire.
///
/// Lenient about trailing NULs and whitespace, which real devices send, and
/// strict about the tag, which they do not vary.
pub fn parse_reply(bytes: &[u8]) -> Result<Reply, ProtocolError> {
    if bytes.len() < 4 {
        return Err(ProtocolError::Short(bytes.len()));
    }
    let tag = String::from_utf8_lossy(&bytes[..4]).to_string();
    let rest = String::from_utf8_lossy(&bytes[4..])
        .trim_end_matches('\0')
        .trim()
        .to_string();
    match tag.as_str() {
        "OKAY" => Ok(Reply::Okay(rest)),
        "FAIL" => Ok(Reply::Fail(rest)),
        "INFO" => Ok(Reply::Info(rest)),
        "DATA" => u32::from_str_radix(rest.trim(), 16)
            .map(Reply::Data)
            .map_err(|_| ProtocolError::BadDataLength(rest)),
        _ => Err(ProtocolError::UnknownTag(tag)),
    }
}

/// Encode a command for the OUT endpoint.
pub fn encode_command(cmd: &str) -> Result<Vec<u8>, ProtocolError> {
    let b = cmd.as_bytes();
    if b.len() > MAX_COMMAND {
        return Err(ProtocolError::CommandTooLong(b.len()));
    }
    Ok(b.to_vec())
}

/// The `download:` command announcing an upload of `size` bytes.
///
/// Lower-case, zero-padded, exactly eight hex digits — the device parses the
/// field by position, so width matters more than it looks.
pub fn download_command(size: u32) -> String {
    format!("download:{size:08x}")
}

/// Split a payload into transfer-sized chunks.
///
/// 16 KiB matches what webflash uses and what the CLI tools settle on. The
/// last chunk is short rather than padded: padding an image changes it.
pub fn chunks(len: usize, chunk: usize) -> Vec<(usize, usize)> {
    if len == 0 || chunk == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut at = 0;
    while at < len {
        let end = (at + chunk).min(len);
        out.push((at, end));
        at = end;
    }
    out
}

/// The default transfer chunk.
pub const CHUNK: usize = 16 * 1024;

// ── the flash script ─────────────────────────────────────────────────────────

/// One step, in the shape `spec-fp5-boot.json` already uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptStep {
    /// A bare command: `erase:vendor_boot`, `reboot`.
    Command { cmd: String, name: String },
    /// Upload an image and write it to a partition. `suffix` selects the file
    /// from the cache by its ending, which is how all three implementations
    /// name images they did not choose the full name of.
    Flash { suffix: String, partition: String, name: String },
}

impl ScriptStep {
    /// What the operator sees.
    pub fn label(&self) -> &str {
        match self {
            ScriptStep::Command { name, .. } | ScriptStep::Flash { name, .. } => name,
        }
    }

    /// Does this step write to the device? Used to gate a confirmation: a
    /// script that only reads is safe to run without asking, and one that
    /// erases a partition is not.
    pub fn is_destructive(&self) -> bool {
        match self {
            ScriptStep::Flash { .. } => true,
            ScriptStep::Command { cmd, .. } => {
                cmd.starts_with("erase:") || cmd.starts_with("format:")
            }
        }
    }
}

/// Why an image was refused before anything was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// The spec carried a digest and the file did not match it.
    DigestMismatch { expected: String, actual: String },
    /// The spec carried no digest at all.
    ///
    /// Its own field, and not merged into "ok", because
    /// `pm_flash_rusty`'s spec has `sha256` and `sha512` fields that nothing
    /// currently checks — an unverified download gets flashed to a boot
    /// partition. Unverifiable and verified-good must be distinguishable or
    /// the gap stays invisible.
    NoDigest,
}

/// Compare a computed digest against the spec's.
///
/// Case-insensitive, because specs and tools disagree about hex case, and
/// whitespace-trimmed, because digests pasted from a checksum file carry it.
pub fn verify_digest(expected: Option<&str>, actual: &str) -> Result<(), VerifyError> {
    match expected {
        None => Err(VerifyError::NoDigest),
        Some(e) => {
            let e = e.trim().to_ascii_lowercase();
            let a = actual.trim().to_ascii_lowercase();
            if e.is_empty() {
                return Err(VerifyError::NoDigest);
            }
            if e == a {
                Ok(())
            } else {
                Err(VerifyError::DigestMismatch { expected: e, actual: a })
            }
        }
    }
}

/// Pick the newest image whose name ends with `suffix`.
///
/// Timestamps are compared as strings, which is correct for the
/// `YYYYMMDD-HHMM` form postmarketOS uses and wrong for anything else — so a
/// non-conforming timestamp sorts predictably rather than panicking, and the
/// caller sees an image it can check rather than an error it cannot act on.
pub fn newest_matching<'a, T, F, G>(items: &'a [T], suffix: &str, name: F, stamp: G) -> Option<&'a T>
where
    F: Fn(&T) -> &str,
    G: Fn(&T) -> &str,
{
    items
        .iter()
        .filter(|i| name(i).ends_with(suffix))
        .max_by(|a, b| stamp(a).cmp(stamp(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_tags_parse_and_info_is_not_an_answer() {
        assert_eq!(parse_reply(b"OKAY0.4"), Ok(Reply::Okay("0.4".into())));
        assert_eq!(parse_reply(b"FAILunknown command"),
                   Ok(Reply::Fail("unknown command".into())));
        // INFO is progress chatter and the exchange continues. A reader that
        // returns on INFO desynchronises the session — the next command then
        // reads the PREVIOUS command's real answer.
        assert_eq!(parse_reply(b"INFOerasing"), Ok(Reply::Info("erasing".into())));
        assert!(matches!(parse_reply(b"INFOerasing"), Ok(Reply::Info(_))));
    }

    #[test]
    fn data_carries_a_hex_length_and_a_bad_one_is_an_error_not_a_zero() {
        assert_eq!(parse_reply(b"DATA00001000"), Ok(Reply::Data(0x1000)));
        // Defaulting a malformed length to 0 would send nothing and then the
        // device would wait forever for bytes that never come.
        assert_eq!(parse_reply(b"DATAzzzz"),
                   Err(ProtocolError::BadDataLength("zzzz".into())));
    }

    #[test]
    fn real_device_padding_is_tolerated_but_a_strange_tag_is_not() {
        // Devices pad short replies with NULs and sometimes a newline.
        assert_eq!(parse_reply(b"OKAYFP5\0\0\0"), Ok(Reply::Okay("FP5".into())));
        assert_eq!(parse_reply(b"OKAYFP5 \n"), Ok(Reply::Okay("FP5".into())));
        // The tag itself never varies, so an unknown one means the stream is
        // out of sync and guessing would write to the wrong partition.
        assert_eq!(parse_reply(b"WHAT.."), Err(ProtocolError::UnknownTag("WHAT".into())));
        assert_eq!(parse_reply(b"OK"), Err(ProtocolError::Short(2)));
    }

    #[test]
    fn download_is_eight_lower_case_hex_digits() {
        // The device parses this field by position: width and case are load
        // bearing, not cosmetic.
        assert_eq!(download_command(0x1000), "download:00001000");
        assert_eq!(download_command(1), "download:00000001");
        assert_eq!(download_command(0xdeadbeef), "download:deadbeef");
        assert_eq!(download_command(0).len(), "download:".len() + 8);
    }

    #[test]
    fn an_over_long_command_is_refused_rather_than_truncated() {
        let ok = "getvar:product";
        assert_eq!(encode_command(ok).unwrap(), ok.as_bytes());
        // A silently cut command runs something the caller did not ask for —
        // "flash:boot_a" truncated is still a valid command.
        let long = "x".repeat(MAX_COMMAND + 1);
        assert_eq!(encode_command(&long), Err(ProtocolError::CommandTooLong(65)));
        assert!(encode_command(&"x".repeat(MAX_COMMAND)).is_ok());
    }

    #[test]
    fn the_last_chunk_is_short_and_never_padded() {
        assert_eq!(chunks(0, CHUNK), Vec::new());
        assert_eq!(chunks(100, 40), vec![(0, 40), (40, 80), (80, 100)]);
        // Exactly divisible: no empty trailing chunk.
        assert_eq!(chunks(80, 40), vec![(0, 40), (40, 80)]);
        // Padding the tail would change the image being written.
        let c = chunks(CHUNK + 1, CHUNK);
        assert_eq!(c.last(), Some(&(CHUNK, CHUNK + 1)));
    }

    #[test]
    fn a_missing_digest_is_distinct_from_a_matching_one() {
        assert!(verify_digest(Some("ABC123"), "abc123").is_ok(), "hex case varies by tool");
        assert!(verify_digest(Some(" abc123 "), "abc123").is_ok());
        assert_eq!(verify_digest(Some("abc"), "def"),
                   Err(VerifyError::DigestMismatch {
                       expected: "abc".into(), actual: "def".into() }));
        // The gap this exists to surface: a spec with sha256/sha512 fields
        // that nothing checks means an unverified download reaches a boot
        // partition. "No digest" must not read as "fine".
        assert_eq!(verify_digest(None, "abc"), Err(VerifyError::NoDigest));
        assert_eq!(verify_digest(Some(""), "abc"), Err(VerifyError::NoDigest));
    }

    #[test]
    fn destructive_steps_are_identified_so_a_confirmation_can_gate_them() {
        let flash = ScriptStep::Flash {
            suffix: "-boot.img.xz".into(), partition: "boot".into(),
            name: "Flash boot partition".into() };
        let erase = ScriptStep::Command {
            cmd: "erase:vendor_boot".into(), name: "Erase vendor_boot".into() };
        let reboot = ScriptStep::Command { cmd: "reboot".into(), name: "Reboot".into() };
        let getvar = ScriptStep::Command {
            cmd: "getvar:product".into(), name: "Read product".into() };
        assert!(flash.is_destructive());
        assert!(erase.is_destructive());
        assert!(!reboot.is_destructive(), "a reboot loses nothing");
        assert!(!getvar.is_destructive());
        assert_eq!(flash.label(), "Flash boot partition");
    }

    #[test]
    fn the_newest_image_wins_and_the_suffix_selects_the_kind() {
        // The real shape: a cache holding both a rootfs and a boot image
        // across several builds, named the way postmarketOS names them.
        let imgs = vec![
            ("20250808-1416-postmarketOS-edge-phosh-fp5-boot.img.xz", "20250808-1416"),
            ("20250810-0900-postmarketOS-edge-phosh-fp5-boot.img.xz", "20250810-0900"),
            ("20250810-0900-postmarketOS-edge-phosh-fp5.img.xz", "20250810-0900"),
        ];
        let pick = newest_matching(&imgs, "-boot.img.xz", |i| i.0, |i| i.1).unwrap();
        assert_eq!(pick.1, "20250810-0900");
        assert!(pick.0.ends_with("-boot.img.xz"));
        // The rootfs suffix must not match the boot image, or a rootfs lands
        // on the boot partition.
        let root = newest_matching(&imgs, "phosh-fp5.img.xz", |i| i.0, |i| i.1).unwrap();
        assert!(!root.0.contains("-boot."));
        assert!(newest_matching(&imgs, "-nothing", |i| i.0, |i| i.1).is_none());
    }
}
