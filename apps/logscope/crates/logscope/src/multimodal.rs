//! multimodal.rs — pure geometry + sequence assembly for vision-language input.
//!
//! Two pure pieces the engine needs to feed images to a language model: (1) how a source image tiles into
//! vision patches and how many tokens it costs (LLaVA / AnyRes-style dynamic tiling), and (2) how the
//! image placeholders in a prompt expand into vision-token *slots* interleaved with text tokens. The
//! vision-encoder forward pass (pixels → patch embeddings) and the pixel resampling are the GPU/IO seam;
//! the layout math that decides how many slots go where — and which the engine must fill with image
//! embeddings vs. token embeddings — is here and unit-tested.
#![allow(dead_code)]

/// Vision preprocessing config. Defaults are a common CLIP-ViT/LLaVA setup: 336px tiles, 14px patches
/// (→ 24×24 = 576 tokens/tile), up to 4 hi-res tiles plus a whole-image thumbnail.
#[derive(Clone, Copy, Debug)]
pub struct VisionConfig {
    pub tile: u32,       // square tile edge, px
    pub patch: u32,      // patch edge, px
    pub max_tiles: u32,  // cap on hi-res tiles
    pub thumbnail: bool, // also include one downscaled whole-image tile
}
impl Default for VisionConfig {
    fn default() -> Self { Self { tile: 336, patch: 14, max_tiles: 4, thumbnail: true } }
}

/// Vision tokens produced per tile = (tile/patch)².
pub fn tokens_per_tile(cfg: &VisionConfig) -> u32 {
    let p = (cfg.tile / cfg.patch.max(1)).max(1);
    p * p
}

/// Choose the tile grid `(cols, rows)` whose aspect ratio best matches the image, within `max_tiles`.
/// Ties prefer more tiles (higher effective resolution).
pub fn tile_grid(w: u32, h: u32, cfg: &VisionConfig) -> (u32, u32) {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let target = w / h;
    let mut best = (1u32, 1u32);
    let mut best_score = f32::INFINITY;
    for c in 1..=cfg.max_tiles.max(1) {
        for r in 1..=cfg.max_tiles.max(1) {
            if c * r > cfg.max_tiles.max(1) { continue; }
            let aspect_err = ((c as f32 / r as f32) / target).ln().abs(); // symmetric ratio distance
            let score = aspect_err - 0.01 * (c * r) as f32;               // tie-break toward more tiles
            if score < best_score { best_score = score; best = (c, r); }
        }
    }
    best
}

/// Total vision tokens an image costs: the hi-res tiles plus an optional thumbnail tile.
pub fn image_tokens(w: u32, h: u32, cfg: &VisionConfig) -> u32 {
    let (c, r) = tile_grid(w, h, cfg);
    let per = tokens_per_tile(cfg);
    c * r * per + if cfg.thumbnail { per } else { 0 }
}

/// A prompt piece before image expansion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece { Text(Vec<u32>), Image(usize) } // Image(i) marks where the i-th attached image goes

/// A position in the assembled multimodal sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot { Text(u32), Image { image: usize, idx: u32 } } // idx = which vision token of that image

/// Expand each `Image(i)` marker into `image_tokens[i]` vision slots, interleaved with the text tokens, to
/// produce the flat sequence the engine builds embeddings for: text slots get token embeddings, image
/// slots get `image[i]`'s vision embedding at `idx`. A marker with no matching count expands to nothing.
pub fn assemble(pieces: &[Piece], image_tokens: &[u32]) -> Vec<Slot> {
    let mut out = Vec::new();
    for p in pieces {
        match p {
            Piece::Text(ids) => out.extend(ids.iter().map(|&t| Slot::Text(t))),
            Piece::Image(i) => {
                let n = image_tokens.get(*i).copied().unwrap_or(0);
                out.extend((0..n).map(|idx| Slot::Image { image: *i, idx }));
            }
        }
    }
    out
}

