//! Configuration management for RustyBoard
//!
//! Loads `config.toml` from `$XDG_CONFIG_HOME/rusty-board/`, falling back to
//! sensible defaults if the file is missing.

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub appearance: AppearanceConfig,
    pub behavior: BehaviorConfig,
    pub voice: VoiceConfig,
    pub gestures: GestureConfig,
    pub layout: LayoutConfig,
    pub llm: LlmConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    /// Theme: "auto", "light", "dark"
    pub theme: String,
    /// Accent color (hex without #)
    pub accent_color: String,
    /// Key corner radius in pixels
    pub key_radius: f64,
    /// Animation duration in milliseconds
    pub animation_ms: u32,
    /// Show key magnifier on press
    pub show_magnifier: bool,
    /// Keyboard opacity (0.0 - 1.0)
    pub opacity: f64,
    /// Use blur effect on background
    pub blur_background: bool,
    /// Font family
    pub font_family: String,
    /// Font size for keys (multiplier of key height, 0.0-1.0)
    pub font_size_ratio: f64,
    /// Customizable palette. When fields are empty strings, fall back to
    /// theme-derived defaults. Hex without `#`.
    pub colors: ColorPalette,
    /// When true, show the word-suggestions ribbon above the keyboard.
    /// Defaults to false because the suggestion engine isn't wired
    /// through to the keys yet — it's just a placeholder bar that
    /// wastes vertical space on phone.
    #[serde(default)]
    pub show_suggestions: bool,
    /// Glass effect: reduce keyboard opacity + apply per-row refraction
    /// gradient so what's behind the window (wallpaper, target app)
    /// shows through. Doesn't actually capture screen content (Wayland
    /// forbids that without compositor cooperation), but visually
    /// suggests material translucency. Default off — solid keyboard
    /// is more legible for typing.
    #[serde(default)]
    pub glass_effect: bool,
    /// Debug aid: render the swipe trail differently per gesture
    /// category (dotted = cardinal, striped = swipe-word, desaturated
    /// = normal/ignored, solid = other). Lets you SEE on-device which
    /// dispatch branch a gesture took. Default off — normal users
    /// want a clean solid accent trail.
    #[serde(default)]
    pub debug_trail_styling: bool,
}

