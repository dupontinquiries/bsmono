//! session — driving the protocol, with the wire behind a trait.
//!
//! [`fastboot`](crate::fastboot) parses and encodes; this decides what to send
//! next and what a reply means. Keeping the two apart is what lets the whole
//! exchange be tested without a phone: a fake [`Transport`] returns scripted
//! replies and records what was written, so the sequencing rules below are
//! pinned by tests rather than by having run it once on a device that
//! happened to work.
//!
//! The real transport — `nusb` bulk endpoints — implements four methods and
//! contains no logic. That is deliberate: the part that needs hardware should
//! be the part with nothing in it to get wrong.
//!
//! ## The upload handshake is the only complicated bit
//!
//! Writing an image is three exchanges, not one:
//!
//! ```text
//!   → download:00a1b2c3        how many bytes are coming
//!   ← DATA00a1b2c3             the device is ready for exactly that many
//!   → <raw bytes, chunked>
//!   ← OKAY                     it took them
//!   → flash:boot_b             now write what you just took
//!   ← OKAY
//! ```
//!
//! The device may answer `DATA` with a **smaller** number than asked for — its
//! buffer is what it is. Sending the full image anyway overruns it, and the
//! failure shows up as a corrupt flash rather than an error. So a short DATA
//! is refused here rather than truncated silently: an image that does not fit
//! needs sparse upload, which is a different feature, not a smaller write.

use crate::fastboot::{
    chunks, download_command, encode_command, parse_reply, ProtocolError, Reply, CHUNK,
};
use crate::slots::{partition_for, Slot, SlotInfo};

/// The wire. Implemented over USB bulk endpoints in the app, faked in tests.
pub trait Transport {
    /// Write a command or a data chunk to the OUT endpoint.
    fn write(&mut self, bytes: &[u8]) -> Result<(), String>;
    /// Read one reply packet from the IN endpoint.
    fn read(&mut self) -> Result<Vec<u8>, String>;
}

/// Anything that can go wrong in an exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// The transport failed.
    Io(String),
    /// The reply could not be parsed.
    Protocol(ProtocolError),
    /// The device refused, with its own words.
    Refused(String),
    /// Expected `DATA` and got something else.
    NoDataHandshake(String),
    /// The device offered a smaller buffer than the image needs.
    BufferTooSmall { needed: usize, offered: u32 },
    /// The device sent `INFO` forever.
    TooManyInfo,
}

impl From<ProtocolError> for SessionError {
    fn from(e: ProtocolError) -> Self {
        SessionError::Protocol(e)
    }
}

/// A device may reasonably chat during a long erase; it may not chat forever.
const MAX_INFO: usize = 10_000;

/// A fastboot conversation.
pub struct Session<T: Transport> {
    t: T,
    /// Everything the device said via `INFO`, in order. Kept rather than
    /// dropped: when a flash fails, the INFO lines before it are usually the
    /// only explanation of why.
    pub info: Vec<String>,
}

impl<T: Transport> Session<T> {
    pub fn new(t: T) -> Self {
        Session { t, info: Vec::new() }
    }

    pub fn into_inner(self) -> T {
        self.t
    }

    /// Read until something that is not `INFO`.
    ///
    /// INFO is progress chatter and the exchange continues. A reader that
    /// returns on the first packet desynchronises the session: the next
    /// command then reads the previous command's real answer, and every reply
    /// after that belongs to the wrong question.
    fn read_final(&mut self) -> Result<Reply, SessionError> {
        for _ in 0..MAX_INFO {
            let raw = self.t.read().map_err(SessionError::Io)?;
            match parse_reply(&raw)? {
                Reply::Info(s) => self.info.push(s),
                other => return Ok(other),
            }
        }
        Err(SessionError::TooManyInfo)
    }

    /// Send a command and take its answer.
    pub fn command(&mut self, cmd: &str) -> Result<Reply, SessionError> {
        let bytes = encode_command(cmd)?;
        self.t.write(&bytes).map_err(SessionError::Io)?;
        self.read_final()
    }

