//! profile — a player's identity + presence, published to the DHT under their id. Serialized with a
//! versioned, length-prefixed frame (same discipline as the voice relay wire) so it round-trips
//! through any backend as opaque bytes.

/// What a player is doing right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presence {
    Offline,
    Online,
    /// In a specific game (name), optionally joinable via a session token.
    InGame { game: String, session: Option<String> },
}

impl Presence {
    fn tag(&self) -> u8 {
        match self {
            Presence::Offline => 0,
            Presence::Online => 1,
            Presence::InGame { .. } => 2,
        }
    }
    /// Is this player reachable to invite/join right now?
    pub fn is_online(&self) -> bool {
        !matches!(self, Presence::Offline)
    }
}

/// A player's shared profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Stable player id (e.g. a crypto-kit key fingerprint) — the DHT key.
    pub id: String,
    pub name: String,
    pub presence: Presence,
    /// Friend ids (their profiles are looked up separately).
    pub friends: Vec<String>,
}

const WIRE_V: u8 = 1;

fn put_str(b: &mut Vec<u8>, s: &str) {
    b.extend_from_slice(&(s.len() as u32).to_le_bytes());
    b.extend_from_slice(s.as_bytes());
}
fn take_str(b: &mut &[u8]) -> Option<String> {
    if b.len() < 4 {
        return None;
    }
    let n = u32::from_le_bytes(b[..4].try_into().ok()?) as usize;
    let rest = &b[4..];
    if rest.len() < n {
        return None;
    }
    let (s, tail) = rest.split_at(n);
    *b = tail;
    Some(String::from_utf8_lossy(s).into_owned())
}

impl Profile {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Profile { id: id.into(), name: name.into(), presence: Presence::Online, friends: Vec::new() }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![WIRE_V, self.presence.tag()];
        put_str(&mut b, &self.id);
        put_str(&mut b, &self.name);
        if let Presence::InGame { game, session } = &self.presence {
            put_str(&mut b, game);
            put_str(&mut b, session.as_deref().unwrap_or(""));
        }
        b.extend_from_slice(&(self.friends.len() as u32).to_le_bytes());
        for f in &self.friends {
            put_str(&mut b, f);
        }
        b
    }

    pub fn decode(bytes: &[u8]) -> Option<Profile> {
        let mut b = bytes;
        let (&v, rest) = b.split_first()?;
        if v != WIRE_V {
            return None;
        }
        let (&ptag, rest) = rest.split_first()?;
        b = rest;
        let id = take_str(&mut b)?;
        let name = take_str(&mut b)?;
        let presence = match ptag {
            0 => Presence::Offline,
            1 => Presence::Online,
            2 => {
                let game = take_str(&mut b)?;
                let s = take_str(&mut b)?;
                Presence::InGame { game, session: if s.is_empty() { None } else { Some(s) } }
            }
            _ => return None,
        };
        if b.len() < 4 {
            return None;
        }
        let n = u32::from_le_bytes(b[..4].try_into().ok()?) as usize;
        b = &b[4..];
        let mut friends = Vec::with_capacity(n);
        for _ in 0..n {
            friends.push(take_str(&mut b)?);
        }
        Some(Profile { id, name, presence, friends })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_round_trips_each_presence() {
        for presence in [
            Presence::Offline,
            Presence::Online,
            Presence::InGame { game: "clawbound".into(), session: Some("lobby-7".into()) },
            Presence::InGame { game: "early-fps".into(), session: None },
        ] {
            let p = Profile { id: "kd@a1b2".into(), name: "kesslerd".into(), presence, friends: vec!["bob@f00".into(), "amy@9c".into()] };
            assert_eq!(Profile::decode(&p.encode()), Some(p));
        }
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(Profile::decode(&[]).is_none());
        assert!(Profile::decode(&[9, 9, 9]).is_none());
    }

    #[test]
    fn online_helper() {
        assert!(!Presence::Offline.is_online());
        assert!(Presence::Online.is_online());
        assert!(Presence::InGame { game: "x".into(), session: None }.is_online());
    }
}