/// User-customizable color palette. Each field is a hex string (no `#`).
/// Empty string means "use the theme default". Persisted to config.toml
/// so user color choices survive restarts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorPalette {
    /// Keyboard background (the area behind keys)
    pub background: String,
    /// Resting key fill
    pub key_fill: String,
    /// Pressed key fill
    pub key_fill_pressed: String,
    /// Key label color
    pub key_text: String,
    /// Modifier key fill (shift, settings, etc.)
    pub modifier_fill: String,
    /// Special key fill (space, return, backspace)
    pub special_fill: String,
    /// Accent / highlight (selected toggles, focus rings)
    pub accent: String,
    /// Swipe trail color
    pub trail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorConfig {
    /// Auto-capitalize first letter of sentences
    pub auto_capitalize: bool,
    /// Auto-correct typos
    pub auto_correct: bool,
    /// Predict next word
    pub word_prediction: bool,
    /// Vibrate on key press
    pub haptic_feedback: bool,
    /// Play sound on key press
    pub sound_feedback: bool,
    /// Long-press duration in ms
    pub long_press_ms: u32,
    /// Double-tap window in ms
    pub double_tap_ms: u32,
    /// Whether keys auto-repeat when held down. When false, holding a
    /// key emits exactly one press regardless of duration; the user
    /// has to lift and re-press for a second emission. Useful for
    /// people who often hold keys longer than intended.
    #[serde(default = "default_true")]
    pub key_repeat: bool,
    /// Initial repeat delay in ms
    pub repeat_delay_ms: u32,
    /// Repeat interval in ms
    pub repeat_interval_ms: u32,
    /// Auto-hide the keyboard when no editable widget is focused. Only
    /// effective on Wayland with a compositor that supports
    /// `zwp_input_method_v2`. When `Some(true)` (default), the keyboard
    /// hides on Deactivate and shows on Activate. `Some(false)` keeps
    /// it always visible. `None` is treated as `Some(true)`.
    #[serde(default)]
    pub auto_hide_on_no_focus: Option<bool>,
    /// When true, focus-in shows the compact arrow-keys tray
    /// instead of the full keyboard. The user taps the "ABC" key
    /// in the tray to summon the full keyboard. Helps users who
    /// don't want a full keyboard auto-popping every time they
    /// touch a text field. Default: false (most users want the
    /// full keyboard on focus; tray mode is opt-in for users who
    /// prefer minimal-by-default).
    #[serde(default)]
    pub focus_show_tray: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// Enable voice input
    pub enabled: bool,
    /// Path to whisper model. `None` triggers the auto-search:
    ///
    ///   1. `$WHISPER_MODELS_DIR` env override
    ///   2. `~/models/` — unified user-models directory (same
    ///      convention as llama.cpp, ollama, jan.ai, etc.; drop
    ///      a `.gguf` or `.bin` file here once and every tool
    ///      finds it)
    ///   3. `~/.local/share/rusty-board/models/`
    ///   4. `~/.config/web2qml/models/`
    ///   5. `~/.local/share/whisper/models/`
    ///   6. `/usr/share/{rusty-board,whisper}/models/`
    ///
    /// Quantized variants (`-q5_1` / `-q5_0`) are preferred over
    /// full-precision because they're 2-4x smaller with negligible
    /// WER loss. `.gguf` is preferred over `.bin` in the same dir
    /// because it's the modern unified format; both load via
    /// whisper.cpp's auto-detection from the file header.
    ///
    /// Set this explicitly only to override the search — for example
    /// to use a non-standard quantization or a fine-tuned model.
    pub model_path: Option<PathBuf>,
    /// Language code (e.g., "en", "auto")
    pub language: String,
    /// Sample rate (Hz)
    pub sample_rate: u32,
    /// Streaming partial results
    pub streaming: bool,
    /// How often the streaming-partial worker runs inference, in ms.
    /// Each iteration transcribes the recording-so-far and reports
    /// the result as a partial. Lower = more responsive UI but
    /// higher CPU. 600ms is a balance — most utterances last 1-3s,
    /// so the user sees 2-5 partials before finalizing. On weak
    /// hardware (single-core phone), bump to 1000+.
    pub streaming_interval_ms: u32,
    /// Whether to log each completed voice transcript to
    /// `~/.local/share/rusty-board/voice-log.jsonl`. Off by default
    /// for privacy. Useful for debugging misrecognitions or
    /// recovering an accidentally-undone transcript via /history.
    #[serde(default)]
    pub log_transcripts: bool,
    /// Min audio length to transcribe (ms)
    pub min_audio_ms: u32,
    /// Number of threads for whisper
    pub threads: i32,
}

