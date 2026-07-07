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

/// A way a prompt's image markers can disagree with the images actually attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementError {
    /// A marker (`<image:N>` or the N-th bare `<image>`) points at an image that wasn't attached.
    MarkerOutOfRange { marker: usize, attached: usize },
    /// An attached image was never referenced by any marker, so it would be silently dropped.
    UnplacedImage(usize),
}

/// Check a parsed prompt places its images sanely against `n_attached` actual attachments: every marker
/// must point at an attached image (`marker < n_attached`), and every attached image must be referenced
/// at least once (else the engine would build embeddings for a picture the prompt never mentions).
///
/// Out-of-range markers are reported before unplaced images (a dangling marker is the more urgent bug —
/// it would index a vision embedding that doesn't exist). Returns the first problem, or `Ok(())` when the
/// placement is complete and in range. Duplicate placement of the same image is allowed (some prompts
/// legitimately refer back to an image), so it isn't an error here.
pub fn validate_placement(segments: &[Segment], n_attached: usize) -> Result<(), PlacementError> {
    let used = image_indices(segments);
    if let Some(&m) = used.iter().find(|&&i| i >= n_attached) {
        return Err(PlacementError::MarkerOutOfRange { marker: m, attached: n_attached });
    }
    for i in 0..n_attached {
        if !used.contains(&i) { return Err(PlacementError::UnplacedImage(i)); }
    }
    Ok(())
}

/// The target pixel size an image is resized to before tiling: `(cols*tile, rows*tile)`. The vision
/// preprocessor resizes/pads the source image to this, then slices it into the `cols×rows` grid of
/// `cfg.tile`-px tiles that [`tile_grid`] chose. (The optional thumbnail is a separate whole-image tile
/// downscaled to `cfg.tile`², so it isn't part of these hi-res dims.)
pub fn preprocess_dims(w: u32, h: u32, cfg: &VisionConfig) -> (u32, u32) {
    let (c, r) = tile_grid(w, h, cfg);
    (c * cfg.tile, r * cfg.tile)
}

/// A single hi-res tile's crop rectangle, in RESIZED-image pixel space (after the source is scaled to
/// [`ImagePlan::resize_to`]). Each tile is `cfg.tile`² and the grid tiles evenly, so these never overlap
/// and exactly cover the resized image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileBox { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }

/// The complete plan for turning one source image into vision tokens: resize target, tile grid, the
/// row-major hi-res tile crop boxes, whether a whole-image thumbnail is also encoded, and the total token
/// cost. This is the single hand-off to the pixel-resampler (the IO seam): scale the source to `resize_to`,
/// crop each [`TileBox`], (optionally) also downscale the whole image to one `cfg.tile²` thumbnail, run the
/// vision encoder on each, and you have `tokens` embeddings to drop into the image slots [`assemble`] made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImagePlan {
    pub resize_to: (u32, u32),
    pub grid: (u32, u32),      // (cols, rows)
    pub tiles: Vec<TileBox>,   // row-major, len == cols*rows
    pub thumbnail: bool,
    pub tokens: u32,
}

/// Build the [`ImagePlan`] for a `w`×`h` source under `cfg`. Bundles [`tile_grid`], [`preprocess_dims`],
/// and [`image_tokens`] and adds the concrete per-tile crop boxes so the resampler doesn't recompute them.
pub fn plan_image(w: u32, h: u32, cfg: &VisionConfig) -> ImagePlan {
    let (cols, rows) = tile_grid(w, h, cfg);
    let tile = cfg.tile.max(1);
    let mut tiles = Vec::with_capacity((cols * rows) as usize);
    for r in 0..rows {
        for c in 0..cols {
            tiles.push(TileBox { x: c * tile, y: r * tile, w: tile, h: tile });
        }
    }
    ImagePlan {
        resize_to: (cols * tile, rows * tile),
        grid: (cols, rows),
        tiles,
        thumbnail: cfg.thumbnail,
        tokens: image_tokens(w, h, cfg),
    }
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

/// An image payload lifted out of an inline `data:` URL: the declared MIME subtype (`png`, `jpeg`, …)
/// and the raw base64 body (still encoded — decoding is the IO seam, like pixel resampling).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataUrlImage { pub mime: String, pub base64: String }

/// Whether a byte is part of a base64 body (standard + URL-safe alphabets, plus `=` padding).
fn is_b64(c: char) -> bool { c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '-' | '_' | '=') }

