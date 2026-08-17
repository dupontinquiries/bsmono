//! udevcheck — does this phone actually reach a rule that grants access?
//!
//! The failure this exists to catch is quiet by construction. A rules file is
//! installed, it looks right, `lsusb` shows the device, and nothing works —
//! because the device matched a vendor block, missed every product line in it,
//! and jumped to the end of the file with no `uaccess` tag, no `GROUP` and no
//! `MODE` applied. Nothing logs. Nothing is obviously wrong. The rules file is
//! *present* and the device gets *nothing*.
//!
//! The android-udev-rules layout makes this easy to hit. Most vendor blocks
//! end with a catch-all:
//!
//! ```text
//! ENV{adb_user}="yes"
//! GOTO="android_usb_rule_match"      ← unlisted products still get access
//! ```
//!
//! but several — Fairphone among them — end with:
//!
//! ```text
//! GOTO="android_usb_rules_end"       ← unlisted products get nothing
//! ```
//!
//! So a phone whose product id is newer than the rules file falls straight
//! through. Same vendor, same file, no access.
//!
//! ## This is a heuristic and `udevadm test` is the authority
//!
//! Real udev evaluates far more than the `ATTR{idVendor}` / `ATTR{idProduct}`
//! / `GOTO` / `LABEL` subset modelled here: `SUBSYSTEM`, `ACTION`, glob
//! patterns, `ENV` propagation across rules, and the ordering of every other
//! file in `rules.d`. This walks one file and answers one question — "does
//! this vid:pid reach something that grants access" — well enough to put a
//! useful warning in front of someone before they spend an evening blaming
//! their USB controller. When it says "no rule reached", believe it enough to
//! run `udevadm test`; do not treat a "yes" as proof the device will work.

/// What a rules file did with a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Reached a rule that grants access, at this line.
    Granted { line: usize, how: String },
    /// Matched the vendor but no product line, and fell out of the block.
    ///
    /// The interesting case: the file supports the phone's manufacturer and
    /// not the phone.
    VendorOnly { vendor_line: usize },
    /// The vendor is not in the file at all.
    NoVendor,
}

impl Verdict {
    pub fn ok(&self) -> bool {
        matches!(self, Verdict::Granted { .. })
    }

    /// What to tell the operator.
    pub fn explain(&self, vid: &str, pid: &str) -> String {
        match self {
            Verdict::Granted { how, .. } => format!("{vid}:{pid} is granted access by {how}"),
            Verdict::VendorOnly { vendor_line } => format!(
                "{vid}:{pid} — the rules know vendor {vid} (line {vendor_line}) but not \
                 product {pid}, and that block has no catch-all, so no access is granted"
            ),
            Verdict::NoVendor => {
                format!("{vid}:{pid} — vendor {vid} does not appear in the rules at all")
            }
        }
    }
}

/// One parsed line, reduced to what matters for reachability.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    /// `ATTR{idVendor}!="x", GOTO="label"` — skip the block unless vendor x.
    VendorGuard { vid: String, goto: String },
    /// `ATTR{idVendor}=="x", ATTR{idProduct}=="y", GOTO="label"`
    VendorProduct { vid: String, pid: String, goto: Option<String> },
    /// `ATTR{idProduct}=="y", GOTO="label"` inside a vendor block.
    Product { pid: String, goto: Option<String> },
    /// `LABEL="x"`
    Label(String),
    /// A bare `GOTO="x"`.
    Goto(String),
    /// A line that grants access: `TAG+="uaccess"`, `MODE=`, `GROUP=`, or the
    /// `ENV{adb_*}="yes"` markers that lead to the shared grant.
    Grant(String),
    Other,
}

fn quoted_after(s: &str, key: &str) -> Option<String> {
    let at = s.find(key)?;
    let rest = &s[at + key.len()..];
    let open = rest.find('"')?;
    let rest = &rest[open + 1..];
    let close = rest.find('"')?;
    Some(rest[..close].to_string())
}