/// LLM configuration. Used by the LLM bridge for synonyms, undo
/// corrections, dictation post-processing, and free-form prompts
/// from the microterm or voice key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    /// Enable LLM features (synonyms, prompts, post-processing).
    /// When false the LLM bridge is not spawned and the LLM key /
    /// gestures are inert.
    pub enabled: bool,
    /// HTTP endpoint of an OpenAI-compatible chat completions API.
    /// Examples:
    ///   - ollama:       http://localhost:11434/v1/chat/completions
    ///   - llama.cpp:    http://localhost:8080/v1/chat/completions
    ///   - vLLM / TGI:   http://localhost:8000/v1/chat/completions
    /// The default points at a local ollama install, which is the
    /// most common path for phone users running a self-hosted model.
    pub endpoint: String,
    /// Model name to request. For ollama this is the local tag
    /// (e.g. "llama3.2:1b", "gemma2:2b"). For OpenAI-compatible
    /// servers it's typically the served model name.
    pub model: String,
    /// API key for endpoints that require auth. Sent as
    /// `Authorization: Bearer <key>` if non-empty.
    pub api_key: String,
    /// System prompt prepended to every conversation. Tunes the
    /// model's behaviour for keyboard tasks: concise outputs,
    /// no markdown, no explanation prose.
    pub system_prompt: String,
    /// Request timeout in milliseconds. Phones with weak LLMs can
    /// take 5-10s for a short completion; default 15s gives some
    /// headroom.
    pub timeout_ms: u32,
    /// Maximum tokens to request per completion. Keyboard tasks
    /// (synonym, brief rewrite) need few tokens; default 128 is
    /// fast and avoids long-running blocking inferences.
    pub max_tokens: u32,
    /// Sampling temperature. Lower = more deterministic. 0.3 is
    /// a reasonable default for "rewrite / find a synonym" tasks
    /// where we don't want creativity.
    pub temperature: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GestureConfig {
    /// Enable swipe typing
    pub swipe_typing: bool,
    /// Enable spacebar cursor swipe
    pub spacebar_cursor: bool,
    /// Enable shift swipe gestures
    pub shift_swipe: bool,
    /// Lenient swipe recognition. Accepts faster, sloppier,
    /// circular-ish swipes by widening edit-distance cap, length
    /// window, and de-emphasizing spatial accuracy. Trades
    /// precision for forgiveness. Default off.
    #[serde(default)]
    pub lenient_swipe: bool,
    /// Cardinal-grid diagonals. When true, the cardinal classifier
    /// recognizes 8-way swipes (NE/SE/SW/NW) so each key can carry up
    /// to four extra characters on its diagonals — fewer keys, denser
    /// reach. When false (default), diagonal swipes snap to the
    /// nearest of the 4 cardinals (N/E/S/W), which is more forgiving
    /// for users who don't want to aim diagonals precisely.
    #[serde(default)]
    pub cardinal_diagonals: bool,
    /// When ON, cardinal-swipe failures (no key under start point,
    /// no gesture mapping for the direction, etc.) post a status
    /// to the microterm so users can see what went wrong on-device.
    /// Off by default — only enable when investigating swipes that
    /// silently don't fire.
    #[serde(default)]
    pub cardinal_diagnostic: bool,
    /// Swipe-typing speed mode: tunes how readily a slowdown counts
    /// as a deliberate letter-marking stop. "slow" raises the
    /// absolute stop-velocity threshold (a careful, slow swiper who
    /// pauses gently on each letter), "fast" lowers it (a quick
    /// swiper who only briefly decelerates). "normal" is the balanced
    /// default. Values: "slow" | "normal" | "fast".
    #[serde(default = "default_swipe_speed_mode")]
    pub swipe_speed_mode: String,
    /// Auto-space between swipes: when true, a swipe-typed word that
    /// follows a previous swipe gets a leading space inserted
    /// automatically, so you can swipe word after word without
    /// reaching for the space bar. The first swipe after a manual
    /// space / punctuation / focus change does NOT get a leading
    /// space (avoids a stray space at the start of a field).
    #[serde(default = "default_true")]
    pub swipe_auto_space: bool,
    /// Min swipe distance to register (pixels)
    pub swipe_threshold: f64,
    /// Sample rate for gesture points (per second)
    pub gesture_sample_rate: u32,
    /// Smoothing factor for swipe path (0.0-1.0)
    pub smoothing: f64,
    /// LLM-assisted swipe disambiguation. When enabled AND the
    /// recognizer's top two candidates are within ~0.5 of each
    /// other in score, a small local LLM (Llama 3.2 1B GGUF
    /// recommended) re-ranks them against the surrounding text
    /// context to pick the contextually-correct one. Adds ~50-100ms
    /// to ambiguous swipes only (clear winners skip the LLM).
    /// Requires `llm-swipe-ffi` build feature. Off by default.
    #[serde(default)]
    pub llm_swipe_enabled: bool,
    /// Path to the GGUF model used for swipe re-ranking. Empty
    /// string means "use the default location": platform data dir
    /// + "rusty-board/models/llama-3.2-1b-instruct-q4_k_m.gguf".
    /// User-overridable for testing other models or for non-
    /// standard install locations.
    #[serde(default)]
    pub llm_swipe_model_path: String,
    /// Master toggle for AI swipe assist (adaptive dictionary +
    /// n-gram next-word LM). When off, the recognizer uses ONLY
    /// the dictionary-based shape recognition — no personalization,
    /// no context-aware re-ranking. Defaults true: the feature
    /// ships on, users can opt out in the settings panel if they
    /// prefer the pure-shape recognizer.
    ///
    /// Note this is independent of `llm_swipe_enabled` above —
    /// that one gates the heavy LLM re-ranker. The lightweight
    /// dictionary + n-gram pieces are cheap and stay on by default.
    #[serde(default = "default_true")]
    pub ai_swipe_assist: bool,
    /// Sub-toggle: adaptive dictionary (per-user word frequency
    /// tracker). Gated by `ai_swipe_assist`. Users who want only
    /// n-gram boosting (no dictionary personalization) can turn
    /// just this off.
    #[serde(default = "default_true")]
    pub adaptive_dictionary_enabled: bool,
    /// Sub-toggle: n-gram next-word language model. Gated by
    /// `ai_swipe_assist`. Users who want only dictionary boosts
    /// (no context-aware re-ranking) can turn just this off.
    #[serde(default = "default_true")]
    pub ngram_assist_enabled: bool,
}