/// `(text_slots, image_slots)` in an assembled sequence.
pub fn counts(slots: &[Slot]) -> (usize, usize) {
    let img = slots.iter().filter(|s| matches!(s, Slot::Image { .. })).count();
    (slots.len() - img, img)
}

/// Whether the assembled sequence fits a context window.
pub fn fits(pieces: &[Piece], image_tokens: &[u32], n_ctx: usize) -> bool {
    let text: usize = pieces.iter().map(|p| if let Piece::Text(t) = p { t.len() } else { 0 }).sum();
    let imgs: usize = pieces.iter().filter_map(|p| if let Piece::Image(i) = p { image_tokens.get(*i).map(|n| *n as usize) } else { None }).sum();
    text + imgs <= n_ctx
}

/// A parsed chat-string segment before tokenization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment { Text(String), Image(usize) }

/// Split a chat string into text/image segments at `<image>` / `<image:N>` markers. A bare `<image>` is
/// auto-numbered in order (0, 1, 2, …); `<image:N>` pins an explicit index (and does not advance the auto
/// counter). Anything that isn't a well-formed marker (`<imagex>`, an unterminated `<image`) is kept as
/// literal text. Adjacent text is coalesced.
pub fn segment_prompt(s: &str) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut buf = String::new();
    let mut auto = 0usize;
    let mut rest = s;
    while !rest.is_empty() {
        let Some(pos) = rest.find("<image") else { buf.push_str(rest); break; };
        buf.push_str(&rest[..pos]);
        let tail = &rest[pos + "<image".len()..];
        let idx = tail.find('>').and_then(|close| {
            let inner = &tail[..close];
            if inner.is_empty() { let i = auto; auto += 1; Some((i, close)) }
            else { inner.strip_prefix(':').and_then(|n| n.trim().parse::<usize>().ok()).map(|n| (n, close)) }
        });
        match idx {
            Some((i, close)) => {
                if !buf.is_empty() { out.push(Segment::Text(std::mem::take(&mut buf))); }
                out.push(Segment::Image(i));
                rest = &tail[close + 1..];
            }
            None => { buf.push_str("<image"); rest = tail; } // not a marker → literal
        }
    }
    if !buf.is_empty() { out.push(Segment::Text(buf)); }
    out
}

/// The distinct image indices a prompt references, in first-seen order (use to check every attached image
/// is actually placed, and that no marker points past the attachments).
pub fn image_indices(segments: &[Segment]) -> Vec<usize> {
    let mut seen = Vec::new();
    for seg in segments { if let Segment::Image(i) = seg { if !seen.contains(i) { seen.push(*i); } } }
    seen
}