/// Pull inline `data:image/<subtype>;base64,<data>` URLs out of a prompt, replacing each with a numbered
/// `<image:N>` marker so a pasted image flows through the same segment/assemble path as an attached one —
/// instead of being tokenized as a giant base64 text blob (thousands of junk tokens that blow the context).
///
/// `offset` is how many images are already attached before these; the first extracted image becomes
/// `<image:offset>`, the next `<image:offset+1>`, and so on, so the markers line up with the caller's
/// combined attachment list. Only `;base64,` data URLs are handled (the only form that carries pixels);
/// a `data:` URL that isn't an image, or isn't base64, is left as literal text. Returns the rewritten
/// prompt and the extracted images in first-seen order.
pub fn extract_data_urls(prompt: &str, offset: usize) -> (String, Vec<DataUrlImage>) {
    const MARK: &str = "data:image/";
    let mut out = String::with_capacity(prompt.len());
    let mut images = Vec::new();
    let mut rest = prompt;
    while let Some(pos) = rest.find(MARK) {
        let after = &rest[pos + MARK.len()..];
        // subtype runs up to the `;`; then require the exact `base64,` token.
        let parsed = after.find(';').and_then(|semi| {
            let mime = &after[..semi];
            let tail = &after[semi + 1..];
            let body = tail.strip_prefix("base64,")?;
            // MIME subtype must be a simple token (no spaces / weird chars) to count as a real data URL.
            if mime.is_empty() || !mime.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
                return None;
            }
            let b64: String = body.chars().take_while(|&c| is_b64(c)).collect();
            if b64.is_empty() { return None; }
            let consumed = MARK.len() + semi + 1 + "base64,".len() + b64.len();
            Some((mime.to_string(), b64, consumed))
        });
        match parsed {
            Some((mime, b64, consumed)) => {
                out.push_str(&rest[..pos]);
                out.push_str(&format!("<image:{}>", offset + images.len()));
                images.push(DataUrlImage { mime, base64: b64 });
                rest = &rest[pos + consumed..];
            }
            None => {
                // not a well-formed base64 image data URL — keep the marker text literal, advance past it.
                let keep = pos + MARK.len();
                out.push_str(&rest[..keep]);
                rest = &rest[keep..];
            }
        }
    }
    out.push_str(rest);
    (out, images)
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

    #[test]
    fn validate_placement_accepts_complete_in_range() {
        let segs = segment_prompt("a <image> b <image>");        // places 0 and 1
        assert_eq!(validate_placement(&segs, 2), Ok(()));
        // duplicate reference to the same image is allowed
        let segs = segment_prompt("<image:0> and again <image:0>");
        assert_eq!(validate_placement(&segs, 1), Ok(()));
    }

    #[test]
    fn validate_placement_flags_out_of_range_before_unplaced() {
        // marker points past the single attachment → out-of-range (reported before any unplaced check)
        let segs = segment_prompt("see <image:5>");
        assert_eq!(validate_placement(&segs, 1), Err(PlacementError::MarkerOutOfRange { marker: 5, attached: 1 }));
        // no markers, but two images attached → first unplaced image is flagged
        let segs = segment_prompt("no markers here");
        assert_eq!(validate_placement(&segs, 2), Err(PlacementError::UnplacedImage(0)));
        // image 0 placed, image 1 attached but never referenced → unplaced(1)
        let segs = segment_prompt("only <image:0>");
        assert_eq!(validate_placement(&segs, 2), Err(PlacementError::UnplacedImage(1)));
        // zero attachments + a marker → out of range
        let segs = segment_prompt("<image>");
        assert_eq!(validate_placement(&segs, 0), Err(PlacementError::MarkerOutOfRange { marker: 0, attached: 0 }));
    }

    #[test]
    fn preprocess_dims_are_the_tile_grid_in_pixels() {
        let cfg = VisionConfig { tile: 336, patch: 14, max_tiles: 4, thumbnail: true };
        assert_eq!(preprocess_dims(1600, 400, &cfg), (4 * 336, 1 * 336)); // wide → 4×1 tiles
        assert_eq!(preprocess_dims(512, 512, &cfg), (2 * 336, 2 * 336));  // square → 2×2 tiles
    }

    #[test]
    fn extract_data_urls_replaces_with_markers() {
        let (clean, imgs) = extract_data_urls("look at data:image/png;base64,iVBORw0KGgo= please", 0);
        assert_eq!(clean, "look at <image:0> please");
        assert_eq!(imgs, vec![DataUrlImage { mime: "png".into(), base64: "iVBORw0KGgo=".into() }]);
        // the rewritten prompt now flows through the normal marker path
        let segs = segment_prompt(&clean);
        assert_eq!(image_indices(&segs), vec![0]);
        assert_eq!(validate_placement(&segs, 1), Ok(()));
    }

    #[test]
    fn extract_data_urls_numbers_from_offset_and_handles_multiple() {
        // two pasted images, with one image already attached before them (offset = 1)
        let p = "a data:image/jpeg;base64,AAAA b data:image/webp;base64,BBBB";
        let (clean, imgs) = extract_data_urls(p, 1);
        assert_eq!(clean, "a <image:1> b <image:2>");
        assert_eq!(imgs.len(), 2);
        assert_eq!(imgs[0].mime, "jpeg");
        assert_eq!(imgs[1].mime, "webp");
        assert_eq!(imgs[1].base64, "BBBB");
    }

    #[test]
    fn extract_data_urls_leaves_non_image_and_malformed_alone() {
        // non-image data URL is untouched
        let (clean, imgs) = extract_data_urls("data:text/plain;base64,SGk=", 0);
        assert_eq!(clean, "data:text/plain;base64,SGk=");
        assert!(imgs.is_empty());
        // an image data URL that isn't base64 (URL-encoded) is left literal
        let (clean, imgs) = extract_data_urls("data:image/svg+xml,<svg/>", 0);
        assert_eq!(clean, "data:image/svg+xml,<svg/>");
        assert!(imgs.is_empty());
        // plain text with no data URL is unchanged
        assert_eq!(extract_data_urls("no images here", 0).0, "no images here");
    }

    #[test]
    fn extract_data_urls_stops_base64_at_delimiter() {
        // the base64 body ends at the first non-base64 char (a space here), not swallowing the trailing word
        let (clean, imgs) = extract_data_urls("data:image/png;base64,QUJD then text", 0);
        assert_eq!(clean, "<image:0> then text");
        assert_eq!(imgs[0].base64, "QUJD");
    }

    #[test]
    fn plan_image_tiles_cover_the_resized_image() {
        let cfg = VisionConfig { tile: 336, patch: 14, max_tiles: 4, thumbnail: true };
        let plan = plan_image(512, 512, &cfg); // square → 2×2
        assert_eq!(plan.grid, (2, 2));
        assert_eq!(plan.resize_to, (672, 672));
        assert_eq!(plan.tiles.len(), 4);
        // row-major order, evenly tiled, no gaps/overlap
        assert_eq!(plan.tiles[0], TileBox { x: 0, y: 0, w: 336, h: 336 });
        assert_eq!(plan.tiles[1], TileBox { x: 336, y: 0, w: 336, h: 336 });
        assert_eq!(plan.tiles[2], TileBox { x: 0, y: 336, w: 336, h: 336 });
        assert_eq!(plan.tiles[3], TileBox { x: 336, y: 336, w: 336, h: 336 });
        // union of tile areas == resized area
        let area: u32 = plan.tiles.iter().map(|t| t.w * t.h).sum();
        assert_eq!(area, plan.resize_to.0 * plan.resize_to.1);
        // tokens match image_tokens (4 tiles + thumbnail) × 576
        assert_eq!(plan.tokens, image_tokens(512, 512, &cfg));
        assert_eq!(plan.tokens, 5 * 576);
    }

    #[test]
    fn plan_image_wide_is_a_single_row() {
        let cfg = VisionConfig { tile: 336, patch: 14, max_tiles: 4, thumbnail: false };
        let plan = plan_image(1600, 400, &cfg); // wide → 4×1
        assert_eq!(plan.grid, (4, 1));
        assert!(plan.tiles.iter().all(|t| t.y == 0));
        assert_eq!(plan.tiles.iter().map(|t| t.x).collect::<Vec<_>>(), vec![0, 336, 672, 1008]);
        assert!(!plan.thumbnail);
        assert_eq!(plan.tokens, 4 * 576);
    }
}