fn parse_line(raw: &str) -> Line {
    let s = raw.trim();
    if s.is_empty() || s.starts_with('#') {
        return Line::Other;
    }
    if let Some(l) = quoted_after(s, "LABEL=") {
        return Line::Label(l);
    }
    let goto = quoted_after(s, "GOTO=");
    let vid_ne = s.contains("ATTR{idVendor}!=");
    let vid = quoted_after(s, "ATTR{idVendor}==").or_else(|| {
        if vid_ne { quoted_after(s, "ATTR{idVendor}!=") } else { None }
    });
    let pid = quoted_after(s, "ATTR{idProduct}==");

    if vid_ne {
        if let (Some(v), Some(g)) = (vid.clone(), goto.clone()) {
            return Line::VendorGuard { vid: v, goto: g };
        }
    }
    match (vid, pid) {
        (Some(v), Some(p)) => return Line::VendorProduct { vid: v, pid: p, goto },
        (None, Some(p)) => return Line::Product { pid: p, goto },
        _ => {}
    }
    // Access-granting forms.
    for marker in ["TAG+=\"uaccess\"", "MODE=", "GROUP=", "ENV{adb_user}=\"yes\""] {
        if s.contains(marker) {
            return Line::Grant(marker.trim_end_matches('=').to_string());
        }
    }
    if let Some(g) = goto {
        // A GOTO is only unconditional if the line carries no test we did not
        // model. The real file opens with
        //   ACTION!="add", ACTION!="bind", GOTO="android_usb_rules_end"
        //   SUBSYSTEM!="usb",              GOTO="android_usb_rules_end"
        // and treating those as unconditional jumps ends the walk on line one,
        // reporting every device as unreachable. Those conditions are all true
        // for a plugged-in USB device, so the honest reading of an unmodelled
        // test is "not taken" — and being wrong that way produces a missed
        // warning rather than a false one.
        if !s.contains("==") && !s.contains("!=") {
            return Line::Goto(g);
        }
        return Line::Other;
    }
    Line::Other
}

/// The label every android-udev-rules block jumps to when it gives up.
const DEAD_END: &str = "android_usb_rules_end";

/// Walk a rules file for one device.
///
/// `vid` and `pid` are lower-case four-digit hex without the `0x`, as `lsusb`
/// prints them.
pub fn evaluate(rules: &str, vid: &str, pid: &str) -> Verdict {
    let lines: Vec<Line> = rules.lines().map(parse_line).collect();
    let raw: Vec<&str> = rules.lines().collect();
    let vid = vid.trim().to_ascii_lowercase();
    let pid = pid.trim().to_ascii_lowercase();

    let find_label = |name: &str| -> Option<usize> {
        lines.iter().position(|l| matches!(l, Line::Label(l2) if l2 == name))
    };

    let mut i = 0usize;
    let mut vendor_line: Option<usize> = None;
    // Bounded: a malformed file with a GOTO loop must not hang a wizard.
    let mut fuel = lines.len() * 4 + 64;

    while i < lines.len() && fuel > 0 {
        fuel -= 1;
        match &lines[i] {
            Line::VendorGuard { vid: v, goto } => {
                if *v == vid {
                    // We are inside this vendor's block now.
                    vendor_line = Some(i + 1);
                    i += 1;
                } else {
                    match find_label(goto) {
                        Some(at) => i = at + 1,
                        None => return Verdict::NoVendor,
                    }
                }
            }
            Line::VendorProduct { vid: v, pid: p, goto } => {
                if *v == vid {
                    vendor_line.get_or_insert(i + 1);
                    if *p == pid {
                        return match goto {
                            Some(g) if g == DEAD_END => Verdict::VendorOnly {
                                vendor_line: vendor_line.unwrap_or(i + 1),
                            },
                            Some(_) | None => Verdict::Granted {
                                line: i + 1,
                                how: raw[i].trim().to_string(),
                            },
                        };
                    }
                }
                i += 1;
            }
            Line::Product { pid: p, goto } => {
                // Only meaningful once inside the right vendor block.
                if vendor_line.is_some() && *p == pid {
                    return match goto {
                        Some(g) if g == DEAD_END => {
                            Verdict::VendorOnly { vendor_line: vendor_line.unwrap() }
                        }
                        _ => Verdict::Granted { line: i + 1, how: raw[i].trim().to_string() },
                    };
                }
                i += 1;
            }
            Line::Goto(g) => {
                if vendor_line.is_some() && g == DEAD_END {
                    // The block gave up on us: matched the vendor, no product.
                    return Verdict::VendorOnly { vendor_line: vendor_line.unwrap() };
                }
                match find_label(g) {
                    Some(at) => i = at + 1,
                    None => break,
                }
            }
            Line::Grant(how) => {
                if vendor_line.is_some() {
                    // A catch-all inside the vendor's own block.
                    return Verdict::Granted { line: i + 1, how: how.clone() };
                }
                i += 1;
            }
            Line::Label(_) | Line::Other => i += 1,
        }
    }
    match vendor_line {
        Some(l) => Verdict::VendorOnly { vendor_line: l },
        None => Verdict::NoVendor,
    }
}