/// Parse a chat string and tokenize its text spans (via the injected tokenizer — the real tokenizer is the
/// seam) into the [`Piece`] stream that [`assemble`] consumes. This closes the multimodal input path:
/// chat string → `Piece`s → (`image_tokens` per image) → `assemble` → `Slot`s.
pub fn pieces_from_prompt(s: &str, tokenize: impl Fn(&str) -> Vec<u32>) -> Vec<Piece> {
    segment_prompt(s).into_iter().map(|seg| match seg {
        Segment::Text(t) => Piece::Text(tokenize(&t)),
        Segment::Image(i) => Piece::Image(i),
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_per_tile_is_patch_grid_squared() {
        assert_eq!(tokens_per_tile(&VisionConfig::default()), 24 * 24); // 336/14 = 24
    }

    #[test]
    fn tile_grid_follows_aspect_ratio() {
        let cfg = VisionConfig { tile: 336, patch: 14, max_tiles: 4, thumbnail: false };
        assert_eq!(tile_grid(1600, 400, &cfg), (4, 1)); // wide → all columns
        assert_eq!(tile_grid(400, 1600, &cfg), (1, 4)); // tall → all rows
        assert_eq!(tile_grid(512, 512, &cfg), (2, 2));  // square → 2×2 (more res beats 1×1 on the tie)
    }

    #[test]
    fn image_tokens_sum_tiles_and_thumbnail() {
        let cfg = VisionConfig { tile: 336, patch: 14, max_tiles: 4, thumbnail: true };
        // square → 2×2 tiles (=4) + 1 thumbnail = 5 tiles × 576
        assert_eq!(image_tokens(512, 512, &cfg), 5 * 576);
        let no_thumb = VisionConfig { thumbnail: false, ..cfg };
        assert_eq!(image_tokens(512, 512, &no_thumb), 4 * 576);
    }

    #[test]
    fn assemble_interleaves_image_slots_with_text() {
        let pieces = vec![Piece::Text(vec![1, 2]), Piece::Image(0), Piece::Text(vec![3])];
        let slots = assemble(&pieces, &[4]); // image 0 costs 4 vision tokens
        assert_eq!(slots.len(), 7);
        assert_eq!(slots[0], Slot::Text(1));
        assert_eq!(slots[2], Slot::Image { image: 0, idx: 0 });
        assert_eq!(slots[5], Slot::Image { image: 0, idx: 3 });
        assert_eq!(slots[6], Slot::Text(3));
        assert_eq!(counts(&slots), (3, 4));
    }

    #[test]
    fn multiple_images_and_missing_counts() {
        let pieces = vec![Piece::Image(0), Piece::Image(1), Piece::Image(2)];
        let slots = assemble(&pieces, &[2, 3]); // image 2 has no count → expands to nothing
        assert_eq!(counts(&slots), (0, 5));
        assert!(slots.iter().any(|s| matches!(s, Slot::Image { image: 1, .. })));
    }

    #[test]
    fn fits_accounts_for_image_tokens() {
        let pieces = vec![Piece::Text(vec![0; 100]), Piece::Image(0)];
        assert!(fits(&pieces, &[500], 1024));
        assert!(!fits(&pieces, &[2000], 1024)); // image blows the context
    }

    #[test]
    fn segment_prompt_auto_numbers_bare_markers() {
        let segs = segment_prompt("look at <image> and <image>");
        assert_eq!(segs, vec![
            Segment::Text("look at ".into()), Segment::Image(0),
            Segment::Text(" and ".into()), Segment::Image(1),
        ]);
    }

    #[test]
    fn segment_prompt_handles_explicit_indices_and_edges() {
        // explicit indices, marker at start, and back-to-back markers
        let segs = segment_prompt("<image:1><image:0>x");
        assert_eq!(segs, vec![Segment::Image(1), Segment::Image(0), Segment::Text("x".into())]);
        assert_eq!(image_indices(&segs), vec![1, 0]);
    }

    #[test]
    fn non_markers_stay_literal() {
        assert_eq!(segment_prompt("no images here"), vec![Segment::Text("no images here".into())]);
        // unterminated / malformed markers are preserved as text (coalesced)
        assert_eq!(segment_prompt("a <imagex> b"), vec![Segment::Text("a <imagex> b".into())]);
        assert_eq!(segment_prompt("trailing <image"), vec![Segment::Text("trailing <image".into())]);
    }

    #[test]
    fn prompt_adapter_feeds_assemble_end_to_end() {
        // a toy tokenizer: one id per whitespace word (the id = word length)
        let toks = |t: &str| t.split_whitespace().map(|w| w.len() as u32).collect::<Vec<_>>();
        let pieces = pieces_from_prompt("describe <image> please", toks);
        assert_eq!(pieces, vec![Piece::Text(vec![8]), Piece::Image(0), Piece::Text(vec![6])]);
        // chat string → pieces → slots, with the image costing 3 vision tokens
        let slots = assemble(&pieces, &[3]);
        assert_eq!(counts(&slots), (2, 3));
        assert_eq!(slots[1], Slot::Image { image: 0, idx: 0 });
    }
}