fn default_swipe_speed_mode() -> String {
    "normal".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutConfig {
    /// Active layout name (legacy / fallback when the orthogonal axes
    /// below can't resolve to a known layout).
    pub active: String,
    /// Available layouts (kept for back-compat with older configs).
    pub available: Vec<String>,
    /// Show number row above the letters
    pub number_row: bool,
    /// Show emoji button
    pub emoji_button: bool,
    /// Show voice button
    pub voice_button: bool,
    /// Geometry: `"staggered"` (rows offset like a physical QWERTY) or
    /// `"ortholinear"` (strict grid). Orthogonal to `letters`.
    pub row_style: String,
    /// Letter system: `"qwerty"` or `"dvorak"`. Orthogonal to row_style.
    /// Replaces the older `active` field's role for letter selection.
    /// NEVER holds "cardinal" or "stenotype" — those are experimental
    /// modes (see `experimental_mode`).
    pub letters: String,
    /// DEPRECATED: kept for config back-compat. Migrated into
    /// `experimental_mode = "cardinal"` at load if true. New code
    /// should read `experimental_mode` instead.
    pub cardinal_enabled: bool,
    /// Experimental keyboard mode layered on the base letter system:
    /// `"none"` (default), `"cardinal"` (3×3 grid), or `"stenotype"`.
    /// The base letters (qwerty/dvorak) still pick the arrangement
    /// inside cardinal mode; stenotype ignores them.
    #[serde(default = "default_experimental_mode")]
    pub experimental_mode: String,
    /// Compact mode: reduces vertical padding on keys & rows.
    pub compact: bool,
    /// Single-hand thumb-reach mode. When `Some(arc)`, the layout is
    /// compacted to fit inside the user's thumb arc. When `None`,
    /// full-width layout (default). See `docs/single-hand-mode.md`.
    #[serde(default)]
    pub thumb_arc: Option<ThumbArc>,
    /// Width adjustment multiplier in the range [0.75, 1.25]. The
    /// keyboard's computed unit_w is multiplied by this value to
    /// fine-tune key sizes. 1.0 = default sizing.
    #[serde(default = "default_width_scale")]
    pub width_scale: f32,
    /// Height adjustment multiplier in the range [0.75, 1.25].
    /// The total keyboard height (post compact / voice-type
    /// scaling) is multiplied by this. 1.0 = default.
    #[serde(default = "default_width_scale")]
    pub height_scale: f32,
    /// Bottom padding in logical pixels. Reserves space below the
    /// keyboard so users with larger hands (who hold the phone
    /// lower) can reach the keys. Range 0..120 px.
    #[serde(default)]
    pub bottom_pad_px: i32,
}

fn default_width_scale() -> f32 {
    1.0
}

/// User-calibrated thumb-reach polygon. When set, the layout is
/// compacted so the letter region fits inside this polygon.
///
/// Coordinates are normalized to [0.0, 1.0] across the layer surface
/// (`(0,0)` = top-left, `(1,1)` = bottom-right). The polygon is closed
/// (last point connects to first).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThumbArc {
    /// Which corner the thumb anchors at. Determines mirroring of the
    /// default polygon and the tray's anchor side.
    pub anchor: ThumbAnchor,
    /// The polygon describing the reach envelope. Each point is in
    /// normalized surface coordinates. Polygon is closed.
    pub polygon: Vec<(f64, f64)>,
}