/// A rule that grants access to exactly this device.
///
/// Deliberately minimal and vendor-scoped rather than a copy of the upstream
/// block: it is an override the operator can read in one line and delete when
/// upstream catches up. `uaccess` is the part that matters — it gives the
/// logged-in seat an ACL, which is what a browser and a user-run fastboot both
/// need. `GROUP` is included for the case where there is no local seat, and is
/// harmless when the group is absent.
pub fn fix_rule(vid: &str, pid: &str) -> String {
    format!(
        "# Added by gitmind: {vid}:{pid} reached no access-granting rule.\n\
         SUBSYSTEM==\"usb\", ATTR{{idVendor}}==\"{vid}\", ATTR{{idProduct}}==\"{pid}\", \
         MODE=\"0660\", GROUP=\"plugdev\", TAG+=\"uaccess\"\n"
    )
}

/// Where the override belongs, and why the number is high.
///
/// udev processes `rules.d` in lexical order and later rules can add tags, so
/// a number above the android rules (51-) means this runs after them rather
/// than being jumped over by their `GOTO`s — those jumps are internal to their
/// own file and cannot skip a later one.
pub const FIX_PATH: &str = "/etc/udev/rules.d/99-gitmind-fleet.rules";

/// The commands that make a new rule take effect without a reboot.
pub fn reload_commands() -> Vec<&'static str> {
    vec!["udevadm control --reload-rules", "udevadm trigger"]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real shape of the Fairphone block: a vendor guard, a handful of
    /// products, and a dead end instead of a catch-all.
    const FAIRPHONE: &str = concat!(
        "ATTR{idVendor}!=\"2ae5\", GOTO=\"not_Fairphone2\"\n",
        "ATTR{idProduct}==\"9015\", GOTO=\"adb\"\n",
        "ATTR{idProduct}==\"9024\", GOTO=\"adbrndis\"\n",
        "ATTR{idProduct}==\"90dc\", GOTO=\"adb\"\n",
        "GOTO=\"android_usb_rules_end\"\n",
        "LABEL=\"not_Fairphone2\"\n",
        "LABEL=\"adb\"\n",
        "ENV{adb_user}=\"yes\"\n",
        "LABEL=\"android_usb_rules_end\"\n",
    );

    /// A block that ends the other way, like HTC and Sony do.
    const WITH_CATCHALL: &str = concat!(
        "ATTR{idVendor}!=\"0bb4\", GOTO=\"not_HTC\"\n",
        "ATTR{idProduct}==\"0c01\", GOTO=\"mass\"\n",
        "ENV{adb_user}=\"yes\"\n",
        "GOTO=\"android_usb_rule_match\"\n",
        "LABEL=\"not_HTC\"\n",
        "LABEL=\"android_usb_rule_match\"\n",
        "LABEL=\"android_usb_rules_end\"\n",
    );

    #[test]
    fn a_listed_product_is_granted() {
        let v = evaluate(FAIRPHONE, "2ae5", "9015");
        assert!(v.ok(), "{v:?}");
    }

    #[test]
    fn the_quiet_failure_is_named_for_what_it_is() {
        // A Fairphone with a product id newer than the rules file: the vendor
        // matches, no product line does, and the block jumps to the end with
        // nothing applied. This is the case that looks like broken hardware.
        let v = evaluate(FAIRPHONE, "2ae5", "4ee0");
        assert!(!v.ok());
        assert!(matches!(v, Verdict::VendorOnly { .. }));
        let msg = v.explain("2ae5", "4ee0");
        assert!(msg.contains("no catch-all"), "{msg}");
        assert!(msg.contains("2ae5"), "the message names the vendor it did match");
    }

    #[test]
    fn a_block_that_ends_with_a_catch_all_grants_an_unlisted_product() {
        // The difference between the two block shapes is the whole finding, so
        // it is worth a test on its own.
        let v = evaluate(WITH_CATCHALL, "0bb4", "ffff");
        assert!(v.ok(), "an unlisted HTC product still reaches the catch-all: {v:?}");
    }

    #[test]
    fn an_absent_vendor_is_distinct_from_a_vendor_with_no_catch_all() {
        // Different problems, different fixes: one needs a rule for the
        // product, the other needs a rule for the vendor at all. Collapsing
        // them into "not found" loses that.
        let v = evaluate(FAIRPHONE, "1234", "5678");
        assert_eq!(v, Verdict::NoVendor);
        assert!(v.explain("1234", "5678").contains("does not appear"));
    }

    #[test]
    fn hex_case_from_lsusb_does_not_matter() {
        assert!(evaluate(FAIRPHONE, "2AE5", "9015").ok());
        assert!(evaluate(FAIRPHONE, " 2ae5 ", " 9015 ").ok());
    }

    #[test]
    fn a_goto_loop_terminates_instead_of_hanging_the_wizard() {
        // A malformed or hand-edited file must not spin a preflight check.
        let looped = concat!(
            "LABEL=\"a\"\n",
            "GOTO=\"b\"\n",
            "LABEL=\"b\"\n",
            "GOTO=\"a\"\n",
        );
        let v = evaluate(looped, "2ae5", "9015");
        assert_eq!(v, Verdict::NoVendor);
    }

    #[test]
    fn the_generated_rule_carries_uaccess_and_sorts_after_the_android_rules() {
        let r = fix_rule("2ae5", "4ee0");
        assert!(r.contains("ATTR{idVendor}==\"2ae5\""));
        assert!(r.contains("ATTR{idProduct}==\"4ee0\""));
        // uaccess is the part that matters: it gives the logged-in seat an
        // ACL, which is what both a browser and a user-run fastboot need.
        assert!(r.contains("TAG+=\"uaccess\""));
        assert!(r.starts_with('#'), "an override should say who added it");
        // 99- runs after 51-android; those GOTOs are internal to their file
        // and cannot jump over a later one.
        assert!(FIX_PATH.contains("/99-"));
        assert_eq!(reload_commands().len(), 2);
    }
}

