//! textures — thumbnails on the GPU, with a ceiling.
//!
//! A 256×256 RGBA thumbnail is 256 KiB. Two thousand photos would be half
//! a gigabyte of texture memory, which is not a number a phone survives,
//! so this keeps the ones drawn recently and lets the rest go. The CPU
//! copy the scanner delivered is dropped after upload for the same reason;
//! an evicted thumbnail comes back from the disk cache, which is a
//! millisecond-sized PNG decode, when the grid scrolls to it again.
//!
//! The policy is in one place and testable: `touch` marks a use,
//! `evict` keeps the [`CAP`] most recently used. Nothing else here
//! knows about eviction.

use crate::library::{Photo, Thumb};
use std::collections::HashMap;
use std::path::Path;

/// Thumbnails kept on the GPU. Three screens of a dense desktop grid,
/// or thirty of a phone's.
pub const CAP: usize = 400;

pub struct Textures {
    map: HashMap<String, (egui::TextureHandle, u64)>,
    frame: u64,
    /// Where evicted thumbnails come back from.
    cache_dir: std::path::PathBuf,
}

impl Textures {
    pub fn new(cache_dir: std::path::PathBuf) -> Textures {
        Textures { map: HashMap::new(), frame: 0, cache_dir }
    }

    pub fn len(&self) -> usize { self.map.len() }
    pub fn is_empty(&self) -> bool { self.map.is_empty() }

    /// Call once per frame, before drawing.
    pub fn begin_frame(&mut self) { self.frame += 1; }

    /// The thumbnail texture for a photo, uploading it if needed. `None`
    /// when there is no thumbnail anywhere yet (the scanner has not got
    /// to it), and the caller draws a placeholder.
    pub fn get(&mut self, ctx: &egui::Context, photo: &mut Photo) -> Option<egui::TextureHandle> {
        if let Some((tex, used)) = self.map.get_mut(&photo.path) {
            *used = self.frame;
            return Some(tex.clone());
        }
        let thumb = match photo.thumb.take() {
            Some(t) => t,
            None => cached(&self.cache_dir, &photo.path)?,
        };
        let img = egui::ColorImage::from_rgba_unmultiplied([thumb.w, thumb.h], &thumb.rgba);
        let tex = ctx.load_texture(format!("thumb:{}", photo.path), img, egui::TextureOptions::LINEAR);
        self.map.insert(photo.path.clone(), (tex.clone(), self.frame));
        Some(tex)
    }

    /// Forget a photo's texture (it was deleted).
    pub fn drop_photo(&mut self, path: &str) { self.map.remove(path); }

    /// Let go of everything not drawn recently, down to `cap`.
    pub fn evict(&mut self, cap: usize) {
        if self.map.len() <= cap { return; }
        let mut by_use: Vec<(u64, String)> = self.map.iter().map(|(k, (_, u))| (*u, k.clone())).collect();
        by_use.sort();
        let drop_n = self.map.len() - cap;
        for (_, k) in by_use.into_iter().take(drop_n) { self.map.remove(&k); }
    }

    /// The eviction order, oldest first, for tests.
    #[cfg(test)]
    fn oldest(&self) -> Vec<String> {
        let mut v: Vec<(u64, String)> = self.map.iter().map(|(k, (_, u))| (*u, k.clone())).collect();
        v.sort();
        v.into_iter().map(|(_, k)| k).collect()
    }
}

fn cached(cache: &Path, photo_path: &str) -> Option<Thumb> {
    let p = crate::scan::thumb_cache_path(cache, photo_path);
    let img = image::open(&p).ok()?.to_rgba8();
    Some(Thumb { w: img.width() as usize, h: img.height() as usize, rgba: img.into_raw() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(path: &str) -> Photo {
        let mut p = crate::library::tests::photo(path, 0, &[7u8; 16], 4, 4, &[]);
        p.thumb = Some(Thumb { w: 2, h: 2, rgba: vec![255; 16] });
        p
    }

    #[test]
    fn the_least_recently_drawn_go_first_and_the_cpu_copy_is_dropped_on_upload() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let mut t = Textures::new(dir.path().to_path_buf());
        let mut a = photo("/p/a");
        let mut b = photo("/p/b");
        let mut c = photo("/p/c");
        t.begin_frame();
        assert!(t.get(&ctx, &mut a).is_some());
        assert!(a.thumb.is_none(), "the CPU copy is released once it is on the GPU");
        t.get(&ctx, &mut b);
        t.begin_frame();
        t.get(&ctx, &mut c);
        t.get(&ctx, &mut a); // a is used again: b is now the oldest
        assert_eq!(t.oldest()[0], "/p/b");
        t.evict(2);
        assert_eq!(t.len(), 2);
        assert!(t.map.contains_key("/p/a") && t.map.contains_key("/p/c"));
    }

    #[test]
    fn a_photo_with_no_thumbnail_anywhere_yields_none_not_a_blank() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let mut t = Textures::new(dir.path().to_path_buf());
        let mut p = crate::library::tests::photo("/p/none", 0, &[7u8; 16], 4, 4, &[]);
        t.begin_frame();
        assert!(t.get(&ctx, &mut p).is_none());
        assert!(t.is_empty());
    }
}
//! textures — thumbnails on the GPU, with a ceiling.
//!
//! A 256×256 RGBA thumbnail is 256 KiB. Two thousand photos would be half
//! a gigabyte of texture memory, which is not a number a phone survives,
//! so this keeps the ones drawn recently and lets the rest go. The CPU
//! copy the scanner delivered is dropped after upload for the same reason;
//! an evicted thumbnail comes back from the disk cache, which is a
//! millisecond-sized PNG decode, when the grid scrolls to it again.
//!
//! The policy is in one place and testable: `touch` marks a use,
//! `evict` keeps the [`CAP`] most recently used. Nothing else here
//! knows about eviction.