    /// A command that must succeed, returning its payload.
    pub fn expect_okay(&mut self, cmd: &str) -> Result<String, SessionError> {
        match self.command(cmd)? {
            Reply::Okay(s) => Ok(s),
            Reply::Fail(s) => Err(SessionError::Refused(s)),
            Reply::Data(n) => Err(SessionError::NoDataHandshake(format!("DATA {n:#x}"))),
            Reply::Info(_) => unreachable!("read_final consumes INFO"),
        }
    }

    /// `getvar:<name>`.
    ///
    /// A refusal is `None`, not an error: asking for a variable a device does
    /// not have is normal probing — `has-slot` on a non-A/B phone answers
    /// FAIL, and that is an answer.
    pub fn getvar(&mut self, name: &str) -> Result<Option<String>, SessionError> {
        match self.command(&format!("getvar:{name}"))? {
            Reply::Okay(s) => Ok(Some(s)),
            Reply::Fail(_) => Ok(None),
            Reply::Data(n) => Err(SessionError::NoDataHandshake(format!("DATA {n:#x}"))),
            Reply::Info(_) => unreachable!("read_final consumes INFO"),
        }
    }

    /// Read the device's slot configuration.
    pub fn slot_info(&mut self) -> Result<SlotInfo, SessionError> {
        let current = self.getvar("current-slot")?.and_then(|s| Slot::parse(&s));
        let count = self
            .getvar("slot-count")?
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(if current.is_some() { 2 } else { 1 });
        Ok(SlotInfo { current, count })
    }

    /// Is this partition slotted? A device that will not say is treated as
    /// unslotted, because suffixing on a guess names a partition that may not
    /// exist.
    pub fn has_slot(&mut self, partition: &str) -> Result<bool, SessionError> {
        Ok(self
            .getvar(&crate::slots::has_slot_query(partition))?
            .map(|v| v.trim().eq_ignore_ascii_case("yes"))
            .unwrap_or(false))
    }

    /// Upload an image into the device's buffer.
    ///
    /// Refuses a short `DATA` rather than writing what fits: a partial image
    /// flashed to a boot partition is worse than a failed flash, because it
    /// looks like it worked.
    pub fn upload(&mut self, image: &[u8]) -> Result<(), SessionError> {
        let cmd = download_command(image.len() as u32);
        let bytes = encode_command(&cmd)?;
        self.t.write(&bytes).map_err(SessionError::Io)?;
        match self.read_final()? {
            Reply::Data(n) if n as usize == image.len() => {}
            Reply::Data(n) => {
                return Err(SessionError::BufferTooSmall { needed: image.len(), offered: n })
            }
            Reply::Fail(s) => return Err(SessionError::Refused(s)),
            other => return Err(SessionError::NoDataHandshake(format!("{other:?}"))),
        }
        for (a, b) in chunks(image.len(), CHUNK) {
            self.t.write(&image[a..b]).map_err(SessionError::Io)?;
        }
        match self.read_final()? {
            Reply::Okay(_) => Ok(()),
            Reply::Fail(s) => Err(SessionError::Refused(s)),
            other => Err(SessionError::NoDataHandshake(format!("{other:?}"))),
        }
    }

    /// Upload an image and write it to a partition.
    ///
    /// `slot` suffixes the partition when the device says it is slotted. Pass
    /// `None` to write wherever fastboot resolves the bare name, which on an
    /// A/B device is the ACTIVE slot — right for installing an OS, wrong for
    /// trying a kernel.
    pub fn flash(
        &mut self,
        partition: &str,
        image: &[u8],
        slot: Option<Slot>,
    ) -> Result<String, SessionError> {
        let slotted = self.has_slot(partition)?;
        let target = partition_for(partition, slot, slotted);
        self.upload(image)?;
        self.expect_okay(&format!("flash:{target}"))?;
        Ok(target)
    }

