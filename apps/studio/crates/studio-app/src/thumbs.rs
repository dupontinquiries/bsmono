//! thumbs — decoding the filmstrip's frames in the background.
//!
//! The timeline lists the frames it wanted (`studio_egui::filmstrip`); this
//! decodes them on a worker, newest request first -- when the view scrolls,
//! what is on screen now matters more than what was on screen a second ago --
//! and wakes the window as each arrives. Requests that pile up behind a fast
//! scroll are dropped (and reported as dropped, so they can be asked for again
//! if still visible). The decoded frames live here as textures, at most CAP of
//! them, oldest out first.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc;
use studio_egui::filmstrip::{Key, Thumbs};

pub const W: u32 = 96;
pub const H: u32 = 54;
pub const CAP: usize = 500;
/// How many waiting requests the worker keeps; older ones are dropped.
pub const QUEUE: usize = 48;

pub struct Cache {
    pub thumbs: Thumbs,
    handles: HashMap<Key, egui::TextureHandle>,
    order: VecDeque<Key>,
    pending: HashSet<Key>,
    tx: mpsc::Sender<(Key, PathBuf)>,
    rx: mpsc::Receiver<(Key, Option<Vec<u8>>)>,
}

/// The worker's queue rule, apart from the thread: newest last in `stack`;
/// keep the newest `QUEUE`, returning the dropped ones.
pub fn trim(stack: &mut Vec<(Key, PathBuf)>) -> Vec<Key> {
    if stack.len() <= QUEUE { return Vec::new(); }
    let n = stack.len() - QUEUE;
    stack.drain(..n).map(|(k, _)| k).collect()
}

impl Cache {
    pub fn new(wake: crate::wake::Wake) -> Cache {
        let (tx, req_rx) = mpsc::channel::<(Key, PathBuf)>();
        let (res_tx, rx) = crate::wake::channel(wake);
        std::thread::Builder::new().name("studio-thumbs".into()).spawn(move || {
            let mut stack: Vec<(Key, PathBuf)> = Vec::new();
            loop {
                if stack.is_empty() {
                    match req_rx.recv() { Ok(r) => stack.push(r), Err(_) => return }
                }
                while let Ok(r) = req_rx.try_recv() { stack.push(r); }
                for k in trim(&mut stack) { if res_tx.send((k, None)).is_err() { return; } }
                let Some((key, path)) = stack.pop() else { continue };
                let px = studio_video::frame_rgb(&path, studio_core::Ticks::from_seconds(key.1 as f64 / 1000.0), W, H).ok();
                if res_tx.send((key, px)).is_err() { return; }
            }
        }).expect("thumbnail worker");
        Cache { thumbs: Thumbs::default(), handles: HashMap::new(), order: VecDeque::new(), pending: HashSet::new(), tx, rx }
    }

    /// After a timeline draw: take in what the worker finished, and ask for
    /// what the timeline wanted and nobody has asked for yet.
    pub fn pump(&mut self, ctx: &egui::Context, path_of: impl Fn(studio_core::SourceId) -> Option<PathBuf>) {
        while let Ok((key, px)) = self.rx.try_recv() {
            self.pending.remove(&key);
            let Some(px) = px else { continue };
            let tex = ctx.load_texture(format!("thumb-{}-{}", key.0 .0, key.1),
                egui::ColorImage::from_rgb([W as usize, H as usize], &px), egui::TextureOptions::LINEAR);
            self.thumbs.tex.insert(key, tex.id());
            self.handles.insert(key, tex);
            self.order.push_back(key);
            while self.order.len() > CAP {
                if let Some(old) = self.order.pop_front() { self.handles.remove(&old); self.thumbs.tex.remove(&old); }
            }
        }
        let wanted: Vec<Key> = std::mem::take(&mut *self.thumbs.wanted.borrow_mut());
        for key in wanted {
            if self.handles.contains_key(&key) || !self.pending.insert(key) { continue; }
            match path_of(key.0) {
                Some(p) => { let _ = self.tx.send((key, p)); }
                None => { self.pending.remove(&key); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::SourceId;

    #[test]
    fn a_fast_scroll_keeps_the_newest_requests() {
        let mut stack: Vec<(Key, PathBuf)> = (0..60u64).map(|i| ((SourceId(1), i * 250), PathBuf::from("a.mp4"))).collect();
        let dropped = trim(&mut stack);
        assert_eq!(stack.len(), QUEUE);
        assert_eq!(dropped.len(), 12, "the oldest twelve are dropped and reported");
        assert_eq!(dropped[0].1, 0);
        assert_eq!(stack.last().unwrap().0 .1, 59 * 250, "the newest is decoded first (popped from the end)");
        let mut small: Vec<(Key, PathBuf)> = vec![((SourceId(1), 0), PathBuf::from("a"))];
        assert!(trim(&mut small).is_empty());
    }
}
