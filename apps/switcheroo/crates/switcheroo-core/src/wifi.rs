//! Reading the wifi link on Linux, for the "when I get home" and "when the
//! signal gets far" triggers.
//!
//! Two sources, both free of dependencies:
//! * `iw dev <if> link` — SSID and signal in one call. Needs `iw`, which
//!   every Linux phone distro ships.
//! * `/proc/net/wireless` — signal level for every wireless interface, no
//!   tool needed, but no SSID.
//!
//! Both are parsed here from captured output; the shell runs the command.
//! `routine-kit` in `shared/` already parses `nmcli monitor` for the
//! association EDGES; this is the LEVEL, which that stream does not carry.

use crate::schedule::Wifi;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Link {
    pub ssid: Option<String>,
    pub signal_dbm: Option<i16>,
}

/// Parse `iw dev wlan0 link`. `Not connected.` → `None`.
pub fn parse_iw_link(out: &str) -> Option<Link> {
    let mut link = Link::default();
    let mut connected = false;
    for line in out.lines() {
        let l = line.trim();
        if l.starts_with("Connected to ") { connected = true; }
        if let Some(s) = l.strip_prefix("SSID: ") { link.ssid = Some(s.trim().to_string()); }
        if let Some(s) = l.strip_prefix("signal: ") {
            link.signal_dbm = s.split_whitespace().next().and_then(|n| n.parse().ok());
        }
    }
    if connected { Some(link) } else { None }
}

/// Parse `/proc/net/wireless`; the first interface's `level` column, dBm.
pub fn parse_proc_net_wireless(out: &str) -> Option<i16> {
    for line in out.lines().skip(2) {
        let mut it = line.split_whitespace();
        let _iface = it.next()?;
        let _status = it.next()?;
        let _link = it.next()?;
        let level = it.next()?.trim_end_matches('.');
        if let Ok(v) = level.parse::<f32>() { return Some(v as i16); }
    }
    None
}

/// Combine a reading with the configured home SSID into what the
/// scheduler wants.
pub fn to_signals(link: Option<&Link>, home_ssid: Option<&str>) -> Wifi {
    match link {
        None => Wifi { connected: false, at_home: false, rssi_dbm: None },
        Some(l) => Wifi {
            connected: true,
            at_home: matches!((home_ssid, &l.ssid), (Some(h), Some(s)) if h.eq_ignore_ascii_case(s)),
            rssi_dbm: l.signal_dbm,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IW: &str = "Connected to aa:bb:cc:dd:ee:ff (on wlan0)\n\tSSID: HomeNet\n\tfreq: 5180\n\tRX: 1234 bytes (12 packets)\n\tTX: 567 bytes (6 packets)\n\tsignal: -54 dBm\n\trx bitrate: 866.7 MBit/s\n";
    const PROC: &str = "Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE\n face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22\n wlan0: 0000   58.  -52.  -256        0      0      0      0      0        0\n";

    #[test]
    fn iw_link_yields_ssid_and_signal() {
        assert_eq!(parse_iw_link(IW), Some(Link { ssid: Some("HomeNet".into()), signal_dbm: Some(-54) }));
        assert_eq!(parse_iw_link("Not connected.\n"), None);
    }

    #[test]
    fn proc_net_wireless_yields_the_level() {
        assert_eq!(parse_proc_net_wireless(PROC), Some(-52));
        assert_eq!(parse_proc_net_wireless("Inter-|\n face |\n"), None);
    }

    #[test]
    fn signals_combine_the_home_ssid() {
        let l = parse_iw_link(IW).unwrap();
        assert_eq!(to_signals(Some(&l), Some("homenet")), Wifi { connected: true, at_home: true, rssi_dbm: Some(-54) });
        assert_eq!(to_signals(Some(&l), Some("Work")), Wifi { connected: true, at_home: false, rssi_dbm: Some(-54) });
        assert_eq!(to_signals(Some(&l), None), Wifi { connected: true, at_home: false, rssi_dbm: Some(-54) });
        assert_eq!(to_signals(None, Some("HomeNet")), Wifi::default());
    }
}
