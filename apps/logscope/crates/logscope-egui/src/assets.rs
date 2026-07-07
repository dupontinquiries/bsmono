//! assets.rs — the uuid-keyed store of generated media: the connective tissue between `convograph` (which
//! holds a node's name, keywords, and uuid) and what actually gets drawn (`imprint` mosaics on the pearl and
//! nodes, egui thumbnails, rendered clips).
//!
//! It's split deliberately: THIS module holds the pure, serializable DATA — name, prompt, the low-res mosaic
//! grid, the file path, dims, and generation status — all keyed by the same `u128` uuid the graph uses. The
//! heavy UI handles (an `egui::TextureHandle` for full-res, a clip player) live in a separate UI-side cache
//! keyed by the same uuid, so this stays testable without egui, the GPU, or a decoder.

use crate::imprint::Rgb;
use std::collections::HashMap;
use std::path::PathBuf;

/// Turn a generation prompt into a short, human-readable NAME for its node — the thing the chat refers to and
/// the fuzzy matcher indexes as the image's keyword surface. Reuses the graph's own keyword extractor (so the
/// name is drawn from the same content words retrieval will match on), keeps the first `max_words` in order.
pub fn slug(prompt: &str, max_words: usize) -> String {
    let words = crate::convograph::keywords(prompt);
    if words.is_empty() {
        return "untitled image".to_string();
    }
    words.into_iter().take(max_words.max(1)).collect::<Vec<_>>().join(" ")
}

/// Generation lifecycle, so the UI can show progress and gate what's drawable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Pending, // spawned (sd-cli / wan running), no pixels yet
    Ready,   // file written + decoded into a grid
    Failed,  // generation or decode failed
}

/// A generated (or attached) image. `grid` is the low-res mosaic [`imprint`] draws on the pearl/node; the
/// full-res texture is cached UI-side by `uuid`, not stored here.
#[derive(Clone, Debug)]
pub struct ImageAsset {
    pub uuid: u128,
    pub name: String,
    pub prompt: String,
    pub grid: Vec<Rgb>,
    pub grid_cols: usize,
    pub grid_rows: usize,
    pub path: PathBuf,
    pub dims: (u32, u32),
    pub status: Status,
}

impl ImageAsset {
    /// A freshly-spawned image: named + placed in the graph, generation in flight, no pixels yet.
    pub fn pending(uuid: u128, name: impl Into<String>, prompt: impl Into<String>, path: PathBuf) -> Self {
        ImageAsset {
            uuid,
            name: name.into(),
            prompt: prompt.into(),
            grid: Vec::new(),
            grid_cols: 0,
            grid_rows: 0,
            path,
            dims: (0, 0),
            status: Status::Pending,
        }
    }
}

/// A shot: the keyframe image it animates, its motion prompt, and the rendered clip (once `Ready`).
#[derive(Clone, Debug)]
pub struct ShotAsset {
    pub uuid: u128,
    pub keyframe: u128,
    pub motion_prompt: String,
    pub clip_path: Option<PathBuf>,
    pub frames: u32,
    pub fps: u32,
    pub status: Status,
}

/// The store. Two maps keyed by uuid; lookups are what the renderer and retrieval both go through.
#[derive(Default)]
pub struct AssetStore {
    images: HashMap<u128, ImageAsset>,
    shots: HashMap<u128, ShotAsset>,
}