use crate::library::{Photo, Thumb};
use std::collections::HashMap;
use std::path::Path;

/// Thumbnails kept on the GPU. Three screens of a dense desktop grid,
/// or thirty of a phone's.
pub const CAP: usize = 400;

pub struct Textures {
    map: HashMap<String, (egui::TextureHandle, u64)>,
    frame: u64,
    /// Where evicted thumbnails come back from.
    cache_dir: std::path::PathBuf,
}

impl Textures {
    pub fn new(cache_dir: std::path::PathBuf) -> Textures {
        Textures { map: HashMap::new(), frame: 0, cache_dir }
    }

    pub fn len(&self) -> usize { self.map.len() }
    pub fn is_empty(&self) -> bool { self.map.is_empty() }

    /// Call once per frame, before drawing.
    pub fn begin_frame(&mut self) { self.frame += 1; }

    /// The thumbnail texture for a photo, uploading it if needed. `None`
    /// when there is no thumbnail anywhere yet (the scanner has not got
    /// to it), and the caller draws a placeholder.
    pub fn get(&mut self, ctx: &egui::Context, photo: &mut Photo) -> Option<egui::TextureHandle> {
        if let Some((tex, used)) = self.map.get_mut(&photo.path) {
            *used = self.frame;
            return Some(tex.clone());
        }
        let thumb = match photo.thumb.take() {
            Some(t) => t,
            None => cached(&self.cache_dir, &photo.path)?,
        };
        let img = egui::ColorImage::from_rgba_unmultiplied([thumb.w, thumb.h], &thumb.rgba);
        let tex = ctx.load_texture(format!("thumb:{}", photo.path), img, egui::TextureOptions::LINEAR);
        self.map.insert(photo.path.clone(), (tex.clone(), self.frame));
        Some(tex)
    }

    /// Forget a photo's texture (it was deleted).
    pub fn drop_photo(&mut self, path: &str) { self.map.remove(path); }

    /// Let go of everything not drawn recently, down to `cap`.
    pub fn evict(&mut self, cap: usize) {
        if self.map.len() <= cap { return; }
        let mut by_use: Vec<(u64, String)> = self.map.iter().map(|(k, (_, u))| (*u, k.clone())).collect();
        by_use.sort();
        let drop_n = self.map.len() - cap;
        for (_, k) in by_use.into_iter().take(drop_n) { self.map.remove(&k); }
    }

    /// The eviction order, oldest first, for tests.
    #[cfg(test)]
    fn oldest(&self) -> Vec<String> {
        let mut v: Vec<(u64, String)> = self.map.iter().map(|(k, (_, u))| (*u, k.clone())).collect();
        v.sort();
        v.into_iter().map(|(_, k)| k).collect()
    }
}

fn cached(cache: &Path, photo_path: &str) -> Option<Thumb> {
    let p = crate::scan::thumb_cache_path(cache, photo_path);
    let img = image::open(&p).ok()?.to_rgba8();
    Some(Thumb { w: img.width() as usize, h: img.height() as usize, rgba: img.into_raw() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(path: &str) -> Photo {
        let mut p = crate::library::tests::photo(path, 0, &[7u8; 16], 4, 4, &[]);
        p.thumb = Some(Thumb { w: 2, h: 2, rgba: vec![255; 16] });
        p
    }

    #[test]
    fn the_least_recently_drawn_go_first_and_the_cpu_copy_is_dropped_on_upload() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let mut t = Textures::new(dir.path().to_path_buf());
        let mut a = photo("/p/a");
        let mut b = photo("/p/b");
        let mut c = photo("/p/c");
        t.begin_frame();
        assert!(t.get(&ctx, &mut a).is_some());
        assert!(a.thumb.is_none(), "the CPU copy is released once it is on the GPU");
        t.get(&ctx, &mut b);
        t.begin_frame();
        t.get(&ctx, &mut c);
        t.get(&ctx, &mut a); // a is used again: b is now the oldest
        assert_eq!(t.oldest()[0], "/p/b");
        t.evict(2);
        assert_eq!(t.len(), 2);
        assert!(t.map.contains_key("/p/a") && t.map.contains_key("/p/c"));
    }

    #[test]
    fn a_photo_with_no_thumbnail_anywhere_yields_none_not_a_blank() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let mut t = Textures::new(dir.path().to_path_buf());
        let mut p = crate::library::tests::photo("/p/none", 0, &[7u8; 16], 4, 4, &[]);
        t.begin_frame();
        assert!(t.get(&ctx, &mut p).is_none());
        assert!(t.is_empty());
    }
}