/// Anchor corner for the thumb in single-hand mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThumbAnchor {
    BottomLeft,
    BottomRight,
}

impl ThumbArc {
    /// Construct a default arc for the given anchor and approximate
    /// surface width in logical pixels. Used when the user enables
    /// single-hand mode without explicit calibration.
    ///
    /// The arc traces a thumb's reach from the bottom corner. It's
    /// shaped like a quarter-circle wedge:
    ///   - at the anchor (bottom corner): WIDTH is narrow, just the
    ///     thumb's resting span (~30% of screen)
    ///   - moving up: width FANS OUT as the thumb can stretch
    ///   - at the top of the reach: width is roughly `reach_width`
    ///     centered over the anchor x
    ///
    /// Importantly this means: at every row including the bottom,
    /// the polygon doesn't span the FULL screen width. That was the
    /// old bug — bottom row got 100% width, top row got compacted,
    /// producing a pyramid shape.
    pub fn default_for_width(anchor: ThumbAnchor, surface_w: f64) -> Self {
        // The thumb's REST WIDTH expressed as a fraction of screen.
        // At rest (bottom of screen, near the anchor), the thumb
        // sweeps comfortably across most of the screen width. Larger
        // screens have proportionally less reach so this scales
        // down.
        //   - small (≤360px): 0.85 — most of the screen is reachable
        //   - medium (≤480px): 0.78
        //   - large (>480px): 0.70
        let rest_w = if surface_w <= 360.0 {
            0.85
        } else if surface_w <= 480.0 {
            0.78
        } else {
            0.70
        };

        // Tip width — what the thumb covers when fully extended
        // toward the top of the screen. Extension constrains lateral
        // motion: the thumb tip can sweep across a narrower band
        // near the anchor side. Empirically about 60% of the rest
        // width.
        let tip_w = rest_w * 0.6;

        // Build polygon for right-handed anchor (bottom-right). The
        // polygon traces:
        //   - bottom-right corner (anchor)
        //   - LEFTWARD along the bottom by `rest_w` (WIDE base —
        //     thumb at rest has full lateral sweep)
        //   - UPWARD AND INWARD along a quarter-circle arc to
        //     (1.0 - tip_w, 0.0) — the constrained reach at the top
        //   - RIGHTWARD along the top edge to (1.0, 0.0)
        //   - DOWN the right edge back to the anchor
        //
        // This makes the polygon's x-range at each y:
        //   y=1.0 (bottom): [1.0 - rest_w,  1.0]   — WIDE
        //   y=0.5 (middle): [1.0 - mid_w,   1.0]   — medium
        //   y=0.0 (top):    [1.0 - tip_w,   1.0]   — NARROW
        //
        // This matches the standard "thumb comfort zone": the bottom
        // half is easy to reach broadly, the top half only reaches
        // the anchor side.
        //
        // We sample the arc at several points to make a smooth-ish
        // polygon (polygon_x_range linearly interpolates between
        // adjacent vertices).
        let arc_samples = 8;
        let mut left_edge_pts: Vec<(f64, f64)> = Vec::with_capacity(arc_samples + 2);
        // Bottom of the left edge (WIDE base).
        left_edge_pts.push((1.0 - rest_w, 1.0));
        // Quarter-circle from (1.0 - rest_w, 1.0) to (1.0 - tip_w, 0.0).
        // Parametrization:
        //   t ∈ [0, π/2]
        //   x_offset(t) = rest_w - (rest_w - tip_w) * (1 - cos(t))
        //   y(t)        = 1 - sin(t)
        // At t=0 → (1 - rest_w, 1.0). At t=π/2 → (1 - tip_w, 0.0).
        // (Note the SIGN of the second term — the left edge moves
        // RIGHTWARD as we go up, narrowing the polygon.)
        for i in 1..arc_samples {
            let t = (i as f64 / arc_samples as f64) * std::f64::consts::FRAC_PI_2;
            let x_offset = rest_w - (rest_w - tip_w) * (1.0 - t.cos());
            let y = 1.0 - t.sin();
            left_edge_pts.push((1.0 - x_offset, y));
        }
        // Top of the left edge (NARROW tip).
        left_edge_pts.push((1.0 - tip_w, 0.0));

        let mut polygon: Vec<(f64, f64)> = Vec::new();
        polygon.push((1.0, 1.0)); // bottom-right (anchor)
        // Trace left edge from bottom upward.
        polygon.extend(left_edge_pts);
        polygon.push((1.0, 0.0)); // top-right corner
        // Implicit close back to (1.0, 1.0).

        if matches!(anchor, ThumbAnchor::BottomLeft) {
            // Mirror horizontally: x -> 1 - x.
            for p in &mut polygon {
                p.0 = 1.0 - p.0;
            }
        }
        Self { anchor, polygon }
    }
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: "auto".into(),
            accent_color: "3584e4".into(), // GNOME blue
            key_radius: 8.0,
            animation_ms: 150,
            show_magnifier: true,
            opacity: 0.95,
            blur_background: true,
            font_family: "Cantarell".into(),
            font_size_ratio: 0.42,
            colors: ColorPalette::default(),
            show_suggestions: false,
            glass_effect: false,
            debug_trail_styling: false,
        }
    }
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            auto_capitalize: false,
            auto_correct: false,
            word_prediction: true,
            haptic_feedback: true,
            sound_feedback: false,
            long_press_ms: 350,
            double_tap_ms: 300,
            key_repeat: true,
            repeat_delay_ms: 400,
            repeat_interval_ms: 50,
            auto_hide_on_no_focus: Some(true),
            focus_show_tray: false,
        }
    }
}