    pub fn erase(&mut self, partition: &str, slot: Option<Slot>) -> Result<String, SessionError> {
        let slotted = self.has_slot(partition)?;
        let target = partition_for(partition, slot, slotted);
        self.expect_okay(&format!("erase:{target}"))?;
        Ok(target)
    }

    pub fn set_active(&mut self, slot: Slot) -> Result<(), SessionError> {
        self.expect_okay(&format!("set_active:{}", slot.letter())).map(|_| ())
    }

    /// Reboot.
    ///
    /// A device that reboots may never answer, and that is success rather than
    /// a transport error — treating the silence as a failure would make every
    /// successful run end in a red line.
    pub fn reboot(&mut self) -> Result<(), SessionError> {
        match self.command("reboot") {
            Ok(Reply::Okay(_)) | Err(SessionError::Io(_)) => Ok(()),
            Ok(Reply::Fail(s)) => Err(SessionError::Refused(s)),
            Ok(other) => Err(SessionError::NoDataHandshake(format!("{other:?}"))),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A transport that answers from a script and records what it was given.
    struct Fake {
        replies: Vec<Vec<u8>>,
        pub writes: Vec<Vec<u8>>,
        /// Fail the read after this many, to model a device that went away.
        die_after: Option<usize>,
    }

    impl Fake {
        fn new(replies: &[&str]) -> Fake {
            Fake {
                replies: replies.iter().map(|r| r.as_bytes().to_vec()).collect(),
                writes: Vec::new(),
                die_after: None,
            }
        }
        /// What was written as text, commands only (chunks are not utf8-safe).
        fn commands(&self) -> Vec<String> {
            self.writes
                .iter()
                .filter_map(|w| std::str::from_utf8(w).ok().map(|s| s.to_string()))
                .filter(|s| s.contains(':') || s == "reboot")
                .collect()
        }
    }

    impl Transport for Fake {
        fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.writes.push(bytes.to_vec());
            Ok(())
        }
        fn read(&mut self) -> Result<Vec<u8>, String> {
            if let Some(n) = self.die_after {
                if self.writes.len() > n {
                    return Err("device disconnected".into());
                }
            }
            if self.replies.is_empty() {
                return Err("no more scripted replies".into());
            }
            Ok(self.replies.remove(0))
        }
    }

    #[test]
    fn info_lines_are_collected_and_do_not_end_the_exchange() {
        // The failure this prevents: returning on the first packet leaves the
        // real answer in the pipe, and every later reply belongs to the wrong
        // question.
        let mut s = Session::new(Fake::new(&["INFOerasing", "INFOstill going", "OKAYdone"]));
        assert_eq!(s.expect_okay("erase:boot").unwrap(), "done");
        assert_eq!(s.info, vec!["erasing", "still going"]);
    }

    #[test]
    fn an_endless_talker_is_cut_off_rather_than_hanging() {
        let replies: Vec<&str> = vec!["INFOchat"; MAX_INFO + 1];
        let mut s = Session::new(Fake::new(&replies));
        assert_eq!(s.expect_okay("erase:boot"), Err(SessionError::TooManyInfo));
    }

    #[test]
    fn the_upload_handshake_announces_then_sends_then_confirms() {
        let image = vec![7u8; CHUNK + 500];
        let data = format!("DATA{:08x}", image.len());
        let mut s = Session::new(Fake::new(&[&data, "OKAY"]));
        s.upload(&image).unwrap();
        let f = s.into_inner();
        // download: first, with the size in eight hex digits.
        assert_eq!(
            std::str::from_utf8(&f.writes[0]).unwrap(),
            format!("download:{:08x}", image.len())
        );
        // Then the bytes, chunked, and totalling exactly the image — a padded
        // tail would change what gets written.
        let sent: usize = f.writes[1..].iter().map(|w| w.len()).sum();
        assert_eq!(sent, image.len());
        assert_eq!(f.writes.len(), 3, "one command plus two chunks");
    }

    #[test]
    fn a_device_offering_a_smaller_buffer_is_refused_not_truncated() {
        // The device's buffer is what it is. Sending the whole image anyway
        // overruns it and the damage shows up as a corrupt flash rather than
        // an error — so a short DATA stops here. Fitting a larger image needs
        // sparse upload, which is a different feature, not a smaller write.
        let image = vec![0u8; 8192];
        let mut s = Session::new(Fake::new(&["DATA00001000"]));
        assert_eq!(
            s.upload(&image),
            Err(SessionError::BufferTooSmall { needed: 8192, offered: 0x1000 })
        );
        // Nothing after the command was written.
        assert_eq!(s.into_inner().writes.len(), 1);
    }

    #[test]
    fn a_refused_download_does_not_send_the_image() {
        let mut s = Session::new(Fake::new(&["FAILnot unlocked"]));
        assert_eq!(
            s.upload(&[1, 2, 3]),
            Err(SessionError::Refused("not unlocked".into()))
        );
        assert_eq!(s.into_inner().writes.len(), 1, "the bytes stayed home");
    }

    #[test]
    fn flashing_a_slotted_partition_targets_the_slot_it_was_told_to() {
        // has-slot:boot → yes, so the suffix applies and the write lands on
        // the spare half rather than the running one.
        let mut s = Session::new(Fake::new(&[
            "OKAYyes",        // has-slot:boot
            "DATA00000003",   // download
            "OKAY",           // upload done
            "OKAY",           // flash
        ]));
        let target = s.flash("boot", &[1, 2, 3], Some(Slot::B)).unwrap();
        assert_eq!(target, "boot_b");
        let cmds = s.into_inner().commands();
        assert!(cmds.contains(&"getvar:has-slot:boot".to_string()));
        assert!(cmds.contains(&"flash:boot_b".to_string()));
    }

    #[test]
    fn an_unslotted_partition_keeps_its_bare_name() {
        // userdata has no _a even on a device where boot does; suffixing it
        // names something that does not exist.
        let mut s = Session::new(Fake::new(&[
            "FAILunknown variable", // has-slot:userdata
            "DATA00000001",
            "OKAY",
            "OKAY",
        ]));
        let target = s.flash("userdata", &[9], Some(Slot::A)).unwrap();
        assert_eq!(target, "userdata");
    }

    #[test]
    fn a_missing_variable_is_an_answer_and_not_an_error() {
        // Probing is normal: has-slot on a single-slot phone answers FAIL, and
        // that means "no", not "the session broke".
        let mut s = Session::new(Fake::new(&["FAILunknown variable"]));
        assert_eq!(s.getvar("has-slot:boot").unwrap(), None);
    }

    #[test]
    fn slot_info_reads_both_variables_and_survives_a_single_slot_device() {
        let mut s = Session::new(Fake::new(&["OKAYb", "OKAY2"]));
        let info = s.slot_info().unwrap();
        assert_eq!(info.current, Some(Slot::B));
        assert!(info.is_ab());
        assert_eq!(info.target(), Some(Slot::A), "the spare half");

        // A device with neither variable is not A/B, and must not be treated
        // as one.
        let mut s = Session::new(Fake::new(&["FAILno", "FAILno"]));
        let info = s.slot_info().unwrap();
        assert!(!info.is_ab());
        assert_eq!(info.target(), None);
    }

    #[test]
    fn a_reboot_that_never_answers_is_success() {
        // The device is gone because it did what it was told. Treating the
        // silence as a transport failure would end every successful run in a
        // red line.
        let mut f = Fake::new(&[]);
        f.die_after = Some(0);
        let mut s = Session::new(f);
        assert_eq!(s.reboot(), Ok(()));
    }

    #[test]
    fn a_refusal_carries_the_devices_own_words() {
        // "FAILED" tells the operator nothing; the device's text is the
        // whole diagnostic.
        let mut s = Session::new(Fake::new(&["FAILdevice is locked"]));
        assert_eq!(
            s.expect_okay("flash:boot_b"),
            Err(SessionError::Refused("device is locked".into()))
        );
    }
}