impl AssetStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put_image(&mut self, a: ImageAsset) {
        self.images.insert(a.uuid, a);
    }
    pub fn put_shot(&mut self, s: ShotAsset) {
        self.shots.insert(s.uuid, s);
    }
    pub fn image(&self, uuid: u128) -> Option<&ImageAsset> {
        self.images.get(&uuid)
    }
    pub fn shot(&self, uuid: u128) -> Option<&ShotAsset> {
        self.shots.get(&uuid)
    }

    /// Promote a pending image to Ready with its decoded mosaic + full-res dims (called once generation and
    /// decode finish, on the UI thread). Returns false if the uuid isn't a known image.
    pub fn mark_image_ready(&mut self, uuid: u128, grid: Vec<Rgb>, cols: usize, rows: usize, dims: (u32, u32)) -> bool {
        match self.images.get_mut(&uuid) {
            Some(a) => {
                a.grid = grid;
                a.grid_cols = cols;
                a.grid_rows = rows;
                a.dims = dims;
                a.status = Status::Ready;
                true
            }
            None => false,
        }
    }

    /// Mark an image or shot as failed (generation or decode error), so the node greys out instead of hanging.
    pub fn mark_failed(&mut self, uuid: u128) -> bool {
        if let Some(a) = self.images.get_mut(&uuid) {
            a.status = Status::Failed;
            return true;
        }
        if let Some(s) = self.shots.get_mut(&uuid) {
            s.status = Status::Failed;
            return true;
        }
        false
    }

    /// The mosaic grid for a Ready image, if present — what the pearl/node imprint draws. `None` while Pending.
    pub fn grid(&self, uuid: u128) -> Option<(&[Rgb], usize, usize)> {
        self.images.get(&uuid).filter(|a| a.status == Status::Ready && !a.grid.is_empty())
            .map(|a| (a.grid.as_slice(), a.grid_cols, a.grid_rows))
    }

    /// All shot uuids that animate a given keyframe image (follows the keyframe edge in the store).
    pub fn shots_of(&self, keyframe: u128) -> Vec<u128> {
        let mut v: Vec<u128> = self.shots.values().filter(|s| s.keyframe == keyframe).map(|s| s.uuid).collect();
        v.sort_unstable();
        v
    }

    pub fn image_count(&self) -> usize {
        self.images.len()
    }
    pub fn ready_image_count(&self) -> usize {
        self.images.values().filter(|a| a.status == Status::Ready).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_names_from_content_words() {
        assert_eq!(slug("a photoreal spiral galaxy above jagged peaks at night", 3), "photoreal spiral galaxy");
        // stopwords/short words dropped, order preserved
        assert_eq!(slug("make me the harbor at dawn", 4), "harbor dawn");
        // empty / all-stopword prompt → stable fallback
        assert_eq!(slug("", 3), "untitled image");
        assert_eq!(slug("the a of to", 3), "untitled image");
    }

    fn img(uuid: u128, name: &str) -> ImageAsset {
        ImageAsset::pending(uuid, name, "a prompt", PathBuf::from(format!("/tmp/{uuid}.png")))
    }

    #[test]
    fn pending_then_ready_lifecycle() {
        let mut s = AssetStore::new();
        s.put_image(img(1, "cat"));
        assert_eq!(s.image(1).unwrap().status, Status::Pending);
        assert!(s.grid(1).is_none(), "no mosaic while pending");
        assert_eq!(s.ready_image_count(), 0);

        let ok = s.mark_image_ready(1, vec![[10, 20, 30]; 4], 2, 2, (1024, 1024));
        assert!(ok);
        assert_eq!(s.image(1).unwrap().status, Status::Ready);
        assert_eq!(s.grid(1).unwrap().1, 2, "cols exposed for the imprint");
        assert_eq!(s.ready_image_count(), 1);

        // unknown uuid can't be promoted
        assert!(!s.mark_image_ready(999, vec![], 0, 0, (0, 0)));
    }

    #[test]
    fn failed_greys_out() {
        let mut s = AssetStore::new();
        s.put_image(img(2, "broken"));
        assert!(s.mark_failed(2));
        assert_eq!(s.image(2).unwrap().status, Status::Failed);
        assert!(s.grid(2).is_none(), "failed image has no drawable grid");
    }

    #[test]
    fn shots_are_found_by_keyframe() {
        let mut s = AssetStore::new();
        s.put_image(img(1, "harbor"));
        s.put_shot(ShotAsset { uuid: 100, keyframe: 1, motion_prompt: "push in".into(), clip_path: None, frames: 48, fps: 16, status: Status::Pending });
        s.put_shot(ShotAsset { uuid: 101, keyframe: 1, motion_prompt: "pan left".into(), clip_path: None, frames: 48, fps: 16, status: Status::Pending });
        s.put_shot(ShotAsset { uuid: 102, keyframe: 7, motion_prompt: "unrelated".into(), clip_path: None, frames: 48, fps: 16, status: Status::Pending });
        assert_eq!(s.shots_of(1), vec![100, 101]);
        assert_eq!(s.shots_of(7), vec![102]);
        assert!(s.shots_of(999).is_empty());
    }
}