/// Serde default helper — used to default `key_repeat = true` for
/// configs that pre-date this field.
fn default_true() -> bool {
    true
}

fn default_experimental_mode() -> String {
    "none".into()
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model_path: None,
            language: "auto".into(),
            sample_rate: 16000,
            // Streaming partials run whisper every ~600ms during
            // recording, which is expensive on phone CPUs. Most
            // users get fine results from the final transcription
            // on stop. Default off; enable in settings if wanted.
            streaming: false,
            streaming_interval_ms: 600,
            log_transcripts: false,
            min_audio_ms: 300,
            threads: 4,
        }
    }
}

impl Default for GestureConfig {
    fn default() -> Self {
        Self {
            swipe_typing: true,
            spacebar_cursor: true,
            shift_swipe: true,
            lenient_swipe: false,
            cardinal_diagonals: false,
            cardinal_diagnostic: false,
            swipe_speed_mode: "normal".to_string(),
            swipe_auto_space: true,
            swipe_threshold: 20.0,
            gesture_sample_rate: 60,
            smoothing: 0.3,
            llm_swipe_enabled: false,
            llm_swipe_model_path: String::new(),
            ai_swipe_assist: true,
            adaptive_dictionary_enabled: true,
            ngram_assist_enabled: true,
        }
    }
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            active: "en_us_qwerty".into(),
            available: vec![
                "en_us_qwerty".into(),
                "en_us_dvorak_ortho".into(),
                "en_us_colemak".into(),
                "fr_azerty".into(),
                "de_qwertz".into(),
                "cardinal_grid_en".into(),
            ],
            number_row: false,
            emoji_button: true,
            voice_button: true,
            row_style: "staggered".into(),
            letters: "qwerty".into(),
            cardinal_enabled: false,
            experimental_mode: "none".into(),
            compact: false,
            thumb_arc: None,
            width_scale: 1.0,
            height_scale: 1.0,
            bottom_pad_px: 0,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            appearance: AppearanceConfig::default(),
            behavior: BehaviorConfig::default(),
            voice: VoiceConfig::default(),
            gestures: GestureConfig::default(),
            layout: LayoutConfig::default(),
            llm: LlmConfig::default(),
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://localhost:11434/v1/chat/completions".into(),
            model: "llama3.2:1b".into(),
            api_key: String::new(),
            system_prompt: "You are a typing assistant embedded in an on-screen \
keyboard. Respond with the requested text only — no explanations, no markdown, \
no quotes around the answer. Keep answers brief; the user is on a phone."
                .into(),
            timeout_ms: 15_000,
            max_tokens: 128,
            temperature: 0.3,
        }
    }
}

