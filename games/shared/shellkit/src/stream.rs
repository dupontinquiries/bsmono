//! stream — LAN render assist: a desktop does the expensive far-field ray
//! work, the phone finishes the cheap near-field residual.
//!
//! The idea (yours): most of a reflection ray's cost is the long march
//! through the scene, but only the LAST stretch before the camera actually
//! changes as the car moves. So split the ray:
//!
//! * the HOST (a big machine on the same LAN, found through the fleet
//!   store / DHT exactly like a multiplayer session) marches the far field
//!   and publishes the result as a grid of world-space RADIANCE PROBES —
//!   "if a ray leaves this point in this direction, here is what it sees".
//! * the CLIENT marches only the first few metres of each ray. If it hits
//!   something close, that is the answer and no network was involved. If it
//!   does not, it samples the nearest probe instead of continuing — the
//!   residual is local, the expensive tail is remote.
//!
//! Why this shape and not video: a probe grid is *view-independent*, so a
//! late packet is still usable — it degrades to a slightly stale
//! reflection rather than a torn frame, and the client never blocks on the
//! network. Probes are also tiny: one octahedral direction set per point.
//!
//! What is proven here (headless, `--sim stream`): the protocol, the UDP
//! transport, the staleness/reprojection accounting, and the ACCURACY of
//! probe-assisted shading against a full local march. What is NOT here is
//! the shader path that samples the cache on the GPU — that needs real
//! hardware at both ends to tune, and is written up in DESIGN.md instead.

use glam::Vec3;
use std::net::{SocketAddr, UdpSocket};

/// Directions per probe (a small octahedral set: 6 axes + 8 diagonals).
pub const DIRS: usize = 14;
/// Probes per packet payload.
pub const PROBES: usize = 32;

pub fn dir_of(i: usize) -> Vec3 {
    const AX: [Vec3; 6] = [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ];
    if i < 6 {
        AX[i]
    } else {
        let k = i - 6;
        Vec3::new(
            if k & 1 == 0 { 1.0 } else { -1.0 },
            if k & 2 == 0 { 1.0 } else { -1.0 },
            if k & 4 == 0 { 1.0 } else { -1.0 },
        )
        .normalize()
    }
}

/// One probe: a point, and what a ray leaving it sees in each direction.
#[derive(Clone, Copy)]
pub struct Probe {
    pub pos: Vec3,
    pub rgb: [[u8; 3]; DIRS],
}

impl Probe {
    fn write(&self, out: &mut Vec<u8>) {
        for c in [self.pos.x, self.pos.y, self.pos.z] {
            out.extend_from_slice(&c.to_le_bytes());
        }
        for d in self.rgb.iter() {
            out.extend_from_slice(d);
        }
    }
    fn read(b: &[u8]) -> Probe {
        let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let mut rgb = [[0u8; 3]; DIRS];
        for (i, d) in rgb.iter_mut().enumerate() {
            let o = 12 + i * 3;
            *d = [b[o], b[o + 1], b[o + 2]];
        }
        Probe { pos: Vec3::new(f(0), f(4), f(8)), rgb }
    }
    pub const BYTES: usize = 12 + DIRS * 3;
}

/// Client -> host: where I am and where I am going (so the host can place
/// probes ahead of the car rather than under it).
pub struct PoseMsg {
    pub seq: u32,
    pub pos: Vec3,
    pub vel: Vec3,
}

impl PoseMsg {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(28);
        v.extend_from_slice(b"SKQ1");
        v.extend_from_slice(&self.seq.to_le_bytes());
        for c in [self.pos.x, self.pos.y, self.pos.z, self.vel.x, self.vel.y, self.vel.z] {
            v.extend_from_slice(&c.to_le_bytes());
        }
        v
    }
    pub fn decode(b: &[u8]) -> Option<PoseMsg> {
        if b.len() < 32 || &b[0..4] != b"SKQ1" {
            return None;
        }
        let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        Some(PoseMsg {
            seq: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            pos: Vec3::new(f(8), f(12), f(16)),
            vel: Vec3::new(f(20), f(24), f(28)),
        })
    }
}

/// Host -> client: a batch of probes answering a given pose sequence.
pub fn encode_probes(seq: u32, probes: &[Probe]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + probes.len() * Probe::BYTES);
    v.extend_from_slice(b"SKR1");
    v.extend_from_slice(&seq.to_le_bytes());
    for p in probes {
        p.write(&mut v);
    }
    v
}

pub fn decode_probes(b: &[u8]) -> Option<(u32, Vec<Probe>)> {
    if b.len() < 8 || &b[0..4] != b"SKR1" {
        return None;
    }
    let seq = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let n = (b.len() - 8) / Probe::BYTES;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let o = 8 + i * Probe::BYTES;
        out.push(Probe::read(&b[o..o + Probe::BYTES]));
    }
    Some((seq, out))
}

/// The client's local store of what the host has told it.
pub struct Cache {
    pub probes: Vec<Probe>,
    pub age: f32,
    pub last_seq: u32,
}

impl Default for Cache {
    fn default() -> Cache {
        Cache { probes: Vec::new(), age: 0.0, last_seq: 0 }
    }
}

impl Cache {
    /// Nearest probe's radiance for a ray leaving `p` toward `d`. This is
    /// the RESIDUAL handoff: the client has already marched the near field
    /// and missed, so the far field comes from here.
    pub fn sample(&self, p: Vec3, d: Vec3) -> Option<[f32; 3]> {
        let pr = self
            .probes
            .iter()
            .min_by(|a, b| a.pos.distance_squared(p).total_cmp(&b.pos.distance_squared(p)))?;
        // blend the two closest directions so the set does not read as 14 facets
        let mut best = (f32::MIN, 0usize);
        let mut second = (f32::MIN, 0usize);
        for i in 0..DIRS {
            let dot = dir_of(i).dot(d);
            if dot > best.0 {
                second = best;
                best = (dot, i);
            } else if dot > second.0 {
                second = (dot, i);
            }
        }
        let w = (best.0 - second.0).clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 * (0.5 + 0.5 * w) + b as f32 * (0.5 - 0.5 * w)) / 255.0;
        let (x, y) = (pr.rgb[best.1], pr.rgb[second.1]);
        Some([mix(x[0], y[0]), mix(x[1], y[1]), mix(x[2], y[2])])
    }
}

/// Non-blocking UDP endpoint shared by both ends.
pub struct Endpoint {
    pub sock: UdpSocket,
}

impl Endpoint {
    pub fn bind(addr: &str) -> std::io::Result<Endpoint> {
        let sock = UdpSocket::bind(addr)?;
        sock.set_nonblocking(true)?;
        Ok(Endpoint { sock })
    }
    pub fn addr(&self) -> std::io::Result<SocketAddr> {
        self.sock.local_addr()
    }
    pub fn send(&self, to: SocketAddr, b: &[u8]) {
        let _ = self.sock.send_to(b, to);
    }
    /// Drain one datagram if any is waiting.
    pub fn recv(&self, buf: &mut [u8]) -> Option<(usize, SocketAddr)> {
        self.sock.recv_from(buf).ok()
    }
}
