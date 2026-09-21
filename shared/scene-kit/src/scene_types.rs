//! The 3D scene's per-frame inputs, shared by the GPU renderer
//! (`glow_scene.rs`) and its e-ink stand-in (`glow_scene_eink.rs`), so the
//! two cannot drift apart. Plain data: no GL here.
/// All per-frame inputs: the scene lighting, the sun's screen position (for god-rays), and the post knobs.
#[derive(Clone)]
pub struct Uniforms {
    pub viewproj: [f32; 16], pub eye: [f32; 3],
    pub sun_dir: [f32; 3], pub sun_col: [f32; 3], pub sky: [f32; 3], pub ground: [f32; 3], pub ambient: f32,
    pub fog_col: [f32; 3], pub fog: f32,
    pub sun_uv: [f32; 2],            // sun position on screen in [0,1]; god-rays stream from here
    pub time: f32,                   // seconds, for animated materials (water)
    pub bump: f32,                   // normal-map relief strength
    pub exposure: f32,               // overall brightness
    /// Last touch: world x,y,z and age in seconds. `None` when nothing has been touched.
    ///
    /// An Option rather than a sentinel at this layer, so a caller cannot forget which value means
    /// "never" — the shader gets the sentinel and the Rust side gets a type that says it.
    ///
    /// This is the SAME mechanism for a plant that breathes and a rune that lights: what differs is
    /// only whether the response drives a displacement along the normal or an emissive term.
    pub touch: Option<[f32; 4]>,
    pub texscale: f32,              // world units → texture tile frequency (triplanar)
    pub threshold: f32,              // bloom bright-pass cutoff
    pub bloom_amt: f32, pub godray_amt: f32, pub godray_weight: f32, pub godray_decay: f32,
    pub caustic: f32, pub caustic_col: [f32; 3],   // underwater dapple: 0 off (most scenes), >0 aquatic
    // COMPOSER PEARL (screen-space, in the composite pass): a pale refractive pearl that scatters the scene
    // through itself with fresnel + thin-film + dispersion + cracks. 0 = off.
    pub pearl: f32, pub pearl_variant: f32, pub pearl_pos: [f32; 2], pub pearl_r: f32, pub aspect: f32,
    pub nacre: f32,                                // thin-film iridescence scale on water (1 = normal; 0 = off)
    pub underwater: f32,                           // 0 in the air dome; ->1 when the eye is outside in open water
    pub defocus: f32,                              // >0 frosts (blurs) the scene — the glass-UI backdrop sets it
    pub frost_a: f32, pub frost_b: f32,            // frost band thickness at each screen edge (UV); 0 = no band
    pub cam_warp: [f32; 2],                        // camera (yaw,pitch) for the 3D-aware underwater ripple
    pub opaque: f32, pub bg: [f32; 3],             // backdrop: force opaque + fill empty regions with bg color
    pub dof: f32, pub focus: f32,                  // depth-of-field strength + focal distance (eye units)
    pub saturation: f32,                           // final chroma gain (1 neutral, >1 vibrant) — counters fog grey
    pub shoulder: f32,                             // luminance highlight rolloff (0 off/linear, 1 full)
    pub light_viewproj: [f32; 16],                 // sun's ortho view-proj for the shadow map
    pub shadow_on: f32,                            // 0 = no shadow pass; 1 = shadow-mapped direct light
    pub fill_dir: [f32; 3], pub fill_col: [f32; 3], // colored secondary fill light (off-sun side)
    /// The inscription raster sampled (by per-vertex UV) on `MAT_INSCRIPTION` faces. `version` gates
    /// the GPU upload — the renderer re-uploads only when it changes, so passing the same page every
    /// frame costs one u64 compare. `None` keeps the 1×1 transparent fallback bound (no inscription).
    pub page: Option<PageTex>,
    /// Static inscription atlas for `MAT_INSCRIPTION2` faces (same versioned
    /// upload discipline; expected to change ~once per file/scene load).
    pub page2: Option<PageTex>,
}

/// CPU-side page pixels headed for `u_page`. `rgba` is top-row-first (matching `panel_uv`'s V axis),
/// tightly packed, `size = [w, h]`. Arc so per-frame Uniforms clones don't copy megabytes.
#[derive(Clone)]
pub struct PageTex {
    pub size: [i32; 2],
    pub rgba: std::sync::Arc<Vec<u8>>,
    pub version: u64,
}
impl Default for Uniforms {
    fn default() -> Self {
        Uniforms {
            touch: None,   // nothing touched yet; the shader reads the sentinel as "no response"
            viewproj: [0.0; 16], eye: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_col: [1.0; 3],
            sky: [0.46, 0.58, 0.77], ground: [0.25, 0.20, 0.16], ambient: 0.55,
            fog_col: [0.1, 0.12, 0.14], fog: 0.012, sun_uv: [0.5, 0.85], time: 0.0, bump: 0.7, exposure: 1.35, texscale: 0.35,
            threshold: 0.62, bloom_amt: 0.85, godray_amt: 0.45, godray_weight: 0.42, godray_decay: 0.92,
            caustic: 0.0, caustic_col: [0.0, 1.0, 0.82],
            pearl: 0.0, pearl_variant: 0.0, pearl_pos: [0.5, 0.5], pearl_r: 0.24, aspect: 1.6,
            nacre: 1.0, underwater: 0.0, defocus: 0.0, frost_a: 0.0, frost_b: 0.0, cam_warp: [0.0, 0.0], opaque: 0.0, bg: [0.04, 0.07, 0.10], dof: 0.0, focus: 18.0,
            saturation: 1.18,
            shoulder: 1.0,
            light_viewproj: [0.0; 16], shadow_on: 0.0, page: None, page2: None,
            fill_dir: [0.0, 1.0, 0.0], fill_col: [0.0, 0.0, 0.0],
        }
    }
}