impl Config {
    /// Compute the canonical config path on this platform.
    pub fn config_path() -> PathBuf {
        ProjectDirs::from("io", "rustyboard", "RustyBoard")
            .map(|p| p.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("config.toml"))
    }

    /// Load config from disk; create with defaults if missing.
    pub fn load_or_default() -> Result<Self> {
        let path = Self::config_path();

        if path.exists() {
            let s = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            match toml::from_str::<Self>(&s) {
                Ok(mut cfg) => {
                    // --- Migration / sanitization ---
                    // (1) If letters got polluted with an experimental
                    // mode name by an older build, move it over and
                    // reset letters to a sane base.
                    if cfg.layout.letters == "stenotype" {
                        cfg.layout.experimental_mode = "stenotype".into();
                        cfg.layout.letters = "qwerty".into();
                    } else if cfg.layout.letters == "cardinal"
                        || cfg.layout.letters == "cardinal_dvorak"
                    {
                        cfg.layout.experimental_mode = "cardinal".into();
                        // cardinal_dvorak implies dvorak base letters.
                        cfg.layout.letters =
                            if cfg.layout.letters == "cardinal_dvorak" {
                                "dvorak".into()
                            } else {
                                "qwerty".into()
                            };
                    }
                    // (2) Migrate the deprecated cardinal_enabled bool.
                    if cfg.layout.cardinal_enabled
                        && cfg.layout.experimental_mode == "none"
                    {
                        cfg.layout.experimental_mode = "cardinal".into();
                    }
                    // (3) Keep cardinal_enabled in sync (some old code
                    // paths still read it; we'll phase those out).
                    cfg.layout.cardinal_enabled =
                        cfg.layout.experimental_mode == "cardinal";

                    tracing::info!(
                        "config loaded from {}: letters={} exp_mode={} row_style={} compact={} key_repeat={}",
                        path.display(),
                        cfg.layout.letters,
                        cfg.layout.experimental_mode,
                        cfg.layout.row_style,
                        cfg.layout.compact,
                        cfg.behavior.key_repeat,
                    );
                    Ok(cfg)
                }
                Err(e) => {
                    // Don't kill the keyboard over a parse error. Most
                    // likely cause: we added a new field with a wrong
                    // default, or the user manually edited config.toml
                    // and made a typo. Rename the broken file (so the
                    // user can recover their custom values), then
                    // continue with defaults. Without this fallback,
                    // any parse error means rusty-board fails to start
                    // and KWin can't summon the keyboard — a much
                    // worse failure mode than "you lost your settings."
                    let backup_path = path.with_extension("toml.broken");
                    tracing::warn!(
                        "config at {} failed to parse: {}. Renaming to {} \
                         and starting with defaults. Edit the .broken file \
                         to recover your settings.",
                        path.display(),
                        e,
                        backup_path.display()
                    );
                    if let Err(rename_err) = std::fs::rename(&path, &backup_path) {
                        tracing::warn!(
                            "couldn't rename broken config: {} (continuing \
                             with defaults anyway)",
                            rename_err
                        );
                    }
                    let cfg = Self::default();
                    cfg.save().ok(); // best effort
                    Ok(cfg)
                }
            }
        } else {
            let cfg = Self::default();
            cfg.save()?;
            Ok(cfg)
        }
    }

    /// Persist current config to disk.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let s = toml::to_string_pretty(self)?;
        // Atomic write: see util::io::atomic_write for rationale.
        // std::fs::write would open the existing file with O_TRUNC
        // and stream the new contents; a crash mid-write would lose
        // the user's entire settings file. The helper handles the
        // temp-file + fsync + rename dance.
        crate::util::io::atomic_write(&path, s.as_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
        tracing::debug!(
            "config saved to {}: letters={} row_style={} compact={} cardinal={}",
            path.display(),
            self.layout.letters,
            self.layout.row_style,
            self.layout.compact,
            self.layout.cardinal_enabled,
        );
        Ok(())
    }
}