#[cfg(test)]
mod real_file_tests {
    use super::*;

    /// Excerpted verbatim from an installed /etc/udev/rules.d/51-android.rules
    /// (M0Rf30 android-udev-rules), keeping the block structure and the two
    /// endings intact. Fixtures I wrote myself only prove the evaluator
    /// agrees with me; this one has to agree with the file on a real machine.
    const REAL: &str = include_str!("../testdata/51-android-excerpt.rules");

    #[test]
    fn the_fairphone_block_drops_an_unlisted_product() {
        // A Fairphone 5 in fastboot presents a product id that this file — cut
        // for the FP2 — does not list. Vendor matches, no product does, and
        // the block's last line is GOTO="android_usb_rules_end".
        let v = evaluate(REAL, "2ae5", "4ee0");
        assert!(!v.ok(), "expected the drop, got {v:?}");
        assert!(matches!(v, Verdict::VendorOnly { .. }));
    }

    #[test]
    fn the_products_the_file_does_list_still_work() {
        // The evaluator must not cry wolf about the devices the file supports,
        // or the warning gets ignored on the day it is right.
        for pid in ["9015", "9024", "9039", "904e", "90bb", "90dc"] {
            assert!(evaluate(REAL, "2ae5", pid).ok(), "2ae5:{pid} should be granted");
        }
    }

    #[test]
    fn a_google_vid_phone_in_fastboot_is_covered() {
        // Many pmOS devices present 18d1:4ee0 in fastboot, and that one IS
        // listed — so a device can be fine on one vid and dropped on another.
        assert!(evaluate(REAL, "18d1", "4ee0").ok());
        assert!(!evaluate(REAL, "18d1", "d00d").ok(), "unlisted Google pid drops too");
    }

    #[test]
    fn htcs_catch_all_still_distinguishes_it_from_fairphone() {
        // Same file, two block endings, opposite outcomes for an unlisted
        // product. This is the structural finding, checked against the real
        // text rather than my paraphrase of it.
        assert!(evaluate(REAL, "0bb4", "ffff").ok(), "HTC has a catch-all");
        assert!(!evaluate(REAL, "2ae5", "ffff").ok(), "Fairphone does not");
    }
}
