//! World renderer — multi-chunk variant of `ChunkRenderer`.
//!
//! Differences from the single-chunk renderer:
//! * `meshes: HashMap<ChunkCoord, UploadedMesh>` instead of `Option<UploadedMesh>`
//! * Frustum culling per chunk before issuing a draw
//! * `FlyCamera` rather than orbit `Camera`
//! * Deferred destroy queue: a buffer freed *now* might still be in use by a
//!   previous frame's command buffer, so we hold the resources for
//!   `MAX_FRAMES_IN_FLIGHT` frames before actually freeing them.
//!
//! The render loop API is structured to take a list of (camera, viewport)
//! pairs to keep the door open for splitscreen later — but for now we always
//! pass a single camera covering the whole framebuffer.

use ash::vk;
use crate::render::camera::{FlyCamera, View};
use crate::render::chunk_pipeline::ChunkPipeline;
use crate::render::entity_pipeline::EntityPipeline;
use crate::render::context::VulkanContext;
use crate::render::depth::DepthBuffer;
use crate::render::froxel::{FroxelVolume, FROXEL_X, FROXEL_Y, FROXEL_Z, FROXEL_NEAR_M, FROXEL_FAR_M};
use crate::render::froxel_fill_pipeline::{FroxelFillPipeline, FroxelFillPushConstants};
use crate::render::light::{single_cascade, LightCascade};
use crate::render::mesher::{mesh_chunk, ChunkMesh, PackedVertex};
use crate::render::shadow_map::ShadowMap;
use crate::render::shadow_pipeline::ShadowPipeline;
use crate::render::swapchain::{create_render_pass_with_depth, Swapchain};
use crate::world::chunk::{Chunk, ChunkCoord};
use crate::constants::CHUNK_SIZE;
use nalgebra_glm as glm;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// v0.24.2c.10: Frames in flight. Reduced from 2 to 1 in the
/// "no-latency baseline" ship. With FIF=1, CPU waits for GPU to
/// finish frame N before recording frame N+1 — no command-buffer
/// queue, no input lag from CPU lookahead, presented frame reflects
/// the most recent input.
///
/// Trade-off: if GPU is slow, CPU sits idle. The companion DRS
/// (dynamic resolution scaling) system adapts resolution down when
/// GPU starts missing budget so this rarely costs throughput.
///
/// Note: this is intentionally kept as a const (not env-tunable
/// per-binary) so the compiler can optimise pool sizes and the
/// codegen for the per-frame cycle. To A/B test FIF values, build
/// with different values and run them against the same .unendtest
/// via the launcher (c.11+).
/// Upper bound (cap) on frames in flight. Pools, descriptor sets, and command
/// buffers are sized for this many; the ACTUAL number used at runtime is
/// `frames_in_flight` (read from `UNENDLICH_FRAMES_IN_FLIGHT`, default 1, clamped
/// to this cap). r476: was a hard `= 1`, which forced a full CPU↔GPU round-trip
/// every frame (no overlap) — the dominant cause of streaming freezes in BOTH
/// mesher modes, since the CPU stalls on `frame.in_flight` while the GPU does
/// worldgen/mesh/render serially. Raising the cap + an opt-in runtime value lets
/// the CPU pipeline ahead. Default stays 1 so visuals are unchanged until tested.
const MAX_FRAMES_IN_FLIGHT: usize = 3;

// Chunks are cubic 32^3 — AABB extent on each axis is CHUNK_SIZE.

#[derive(Debug)]
pub enum WorldRenderError {
    Vulkan(vk::Result),
    Swapchain(crate::render::swapchain::SwapchainError),
    Other(String),
}

/// r313: themed-button shims so menu buttons use the toolkit look instead of raw
/// egui. Each resolves the accent from the active theme; drop-in replacements for
/// `ui.button(label).clicked()` (+ a sized variant for fixed-width columns and a
/// Response variant for sites that need the response, e.g. tooltips).
#[cfg(feature = "ui-overlay")]
fn tbtn(ui: &mut egui::Ui, label: &str) -> bool {
    let accent = unendlich_ui::current_theme(ui.ctx()).accent;
    unendlich_ui::button(ui, label, accent).clicked()
}
#[cfg(feature = "ui-overlay")]
fn tbtn_sized(ui: &mut egui::Ui, label: &str, w: f32, h: f32) -> bool {
    let accent = unendlich_ui::current_theme(ui.ctx()).accent;
    unendlich_ui::button_sized(ui, label, accent, egui::vec2(w, h)).clicked()
}

impl From<vk::Result> for WorldRenderError {
    fn from(r: vk::Result) -> Self { WorldRenderError::Vulkan(r) }
}
impl From<crate::render::swapchain::SwapchainError> for WorldRenderError {
    fn from(e: crate::render::swapchain::SwapchainError) -> Self { WorldRenderError::Swapchain(e) }
}

impl std::fmt::Display for WorldRenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorldRenderError::Vulkan(r) => write!(f, "Vulkan error: {:?}", r),
            WorldRenderError::Swapchain(e) => write!(f, "Swapchain: {}", e),
            WorldRenderError::Other(s) => write!(f, "{}", s),
        }
    }
}
impl std::error::Error for WorldRenderError {}

pub type Result<T> = std::result::Result<T, WorldRenderError>;

/// Convert a glm::Mat4 (column-major nalgebra storage) into a
/// `[[f32; 4]; 4]` with the same column-major convention. Used by
/// v0.24.0b to push view/projection matrices into the ViewParams
/// SSBO in a form the GLSL `mat4` indexing matches.
///
/// glm::Mat4 stores columns sequentially in memory; accessing
/// `m[(row, col)]` returns the element at that position. Our output
/// `[col][row]` indexing matches GLSL's `m[col][row]` convention.
fn mat4_to_col_array(m: &glm::Mat4) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            out[col][row] = m[(row, col)];
        }
    }
    out
}

/// Camera + atmosphere uniform. Mirrors the `CameraUbo` block in
/// `chunk.vert` / `chunk.frag` exactly. std140 layout: every field is
/// at a `vec4` boundary (16 bytes), and `view_proj` is the 64-byte
/// matrix at offset 0.
///
/// Total size = 64 + 7*16 = 176 bytes.
///
/// Why one UBO instead of two:
/// Two would be cleaner (camera vs. atmosphere are conceptually different)
/// but they're filled in the same place at the same time, and the data is
/// small. One binding, one buffer, one upload per frame.
#[repr(C)]
#[derive(Copy, Clone)]
struct CameraUbo {
    view_proj:     [[f32; 4]; 4],
    /// xyz = camera world-space position, w = unused (1.0).
    camera_pos:    [f32; 4],
    /// xyz = direction sunlight TRAVELS (sun→ground, unit), w = directional intensity in [0,1].
    sun_dir:       [f32; 4],
    /// rgb = sun radiance color, a = unused.
    sun_color:     [f32; 4],
    /// rgb = sky / fog target color, a = unused.
    sky_color:     [f32; 4],
    /// rgb = ambient term added to diffuse, a = unused.
    ambient_color: [f32; 4],
    /// x = base fog density, y = humidity at player, z = temperature at player, w = time_of_day in seconds.
    atmos_params:  [f32; 4],
    /// xy = wind vector at player (world XZ), z = wind magnitude, w = unused.
    wind_params:   [f32; 4],
    /// xy = viewport size in pixels (used by fragment shader for screen-uv
    /// effects like vignette), zw = viewport offset in pixels (zero for
    /// fullscreen, populated when split-screen lands). This is per-RenderView
    /// metadata: when multi-camera/split-screen comes in, each RenderView
    /// gets its own UBO with its own viewport rect, and `gl_FragCoord` in
    /// the shader resolves to that viewport's local pixels via this field.
    viewport_rect: [f32; 4],
    /// Light-space view-projection matrix for the (currently single) shadow
    /// cascade. Computed each frame from the sun direction + camera view
    /// frustum. The shadow vertex shader uses this to transform world-space
    /// vertices into the shadow map's clip space; the main fragment shader
    /// will sample the shadow map using this matrix in Phase 2 (the shader
    /// declares the field but does not yet sample, so the matrix sitting
    /// here is currently inert from the visible-world perspective).
    light_view_proj: [[f32; 4]; 4],
    /// r419 (flow-as-shader): xy = world origin of the coarse flow grid, z = blocks per
    /// cell, w = enable flag (0 = ignore, water uses wind only). Appended LAST so existing
    /// field offsets are unchanged (normal rendering is identical when w = 0).
    flow_coverage: [f32; 4],
    /// Coarse flow grid, 16×16 cells packed 2-per-vec4 (.xy even cell, .zw odd). 128 vec4.
    flow_grid: [[f32; 4]; 128],
    /// r427: screen FX. x = camera submerged (0/1), y = time (s) for the underwater shimmer,
    /// z/w spare. Appended last; zero = no effect, rendering unchanged.
    fx_params: [f32; 4],
    /// r431: water-depth grid coverage [origin_x, origin_z, blocks_per_cell, enable].
    water_depth_cov: [f32; 4],
    /// r431: 16×16 water-depth cells (blocks), packed 4 per vec4. Drives real depth-color
    /// (shallow→clear, deep→dark) instead of the view-angle guess. Zero/disabled = unchanged.
    water_depth_grid: [[f32; 4]; 64],
}

/// Per-frame atmosphere/climate state, sampled by the bin and passed to
/// the renderer for upload to the fragment shader. Decouples "how the
/// world looks" from "where the camera points": the renderer doesn't
/// reach into the simulation, the bin reaches into both and bridges.
///
/// Use `AtmosphereParams::neutral()` for test/headless paths that have
/// no climate model — gives a reasonable midday-clear-sky look.
#[derive(Debug, Clone, Copy)]
pub struct AtmosphereParams {
    /// Direction sunlight TRAVELS (sun → ground). Unit vector.
    pub sun_dir: [f32; 3],
    /// Directional intensity, [0, 1]. Drives `dot(N, -sun_dir) * intensity`.
    pub sun_intensity: f32,
    /// rgb sun radiance color (mid-day = warm white; low sun = orange).
    pub sun_color: [f32; 3],
    /// rgb sky / fog target color.
    pub sky_color: [f32; 3],
    /// rgb ambient (added to diffuse before applying material).
    pub ambient_color: [f32; 3],
    /// Base fog density. Multiplied by humidity inside the shader, so
    /// pass an unmodulated baseline (e.g. 0.008). Larger = denser fog.
    pub fog_density: f32,
    /// Humidity at the camera position, [0, 1]. Increases fog density.
    pub humidity: f32,
    /// Temperature at the camera position, [0, 1]. Currently informational
    /// (reserved for the shader's future use); not yet driving any visible
    /// effect.
    pub temperature: f32,
    /// In-game time-of-day, seconds. Used as an animation phase — wind
    /// shimmer and water waves drift over time.
    pub time_of_day: f32,
    /// Wind vector at the camera position, world-space XZ. Drives the
    /// fog-shimmer effect that visualizes the wind field.
    pub wind_xz: [f32; 2],
    /// Wind magnitude scalar. Convenience: `wind_xz.length()`. The shader
    /// uses it to scale shimmer amplitude — calm air → no shimmer.
    pub wind_magnitude: f32,
    /// r420: global water-wave-speed multiplier (1.0 = unchanged). Uploaded in
    /// `wind_params.w`; lets the bin slow the wave animation without touching amplitude.
    pub wave_speed: f32,
}

impl AtmosphereParams {
    /// Mid-day clear-sky defaults. No wind, no humidity, neutral temp.
    /// Used by tests and any draw path that doesn't know about climate.
    pub fn neutral() -> Self {
        Self {
            sun_dir: [0.0, -1.0, 0.0],          // straight down
            sun_intensity: 1.0,
            sun_color: [1.0, 0.95, 0.85],
            sky_color: [0.55, 0.75, 1.0],
            ambient_color: [0.20, 0.22, 0.28],
            fog_density: 0.008,
            humidity: 0.5,
            temperature: 0.5,
            time_of_day: 0.0,
            wind_xz: [0.0, 0.0],
            wind_magnitude: 0.0,
            wave_speed: 1.0,
        }
    }
}

/// Predicate for "is this mesh-slot key stale relative to a new
/// upload at `(new_coord, new_lod)`?" A key is stale iff:
///
/// * It belongs to the same coord as the incoming upload, AND
/// * It is at a DIFFERENT LOD than the incoming upload.
///
/// Same-coord-same-LOD keys are NOT stale: the HashMap insert in
/// `upload_mesh` will replace them in place (and route the old slot
/// to `pending_destroy` via the `insert` return value).
///
/// Different-coord keys are NEVER stale here: they describe other
/// chunks' meshes that have nothing to do with this upload.
///
/// Extracted as a free function so the predicate can be unit-tested
/// without constructing a real `WorldRenderer` (which requires a
/// Vulkan context). v0.23.2 bug-fix landing.
#[allow(dead_code)] // retained for the stale_lod_key_* predicate unit tests
fn stale_lod_key(
    candidate: crate::world::lod::MeshSlotKey,
    new_coord: ChunkCoord,
    new_lod: crate::world::lod::LodLevel,
) -> bool {
    candidate.coord == new_coord && candidate.lod != new_lod
}

#[allow(dead_code)] // index_count, min_y, max_y kept for debug/inspection
struct UploadedMesh {
    /// Slot in the mesh buffer pool. The pool owns the underlying GPU
    /// buffers; this struct just records which slot we're using.
    slot: crate::render::mesh_buffer_pool::MeshSlot,
    index_count: u32,
    /// Chunk version when this mesh was uploaded. Used by callers to
    /// detect "the chunk has been edited since this mesh was generated"
    /// and trigger a re-mesh. Without this version tracking, the
    /// submission code can't tell stale-vs-fresh in the renderer.
    version: u64,
    /// World-space Y bounds of this mesh's vertices. Used to tighten
    /// the chunk's frustum-culling AABB on the Y axis instead of the
    /// full CHUNK_SIZE-tall slab.
    ///
    /// **Why this exists**: a chunk that contains only a thin layer of
    /// terrain (e.g. an above-ground chunk where bedrock starts at
    /// y=oy+2 and tops out at y=oy+5) has 27 of its 32 vertical
    /// units containing nothing visible. The naive AABB
    /// `[oy, oy+CHUNK_SIZE]` makes the frustum test conservative and
    /// the chunk gets drawn even when the camera frustum only grazes
    /// the empty top of its slab. With tight Y bounds the same chunk
    /// culls correctly the moment the camera tilts enough that the
    /// thin terrain band is out of frustum.
    ///
    /// Stepping-stone toward the deeper "POI-pass piggy-back" idea:
    /// the chunk struct itself eventually gets `min_y_solid` /
    /// `max_y_solid` maintained on `set()`. Until then, the mesh's
    /// vertex bounds are the cheapest proxy: they ARE the bounds of
    /// solid matter as far as the renderer cares (only solid faces
    /// produce vertices).
    ///
    /// Empty meshes get the chunk's full Y extent — they aren't drawn
    /// anyway, so the value doesn't matter. We use the full extent so
    /// downstream code can't mistake the bounds for "no solid here"
    /// when really we just have no data.
    min_y: f32,
    max_y: f32,
    /// Slot in the indirect-draw metadata SSBO. The cull shader reads
    /// each frame from these slots to emit `VkDrawIndexedIndirectCommand`
    /// entries for visible chunks. Allocated when the mesh is uploaded;
    /// freed when the chunk is evicted (with the same MAX_FRAMES_IN_FLIGHT
    /// deferral as the mesh slot).
    draw_slot: crate::render::chunk_indirect::DrawSlot,
}

/// A pool slot waiting to be recycled once the GPU is definitely done
/// reading from it. Held for `MAX_FRAMES_IN_FLIGHT` frames after eviction;
/// the slot itself stays allocated in the pool, just not reusable until
/// the GPU has cycled past any draw that referenced it.
struct PendingDestroy {
    slot: crate::render::mesh_buffer_pool::MeshSlot,
    /// Indirect-draw slot (separate from the mesh slot — different pool).
    draw_slot: crate::render::chunk_indirect::DrawSlot,
    frames_remaining: usize,
}

/// v0.24.19: an entry in the renderer-owned mesh cache. When a chunk
/// is evicted from the active set (`meshes`), instead of immediately
/// returning its GPU buffers to the pool we move them here. On revisit
/// (the chunk-manager regenerates the chunk and the mesh-pool produces
/// a fresh `MeshResult` whose `version` matches the cached entry's
/// version), we restore the cached buffers and discard the freshly-
/// meshed copy — saving the upload bytes and any GPU submission-queue
/// pressure that came with them.
///
/// **Version match required.** If the chunk was edited between
/// eviction and revisit, the mesh-pool's version stamp will differ
/// from the cached one. We then drop the cache entry (its buffers go
/// to `pending_destroy`) and fall through to the normal upload path.
///
/// **Cache capacity** is controlled by `UNENDLICH_MESH_CACHE_SIZE`
/// (default 0 = disabled, so the behaviour matches pre-v0.24.19
/// exactly). When the cache is full and a new entry needs space, the
/// least-recently-used entry is evicted (its slot returns to the pool
/// via `pending_destroy`).
struct CachedMesh {
    slot: crate::render::mesh_buffer_pool::MeshSlot,
    draw_slot: crate::render::chunk_indirect::DrawSlot,
    index_count: u32,
    version: u64,
    min_y: f32,
    max_y: f32,
    /// Wall-clock sequence number for LRU ordering. Newer = more
    /// recently used. Incremented on every cache touch.
    last_touched: u64,
}

struct FrameSync {
    image_available: vk::Semaphore,
    render_finished: vk::Semaphore,
    in_flight: vk::Fence,
    command_buffer: vk::CommandBuffer,
    ubo_buffer: vk::Buffer,
    ubo_memory: vk::DeviceMemory,
    ubo_ptr: *mut u8,
    descriptor_set: vk::DescriptorSet,
    /// v0.24.3 chug-investigation instrumentation: wall-clock Instant
    /// when this slot was most recently submitted (or None if never).
    /// Used to compute `last_submit_to_wait_ms` for the wait_fences
    /// diagnostic. Read-only side channel; does NOT affect scheduling.
    last_submit_at: Option<std::time::Instant>,
}

/// r545: per-frame survival meters the bin hands to the renderer for the HUD.
/// `hunger`/`thirst`/`stamina` are `[0,1]` (1 = full); `comfort` is `[-1,1]`
/// (−1 freezing … 0 comfortable … +1 overheating); `body_c` is body temp °C.
/// thirst/stamina default to 1.0 until those pillars land (Roll 3).
#[derive(Clone, Copy, Debug)]
pub struct SurvivalHud {
    pub hunger: f32,
    pub thirst: f32,
    pub stamina: f32,
    pub comfort: f32,
    pub body_c: f32,
}

/// r576: one occupied backpack slot for the survival inventory panel — a swatch
/// colour + short label + stack count, pre-resolved by the bin so the renderer
/// stays a pure view.
#[derive(Clone)]
pub struct InvSlotView {
    pub color: [u8; 3],
    pub name: String,
    pub count: u32,
}

pub struct WorldRenderer {
    pub render_pass: vk::RenderPass,
    pub swapchain: Swapchain,
    pub depth: DepthBuffer,
    pub pipeline: ChunkPipeline,
    pub entity_pipeline: EntityPipeline,
    pub command_pool: vk::CommandPool,
    descriptor_pool: vk::DescriptorPool,
    frames: Vec<FrameSync>,
    current_frame: usize,
    /// r476: actual frames in flight (1..=MAX_FRAMES_IN_FLIGHT), from
    /// UNENDLICH_FRAMES_IN_FLIGHT (default 1). Drives the current_frame cycle and
    /// the deferred-free hold so the CPU can pipeline ahead of the GPU.
    frames_in_flight: usize,
    pub framebuffer_size: (u32, u32),
    /// r419 (flow-as-shader): latest coarse water-flow grid set by the bin each frame
    /// (`set_water_flow`). Uploaded in the UBO; `flow_coverage[3]` is the enable flag, 0 by
    /// default so the water shader uses wind only until the bin turns it on.
    water_flow_coverage: [f32; 4],
    water_flow_grid: Box<[[f32; 4]; 128]>,
    /// r427: underwater screen-FX params (x = submerged, y = time).
    fx_params: [f32; 4],
    /// r499: current season phase [0,1) (spring→summer→autumn→winter), packed
    /// into fx_params.z for the foliage seasonal tint in chunk.frag.
    world_season01: f32,
    /// r431: water-depth grid + coverage for depth-color.
    water_depth_cov: [f32; 4],
    water_depth_grid: Box<[[f32; 4]; 64]>,

    /// r306: neural deferred G-buffer (gated parallel pass). `Some` only when
    /// `UNENDLICH_NEURAL_GBUFFER=1`; otherwise `None` and the path is inert /
    /// byte-identical to before. An inert producer for now — nothing samples it.
    gbuffer: Option<crate::render::gbuffer::NeuralGBuffer>,

    /// r307: neural feature pass — samples the G-buffer into the feature buffer.
    /// Gated by the same flag; constructed iff `gbuffer` is. Inert producer (the
    /// feature buffer is unread until the shade pass is wired).
    neural_feat: Option<crate::render::neural_feat_pipeline::NeuralFeatPipeline>,

    /// r308: background MLP trainer (its own thread). `Some` only when
    /// `UNENDLICH_NEURAL_TRAIN=1`. Self-trains the shading net from game-fed
    /// (feature, ground-truth) samples and bakes Q8 weights for the shade pass to
    /// hot-swap. Idles until the ground-truth sample feed is wired.
    neural_trainer: Option<crate::render::neural_train::BackgroundTrainer>,

    /// In-game egui overlay (hand-rolled egui-0.34-on-ash renderer). Drawn at the
    /// end of the main pass, on top of the world. Off unless `ui-overlay`.
    #[cfg(feature = "ui-overlay")]
    pub egui_overlay: crate::render::egui_overlay::EguiOverlay,
    /// Index into the creative item-picker list (the slot currently "in hand").
    /// Selection becomes clickable once pointer input is fed to the overlay.
    #[cfg(feature = "ui-overlay")]
    pub creative_selected: usize,
    /// r655: items the player has put onto the creative hotbar (per slot).
    /// `None` = unassigned (that slot falls back to showing the palette item).
    #[cfg(feature = "ui-overlay")]
    pub creative_hotbar: [Option<crate::item::ItemKind>; 10],
    /// r657: the player's creative inventory — a 3×9 grid they fill by clicking
    /// catalog items. Shown on the Inventory tab (which was empty before).
    #[cfg(feature = "ui-overlay")]
    pub creative_inventory: [Option<crate::item::ItemKind>; 27],
    /// r657: which creative hotbar slot is active (clicking a palette item fills
    /// this slot; the slot's item is what gets placed).
    #[cfg(feature = "ui-overlay")]
    pub creative_hotbar_sel: usize,
    /// r657: the item the player actually wants to place, resolved from their last
    /// click (palette item or hotbar slot). The bin reads this and, for a block,
    /// makes it the creative placement block — so every catalog block is testable.
    #[cfg(feature = "ui-overlay")]
    pub creative_place_item: Option<crate::item::ItemKind>,
    /// Which creative tab is showing (Inventory / Items / SpawnEggs).
    #[cfg(feature = "ui-overlay")]
    pub creative_tab: crate::inventory_screen::Tab,
    /// Overlay UI scale (egui pixels_per_point). 2.0 = crisp on HiDPI; a future
    /// settings slider writes 1.0..5.0 here. Ideally matches the window content
    /// scale (`glfwGetWindowContentScale`).
    #[cfg(feature = "ui-overlay")]
    pub ui_scale: f32,

    /// Pointer state forwarded from the windowing layer to egui. `ui_pointer_pos`
    /// is in framebuffer pixels (converted to logical points in prepare).
    /// `ui_pointer_down` is the current left-button state; the rising/falling
    /// edge vs `ui_pointer_down_prev` becomes a PointerButton event. When
    /// `ui_pointer_active` is false the events are not forwarded (game-mode:
    /// cursor is captured, deltas drive the camera).
    #[cfg(feature = "ui-overlay")]
    pub ui_pointer_pos: (f32, f32),
    #[cfg(feature = "ui-overlay")]
    pub ui_pointer_down: bool,
    /// r186: right button state (preview avoid-click etc.).
    pub ui_pointer2_down: bool,
    pub ui_pointer2_down_prev: bool,
    #[cfg(feature = "ui-overlay")]
    pub ui_pointer_down_prev: bool,
    /// r181: text typed this frame + edit keys (drained into egui events).
    pub ui_text_input: String,
    pub ui_backspace: bool,
    pub ui_enter: bool,
    /// r185: true while an egui text field has focus — the bin must not
    /// run letter hotkeys (C/B/I/P...) on typed characters.
    pub ui_wants_keyboard: bool,
    /// Clicks that collapsed into one frame (press+release same frame);
    /// forwarded to egui as discrete press/release pairs so fast / low-fps
    /// clicks still register on toggles.
    #[cfg(feature = "ui-overlay")]
    pub ui_click_pending: u32,
    #[cfg(feature = "ui-overlay")]
    pub ui_pointer_active: bool,

    /// In-game debug/settings panel (Esc). Un-gated so the bin can flip it
    /// regardless of the `ui-overlay` feature; only rendered when that
    /// feature is on.
    pub settings_open: bool,
    /// r126: bin-supplied survival status lines (mode, clock, stamina,
    /// vitality, inventory). Rendered in the on-screen HUD card each frame.
    pub hud_status_lines: Vec<String>,

    /// r545: survival meters (hunger + thermal comfort) shown as on-screen bars
    /// in survival mode. `None` hides the panel (creative mode / menus). The bin
    /// refreshes it every frame from the player's Vitality + Thermal state.
    pub hud_survival: Option<SurvivalHud>,

    /// r576: the player's real backpack contents for the centred inventory panel in
    /// SURVIVAL (creative still shows the placeable catalog). Each entry is one
    /// occupied slot, resolved via `item::item_display`. The bin refreshes it each
    /// frame from `App::inventory().slots()`. Empty in creative.
    pub backpack: Vec<InvSlotView>,
    /// r662: POSITIONAL survival inventory (all 36 slots, `None` = empty), so the
    /// panel can draw a real hotbar row + 3×9 backpack and support click-to-move.
    /// Slots `0..inv_hotbar_size` are the hotbar; the rest are the backpack.
    #[cfg(feature = "ui-overlay")]
    pub inv_slots: Vec<Option<InvSlotView>>,
    /// r662: how many leading slots of `inv_slots` are the hotbar (usually 9).
    #[cfg(feature = "ui-overlay")]
    pub inv_hotbar_size: usize,
    /// r662: the item currently held on the cursor (picked up from a slot), drawn
    /// at the pointer. Resolved by the bin from its `fg_inv_cursor` stack.
    #[cfg(feature = "ui-overlay")]
    pub inv_cursor: Option<InvSlotView>,
    /// r662: slot the player clicked this frame (global index), or -1. The bin
    /// reads it, applies the pick/place against the real inventory, and clears it.
    #[cfg(feature = "ui-overlay")]
    pub inv_click: i32,

    /// Title/pause menu overlay (unendlich-fullgame). The bin sets `menu_open`;
    /// the centered menu panel writes the player's click into `menu_choice`
    /// (0=none, 1=Singleplayer, 2=Host, 3=Join, 4=Quit) which the bin reads + resets.
    pub menu_open: bool,
    /// r167: in-world PAUSE menu (Esc). Separate from the debug panel
    /// (now F3) and the title menu. The bin reads+clears `pause_action`.
    pub pause_menu_open: bool,
    /// 1 = Resume, 2 = Settings (debug panel), 3 = Save & Quit to Title.
    pub pause_action: u8,
    /// r169: categorized SETTINGS window (pause menu -> Settings).
    pub settings_menu_open: bool,
    pub settings_tab: u8, // 0 video, 1 audio, 2 gameplay, 3 controls
    /// Live slider values; the bin applies them each frame.
    pub ui_day_length: f32,
    /// r235: server game-speed (time-dilation) slider position in [0,1].
    /// 0=freeze, 0.5=1x normal (default), 1.0=10x; logarithmic. Reconciled to
    /// "closest to 1x" across peers (see sim::game_speed). Server-side only.
    pub ui_game_speed_pos: f32,
    pub ui_music_vol: f32,
    /// r184: Performance tab (live levers + autotune).
    pub ui_plant_stride: u64,
    pub ui_leaf_stride: u64,
    pub ui_apply_budget: u32,
    /// r324: selected perf/graphics preset (index into PerfPreset::SELECTABLE).
    pub ui_perf_preset: u8,
    pub ui_fps_target: u32, // 0 = autotune off
    pub ui_autotune: bool,
    /// r476: GPU-mesher throttle (percent of dispatch time slept after each
    /// octree dispatch, to leave the shared queue idle for rendering). 0 = off.
    pub ui_gpu_mesh_yield: u64,
    /// r476: dynamic render distance — caps the radius below the chosen value
    /// when frames run long, grows back when they recover.
    pub ui_adaptive_rd: bool,
    /// r480: tiled shared-memory octree mesher (only active with GPU_MESHER=2).
    pub ui_octree_tile: bool,
    /// r480: detached terrain — glide over not-yet-streamed ground, settle on spawn.
    pub ui_detached_terrain: bool,
    pub ui_fog_on: bool,
    pub ui_shadows_on: bool,
    /// r437: debug-logging toggles, surfaced in Settings → Debug. `master` gates ALL of them and
    /// defaults off (no log spam); the bin reads these each frame to gate its log lines.
    pub ui_debug_master: bool,
    pub ui_debug_water: bool,
    pub ui_debug_flow: bool,
    pub ui_debug_echo: bool,
    /// r169: CRAFTING window (C). The bin fills `craft_lines` (name, can,
    /// have-summary) and consumes `craft_pick` (1-based recipe index).
    pub craft_menu_open: bool,
    /// (name, craftable, have-summary, canonical tooltip)
    pub craft_lines: Vec<(String, bool, String, String)>,
    pub craft_pick: u8,
    /// r488: CHARACTER-SELECT window. The bin populates `character_roster`; the
    /// window edits it (select/reroll/save) and, on Save, sets `character_chosen`
    /// for the bin to apply + persist (loads across worlds). Opened from the
    /// title menu's "Character" button.
    pub character_menu_open: bool,
    pub character_roster: Option<crate::player_character::CharacterRoster>,
    pub character_chosen: Option<crate::player_character::CharacterProfile>,
    /// r169: ANALYTICS window (F4). Bin feeds samples + stat lines.
    pub perf_window_open: bool,
    /// r177: latch — bin dumps an ascii frame-time graph to the log.
    pub perf_export_ascii: bool,
    /// r178: world-entry veil (1=black, eases to 0) + its label.
    pub loading_fade: f32,
    pub loading_label: String,
    pub perf_samples: Vec<f32>,
    pub perf_stats: Vec<String>,
    prev_shadows_disabled: bool,
    pub menu_choice: u32,

    /// New-game wizard overlay (driven by `sim::new_game_wizard` in the bin; the
    /// renderer is display-only). The bin sets `wizard_open` + the display fields;
    /// the panel writes the player's click into `wizard_action`
    /// (0=none,1=Back,2=Next,3=Create,4=Cancel,5=cycle Mode,6=cycle Seed).
    pub wizard_open: bool,
    pub wizard_action: u32,
    /// r179: world-name edit buffer (wizard); bin reads on action 7.
    pub wizard_name_buf: String,
    /// r181: seed entry buffer; bin parses on action 8.
    pub wizard_seed_buf: String,
    /// r181: 96x48 RGB top-down seed preview (+dirty flag); bin fills it.
    pub wizard_preview_rgb: Vec<u8>,
    pub wizard_preview_dirty: bool,
    /// r185: click on the preview — (u,v in 0..1, negative?) latch.
    pub wizard_preview_click: Option<(f32, f32, bool)>,
    wizard_preview_tex: Option<egui::TextureHandle>,
    pub wizard_title: String,
    pub wizard_summary: String,
    pub wizard_can_next: bool,
    pub wizard_at_confirm: bool,
    /// r186: 0=title 1=confirm-quit 2=multiplayer (display-only).
    pub menu_screen_kind: u8,
    /// r206: rollback-point index to delete (bin consumes)
    pub menu_rollback_delete: i32,
    /// r206: dev button — force a rollback/history point now (bin consumes)
    pub dev_make_history_point: bool,
    /// r205: confirmed world index to delete (bin consumes)
    pub menu_world_delete_confirm: i32,
    /// r205: world index pending delete-confirmation, -1 none
    pub menu_world_delete_ask: i32,
    /// r205: tracks wizard-open edge so the name field can auto-focus on first frame
    pub wizard_was_open: bool,
    /// r205: vertical scroll wheel delta this frame, forwarded to egui as MouseWheel
    pub ui_scroll_delta: f32,
    /// r318: horizontal scroll accumulator (trackpad); forwarded to egui too.
    pub ui_scroll_delta_x: f32,
    /// r203: rollback-point index to restore (bin consumes)
    pub menu_rollback_restore: i32,
    /// r203: world index whose rollback history to load (bin consumes)
    pub menu_rollback_peek: i32,
    /// r203: when true the Load screen shows the selected world's rollback points
    pub menu_show_rollbacks: bool,
    /// r203: rollback-point rows (ts/reason/chunks) for the restore browse panel
    pub menu_rollback_list: Vec<String>,
    /// r199: action index currently awaiting a key (bin sets; shows 'press a key')
    pub keybind_capturing: i32,
    /// r199: action index the user clicked to rebind (consumed by bin), -1=none
    pub keybind_rebind_request: i32,
    /// r199: (action label, key label) rows for the Controls rebind page
    pub menu_keybind_list: Vec<(String, String)>,
    /// r197: live render-distance slider (writes RENDER_DISTANCE_OVERRIDE)
    pub ui_render_distance: i32,
    /// r325: render resolution scale (neural upscale), 0.5..1.0.
    pub ui_render_scale: f32,
    /// r197: clicked saved-world index (consumed by bin), -1 = none
    pub menu_world_pick: i32,
    /// r197: saved-world names+meta for the LoadSlots screen list
    pub menu_world_list: Vec<String>,
    /// r394: discovered LAN multiplayer hosts (display labels) for the MP screen
    pub menu_mp_list: Vec<String>,
    /// r397: clicked LAN host index (consumed by bin), -1 = none
    pub menu_mp_pick: i32,

    pub camera: FlyCamera,

    /// Shadow-mapping infrastructure (Phase 1 v0.13: groundwork only).
    ///
    /// The shadow pre-pass runs each frame BEFORE the main color pass,
    /// drawing the world's chunk meshes into a 2048×2048 depth-only
    /// image via `shadow_pipeline`. The main pass's fragment shader
    /// will sample that image to determine which fragments are
    /// shadowed — but as of v0.13 the fragment shader does not yet
    /// sample (the descriptor binding doesn't exist yet either; the
    /// shadow map is rendered, the matrix is uploaded, but consumption
    /// is filed for v0.14). This split is intentional: each session
    /// is shippable; v0.13 lays the groundwork without changing the
    /// visible image.
    shadow: ShadowMap,
    shadow_pipeline: ShadowPipeline,
    /// Volumetric scattering froxel grid. Filled per-frame by a
    /// compute shader (`froxel_fill_pipeline`) before the main
    /// color pass; sampled by the chunk fragment shader to provide
    /// shadowed in-scattered fog with structure.
    froxel: FroxelVolume,
    /// Compute pipeline that fills the froxel volume each frame.
    froxel_fill_pipeline: FroxelFillPipeline,
    /// One descriptor set per frame-in-flight for the compute pass.
    /// Distinct from the graphics descriptor sets because the compute
    /// pass uses a different descriptor set layout (storage image
    /// instead of sampled image at binding 2; all bindings are
    /// COMPUTE-stage rather than VERTEX/FRAGMENT).
    froxel_compute_descriptor_sets: Vec<vk::DescriptorSet>,
    /// Latest computed light cascade (one for now; multi-cascade
    /// expands this to `[LightCascade; 4]` in a future increment).
    /// Computed each frame in `draw_frame_with_view_and_atmos` from
    /// the sun direction + camera View. Cached on the renderer so
    /// the fragment-shader sampling path in v0.14 has it available
    /// without recomputing.
    last_cascade: Option<LightCascade>,

    /// v0.24.4 (shadow-stride): monotonic frame counter, incremented
    /// at the start of every `draw_frame_with_view_and_atmos` call.
    /// Used by the shadow cull's stride/phase rotation when
    /// `UNENDLICH_SHADOW_CHUNK_STRIDE > 1`. Independent of the FIF
    /// slot index (which is fixed at 0 when MAX_FRAMES_IN_FLIGHT=1).
    monotonic_frame: u64,

    /// v0.24.4 (shadow-stride): runtime stride for the shadow cull,
    /// resolved once at startup from UNENDLICH_SHADOW_CHUNK_STRIDE.
    /// Default 1 = render all chunks every frame (no temporal
    /// amortization). > 1 = render only 1/N chunks per frame and
    /// rely on the shadow map preserving previous frames' depth.
    shadow_chunk_stride: u32,

    /// v0.24.NEXT (shadow-cache): when enabled, skip the entire shadow render
    /// pass on frames where the cascade view-projection is bit-identical to the
    /// last frame the shadow map was rendered — i.e. sun AND camera are static,
    /// so the shadow map content is unchanged and can be reused. Reuses the
    /// persistent shadow image (left in SHADER_READ_ONLY by the render pass's
    /// own subpass dependency), so the main pass still samples valid shadows.
    /// THE lever for the dominant GPU cost (shadow p50≈49ms): when the player
    /// stands still, shadow drops to ~0. `UNENDLICH_SHADOW_CACHE`, default off.
    /// Tradeoff: a chunk that finishes loading while the camera is perfectly
    /// still won't cast a shadow until the next camera move.
    shadow_cache: bool,
    /// The cascade view-projection at the last frame the shadow map was
    /// actually rendered. Compared against the current frame's to decide reuse.
    last_shadow_rendered_vp: Option<glm::Mat4>,

    /// v0.24.4 (shadow-stride): the previous frame's cascade center
    /// in world space. When the cascade origin shifts by more than
    /// one chunk-width between frames (fast camera motion), the
    /// shadow stride is bypassed for that frame (full render) so
    /// stale chunks don't smear into now-incorrect texel positions.
    /// None = no previous frame yet; first frame always forces full.
    last_cascade_center: Option<glm::Vec3>,

    /// Background clear color in linear-sRGB-ish space, RGBA. The
    /// sky color the player sees beyond loaded geometry. Tracked
    /// dynamically by the bin (the bin reads `Sky` and computes the
    /// horizon color, then writes to this field) so the world's
    /// "void" beyond the render distance feels like atmosphere
    /// rather than a hard background. Defaults to sky-blue for
    /// backward compat with bins that don't update it.
    pub clear_color: [f32; 4],

    /// Per-chunk uploaded meshes. Keyed by chunk coordinate. The mesh
    /// data lives in `mesh_pool`; this map just records which slot
    /// each chunk owns.
    /// v0.22.0 (F1): mesh slot table keyed by `(coord, lod)` so a
    /// chunk can hold one mesh per LOD level. In F1 every entry has
    /// `lod = LodLevel::Near` (call sites use `MeshSlotKey::near`),
    /// so external behaviour matches v0.21.x exactly. Future
    /// increments (F2+) populate non-Near entries as the LOD-aware
    /// mesher wires up.
    meshes: HashMap<crate::world::lod::MeshSlotKey, UploadedMesh>,

    /// v0.24.72: tracks (coord, lod) keys that have been meshed
    /// successfully AND found empty. Without this, all-air chunks
    /// (and any chunk whose greedy-meshing emits zero faces, e.g.,
    /// fully-buried stone chunks with no exposed face) get
    /// re-meshed every dispatch round because `meshes` doesn't
    /// store empty results — the dispatcher's `mesh_version_for`
    /// check then keeps returning None, triggering an endless
    /// re-mesh loop.
    ///
    /// Semantics: `empty_mesh_versions[key] = V` means the mesher
    /// ran on chunk(key) at version V and produced an empty mesh.
    /// We can skip re-meshing while `chunk.version == V`.
    ///
    /// Cleanup: when a non-empty mesh is uploaded for the same key,
    /// the entry here is removed (the chunk transitioned to having
    /// geometry, so the stale "empty at v" record would just
    /// confuse future checks if we left it).
    ///
    /// Eviction: stale entries for unloaded chunks are harmless
    /// (no chunk exists to compare against). Periodic cleanup
    /// could prune them but isn't currently worth the complexity —
    /// each entry is ~24 bytes, ~1000 entries = ~24KB.
    empty_mesh_versions: HashMap<crate::world::lod::MeshSlotKey, u64>,

    /// Pool of pre-allocated mesh buffers. Eliminates per-chunk
    /// `vkAllocateMemory` calls that were causing whole-screen freezes
    /// during chunk streaming bursts (v0.18.2 fix).
    mesh_pool: crate::render::mesh_buffer_pool::VulkanMeshPool,

    /// v0.24.0a: per-view GPU buffer. Holds frustum planes (and
    /// eventually matrices, populated by GPU compute in v0.24.0b) for
    /// each rendering view. The cull shader reads from
    /// `view_params[push.view_index].planes` instead of getting them
    /// inline via push constant. Must outlive `indirect` because
    /// `indirect`'s descriptor set holds a reference to this buffer
    /// at binding 3 — destroyed in `destroy()` AFTER `indirect`.
    view_params: crate::render::view_params::ViewParamsBuffer,

    /// v0.24.0b: GPU compute pass that derives `view_proj`,
    /// `inverse_view_proj`, and frustum planes from CPU-written
    /// `view` and `projection` matrices in each ViewParam slot.
    /// Dispatched per active view before the cull pass each frame.
    /// Owns its own descriptor set bound to `view_params.buffer`;
    /// must be destroyed before `view_params` (same constraint
    /// as `indirect`'s descriptor pool).
    view_compute_pipeline: crate::render::view_compute_pipeline::ViewComputePipeline,

    /// v0.24.2b: GPU compute pass that derives one shadow cascade's
    /// (`view`, `projection`) pair from camera basis + sun + split
    /// distances. Dispatched once per cascade per frame, then
    /// view_compute_pipeline derives planes from the result. Same
    /// descriptor wiring as view_compute_pipeline; destroyed before
    /// `view_params`.
    cascade_fit_pipeline: crate::render::cascade_fit_pipeline::CascadeFitPipeline,

    /// Indirect drawing infrastructure (v0.20). Holds the chunk metadata
    /// SSBO + indirect command buffer + cull pipeline. Each frame we run
    /// one compute dispatch (frustum cull → emit draw commands) and one
    /// `vkCmdDrawIndexedIndirectCount` per pass, replacing the per-chunk
    /// draw loop that was the steady-state bottleneck.
    indirect: crate::render::chunk_indirect::IndirectDrawSet,

    /// v0.21.0: per-pass GPU timing via Vulkan timestamp queries.
    /// Surfaced in the FPS log alongside CPU timers so we can see
    /// where the GPU is actually spending time. See
    /// `crate::render::gpu_timer` for the design.
    gpu_timer: crate::render::gpu_timer::GpuTimer,

    /// v0.23.0: CPU timer for the render thread. Tracks wall-clock
    /// time spent in each draw-frame zone (wait_fences, acquire,
    /// record, submit, present). Surfaced in the per-second log
    /// alongside `gpu_timer` and dumped on shutdown. See
    /// `crate::render::cpu_timer` for the design.
    cpu_timer: crate::render::cpu_timer::CpuTimer,

    /// c.13-instrumentation: optional auxiliary timer that subdivides
    /// the shadow pass into per-chunk-group buckets, for diagnosing
    /// whether shadow cost is front/middle/back-loaded across the
    /// chunk draw list. Only Some(_) when `UNENDLICH_SHADOW_BUCKETS`
    /// is set. Forces the per-chunk fallback path (indirect drawing
    /// can't accept timestamps mid-list).
    shadow_bucket_timer: Option<crate::render::shadow_bucket_timer::ShadowBucketTimer>,

    /// v0.20.1: kill-switch for the GPU-driven indirect path. When false,
    /// falls back to v0.19's per-chunk CPU draw loop. Set via env var
    /// `UNENDLICH_INDIRECT=0` (default ON). Useful for diagnosing
    /// regressions in the indirect path — ship a known-bad release with
    /// this flag and the user can A/B compare.
    use_indirect: bool,

    /// Counter for upload failures since startup. Surfaced in FPS logs
    /// so silent drops are visible. Resets never; a non-zero value over
    /// time suggests pool exhaustion or other allocation issues.
    upload_fail_count: u64,

    /// headlesshorseman--T3: per-frame cached visibility frustum for
    /// `chunk_visible`. The view and extent are constant within a frame, so the
    /// frustum (one matrix build + 6-plane extract) is rebuilt only when the
    /// `(view, extent)` key changes — once per frame in steady state — instead
    /// of on every per-chunk visibility test. Plain POD, so the `unsafe impl
    /// Send` above stays valid.
    cached_frustum: Frustum,
    cached_frustum_key: Option<FrustumKey>,

    /// v0.24.63-adaptive-uploads: most-recent wait_fences measurement
    /// in milliseconds. Captured at the top of `draw_frame` right
    /// after `wait_for_fences`. Used by the bin's adaptive upload
    /// controller to throttle uploads when the GPU is saturated;
    /// also useful as a per-frame telemetry value without going
    /// through the cpu_zone aggregator.
    ///
    /// Read-only side-channel; no rendering decisions depend on this
    /// field within the renderer itself. Value is 0.0 before the
    /// first frame completes wait_for_fences.
    last_wait_fences_ms: f64,

    /// Resources awaiting deferred destruction (chunks evicted since last
    /// frame). Drained at the top of each frame.
    pending_destroy: Vec<PendingDestroy>,

    /// v0.24.19: renderer-owned mesh cache. See [`CachedMesh`]. Capacity
    /// is set from `UNENDLICH_MESH_CACHE_SIZE` at construction time;
    /// when 0 (default), the cache is disabled and behaviour matches
    /// pre-v0.24.19 exactly.
    mesh_cache: HashMap<crate::world::lod::MeshSlotKey, CachedMesh>,
    mesh_cache_capacity: usize,
    /// Monotonic counter for LRU ordering. Bumped on every cache touch.
    mesh_cache_seq: u64,
    /// Lifetime hit counter — number of revisits that recovered buffers
    /// from the cache instead of doing a fresh upload. Surfaced in the
    /// per-window FPS log.
    mesh_cache_hits: u64,
    /// Lifetime miss counter — number of (coord, lod) revisits where a
    /// cache entry existed but its version was stale (chunk was edited).
    /// These cost a cache slot AND a fresh upload, the worst case.
    /// Compared to `hits` tells the user whether the cache is paying
    /// off for their playstyle.
    mesh_cache_misses: u64,
    /// v0.24.20-vertex-pack: lifetime count of times `try_cache_park`
    /// was called (i.e., a chunk was evicted from the active set).
    /// Surfaces "evict-rate" — the upstream signal that determines
    /// whether the cache has anything to do. With size=0 hits=0
    /// misses=0 and park_attempts=0, the playtest never moved enough
    /// to evict chunks; the cache was correct but dormant. With
    /// park_attempts > 0 but size=0, parks happened but everything
    /// got LRU-evicted before re-use (cache too small for the
    /// movement pattern).
    mesh_cache_park_attempts: u64,

    /// v0.24.26-octree-on-gpu: GPU-resident octree node buffer.
    /// Mirrors the CPU-side `OctreeArray` per chunk so future
    /// consumers (mesher, raycast, physics) can read world state
    /// from a single GPU substrate instead of the four fragmented
    /// CPU-extracted representations that v0.24.25 inherited
    /// (dense BlockPool, SurfaceRecord[], SurfaceRun[], explicit
    /// `Chunk.blocks`). This ship just installs the buffer + the
    /// upload path; no production consumer migrates yet.
    /// See `crate::render::octree_buffer` for the design.
    ///
    /// v0.24.30-shared-octree: the pool is now wrapped in
    /// `Arc<Mutex>` so the `OctreeGpuMesher` can read from the
    /// same buffer instead of maintaining its own (100 MB saved,
    /// plus the mesher now inherits the v0.24.26 probe's coverage).
    /// `WorldRenderer::octree_pool_handle()` returns the shared
    /// reference for the mesher's constructor.
    octree_pool: Arc<Mutex<crate::render::octree_buffer::OctreeBufferPool>>,

    /// v0.24.26-octree-on-gpu: probe pipeline for verifying the
    /// upload format end-to-end. Lazily constructed on first
    /// `run_octree_verify()` call (the binary enables this via
    /// `UNENDLICH_OCTREE_VERIFY=1`). Holds its own queue submit
    /// lock and host-visible IO buffers; see
    /// `crate::render::octree_probe_pipeline` for the design.
    octree_probe: Option<crate::render::octree_probe_pipeline::OctreeProbePipeline>,

    /// v0.24.26-octree-on-gpu: cached graphics queue handle + family
    /// index, kept so we can lazily construct the probe pipeline
    /// without threading `VulkanContext` everywhere. Set by
    /// `WorldRenderer::new`; never mutated.
    octree_probe_queue: vk::Queue,
    octree_probe_queue_family: u32,

    /// v0.24.26-octree-on-gpu: cached physical device handle for the
    /// lazy probe construction. Same rationale as the queue fields.
    octree_probe_physical_device: vk::PhysicalDevice,
}

// SAFETY: The raw `*mut u8` UBO pointers are persistent maps into
// HOST_VISIBLE + HOST_COHERENT memory. They're written exclusively by the
// render thread (which owns &mut WorldRenderer). The renderer is not used
// across threads.
unsafe impl Send for WorldRenderer {}

/// v0.24.26-octree-on-gpu: report from
/// [`WorldRenderer::run_octree_verify`]. Aggregates how many sample
/// queries matched CPU truth and pins the first mismatch for
/// debugging.
///
/// The verify path's contract is: `mismatched == 0` iff the GPU
/// octree wire format and the GLSL `octree_sample()` agree with the
/// CPU `OctreeArray::sample`. Any non-zero mismatched value means
/// the format has drifted and a consumer migration that depends on
/// it would silently corrupt world reads.
#[derive(Debug, Default, Clone)]
pub struct OctreeVerifyReport {
    /// Total queries dispatched across all batches.
    pub queries: u64,
    /// Queries whose GPU result matched the CPU `OctreeArray::sample`.
    pub matched: u64,
    /// Queries whose GPU result differed. CI gates on this being 0.
    pub mismatched: u64,
    /// First mismatch (if any), useful for debugging:
    /// `(coord, chunk-local xyz, gpu_value, cpu_value)`.
    pub first_mismatch: Option<(ChunkCoord, [u32; 3], u32, u32)>,
    /// Chunks examined.
    pub chunks_examined: u64,
    /// Chunks skipped because the renderer hadn't uploaded their
    /// octree yet (still streaming, evicted between snapshot and
    /// dispatch, or upload failed).
    pub chunks_skipped: u64,
}

/// v0.24.26-octree-on-gpu: deterministic sample positions for the
/// verify path. Covers the 32³ chunk volume with corners, edge
/// midpoints, face centers, and a few interior cells so any
/// child-bit / depth-handling error surfaces quickly.
///
/// Returns `count` positions; if `count` is larger than the fixed
/// pattern below, repeats it (probe is robust to duplicate queries —
/// CPU and GPU both walk the same tree to the same answer).
fn generate_probe_sample_positions(count: u32) -> Vec<[u32; 3]> {
    // Pattern of 24 positions: 8 corners + 6 face centers + 4 random
    // interior + 6 mid-edge points. Hand-picked rather than RNG so
    // verify runs are bit-reproducible across machines.
    const PATTERN: &[[u32; 3]] = &[
        // 8 corners (32 is out-of-range; clamp to 31 for valid index)
        [0, 0, 0], [31, 0, 0], [0, 31, 0], [31, 31, 0],
        [0, 0, 31], [31, 0, 31], [0, 31, 31], [31, 31, 31],
        // 6 face centers
        [16, 16, 0], [16, 16, 31], [16, 0, 16], [16, 31, 16],
        [0, 16, 16], [31, 16, 16],
        // 4 interior "random" cells
        [5, 9, 23], [11, 27, 4], [29, 3, 17], [22, 19, 14],
        // 6 mid-edge points
        [16, 0, 0], [0, 16, 0], [0, 0, 16],
        [16, 31, 31], [31, 16, 31], [31, 31, 16],
    ];
    (0..count as usize).map(|i| PATTERN[i % PATTERN.len()]).collect()
}

/// v0.24.19: pick the LRU-eligible key from a list of
/// `(key, last_touched)` pairs. Free function so unit tests can
/// exercise it without a full WorldRenderer (which would require a
/// Vulkan device).
///
/// Returns `None` if the input is empty. The key with the SMALLEST
/// `last_touched` value wins. Ties resolve to the first encountered
/// key — that's fine since real `last_touched` values are unique
/// (every cache touch bumps `mesh_cache_seq`).
fn cache_lru_key(
    touches: &[(crate::world::lod::MeshSlotKey, u64)],
) -> Option<crate::world::lod::MeshSlotKey> {
    touches.iter()
        .min_by_key(|(_, t)| *t)
        .map(|(k, _)| *k)
}

impl WorldRenderer {
    pub fn new(
        ctx: &VulkanContext,
        framebuffer_size: (u32, u32),
        camera: FlyCamera,
    ) -> Result<Self> {
        let extent = vk::Extent2D { width: framebuffer_size.0, height: framebuffer_size.1 };

        // Color-only swapchain first to learn its format.
        let initial_render_pass = crate::render::swapchain::create_render_pass(
            &ctx.device, vk::Format::B8G8R8A8_SRGB,
        )?;
        let mut swapchain = Swapchain::new(ctx, initial_render_pass, extent)?;
        unsafe { ctx.device.destroy_render_pass(initial_render_pass, None); }

        let depth = DepthBuffer::new(&ctx.instance, &ctx.device, ctx.physical_device, swapchain.extent)?;
        let render_pass = create_render_pass_with_depth(&ctx.device, swapchain.format, depth.format)?;
        swapchain.rebuild_framebuffers_with_depth(ctx, render_pass, depth.view)?;

        let pipeline = ChunkPipeline::new(&ctx.device, render_pass)?;

        // r306: build the neural G-buffer pass only when explicitly enabled. It
        // reuses the chunk pipeline's layout so the same per-frame descriptor set
        // binds against it. Default-off ⇒ the rest of the renderer is unchanged.
        let gbuffer = if std::env::var("UNENDLICH_NEURAL_GBUFFER").ok().as_deref() != Some("0") {
            log::info!(
                "[neural] UNENDLICH_NEURAL_GBUFFER=1 — building parallel G-buffer pass \
                 (inert producer: fills normal/albedo/worldpos, nothing samples them yet)"
            );
            Some(crate::render::gbuffer::NeuralGBuffer::new(
                &ctx.instance, &ctx.device, ctx.physical_device,
                swapchain.extent, pipeline.layout,
            )?)
        } else {
            None
        };

        // r307: the feature pass is built iff the G-buffer is (same gate). It samples
        // the G-buffer images, so it shares the swapchain extent for its buffer.
        let neural_feat = if gbuffer.is_some() {
            Some(crate::render::neural_feat_pipeline::NeuralFeatPipeline::new(
                &ctx.instance, &ctx.device, ctx.physical_device, swapchain.extent,
            )?)
        } else {
            None
        };

        // r308: background MLP trainer. Independent flag — it's a CPU thread that
        // needs no GPU resources; it idles until the ground-truth sample feed is
        // wired. Bakes Q8 weights the shade pass can poll + hot-swap.
        let neural_trainer = if std::env::var("UNENDLICH_NEURAL_TRAIN").ok().as_deref() != Some("0") {
            log::info!(
                "[neural] UNENDLICH_NEURAL_TRAIN=1 — background trainer thread spawned \
                 (idle until the ground-truth sample feed is wired)"
            );
            Some(crate::render::neural_train::BackgroundTrainer::spawn(
                0,
                crate::render::neural_train::TrainCfg::default(),
            ))
        } else {
            None
        };

        // r309: one-flag trainer validation. CPU-only, runs once at startup and logs
        // the loss curve — proves the Adam/backprop math learns on this machine,
        // independent of the GPU/render path.
        if std::env::var("UNENDLICH_NEURAL_TRAIN_SELFTEST").ok().as_deref() == Some("1") {
            crate::render::neural_train::TrainMlp::self_test(2000);
        }
        let entity_pipeline = EntityPipeline::new(
            &ctx.device, render_pass, pipeline.descriptor_set_layout,
            unsafe { ctx.instance.get_physical_device_memory_properties(ctx.physical_device) },
        )?;

        let command_pool = unsafe {
            ctx.device.create_command_pool(
                &vk::CommandPoolCreateInfo::builder()
                    .queue_family_index(ctx.graphics_queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?
        };

        let cmd_buffers = unsafe {
            ctx.device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::builder()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(MAX_FRAMES_IN_FLIGHT as u32),
            )?
        };

        let descriptor_pool = unsafe {
            // Pool sizes:
            //   per graphics frame: 1 UBO + 2 combined samplers (shadow, froxel)
            //     + 1 storage buffer (chunks SSBO, v0.24.20-vertex-pack)
            //   per compute frame:  1 UBO + 1 combined sampler (shadow) + 1 storage image (froxel)
            // Total per frame: 2 UBO + 3 sampler + 1 storage image + 1 storage buffer.
            // Sets per frame: 2 (graphics + compute).
            let sizes = [
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::UNIFORM_BUFFER,
                    descriptor_count: (MAX_FRAMES_IN_FLIGHT * 2) as u32,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                    descriptor_count: (MAX_FRAMES_IN_FLIGHT * 3) as u32,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::STORAGE_IMAGE,
                    descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::STORAGE_BUFFER,
                    descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
                },
            ];
            ctx.device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::builder()
                    .pool_sizes(&sizes)
                    .max_sets((MAX_FRAMES_IN_FLIGHT * 2) as u32),
                None,
            )?
        };

        let dsl = pipeline.descriptor_set_layout;
        let layouts: Vec<vk::DescriptorSetLayout> = vec![dsl; MAX_FRAMES_IN_FLIGHT];
        let descriptor_sets = unsafe {
            ctx.device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::builder()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&layouts),
            )?
        };

        // Shadow infrastructure created BEFORE the frames loop so the
        // descriptor-set writes below can bind binding 1 (the shadow
        // map sampler) immediately. The shadow image view + sampler
        // are stable for the renderer's lifetime — the depth contents
        // get overwritten each frame in the shadow pre-pass, but the
        // resource handles don't change. So all frames-in-flight
        // reference the same shadow image. (Multi-cascade or temporal
        // shadow techniques would change this.)
        let shadow = ShadowMap::new(&ctx.instance, &ctx.device, ctx.physical_device)?;
        let shadow_pipeline = ShadowPipeline::new(&ctx.device, shadow.render_pass)?;

        // Froxel volume — v0.16 groundwork. Allocated, cleared to
        // sentinel (alpha=0), bound at descriptor binding 2 below.
        // No per-frame fill yet; fragment shader detects sentinel
        // and falls back to the per-fragment march.
        let froxel = FroxelVolume::new(&ctx.instance, &ctx.device, ctx.physical_device)?;
        froxel.clear_to_sentinel(&ctx.device, command_pool, ctx.graphics_queue)?;

        // Froxel fill compute pipeline. Will be dispatched per-frame
        // before the main color pass to write per-froxel scattering
        // into the volume's storage image.
        let froxel_fill_pipeline = FroxelFillPipeline::new(&ctx.device)?;

        // Allocate compute descriptor sets — one per frame-in-flight.
        let compute_dsl = froxel_fill_pipeline.descriptor_set_layout;
        let compute_layouts: Vec<vk::DescriptorSetLayout> = vec![compute_dsl; MAX_FRAMES_IN_FLIGHT];
        let froxel_compute_descriptor_sets = unsafe {
            ctx.device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::builder()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&compute_layouts),
            )?
        };

        // r476: frames in flight default 2 (CPU/GPU overlap — the single biggest
        // smoothness win on Asahi). Metadata SSBO is double-buffered so >1 is
        // crash-safe. Override with UNENDLICH_FRAMES_IN_FLIGHT (=1 old behavior).
        let frames_in_flight: usize = std::env::var("UNENDLICH_FRAMES_IN_FLIGHT")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .map(|n| n.clamp(1, MAX_FRAMES_IN_FLIGHT))
            .unwrap_or(2);
        log::info!(
            "WorldRenderer: frames_in_flight = {} (cap {}; UNENDLICH_FRAMES_IN_FLIGHT)",
            frames_in_flight, MAX_FRAMES_IN_FLIGHT
        );
        let mut frames = Vec::with_capacity(frames_in_flight);
        for i in 0..frames_in_flight {
            let image_available = unsafe { ctx.device.create_semaphore(&vk::SemaphoreCreateInfo::builder(), None)? };
            let render_finished = unsafe { ctx.device.create_semaphore(&vk::SemaphoreCreateInfo::builder(), None)? };
            let in_flight = unsafe {
                ctx.device.create_fence(
                    &vk::FenceCreateInfo::builder().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )?
            };

            let (ubo_buffer, ubo_memory, ubo_ptr) = create_ubo(ctx)?;

            let buffer_info = vk::DescriptorBufferInfo::builder()
                .buffer(ubo_buffer)
                .offset(0)
                .range(std::mem::size_of::<CameraUbo>() as u64);
            let buffer_infos = [*buffer_info];

            // Image info for the shadow map binding. The image is in
            // DEPTH_STENCIL_READ_ONLY_OPTIMAL layout after each shadow
            // render pass (its subpass dependency transitions it
            // there), which is the right layout for sampling.
            let shadow_image_info = vk::DescriptorImageInfo::builder()
                .image_view(shadow.view)
                .sampler(shadow.sampler)
                .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL);
            let shadow_image_infos = [*shadow_image_info];

            // Image info for the froxel volume binding. The volume
            // sits in GENERAL layout (transitioned by clear_to_sentinel
            // at startup). GENERAL is required because the future
            // fill path will be a compute shader writing into the
            // same image — compute storage writes need GENERAL.
            // Sampling from GENERAL is also valid.
            let froxel_image_info = vk::DescriptorImageInfo::builder()
                .image_view(froxel.view)
                .sampler(froxel.sampler)
                .image_layout(vk::ImageLayout::GENERAL);
            let froxel_image_infos = [*froxel_image_info];

            let writes = [
                vk::WriteDescriptorSet::builder()
                    .dst_set(descriptor_sets[i])
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&buffer_infos)
                    .build(),
                vk::WriteDescriptorSet::builder()
                    .dst_set(descriptor_sets[i])
                    .dst_binding(1)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&shadow_image_infos)
                    .build(),
                vk::WriteDescriptorSet::builder()
                    .dst_set(descriptor_sets[i])
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&froxel_image_infos)
                    .build(),
            ];
            unsafe { ctx.device.update_descriptor_sets(&writes, &[]); }

            // Compute descriptor set for the froxel fill pass:
            // binding 0 = same UBO, binding 1 = shadow sampler,
            // binding 2 = froxel volume as STORAGE_IMAGE (not as a
            // sampled image — compute writes into it).
            let froxel_storage_info = vk::DescriptorImageInfo::builder()
                .image_view(froxel.view)
                .image_layout(vk::ImageLayout::GENERAL);
            let froxel_storage_infos = [*froxel_storage_info];

            let compute_writes = [
                vk::WriteDescriptorSet::builder()
                    .dst_set(froxel_compute_descriptor_sets[i])
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&buffer_infos)
                    .build(),
                vk::WriteDescriptorSet::builder()
                    .dst_set(froxel_compute_descriptor_sets[i])
                    .dst_binding(1)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&shadow_image_infos)
                    .build(),
                vk::WriteDescriptorSet::builder()
                    .dst_set(froxel_compute_descriptor_sets[i])
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .image_info(&froxel_storage_infos)
                    .build(),
            ];
            unsafe { ctx.device.update_descriptor_sets(&compute_writes, &[]); }

            frames.push(FrameSync {
                image_available, render_finished, in_flight,
                command_buffer: cmd_buffers[i],
                ubo_buffer, ubo_memory, ubo_ptr,
                descriptor_set: descriptor_sets[i],
                last_submit_at: None,
            });
        }

        // Mesh buffer pool. v0.19 sub-allocator design: two big GPU
        // buffers (vertex + index), variable-size span allocation per
        // chunk via the free-list allocator in `mesh_buffer_alloc`.
        // Defaults: 3 GB vertex + 2 GB index. Sized to comfortably hold
        // a render-distance-6 working set with mesh sizes up to 7 MB
        // observed in the v0.18.x playtest.
        let mesh_pool = unsafe {
            crate::render::mesh_buffer_pool::VulkanMeshPool::new(
                &ctx.device, &ctx.instance, ctx.physical_device,
            )
        }.map_err(|e| WorldRenderError::Other(
            format!("Failed to create mesh buffer pool: {}", e)
        ))?;

        // v0.24.26-octree-on-gpu: GPU-resident octree node pool. One
        // HOST_VISIBLE storage buffer with span-allocated per-chunk
        // slices. No production consumer this ship; the buffer
        // exists so the next ship can migrate the mesher to read
        // octree-canonical world state instead of CPU-extracted
        // SurfaceRecord[]. The probe pipeline is constructed lazily
        // on first verify invocation (see `run_octree_verify`).
        //
        // v0.24.30-shared-octree: wrap in Arc<Mutex> so the
        // `OctreeGpuMesher` can share read-and-upload access. The
        // lock duration is bounded by a single mesh-batch dispatch
        // (~1 ms) which contends with main-thread per-chunk
        // uploads (~10 µs each, ~16 per frame). Expected wait per
        // upload is ~30 µs; acceptable for foundation. A deferred-
        // free pattern would eliminate the eviction race entirely
        // and is filed as a follow-up.
        let octree_pool = Arc::new(Mutex::new(unsafe {
            crate::render::octree_buffer::OctreeBufferPool::new(
                &ctx.device, &ctx.instance, ctx.physical_device,
            )
        }.map_err(|e| WorldRenderError::Other(
            format!("Failed to create octree buffer pool: {}", e)
        ))?));

        // v0.24.0a: per-view buffer. Allocate before IndirectDrawSet so
        // we can hand its buffer handle in for descriptor binding 3.
        let view_params = unsafe {
            crate::render::view_params::ViewParamsBuffer::new(
                &ctx.device, &ctx.instance, ctx.physical_device,
            )
        }.map_err(|e| WorldRenderError::Other(
            format!("Failed to create ViewParams buffer: {:?}", e)
        ))?;

        // Indirect drawing infrastructure (v0.20). Owns the chunk metadata
        // SSBO + indirect command buffer + cull pipeline. See
        // chunk_indirect.rs for the design.
        let indirect = unsafe {
            crate::render::chunk_indirect::IndirectDrawSet::new(
                &ctx.device, &ctx.instance, ctx.physical_device,
                view_params.buffer,
                frames_in_flight,
            )
        }.map_err(|e| WorldRenderError::Other(
            format!("Failed to create indirect draw set: {}", e)
        ))?;

        // v0.24.20-vertex-pack: bind the chunks-metadata SSBO at
        // descriptor binding 3 for both the main chunk pipeline and
        // (via the shared per-frame descriptor set) the shadow pipeline.
        // The vertex shader reads `chunks[gl_InstanceIndex].aabb_min` to
        // reconstruct each vertex's world position from its chunk-local
        // packed form. The chunks SSBO is the same buffer the cull
        // shader reads at binding 0 of its own descriptor set — one
        // physical buffer, two distinct descriptor-set views.
        //
        // Done in a second pass (after `indirect` is constructed, after
        // the per-frame descriptor sets in `frames` already have
        // bindings 0/1/2 written above) so the construction order
        // doesn't have to be rearranged. Each descriptor-set update is
        // idempotent and doesn't disturb the previously-written
        // bindings.
        for (i, frame) in frames.iter().enumerate() {
            let chunks_buffer_info = vk::DescriptorBufferInfo::builder()
                .buffer(indirect.metadata_buffer(i))
                .offset(0)
                .range(indirect.metadata_buffer_size());
            let chunks_buffer_infos = [*chunks_buffer_info];
            let write = vk::WriteDescriptorSet::builder()
                .dst_set(frame.descriptor_set)
                .dst_binding(3)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&chunks_buffer_infos)
                .build();
            unsafe { ctx.device.update_descriptor_sets(&[write], &[]); }
        }

        // v0.24.0b: GPU view-derivation pipeline. Reads CPU-written
        // view+projection matrices from a ViewParam slot, derives
        // view_proj/inverse_view_proj/frustum planes back into the
        // same slot. Dispatched once per active view per frame,
        // before the cull pass.
        let view_compute_pipeline = unsafe {
            crate::render::view_compute_pipeline::ViewComputePipeline::new(
                &ctx.device, view_params.buffer,
            )
        }.map_err(|e| WorldRenderError::Other(
            format!("Failed to create view-compute pipeline: {}", e)
        ))?;

        // v0.24.2b: GPU cascade-fit pipeline. Computes one cascade's
        // view+projection from camera basis + sun direction + split
        // distances. Writes into a ViewParam slot specified per
        // dispatch. Dispatched once per cascade per frame.
        let cascade_fit_pipeline = unsafe {
            crate::render::cascade_fit_pipeline::CascadeFitPipeline::new(
                &ctx.device, view_params.buffer,
            )
        }.map_err(|e| WorldRenderError::Other(
            format!("Failed to create cascade-fit pipeline: {}", e)
        ))?;

        // Indirect drawing kill-switch. UNENDLICH_INDIRECT=0 falls back
        // to the v0.19 per-chunk draw loop (preserved as a fallback for
        // diagnostic A/B testing).
        let use_indirect = std::env::var("UNENDLICH_INDIRECT")
            .map(|v| v != "0")
            .unwrap_or(true);
        if !use_indirect {
            log::info!("UNENDLICH_INDIRECT=0: using v0.19 per-chunk draw loop fallback");
        } else {
            log::info!("Indirect drawing ENABLED (set UNENDLICH_INDIRECT=0 for fallback)");
        }

        // v0.24.4 (shadow-stride): temporal amortization of shadow
        // rendering. UNENDLICH_SHADOW_CHUNK_STRIDE=N means each frame
        // we render only chunks where `slot_index % N == frame % N`,
        // rotating the residue class each frame so all chunks get
        // refreshed over N frames. The shadow map is NOT cleared
        // between frames; old depth from previous frames remains valid
        // for unrendered chunks (texel-snap math anchors each chunk's
        // contribution to fixed shadow texels).
        //
        // Default 1 = render all chunks every frame (current behavior).
        // 2-16 = amortize over that many frames. Beyond 16 the staleness
        // becomes visually obvious even at 60+ fps.
        //
        // Fast camera motion forces a full render (see
        // last_cascade_center logic in draw_frame).
        let shadow_chunk_stride: u32 = std::env::var("UNENDLICH_SHADOW_CHUNK_STRIDE")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|&n| (1..=16).contains(&n))
            .unwrap_or(1);
        if shadow_chunk_stride > 1 {
            log::info!(
                "UNENDLICH_SHADOW_CHUNK_STRIDE={}: shadow amortized over {} frames \
                 (depth persists, refreshed round-robin)",
                shadow_chunk_stride, shadow_chunk_stride,
            );
        }

        // v0.21.0: GPU timestamp queries. Created here so it lives
        // alongside the renderer's command buffers; one query pool
        // shared across all frames in flight.
        let gpu_timer = crate::render::gpu_timer::GpuTimer::new(
            &ctx.device,
            &ctx.instance,
            ctx.physical_device,
            ctx.graphics_queue_family,
            MAX_FRAMES_IN_FLIGHT as u32,
        ).map_err(|e| WorldRenderError::Other(
            format!("Failed to create GPU timer: {}", e)
        ))?;

        // c.13-instrumentation: optional shadow bucket timer for
        // diagnosing where the shadow cost lives in the chunk list.
        //
        // c.13-instrumentation-followup: DISABLED. The initial
        // implementation triggered Vulkan ERROR_DEVICE_LOST on
        // Honeykrisp (Asahi Linux, Mesa 25.3.6, May 2026) when writing
        // multiple timestamps inside the shadow render pass with no
        // intervening draw calls on early frames. The crash was
        // reproducible: every bucket-mode run died at ~2 seconds with
        // zero chunks loaded. Pending follow-up to either:
        //   - Move timestamps outside the render pass via N sub-passes
        //     (heavier code change, possibly slower on Honeykrisp but
        //     diagnostic mode tolerates that), OR
        //   - Only emit boundary timestamps between actual draws,
        //     skipping empty-frame timestamps entirely.
        //
        // See docs/DESIGN_NOTES.md "shadow_buckets DEVICE_LOST".
        let shadow_buckets_requested: Option<u32> = std::env::var("UNENDLICH_SHADOW_BUCKETS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|&n| (1..=crate::render::shadow_bucket_timer::MAX_SHADOW_BUCKETS).contains(&n));

        let (shadow_bucket_timer, use_indirect) = if let Some(n) = shadow_buckets_requested {
            log::error!(
                "UNENDLICH_SHADOW_BUCKETS={}: feature DISABLED pending Honeykrisp \
                 investigation. The initial implementation triggered ERROR_DEVICE_LOST \
                 on early frames. See docs/DESIGN_NOTES.md. Continuing without bucketing.",
                n);
            // Don't construct the bucket timer; keep indirect drawing
            // active. The env var is acknowledged-and-ignored rather
            // than silently passing through.
            (None, use_indirect)
        } else {
            (None, use_indirect)
        };

        // In-game egui overlay (hand-rolled). Created against the main render
        // pass so its pipeline is subpass-0 compatible; drawn at pass end.
        #[cfg(feature = "ui-overlay")]
        let egui_overlay = crate::render::egui_overlay::EguiOverlay::new(ctx, render_pass, frames.len())?;

        Ok(Self {
            render_pass, swapchain, depth, pipeline, entity_pipeline, command_pool, descriptor_pool,
            frames, current_frame: 0, frames_in_flight,
            framebuffer_size,
            water_flow_coverage: [0.0; 4],
            water_flow_grid: Box::new([[0.0; 4]; 128]),
            fx_params: [0.0; 4],
            world_season01: 0.0,
            water_depth_cov: [0.0; 4],
            water_depth_grid: Box::new([[0.0; 4]; 64]),
            gbuffer,
            neural_feat,
            neural_trainer,
            #[cfg(feature = "ui-overlay")]
            egui_overlay,
            #[cfg(feature = "ui-overlay")]
            creative_selected: 0,
            creative_hotbar: [None; 10],
            creative_inventory: [None; 27],
            creative_hotbar_sel: 0,
            creative_place_item: None,
            #[cfg(feature = "ui-overlay")]
            creative_tab: crate::inventory_screen::Tab::All,
            #[cfg(feature = "ui-overlay")]
            ui_scale: 2.0,
            #[cfg(feature = "ui-overlay")]
            ui_pointer_pos: (0.0, 0.0),
            #[cfg(feature = "ui-overlay")]
            ui_pointer_down: false,
            ui_pointer2_down: false,
            ui_pointer2_down_prev: false,
            #[cfg(feature = "ui-overlay")]
            ui_pointer_down_prev: false,
            ui_text_input: String::new(),
            ui_backspace: false,
            ui_enter: false,
            ui_wants_keyboard: false,
            #[cfg(feature = "ui-overlay")]
            ui_click_pending: 0,
            #[cfg(feature = "ui-overlay")]
            ui_pointer_active: false,
            settings_open: false,
            hud_status_lines: Vec::new(),
            hud_survival: None,
            backpack: Vec::new(),
            inv_slots: Vec::new(),
            inv_hotbar_size: 9,
            inv_cursor: None,
            inv_click: -1,
            menu_open: false,
            pause_menu_open: false,
            pause_action: 0,
            settings_menu_open: false,
            settings_tab: 0,
            ui_day_length: 1200.0,
            ui_game_speed_pos: crate::sim::game_speed::DEFAULT_POS,
            ui_music_vol: 1.0,
            ui_plant_stride: 4,
            ui_leaf_stride: 2,
            ui_apply_budget: 8,
            ui_perf_preset: 2, // Medium
            ui_fps_target: 500,
            ui_autotune: false,
            ui_gpu_mesh_yield: 0,
            ui_adaptive_rd: false,
            ui_octree_tile: true,
            ui_detached_terrain: true,
            ui_fog_on: true,
            ui_debug_master: false,
            ui_debug_water: false,
            ui_debug_flow: false,
            ui_debug_echo: false,
            ui_shadows_on: true,
            craft_menu_open: false,
            craft_lines: Vec::new(),
            craft_pick: 0,
            character_menu_open: false,
            character_roster: None,
            character_chosen: None,
            perf_window_open: false,
            perf_export_ascii: false,
            loading_fade: 0.0,
            loading_label: String::new(),
            perf_samples: Vec::new(),
            perf_stats: Vec::new(),
            prev_shadows_disabled: false,
            menu_choice: 0,
            wizard_open: false,
            wizard_action: 0,
            wizard_name_buf: "New World".to_string(),
            wizard_seed_buf: String::new(),
            wizard_preview_rgb: Vec::new(),
            wizard_preview_dirty: false,
            wizard_preview_click: None,
            wizard_preview_tex: None,
            wizard_title: String::new(),
            wizard_summary: String::new(),
            wizard_can_next: false,
            wizard_at_confirm: false,
            menu_screen_kind: 0,
            menu_rollback_delete: -1,
            dev_make_history_point: false,
            menu_world_delete_confirm: -1,
            menu_world_delete_ask: -1,
            wizard_was_open: false,
            ui_scroll_delta: 0.0,
            ui_scroll_delta_x: 0.0,
            menu_rollback_restore: -1,
            menu_rollback_peek: -1,
            menu_show_rollbacks: false,
            menu_rollback_list: Vec::new(),
            keybind_capturing: -1,
            keybind_rebind_request: -1,
            menu_keybind_list: Vec::new(),
            ui_render_distance: 6,
            ui_render_scale: 1.0,
            menu_world_pick: -1,
            menu_world_list: Vec::new(),
            menu_mp_list: Vec::new(),
            menu_mp_pick: -1,
            camera,
            shadow,
            shadow_pipeline,
            froxel,
            froxel_fill_pipeline,
            froxel_compute_descriptor_sets,
            last_cascade: None,
            monotonic_frame: 0,
            shadow_chunk_stride,
            shadow_cache: std::env::var("UNENDLICH_SHADOW_CACHE")
                .ok().map(|v| !(v == "0" || v.eq_ignore_ascii_case("false"))).unwrap_or(true), // R27: default ON
            last_shadow_rendered_vp: None,
            last_cascade_center: None,
            clear_color: [0.55, 0.75, 0.95, 1.0],
            meshes: HashMap::new(),
            // v0.24.72: see field doc comment in struct definition.
            empty_mesh_versions: HashMap::new(),
            mesh_pool,
            view_params,
            view_compute_pipeline,
            cascade_fit_pipeline,
            indirect,
            gpu_timer,
            cpu_timer: crate::render::cpu_timer::CpuTimer::new(),
            shadow_bucket_timer,
            use_indirect,
            upload_fail_count: 0,
            cached_frustum: Frustum { planes: [[0.0; 4]; 6] },
            cached_frustum_key: None,
            last_wait_fences_ms: 0.0,
            pending_destroy: Vec::new(),
            mesh_cache: HashMap::new(),
            mesh_cache_capacity: std::env::var("UNENDLICH_MESH_CACHE_SIZE")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(256),
            mesh_cache_seq: 0,
            mesh_cache_hits: 0,
            mesh_cache_misses: 0,
            mesh_cache_park_attempts: 0,
            // v0.24.26-octree-on-gpu: octree substrate + lazy probe.
            octree_pool,
            octree_probe: None,
            octree_probe_queue: ctx.graphics_queue,
            octree_probe_queue_family: ctx.graphics_queue_family,
            octree_probe_physical_device: ctx.physical_device,
        })
    }

    /// Mesh + upload a chunk. If a mesh already exists for this coord, the
    /// old one is moved to the deferred-destroy queue.
    ///
    /// This is the synchronous path. For async uploads (mesh on a worker
    /// thread, upload here), see `upload_prebuilt_mesh`.
    pub fn upload_chunk(&mut self, ctx: &VulkanContext, chunk: &Chunk) -> Result<()> {
        let mesh = mesh_chunk(chunk);
        self.upload_prebuilt_mesh(ctx, chunk.coord, mesh, chunk.version)
    }

    /// Upload a chunk mesh that was pre-built off-thread.
    ///
    /// Pre-v0.18.2: this allocated fresh GPU memory per chunk via
    /// `vkAllocateMemory`. That caused whole-screen freezes (multi-frame
    /// stalls) under streaming load because the driver serializes
    /// allocations against the queue.
    ///
    /// Post-v0.18.2: the renderer owns a `VulkanMeshPool` with two big
    /// pre-allocated buffers (vertex + index) sliced into fixed-size slots.
    /// Upload allocates a slot if needed (O(1), no Vulkan calls) and
    /// memcpys the mesh bytes into the slot's persistent map. No
    /// `vkAllocateMemory` on the hot path.
    ///
    /// If `mesh` is empty, any existing mesh for this coord is evicted.
    /// If the mesh exceeds the pool's per-slot cap, falls back to the
    /// pre-pool path (rare; logged via `MeshPoolError::Overflow`).
    pub fn upload_prebuilt_mesh(
        &mut self,
        ctx: &VulkanContext,
        coord: ChunkCoord,
        mesh: ChunkMesh,
        version: u64,
    ) -> Result<()> {
        // Backward-compatible shim: old call sites assumed LOD::Near.
        // F3 added the per-LOD variant; this delegates so a single
        // implementation handles both.
        self.upload_prebuilt_mesh_at_lod(
            ctx, coord, crate::world::lod::LodLevel::Near, mesh, version,
        )
    }

    /// Upload a mesh for `(coord, lod)`. Replaces any prior mesh at
    /// the same slot key. Empty mesh evicts.
    ///
    /// F3+ users (the meshing pool consumer) plumb the LOD through
    /// from MeshResult. F1/F2 callers continue to use
    /// `upload_prebuilt_mesh` which delegates here with LOD::Near.
    pub fn upload_prebuilt_mesh_at_lod(
        &mut self,
        ctx: &VulkanContext,
        coord: ChunkCoord,
        lod: crate::world::lod::LodLevel,
        mesh: ChunkMesh,
        version: u64,
    ) -> Result<()> {
        // Immediate (visible) upload — device copy flushed in full this frame.
        self.upload_prebuilt_mesh_at_lod_inner(ctx, coord, lod, mesh, version, false)
    }

    /// v0.25-prefetch (T05-R27): deferrable counterpart for **prefetched,
    /// not-yet-visible** chunks. Identical to `upload_prebuilt_mesh_at_lod`
    /// except the device copy rides the mesh pool's budgeted
    /// `write_at_deferrable` lane instead of `write_at`, so a burst of
    /// ahead-of-camera uploads is spread across frames instead of stalling one
    /// frame's head.
    ///
    /// SAFETY: the caller must have established that this chunk is NOT drawn
    /// this frame (the draw loop binds every resident mesh, so deferring a
    /// visible slot would draw stale device memory). Use `chunk_visible` to
    /// decide, and fall back to `upload_prebuilt_mesh_at_lod` when in doubt.
    pub fn upload_prebuilt_mesh_at_lod_deferrable(
        &mut self,
        ctx: &VulkanContext,
        coord: ChunkCoord,
        lod: crate::world::lod::LodLevel,
        mesh: ChunkMesh,
        version: u64,
    ) -> Result<()> {
        self.upload_prebuilt_mesh_at_lod_inner(ctx, coord, lod, mesh, version, true)
    }

    /// v0.25-prefetch (T05-R27): is the chunk at `coord` potentially visible
    /// under `view` this frame? Uses the SAME `Frustum` as the GPU cull, with a
    /// conservative full-slab chunk AABB (`block_origin .. +CHUNK_SIZE`). The
    /// conservatism is one-directional and exactly what the deferrable upload
    /// lane needs: a `false` means even the enclosing box is fully outside the
    /// frustum, so the chunk is definitely not drawn and deferring its copy is
    /// safe; a marginal chunk only ever errs toward `true` (immediate upload).
    /// Cheap: one matrix build + 6 plane tests, no DashMap / GPU work.
    pub fn chunk_visible(&mut self, coord: ChunkCoord, view: &View) -> bool {
        let w = self.swapchain.extent.width;
        let h = self.swapchain.extent.height.max(1);
        // headlesshorseman--T3: rebuild the frustum only when the (view, extent)
        // key changes — once per frame in steady state — instead of on every
        // per-chunk call. A cache hit reuses planes that are bit-identical to
        // recomputing, so the visibility result is unchanged; we just skip the
        // matrix build + 6-plane extract for every chunk after the first.
        let key = FrustumKey {
            pos: [view.position.x, view.position.y, view.position.z],
            yaw: view.yaw,
            pitch: view.pitch,
            fov: view.fov_y_radians,
            near: view.near,
            far: view.far,
            extent: [w, h],
        };
        if self.cached_frustum_key != Some(key) {
            let aspect = w as f32 / h as f32;
            let vp = view.view_projection(aspect);
            self.cached_frustum = Frustum::from_view_projection(&vp);
            self.cached_frustum_key = Some(key);
        }
        let (ox, oy, oz) = coord.block_origin();
        let cs = CHUNK_SIZE as f32;
        let min = glm::vec3(ox as f32, oy as f32, oz as f32);
        let max = glm::vec3(ox as f32 + cs, oy as f32 + cs, oz as f32 + cs);
        self.cached_frustum.aabb_visible(&min, &max)
    }

    /// Evict any meshes for `coord` at a LOD *other* than `lod` (a new mesh at
    /// `lod` is replacing them — see the LOD double-draw fix below).
    ///
    /// Perf (T10): there are only a handful of `LodLevel`s, so probe the few
    /// possible other-LOD keys for this one coord directly instead of scanning
    /// every uploaded mesh key. The old `self.meshes.keys().filter(...).collect()`
    /// was O(total uploaded meshes) — thousands at high render distance — and
    /// ran on *every* mesh upload during streaming bursts; this is O(#LODs).
    /// Behaviour is identical: the same set of present other-LOD entries is
    /// evicted (`MeshSlotKey` is exactly `{coord, lod}`).
    fn evict_stale_lods(&mut self, coord: ChunkCoord, lod: crate::world::lod::LodLevel) {
        for other in crate::world::lod::LodLevel::iter() {
            if other != lod
                && self.meshes.contains_key(&crate::world::lod::MeshSlotKey::at(coord, other))
            {
                self.evict_chunk_lod(coord, other);
            }
        }
    }

    fn upload_prebuilt_mesh_at_lod_inner(
        &mut self,
        _ctx: &VulkanContext,
        coord: ChunkCoord,
        lod: crate::world::lod::LodLevel,
        mesh: ChunkMesh,
        version: u64,
        deferrable: bool,
    ) -> Result<()> {
        let key = crate::world::lod::MeshSlotKey::at(coord, lod);
        if mesh.is_empty() {
            if let Some(old) = self.meshes.remove(&key) {
                self.pending_destroy.push(PendingDestroy {
                    slot: old.slot,
                    draw_slot: old.draw_slot,
                    frames_remaining: self.frames_in_flight,
                });
            }
            // v0.24.72: record "meshed empty at this version" so the
            // dispatcher doesn't re-mesh this coord on every dispatch
            // round just because `meshes` has no entry. The next mesh
            // request fires only when chunk.version changes.
            self.empty_mesh_versions.insert(key, version);
            return Ok(());
        }

        // v0.24.72: a non-empty mesh is being uploaded — clear any
        // prior "empty at v" record. The chunk transitioned to
        // having geometry; the stale entry would confuse future
        // dispatch checks if it stayed.
        self.empty_mesh_versions.remove(&key);

        // v0.24.19: try mesh-cache restore. If we have a cached entry
        // for (coord, lod) AND its version matches the fresh mesh's
        // version, the cached GPU buffers are still valid and we can
        // discard `mesh` instead of uploading. This addresses the
        // upload-pressure-driven chug on chunk revisits.
        //
        // Stale (version mismatch) entries are dropped by
        // try_cache_restore so they don't leak. Cache disabled
        // (capacity 0) short-circuits this to a no-op.
        if self.try_cache_restore(coord, lod, version) {
            // Cache hit. We've already restored buffers into `meshes`
            // and re-registered the indirect-draw entry. Also evict
            // stale-LOD entries (same logic as below) since a different
            // LOD's mesh may still be live.
            //
            // v0.24.NEXT: route through evict_chunk_lod so the
            // empty-mesh version record for the stale tier also gets
            // cleared. Previously inlined cache-park+pending_destroy
            // forgot empty_mesh_versions, slowly leaking one entry per
            // LOD-band crossing until full chunk eviction.
            self.evict_stale_lods(coord, lod);
            return Ok(());
        }

        // v0.23.2 bug fix (LOD double-draw): when a NEW LOD's mesh
        // lands for a coord, evict all OTHER LODs of that coord. The
        // current LOD's old entry will be replaced by `upload_mesh`
        // below (it does its own deferred-destroy via the HashMap
        // insert), but other-LOD entries would otherwise persist and
        // draw alongside the new mesh. Symptoms before fix:
        // double-/triple-draw of every chunk that crossed a LOD band,
        // with corresponding GPU shadow + main time inflation.
        //
        // F3a (v0.22.3) shipped LOD-aware dispatch but deferred
        // transition handling to "F3b". The streaming loop's natural
        // re-mesh-on-band-crossing behaviour exposed the gap: chunks
        // cross bands, get re-meshed at new LOD, old LOD never
        // evicted. Confirmed by playtest where RD=32 with aggressive
        // factors produced WORSE GPU times than RD=6 with default
        // factors — each chunk drawing 2-3× as LOD copies accumulated.
        //
        // We could keep the previous coarser LOD as a fallback for
        // upload-in-progress to avoid the visible flicker. Filed for
        // F3b proper, when we add the "double-buffer during transition"
        // pattern. For now, the bug-free behaviour wins over the
        // transition smoothing.
        //
        // v0.24.NEXT: same evict_chunk_lod routing as the cache-hit
        // branch above — fixes the empty_mesh_versions leak that the
        // original inline body silently introduced.
        self.evict_stale_lods(coord, lod);

        self.upload_mesh(coord, lod, &mesh, version, deferrable)
    }

    fn upload_mesh(
        &mut self,
        coord: ChunkCoord,
        lod: crate::world::lod::LodLevel,
        mesh: &ChunkMesh,
        version: u64,
        deferrable: bool,
    ) -> Result<()> {
        // v0.24.20-vertex-pack: bytes on the wire are PackedVertex
        // (8 bytes), not the CPU-side Vertex (24 bytes). 3× upload
        // bandwidth reduction vs the pre-pack path.
        let vert_count = mesh.vertex_count() as usize;
        let vert_bytes = vert_count * std::mem::size_of::<PackedVertex>();
        let idx_bytes = (mesh.index_count() as usize) * std::mem::size_of::<u32>();

        // v0.24.53-pack-singlepass: combine Y-bounds computation
        // and vertex packing into a single linear pass over
        // mesh.vertices. The previous form did two passes: one
        // for fold-based min/max Y, another for .map().collect()
        // into packed. For a 44K-vertex chunk that's 88K vertex
        // ops; now it's 44K. CPU savings scale linearly with
        // mesh size — meaningful during burst-upload after
        // movement when 64 chunks/frame are getting packed.
        //
        // Branches in the body (min/max update conditionals) are
        // predicted-not-taken for the common case (most iterations
        // don't update the bounds). The pack call is the
        // bottleneck.
        let (ox, oy, oz) = coord.block_origin();
        let chunk_origin_f = [ox as f32, oy as f32, oz as f32];

        let (packed_verts, min_y, max_y) = if mesh.vertices.is_empty() {
            let oy_f = oy as f32;
            (Vec::new(), oy_f, oy_f + CHUNK_SIZE as f32)
        } else {
            let mut packed: Vec<PackedVertex> = Vec::with_capacity(mesh.vertices.len());
            let first_y = mesh.vertices[0].position[1];
            let mut min_y = first_y;
            let mut max_y = first_y;
            for v in mesh.vertices.iter() {
                let y = v.position[1];
                if y < min_y { min_y = y; }
                if y > max_y { max_y = y; }
                packed.push(PackedVertex::from_world(v, chunk_origin_f));
            }
            (packed, min_y, max_y)
        };

        // Sub-allocator design (v0.19): every upload allocates fresh
        // spans sized to the actual mesh. No slot reuse — the previous
        // re-mesh "reuse the same slot" optimization doesn't apply
        // because new mesh sizes may differ from old. Instead: alloc
        // new, defer-free old once new succeeds.
        //
        // Free-then-alloc would risk fragmenting the freed span before
        // the new alloc could use it; alloc-then-free is the safer
        // ordering even though it briefly holds both.
        const VERTEX_SIZE: u64 = std::mem::size_of::<PackedVertex>() as u64; // 8
        const INDEX_SIZE: u64 = std::mem::size_of::<u32>() as u64;

        // Prepare a byte view of the packed vertex array for the
        // memcpy below. The packed array lives on this stack frame's
        // heap and is dropped at end of `upload_mesh`; the GPU has
        // its own copy via `mesh_pool.write_at` before we return.
        let vert_byte_slice = unsafe {
            std::slice::from_raw_parts(
                packed_verts.as_ptr() as *const u8,
                vert_bytes,
            )
        };

        // Step 1: alloc with vertex alignment.
        let alloc_slot = match {
            // Use the pool's CPU-side allocator path directly to get a
            // slot WITHOUT uploading data — we'll write both halves in
            // step 3 once we have the slot.
            self.mesh_pool.alloc(vert_bytes as u64, idx_bytes as u64, VERTEX_SIZE)
        } {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "Mesh pool alloc failed for {:?}: {} \
                     (vert_bytes={}, idx_bytes={}; pool: vert {}/{}, idx {}/{}, \
                     largest free vert {}, largest free idx {}, slots {})",
                    coord, e, vert_bytes, idx_bytes,
                    self.mesh_pool.vert_allocated_bytes(),
                    self.mesh_pool.vert_buffer_size(),
                    self.mesh_pool.idx_allocated_bytes(),
                    self.mesh_pool.idx_buffer_size(),
                    self.mesh_pool.vert_largest_free_span(),
                    self.mesh_pool.idx_largest_free_span(),
                    self.mesh_pool.slot_count(),
                );
                self.upload_fail_count += 1;
                return Ok(());
            }
        };
        let new_slot = alloc_slot;

        // Step 2: alloc indirect-draw slot.
        let draw_slot = match self.indirect.alloc() {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "Indirect draw slot alloc failed for {:?}: {} (in_use={}/{})",
                    coord, e, self.indirect.in_use(), self.indirect.capacity());
                let _ = self.mesh_pool.free(new_slot);
                self.upload_fail_count += 1;
                return Ok(());
            }
        };

        // Step 3: write both buffers.
        //
        // v0.24.20-vertex-pack: indices are uploaded UNSHIFTED. The
        // per-chunk vertex base offset is communicated to the GPU via
        // the indirect-draw command's `vertexOffset` field (set in
        // step 4 below). Vulkan adds vertexOffset to each index before
        // fetching the vertex, so the same effect is achieved without
        // a CPU pass over all indices.
        //
        // Previous shape (pre-v0.24.20): allocate a fresh `Vec<u32>`
        // of length `idx_count`, copy + add per element, memcpy to GPU.
        // For a 30K-index chunk that's ~120 KB allocation + 30K adds
        // per upload — meaningful at sustained upload pressure.
        let idx_byte_slice = unsafe {
            std::slice::from_raw_parts(
                mesh.indices.as_ptr() as *const u8,
                idx_bytes,
            )
        };
        unsafe {
            // v0.25-prefetch (T05-R27): prefetched, not-yet-visible chunks ride
            // the budgeted deferrable lane so an ahead-of-camera upload burst
            // doesn't stall the frame head; visible chunks flush immediately.
            if deferrable {
                self.mesh_pool.write_at_deferrable(
                    new_slot, vert_byte_slice, idx_byte_slice,
                );
            } else {
                self.mesh_pool.write_at(
                    new_slot, vert_byte_slice, idx_byte_slice,
                );
            }
        }

        // Step 4: write the indirect-draw entry.
        //
        // v0.24.20-vertex-pack:
        // - `aabb_min.w` now carries the chunk's world-space origin Y
        //   (was unused padding). The vertex shader reads it to
        //   reconstruct world position from the packed chunk-local
        //   vertex. `aabb_min.xyz` is unchanged — `.x` and `.z`
        //   continue to be the chunk's XZ origin (also used by the
        //   vertex shader); `.y` is the tight lower-Y bound used by
        //   the cull shader for frustum culling. The cull shader
        //   doesn't read `.w`, so this reuse is safe.
        // - `vertex_offset` now carries the chunk's vertex base index
        //   (was always 0; the CPU pre-shifted indices). The cull
        //   shader propagates this to the indirect draw command and
        //   Vulkan adds it during index fetch.
        let vert_base_count = (new_slot.vert_base_aligned / VERTEX_SIZE) as i32;
        let aabb_min = [ox as f32, min_y, oz as f32, oy as f32];
        let aabb_max = [
            ox as f32 + CHUNK_SIZE as f32,
            max_y,
            oz as f32 + CHUNK_SIZE as f32,
            0.0,
        ];
        // Debug-overlay sentinel chunk skips frustum cull because its
        // primitives can extend far beyond a single-chunk AABB (e.g., the
        // tick-cull boundary outline is render-distance wide).
        let is_debug_overlay = coord.x == i32::MIN
            && coord.y == i32::MIN
            && coord.z == i32::MIN;
        let flag_bits = if is_debug_overlay {
            crate::render::chunk_indirect::FLAG_DEBUG_OVERLAY
        } else { 0 };
        // F3: pack the actual LOD into flags upper byte. The cull
        // shader doesn't read this yet (cull is LOD-agnostic), but
        // future shadow-cascade alignment (F6) and any per-LOD GPU
        // logic will consume it.
        let flags = crate::world::lod::flags_with_lod(flag_bits, lod);
        let chunk_draw = crate::render::chunk_indirect::ChunkDraw {
            index_count: mesh.index_count(),
            first_index: (new_slot.idx_offset / INDEX_SIZE) as u32,
            vertex_offset: vert_base_count,
            flags,
            aabb_min, aabb_max,
        };
        self.indirect.set(draw_slot, chunk_draw);

        let new = UploadedMesh {
            slot: new_slot,
            index_count: mesh.index_count(),
            version,
            min_y, max_y,
            draw_slot,
        };

        // Defer-free the old mesh's slots (mesh + draw) so the GPU
        // finishes any in-flight draw using them before reuse.
        if let Some(old) = self.meshes.insert(crate::world::lod::MeshSlotKey::at(coord, lod), new) {
            self.pending_destroy.push(PendingDestroy {
                slot: old.slot,
                draw_slot: old.draw_slot,
                frames_remaining: self.frames_in_flight,
            });
        }

        Ok(())
    }

    // ─── v0.24.19: mesh cache helpers ─────────────────────────────────

    /// Park an evicted UploadedMesh in the cache instead of returning
    /// its slots to the pool. If the cache is at capacity, drop the
    /// LRU entry first (its slots go to `pending_destroy`).
    ///
    /// Returns `Some(UploadedMesh)` if the entry could NOT be cached
    /// (cache disabled, or cache parking failed). Caller must
    /// `pending_destroy.push` the returned entry. Returns `None` if
    /// the entry was successfully parked.
    fn try_cache_park(
        &mut self,
        key: crate::world::lod::MeshSlotKey,
        old: UploadedMesh,
    ) -> Option<UploadedMesh> {
        // v0.24.20-vertex-pack: bumped before the disabled-cache early
        // return so park_attempts also surfaces "evict-rate" when the
        // cache is off — useful for sizing capacity before enabling it.
        self.mesh_cache_park_attempts += 1;
        if self.mesh_cache_capacity == 0 {
            return Some(old);
        }
        // If a stale entry under this key already exists in the cache,
        // it gets replaced; the displaced entry goes to pending_destroy.
        // This shouldn't happen normally (cache entries are removed on
        // restore or LRU eviction) but handle it defensively.
        if let Some(stale) = self.mesh_cache.remove(&key) {
            self.pending_destroy.push(PendingDestroy {
                slot: stale.slot,
                draw_slot: stale.draw_slot,
                frames_remaining: self.frames_in_flight,
            });
        }
        // LRU eviction if at capacity.
        if self.mesh_cache.len() >= self.mesh_cache_capacity {
            self.cache_evict_lru();
        }
        self.mesh_cache_seq += 1;
        self.mesh_cache.insert(key, CachedMesh {
            slot: old.slot,
            draw_slot: old.draw_slot,
            index_count: old.index_count,
            version: old.version,
            min_y: old.min_y,
            max_y: old.max_y,
            last_touched: self.mesh_cache_seq,
        });
        None
    }

    /// Drop the LRU cache entry. Its GPU slots go to `pending_destroy`
    /// so the GPU has time to drain any references before the slots
    /// rejoin the free pool.
    fn cache_evict_lru(&mut self) {
        // Pick the entry with the smallest last_touched. The free
        // function `cache_lru_key` is broken out for testability.
        let touches: Vec<(crate::world::lod::MeshSlotKey, u64)> = self.mesh_cache.iter()
            .map(|(k, c)| (*k, c.last_touched))
            .collect();
        let Some(lru_key) = cache_lru_key(&touches) else { return };
        if let Some(evicted) = self.mesh_cache.remove(&lru_key) {
            self.pending_destroy.push(PendingDestroy {
                slot: evicted.slot,
                draw_slot: evicted.draw_slot,
                frames_remaining: self.frames_in_flight,
            });
        }
    }

    /// On upload, check whether a fresh mesh would land on a cached
    /// entry with the SAME version stamp. If yes, restore the cached
    /// buffers, re-write the indirect-draw entry, and signal success.
    /// Returns `true` if the cache hit was used (caller should skip
    /// the actual upload). Returns `false` if the caller must do a
    /// normal upload.
    ///
    /// Version mismatch means the cached mesh is stale (chunk was
    /// edited between eviction and revisit). The stale entry is
    /// removed and its slots queued for destroy; caller falls through
    /// to upload the fresh mesh.
    fn try_cache_restore(
        &mut self,
        coord: ChunkCoord,
        lod: crate::world::lod::LodLevel,
        version: u64,
    ) -> bool {
        if self.mesh_cache_capacity == 0 {
            return false;
        }
        let key = crate::world::lod::MeshSlotKey::at(coord, lod);
        let Some(cached) = self.mesh_cache.remove(&key) else {
            return false;
        };
        if cached.version != version {
            // Stale: chunk was edited. Drop the cached entry.
            self.mesh_cache_misses += 1;
            self.pending_destroy.push(PendingDestroy {
                slot: cached.slot,
                draw_slot: cached.draw_slot,
                frames_remaining: self.frames_in_flight,
            });
            return false;
        }
        // Hit. Re-register the indirect-draw entry and restore to
        // `meshes`. We do NOT take the cached entry's last_touched
        // counter forward; once restored, it leaves the cache. If
        // it's evicted again later, it'll re-enter as fresh.
        const INDEX_SIZE: u64 = std::mem::size_of::<u32>() as u64;
        const VERTEX_SIZE: u64 = std::mem::size_of::<PackedVertex>() as u64; // 8
        let (ox, oy, oz) = coord.block_origin();
        // v0.24.20-vertex-pack: aabb_min.w carries chunk_origin.y for
        // the vertex shader's chunk-local unpack path. Must be set on
        // cache restore as well as fresh upload. `vertex_offset` is
        // the chunk's vertex base index (was always 0 pre-v0.24.20;
        // now the GPU does the index-base addition).
        let vert_base_count = (cached.slot.vert_base_aligned / VERTEX_SIZE) as i32;
        let aabb_min = [ox as f32, cached.min_y, oz as f32, oy as f32];
        let aabb_max = [
            ox as f32 + CHUNK_SIZE as f32,
            cached.max_y,
            oz as f32 + CHUNK_SIZE as f32,
            0.0,
        ];
        let is_debug_overlay = coord.x == i32::MIN
            && coord.y == i32::MIN
            && coord.z == i32::MIN;
        let flag_bits = if is_debug_overlay {
            crate::render::chunk_indirect::FLAG_DEBUG_OVERLAY
        } else { 0 };
        let flags = crate::world::lod::flags_with_lod(flag_bits, lod);
        let chunk_draw = crate::render::chunk_indirect::ChunkDraw {
            index_count: cached.index_count,
            first_index: (cached.slot.idx_offset / INDEX_SIZE) as u32,
            vertex_offset: vert_base_count,
            flags,
            aabb_min,
            aabb_max,
        };
        self.indirect.set(cached.draw_slot, chunk_draw);

        // Replace any prior live entry under this key (shouldn't happen
        // — if it were live, it wouldn't have been re-meshed — but be
        // defensive).
        let restored = UploadedMesh {
            slot: cached.slot,
            index_count: cached.index_count,
            version: cached.version,
            min_y: cached.min_y,
            max_y: cached.max_y,
            draw_slot: cached.draw_slot,
        };
        if let Some(displaced) = self.meshes.insert(key, restored) {
            self.pending_destroy.push(PendingDestroy {
                slot: displaced.slot,
                draw_slot: displaced.draw_slot,
                frames_remaining: self.frames_in_flight,
            });
        }
        // v0.24.72: defensive — if any stale empty-mesh record exists
        // for this key, clear it. A cache-restore means we now have a
        // real mesh, so the dispatcher should treat this as the
        // authoritative state.
        self.empty_mesh_versions.remove(&key);
        self.mesh_cache_hits += 1;
        true
    }

    /// Surface mesh-cache counters for the per-window FPS log.
    /// Format (v0.24.20-vertex-pack):
    ///   `cache: hits=N misses=M parks=P size=S/cap=C`
    /// `parks` is the lifetime count of `try_cache_park` calls
    /// (chunk evictions that could have populated the cache). When
    /// `parks=0` despite long sessions, the play pattern simply
    /// didn't trigger evictions — the cache was correct but dormant.
    /// When `parks >> size`, the working set churned past cache
    /// capacity and an increase to `UNENDLICH_MESH_CACHE_SIZE`
    /// would help.
    /// Returns empty string when the cache is disabled so the log
    /// line stays clean.
    pub fn mesh_cache_log(&self) -> String {
        if self.mesh_cache_capacity == 0 {
            return String::new();
        }
        format!(
            "cache: hits={} misses={} parks={} size={}/cap={}",
            self.mesh_cache_hits,
            self.mesh_cache_misses,
            self.mesh_cache_park_attempts,
            self.mesh_cache.len(),
            self.mesh_cache_capacity,
        )
    }

    /// Override the mesh-cache capacity at runtime (used by the crash-safe
    /// tuner at startup; 0 disables the cache). Safe before the cache fills.
    pub fn set_mesh_cache_capacity(&mut self, cap: usize) {
        self.mesh_cache_capacity = cap;
    }

    /// Number of mesh-cache hits since startup. Exposed for tests.
    pub fn mesh_cache_hits(&self) -> u64 {
        self.mesh_cache_hits
    }

    /// Number of mesh-cache version-mismatch misses since startup.
    /// Exposed for tests.
    pub fn mesh_cache_misses(&self) -> u64 {
        self.mesh_cache_misses
    }

    /// Number of entries currently in the mesh cache. Exposed for tests.
    pub fn mesh_cache_size(&self) -> usize {
        self.mesh_cache.len()
    }

    /// Remove a chunk from the rendered set. The slot returns to the pool
    /// after `MAX_FRAMES_IN_FLIGHT` frames (deferred to ensure no in-flight
    /// GPU draw still references it).
    ///
    /// F3: evicts ALL LODs of the given coord. This is the right
    /// behaviour when a chunk leaves the streaming radius — every
    /// stored mesh is now stale. For LOD-tier transitions where we
    /// want to evict only one specific tier (and keep the rest), see
    /// [`evict_chunk_lod`].
    pub fn evict_chunk(&mut self, coord: ChunkCoord) {
        self.evict_chunk_mesh(coord);
        // v0.24.26-octree-on-gpu: the GPU-resident octree slot for
        // this chunk follows the mesh. No defer-destroy because the
        // probe pipeline is the only reader and it runs synchronously
        // (the bin invokes verify on shutdown when there's nothing
        // streaming). When a real consumer lands — likely the
        // octree-driven mesher in the next ship — this eviction will
        // need MAX_FRAMES_IN_FLIGHT deferment like the mesh path.
        self.octree_pool.lock().expect("octree_pool mutex poisoned").evict(coord);
    }

    /// Batched eviction: evict many coords taking the shared octree pool lock
    /// **once** for the whole set instead of once per coord. Semantically
    /// identical to calling [`evict_chunk`] per coord (same coords evicted) —
    /// the only difference is lock churn: the mesh-slot eviction is done for
    /// every coord first, then a single pool lock evicts all their octree slots,
    /// cutting contention with the mesher on the shared `Arc<Mutex<…>>`.
    pub fn evict_chunks(&mut self, coords: &[ChunkCoord]) {
        if coords.is_empty() {
            return;
        }
        for &coord in coords {
            self.evict_chunk_mesh(coord);
        }
        // ONE lock for the whole batch (see per-coord rationale in `evict_chunk`).
        let mut pool = self.octree_pool.lock().expect("octree_pool mutex poisoned");
        for &coord in coords {
            pool.evict(coord);
        }
    }

    /// Full world reset: drop EVERY uploaded chunk mesh + the parked cache so the
    /// old world stops rendering immediately. Used when launching a new seed from
    /// the menu — without this the new world streams in ON TOP of the old
    /// titlescreen world (the "merged worlds" bug). Removing from `meshes` stops the
    /// draw this frame; GPU buffers free via the normal deferred-destroy path. The
    /// cache is drained too so no old-world mesh is reused for a same-coord chunk.
    pub fn clear_all_chunk_meshes(&mut self) {
        use std::collections::HashSet;
        let coords: Vec<ChunkCoord> = self
            .meshes
            .keys()
            .map(|k| k.coord)
            .chain(self.empty_mesh_versions.keys().map(|k| k.coord))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        self.evict_chunks(&coords);
        for (_k, cm) in self.mesh_cache.drain() {
            self.pending_destroy.push(PendingDestroy {
                slot: cm.slot,
                draw_slot: cm.draw_slot,
                frames_remaining: self.frames_in_flight,
            });
        }
    }

    /// Evict just the CPU/GPU **mesh** slots for a chunk (all 4 LODs) — the
    /// octree-pool slot is handled separately so callers can batch the pool
    /// lock. Not public: the lock-bearing entry points are [`evict_chunk`] /
    /// [`evict_chunks`].
    fn evict_chunk_mesh(&mut self, coord: ChunkCoord) {
        // A chunk has at most one mesh per LOD level (4 total), so look the
        // keys up directly instead of scanning the whole `meshes` map. The old
        // code did `self.meshes.keys().filter(...)` — O(total meshes) PER call —
        // which made a moving-player eviction wave O(evicted × total) ≈ O(n²)
        // and cost ~109ms/frame at RD=12 in run data. This is O(4).
        for lod in crate::world::lod::LodLevel::iter() {
            let key = crate::world::lod::MeshSlotKey::at(coord, lod);
            if let Some(old) = self.meshes.remove(&key) {
                // v0.24.19: try to park in the mesh cache before
                // queuing for destroy. If the cache is full or
                // disabled, fall through to pending_destroy.
                if let Some(unparked) = self.try_cache_park(key, old) {
                    self.pending_destroy.push(PendingDestroy {
                        slot: unparked.slot,
                        draw_slot: unparked.draw_slot,
                        frames_remaining: self.frames_in_flight,
                    });
                }
            }
            // Also drop any empty-mesh record for this (coord, lod) — direct
            // remove instead of the old O(n) `empty_mesh_versions.retain`.
            self.empty_mesh_versions.remove(&key);
        }
    }

    /// Remove a single (coord, lod) entry. Used during LOD transitions
    /// where the renderer keeps the OLD lod's mesh visible until the
    /// NEW lod's mesh has finished uploading, then drops the old one.
    pub fn evict_chunk_lod(
        &mut self,
        coord: ChunkCoord,
        lod: crate::world::lod::LodLevel,
    ) {
        let key = crate::world::lod::MeshSlotKey::at(coord, lod);
        if let Some(old) = self.meshes.remove(&key) {
            if let Some(unparked) = self.try_cache_park(key, old) {
                self.pending_destroy.push(PendingDestroy {
                    slot: unparked.slot,
                    draw_slot: unparked.draw_slot,
                    frames_remaining: self.frames_in_flight,
                });
            }
        }
        // v0.24.72: also clear the empty-mesh record for THIS specific
        // (coord, lod). The LOD-transition caller expects the next
        // dispatch to re-evaluate this entry; leaving an empty record
        // would suppress that.
        self.empty_mesh_versions.remove(&key);
    }

    /// External accessor that yields one ChunkCoord per stored mesh,
    /// regardless of LOD. In F3+ a chunk may have meshes at multiple
    /// LODs during a transition; we dedupe by coord so the streaming
    /// loop sees each chunk once.
    ///
    /// v0.24.72: also includes coords that have only empty-mesh
    /// records (no real meshes). Without this, those coords stay in
    /// `empty_mesh_versions` forever after they leave render distance
    /// — the bin's eviction loop calls `evict_chunk` only for coords
    /// returned here. Including them ensures eviction cleans up
    /// stale empty records as the player moves.
    pub fn loaded_chunk_coords(&self) -> impl Iterator<Item = ChunkCoord> + '_ {
        let mut unique: std::collections::HashSet<ChunkCoord> =
            self.meshes.keys().map(|k| k.coord).collect();
        for k in self.empty_mesh_versions.keys() {
            unique.insert(k.coord);
        }
        // v0.24.NEXT+11-octree-ghost-fix: also include chunks that have a
        // GPU octree slot but no mesh yet. Without this, an octree uploaded
        // but never meshed (or meshed and the slot freed without clearing
        // the upload) stays in the pool forever — visible as
        // `octree_chunks > rendering chunks` by a large multiple in the
        // run-summary memory line. The eviction caller walks this set and
        // calls evict_chunk for each out-of-distance coord, which now
        // correctly frees the octree slot.
        // headlesshorseman--T8: narrow the octree_pool critical section. Only
        // reading the slot table needs the lock; the HashSet insertions below
        // (hashing + possible rehash/realloc) do not. Snapshot the coords under
        // the lock into a small Vec (ChunkCoord is Copy — cheap), release the
        // lock, then merge into the set outside it — so the mesher/render
        // threads aren't blocked on the pool mutex while we hash. The resulting
        // set is identical to inserting under the lock.
        let pool_coords: Vec<ChunkCoord> = {
            let pool = self.octree_pool.lock().expect("octree_pool mutex poisoned");
            pool.iter_slots().map(|(coord, _slot)| coord).collect()
        };
        for coord in pool_coords {
            unique.insert(coord);
        }
        unique.into_iter()
    }

    /// v0.24.NEXT+14-mesh-diag: count of chunks that the mesher ran on and
    /// produced an EMPTY mesh, bucketed by LOD `[near, mid, far, vfar]`.
    ///
    /// Empty is legitimate for all-air sky chunks, but a large empty count at
    /// near/mid LOD — where the player can see solid terrain — is the
    /// signature of the "mesh not showing on GPU" failure: the GPU mesher
    /// returned no geometry for a chunk that has content. Comparing the
    /// CPU-meshed near bucket against the GPU-meshed mid/far/vfar buckets
    /// isolates whether the GPU mesher specifically is dropping geometry.
    pub fn empty_mesh_counts_by_lod(&self) -> [usize; 4] {
        let mut counts = [0usize; 4];
        for key in self.empty_mesh_versions.keys() {
            counts[key.lod as usize] += 1;
        }
        counts
    }

    /// Cumulative count of mesh uploads that failed since startup.
    /// Surfaced in the FPS log so silent drops are visible to the user.
    /// Non-zero values usually indicate pool exhaustion or fragmentation.
    pub fn upload_fail_count(&self) -> u64 {
        self.upload_fail_count
    }

    /// r736: apply a CPU chunk-occlusion set to the indirect draw metadata. The
    /// caller (which owns the world) computes the set of *visible* chunk coords —
    /// e.g. `world::occlusion_flood::flood_visible` over the loaded chunks'
    /// `cached_all_opaque` around the camera — and passes it in each frame *before*
    /// `render()`. Resident chunks not in the set are marked occluded and skipped by
    /// the cull shader. Returns how many resident slots are currently occluded.
    ///
    /// Only sets flags on changed slots; the per-frame `flush_frame` inside
    /// `render()` propagates them. When occlusion is disabled call
    /// [`clear_chunk_occlusion`] once so nothing stays hidden.
    pub fn apply_chunk_occlusion(
        &mut self,
        chunk_size: i32,
        visible: &std::collections::HashSet<[i32; 3]>,
    ) -> u32 {
        self.indirect.apply_occlusion(chunk_size, visible)
    }

    /// r736: clear all occlusion flags (turn the pre-cull off).
    pub fn clear_chunk_occlusion(&mut self) {
        self.indirect.clear_occlusion();
    }

    /// v0.24.63-adaptive-uploads: most-recent `wait_for_fences`
    /// measurement in milliseconds. Captured at the top of every
    /// `draw_frame` call. The bin's adaptive upload controller polls
    /// this each frame to decide whether to throttle uploads.
    ///
    /// Returns `0.0` before the first frame has waited (i.e., on
    /// the very first frame). Callers reading the value during
    /// startup should treat 0.0 as "no signal yet" rather than "GPU
    /// is idle"; the adaptive controller handles this via its EMA
    /// initialized at 0.0 plus the dead band logic.
    pub fn last_wait_fences_ms(&self) -> f64 {
        self.last_wait_fences_ms
    }

    /// Format and reset the per-pass GPU timing window. Call once
    /// per second alongside the existing FPS log line.
    ///
    /// Returns a string like:
    ///   "gpu: shadow=2.3ms cull=0.1ms main=12.5ms total=15.0ms (over N frames, M skipped)"
    /// Or a placeholder if the device doesn't support timestamps,
    /// or if no frames have been collected since the last flush.
    pub fn flush_gpu_timer_log(&mut self) -> String {
        self.gpu_timer.flush_log()
    }

    /// c.13-instrumentation: clear lifetime accumulators on both GPU
    /// and CPU timers. Called at the warmup→steady transition so the
    /// assertion-relevant samples reflect only the steady-state window.
    /// The per-second rolling window is not touched (it rolls over on
    /// its own).
    pub fn reset_timer_lifetime_stats(&mut self) {
        self.gpu_timer.reset_lifetime_stats();
        self.cpu_timer.reset_lifetime_stats();
    }

    /// Format the GPU timer's lifetime per-zone stats (count, mean,
    /// stddev, min, max). Multi-line. Intended for shutdown output.
    pub fn flush_gpu_timer_summary(&self) -> String {
        self.gpu_timer.flush_lifetime_summary()
    }

    /// Format and reset the per-zone CPU timing window. Mirrors
    /// `flush_gpu_timer_log` for the CPU side. Returns a one-line
    /// string suitable for the FPS log block.
    pub fn flush_cpu_timer_log(&mut self) -> String {
        self.cpu_timer.flush_log()
    }

    /// Format the CPU timer's lifetime per-zone stats. Multi-line;
    /// intended for shutdown output.
    pub fn flush_cpu_timer_summary(&self) -> String {
        self.cpu_timer.flush_lifetime_summary()
    }

    /// v0.24.2c.9: structured per-GPU-zone lifetime stats for the
    /// `.unendresult.json` writer. Pass-through to the GPU timer.
    pub fn gpu_zone_percentiles(&self) -> std::collections::BTreeMap<&'static str, crate::render::gpu_timer::ZonePercentiles> {
        self.gpu_timer.lifetime_zone_percentiles()
    }

    /// v0.24.2c.9: structured per-CPU-zone lifetime stats for the
    /// `.unendresult.json` writer. Pass-through to the CPU timer.
    pub fn cpu_zone_percentiles(&self) -> std::collections::BTreeMap<&'static str, crate::render::gpu_timer::ZonePercentiles> {
        self.cpu_timer.lifetime_zone_percentiles()
    }

    /// Get the chunk version stamped on a currently-uploaded mesh for
    /// `coord`, if any. Used by the chunk-streaming loop to detect
    /// stale meshes after a block edit and trigger a re-mesh while
    /// keeping the old mesh visible (no eviction-flash).
    ///
    /// F3: returns the version of the most-detailed mesh present
    /// (lowest LodLevel discriminant — Near beats Mid beats Far,
    /// etc.). The streaming loop's stale-detection logic doesn't
    /// care which LOD the mesh is, only whether the chunk's data has
    /// advanced past it.
    pub fn mesh_version_for(&self, coord: ChunkCoord) -> Option<u64> {
        // Iterate all LODs in order (Near, Mid, Far, VeryFar) and
        // return the version of the first one we find. This is O(4)
        // in the worst case which is fine for a per-chunk scan.
        for lod in crate::world::lod::LodLevel::iter() {
            if let Some(m) = self.meshes.get(
                &crate::world::lod::MeshSlotKey::at(coord, lod)
            ) {
                return Some(m.version);
            }
        }
        None
    }

    /// v0.24.72: like `mesh_version_for_lod`, but ALSO returns the
    /// version of any "meshed empty" record FOR THIS SPECIFIC LOD.
    /// Used by the dispatcher to detect "this chunk has been meshed
    /// at this LOD at version V and the result was empty" —
    /// preventing endless re-mesh loops for all-air / fully-buried
    /// chunks at this LOD.
    ///
    /// LOD-specific because "empty at Near" doesn't guarantee
    /// "empty at Far" — different LODs see different downsampled
    /// views and may emit different face counts. An all-air chunk
    /// will be empty at all LODs (and Chunk::cached_all_air is the
    /// right invalidation signal there), but a fully-buried stone
    /// chunk at Near could emit faces at Far if its Far neighbors
    /// differ.
    ///
    /// Returns `Some(v)` if (coord, lod) has either a stored mesh OR
    /// an empty-mesh record. Returns `None` only if neither exists.
    ///
    /// Precedence: real mesh wins over empty record.
    pub fn mesh_or_empty_version_for_lod(
        &self,
        coord: ChunkCoord,
        lod: crate::world::lod::LodLevel,
    ) -> Option<u64> {
        let key = crate::world::lod::MeshSlotKey::at(coord, lod);
        if let Some(m) = self.meshes.get(&key) {
            return Some(m.version);
        }
        self.empty_mesh_versions.get(&key).copied()
    }

    /// r703: record a chunk as "meshed empty at this version" WITHOUT running the
    /// mesher — used by the fully-buried-chunk cull so a totally-occluded chunk is
    /// marked done (via `mesh_or_empty_version_for_lod`) and stops being re-picked
    /// as a mesh candidate every frame. Identical bookkeeping to what the empty-
    /// mesh branch of the upload path does; no GPU work. When the chunk (or a
    /// neighbour) is edited, `chunk.version` bumps past this recorded version, so
    /// it is re-picked and meshed normally — exposing the newly-uncovered faces.
    pub fn mark_meshed_empty(
        &mut self,
        coord: ChunkCoord,
        lod: crate::world::lod::LodLevel,
        version: u64,
    ) {
        let key = crate::world::lod::MeshSlotKey::at(coord, lod);
        self.empty_mesh_versions.insert(key, version);
    }

    /// v0.24.72: like `mesh_version_for`, but ALSO checks the
    /// empty-mesh records across all LODs. Returns the highest
    /// version found across both maps.
    ///
    /// **Less precise than `mesh_or_empty_version_for_lod`** — use
    /// the LOD-specific variant when you know the target LOD. This
    /// any-LOD form exists for diagnostic / "is this chunk known
    /// at all?" callers that don't care about LOD precision.
    pub fn mesh_or_empty_version_for(&self, coord: ChunkCoord) -> Option<u64> {
        // Check real meshes first.
        if let Some(v) = self.mesh_version_for(coord) {
            return Some(v);
        }
        // Then empty-mesh records (any LOD).
        for lod in crate::world::lod::LodLevel::iter() {
            if let Some(v) = self.empty_mesh_versions.get(
                &crate::world::lod::MeshSlotKey::at(coord, lod)
            ) {
                return Some(*v);
            }
        }
        None
    }

    /// Per-LOD version lookup. Used during transitions where a caller
    /// wants to know specifically "do I have the Far mesh for this
    /// chunk yet?" rather than "is anything uploaded for this chunk?".
    pub fn mesh_version_for_lod(
        &self,
        coord: ChunkCoord,
        lod: crate::world::lod::LodLevel,
    ) -> Option<u64> {
        self.meshes.get(
            &crate::world::lod::MeshSlotKey::at(coord, lod)
        ).map(|m| m.version)
    }

    /// Tick the deferred-destroy queue and free anything ripe. Slots
    /// that have cycled past `MAX_FRAMES_IN_FLIGHT` are returned to the
    /// mesh buffer pool for reuse — no GPU resource destruction; that
    /// only happens when the pool itself is torn down at shutdown.

    /// v0.24.0b: write view+projection matrices for each given slot
    /// and dispatch the GPU view-derivation shader. After this call,
    /// each slot's `view_proj`, `inverse_view_proj`, and frustum planes
    /// are GPU-derived and visible to downstream shader reads.
    ///
    /// Caller must NOT be inside a render pass.
    ///
    /// `slots` is a slice of `(view_slot, view_matrix, projection_matrix)`.
    /// Each matrix is column-major (matches glm::Mat4 and the GLSL layout).
    /// CPU writes the matrices to the host-visible buffer, then a compute
    /// dispatch derives the rest. A single shader-write→shader-read
    /// barrier at the end ensures the cull pass sees the derived planes.
    unsafe fn prepare_views(
        &self,
        ctx: &VulkanContext,
        cmd: vk::CommandBuffer,
        slots: &[(u32, glm::Mat4, glm::Mat4)],
    ) {
        if slots.is_empty() { return; }

        // Step 1: CPU writes view+projection matrices for each slot.
        // Host-coherent writes are visible to the GPU at submit time;
        // no explicit barrier is needed between this and the dispatch.
        for (slot, view, projection) in slots {
            let view_arr = mat4_to_col_array(view);
            let proj_arr = mat4_to_col_array(projection);
            self.view_params.write_view_proj_pair(*slot, &view_arr, &proj_arr);
        }

        // Step 2: dispatch view_compute.comp once per slot. Each
        // dispatch is one invocation that reads matrices, derives
        // view_proj/inverse_view_proj/planes, writes back to the same
        // slot.
        for (slot, _, _) in slots {
            self.view_compute_pipeline.record_dispatch(cmd, *slot);
        }

        // Step 3: barrier — view_compute's writes to the ViewParams
        // buffer must be visible to the subsequent cull dispatches
        // that read .planes from the same buffer.
        let view_to_cull = vk::BufferMemoryBarrier::builder()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(self.view_params.buffer)
            .offset(0)
            .size(vk::WHOLE_SIZE)
            .build();
        ctx.device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            std::slice::from_ref(&view_to_cull),
            &[],
        );
    }

    /// v0.24.2b: dispatch the GPU cascade-fit shader for every cascade
    /// of `view_group`, then run view-derivation on those slots so
    /// planes/view_proj/inverse_view_proj are downstream-ready.
    ///
    /// Caller is responsible for camera basis (`forward`, `up`,
    /// `right`) being unit-length and orthogonal — this is what the
    /// GLSL `frustum_corners` math assumes. Split distances are passed
    /// as a slice of `NUM_CASCADES` `(near, far)` pairs.
    ///
    /// Self-contained: emits two barriers internally. The cascade_fit
    /// → view_compute hop and the view_compute → cull hop. The latter
    /// is redundant if `prepare_views` follows immediately (its barrier
    /// covers compute writes anyway), but keeping it makes
    /// `prepare_cascades` callable independently — important for
    /// future multi-camera code that might run cascades and main
    /// concurrently per-group.
    ///
    /// Caller must NOT be inside a render pass.
    unsafe fn prepare_cascades(
        &self,
        ctx: &VulkanContext,
        cmd: vk::CommandBuffer,
        view_group: &crate::render::view_params::ViewGroup,
        cam_pos: glm::Vec3,
        cam_forward: glm::Vec3,
        cam_up: glm::Vec3,
        cam_right: glm::Vec3,
        fov_y_radians: f32,
        aspect: f32,
        sun_dir: glm::Vec3,
        cascade_splits: &[(f32, f32); crate::render::view_params::NUM_CASCADES],
        shadow_map_size: f32,
    ) {
        use crate::render::cascade_fit_pipeline::CascadeFitPushConstants;

        // Step 1: dispatch cascade_fit once per cascade. Each dispatch
        // writes (view, projection) into the cascade's slot.
        for ci in 0..crate::render::view_params::NUM_CASCADES {
            let (near_m, far_m) = cascade_splits[ci];
            let push = CascadeFitPushConstants::for_cascade(
                view_group, ci,
                [cam_pos.x, cam_pos.y, cam_pos.z],
                [cam_forward.x, cam_forward.y, cam_forward.z],
                [cam_up.x, cam_up.y, cam_up.z],
                [cam_right.x, cam_right.y, cam_right.z],
                fov_y_radians, aspect,
                [sun_dir.x, sun_dir.y, sun_dir.z],
                near_m, far_m, shadow_map_size,
            );
            self.cascade_fit_pipeline.record_dispatch(cmd, &push);
        }

        // Step 2: barrier — cascade_fit's writes to view+projection
        // must be visible to view_compute's reads of the same fields.
        let fit_to_view = vk::BufferMemoryBarrier::builder()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(self.view_params.buffer)
            .offset(0)
            .size(vk::WHOLE_SIZE)
            .build();
        ctx.device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            std::slice::from_ref(&fit_to_view),
            &[],
        );

        // Step 3: dispatch view_compute once per cascade slot to
        // derive view_proj / inverse_view_proj / frustum planes.
        for ci in 0..crate::render::view_params::NUM_CASCADES {
            self.view_compute_pipeline.record_dispatch(cmd, view_group.cascade_slots[ci]);
        }

        // Step 4: barrier — view_compute's writes to view_proj/planes
        // must be visible to subsequent cull dispatches that read them.
        let view_to_cull = vk::BufferMemoryBarrier::builder()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(self.view_params.buffer)
            .offset(0)
            .size(vk::WHOLE_SIZE)
            .build();
        ctx.device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            std::slice::from_ref(&view_to_cull),
            &[],
        );
    }

    /// Record the GPU-cull preamble for one render pass.
    ///
    /// Resets the visible-count counter to 0, then dispatches the cull
    /// compute shader against the given frustum. Each invocation reads
    /// one chunk's metadata; visible chunks atomically claim a slot in
    /// the indirect-commands buffer and write a `VkDrawIndexedIndirect`
    /// entry there.
    ///
    /// Inserts the necessary barriers:
    /// - TRANSFER → COMPUTE_SHADER on the count buffer (so the shader's
    ///   atomic increments don't race with the prior fill_buffer).
    /// - COMPUTE_SHADER → DRAW_INDIRECT on the commands+count buffers
    ///   (so the subsequent draw call sees the shader's writes).
    ///
    /// **Must be called outside any render pass** — `cmd_dispatch` and
    /// `cmd_fill_buffer` are not allowed inside `vkCmdBeginRenderPass`/
    /// `vkCmdEndRenderPass`.
    ///
    /// v0.24.0b: this no longer writes planes. Caller must have already
    /// called `prepare_views` for this frame, which dispatches the
    /// GPU view-derivation shader for each active slot and emits a
    /// shader-write→shader-read barrier. The cull shader reads the
    /// derived planes from the SSBO at binding 3.
    unsafe fn record_cull_pass(
        &self,
        ctx: &VulkanContext,
        cmd: vk::CommandBuffer,
        view_slot: u32,
        // v0.24.4: shadow temporal-stride support. Main cull always
        // passes (1, 0); shadow cull passes (UNENDLICH_SHADOW_CHUNK_STRIDE,
        // frame_count % stride). When stride > 1 the cull shader admits
        // only chunks where (slot_index % stride) == stride_phase.
        stride: u32,
        stride_phase: u32,
    ) {
        // v0.24.2c: when this is called multiple times per frame (for
        // sequential per-cascade culls), the PRIOR cascade's draw is
        // still INDIRECT_COMMAND_READ-ing the commands+count buffers
        // when we're about to start writing them again. Insert a
        // DRAW_INDIRECT → COMPUTE_SHADER barrier so we serialise
        // draw-N-of-the-previous-cull against cull-write-of-this-call.
        //
        // For the very first call of the frame this barrier is a no-op
        // (no prior indirect read in flight). Vulkan handles that
        // correctly — the synchronisation guarantees still hold.
        let prev_draw_to_this_cull = [
            vk::BufferMemoryBarrier::builder()
                .src_access_mask(vk::AccessFlags::INDIRECT_COMMAND_READ)
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE
                    | vk::AccessFlags::TRANSFER_WRITE)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.indirect.commands_buffer)
                .offset(0)
                .size(vk::WHOLE_SIZE)
                .build(),
            vk::BufferMemoryBarrier::builder()
                .src_access_mask(vk::AccessFlags::INDIRECT_COMMAND_READ)
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE
                    | vk::AccessFlags::TRANSFER_WRITE)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.indirect.count_buffer)
                .offset(0)
                .size(vk::WHOLE_SIZE)
                .build(),
        ];
        ctx.device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::DRAW_INDIRECT,
            vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &prev_draw_to_this_cull,
            &[],
        );

        // Step 1: zero the count buffer.
        self.indirect.record_reset_count(cmd);

        // Step 2: barrier — fill_buffer's TRANSFER_WRITE must be visible
        // to the cull shader's atomicAdd.
        let to_compute = vk::BufferMemoryBarrier::builder()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(
                vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(self.indirect.count_buffer)
            .offset(0)
            .size(vk::WHOLE_SIZE)
            .build();
        ctx.device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            std::slice::from_ref(&to_compute),
            &[],
        );

        // Step 3: dispatch cull. Push constant: (view_index, chunk_count,
        // stride, stride_phase).
        let push = crate::render::chunk_indirect::CullPushConstants {
            view_index: view_slot,
            chunk_count: self.indirect.high_water(),
            stride,
            stride_phase,
        };
        self.indirect.record_cull_dispatch(cmd, &push, self.current_frame);

        // Step 4: barrier — the cull shader's writes to the commands
        // and count buffers must be visible to the indirect-draw stage
        // before the subsequent draw.
        let cmds_to_indirect = vk::BufferMemoryBarrier::builder()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags::INDIRECT_COMMAND_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(self.indirect.commands_buffer)
            .offset(0)
            .size(vk::WHOLE_SIZE)
            .build();
        let count_to_indirect = vk::BufferMemoryBarrier::builder()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags::INDIRECT_COMMAND_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(self.indirect.count_buffer)
            .offset(0)
            .size(vk::WHOLE_SIZE)
            .build();
        ctx.device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::DRAW_INDIRECT,
            vk::DependencyFlags::empty(),
            &[],
            &[cmds_to_indirect, count_to_indirect],
            &[],
        );
    }

    fn process_pending_destroys(&mut self, _ctx: &VulkanContext) {
        let mut i = 0;
        while i < self.pending_destroy.len() {
            if self.pending_destroy[i].frames_remaining == 0 {
                let pd = self.pending_destroy.swap_remove(i);
                if let Err(e) = self.mesh_pool.free(pd.slot) {
                    log::warn!("Failed to return mesh pool slot: {}", e);
                }
                self.indirect.free(pd.draw_slot);
            } else {
                self.pending_destroy[i].frames_remaining -= 1;
                i += 1;
            }
        }
    }

    /// Render the world. Reads the View pose from `self.camera` for
    /// backward compatibility — existing call sites can keep doing
    /// `renderer.camera.position = ...` and call `draw_frame`.
    ///
    /// New code should prefer `draw_frame_with_view` (below) which
    /// takes a `View` by parameter — this is the multiplayer-prep
    /// path: a single renderer can be driven by multiple Views (e.g.
    /// for split-screen render passes, picture-in-picture, etc.)
    /// without the renderer owning any view state.
    /// Default-atmosphere convenience wrapper. Uses
    /// `AtmosphereParams::neutral()` (mid-day clear sky), suitable for
    /// tests and headless paths that have no climate data.
    /// Upload this frame's baked entity geometry (from
    /// [`crate::world::entity::snapshot_to_vertices`]). Call after the frame
    /// fence wait and before `draw_frame`, since the entity buffers are
    /// single-buffered host memory. Infallible: a failed upload just skips the
    /// mobs this frame rather than killing the frame.
    pub fn upload_entities(
        &mut self,
        ctx: &VulkanContext,
        verts: &[crate::world::entity::EntityVertex],
        indices: &[u32],
    ) {
        if let Err(e) = self.entity_pipeline.upload(&ctx.device, verts, indices) {
            log::warn!("entity upload failed, skipping mobs this frame: {e:?}");
        }
    }

    /// r419 (flow-as-shader): set the coarse water-flow grid for the next frame's UBO.
    /// `coverage` = [origin_x, origin_z, blocks_per_cell, enable]; `grid` is up to 128 vec4
    /// (16×16 cells, 2 per vec4). With `coverage[3] == 0` the water shader ignores it.
    pub fn set_water_flow(&mut self, coverage: [f32; 4], grid: &[[f32; 4]]) {
        self.water_flow_coverage = coverage;
        let m = grid.len().min(128);
        self.water_flow_grid[..m].copy_from_slice(&grid[..m]);
        for v in self.water_flow_grid[m..].iter_mut() {
            *v = [0.0; 4];
        }
    }

    /// r431: upload a 16×16 water-depth grid (blocks) + coverage so the surface shader can
    /// shade by real depth. `cov` = [origin_x, origin_z, blocks_per_cell, _]. cells row-major.
    pub fn set_water_depth(&mut self, cov: [f32; 3], cells: &[f32; 256]) {
        self.water_depth_cov = [cov[0], cov[1], cov[2], 1.0];
        for v in 0..64 {
            self.water_depth_grid[v] = [
                cells[v * 4],
                cells[v * 4 + 1],
                cells[v * 4 + 2],
                cells[v * 4 + 3],
            ];
        }
    }

    /// r427: set the underwater screen-FX state for the next frame (submerged 0/1, time s).
    pub fn set_underwater(&mut self, submerged: f32, time: f32) {
        self.fx_params = [submerged, time, 0.0, 0.0];
    }

    /// r499: set the world season phase [0,1) for the foliage seasonal tint.
    pub fn set_season(&mut self, season01: f32) {
        self.world_season01 = season01.rem_euclid(1.0);
    }

    pub fn draw_frame(&mut self, ctx: &VulkanContext) -> Result<bool> {
        let view = self.camera.as_view();
        let atmos = AtmosphereParams::neutral();
        self.draw_frame_with_view_and_atmos(ctx, &view, &atmos)
    }

    /// Render with explicit View, neutral atmosphere. Kept for callers
    /// that want a custom View but don't yet plumb climate data.
    pub fn draw_frame_with_view(&mut self, ctx: &VulkanContext, view: &View) -> Result<bool> {
        let atmos = AtmosphereParams::neutral();
        self.draw_frame_with_view_and_atmos(ctx, view, &atmos)
    }

    /// Render with explicit View AND atmosphere state. The bin should
    /// call this from its main loop with climate-derived values so the
    /// fragment shader can react to humidity/temperature/wind/sun.
    pub fn draw_frame_with_view_and_atmos(
        &mut self,
        ctx: &VulkanContext,
        view: &View,
        atmos: &AtmosphereParams,
    ) -> Result<bool> {
        // v0.23.0: CPU timer for the render thread. Marks zone
        // boundaries so we can see where wall-clock time goes when
        // the GPU is fast (post-LOD: GPU mean ~2.4ms, but CPU draw
        // wall-clock ~1000ms/sec — gap is here somewhere).
        self.cpu_timer.begin_frame();

        // v0.24.4 (shadow-stride): monotonic frame counter, used by
        // the shadow temporal-stride logic below (and emitted in
        // events). Independent of FIF slot.
        self.monotonic_frame = self.monotonic_frame.wrapping_add(1);
        let monotonic_frame = self.monotonic_frame;

        // Tick deferred destroys at the start of the frame.
        self.process_pending_destroys(ctx);

        let aspect = self.swapchain.extent.width as f32 / self.swapchain.extent.height as f32;
        let vp = view.view_projection(aspect);
        let frustum = Frustum::from_view_projection(&vp);

        // v0.24.0b: also compute view and projection separately. The GPU
        // `view_compute.comp` shader takes them and derives view_proj +
        // frustum planes on-device. The CPU `frustum` above is still
        // used by the fallback non-indirect cull path (UNENDLICH_INDIRECT=0),
        // and as a sanity reference for the GPU result.
        let view_mat = view.view_matrix();
        let proj_mat = view.projection_matrix(aspect);

        let frame_idx = self.current_frame;
        let frame = &self.frames[frame_idx];

        // v0.24.3 chug investigation: capture wait_fences side-channel.
        //
        // What this measures:
        //  - wait_ms: how long the CPU sat in vkWaitForFences for the
        //    previous submission to this slot to complete.
        //  - last_submit_to_wait_ms: time between submitting the work
        //    we're now waiting on, and starting the wait. Small =
        //    submitted then immediately waited (GPU was the bottleneck).
        //    Large = submitted, did other CPU work, then waited (CPU
        //    side was the bottleneck for that interval).
        //
        // Emits an event when wait_ms > 10ms. The threshold is
        // intentionally generous so steady-state under-budget frames
        // don't fill events.jsonl. We're hunting tail-latency outliers.
        //
        // Read-only — does NOT change scheduling. Per c.17-a's
        // cautionary tale, instrumentation must not perturb behavior
        // while we're trying to diagnose existing behavior.
        let wait_start = std::time::Instant::now();
        let prev_submit_at = frame.last_submit_at;
        unsafe {
            ctx.device.wait_for_fences(std::slice::from_ref(&frame.in_flight), true, u64::MAX)?;
        }
        // r476: the GPU is now finished with this frame slot, so its metadata
        // copy is safe to write. Sync the chunk-draw entries that changed since
        // this slot last rendered, BEFORE the cull/vertex shaders read them.
        // At frames_in_flight==1 this is the same single buffer as before.
        self.indirect.flush_frame(frame_idx);
        let wait_dur = wait_start.elapsed();
        let wait_ms = wait_dur.as_secs_f64() * 1000.0;
        // v0.24.63-adaptive-uploads: expose this measurement on the
        // renderer so the bin's adaptive controller can read it next
        // frame. Pure side-effect; no behavioural change here. The
        // event emission below is unchanged.
        self.last_wait_fences_ms = wait_ms;
        if wait_ms > 10.0 {
            let last_submit_to_wait_ms = prev_submit_at
                .map(|t| (wait_start - t).as_secs_f64() * 1000.0);
            unendlich_shared::events::record(
                "wait_fences_spike",
                serde_json::json!({
                    "frame_idx": frame_idx,
                    "wait_ms": wait_ms,
                    "last_submit_to_wait_ms": last_submit_to_wait_ms,
                }),
            );
        }
        self.cpu_timer.mark(crate::render::cpu_timer::CpuZone::WaitFences);

        // ── In-game egui overlay: PRE-PASS prepare. The fence wait above
        // guarantees the previous GPU use of ring slot `frame_idx` is complete,
        // so overwriting that slot's vertex/index buffer now is safe (no
        // write-while-in-use). Runs egui + uploads any new textures (a queue
        // submit on the first frame) — still before begin_command_buffer / the
        // render pass. `record` replays the draw list in-pass.
        #[cfg(feature = "ui-overlay")]
        {
            use crate::world::blocks::BlockId;
            // Curated creative item set. BlockId discriminants are sparse, so this
            // is an explicit list rather than a 0..N range. Spawn eggs await the
            // mob/entity pipeline; these blocks are placeable today.
            const ITEMS: &[BlockId] = &[
                BlockId::Stone, BlockId::Grass, BlockId::Dirt, BlockId::Cobblestone,
                BlockId::Sand, BlockId::Gravel, BlockId::Clay, BlockId::Moss,
                BlockId::OakLog, BlockId::OakLeaves, BlockId::BirchLog, BlockId::BirchLeaves,
                BlockId::Cactus, BlockId::Flower, BlockId::Vine, BlockId::SnowBlock,
                BlockId::GoldOre, BlockId::IronOre, BlockId::CoalOre, BlockId::DiamondOre,
                BlockId::Amber, BlockId::Crystal,
            ];
            let ext = self.swapchain.extent;
            // Gather telemetry into Copy locals BEFORE the closure so it captures
            // only locals (no borrow of `self`, which `prepare` holds mutably via
            // `self.egui_overlay`).
            let hud_frame = self.monotonic_frame;
            let hud_chunks = self.meshes.len();
            let hud_wait_ms = self.last_wait_fences_ms;
            let hud_pos = view.position;
            let hud_slot = frame_idx;
            // Ambient light from the atmosphere (sun intensity, day→1 night→0).
            // Drives Theme::lit so in-game panels dim at night, brighten by day.
            let hud_ambient = atmos.sun_intensity.clamp(0.0, 1.0);
            // Local copy of the picker selection so the closure mutates a LOCAL
            // (captured &mut), never `self`. Written back after prepare returns.
            // The active tab chooses the catalog; eggs come from the item module,
            // everything else is the block list above wrapped as items. Rendered
            // uniformly via ItemKind::display_color (UI brick, pure-tested).
            use crate::inventory_screen::Tab;
            let mut creative_tab = self.creative_tab;
            // r638: every creative tab is populated by the categorized backend
            // (item::creative_palette → creative_items over the full catalog).
            // Inventory returns empty (the live grid is drawn separately below).
            let items: Vec<crate::item::ItemKind> = crate::item::creative_palette(creative_tab);
            let item_count = items.len().max(1);
            let mut selected = self.creative_selected.min(item_count - 1);
            // r655/r657: working copies of the drag-free creative hotbar; written back below.
            let mut creative_hotbar = self.creative_hotbar;
            let mut creative_inventory = self.creative_inventory;
            let mut creative_hotbar_sel = self.creative_hotbar_sel.min(9);
            let mut creative_place_item = self.creative_place_item;
            // #1: the creative picker is an overlay you interact with only in
            // panel mode; when the mouse is captured for look (game mode) it must
            // disappear. `ui_pointer_active` mirrors panel mode. The hotbar stays.
            // r577: the `p`/creative panel is one EXCLUSIVE surface — it must yield to
            // settings, pause, debug, crafting, the title and the wizard rather than
            // drawing on top of them (was `= ui_pointer_active`, which is true for ANY
            // open UI, so the panel stacked over settings). At the state level these are
            // already mutually exclusive; this makes the render agree.
            let show_creative = self.ui_pointer_active
                && !self.settings_menu_open
                && !self.pause_menu_open
                && !self.settings_open
                && !self.perf_window_open
                && !self.craft_menu_open
                && !self.character_menu_open
                && !self.menu_open
                && !self.wizard_open;
            let settings_open = self.settings_open;
            let mut settings_menu_open_local = self.settings_menu_open; // r203 (menu opens tabbed settings)
            // r167: a clean title — no in-world HUD while the menu is up.
            let hud_status_lines = if self.menu_open {
                Vec::new()
            } else {
                self.hud_status_lines.clone()
            };
            // r545: survival meters local (hidden while a menu is up).
            let hud_survival = if self.menu_open { None } else { self.hud_survival };
            // Title/pause menu: copy the flag to a Copy local (the closure can't
            // borrow self while prepare holds it mutably); the click is collected
            // into menu_pick and written back to self.menu_choice after prepare.
            let menu_open = self.menu_open;
            let pause_menu_open = self.pause_menu_open;
            let mut pause_pick: u8 = 0;
            let mut wizard_name_buf = self.wizard_name_buf.clone();
            let mut ui_plant_stride = self.ui_plant_stride;
            let mut ui_leaf_stride = self.ui_leaf_stride;
            let mut ui_apply_budget = self.ui_apply_budget;
            let mut ui_perf_preset = self.ui_perf_preset;
            let mut ui_render_distance = self.ui_render_distance; // r197
            let mut ui_render_scale = self.ui_render_scale; // r325
            let mut ui_fps_target = self.ui_fps_target;
            let mut ui_autotune = self.ui_autotune;
            let mut ui_gpu_mesh_yield = self.ui_gpu_mesh_yield;
            let mut ui_adaptive_rd = self.ui_adaptive_rd;
            let mut ui_octree_tile = self.ui_octree_tile;
            let mut ui_detached_terrain = self.ui_detached_terrain;
            let mut wizard_seed_buf = self.wizard_seed_buf.clone();
            // r181/r182: preview upload happens INSIDE the closure (ctx
            // lives there); captured locals + write-back, like the latches.
            let mut wizard_preview_tex = self.wizard_preview_tex.clone();
            let wizard_preview_dirty =
                self.wizard_preview_dirty && self.wizard_preview_rgb.len() == 96 * 48 * 3;
            let wizard_preview_rgb = if wizard_preview_dirty {
                self.wizard_preview_rgb.clone()
            } else {
                Vec::new()
            };
            let mut wizard_preview_uploaded = false;
            let mut wants_keyboard_out = false;
            let mut wizard_preview_click_out: Option<(f32, f32, bool)> = None;
            let loading_fade = self.loading_fade;
            let loading_label = self.loading_label.clone();
            // r187: destiny-esque menu button — full-width hit area, an
            // accent bar that grows on hover (animate_bool), letterspaced
            // caps. Kit v1; sounds + sweep shaders join at frosted.
fn dest_button(ui: &mut egui::Ui, label: &str) -> bool {
                // r317: the front/title menu uses the candidate button (style 1 default).
                let accent = unendlich_ui::current_theme(ui.ctx()).accent;
                unendlich_ui::button_sized(ui, label, accent, egui::vec2(220.0, 34.0)).clicked()
            }
            // r181: shared translucent window chrome ("glass" until the
            // frosted-blur PostFx pass lands).
            // r190: glass v2 — per-window open interpolation (margin grows
            // 6->14 + fill alpha ramps over ~200ms: the "geometry change
            // needs interpolation" note) and a fresnel pass painted after
            // content (bright top edge, falloff sides). DOF = PostFx.
            let glass_t = |c: &egui::Context, id: &str, open: bool| -> f32 {
                c.animate_bool(egui::Id::new((id, "glass_open")), open)
            };
            // r233: reusable cone-parallax for any menu window — the panel
            // drifts a few px toward the cursor (cursor = cone apex), smoothed
            // for weight, stored per-id across frames.
            //
            // r245 REGRESSION FIX: applying this as the Window .anchor() offset
            // made the panel DRIFT under the cursor every frame, which broke
            // button clicks (the operator reported the main-menu buttons dead —
            // couldn't create/load worlds). A cursor-driven anchor offset moves
            // the buttons' hit rects out from under the click, and the r234
            // repaint-gating compounds it. Cosmetic parallax is not worth
            // breaking core navigation, so the offset is forced to ZERO here —
            // restoring the proven pre-r229 fixed-center anchor for ALL menu
            // windows — while the helper + the menu::cone_offset/parallax_lerp
            // kit (still unit-tested) stay for a future SAFE re-enable (a
            // visual-only transform that does NOT move interaction rects, e.g.
            // translating the painter or the panel contents rather than the
            // window anchor). See docs/design/menu_parallax_regression.md.
            let cone_par = |_c: &egui::Context, _id: &str, _z: f32| -> egui::Vec2 {
                egui::Vec2::ZERO
            };
            let glass_at = |t01: f32| unendlich_ui::glass_at(t01); // r191 kit
            let glass = glass_at(1.0);
            // fresnel: call INSIDE a window's show with its outer rect.
fn fresnel(ui: &egui::Ui, r: egui::Rect, a: f32) {
                unendlich_ui::fresnel(ui, r, a) // r191: kit (v2 sheen)
            }
            let _ = (&glass_t, &glass_at, &fresnel); // adopted per-window below
            // r169: settings/craft/perf window state in+out.
            let settings_menu_open = self.settings_menu_open;
            let mut settings_tab = self.settings_tab;
            let mut ui_day_length = self.ui_day_length;
            let mut ui_game_speed_pos = self.ui_game_speed_pos;
            let mut ui_music_vol = self.ui_music_vol;
            let mut ui_fog_on = self.ui_fog_on;
            let mut ui_shadows_on = self.ui_shadows_on;
            let mut ui_debug_master = self.ui_debug_master;
            let mut ui_debug_water = self.ui_debug_water;
            let mut ui_debug_flow = self.ui_debug_flow;
            let mut ui_debug_echo = self.ui_debug_echo;
            let mut settings_close = false;
            let craft_menu_open = self.craft_menu_open;
            let craft_lines = self.craft_lines.clone();
            // r576: survival backpack snapshot for the centred inventory panel.
            // r662: positional slots + held cursor for the interactive inventory.
            let inv_slots = self.inv_slots.clone();
            let inv_hotbar_size = self.inv_hotbar_size.max(1);
            let inv_cursor = self.inv_cursor.clone();
            let mut inv_click: i32 = -1; // set by a slot click below; written back to self
            let survival_inv = self.hud_survival.is_some();
            let mut craft_pick: u8 = 0;
            let mut craft_close = false;
            let perf_window_open = self.perf_window_open;
            let perf_samples = self.perf_samples.clone();
            let perf_stats = self.perf_stats.clone();
            let mut perf_export = false;
            let mut menu_pick: u32 = 0;
            // New-game wizard: copy display state to locals; collect the click.
            let wizard_open = self.wizard_open;
            // r205: rising edge of wizard_open -> focus the name field once.
            let wizard_focus_name = wizard_open && !self.wizard_was_open;
            self.wizard_was_open = wizard_open;
            let wizard_title = self.wizard_title.clone();
            let wizard_summary = self.wizard_summary.clone();
            let wizard_can_next = self.wizard_can_next;
            let wizard_at_confirm = self.wizard_at_confirm;
            let menu_screen_kind = self.menu_screen_kind;
            let menu_world_list = self.menu_world_list.clone(); // r197
            let menu_mp_list = self.menu_mp_list.clone(); // r394: discovered LAN hosts
            let mut menu_world_pick_local: i32 = -1; // r197 (write-back below)
            let mut menu_mp_pick_local: i32 = -1; // r397 (write-back below)
            let menu_rollback_list = self.menu_rollback_list.clone(); // r203
            let menu_show_rollbacks = self.menu_show_rollbacks; // r203
            let mut menu_rollback_peek_local: i32 = -1; // r203 (which world's history)
            let mut menu_rollback_restore_local: i32 = -1; // r203 (which point to restore)
            let menu_world_delete_ask = self.menu_world_delete_ask; // r205 (read)
            let dev_mode_on = crate::dev_mode::is_dev(); // r206 (gates dev UI)
            let mut dev_make_point_local = false; // r206 (force history point)
            let mut menu_rollback_delete_local: i32 = -1; // r206 (delete a point)
            let mut menu_world_delete_ask_local: i32 = -1; // r205 (arm confirm)
            let mut menu_world_delete_confirm_local: i32 = -1; // r205 (do delete)
            let mut menu_world_delete_cancel_local = false; // r205 (dismiss confirm)
            let menu_keybind_list = self.menu_keybind_list.clone(); // r199
            let keybind_capturing = self.keybind_capturing; // r199 (read)
            // r488: character-select snapshot (closure can't borrow self) + action latches.
            let character_menu_open = self.character_menu_open;
            let mut char_cards: Vec<(String, String, bool)> = Vec::new(); // (name, plan, is_custom)
            let mut char_selected: usize = 0;
            let mut char_custom_index: usize = 0;
            let mut char_stat = String::new();
            let mut char_preview: Vec<crate::creature_viewer::ProjectedBox> = Vec::new();
            // r538: live-edited copy of the custom slot's body-build sliders.
            let mut char_build_edit = crate::player_character::BuildParams::default();
            let mut char_is_hero = false;
            if character_menu_open {
                if let Some(roster) = self.character_roster.as_ref() {
                    char_selected = roster.selected;
                    char_custom_index = roster.custom_index;
                    char_build_edit = roster.current().build;
                    char_is_hero = crate::hero_forge::HeroKind::from_name(
                        &roster.current().name,
                    ).is_some();
                    for (i, c) in roster.options.iter().enumerate() {
                        char_cards.push((c.name.clone(), c.arche.clone(), i == roster.custom_index));
                    }
                    let cur = roster.current();
                    char_stat = format!("{} · {} · seed {:#x}", cur.name, cur.arche, cur.seed);
                    // r541: heroes preview their actual voxel mesh (shaped by the build
                    // sliders), not the generic blueprint silhouette.
                    char_preview = if let Some(kind) =
                        crate::hero_forge::HeroKind::from_name(&cur.name)
                    {
                        crate::hero_forge::preview_boxes(kind, &cur.build)
                    } else {
                        crate::creature_viewer::project(
                            &cur.blueprint(),
                            crate::creature_viewer::ViewAxis::Side,
                        )
                    };
                    char_preview.sort_by(|a, b| {
                        a.depth.partial_cmp(&b.depth).unwrap_or(std::cmp::Ordering::Equal)
                    });
                }
            }
            let mut char_pick_local: i32 = -1;
            let mut char_reroll_local = false;
            let mut char_save_local = false;
            let mut char_close_local = false;
            let mut char_cycle_arche_local = false;
            let mut char_tint_local: Option<[f32; 3]> = None;
            let mut char_build_local: Option<crate::player_character::BuildParams> = None;
            let mut open_character_local = false;
            let mut keybind_rebind_local: i32 = -1; // r199 (write-back below)
            let mut ui_scale_local = self.ui_scale; // r200 GUI scale (write-back below)
            let mut ui_scale_staged = self.ui_scale; // r202: slider edits this; commits on release
            let mut ui_scale_commit = false; // r202: only write self.ui_scale when true
            let mut wizard_pick: u32 = 0;
            // UI scale → egui pixels_per_point. The framebuffer is `ext` px; egui
            // works in logical *points* = px / scale, so a 2× scale gives a crisp
            // 2×-rasterized atlas and a 2×-larger UI. Positions below are points.
            let scale = self.ui_scale.clamp(1.0, 5.0);
            let pw = ext.width as f32 / scale;
            let ph = ext.height as f32 / scale;
            // Build pointer events for egui. Pos is in logical points (px/scale).
            // Only forwarded when `ui_pointer_active` so game-mode (cursor
            // captured, mouse drives camera) doesn't accidentally click UI.
            let mut events: Vec<egui::Event> = Vec::new();
            if self.ui_pointer_active {
                let pos = egui::pos2(
                    self.ui_pointer_pos.0 / scale,
                    self.ui_pointer_pos.1 / scale,
                );
                events.push(egui::Event::PointerMoved(pos));
                // r205: text/edit events MOVED to after the pointer-button
                // events below. egui processes a frame's events in order, so
                // a click that focuses a TextEdit must be handled BEFORE the
                // Text event or the very first keystrokes are dropped (the
                // field isn't focused yet when the text arrives). This was
                // "text entry not passing through" on a fresh click.
                if self.ui_pointer_down != self.ui_pointer_down_prev {
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: self.ui_pointer_down,
                        modifiers: egui::Modifiers::default(),
                    });
                }
                // r186: secondary button edges (right-click avoid-pick).
                if self.ui_pointer2_down != self.ui_pointer2_down_prev {
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Secondary,
                        pressed: self.ui_pointer2_down,
                        modifiers: egui::Modifiers::default(),
                    });
                }
                // Collapsed clicks → full press+release pairs at current pos.
                for _ in 0..self.ui_click_pending {
                    events.push(egui::Event::PointerButton {
                        pos, button: egui::PointerButton::Primary,
                        pressed: true, modifiers: egui::Modifiers::default(),
                    });
                    events.push(egui::Event::PointerButton {
                        pos, button: egui::PointerButton::Primary,
                        pressed: false, modifiers: egui::Modifiers::default(),
                    });
                }
                // r205: text + edit keys LAST, so any focusing click above is
                // processed first this frame (fixes dropped first keystrokes).
                if !self.ui_text_input.is_empty() {
                    events.push(egui::Event::Text(std::mem::take(&mut self.ui_text_input)));
                }
                if self.ui_backspace {
                    events.push(egui::Event::Key {
                        key: egui::Key::Backspace,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::default(),
                    });
                    self.ui_backspace = false;
                }
                if self.ui_enter {
                    events.push(egui::Event::Key {
                        key: egui::Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::default(),
                    });
                    self.ui_enter = false;
                }
            }
            self.ui_pointer_down_prev = self.ui_pointer_down;
            self.ui_pointer2_down_prev = self.ui_pointer2_down; // r186
            // r205: forward the scroll wheel to egui so ScrollAreas inside
            // dialogs actually scroll (operator: "scroll not passing to
            // dialogs"). egui 0.34 wants a MouseWheel event in points; one
            // wheel notch ≈ 48px feels right at this scale. The bin only
            // feeds this when a UI surface is active, so gameplay scroll
            // (hotbar) is unaffected.
            if self.ui_scroll_delta.abs() > 1e-4 || self.ui_scroll_delta_x.abs() > 1e-4 {
                events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(self.ui_scroll_delta_x * 48.0, self.ui_scroll_delta * 48.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::default(),
                });
                self.ui_scroll_delta = 0.0;
                self.ui_scroll_delta_x = 0.0;
            }
            let raw_input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_max(
                    egui::pos2(0.0, 0.0),
                    egui::pos2(pw, ph),
                )),
                events,
                ..Default::default()
            };
            self.egui_overlay.prepare(ctx, frame_idx, scale, raw_input, |c| {
                // r228: ambient surface is FROSTED for all menu UI — it draws
                // over the live 3D scene, so kit components composite
                // transparent-first (translucent fills, no opaque boxes).
                // Until the blur PostFx texture exists this is a translucent
                // wash; the blur pass upgrades it via frosted_blur(tex, tint).
                unendlich_ui::set_surface(c, unendlich_ui::Surface::frosted());
                // ── Title / pause menu (unendlich-fullgame) ──
                if menu_open {
                    {
                    let gt = glass_t(c, "unendlich_menu", true);
                    // r229/r233: cone-parallax drift toward the cursor.
                    let menu_par = cone_par(c, "unendlich_menu", 0.018);
                    egui::Window::new("unendlich_menu")
                        .frame(glass_at(gt))
                        .anchor(egui::Align2::CENTER_CENTER, menu_par)
                        .collapsible(false)
                        .resizable(false)
                        .title_bar(false)
                        .show(c, |ui| {
                            unendlich_ui::set_theme(ui.ctx(), unendlich_ui::Theme::DARK);
                            ui.vertical_centered(|ui| {
                                // r223: title in the blocky voxel game font
                                // (UnendlichBlocks), letterspaced caps — the
                                // mockup's hero treatment.
                                ui.label(
                                    egui::RichText::new("UNENDLICH")
                                        .font(unendlich_ui::menu::blocks_font(34.0))
                                        .color(unendlich_ui::INK)
                                        .extra_letter_spacing(6.0),
                                );
                                ui.add_space(14.0);
                                match menu_screen_kind {
                                    1 => {
                                        // r186: ConfirmQuit screen (was
                                        // undrawn -> quit "did nothing").
                                        ui.label("quit unendlich?");
                                        ui.add_space(8.0);
                                        if dest_button(ui, "Quit") { menu_pick = 5; }
                                        ui.add_space(4.0);
                                        if dest_button(ui, "Back") { menu_pick = 6; }
                                    }
                                    2 => {
                                        ui.label("multiplayer");
                                        ui.add_space(8.0);
                                        if dest_button(ui, "Host World") { menu_pick = 2; }
                                        ui.add_space(4.0);
                                        if dest_button(ui, "Join Server") { menu_pick = 3; }
                                        ui.add_space(8.0);
                                        // r394: LAN hosts discovered via the gossip beacon
                                        // (display-only this slice; selecting → join is next).
                                        ui.separator();
                                        ui.label("LAN games");
                                        if menu_mp_list.is_empty() {
                                            ui.weak("(searching… none found yet)");
                                        } else {
                                            for (i, label) in menu_mp_list.iter().enumerate() {
                                                // click a host → bin sends a JOIN to its candidate
                                                if dest_button(ui, label) { menu_mp_pick_local = i as i32; }
                                            }
                                        }
                                        ui.add_space(8.0);
                                        if dest_button(ui, "Back") { menu_pick = 6; }
                                    }
                                    3 => {
                                        // r197: LOAD WORLD — list saved worlds
                                        // (was undrawn: "can't view previously
                                        // created worlds"). Each row is a button;
                                        // click sets menu_world_pick for the bin.
                                        ui.label("load world");
                                        ui.add_space(8.0);
                                        if menu_world_list.is_empty() {
                                            ui.weak("(no saved worlds yet)");
                                        } else {
                                            egui::ScrollArea::vertical()
                                                .id_salt("world_list_scroll")
                                                .max_height(180.0)
                                                .show(ui, |ui| {
                                                    for (i, label) in menu_world_list.iter().enumerate() {
                                                        ui.horizontal(|ui| {
                                                            if dest_button(ui, label) {
                                                                menu_world_pick_local = i as i32;
                                                            }
                                                            // r203: peek this world's rollback
                                                            // history without launching it.
                                                            if ui.add_sized([70.0, 20.0],
                                                                egui::Button::new("history")).clicked()
                                                            {
                                                                menu_rollback_peek_local = i as i32;
                                                            }
                                                            // r205: delete (with confirm). Lets
                                                            // the dev prune accumulated test
                                                            // worlds without leaving the menu.
                                                            let del = egui::Button::new("\u{1f5d1}")
                                                                .fill(egui::Color32::from_rgb(60, 24, 24));
                                                            if ui.add_sized([28.0, 20.0], del)
                                                                .on_hover_text("delete world")
                                                                .clicked()
                                                            {
                                                                menu_world_delete_ask_local = i as i32;
                                                            }
                                                        });
                                                    }
                                                });
                                            // r205: delete confirmation row — armed by the
                                            // trash button, shows the world name and a
                                            // confirm/cancel pair so a misclick can't wipe
                                            // a world.
                                            if menu_world_delete_ask >= 0
                                                && (menu_world_delete_ask as usize) < menu_world_list.len()
                                            {
                                                ui.add_space(6.0);
                                                ui.separator();
                                                let idx = menu_world_delete_ask as usize;
                                                ui.colored_label(
                                                    egui::Color32::from_rgb(230, 140, 140),
                                                    format!("delete \u{201c}{}\u{201d}? this cannot be undone.",
                                                        menu_world_list[idx]),
                                                );
                                                ui.horizontal(|ui| {
                                                    let yes = egui::Button::new("  delete  ")
                                                        .fill(egui::Color32::from_rgb(120, 30, 30));
                                                    if ui.add(yes).clicked() {
                                                        menu_world_delete_confirm_local = idx as i32;
                                                    }
                                                    if ui.button("  cancel  ").clicked() {
                                                        menu_world_delete_cancel_local = true;
                                                    }
                                                });
                                            }
                                        }
                                        // r203: ROLLBACK BROWSE. When a world's
                                        // history is requested, the bin fills
                                        // menu_rollback_list with its restore
                                        // points (newest first). "Step backward
                                        // through the rollback system" begins as
                                        // visibility: see the points, their reason
                                        // and size, then restore.
                                        if menu_show_rollbacks {
                                            ui.add_space(6.0);
                                            ui.separator();
                                            ui.horizontal(|ui| {
                                                // r227: blocky header in the menu language
                                                ui.label(
                                                    egui::RichText::new("WORLD HISTORY")
                                                        .font(unendlich_ui::menu::blocks_font(15.0))
                                                        .color(egui::Color32::from_rgb(0xee, 0xf3, 0xe8))
                                                        .extra_letter_spacing(3.0),
                                                );
                                                ui.weak("(newest first)");
                                                // r206: a small "more info" hint —
                                                // explains what history points are
                                                // and how they're made.
                                                ui.label("\u{2139}").on_hover_text(
                                                    "World-history points snapshot your world so you can step back.\n\
                                                     They're created on a timer, after long travel, and on crafting.\n\
                                                     In dev mode (UNENDLICH_DEV=1) they're created far more often,\n\
                                                     and you can force one with the button below. Restore loads a\n\
                                                     point on next launch; delete prunes a bad one.",
                                                );
                                            });
                                            ui.add_space(4.0);
                                            if menu_rollback_list.is_empty() {
                                                ui.weak("(no rollback points for this world yet)");
                                            } else {
                                                let accent = unendlich_ui::current_theme(ui.ctx()).accent;
                                                egui::ScrollArea::vertical()
                                                    .id_salt("rollback_scroll")
                                                    .max_height(140.0)
                                                    .show(ui, |ui| {
                                                        for (i, label) in menu_rollback_list.iter().enumerate() {
                                                            ui.horizontal(|ui| {
                                                                // r227: diamond marker (the menu's
                                                                // history-point motif) before each row.
                                                                let (drect, _) = ui.allocate_exact_size(
                                                                    egui::vec2(16.0, 20.0), egui::Sense::hover());
                                                                let c = drect.center();
                                                                let h = 5.0;
                                                                ui.painter().add(egui::Shape::convex_polygon(
                                                                    vec![
                                                                        egui::pos2(c.x, c.y - h),
                                                                        egui::pos2(c.x + h, c.y),
                                                                        egui::pos2(c.x, c.y + h),
                                                                        egui::pos2(c.x - h, c.y),
                                                                    ],
                                                                    accent,
                                                                    egui::Stroke::NONE,
                                                                ));
                                                                ui.add_sized([186.0, 20.0],
                                                                    egui::Label::new(label.as_str()));
                                                                if ui.add_sized([72.0, 20.0],
                                                                    egui::Button::new("restore")).clicked()
                                                                {
                                                                    menu_rollback_restore_local = i as i32;
                                                                }
                                                                // r206: delete a bad point
                                                                // (operator request).
                                                                let del = egui::Button::new("\u{1f5d1}")
                                                                    .fill(egui::Color32::from_rgb(60, 24, 24));
                                                                if ui.add_sized([26.0, 20.0], del)
                                                                    .on_hover_text("delete this point")
                                                                    .clicked()
                                                                {
                                                                    menu_rollback_delete_local = i as i32;
                                                                }
                                                            });
                                                        }
                                                    });
                                            }
                                            // r206: dev-only "create history point now".
                                            if dev_mode_on {
                                                ui.add_space(4.0);
                                                if ui.button("\u{2795} create history point now").clicked() {
                                                    dev_make_point_local = true;
                                                }
                                            }
                                        }
                                        ui.add_space(8.0);
                                        if dest_button(ui, "Back") { menu_pick = 6; }
                                    }

                                    4 => {
                                        // r656: Singleplayer submenu — groups
                                        // Create / Load / Bending Tutorial.
                                        ui.label("singleplayer");
                                        ui.add_space(8.0);
                                        if dest_button(ui, "Create World") { menu_pick = 1; }
                                        ui.add_space(4.0);
                                        if dest_button(ui, "Load World") { menu_pick = 7; }
                                        ui.add_space(4.0);
                                        if dest_button(ui, "Bending Tutorial") { menu_pick = 9; }
                                        ui.add_space(8.0);
                                        if dest_button(ui, "Back") { menu_pick = 6; }
                                    }

                                    _ => {
                                        // r656: top level now groups play modes —
                                        // Singleplayer (Create/Load/Tutorial) and
                                        // Multiplayer (Host/Join) each open a submenu.
                                        if dest_button(ui, "Singleplayer") { menu_pick = 8; }
                                        ui.add_space(4.0);
                                        if dest_button(ui, "Multiplayer") { menu_pick = 2; }
                                        ui.add_space(4.0);
                                        // r203: Settings reachable from the MAIN MENU
                                        // (was pause-menu only). Opens the tabbed
                                        // settings window (Video/Audio/Gameplay/
                                        // Controls/Performance) — which already
                                        // renders over any screen and has its own
                                        // "Done" close button.
                                        if dest_button(ui, "Settings") { settings_menu_open_local = true; }
                                        ui.add_space(4.0);
                                        // r488: open the character-select / customizer.
                                        if dest_button(ui, "Character") { open_character_local = true; }
                                        ui.add_space(12.0);
                                        if dest_button(ui, "Quit") { menu_pick = 4; }
                                    }

                                }
                                fresnel(ui, ui.max_rect().expand(10.0), gt); // r190
                                // r232: the mockup's "frame draws itself in" —
                                // an outer→inner border ring that fills on each
                                // navigation. animate_bool_with_time, keyed by
                                // the screen kind, ramps 0→1 once when a screen
                                // appears; border_fill wants 0=thick..1=gone, so
                                // we pass 1.0 - that ramp and replay per screen.
                                let bf_id = egui::Id::new(("menu_border_fill", menu_screen_kind));
                                let ramp = ui.ctx().animate_bool_with_time(bf_id, true, 0.62);
                                let accent = unendlich_ui::current_theme(ui.ctx()).accent;
                                if unendlich_ui::menu::border_fill(
                                    ui, ui.max_rect().expand(10.0), accent, 1.0 - ramp,
                                ) {
                                    ui.ctx().request_repaint();
                                }
                            });
                        });
                    } // r190: end gt scope
                }
                // ── r178: LOADING VEIL between menu and world ──
                if loading_fade > 0.01 {
                    let a = (loading_fade.clamp(0.0, 1.0) * 255.0) as u8;
                    egui::Area::new(egui::Id::new("unendlich_loading_veil"))
                        .fixed_pos(egui::pos2(0.0, 0.0))
                        .order(egui::Order::Foreground)
                        .show(c, |ui| {
                            let r = ui.ctx().screen_rect();
                            ui.painter().rect_filled(
                                r,
                                0.0,
                                egui::Color32::from_black_alpha(a),
                            );
                            // concentric bending rings, slowly breathing —
                            // placeholder art until the shader/neural pass.
                            let t01 = 1.0 - loading_fade;
                            let center = r.center();
                            for i in 0..4 {
                                let rad = 40.0 + i as f32 * 26.0 + t01 * 30.0;
                                ui.painter().circle_stroke(
                                    center,
                                    rad,
                                    egui::Stroke::new(
                                        1.5,
                                        egui::Color32::from_rgba_unmultiplied(150, 200, 220, a / (i + 2) as u8),
                                    ),
                                );
                            }
                            ui.painter().text(
                                egui::pos2(center.x, center.y + 160.0),
                                egui::Align2::CENTER_CENTER,
                                &loading_label,
                                egui::FontId::proportional(18.0),
                                egui::Color32::from_white_alpha(a),
                            );
                        });
                }
                // ── r167: PAUSE menu (Esc, in-world) ──
                if pause_menu_open && !menu_open {
                    egui::Window::new("unendlich_pause")
                        .frame(glass.clone())
                        .anchor(egui::Align2::CENTER_CENTER, cone_par(c, "unendlich_pause", 0.014))
                        .collapsible(false)
                        .resizable(false)
                        .title_bar(false)
                        .show(c, |ui| {
                            unendlich_ui::set_theme(ui.ctx(), unendlich_ui::Theme::DARK);
                            ui.set_min_width(300.0);
                            ui.vertical_centered(|ui| {
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new("unendlich")
                                        .size(30.0)
                                        .strong(),
                                );
                                ui.label(egui::RichText::new("\u{2014} paused \u{2014}").size(13.0).weak());
                                ui.weak("menu build r171"); // r170: stale-binary detector
                                ui.add_space(14.0);
                                if tbtn_sized(ui, "Resume", 220.0, 32.0) { pause_pick = 1; }
                                ui.add_space(6.0);
                                if tbtn_sized(ui, "Settings", 220.0, 32.0) { pause_pick = 2; }
                                ui.add_space(6.0);
                                // r533: change character WITHOUT leaving the world — the
                                // selector was previously only on the title screen.
                                if tbtn_sized(ui, "Character", 220.0, 32.0) { open_character_local = true; }
                                ui.add_space(6.0);
                                ui.separator();
                                ui.add_space(6.0);
                                if tbtn_sized(ui, "Save & Quit to Title", 220.0, 32.0) { pause_pick = 3; }
                            });
                        });
                }
                // r169: shared sparkline subcomponent (settings·video +
                // analytics both draw it — the operator's shared-component
                // requirement, in egui form).
                let sparkline = |ui: &mut egui::Ui, samples: &[f32], h: f32| {
                    let w = 240.0f32;
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(w, h),
                        egui::Sense::hover(),
                    );
                    if samples.len() >= 2 {
                        // r175: FIXED scale (50ms ceiling) — the strip
                        // scrolls; spikes clip rather than rescale the view.
                        let maxv = 50.0f32;
                        let pts: Vec<egui::Pos2> = samples
                            .iter()
                            .enumerate()
                            .map(|(i, v)| {
                                egui::pos2(
                                    rect.left() + rect.width() * (i as f32) / (samples.len() - 1) as f32,
                                    rect.bottom() - rect.height() * (v / maxv).min(1.0),
                                )
                            })
                            .collect();
                        ui.painter().add(egui::Shape::line(
                            pts,
                            egui::Stroke::new(1.5, egui::Color32::from_rgb(120, 200, 160)),
                        ));
                    }
                    // r312: numeric readout on the graph — current + max in the window.
                    if let Some(&cur) = samples.last() {
                        let maxv_seen = samples.iter().cloned().fold(0.0f32, f32::max);
                        ui.painter().text(
                            rect.left_top() + egui::vec2(3.0, 1.0),
                            egui::Align2::LEFT_TOP,
                            format!("{:.1} ms", cur),
                            egui::FontId::proportional(11.0),
                            egui::Color32::from_rgb(190, 235, 205),
                        );
                        ui.painter().text(
                            rect.right_top() + egui::vec2(-3.0, 1.0),
                            egui::Align2::RIGHT_TOP,
                            format!("max {:.1}", maxv_seen),
                            egui::FontId::proportional(10.0),
                            egui::Color32::from_rgb(150, 185, 165),
                        );
                    }
                };
                // ── r169: SETTINGS (categorized, sliders) ──
                if settings_menu_open {
                    egui::Window::new("settings")
                        .frame(glass.clone())
                        .anchor(egui::Align2::CENTER_CENTER, cone_par(c, "settings", 0.012))
                        .collapsible(false)
                        .resizable(false)
                        .max_height(440.0)
                        .show(c, |ui| {
                            unendlich_ui::set_theme(ui.ctx(), unendlich_ui::Theme::DARK);
                            ui.set_min_width(360.0);
                            ui.add_space(2.0);
                            // r225: blocky title to match the menu language.
                            ui.label(
                                egui::RichText::new("SETTINGS")
                                    .font(unendlich_ui::menu::blocks_font(22.0))
                                    .color(egui::Color32::from_rgb(0xee, 0xf3, 0xe8))
                                    .extra_letter_spacing(4.0),
                            );
                            ui.weak("settings \u{00b7} r171");
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                for (i, name) in ["Video", "Audio", "Gameplay", "Controls", "Performance", "Debug"].iter().enumerate() {
                                    if unendlich_ui::tab_button(ui, *name, settings_tab == i as u8).clicked() {
                                        settings_tab = i as u8;
                                    }
                                }
                                // r235: per-WORLD (server) settings page, reached
                                // by a two-column icon button — a settings glyph
                                // (gear/diamond) + a world glyph (globe). Pushed
                                // to the right. Opens tab 5.
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let icons = egui::RichText::new("\u{2699}\u{1f310}") // ⚙🌐
                                        .size(15.0);
                                    if ui.selectable_label(settings_tab == 6, icons)
                                        .on_hover_text("Per-world server settings")
                                        .clicked()
                                    {
                                        settings_tab = 6;
                                    }
                                });
                            });
                            ui.separator();
                            match settings_tab {
                                0 => {
                                    // r223: the mockup's diamond toggle (a
                                    // <...> holder shrink-wrapping a diamond
                                    // knob, fractal-dither infill). Labelled
                                    // rows; click flips the bool.
                                    let accent = unendlich_ui::current_theme(ui.ctx()).accent;
                                    ui.horizontal(|ui| {
                                        if unendlich_ui::menu::toggle(ui, ui_fog_on, accent).clicked() {
                                            ui_fog_on = !ui_fog_on;
                                        }
                                        ui.label("Fog");
                                    });
                                    ui.horizontal(|ui| {
                                        if unendlich_ui::menu::toggle(ui, ui_shadows_on, accent).clicked() {
                                            ui_shadows_on = !ui_shadows_on;
                                        }
                                        ui.label("Shadows");
                                    });
                                    ui.add_space(6.0);
                                    // r200: GUI scale (egui pixels_per_point).
                                    // 1.0 small/crisp .. 3.0 large; applied
                                    // next frame via self.ui_scale.
                                    // r202: GUI scale must NOT update live —
                                    // resizing the whole UI mid-drag is
                                    // disorienting (operator: "GUI scale
                                    // should wait for release to update"). The
                                    // slider edits a STAGED value; it only
                                    // commits to the live scale when the drag
                                    // stops (or on release of a click-set).
                                    ui.label("GUI scale (applies on release)");
                                    let scale_accent = unendlich_ui::current_theme(ui.ctx()).accent;
                                    let resp = ui.horizontal(|ui| {
                                        let r = unendlich_ui::menu::slider(ui, &mut ui_scale_staged, 1.0, 3.0, 9, scale_accent);
                                        ui.add_space(6.0);
                                        ui.label(egui::RichText::new(format!("{:.2}", ui_scale_staged)).strong().color(scale_accent));
                                        r
                                    }).inner;
                                    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                                        ui_scale_local = ui_scale_staged;
                                        ui_scale_commit = true;
                                    }
                                    if (ui_scale_staged - ui_scale_local).abs() > f32::EPSILON {
                                        ui.weak(format!("pending: {:.2}× (release to apply)", ui_scale_staged));
                                    }

                                    ui.add_space(6.0);
                                    ui.label("frame time (live)");
                                    sparkline(ui, &perf_samples, 36.0);
                                }
                                1 => {
                                    // r223: the mockup's custom slider — dither
                                    // fill wrapping a square knob, grayscale-
                                    // faded tail, push/pull stretch, detents.
                                    let accent = unendlich_ui::current_theme(ui.ctx()).accent;
                                    ui.horizontal(|ui| {
                                        ui.label("music volume");
                                        unendlich_ui::menu::slider(ui, &mut ui_music_vol, 0.0, 1.0, 11, accent);
                                    });
                                    ui.label("(audible after the audio backend lands; the director honours it now)");
                                }
                                2 => {
                                    // r225: day length on the custom slider.
                                    let accent = unendlich_ui::current_theme(ui.ctx()).accent;
                                    ui.horizontal(|ui| {
                                        ui.label("day length (s)");
                                        unendlich_ui::menu::slider(ui, &mut ui_day_length, 240.0, 4800.0, 0, accent);
                                    });
                                    // r235: SERVER game-speed (time dilation),
                                    // logarithmic 0x..1x..10x, default 1x at the
                                    // midpoint. Server-side only; across peers
                                    // the world runs at the value CLOSEST TO 1x
                                    // (sim::game_speed::reconcile), so one player
                                    // at normal speed pulls the session back to
                                    // normal. The label shows the live multiplier.
                                    let mult = crate::sim::game_speed::pos_to_multiplier(ui_game_speed_pos);
                                    ui.horizontal(|ui| {
                                        ui.label(format!("game speed  ({mult:.2}x)"));
                                        unendlich_ui::menu::slider(ui, &mut ui_game_speed_pos, 0.0, 1.0, 0, accent);
                                    });
                                    ui.weak(
                                        "server-authoritative · logarithmic 0x–1x–10x · in multiplayer \
                                         the world runs at whichever setting is closest to 1x",
                                    );
                                    ui.label("F6 toggles survival/creative · F7 hold = rest ritual");
                                }
                                3 => {
                                    // r199: HOTKEY REBINDING PAGE. Each row
                                    // shows an action + its current key; the
                                    // button captures the next keypress (the
                                    // bin watches keybind_capturing). The
                                    // list + capture state are fed by the bin.
                                    ui.label("controls — click a binding, then press a key");
                                    ui.add_space(6.0);
                                    egui::ScrollArea::vertical()
                                        .id_salt("keybind_scroll")
                                        .auto_shrink([false, false])
                                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                                        .max_height(260.0)
                                        .show(ui, |ui| {
                                            for (i, (action, key)) in menu_keybind_list.iter().enumerate() {
                                                ui.horizontal(|ui| {
                                                    ui.add_sized(
                                                        [150.0, 22.0],
                                                        egui::Label::new(action.as_str()),
                                                    );
                                                    let capturing = keybind_capturing == i as i32;
                                                    let label = if capturing {
                                                        "< press a key >".to_string()
                                                    } else {
                                                        key.clone()
                                                    };
                                                    // r317: the rebind button uses the elegant (cursor-infill) style.
                                                    if unendlich_ui::elegant_button_sized(ui, &label, egui::vec2(130.0, 22.0)).clicked()
                                                    {
                                                        keybind_rebind_local = i as i32;
                                                    }
                                                });
                                                ui.add_space(2.0); // r204: row breathing room
                                            }
                                        });
                                    ui.add_space(6.0);
                                    ui.weak("Esc cancels a pending rebind");
                                }
                                4 => {

                                    // r225: live performance levers on the
                                    // custom slider. These bind integer state
                                    // (u64/u32/i32), so each round-trips through
                                    // a temp f32 and casts back on change; the
                                    // detent count == the integer span so the
                                    // knob snaps to whole steps.
                                    let accent = unendlich_ui::current_theme(ui.ctx()).accent;
                                    // r325: graphics/perf PROFILES. Presets are starting points;
                                    // tuning any knob turns the active preset into a custom PROFILE
                                    // (builder glyph + reset). Glyphs are game-themed (seedling ->
                                    // dragon = rising cost); the label still reads if the font lacks them.
                                    let preset_glyph = ["🔋", "🌱", "🌲", "⛰", "🐉"];
                                    let warn_col = egui::Color32::from_rgb(0xff, 0xd2, 0x7f);
                                    ui.label(egui::RichText::new("PROFILE").color(accent).small());
                                    ui.horizontal(|ui| {
                                        for (i, p) in crate::runtime_config::PerfPreset::SELECTABLE.iter().enumerate() {
                                            let lbl = format!("{} {}", preset_glyph[i], p.label());
                                            if unendlich_ui::tab_button(ui, &lbl, ui_perf_preset == i as u8).clicked() {
                                                let cfg = crate::runtime_config::RuntimeConfig::preset(*p);
                                                ui_perf_preset = i as u8;
                                                ui_render_distance = cfg.render_distance as i32;
                                                ui_render_scale = cfg.render_scale;
                                            }
                                        }
                                    });
                                    {
                                        let base = crate::runtime_config::RuntimeConfig::preset(
                                            crate::runtime_config::PerfPreset::SELECTABLE[(ui_perf_preset.min(4)) as usize]
                                        );
                                        let modified = ui_render_distance != base.render_distance as i32
                                            || (ui_render_scale - base.render_scale).abs() > 0.001;
                                        ui.horizontal(|ui| {
                                            if modified {
                                                ui.label(egui::RichText::new("🔨 custom profile").color(warn_col).small());
                                                if unendlich_ui::tab_button(ui, "reset", false).clicked() {
                                                    ui_render_distance = base.render_distance as i32;
                                                    ui_render_scale = base.render_scale;
                                                }
                                            } else {
                                                ui.label(egui::RichText::new("● preset").color(accent).small());
                                            }
                                        });
                                    }
                                    ui.separator();
                                    // small helper: integer slider via menu::slider
                                    macro_rules! int_slider {
                                        ($label:expr, $var:expr, $ty:ty, $lo:expr, $hi:expr) => {{
                                            let mut f = $var as f32;
                                            ui.horizontal(|ui| {
                                                ui.label($label);
                                                unendlich_ui::menu::slider(
                                                    ui, &mut f, $lo as f32, $hi as f32,
                                                    (($hi - $lo) + 1) as usize, accent,
                                                );
                                                // r312: numeric value next to the slider so the
                                                // setting is legible (render distance, strides, etc.).
                                                ui.add_space(6.0);
                                                ui.label(
                                                    egui::RichText::new(
                                                        format!("{}", f.round() as $ty)
                                                    )
                                                    .strong()
                                                    .color(accent),
                                                );
                                            });
                                            $var = f.round() as $ty;
                                        }};
                                    }
                                    ui.label("tick cadence (higher = cheaper, coarser)");
                                    int_slider!("plants", ui_plant_stride, u64, 1, 16);
                                    int_slider!("leaves", ui_leaf_stride, u64, 1, 8);
                                    int_slider!("apply budget", ui_apply_budget, u32, 1, 32);
                                    ui.separator();
                                    // r197: live render distance (chunks).
                                    ui.label("render distance (chunks; higher = farther, costlier)");
                                    int_slider!("distance", ui_render_distance, i32, 2, 32);
                                    ui.add_space(6.0);
                                    // r325: render resolution scale (neural upscale).
                                    ui.label("render scale (neural upscale; <100% renders smaller)");
                                    ui.horizontal(|ui| {
                                        unendlich_ui::menu::slider(ui, &mut ui_render_scale, 0.5, 1.0, 10, accent);
                                        ui.add_space(6.0);
                                        ui.label(egui::RichText::new(format!("{:.0}%", ui_render_scale * 100.0)).strong().color(accent));
                                    });
                                    ui.separator();

                                    if unendlich_ui::menu::toggle(ui, ui_autotune, accent).clicked() {
                                        ui_autotune = !ui_autotune;
                                    }
                                    ui.label("autotune to FPS target");
                                    if ui_autotune {
                                        int_slider!("target fps", ui_fps_target, u32, 15, 500);
                                    }
                                    ui.weak("autotune raises strides / lowers budget below target, relaxes above");

                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.label(egui::RichText::new("GPU MESHER").weak().extra_letter_spacing(2.0));
                                    ui.add_space(4.0);
                                    // r476: live mesher throttle. Sleeps a % of each octree dispatch
                                    // so the shared GPU queue is left idle for rendering — trades
                                    // chunk pop-in speed for FPS while streaming.
                                    ui.label("mesh throttle (% — higher = smoother streaming, slower chunk pop-in)");
                                    int_slider!("throttle", ui_gpu_mesh_yield, u64, 0, 200);
                                    ui.add_space(6.0);
                                    if unendlich_ui::menu::toggle(ui, ui_adaptive_rd, accent).clicked() {
                                        ui_adaptive_rd = !ui_adaptive_rd;
                                    }
                                    ui.label("adaptive render distance");
                                    ui.weak("auto-shrinks the radius when frames run long, grows back when they recover");
                                    ui.add_space(6.0);
                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.label(egui::RichText::new("ADVANCED").weak().extra_letter_spacing(2.0));
                                    ui.add_space(4.0);
                                    if unendlich_ui::menu::toggle(ui, ui_octree_tile, accent).clicked() {
                                        ui_octree_tile = !ui_octree_tile;
                                    }
                                    ui.label("GPU tile mesher (on by default)");
                                    ui.weak("shared-memory tiled octree mesh — fewer GPU descents, faster remeshing; only active with the octree GPU mesher");
                                    ui.add_space(6.0);
                                    if unendlich_ui::menu::toggle(ui, ui_detached_terrain, accent).clicked() {
                                        ui_detached_terrain = !ui_detached_terrain;
                                    }
                                    ui.label("detached terrain");
                                    ui.weak("glide over not-yet-streamed ground instead of falling, settle when it spawns");
                                }
                                5 => {
                                    // r437: DEBUG tab. Master gate (off by default) + per-system
                                    // log toggles, grouped in sections. The bin reads these each
                                    // frame to decide what to print.
                                    let accent = unendlich_ui::current_theme(ui.ctx()).accent;
                                    ui.horizontal(|ui| {
                                        if unendlich_ui::menu::toggle(ui, ui_debug_master, accent).clicked() {
                                            ui_debug_master = !ui_debug_master;
                                        }
                                        ui.label(egui::RichText::new("Enable debug logging").strong());
                                    });
                                    ui.weak("master switch — off by default; turns the lines below on/off");
                                    ui.add_space(6.0);
                                    ui.separator();
                                    ui.add_enabled_ui(ui_debug_master, |ui| {
                                        ui.label(egui::RichText::new("WATER").weak().extra_letter_spacing(2.0));
                                        ui.horizontal(|ui| {
                                            if unendlich_ui::menu::toggle(ui, ui_debug_water, accent).clicked() {
                                                ui_debug_water = !ui_debug_water;
                                            }
                                            ui.label("[water] — wind, waves, depth + sim activity");
                                        });
                                        ui.horizontal(|ui| {
                                            if unendlich_ui::menu::toggle(ui, ui_debug_flow, accent).clicked() {
                                                ui_debug_flow = !ui_debug_flow;
                                            }
                                            ui.label("[water-flow] — current field, tiles, rain/evap");
                                        });
                                        ui.add_space(6.0);
                                        ui.label(egui::RichText::new("ACOUSTICS").weak().extra_letter_spacing(2.0));
                                        ui.horizontal(|ui| {
                                            if unendlich_ui::menu::toggle(ui, ui_debug_echo, accent).clicked() {
                                                ui_debug_echo = !ui_debug_echo;
                                            }
                                            ui.label("[echo] — reverb/occlusion computed at the player");
                                        });
                                    });
                                }
                                6 => {
                                    // r235: PER-WORLD (server) settings page.
                                    // Server-authoritative knobs that belong to
                                    // the world, not the client. Reconciled
                                    // across peers (closest-to-1x for speed).
                                    let accent = unendlich_ui::current_theme(ui.ctx()).accent;
                                    ui.label(
                                        egui::RichText::new("WORLD")
                                            .font(unendlich_ui::menu::blocks_font(18.0))
                                            .color(egui::Color32::from_rgb(0xee, 0xf3, 0xe8))
                                            .extra_letter_spacing(3.0),
                                    );
                                    ui.weak("per-world · server-authoritative");
                                    ui.add_space(8.0);
                                    // game speed (the same server setting also on
                                    // the Gameplay tab — surfaced here as its
                                    // natural home).
                                    let mult = crate::sim::game_speed::pos_to_multiplier(ui_game_speed_pos);
                                    ui.horizontal(|ui| {
                                        ui.label(format!("game speed  ({mult:.2}x)"));
                                        unendlich_ui::menu::slider(ui, &mut ui_game_speed_pos, 0.0, 1.0, 0, accent);
                                    });
                                    ui.weak(
                                        "0x = frozen · 1x = normal · 10x = fast · logarithmic. \
                                         In multiplayer the world runs at whichever player's setting \
                                         is closest to 1x.",
                                    );
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        ui.label("day length (s)");
                                        unendlich_ui::menu::slider(ui, &mut ui_day_length, 240.0, 4800.0, 0, accent);
                                    });
                                    ui.weak("more per-world knobs will gather here (rules, spawn, persistence).");
                                }
                                _ => {
                                    for line in [
                                        "B bend · C craft · I inventory · P picker",
                                        "F flight · H egg · F3 debug · F4 analytics",
                                        "F6 mode · F7 rest · F9 fire · Esc pause",
                                    ] {
                                        ui.label(line);
                                    }
                                }
                            }
                            ui.add_space(8.0);
                            if tbtn(ui, "  Done  ") { settings_close = true; }
                        });
                }
                // ── r169: CRAFTING (Minecraft-nostalgia rows; backend stress) ──
                if craft_menu_open {
                    egui::Window::new("crafting")
                        .frame(glass.clone())
                        .anchor(egui::Align2::CENTER_CENTER, cone_par(c, "crafting", 0.012))
                        .collapsible(false)
                        .resizable(false)
                        .show(c, |ui| {
                            unendlich_ui::set_theme(ui.ctx(), unendlich_ui::Theme::DARK);
                            ui.label(
                                egui::RichText::new("CRAFTING")
                                    .font(unendlich_ui::menu::blocks_font(22.0))
                                    .color(egui::Color32::from_rgb(0xee, 0xf3, 0xe8))
                                    .extra_letter_spacing(4.0),
                            );
                            ui.add_space(6.0);
                            for (i, (name, can, have, tip)) in craft_lines.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.add_enabled_ui(*can, |ui| {
                                        let b = ui.button(format!("  craft  {name}  "));
                                        // r176: canonical tooltip (doc
                                        // format: effects, italic lore,
                                        // attribution) on hover.
                                        let b = b.on_hover_text(tip.clone());
                                        if b.clicked() {
                                            craft_pick = (i + 1) as u8;
                                        }
                                    });
                                    ui.weak(have);
                                });
                            }
                            ui.add_space(8.0);
                            if tbtn(ui, "  Close (C)  ") { craft_close = true; }
                        });
                }
                // ── r488: CHARACTER-SELECT / customizer ──
                if character_menu_open {
                    egui::Window::new("character_select")
                        .frame(glass.clone())
                        .anchor(egui::Align2::CENTER_CENTER, cone_par(c, "character_select", 0.012))
                        .collapsible(false)
                        .resizable(false)
                        .show(c, |ui| {
                            unendlich_ui::set_theme(ui.ctx(), unendlich_ui::Theme::DARK);
                            ui.label(
                                egui::RichText::new("CHOOSE YOUR CHARACTER")
                                    .font(unendlich_ui::menu::blocks_font(22.0))
                                    .color(egui::Color32::from_rgb(0xee, 0xf3, 0xe8))
                                    .extra_letter_spacing(4.0),
                            );
                            ui.add_space(8.0);
                            ui.horizontal_wrapped(|ui| {
                                for (i, (name, plan, is_custom)) in char_cards.iter().enumerate() {
                                    let marker = if i == char_selected { "\u{25b8} " } else { "" };
                                    let star = if *is_custom { " \u{2605}" } else { "" };
                                    if ui.button(format!("{marker}{name}{star}\n{plan}")).clicked() {
                                        char_pick_local = i as i32;
                                    }
                                }
                            });
                            ui.add_space(10.0);
                            // live silhouette preview of the highlighted character
                            let (rect, _resp) = ui.allocate_exact_size(
                                egui::vec2(260.0, 200.0),
                                egui::Sense::hover(),
                            );
                            let painter = ui.painter_at(rect);
                            painter.rect_filled(
                                rect,
                                6.0,
                                egui::Color32::from_rgba_unmultiplied(10, 12, 16, 150),
                            );
                            let pad = 18.0;
                            let area = egui::vec2(
                                (rect.width() - pad * 2.0).max(1.0),
                                (rect.height() - pad * 2.0).max(1.0),
                            );
                            for pb in &char_preview {
                                // projection space: unit square, +y up → flip for egui.
                                let x = rect.min.x + pad + pb.x * area.x;
                                let y = rect.min.y + pad + (1.0 - pb.y - pb.h) * area.y;
                                let r = egui::Rect::from_min_size(
                                    egui::pos2(x, y),
                                    egui::vec2((pb.w * area.x).max(1.0), (pb.h * area.y).max(1.0)),
                                );
                                let col = egui::Color32::from_rgb(
                                    (pb.color[0].clamp(0.0, 1.0) * 255.0) as u8,
                                    (pb.color[1].clamp(0.0, 1.0) * 255.0) as u8,
                                    (pb.color[2].clamp(0.0, 1.0) * 255.0) as u8,
                                );
                                painter.rect_filled(r, 1.0, col);
                            }
                            ui.add_space(8.0);
                            ui.weak(char_stat.clone());
                            // r668: DERIVED SECONDARY STATS (sim::secondary_stats) for
                            // the highlighted character at a base loadout. Mobility
                            // differs by bending discipline, so each hero reads
                            // distinctly (Air fast, Earth grounded); Resilience needs
                            // armour (0 at base); Recovery scales with level.
                            {
                                use crate::sim::secondary_stats as ss;
                                let sel_name = char_cards
                                    .get(char_selected)
                                    .map(|c| c.0.clone())
                                    .unwrap_or_default();
                                let disc = match crate::player_character::bend_element(&sel_name) {
                                    Some(crate::sim::gesture_resolve::Element::Air) => ss::Discipline::Air,
                                    Some(crate::sim::gesture_resolve::Element::Water) => ss::Discipline::Water,
                                    Some(crate::sim::gesture_resolve::Element::Fire) => ss::Discipline::Fire,
                                    _ => ss::Discipline::Earth,
                                };
                                let level = 20;
                                let mob = ss::mobility(ss::GearWeight::Bare, disc, 0.0, level);
                                let res = ss::resilience(ss::ArmorMaterial::Leather, 0, 0.0, 0.0, 0.0);
                                let rec = ss::recovery(10.0, level, 0, 0.0, 0.0);
                                ui.add_space(6.0);
                                ui.weak(
                                    egui::RichText::new("DERIVED STATS  ·  base loadout")
                                        .size(10.0)
                                        .extra_letter_spacing(2.0),
                                );
                                ui.weak(format!("Mobility   {:>3.0}   ·  ×{:.2} speed", mob, ss::move_speed_multiplier(mob)));
                                ui.weak(format!("Resilience {:>3.0}   ·  {:.0}% dmg reduction", res, ss::damage_reduction(res) * 100.0));
                                ui.weak(format!("Recovery   {:>3.0}   ·  {:.1} chi/s", rec, ss::chi_regen_per_sec(rec)));
                                // r669: BENDING KIT — the techniques this character's
                                // element can cast (sim::technique_catalogue), so the
                                // screen shows what each hero *does*, not just stats.
                                if let Some(elem) = crate::player_character::bend_element(&sel_name) {
                                    let techs = crate::sim::technique_catalogue::techniques_for(elem);
                                    if !techs.is_empty() {
                                        ui.add_space(6.0);
                                        ui.weak(
                                            egui::RichText::new("BENDING KIT")
                                                .size(10.0)
                                                .extra_letter_spacing(2.0),
                                        );
                                        for t in techs.iter().take(6) {
                                            let sig = if t.expensive { " \u{2605}" } else { "" };
                                            ui.weak(format!(
                                                "{}{}   ·  T{}  ·  {:.1}r",
                                                t.name, sig, t.min_tier, t.resonance_cost
                                            ));
                                        }
                                    }
                                    // r672: DISCIPLINE DEPTH + sub-discipline title
                                    // (sim::sub_discipline). Heroes read as attuned
                                    // Warlocks in their element; the canonical
                                    // sub-discipline name shows (a registered title
                                    // like "Oceanbinder" would need Known renown).
                                    {
                                        use crate::sim::sub_discipline as sd;
                                        use crate::sim::gesture_resolve::Element as E;
                                        let depth = 0.75_f32;
                                        let canonical = match elem {
                                            E::Air => "Windward Practitioner",
                                            E::Water => "Tidal Practitioner",
                                            E::Fire => "Forge Practitioner",
                                            E::Earth => "Root Practitioner",
                                        };
                                        ui.add_space(6.0);
                                        ui.weak(format!("Depth  ·  {}", sd::depth_tier(depth).label()));
                                        ui.weak(format!("Sub-discipline  ·  {}", canonical));
                                    }
                                }
                            }
                            // customizer controls — only on the player's own (custom) slot
                            if char_selected == char_custom_index {
                                ui.add_space(8.0);
                                let plan = char_cards
                                    .get(char_custom_index)
                                    .map(|c| c.1.clone())
                                    .unwrap_or_default();
                                if tbtn(ui, &format!("  Body: {plan}  \u{25b8}")) {
                                    char_cycle_arche_local = true;
                                }
                                ui.add_space(6.0);
                                ui.horizontal_wrapped(|ui| {
                                    ui.weak("Colour:");
                                    for (name, tint) in
                                        crate::player_character::TINT_SWATCHES.iter()
                                    {
                                        let sw = egui::Color32::from_rgb(
                                            (tint[0].clamp(0.0, 1.0) * 200.0) as u8 + 40,
                                            (tint[1].clamp(0.0, 1.0) * 200.0) as u8 + 40,
                                            (tint[2].clamp(0.0, 1.0) * 200.0) as u8 + 40,
                                        );
                                        let b = egui::Button::new(
                                            egui::RichText::new(*name).color(sw),
                                        );
                                        if ui.add(b).clicked() {
                                            char_tint_local = Some(*tint);
                                        }
                                    }
                                });
                            }
                            // r686: BUILD SLIDERS now show for EVERY selected character
                            // (was gated to the custom slot, so heroes/presets had none).
                            // char_build_edit is loaded from the selected character, so
                            // edits apply to that model live; "Save & Use" persists it.
                            {
                                // r538: BODY BUILD sliders (forge heroes are shaped by
                                // these live; ordinary body-plans ignore them for now).
                                ui.add_space(8.0);
                                ui.weak(if char_is_hero {
                                    "Build:"
                                } else {
                                    "Build (affects forge heroes):"
                                });
                                let mut build_changed = false;
                                let accent = unendlich_ui::current_theme(ui.ctx()).accent;
                                // tallness is the stature in BLOCKS (0.1..50.0, default 1.8);
                                // see docs/STATURE.md. Semi-LOG slider (equal travel = equal
                                // ratio) so there's fine control near the human default and it
                                // still reaches the giants; the value is shown alongside.
                                ui.horizontal(|ui| {
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(78.0, 22.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| { ui.weak("tallness"); },
                                    );
                                    // r569: max raised 25 → 50 blocks.
                                    let (lo, hi) = (
                                        crate::sim::stature::MIN_HEIGHT,
                                        crate::sim::stature::MAX_HEIGHT,
                                    );
                                    let mut t = char_build_edit.height.clamp(lo, hi).ln();
                                    if unendlich_ui::menu::slider(ui, &mut t, lo.ln(), hi.ln(), 0, accent).changed() {
                                        char_build_edit.height = t.exp().clamp(lo, hi);
                                        build_changed = true;
                                    }
                                    ui.add_space(6.0);
                                    ui.label(egui::RichText::new(format!("{:.1} blk", char_build_edit.height)).strong().color(accent));
                                });
                                let mut row = |ui: &mut egui::Ui, label: &str, v: &mut f32, lo: f32, hi: f32, dec: usize| {
                                    ui.horizontal(|ui| {
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(78.0, 22.0),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| { ui.weak(label); },
                                        );
                                        if unendlich_ui::menu::slider(ui, v, lo, hi, dec, accent).changed() {
                                            build_changed = true;
                                        }
                                    });
                                };
                                row(ui, "muscle", &mut char_build_edit.bulk, 0.7, 1.5, 0);
                                row(ui, "head", &mut char_build_edit.head, 0.7, 1.4, 0);
                                row(ui, "shoulders", &mut char_build_edit.shoulders, 0.7, 1.5, 0);
                                row(ui, "arms", &mut char_build_edit.arm, 0.7, 1.5, 0);
                                row(ui, "legs", &mut char_build_edit.leg, 0.7, 1.4, 0);
                                if build_changed {
                                    char_build_local = Some(char_build_edit);
                                }
                            }
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                if char_selected == char_custom_index {
                                    if tbtn(ui, "  Reroll look  ") { char_reroll_local = true; }
                                    if tbtn(ui, "  Save & Use  ") { char_save_local = true; }
                                } else if tbtn(ui, "  Use this one  ") {
                                    char_save_local = true;
                                }
                                if tbtn(ui, "  Close  ") { char_close_local = true; }
                            });
                            ui.add_space(4.0);
                            ui.weak("\u{2605} is your saved character — it carries across every world.");
                            ui.weak("In-world, press F5 to see your character (third-person).");
                        });
                }
                // ── r169: ANALYTICS dashboard (F4) ──
                if perf_window_open {
                    egui::Window::new("analytics")
                        .frame(glass.clone())
                        .default_pos(egui::pos2(16.0, 60.0))
                        .resizable(false)
                        .show(c, |ui| {
                            unendlich_ui::set_theme(ui.ctx(), unendlich_ui::Theme::DARK);
                            ui.label("frame time (ms, recent)");
                            sparkline(ui, &perf_samples, 60.0);
                            ui.separator();
                            for s in &perf_stats {
                                ui.label(s);
                            }
                            ui.add_space(6.0);
                            if tbtn(ui, "export ascii graph to log") {
                                perf_export = true;
                            }
                        });
                }
                // ── New-game wizard (Singleplayer → configure a world) ──
                if wizard_open {
                    egui::Window::new("unendlich_wizard")
                        .frame(glass.clone())
                        .anchor(egui::Align2::CENTER_CENTER, cone_par(c, "unendlich_wizard", 0.012))
                        .collapsible(false)
                        .resizable(false)
                        .title_bar(false)
                        .show(c, |ui| {
                            unendlich_ui::set_theme(ui.ctx(), unendlich_ui::Theme::DARK);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new(wizard_title.to_uppercase())
                                        .font(unendlich_ui::menu::blocks_font(24.0))
                                        .color(egui::Color32::from_rgb(0xee, 0xf3, 0xe8))
                                        .extra_letter_spacing(4.0),
                                );
                                ui.add_space(8.0);
                                ui.label(&wizard_summary);
                                ui.add_space(12.0);
                                ui.horizontal(|ui| {
                                    if tbtn(ui, " Mode ") { wizard_pick = 5; }
                                    if tbtn(ui, " Seed ") { wizard_pick = 6; }
                                });
                                // r179: name the world (action 7 applies).
                                ui.horizontal(|ui| {
                                    ui.label("name:");
                                    let resp = ui.text_edit_singleline(&mut wizard_name_buf);
                                    // r205: focus the name box the first frame
                                    // the wizard opens, so typing works without
                                    // a precise click on the field (part of the
                                    // "text entry not passing through" fix).
                                    if wizard_focus_name {
                                        resp.request_focus();
                                    }
                                    if resp.lost_focus() || tbtn(ui, "set") {
                                        wizard_pick = 7;
                                    }
                                });
                                // r181: birds-eye equirect strip of the
                                // chosen seed (96x48, nearest-scaled 3x).
                                if wizard_preview_dirty && !wizard_preview_uploaded {
                                    let img = egui::ColorImage::from_rgb(
                                        [96, 48],
                                        &wizard_preview_rgb,
                                    );
                                    wizard_preview_tex = Some(c.load_texture(
                                        "unendlich_seed_preview",
                                        img,
                                        egui::TextureOptions::NEAREST,
                                    ));
                                    wizard_preview_uploaded = true;
                                }
                                if let Some(tex) = &wizard_preview_tex {
                                    ui.add_space(8.0);
                                    let img = ui.add(
                                        egui::Image::new((tex.id(), egui::vec2(288.0, 144.0)))
                                            .sense(egui::Sense::click()),
                                    );
                                    // r185: click a region -> seek seeds that
                                    // MATCH it (right-click: AVOID it).
                                    if img.clicked() || img.secondary_clicked() {
                                        if let Some(pos) = img.interact_pointer_pos() {
                                            let r = img.rect;
                                            let u = ((pos.x - r.min.x) / r.width()).clamp(0.0, 1.0);
                                            let v = ((pos.y - r.min.y) / r.height()).clamp(0.0, 1.0);
                                            wizard_preview_click_out = Some((u, v, img.secondary_clicked()));
                                        }
                                    }
                                    ui.weak("birds-eye \u{00b7} equirect strip");
                                    ui.add_space(4.0);
                                }
                                // r181: type a seed directly (Enter/apply).
                                ui.horizontal(|ui| {
                                    ui.label("seed:");
                                    let resp = ui.text_edit_singleline(&mut wizard_seed_buf);
                                    if resp.lost_focus() || tbtn(ui, "apply") {
                                        wizard_pick = 8;
                                    }
                                });
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    if tbtn(ui, " Back ") { wizard_pick = 1; }
                                    if wizard_at_confirm {
                                        if tbtn(ui, " Create ") { wizard_pick = 3; }
                                    } else if wizard_can_next {
                                        if tbtn(ui, " Next ") { wizard_pick = 2; }
                                    }
                                    // r185: one-click create from any step —
                                    // blank name gets a default bin-side.
                                    if tbtn(ui, " Create World ") { wizard_pick = 9; }
                                });
                                ui.add_space(6.0);
                                if tbtn(ui, " Cancel ") { wizard_pick = 4; }
                            });
                        });
                }
                // ── Debug · shaderpack panel (Esc) ──
                if settings_open {
                    use crate::debug_settings::{DEBUG, Look};
                    use std::sync::atomic::Ordering::Relaxed;
                    egui::Window::new("debug \u{00b7} shaderpack")
                        .default_pos(egui::pos2(16.0, 220.0))
                        .resizable(false)
                        .show(c, |ui| {
                            unendlich_ui::set_theme(ui.ctx(), unendlich_ui::Theme::DARK);
                            ui.label("look (atmosphere preset)");
                            let mut look = DEBUG.look.load(Relaxed);
                            let before = look;
                            for (v, name) in Look::ALL.iter().copied() {
                                ui.radio_value(&mut look, v, name);
                            }
                            if look != before { DEBUG.look.store(look, Relaxed); }
                            ui.separator();
                            macro_rules! tog { ($f:ident, $l:expr) => {{
                                let mut v = DEBUG.$f.load(Relaxed);
                                if ui.checkbox(&mut v, $l).changed() { DEBUG.$f.store(v, Relaxed); }
                            }}; }
                            tog!(disable_fog, "disable fog");
                            tog!(disable_ssr, "disable SSR (hook)");
                            tog!(disable_bloom, "disable bloom (hook)");
                            tog!(disable_shadows, "disable shadows (hook)");
                            tog!(no_cull, "no backface cull (rebuild)");
                            tog!(freeze_lod, "freeze LOD (hook)");
                            tog!(freeze_time, "freeze time (hook)");
                            tog!(pause_sim, "pause sim (hook)");
                            tog!(mesher_mid_gpu, "GPU mesher \u{00b7} mid (hook)");
                            tog!(neutral_tint, "neutral tint (hook)");
                            tog!(show_chunk_borders, "chunk borders (hook)");
                            ui.separator();
                            // r314: live button-style harness — flip through the 10
                            // button looks in-game and tell me which you want.
                            ui.label("button style (0-9, live)");
                            let mut bs = unendlich_ui::button_style() as i32;
                            if ui.add(egui::Slider::new(&mut bs, 0..=9).text("style")).changed() {
                                unendlich_ui::set_button_style(bs.clamp(0, 9) as u8);
                            }
                            ui.label(match unendlich_ui::button_style() {
                                0 => "0: header bar",
                                1 => "1: SPECIAL (default / main menu)",
                                2 => "2: pulse fill",
                                3 => "3: pulse ghost",
                                4 => "4: pulse framed",
                                6 => "6: ELEGANT (cursor infill)",
                                5 | 7 | 8 | 9 => "(removed -> special)",
                                _ => "(default: special)",
                            });
                            ui.separator();
                            ui.label("winding sweep (front-face \u{00d7} cull)");
                            let mut wv = DEBUG.winding_variant.load(Relaxed) as i32;
                            if ui.add(egui::Slider::new(&mut wv, 0..=5).text("variant")).changed() {
                                DEBUG.winding_variant.store(wv.clamp(0, 5) as u32, Relaxed);
                            }
                            ui.label(match DEBUG.winding_variant.load(Relaxed) {
                                1 => "1: CW + cull FRONT  (shows back-faces)",
                                2 => "2: CW + cull NONE   (both sides)",
                                3 => "3: CCW + cull BACK  (flipped front-face)",
                                4 => "4: CCW + cull FRONT",
                                5 => "5: CCW + cull NONE  (both sides)",
                                _ => "0: CW + cull BACK   (default)",
                            });
                            ui.separator();
                            // r592: live foliage-disturber strength — amp the "plants
                            // part around you" push to confirm it's working. 1.0 = the
                            // r591 default; higher also widens the reach so cranking it
                            // is a decisive test even if little foliage is right at your feet.
                            ui.label("foliage disturber (live) — plants part around you");
                            let mut fd = DEBUG.foliage_disturb.load(Relaxed) as f32 / 10.0;
                            if ui.add(egui::Slider::new(&mut fd, 0.0..=30.0).text("strength")).changed() {
                                DEBUG.foliage_disturb.store((fd.clamp(0.0, 30.0) * 10.0) as u32, Relaxed);
                            }
                            ui.label("1.0 = default · 0 = off · crank to ~15+ to confirm");
                            ui.separator();
                            if ui.button("Quit").clicked() {
                                DEBUG.request_quit.store(true, Relaxed);
                            }
                        });
                }
                // ── HUD (top-left) ──
                egui::Area::new(egui::Id::new("unendlich_hud"))
                    .fixed_pos(egui::pos2(16.0, 16.0))
                    .show(c, |ui| {
                        // Components now read the active theme from ctx memory,
                        // which persists across panels/frames — so each panel sets
                        // its own. The HUD is the default dark theme.
                        unendlich_ui::set_theme(ui.ctx(), unendlich_ui::Theme::DARK);
                        unendlich_ui::card(
                            ui,
                            unendlich_ui::AMBER,
                            Some("unendlich"),
                            |ui| {
                                ui.label(format!("frame {}", hud_frame));
                                ui.label(format!("chunks loaded: {}", hud_chunks));
                                ui.horizontal(|ui| {
                                    let wait_color = if hud_wait_ms > 10.0 {
                                        unendlich_ui::RED
                                    } else {
                                        unendlich_ui::GREEN
                                    };
                                    unendlich_ui::pill(
                                        ui,
                                        &format!("wait {:.1}ms", hud_wait_ms),
                                        wait_color,
                                        false,
                                    );
                                    unendlich_ui::pill(
                                        ui,
                                        &format!("fif {}", hud_slot),
                                        unendlich_ui::CYAN,
                                        false,
                                    );
                                });
                                ui.label(format!(
                                    "xyz  {:.1}  {:.1}  {:.1}",
                                    hud_pos.x, hud_pos.y, hud_pos.z
                                ));
                                ui.label(format!("light: {:.2}", hud_ambient));
                                if let Some(it) = items.get(selected) {
                                    ui.label(format!("in hand: {}", it.display_name()));
                                }
                                for line in &hud_status_lines {
                                    ui.label(line);
                                }
                            },
                        );
                    });
                // ── Creative item picker (top-right). Clickable once pointer input
                // is fed to the overlay (build-loop step B); renders now. ──
                // #1: only while in panel mode (mouse free); hidden in game mode.
                if show_creative && !menu_open && !wizard_open { // r175: not on the title
                egui::Area::new(egui::Id::new("unendlich_creative"))
                    .fixed_pos(egui::pos2(
                        ((pw - 372.0) * 0.5).max(16.0),
                        ((ph - 520.0) * 0.5).max(16.0),
                    ))
                    .show(c, |ui| {
                        ui.set_max_width(356.0); // bound the grid so grid_fit wraps
                        // Theme the picker by the in-hand item's display color
                        // (block albedo / creature / metal). Shifts per selection.
                        let [tr, tg, tb] = items.get(selected).map(|i| i.display_color()).unwrap_or([120, 120, 120]);
                        let theme = unendlich_ui::Theme::from_material(
                            egui::Color32::from_rgb(tr, tg, tb),
                        )
                        .lit(hud_ambient);
                        let title = if survival_inv {
                            "backpack"
                        } else {
                            creative_tab.label()
                        };
                        unendlich_ui::card_themed(
                            ui,
                            theme,
                            Some(title),
                            |ui| {
                                // Tab strip (creative only — survival shows the real
                                // backpack, no catalog tabs).
                                if !survival_inv {
                                ui.horizontal(|ui| {
                                    for t in Tab::ALL {
                                        let lbl = t.label();
                                        if unendlich_ui::tab_button(ui, lbl, creative_tab == t).clicked() {
                                            creative_tab = t;
                                            selected = 0;
                                        }
                                    }
                                });
                                }
                                let accent = unendlich_ui::current_theme(ui.ctx()).accent;
                                if survival_inv {
                                    // r662: interactive inventory — a 3×9 backpack
                                    // over a 9-slot hotbar row, drawn from the bin's
                                    // POSITIONAL `inv_slots` ([0..hb) = hotbar, rest =
                                    // backpack). Click a slot to pick its stack onto
                                    // the cursor; click another to drop it (e.g. into
                                    // the hotbar). The clicked GLOBAL index rides back
                                    // to the bin via `inv_click`.
                                    let hb = inv_hotbar_size.min(inv_slots.len());
                                    let draw_slot = |ui: &mut egui::Ui, rect: egui::Rect, view: Option<&InvSlotView>| {
                                        let p = ui.painter();
                                        if let Some(s) = view {
                                            let [r, g, b] = s.color;
                                            p.rect_filled(rect.shrink(8.0), egui::CornerRadius::same(2), egui::Color32::from_rgb(r, g, b));
                                            if s.count > 1 {
                                                p.text(
                                                    rect.right_top() + egui::vec2(-4.0, 2.0),
                                                    egui::Align2::RIGHT_TOP,
                                                    format!("{}", s.count),
                                                    egui::FontId::proportional(11.0),
                                                    egui::Color32::from_rgb(236, 240, 248),
                                                );
                                            }
                                            let short: String = s.name.chars().take(6).collect();
                                            p.text(
                                                rect.center_bottom() - egui::vec2(0.0, 3.0),
                                                egui::Align2::CENTER_BOTTOM,
                                                short,
                                                egui::FontId::proportional(9.0),
                                                egui::Color32::from_rgb(220, 228, 242),
                                            );
                                        }
                                    };
                                    // Backpack: slots [hb..len) forced 9-wide → 3 rows for 27.
                                    let back_count = inv_slots.len().saturating_sub(hb);
                                    if let Some(k) = unendlich_ui::menu::cell_grid_cols(
                                        ui, "survival_backpack", back_count, 9, 48.0, 6.0, accent,
                                        |ui, i, rect| {
                                            draw_slot(ui, rect, inv_slots.get(hb + i).and_then(|o| o.as_ref()));
                                        },
                                    ) {
                                        inv_click = (hb + k) as i32;
                                    }
                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new("HOTBAR")
                                            .size(11.0)
                                            .color(egui::Color32::from_rgb(176, 186, 202))
                                            .extra_letter_spacing(3.0),
                                    );
                                    // Hotbar row: slots [0..hb) as a single 9-wide row.
                                    if let Some(k) = unendlich_ui::menu::cell_grid_cols(
                                        ui, "survival_hotbar", hb, 9, 48.0, 6.0, accent,
                                        |ui, i, rect| {
                                            draw_slot(ui, rect, inv_slots.get(i).and_then(|o| o.as_ref()));
                                        },
                                    ) {
                                        inv_click = k as i32;
                                    }
                                    // Cursor: draw the held stack near the pointer.
                                    if let Some(cur) = inv_cursor.as_ref() {
                                        if let Some(pp) = ui.ctx().pointer_hover_pos() {
                                            let p = ui.painter();
                                            let r = egui::Rect::from_center_size(
                                                pp + egui::vec2(16.0, 16.0),
                                                egui::vec2(26.0, 26.0),
                                            );
                                            let [rr, gg, bb] = cur.color;
                                            p.rect_filled(r, egui::CornerRadius::same(3), egui::Color32::from_rgb(rr, gg, bb));
                                            if cur.count > 1 {
                                                p.text(
                                                    r.right_bottom(),
                                                    egui::Align2::RIGHT_BOTTOM,
                                                    format!("{}", cur.count),
                                                    egui::FontId::proportional(11.0),
                                                    egui::Color32::from_rgb(236, 240, 248),
                                                );
                                            }
                                        }
                                    }
                                } else if creative_tab == Tab::Inventory {
                                    // r657: the creative Inventory tab is now the
                                    // player's own 3×9 grid (previously empty — it
                                    // showed the catalog's empty "Inventory" category).
                                    // The player fills it by clicking catalog items;
                                    // clicking a slot here puts that item in hand.
                                    let inv_snapshot = creative_inventory;
                                    if let Some(i) = unendlich_ui::menu::cell_grid(
                                        ui, "creative_backpack", 27, 56.0, 6.0, accent,
                                        |ui, i, rect| {
                                            if let Some(Some(it)) = inv_snapshot.get(i) {
                                                let [r, g, b] = it.display_color();
                                                let p = ui.painter();
                                                p.rect_filled(rect.shrink(8.0), egui::CornerRadius::same(2), egui::Color32::from_rgb(r, g, b));
                                                let short: String = it.display_name().chars().take(6).collect();
                                                p.text(
                                                    rect.center_bottom() - egui::vec2(0.0, 3.0),
                                                    egui::Align2::CENTER_BOTTOM,
                                                    short,
                                                    egui::FontId::proportional(9.0),
                                                    egui::Color32::from_rgb(220, 228, 242),
                                                );
                                            }
                                        },
                                    ) {
                                        if let Some(Some(it)) = inv_snapshot.get(i).copied() {
                                            creative_place_item = Some(it);
                                            if let Some(slot) = creative_hotbar.get_mut(creative_hotbar_sel) {
                                                *slot = Some(it);
                                            }
                                        }
                                    }
                                } else if let Some(i) = unendlich_ui::menu::cell_grid(
                                    ui, "creative_inv", items.len(), 56.0, 6.0, accent,
                                    |ui, i, rect| {
                                        let name = items[i].display_name();
                                        let short: String = name.chars().take(6).collect();
                                        let [r, g, b] = items[i].display_color();
                                        // colour swatch (a small filled inset),
                                        // brighter when this is the selected slot
                                        let sw = egui::Color32::from_rgb(r, g, b);
                                        let inset = rect.shrink(8.0);
                                        let p = ui.painter();
                                        p.rect_filled(
                                            inset,
                                            egui::CornerRadius::same(2),
                                            if i == selected { sw } else { sw.gamma_multiply(0.7) },
                                        );
                                        if i == selected {
                                            p.rect_stroke(
                                                rect.shrink(1.0),
                                                egui::CornerRadius::same(0),
                                                egui::Stroke::new(1.5, accent),
                                                egui::StrokeKind::Inside,
                                            );
                                        }
                                        p.text(
                                            rect.center_bottom() - egui::vec2(0.0, 3.0),
                                            egui::Align2::CENTER_BOTTOM,
                                            short,
                                            egui::FontId::proportional(9.0),
                                            egui::Color32::from_rgb(220, 228, 242),
                                        );
                                    },
                                ) {
                                    // r657: clicking a catalog item selects it,
                                    // makes it the item to PLACE (the bin reads
                                    // creative_place_item), and drops it into the
                                    // active hotbar slot so the bar fills as you pick.
                                    selected = i;
                                    if let Some(&it) = items.get(i) {
                                        creative_place_item = Some(it);
                                        if let Some(slot) = creative_hotbar.get_mut(creative_hotbar_sel) {
                                            *slot = Some(it);
                                        }
                                        // collect into the 3×9 inventory (first empty
                                        // slot), skipping if it's already in there.
                                        if !creative_inventory.iter().any(|s| *s == Some(it)) {
                                            if let Some(free) = creative_inventory.iter_mut().find(|s| s.is_none()) {
                                                *free = Some(it);
                                            }
                                        }
                                    }
                                }
                                // r553: HOTBAR SWITCHER — the quick-bar slots inline
                                // in the centered inventory; click to pick the in-hand
                                // slot (mirrors the bottom hotbar + the scroll wheel).
                                // r576: creative-only (it lists the placeable catalog,
                                // not the survival backpack).
                                if !survival_inv {
                                ui.add_space(8.0);
                                ui.separator();
                                ui.label(
                                    egui::RichText::new("HOTBAR")
                                        .size(11.0)
                                        .color(egui::Color32::from_rgb(176, 186, 202))
                                        .extra_letter_spacing(3.0),
                                );
                                ui.horizontal(|ui| {
                                    // r665: mirror the REAL creative hotbar (the same
                                    // creative_hotbar the bottom HUD row shows), so the
                                    // two are always in sync — was `items[i]` (the
                                    // current tab's first 9), which diverged per-tab and
                                    // read empty on tabs like Inventory.
                                    for i in 0..9 {
                                        let assigned = creative_hotbar.get(i).copied().flatten();
                                        let (rect, resp) = ui.allocate_exact_size(
                                            egui::vec2(30.0, 30.0),
                                            egui::Sense::click(),
                                        );
                                        let p = ui.painter();
                                        p.rect_filled(
                                            rect,
                                            egui::CornerRadius::same(3),
                                            egui::Color32::from_rgba_unmultiplied(20, 24, 30, 200),
                                        );
                                        if let Some(it) = assigned {
                                            let [r, g, b] = it.display_color();
                                            let sw = egui::Color32::from_rgb(r, g, b);
                                            p.rect_filled(
                                                rect.shrink(5.0),
                                                egui::CornerRadius::same(2),
                                                if i == creative_hotbar_sel { sw } else { sw.gamma_multiply(0.65) },
                                            );
                                        }
                                        if i == creative_hotbar_sel {
                                            p.rect_stroke(
                                                rect.shrink(1.0),
                                                egui::CornerRadius::same(0),
                                                egui::Stroke::new(1.5, accent),
                                                egui::StrokeKind::Inside,
                                            );
                                        }
                                        p.text(
                                            rect.left_top() + egui::vec2(3.0, 1.0),
                                            egui::Align2::LEFT_TOP,
                                            format!("{}", i + 1),
                                            egui::FontId::proportional(8.0),
                                            egui::Color32::from_rgb(210, 216, 228),
                                        );
                                        if resp.clicked() {
                                            // select this hotbar slot (so the next catalog
                                            // click fills it) and, if it holds an item,
                                            // make that the item to place.
                                            creative_hotbar_sel = i.min(9);
                                            if let Some(it) = assigned {
                                                creative_place_item = Some(it);
                                            }
                                        }
                                    }
                                });
                                } // end hotbar switcher (creative only)
                                // r667: SELECTED-ITEM TOOLTIP — compose the canonical
                                // tooltip lines (name / category · rarity / …) via the
                                // pure sim::item_tooltip core and draw them under the
                                // catalog, styled by each line's emphasis/role.
                                if !survival_inv {
                                    if let Some(sel_item) = items.get(selected) {
                                        let name = sel_item.display_name();
                                        let (kind_label, rarity) = match sel_item {
                                            crate::item::ItemKind::Block(_) => ("Block", 0u8),
                                            crate::item::ItemKind::SpawnEgg(_) => ("Spawn Egg", 0),
                                            crate::item::ItemKind::Ingot(_) => ("Ingot", 1),
                                            crate::item::ItemKind::Tool(..) => ("Tool", 1),
                                            crate::item::ItemKind::Legendary(_) => ("Legendary Weapon", 4),
                                        };
                                        let view = crate::sim::item_tooltip::ItemTooltipView {
                                            name: &name,
                                            rarity,
                                            kind_label: Some(kind_label),
                                            slot_label: None,
                                            affixes: &[],
                                            lore: "",
                                            attribution: None,
                                            lore_cols: 0,
                                        };
                                        ui.add_space(8.0);
                                        ui.separator();
                                        for line in crate::sim::item_tooltip::compose(&view) {
                                            use crate::sim::item_tooltip::{Emphasis, TooltipRole};
                                            let color = match line.emphasis {
                                                Emphasis::Strong => egui::Color32::from_rgb(236, 240, 248),
                                                Emphasis::Normal => egui::Color32::from_rgb(210, 216, 228),
                                                Emphasis::Muted => egui::Color32::from_rgb(150, 158, 172),
                                            };
                                            let mut rt = egui::RichText::new(&line.text).color(color);
                                            if line.emphasis == Emphasis::Strong {
                                                rt = rt.size(15.0).strong();
                                            } else if line.role == TooltipRole::Lore {
                                                rt = rt.italics();
                                            }
                                            ui.label(rt);
                                        }
                                    }
                                }
                                // r553: CRAFTING merged into the same panel (the single
                                // `p` UI now shows backpack + hotbar + crafting). Recipes
                                // are filled by the bin while this panel is open.
                                if !craft_lines.is_empty() {
                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.label(
                                        egui::RichText::new("CRAFTING")
                                            .size(11.0)
                                            .color(egui::Color32::from_rgb(176, 186, 202))
                                            .extra_letter_spacing(3.0),
                                    );
                                    egui::ScrollArea::vertical()
                                        .max_height(210.0)
                                        .auto_shrink([false, true])
                                        .show(ui, |ui| {
                                            for (i, (name, can, have, tip)) in craft_lines.iter().enumerate() {
                                                ui.horizontal(|ui| {
                                                    ui.add_enabled_ui(*can, |ui| {
                                                        if ui
                                                            .button(format!("craft  {name}"))
                                                            .on_hover_text(tip.clone())
                                                            .clicked()
                                                        {
                                                            craft_pick = (i + 1) as u8;
                                                        }
                                                    });
                                                    ui.weak(have);
                                                });
                                            }
                                        });
                                }
                            },
                        );
                    });
                } // end `if show_creative` (#1: hide picker in game mode)
                // ── Hotbar (bottom-center): the classic quick-bar, neutral
                // (dark) chrome. Shows the first slots; selected highlighted. ──
                // r175: in-world overlays draw only IN WORLD — the title is
                // menus + orbit backdrop, nothing else. One predicate; any
                // future overlay must join it rather than invent its own.
                let in_world_overlays = !menu_open && !wizard_open;
                if in_world_overlays {
                let hb_n = items.len().min(9);
                let hb_w = unendlich_ui::hotbar_width(hb_n, 44.0, 4.0);
                egui::Area::new(egui::Id::new("unendlich_hotbar"))
                    .fixed_pos(egui::pos2(
                        ((pw - hb_w) * 0.5).max(8.0),
                        ph - 64.0,
                    ))
                    .show(c, |ui| {
                        unendlich_ui::set_theme(ui.ctx(), unendlich_ui::Theme::DARK.lit(hud_ambient));
                        unendlich_ui::hotbar(ui, hb_n, 4.0, |ui, i| {
                            // r655: show the drag-assigned item if any, else fall
                            // back to the palette item in that column.
                            // r665: creative_hotbar is the single source of truth for
                            // BOTH this HUD row and the panel switcher (no per-tab
                            // palette fallback, which made them disagree / go empty).
                            let shown = creative_hotbar.get(i).copied().flatten();
                            let sw = shown.map(|it| {
                                let [r, g, b] = it.display_color();
                                egui::Color32::from_rgb(r, g, b)
                            });
                            // Dvorak home-row key that fast-selects this slot.
                            let key = crate::hotbar::slot_homerow_key(i).map(|c| c.to_string());
                            let resp = unendlich_ui::slot_swatch(ui, 44.0, i == creative_hotbar_sel, sw, key.as_deref());
                            // r657: click a hotbar slot to make it active; if it
                            // holds an item, that becomes the item to place.
                            if resp.clicked() {
                                creative_hotbar_sel = i.min(9);
                                if let Some(Some(it)) = creative_hotbar.get(i) {
                                    creative_place_item = Some(*it);
                                }
                            }
                        });
                    });
                // r545: SURVIVAL METERS — small bars above the hotbar (hunger +
                // thermal; thirst/stamina join in Roll 3). Only in survival mode.
                if let Some(sv) = hud_survival {
                    egui::Area::new(egui::Id::new("unendlich_survival_hud"))
                        .fixed_pos(egui::pos2(12.0, ph - 190.0))
                        .show(c, |ui| {
                            let bar = |ui: &mut egui::Ui, label: &str, frac: f32, col: egui::Color32| {
                                let f = frac.clamp(0.0, 1.0);
                                let (rect, _resp) =
                                    ui.allocate_exact_size(egui::vec2(140.0, 14.0), egui::Sense::hover());
                                let p = ui.painter_at(rect);
                                p.rect_filled(rect, 3.0, egui::Color32::from_rgba_unmultiplied(8, 10, 14, 180));
                                let mut fill = rect;
                                fill.set_width(rect.width() * f);
                                p.rect_filled(fill, 3.0, col);
                                p.text(
                                    rect.left_center() + egui::vec2(6.0, 0.0),
                                    egui::Align2::LEFT_CENTER,
                                    label,
                                    egui::FontId::proportional(10.0),
                                    egui::Color32::from_rgb(235, 238, 242),
                                );
                            };
                            // hunger: green→amber→red as it empties.
                            let hcol = if sv.hunger > 0.5 {
                                egui::Color32::from_rgb(96, 184, 96)
                            } else if sv.hunger > 0.25 {
                                egui::Color32::from_rgb(210, 170, 60)
                            } else {
                                egui::Color32::from_rgb(200, 70, 60)
                            };
                            bar(ui, "Hunger", sv.hunger, hcol);
                            ui.add_space(3.0);
                            // thirst: blue, amber when thirsty, red when parched.
                            let wcol = if sv.thirst > 0.5 {
                                egui::Color32::from_rgb(70, 150, 210)
                            } else if sv.thirst > 0.2 {
                                egui::Color32::from_rgb(210, 170, 60)
                            } else {
                                egui::Color32::from_rgb(200, 70, 60)
                            };
                            bar(ui, "Thirst", sv.thirst, wcol);
                            ui.add_space(3.0);
                            // stamina: cyan-green; dims as it empties.
                            bar(ui, "Stamina", sv.stamina, egui::Color32::from_rgb(90, 200, 170));
                            ui.add_space(3.0);
                            // thermal: comfort −1..1 → bar fill from center; blue cold, green ok, red hot.
                            let c01 = (sv.comfort.clamp(-1.0, 1.0) * 0.5 + 0.5);
                            let warmth = sv.comfort.clamp(-1.0, 1.0);
                            let tcol = if warmth < -0.25 {
                                egui::Color32::from_rgb(90, 150, 220) // cold
                            } else if warmth > 0.25 {
                                egui::Color32::from_rgb(220, 110, 60) // hot
                            } else {
                                egui::Color32::from_rgb(96, 184, 120) // comfortable
                            };
                            // show "distance from comfortable" as the fill so either extreme grows the bar
                            bar(ui, &format!("Warmth {:.0}\u{b0}C", sv.body_c), 1.0 - (c01 - 0.5).abs() * 2.0, tcol);
                        });
                }
                } // r175b: end in_world_overlays gate (correct nesting)
                wants_keyboard_out = c.wants_keyboard_input(); // r185
            })?;
            self.creative_selected = selected;
            self.creative_hotbar = creative_hotbar;
            self.creative_inventory = creative_inventory;
            self.creative_hotbar_sel = creative_hotbar_sel;
            self.creative_place_item = creative_place_item;
            self.inv_click = inv_click;
            self.creative_tab = creative_tab;
            if menu_pick != 0 { self.menu_choice = menu_pick; }
            if menu_world_pick_local >= 0 { self.menu_world_pick = menu_world_pick_local; } // r197
            if menu_mp_pick_local >= 0 { self.menu_mp_pick = menu_mp_pick_local; } // r397
            if menu_rollback_peek_local >= 0 { self.menu_rollback_peek = menu_rollback_peek_local; } // r203
            if menu_rollback_restore_local >= 0 { self.menu_rollback_restore = menu_rollback_restore_local; } // r203
            if menu_world_delete_ask_local >= 0 { self.menu_world_delete_ask = menu_world_delete_ask_local; } // r205
            if dev_make_point_local { self.dev_make_history_point = true; } // r206
            if menu_rollback_delete_local >= 0 { self.menu_rollback_delete = menu_rollback_delete_local; } // r206
            if menu_world_delete_cancel_local { self.menu_world_delete_ask = -1; } // r205
            if menu_world_delete_confirm_local >= 0 {
                self.menu_world_delete_confirm = menu_world_delete_confirm_local; // r205
                self.menu_world_delete_ask = -1; // close the confirm row
            }
            if keybind_rebind_local >= 0 { self.keybind_rebind_request = keybind_rebind_local; } // r199
            if settings_menu_open_local { self.settings_menu_open = true; } // r203 menu-opened settings
            // r202: only commit GUI scale on release (see slider). Without
            // the commit gate the staged value never touches the live scale
            // mid-drag, so the UI no longer resizes under the cursor.
            if ui_scale_commit {
                self.ui_scale = ui_scale_local.clamp(1.0, 3.0);
            }
            if pause_pick != 0 { self.pause_action = pause_pick; } // r167
            // r169 write-backs.
            self.settings_tab = settings_tab;
            self.ui_day_length = ui_day_length;
            self.ui_game_speed_pos = ui_game_speed_pos;
            self.ui_music_vol = ui_music_vol;
            self.ui_fog_on = ui_fog_on;
            self.ui_shadows_on = ui_shadows_on;
            self.ui_debug_master = ui_debug_master;
            self.ui_debug_water = ui_debug_water;
            self.ui_debug_flow = ui_debug_flow;
            self.ui_debug_echo = ui_debug_echo;
            if settings_close { self.settings_menu_open = false; }
            if craft_pick != 0 { self.craft_pick = craft_pick; }
            if perf_export { self.perf_export_ascii = true; } // r177
            if craft_close { self.craft_menu_open = false; }
            // r488: character-select write-back. Apply selection/reroll, then on
            // Save make the highlighted character the saved global default (so it
            // loads in every world) and hand it to the bin via character_chosen.
            if open_character_local {
                if self.character_roster.is_none() {
                    self.character_roster = Some(crate::player_character::CharacterRoster::load());
                }
                self.character_menu_open = true;
            }
            let mut char_chosen_out: Option<crate::player_character::CharacterProfile> = None;
            let mut char_close_out = char_close_local;
            if let Some(roster) = self.character_roster.as_mut() {
                if char_pick_local >= 0 {
                    roster.select(char_pick_local as usize);
                }
                if char_reroll_local && roster.selected == roster.custom_index {
                    roster.custom_mut().reroll();
                }
                if char_cycle_arche_local && roster.selected == roster.custom_index {
                    roster.custom_mut().cycle_arche();
                }
                if let Some(t) = char_tint_local {
                    if roster.selected == roster.custom_index {
                        roster.custom_mut().set_tint(t);
                    }
                }
                if let Some(b) = char_build_local {
                    // r686: apply the edited build to the SELECTED character (was
                    // custom-only, so hero/preset sliders did nothing). The custom
                    // slot persists across sessions; a preset edit is session-live
                    // (presets regenerate on reload) until the player hits "Save &
                    // Use", which forks the tweaked preset into the custom slot.
                    let sel = roster.selected;
                    if let Some(opt) = roster.options.get_mut(sel) {
                        opt.build = b;
                    }
                }
                if char_save_local {
                    // a chosen preset is copied into the custom slot so whatever the
                    // player picks becomes their persisted, cross-world character.
                    let sel = roster.selected;
                    if sel != roster.custom_index {
                        let picked = roster.options[sel].clone();
                        *roster.custom_mut() = picked;
                        roster.selected = roster.custom_index;
                    }
                    let _ = roster.save_custom();
                    char_chosen_out = Some(roster.current().clone());
                    char_close_out = true;
                }
            }
            if let Some(c) = char_chosen_out {
                self.character_chosen = Some(c);
            }
            if char_close_out {
                self.character_menu_open = false;
            }
            if wizard_pick != 0 { self.wizard_action = wizard_pick; }
            self.wizard_name_buf = wizard_name_buf; // r179
            self.wizard_seed_buf = wizard_seed_buf; // r181
            self.ui_wants_keyboard = wants_keyboard_out; // r185
            if wizard_preview_click_out.is_some() {
                self.wizard_preview_click = wizard_preview_click_out;
            }
            self.ui_plant_stride = ui_plant_stride; // r184
            self.ui_leaf_stride = ui_leaf_stride;
            self.ui_apply_budget = ui_apply_budget;
            self.ui_perf_preset = ui_perf_preset;
            self.ui_render_distance = ui_render_distance; // r197
            self.ui_render_scale = ui_render_scale; // r325
            self.ui_fps_target = ui_fps_target;
            self.ui_autotune = ui_autotune;
            self.ui_gpu_mesh_yield = ui_gpu_mesh_yield;
            self.ui_adaptive_rd = ui_adaptive_rd;
            self.ui_octree_tile = ui_octree_tile;
            self.ui_detached_terrain = ui_detached_terrain;
            self.wizard_preview_tex = wizard_preview_tex; // r182
            if wizard_preview_uploaded {
                self.wizard_preview_dirty = false;
            }
        }

        // v0.21.0: collect this slot's GPU timestamps from the
        // *previous* frame that used it. The fence above guarantees
        // those writes are complete; on the very first cycle through
        // a slot the queries are unwritten and `collect_frame`
        // correctly skips via the availability bit.
        //
        // This must run BEFORE `begin_frame` resets the slot — once
        // we reset, the previous results are gone.
        self.gpu_timer.collect_frame(frame_idx as u32);

        // v0.24.3 chug-debug-2: pull the per-frame GPU zone breakdown
        // out of gpu_timer (it was just populated by collect_frame
        // above) and emit a `gpu_spike` event if total > 100ms.
        //
        // The `frame_idx` field matches the field name in
        // `wait_fences_spike` events. An external tool can join them
        // on (uuid, frame_idx) to learn "for THIS chug, which GPU pass
        // was the culprit."
        //
        // Side-channel only: take_last_frame() consumes the breakdown
        // so this code path doesn't fire twice for the same frame.
        if let Some((collected_idx, per_zone_us)) = self.gpu_timer.take_last_frame() {
            if let Some(&total_us) = per_zone_us.get("total") {
                if total_us > 100_000 {
                    unendlich_shared::events::record(
                        "gpu_spike",
                        serde_json::json!({
                            "frame_idx": collected_idx,
                            "total_us": total_us,
                            "shadow_us": per_zone_us.get("shadow").copied().unwrap_or(0),
                            "main_us":   per_zone_us.get("main").copied().unwrap_or(0),
                            "froxel_us": per_zone_us.get("froxel").copied().unwrap_or(0),
                            "cull_us":   per_zone_us.get("cull").copied().unwrap_or(0),
                        }),
                    );
                }
            }
        }

        // c.13-instrumentation: collect previous frame's shadow bucket
        // timings if bucketing is active. The result is emitted as a
        // structured event so the analyzer can pull bucket-by-bucket
        // microseconds out of events.jsonl.
        if let Some(t) = self.shadow_bucket_timer.as_ref() {
            let bucket_us = t.collect_frame(frame_idx as u32);
            if !bucket_us.is_empty() {
                unendlich_shared::events::record("shadow_buckets_timing", serde_json::json!({
                    "frame_idx": frame_idx,
                    "us_per_bucket": bucket_us,
                    "total_us": bucket_us.iter().sum::<u64>(),
                }));
            }
        }

        let pos = view.position;
        let extent = self.swapchain.extent;

        // Compute the light cascade for this frame. Single cascade
        // for v0.13 — covering the near 200m of the camera frustum.
        // This is the "ground" cascade that catches the player's
        // immediate surroundings; a future increment will add 3 more
        // cascades at 60m / 180m / 600m for distant geometry.
        //
        // Sun direction is already in atmos. We require a unit vector;
        // the Sky module produces one, but defensively normalize.
        let sun_dir_v = glm::vec3(
            atmos.sun_dir[0], atmos.sun_dir[1], atmos.sun_dir[2],
        );
        let sun_dir_norm = if sun_dir_v.norm_squared() > 1e-6 {
            sun_dir_v.normalize()
        } else {
            // Defensive: a zero sun direction would NaN the cascade.
            // Pick a sensible "early afternoon" sun.
            glm::vec3(0.5, -0.85, 0.2).normalize()
        };

        // v0.24.2c: 4 cascades. C0 keeps the (view.near, 200.0) split
        // so the layer-0 render and the main fragment shader's
        // existing UBO-driven sample stay consistent — no visual
        // change. C1..C3 use the design's logarithmic splits but
        // are not sampled by anything yet (v0.24.2d wires that up).
        //
        // The 4 splits MUST match the splits passed to prepare_cascades
        // below, because the GPU cascade_fit feeds the cull frustum
        // and the CPU `cascades[i]` feeds the push-constant matrix the
        // shadow vertex shader rasterizes with. If they diverge,
        // chunks get culled with one frustum and rendered with a
        // different one — chunks pop in/out at frustum edges.
        // v0.30 (shadow-distance cull, A/B knob): UNENDLICH_SHADOW_CULL_DIST=<meters>
        // tightens cascade-0's far so the EXISTING per-view frustum cull drops
        // distant shadow casters — the dominant shadow-pass cost at a fully
        // populated render distance. Default 0/unset = OFF = byte-identical to
        // the prior 200m frustum. The clamp lives on this one array, so it flows
        // into BOTH `single_cascade` (the CPU render matrix) AND `prepare_cascades`
        // (the GPU cull frustum) below — they stay in sync, so chunks don't pop at
        // frustum edges. Visual effect when on: terrain beyond the distance renders
        // unshadowed (lit). That is the intended A/B: distant-shadow feel vs cost.
        let c0_far = match std::env::var("UNENDLICH_SHADOW_CULL_DIST")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|&v| v.is_finite() && v > 0.0)
        {
            // Floor above near so a tiny value can't make a degenerate frustum.
            Some(d) => 200.0_f32.min(d).max(view.near + 1.0),
            None => 200.0_f32,
        };
        let cascade_splits = [
            (view.near, c0_far),     // C0 — matches CPU UBO / fragment sample; far clampable via UNENDLICH_SHADOW_CULL_DIST
            (20.0_f32, 60.0_f32),    // C1 — design split, unused in v0.24.2c
            (60.0_f32, 180.0_f32),   // C2 — design split, unused in v0.24.2c
            (180.0_f32, 600.0_f32),  // C3 — design split, unused in v0.24.2c
        ];
        // v0.24.4: pass the actual shadow extent so the CPU's
        // texel-snap stays in sync with the rendered shadow size.
        // Reading self.shadow.extent.width (set at startup from
        // runtime_shadow_map_size) keeps these aligned even when
        // UNENDLICH_SHADOW_MAP_SIZE overrides the default.
        let shadow_size_f = self.shadow.extent.width as f32;
        let cascades: [_; 4] = [
            single_cascade(view, sun_dir_norm, aspect, cascade_splits[0].0, cascade_splits[0].1, shadow_size_f),
            single_cascade(view, sun_dir_norm, aspect, cascade_splits[1].0, cascade_splits[1].1, shadow_size_f),
            single_cascade(view, sun_dir_norm, aspect, cascade_splits[2].0, cascade_splits[2].1, shadow_size_f),
            single_cascade(view, sun_dir_norm, aspect, cascade_splits[3].0, cascade_splits[3].1, shadow_size_f),
        ];
        let cascade = cascades[0];
        self.last_cascade = Some(cascade);

        // v0.24.4 (shadow-stride): decide whether to use stride this
        // frame or force a full render.
        //
        // Force-full-render triggers:
        //   - stride == 1 (feature disabled)
        //   - first STRIDE frames of the run (initialize the shadow map)
        //   - cascade center moved more than 1 chunk-width since last
        //     frame (fast camera motion → previous frame's depth is
        //     for incorrect texel positions)
        //
        // When NOT forcing full render: pass stride and phase to the
        // shadow cull, use the LOAD render pass (preserves prior depth).
        //
        // Eviction-aware forcing is filed for later; current naive
        // assumption is the cascade-shift trigger handles most cases
        // where stale chunks would matter (player moving = both
        // generates new chunks AND shifts cascade).
        const CHUNK_SIZE_M: f32 = 32.0; // CHUNK_SIZE in meters; matches chunk.rs.
        let cascade_center = view.position;
        let cascade_shifted_far = match self.last_cascade_center {
            None => true, // first frame
            Some(prev) => {
                let delta = cascade_center - prev;
                delta.x.abs() > CHUNK_SIZE_M
                    || delta.y.abs() > CHUNK_SIZE_M
                    || delta.z.abs() > CHUNK_SIZE_M
            }
        };
        self.last_cascade_center = Some(cascade_center);

        let warmup_full_render = monotonic_frame <= self.shadow_chunk_stride as u64;
        // r157: the panel's "disable shadows" toggle — force the CLEAR
        // variant of the pass (an empty shadow map reads as fully lit; the
        // image/barrier contract is untouched) and skip the chunk draws.
        let shadows_disabled = crate::debug_settings::DEBUG
            .disable_shadows
            .load(std::sync::atomic::Ordering::Relaxed);
        // r175: re-enabling must invalidate the reuse cache, or the
        // cleared map is "reused" forever (operator: shadows never return).
        if self.prev_shadows_disabled && !shadows_disabled {
            self.last_shadow_rendered_vp = None;
        }
        self.prev_shadows_disabled = shadows_disabled;
        let force_full_shadow = self.shadow_chunk_stride <= 1
            || warmup_full_render
            || cascade_shifted_far
            || shadows_disabled;

        // Effective stride/phase for the shadow cull dispatch.
        let (shadow_cull_stride, shadow_cull_phase) = if force_full_shadow {
            (1u32, 0u32)
        } else {
            (
                self.shadow_chunk_stride,
                (monotonic_frame % self.shadow_chunk_stride as u64) as u32,
            )
        };

        // Emit shadow_stride diagnostic event.
        if self.shadow_chunk_stride > 1 {
            unendlich_shared::events::record(
                "shadow_stride",
                serde_json::json!({
                    "frame": monotonic_frame,
                    "stride_setting": self.shadow_chunk_stride,
                    "effective_stride": shadow_cull_stride,
                    "phase": shadow_cull_phase,
                    "force_full": force_full_shadow,
                    "warmup_force": warmup_full_render,
                    "cascade_shift_force": cascade_shifted_far,
                }),
            );
        }

        // v0.24.NEXT+5-weather: modulate a local copy of the atmosphere
        // by simulated weather before building the UBO. Rain darkens the
        // sky/sun/ambient and thickens fog; snow whitens; dry overcast
        // greys the sky and diffuses the sun; fog thickens the haze; the
        // prevailing wind adds a directed breeze; a lightning strike
        // briefly adds a bright flash to ambient + sky. The state model
        // lives in world::weather (pure, render-agnostic); we apply its
        // output to the render-side AtmosphereParams here. The bin's
        // atmosphere computation is untouched. Off unless
        // UNENDLICH_WEATHER=1, in which case this is byte-identical to
        // passing `*atmos` straight through. `monotonic_frame` is the
        // deterministic time base, so launcher replays reproduce
        // identical weather.
        let atmos_eff = {
            let mut a = *atmos;
            if crate::world::weather::weather_enabled() {
                let w = crate::world::weather::sample(
                    monotonic_frame as f64, a.humidity, a.temperature);
                let liquid = w.liquid_rain();  // rain: darkens
                let snow = w.snow_amount();     // snow: brightens / whitens

                // Rain darkens sky/sun/ambient and thickens fog — an
                // overcast wet gloom.
                if liquid > 0.0 {
                    a.fog_density *= 1.0 + liquid * 3.0;
                    let sky_mul = 1.0 - liquid * 0.45;
                    let sun_mul = 1.0 - liquid * 0.50;
                    let amb_mul = 1.0 - liquid * 0.30;
                    for c in a.sky_color.iter_mut()     { *c *= sky_mul; }
                    for c in a.sun_color.iter_mut()     { *c *= sun_mul; }
                    for c in a.ambient_color.iter_mut() { *c *= amb_mul; }
                }

                // Snow brightens instead: a white-out. Sky lerps toward a
                // bright neutral grey-white, ambient lifts (snow scatters
                // light), fog thickens (reduced visibility), sun softens.
                if snow > 0.0 {
                    a.fog_density *= 1.0 + snow * 2.5;
                    let whiteout = 0.85;
                    let k = (snow * 0.6).clamp(0.0, 1.0);
                    for c in a.sky_color.iter_mut() {
                        *c += (whiteout - *c) * k;        // lerp toward white-grey
                    }
                    for c in a.ambient_color.iter_mut() {
                        *c = (*c + snow * 0.25).min(2.0); // brighter ambient
                    }
                    for c in a.sun_color.iter_mut() {
                        *c *= 1.0 - snow * 0.20;          // diffuse/soften sun
                    }
                }

                // Overcast greys the sky on dry cloudy days. Only the
                // portion of the cloud cover NOT already explained by rain
                // is applied here, so a downpour (handled above) is not
                // double-darkened — `overcast_dry` is ~0 when raining.
                let overcast_dry = (w.cloud_cover - w.precipitation).max(0.0);
                if overcast_dry > 0.0 {
                    let grey = 0.55;
                    let k = overcast_dry * 0.5;
                    for c in a.sky_color.iter_mut() {
                        *c += (grey - *c) * k;             // flatten toward grey
                    }
                    let sun_mul = 1.0 - overcast_dry * 0.35; // diffuse the sun
                    for c in a.sun_color.iter_mut() { *c *= sun_mul; }
                }

                // r157: the debug panel's "disable fog" toggle finally has
                // a consumer — zero the density before upload (exact off).
                if crate::debug_settings::DEBUG
                    .disable_fog
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    a.fog_density = 0.0;
                }
                // Fog/mist cuts visibility independently of precipitation.
                if w.fog > 0.0 {
                    a.fog_density *= 1.0 + w.fog * 2.0;
                }

                // Wind: the prevailing weather wind adds a directed breeze
                // on top of whatever the bin set, and lifts the shimmer
                // magnitude. `wind_speed` already rises during storms, so
                // this supersedes the old precipitation-only shimmer.
                a.wind_xz[0] += w.wind_dir.cos() * w.wind_speed * 0.5;
                a.wind_xz[1] += w.wind_dir.sin() * w.wind_speed * 0.5;
                a.wind_magnitude += w.wind_speed * 0.5;

                // Lightning flash adds a bright pulse to ambient + sky
                // (gated to rain in the state model — snow won't flash).
                let f = w.lightning_flash;
                if f > 0.0 {
                    for c in a.ambient_color.iter_mut() { *c = (*c + f * 0.8).min(2.0); }
                    for c in a.sky_color.iter_mut()     { *c = (*c + f * 0.6).min(2.0); }
                }
            }
            a
        };

        let ubo = CameraUbo {
            view_proj:     mat4_to_array(&vp),
            // r354: camera_pos.w carries the block-texture gate (UNENDLICH_TEXTURES).
            // .w is unused by every shader (they read only .xyz/.y); chosen over
            // wind_params to avoid the wind system entirely.
            camera_pos:    [pos.x, pos.y, pos.z,
                if crate::render::mesher::textures_enabled() { 1.0 } else { 0.0 }],
            // Use the SAME normalized sun direction the cascade
            // computation uses. Without this, lighting and shadow
            // sampling drift apart slightly when atmos.sun_dir is
            // not exactly unit length, producing subtle directional
            // bias in fog/shadow alignment.
            sun_dir:       [
                sun_dir_norm.x, sun_dir_norm.y, sun_dir_norm.z,
                atmos.sun_intensity,
            ],
            sun_color:     [atmos_eff.sun_color[0], atmos_eff.sun_color[1], atmos_eff.sun_color[2],
                // r592: sun_color.a is unused by shading — repurposed to carry the live
                // foliage-disturber strength (F3 slider) to chunk.vert. ×10 fixed-point.
                crate::debug_settings::DEBUG.foliage_disturb.load(std::sync::atomic::Ordering::Relaxed) as f32 / 10.0],
            sky_color:     [atmos_eff.sky_color[0], atmos_eff.sky_color[1], atmos_eff.sky_color[2],
                // r358: w carries the snow-cap line altitude (0 = feature off).
                // Unread by the shader as a color (only .rgb is sampled).
                if crate::world::climate_surface::snow_caps_enabled() {
                    crate::world::climate_surface::DEFAULT_SNOW_LINE
                } else { 0.0 }],
            ambient_color: [
                atmos_eff.ambient_color[0], atmos_eff.ambient_color[1], atmos_eff.ambient_color[2], 1.0,
            ],
            atmos_params:  [
                atmos_eff.fog_density, atmos.humidity, atmos.temperature, atmos.time_of_day,
            ],
            wind_params:   [
                // r637: floor the wind magnitude the shader sees so foliage always
                // has a gentle ambient sway (chunk.vert sway ∝ wind²); in calm weather
                // atmos.wind_magnitude is ~0, which made plants look dead. Weather still
                // drives stronger gusts above this baseline.
                atmos.wind_xz[0], atmos.wind_xz[1], atmos.wind_magnitude.max(0.35), atmos.wave_speed,
            ],
            // Viewport is the full swapchain for now. When RenderView
            // lands and split-screen / multi-cam is supported, each
            // RenderView will provide its own (size_x, size_y, off_x, off_y)
            // to drive shader screen-space effects per viewport.
            viewport_rect: [
                extent.width as f32, extent.height as f32,
                0.0, 0.0,
            ],
            light_view_proj: mat4_to_array(&cascade.view_proj),
            flow_coverage: self.water_flow_coverage,
            flow_grid: *self.water_flow_grid,
            fx_params: {
                let mut f = self.fx_params;
                f[2] = self.world_season01; // r499: season for foliage tint
                f
            },
            water_depth_cov: self.water_depth_cov,
            water_depth_grid: *self.water_depth_grid,
        };
        unsafe {
            std::ptr::copy_nonoverlapping(
                &ubo as *const _ as *const u8,
                frame.ubo_ptr,
                std::mem::size_of::<CameraUbo>(),
            );
        }

        let acquire = unsafe {
            ctx.swapchain_loader.acquire_next_image(
                self.swapchain.handle, u64::MAX,
                frame.image_available, vk::Fence::null(),
            )
        };
        let image_index = match acquire {
            Ok((idx, _)) => idx,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        self.cpu_timer.mark(crate::render::cpu_timer::CpuZone::AcquireImage);

        unsafe { ctx.device.reset_fences(std::slice::from_ref(&frame.in_flight))?; }

        let cmd = frame.command_buffer;
        unsafe {
            ctx.device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            ctx.device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::builder().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;

            // v0.21.0: GPU timestamp — frame start. Resets the
            // pool slice for this frame and writes Marker::FrameStart.
            self.gpu_timer.begin_frame(cmd, frame_idx as u32);

            // c.13-instrumentation: also reset the shadow bucket
            // timer's slice if active. Must happen before any bucket
            // boundary timestamps fire.
            if let Some(t) = self.shadow_bucket_timer.as_ref() {
                t.reset_frame(cmd, frame_idx as u32);
            }

            // v0.24.71-stage3a: when DEVICE_LOCAL mesh backing is on
            // (UNENDLICH_DEVICE_LOCAL_MESH=1), all per-chunk write_at
            // calls have been writing into HOST_VISIBLE staging buffers
            // and queuing vk::BufferCopy regions. Drain those now:
            //   - Records vkCmdCopyBuffer for any pending vertex copies
            //   - Records vkCmdCopyBuffer for any pending index copies
            //   - Records a TRANSFER→VERTEX_INPUT barrier so subsequent
            //     vertex/index fetches see the copied data coherently
            //
            // MUST happen before the first cmd_bind_vertex_buffers /
            // cmd_bind_index_buffer call in this command buffer (those
            // start firing inside the shadow render pass below). When
            // DEVICE_LOCAL mode is OFF, this is a no-op (no staging,
            // nothing to flush).
            //
            // No early-return: the call is idempotent and cheap when
            // pending lists are empty.
            self.mesh_pool.flush_pending_copies(&ctx.device, cmd);

            // v0.24.0b: GPU view-derivation. CPU writes (view, proj)
            // matrices for the main camera slot; GPU compute derives
            // view_proj / inverse_view_proj / frustum planes back into
            // the same slot.
            //
            // v0.24.2b: shadow cascade C0's (view, proj) is now
            // GPU-derived too — via `cascade_fit.comp` consuming the
            // camera basis and sun direction (see prepare_cascades).
            // The CPU `cascade` value below still feeds the UBO's
            // `light_view_proj` field (shadow vertex shader + main
            // fragment shader read that), but the cull's frustum
            // planes come from the GPU cascade-fit + view-compute
            // chain. The two must agree, and the
            // `cpu_reference_matches_single_cascade_for_typical_camera`
            // test pins that agreement.
            if self.use_indirect {
                // Main view: CPU-authored matrices.
                let main_slots = [(
                    crate::render::view_params::ViewGroup::PRIMARY.main_slot,
                    view_mat,
                    proj_mat,
                )];
                self.prepare_views(ctx, cmd, &main_slots);

                // Cascades: GPU-derived. v0.24.2c renders into all
                // 4 cascade layers using the same `cascade_splits`
                // computed above (single source of truth so the CPU
                // matrix used for push-constant rasterization and the
                // GPU matrix used for cull frustum are computed from
                // identical inputs).
                let cam_forward = view.forward();
                let cam_right = view.right();
                // Same up-axis derivation as `frustum_corners_world`
                // in `light.rs` so the GPU cascade math sees the same
                // basis the CPU reference does.
                let cam_up = glm::cross(&cam_right, &cam_forward).normalize();
                self.prepare_cascades(
                    ctx, cmd,
                    &crate::render::view_params::ViewGroup::PRIMARY,
                    view.position, cam_forward, cam_up, cam_right,
                    view.fov_y_radians, aspect,
                    sun_dir_norm,
                    &cascade_splits,
                    // v0.24.4: read the actual shadow map extent, which
                    // is set at startup from `runtime_shadow_map_size`.
                    // Using the field (not the default const) keeps the
                    // cascade-fit math in sync with the rendered extent.
                    self.shadow.extent.width as f32,
                );
            }

            // ─── Shadow pre-pass ────────────────────────────────────
            //
            // Render every loaded chunk's mesh into the offscreen
            // shadow map using the shadow pipeline (depth-only,
            // light-space VP). The render pass's subpass dependency
            // handles the depth-write → main-pass-sample barrier.
            //
            // No frustum culling here yet: the LIGHT's frustum and the
            // CAMERA's frustum are different. A chunk that's outside
            // the camera frustum can still cast a shadow into the
            // visible region. Phase 2 will add a proper light-space
            // frustum cull. For now we just submit everything; cost is
            // bounded because the depth-only pipeline has no fragment
            // shader work.
            //
            // We bind the SAME descriptor set as the main pass — the
            // shadow vertex shader reads `light_view_proj` from the
            // shared UBO. The descriptor-set-layout-compatibility rule
            // (Vulkan: layouts are compatible if their bindings match)
            // makes this safe between the two pipelines.
            // Shadow pre-pass — populate cascade 0's depth texture from
            // the sun's POV.
            //
            // v0.20: GPU-driven culling. We dispatch the cull compute
            // shader against the cascade's frustum BEFORE the render
            // pass, then issue ONE `cmd_draw_indexed_indirect_count`
            // inside the pass.
            // v0.24.NEXT shadow-cache: reuse last frame's shadow map when the
            // cascade view-projection is unchanged (sun + camera static). The
            // shadow image persists in SHADER_READ_ONLY (render-pass subpass
            // dependency), so the main pass samples valid shadows without a
            // re-render. The ShadowEnd timestamp below is written
            // unconditionally, so the GPU timer stays aligned (shadow ≈ 0ms
            // on a reused frame).
            let skip_shadow = self.shadow_cache
                && !force_full_shadow
                && monotonic_frame > self.shadow_chunk_stride as u64
                && self.last_shadow_rendered_vp == Some(cascade.view_proj);
            if !skip_shadow {
                let shadow_frustum = Frustum::from_view_projection(&cascade.view_proj);

                // v0.24.2c.6: render ONLY cascade 0 into the legacy
                // `framebuffer` (which wraps the layer-0 view created
                // at startup, the same way pre-v0.24.2c). This is
                // the configuration the c.5 bisect confirmed runs
                // dramatically faster on Honeykrisp than any path
                // using the new `layer_framebuffers[i]`. Specifically:
                //
                //   c.5 bisect at RD=8, vsync=on, indirect=on (15s runs):
                //     CASCADES=1, LEGACY_FB=1: shadow p50=25.42ms  ← this path
                //     CASCADES=1, LEGACY_FB=0: shadow p50=70.68ms  (2.8× slower)
                //     CASCADES=4, LEGACY_FB=0: shadow p50=217.19ms (linear scaling)
                //
                // The driver-side explanation isn't fully understood,
                // but creating fresh VkImageView+VkFramebuffer objects
                // per-layer is far more expensive on this driver than
                // reusing the single image-view+framebuffer pair we
                // already had pre-v0.24.2c. Possibly tile-binner state
                // re-emit per-framebuffer-handle; possibly something
                // else entirely. Filed for investigation.
                //
                // What CSM-ready-ness this preserves:
                //  - The 4-cascade CPU math (computing `cascades[0..4]`).
                //  - The GPU cascade_fit and view_compute pipelines
                //    populate ViewParams slots for all 4 cascades.
                //  - The shadow image has 4 array layers and per-layer
                //    image views.
                //  - The shadow vertex shader's push-constant route
                //    reads its matrix from a per-call push, not the UBO.
                //
                // What's deferred:
                //  - Actually rendering into cascades 1/2/3.
                //  - Sampling those cascades from the main fragment
                //    shader.
                //
                // The path forward for CSM is probably multiview
                // (single render pass writing to multiple array
                // layers in one go), which sidesteps the
                // per-framebuffer overhead on this driver.
                let view_group = crate::render::view_params::ViewGroup::PRIMARY;

                // Cull against C0's view-slot (= cascade_slots[0]).
                if self.use_indirect {
                    self.record_cull_pass(
                        ctx, cmd,
                        view_group.cascade_slots[0],
                        // v0.24.4: shadow temporal-stride.
                        shadow_cull_stride,
                        shadow_cull_phase,
                    );
                }

                // v0.24.4 (shadow-stride): pick render pass + framebuffer
                // based on whether we're forcing a full render. CLEAR
                // path resets depth to 1.0 (everything-out-of-shadow).
                // LOAD path preserves previous frames' depth, so chunks
                // not rendered this frame keep their old contribution.
                let shadow_clears = [
                    vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 } },
                ];
                let (shadow_rp, shadow_fb) = if force_full_shadow {
                    (self.shadow.render_pass, self.shadow.framebuffer)
                } else {
                    (self.shadow.render_pass_load, self.shadow.framebuffer_load)
                };
                let shadow_rp_begin = vk::RenderPassBeginInfo::builder()
                    .render_pass(shadow_rp)
                    .framebuffer(shadow_fb)
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: self.shadow.extent,
                    })
                    .clear_values(&shadow_clears);
                ctx.device.cmd_begin_render_pass(cmd, &shadow_rp_begin, vk::SubpassContents::INLINE);
                ctx.device.cmd_bind_pipeline(
                    cmd, vk::PipelineBindPoint::GRAPHICS, self.shadow_pipeline.pipeline,
                );

                let shadow_viewport = vk::Viewport {
                    x: 0.0, y: 0.0,
                    width: self.shadow.extent.width as f32,
                    height: self.shadow.extent.height as f32,
                    min_depth: 0.0, max_depth: 1.0,
                };
                let shadow_scissor = vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.shadow.extent,
                };
                ctx.device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&shadow_viewport));
                ctx.device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&shadow_scissor));

                ctx.device.cmd_bind_descriptor_sets(
                    cmd, vk::PipelineBindPoint::GRAPHICS,
                    self.shadow_pipeline.layout,
                    0, std::slice::from_ref(&frame.descriptor_set), &[],
                );

                // Push C0's view-projection. chunk_shadow.vert reads
                // `pc.cascade_view_proj` and transforms vertices into
                // light-clip-space using it.
                let cascade_vp = mat4_to_array(&cascades[0].view_proj);
                let cascade_vp_bytes = std::slice::from_raw_parts(
                    cascade_vp.as_ptr() as *const u8,
                    std::mem::size_of_val(&cascade_vp),
                );
                ctx.device.cmd_push_constants(
                    cmd, self.shadow_pipeline.layout,
                    vk::ShaderStageFlags::VERTEX, 0,
                    cascade_vp_bytes,
                );

                if shadows_disabled {
                    // r157: cleared map, no draws — fully lit scene.
                } else if self.use_indirect {
                    // v0.20: GPU-driven indirect draw — one call.
                    ctx.device.cmd_bind_vertex_buffers(
                        cmd, 0, &[self.mesh_pool.vertex_buffer], &[0]);
                    ctx.device.cmd_bind_index_buffer(
                        cmd, self.mesh_pool.index_buffer, 0, vk::IndexType::UINT32);
                    ctx.device.cmd_draw_indexed_indirect_count(
                        cmd,
                        self.indirect.commands_buffer, 0,
                        self.indirect.count_buffer, 0,
                        self.indirect.capacity(),
                        std::mem::size_of::<vk::DrawIndexedIndirectCommand>() as u32,
                    );
                } else {
                    // v0.19 fallback: per-chunk loop with CPU-side cull
                    // against C0's frustum.
                    //
                    // c.13-instrumentation: if a shadow bucket timer
                    // is active, we collect the visible chunk list
                    // FIRST so we can compute the per-bucket chunk
                    // boundaries before drawing. This lets us emit
                    // a timestamp every chunks_per_bucket draws and
                    // get per-bucket GPU timing.
                    // T7-bonus B2: the per-frame visible-mesh Vec is only needed by the
                    // shadow BUCKET timer (a diagnostic, normally OFF) to compute per-bucket
                    // draw boundaries. On the normal path, iterate + draw live with no
                    // per-frame heap allocation.
                    let bucket_active = self.shadow_bucket_timer
                        .as_ref()
                        .map(|t| t.supported())
                        .unwrap_or(false);

                    // The vertex/index buffers are identical for every chunk in this
                    // fallback loop (always self.mesh_pool.vertex/index_buffer at offset 0),
                    // so bind ONCE rather than per chunk. Per-draw state (first_index, vertex
                    // base, instance) still varies inside the draw below.
                    ctx.device.cmd_bind_vertex_buffers(
                        cmd, 0, &[self.mesh_pool.vertex_buffer], &[0]);
                    ctx.device.cmd_bind_index_buffer(
                        cmd, self.mesh_pool.index_buffer, 0, vk::IndexType::UINT32);

                    let visible = |key: &crate::world::lod::MeshSlotKey, m: &UploadedMesh| -> bool {
                        let coord = key.coord;
                        if coord.x == i32::MIN && coord.y == i32::MIN && coord.z == i32::MIN {
                            return false; // debug overlay sentinel
                        }
                        let (ox, _oy, oz) = coord.block_origin();
                        let aabb_min = glm::vec3(ox as f32, m.min_y, oz as f32);
                        let aabb_max = glm::vec3(
                            ox as f32 + CHUNK_SIZE as f32, m.max_y, oz as f32 + CHUNK_SIZE as f32,
                        );
                        shadow_frustum.aabb_visible(&aabb_min, &aabb_max)
                    };
                    let draw1 = |m: &UploadedMesh| {
                        let first_index = (m.slot.idx_offset
                            / std::mem::size_of::<u32>() as u64) as u32;
                        let vert_base_count = (m.slot.vert_base_aligned
                            / std::mem::size_of::<PackedVertex>() as u64) as i32;
                        ctx.device.cmd_draw_indexed(
                            cmd, m.index_count, 1, first_index, vert_base_count, m.draw_slot.0);
                    };

                    if !bucket_active {
                        // Fast path: no allocation, no bucket bookkeeping.
                        for (key, m) in self.meshes.iter() {
                            if visible(key, m) { draw1(m); }
                        }
                    } else {
                        // Diagnostic path: collect so we can compute bucket boundaries.
                        let visible_meshes: Vec<_> = self.meshes.iter()
                            .filter(|(key, m)| visible(*key, *m))
                            .collect();
                        let bucket_count = self.shadow_bucket_timer
                            .as_ref().unwrap().bucket_count() as usize;
                        // Ceiling division so a partial final bucket still holds the trailing chunks.
                        let chunks_per_bucket = if !visible_meshes.is_empty() {
                            (visible_meshes.len() + bucket_count - 1) / bucket_count
                        } else {
                            usize::MAX
                        };
                        // Boundary 0: before any chunks draw.
                        if let Some(t) = self.shadow_bucket_timer.as_ref() {
                            t.mark_boundary(cmd, frame_idx as u32, 0);
                        }
                        let mut last_boundary_emitted: usize = 0;
                        for (drawn_idx, (_key, m)) in visible_meshes.iter().enumerate() {
                            draw1(m);
                            // Boundary after every chunks_per_bucket draws (1..=bucket_count).
                            if (drawn_idx + 1) % chunks_per_bucket == 0 {
                                last_boundary_emitted += 1;
                                if last_boundary_emitted as u32 <= bucket_count as u32 {
                                    if let Some(t) = self.shadow_bucket_timer.as_ref() {
                                        t.mark_boundary(cmd, frame_idx as u32,
                                            last_boundary_emitted as u32);
                                    }
                                }
                            }
                        }
                        // Final boundaries: reach boundary_count even on uneven division.
                        while last_boundary_emitted < bucket_count {
                            last_boundary_emitted += 1;
                            if let Some(t) = self.shadow_bucket_timer.as_ref() {
                                t.mark_boundary(cmd, frame_idx as u32,
                                    last_boundary_emitted as u32);
                            }
                        }
                        unendlich_shared::events::record("shadow_buckets_record", serde_json::json!({
                            "frame": frame_idx,
                            "visible_chunks": visible_meshes.len(),
                            "bucket_count": bucket_count,
                            "chunks_per_bucket": chunks_per_bucket.min(visible_meshes.len()),
                        }));
                    }
                }

                ctx.device.cmd_end_render_pass(cmd);
                // Remember the cascade we just rendered so an identical next
                // frame can reuse this shadow map.
                self.last_shadow_rendered_vp = Some(cascade.view_proj);
            }
            // End shadow pre-pass. The render-pass's subpass dependency
            // already inserted the depth-write → shader-read barrier
            // for when the main pass samples this image (Phase 2).

            // v0.21.0: GPU timestamp — shadow pass complete.
            self.gpu_timer.mark(cmd, frame_idx as u32, crate::render::gpu_timer::Marker::ShadowEnd);

            // ─── Froxel fill compute pass ───────────────────────────
            //
            // Run AFTER the shadow pre-pass (so we can read the
            // current frame's shadow map) and BEFORE the main color
            // pass (so the main pass's fragment shader can sample
            // the just-written volume).
            //
            // The shadow render-pass's external-output subpass
            // dependency already transitions shadow image to
            // DEPTH_STENCIL_READ_ONLY_OPTIMAL and inserts the
            // depth-write → shader-read barrier — covers our compute
            // sampling.
            //
            // The froxel volume needs: storage-image-write barrier
            // BEFORE compute (so prior frame's reads from the volume
            // by the main pass don't race), then storage-image-write
            // → shader-sampled-read barrier AFTER compute (so the
            // main pass sees the new contents).
            {
                // Pre-compute barrier: previous frame's main-pass
                // reads → this frame's compute writes. Since the
                // image is in GENERAL layout permanently, only an
                // execution + memory barrier is needed.
                let pre_barrier = vk::ImageMemoryBarrier::builder()
                    .src_access_mask(vk::AccessFlags::SHADER_READ)
                    .dst_access_mask(vk::AccessFlags::SHADER_WRITE)
                    .old_layout(vk::ImageLayout::GENERAL)
                    .new_layout(vk::ImageLayout::GENERAL)
                    .image(self.froxel.image)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0, level_count: 1,
                        base_array_layer: 0, layer_count: 1,
                    })
                    .build();
                ctx.device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    std::slice::from_ref(&pre_barrier),
                );

                // Bind compute pipeline + descriptor set.
                ctx.device.cmd_bind_pipeline(
                    cmd, vk::PipelineBindPoint::COMPUTE,
                    self.froxel_fill_pipeline.pipeline,
                );
                ctx.device.cmd_bind_descriptor_sets(
                    cmd, vk::PipelineBindPoint::COMPUTE,
                    self.froxel_fill_pipeline.layout,
                    0,
                    std::slice::from_ref(&self.froxel_compute_descriptor_sets[frame_idx]),
                    &[],
                );

                // Push constants.
                let inv_vp = vp.try_inverse().unwrap_or(glm::Mat4::identity());
                let push = FroxelFillPushConstants {
                    inv_view_proj: mat4_to_array(&inv_vp),
                    cam_near_far: [view.near, view.far, 0.0, 0.0],
                    froxel_extent_packed: [
                        FROXEL_X as f32, FROXEL_Y as f32, FROXEL_Z as f32, 0.0,
                    ],
                    froxel_near_far: [FROXEL_NEAR_M, FROXEL_FAR_M, 0.0, 0.0],
                };
                let push_bytes = std::slice::from_raw_parts(
                    &push as *const _ as *const u8,
                    std::mem::size_of::<FroxelFillPushConstants>(),
                );
                ctx.device.cmd_push_constants(
                    cmd,
                    self.froxel_fill_pipeline.layout,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    push_bytes,
                );

                // Dispatch: workgroup size 8×8×1, volume 160×90 in
                // XY → (20, 12, 1) workgroups. Each invocation is
                // one (x, y) pixel column and walks ALL Z slices
                // serially, accumulating cumulative front-to-back
                // integration. Total = 14,400 invocations, each
                // doing 64 Z steps of work — same total work as the
                // old 921,600-invocation version, with the benefit
                // that the fragment shader can sample ONCE instead
                // of compositing 6+ samples per pixel.
                let groups_x = (FROXEL_X + 7) / 8;
                let groups_y = (FROXEL_Y + 7) / 8;
                ctx.device.cmd_dispatch(cmd, groups_x, groups_y, 1);

                // Post-compute barrier: compute writes → main pass
                // fragment shader reads.
                let post_barrier = vk::ImageMemoryBarrier::builder()
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)
                    .old_layout(vk::ImageLayout::GENERAL)
                    .new_layout(vk::ImageLayout::GENERAL)
                    .image(self.froxel.image)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0, level_count: 1,
                        base_array_layer: 0, layer_count: 1,
                    })
                    .build();
                ctx.device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    std::slice::from_ref(&post_barrier),
                );
            }
            // End froxel fill compute pass.

            // v0.21.1: GPU timestamp — froxel fill complete (post-barrier
            // emitted, fragment shader can sample volume).
            self.gpu_timer.mark(cmd, frame_idx as u32, crate::render::gpu_timer::Marker::FroxelEnd);

            // v0.24.NEXT+5-weather: when weather is active, the background
            // clear (the visible sky behind geometry) tracks the same
            // weather-modulated sky color the UBO uses, so the horizon
            // darkens in rain and flashes with lightning instead of staying
            // a static bright blue. When weather is off, `atmos_eff` equals
            // the unmodulated atmosphere, so this is the prior clear color
            // (modulo the base atmosphere's sky vs. the const default —
            // matched closely: defaults are [0.55,0.75,0.95] vs sky
            // [0.55,0.75,1.0]).
            let clear_rgba = if crate::world::weather::weather_enabled() {
                [
                    atmos_eff.sky_color[0],
                    atmos_eff.sky_color[1],
                    atmos_eff.sky_color[2],
                    1.0,
                ]
            } else {
                self.clear_color
            };
            let clears = [
                vk::ClearValue { color: vk::ClearColorValue { float32: clear_rgba } },
                vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 } },
            ];

            let rp_begin = vk::RenderPassBeginInfo::builder()
                .render_pass(self.render_pass)
                .framebuffer(self.swapchain.framebuffers[image_index as usize])
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.swapchain.extent,
                })
                .clear_values(&clears);

            // v0.20: GPU cull for the camera frustum (only in indirect mode).
            // v0.24.0b: planes for VIEW_SLOT_MAIN were derived by
            // prepare_views earlier this frame.
            if self.use_indirect {
                self.record_cull_pass(
                    ctx, cmd,
                    crate::render::view_params::VIEW_SLOT_MAIN,
                    // Main cull never strides — always admit all visible chunks.
                    1, 0,
                );
            }

            // v0.24.4 chug-debug: CPU-side count of how many chunks pass
            // each cull's frustum test, emitted as a side-channel event.
            //
            // v0.24.72: gated behind UNENDLICH_CULL_DIAG=1 (default OFF).
            // The investigation that motivated this loop has concluded;
            // it ran every frame for many sessions doing ~554 frustum
            // tests at p50=277 rendered chunks. Saves ~10µs/frame when
            // off. Set the env to revive it for new investigations.
            //
            // This is independent of the GPU cull (which actually drives
            // rendering); we recompute the same frustum-AABB tests on CPU
            // here to count for diagnostic purposes. The numbers should
            // match the GPU's atomic-counter output to within floating-
            // point noise — what we care about is the RATIO of shadow:main.
            //
            // Hypothesis: shadow's sun-aligned ortho frustum passes more
            // chunks than the camera's perspective frustum, and shadow
            // GPU time is vertex/draw-bound (not fragment-bound — proven
            // false in v0.24.3-shadow-size). If shadow_count >> main_count,
            // the right fix is shadow-specific LOD or cull tightening.
            //
            // To avoid event-log spam, only emit when the ratio is
            // interesting (shadow > 1.5× main). Steady-state non-spike
            // frames where the ratio is ~1 don't pollute the log.
            let cull_diag_enabled = std::env::var("UNENDLICH_CULL_DIAG")
                .ok().as_deref() == Some("1");
            if cull_diag_enabled {
            if let Some(cascade) = self.last_cascade {
                let shadow_frustum_count = Frustum::from_view_projection(&cascade.view_proj);
                let mut shadow_count: u32 = 0;
                let mut main_count: u32 = 0;
                let mut total_count: u32 = 0;
                for (key, m) in self.meshes.iter() {
                    let coord = key.coord;
                    let is_debug_overlay = coord.x == i32::MIN
                        && coord.y == i32::MIN
                        && coord.z == i32::MIN;
                    if is_debug_overlay { continue; }
                    total_count += 1;
                    let (ox, _oy, oz) = coord.block_origin();
                    let aabb_min = glm::vec3(ox as f32, m.min_y, oz as f32);
                    let aabb_max = glm::vec3(
                        ox as f32 + CHUNK_SIZE as f32,
                        m.max_y,
                        oz as f32 + CHUNK_SIZE as f32,
                    );
                    if shadow_frustum_count.aabb_visible(&aabb_min, &aabb_max) {
                        shadow_count += 1;
                    }
                    if frustum.aabb_visible(&aabb_min, &aabb_max) {
                        main_count += 1;
                    }
                }
                // Emit only when ratio is interesting (shadow > 1.5× main)
                // OR when total chunk count is large enough that shadow
                // load is the dominant cost regardless of ratio.
                let shadow_main_ratio = if main_count > 0 {
                    shadow_count as f32 / main_count as f32
                } else {
                    1.0
                };
                if shadow_main_ratio > 1.5 || shadow_count > 50 {
                    unendlich_shared::events::record(
                        "cull_count",
                        serde_json::json!({
                            "frame_idx": frame_idx,
                            "shadow_count": shadow_count,
                            "main_count": main_count,
                            "total_count": total_count,
                            "ratio": shadow_main_ratio,
                        }),
                    );
                }
            }
            } // end if cull_diag_enabled

            // v0.21.0: GPU timestamp — main-pass cull dispatch complete
            // (or no-op if !use_indirect, which gives us a near-zero
            // "cull" zone in fallback mode — useful for confirming the
            // toggle is working from the log).
            self.gpu_timer.mark(cmd, frame_idx as u32, crate::render::gpu_timer::Marker::MainCullEnd);

            // r306: gated parallel G-buffer pass (neural deferred producer).
            // Re-draws the visible chunks through chunk_gbuffer.frag into the
            // G-buffer images, reusing the SAME cull output (commands/count
            // buffers, ready after the dispatch above), the SAME per-frame
            // descriptor set, and the SAME vertex/index buffers as the main pass —
            // only the pipeline (chunk_gbuffer.frag, 3 attachments) and the render
            // pass/framebuffer differ. Inert producer: nothing samples the result
            // yet. Indirect path only; the non-indirect fallback skips it.
            if let Some(gb) = &self.gbuffer {
                if self.use_indirect {
                    let gb_clears = [
                        vk::ClearValue { color: vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 0.0] } },
                        vk::ClearValue { color: vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 0.0] } },
                        vk::ClearValue { color: vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 0.0] } },
                        vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 } },
                    ];
                    let gb_rp_begin = vk::RenderPassBeginInfo::builder()
                        .render_pass(gb.render_pass)
                        .framebuffer(gb.framebuffer)
                        .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: gb.extent })
                        .clear_values(&gb_clears);
                    ctx.device.cmd_begin_render_pass(cmd, &gb_rp_begin, vk::SubpassContents::INLINE);
                    ctx.device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, gb.pipeline);
                    let gb_vp = vk::Viewport {
                        x: 0.0, y: 0.0,
                        width: gb.extent.width as f32, height: gb.extent.height as f32,
                        min_depth: 0.0, max_depth: 1.0,
                    };
                    let gb_sc = vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: gb.extent };
                    ctx.device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&gb_vp));
                    ctx.device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&gb_sc));
                    ctx.device.cmd_bind_descriptor_sets(
                        cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline.layout,
                        0, std::slice::from_ref(&frame.descriptor_set), &[],
                    );
                    ctx.device.cmd_bind_vertex_buffers(cmd, 0, &[self.mesh_pool.vertex_buffer], &[0]);
                    ctx.device.cmd_bind_index_buffer(cmd, self.mesh_pool.index_buffer, 0, vk::IndexType::UINT32);
                    ctx.device.cmd_draw_indexed_indirect_count(
                        cmd,
                        self.indirect.commands_buffer, 0,
                        self.indirect.count_buffer, 0,
                        self.indirect.capacity(),
                        std::mem::size_of::<vk::DrawIndexedIndirectCommand>() as u32,
                    );
                    ctx.device.cmd_end_render_pass(cmd);

                    // r310: feature pass — sample the G-buffer + march the octree for
                    // the 6 hint rays. Rebuild the per-chunk base grid centered on the
                    // player each frame (chunk coord → root index, EMPTY for absent),
                    // snapshot the octree node buffer, then dispatch. sun_dir / y_scale
                    // are PLACEHOLDERS — wire to the real sun and retrain (B2). The
                    // G-buffer render pass already released its attachments to COMPUTE
                    // SHADER_READ, so no extra barrier is needed.
                    if let Some(nf) = &self.neural_feat {
                        let dims = crate::render::neural_feat_pipeline::GRID_DIMS;
                        let pcx = (view.position.x / 32.0).floor() as i32;
                        let pcy = (view.position.y / 32.0).floor() as i32;
                        let pcz = (view.position.z / 32.0).floor() as i32;
                        let origin = [
                            pcx - (dims[0] / 2) as i32,
                            pcy - (dims[1] / 2) as i32,
                            pcz - (dims[2] / 2) as i32,
                        ];
                        let mut grid = vec![u32::MAX; nf.grid_len()];
                        let octree_buf = {
                            let pool = self
                                .octree_pool
                                .lock()
                                .expect("octree_pool mutex poisoned (neural feat)");
                            pool.fill_base_grid(origin, dims, &mut grid);
                            pool.buffer
                        };
                        nf.upload_base_grid(&grid);
                        nf.record(
                            &ctx.device, cmd, gb, octree_buf,
                            [0.282, 0.940, 0.188], 1.0 / 256.0, origin,
                        );
                    }
                }
            }

            ctx.device.cmd_begin_render_pass(cmd, &rp_begin, vk::SubpassContents::INLINE);
            ctx.device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline.current());

            let viewport = vk::Viewport {
                x: 0.0, y: 0.0,
                width: self.swapchain.extent.width as f32,
                height: self.swapchain.extent.height as f32,
                min_depth: 0.0, max_depth: 1.0,
            };
            let scissor = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.swapchain.extent,
            };
            ctx.device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&viewport));
            ctx.device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&scissor));

            ctx.device.cmd_bind_descriptor_sets(
                cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline.layout,
                0, std::slice::from_ref(&frame.descriptor_set), &[],
            );

            if self.use_indirect {
                ctx.device.cmd_bind_vertex_buffers(
                    cmd, 0, &[self.mesh_pool.vertex_buffer], &[0]);
                ctx.device.cmd_bind_index_buffer(
                    cmd, self.mesh_pool.index_buffer, 0, vk::IndexType::UINT32);
                ctx.device.cmd_draw_indexed_indirect_count(
                    cmd,
                    self.indirect.commands_buffer, 0,
                    self.indirect.count_buffer, 0,
                    self.indirect.capacity(),
                    std::mem::size_of::<vk::DrawIndexedIndirectCommand>() as u32,
                );
            } else {
                // v0.19 fallback: per-chunk loop. Indices are PRE-SHIFTED
                // at upload time (a v0.20 invariant), so we still bind at
                // offset 0 and use first_index from the slot.
                // F1: iterates all stored MeshSlotKeys; in F1 only
                // LOD::Near entries exist so behaviour is identical.
                let mut drawn = 0u32;
                let mut culled = 0u32;
                // T9: the vertex/index buffers are identical for every chunk in
                // this fallback loop (always self.mesh_pool.vertex_buffer /
                // index_buffer bound at offset 0), so bind them ONCE here
                // instead of re-binding the same handles on every iteration.
                // Mirrors the already-hoisted shadow non-indirect fallback
                // above. Bound-buffer state persists across draws in Vulkan, so
                // output is identical; only the redundant per-chunk bind calls
                // go away. Per-draw state (first_index, vertex base, instance)
                // still varies inside the loop on cmd_draw_indexed below.
                ctx.device.cmd_bind_vertex_buffers(
                    cmd, 0, &[self.mesh_pool.vertex_buffer], &[0]);
                ctx.device.cmd_bind_index_buffer(
                    cmd, self.mesh_pool.index_buffer, 0, vk::IndexType::UINT32);
                for (key, m) in &self.meshes {
                    let coord = key.coord;
                    let is_debug_overlay = coord.x == i32::MIN
                        && coord.y == i32::MIN
                        && coord.z == i32::MIN;
                    if !is_debug_overlay {
                        let (ox, _oy, oz) = coord.block_origin();
                        let aabb_min = glm::vec3(ox as f32, m.min_y, oz as f32);
                        let aabb_max = glm::vec3(
                            ox as f32 + CHUNK_SIZE as f32,
                            m.max_y,
                            oz as f32 + CHUNK_SIZE as f32,
                        );
                        if !frustum.aabb_visible(&aabb_min, &aabb_max) {
                            culled += 1;
                            continue;
                        }
                    }
                    let first_index = (m.slot.idx_offset
                        / std::mem::size_of::<u32>() as u64) as u32;
                    // v0.24.20-vertex-pack: same fix as the shadow
                    // non-indirect fallback above — see that comment.
                    let vert_base_count = (m.slot.vert_base_aligned
                        / std::mem::size_of::<PackedVertex>() as u64) as i32;
                    ctx.device.cmd_draw_indexed(
                        cmd, m.index_count, 1, first_index,
                        vert_base_count, m.draw_slot.0);
                    drawn += 1;
                }
                log::trace!("frame: drew {drawn} chunks, culled {culled}");
            }
            let _ = &frustum;

            // Entities (mobs / dropped items): draw after the world, before the UI.
            self.entity_pipeline.record(&ctx.device, cmd, frame.descriptor_set);

            // ── In-game egui overlay: IN-PASS record. Replays the UI draw list
            // prepared above, on top of the world, still inside the main subpass.
            #[cfg(feature = "ui-overlay")]
            self.egui_overlay.record(
                ctx,
                cmd,
                [self.swapchain.extent.width, self.swapchain.extent.height],
            );

            ctx.device.cmd_end_render_pass(cmd);

            // v0.21.0: GPU timestamp — main pass complete. This is
            // the end of the timed region; presentation isn't part of
            // any zone (it's overlapped with the next frame's CPU work
            // anyway under FIFO).
            self.gpu_timer.mark(cmd, frame_idx as u32, crate::render::gpu_timer::Marker::MainEnd);

            ctx.device.end_command_buffer(cmd)?;
        }
        self.cpu_timer.mark(crate::render::cpu_timer::CpuZone::RecordCmd);

        let wait_sems = [frame.image_available];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let signal_sems = [frame.render_finished];
        let cmd_bufs = [cmd];

        let submit = vk::SubmitInfo::builder()
            .wait_semaphores(&wait_sems)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(&cmd_bufs)
            .signal_semaphores(&signal_sems);
        unsafe {
            // v0.24.21-gpu-mesher-prod: serialize against `SparseGpuMesher`
            // submits on the same queue. Lock held only across the
            // submit call itself; released before the next CPU work or
            // present.
            let _g = ctx.queue_submit_lock.lock().expect("queue submit lock poisoned");
            ctx.device.queue_submit(ctx.graphics_queue, std::slice::from_ref(&submit), frame.in_flight)?;
        }
        self.cpu_timer.mark(crate::render::cpu_timer::CpuZone::QueueSubmit);

        // v0.24.3 chug instrumentation: record when this slot was just
        // submitted. The NEXT time we wait on this slot's fence, the
        // duration since this Instant tells us whether CPU did other
        // work between submit and wait. Side-channel only; no
        // scheduling effect.
        self.frames[frame_idx].last_submit_at = Some(std::time::Instant::now());

        let swapchains = [self.swapchain.handle];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::builder()
            .wait_semaphores(&signal_sems)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        let present = unsafe { ctx.swapchain_loader.queue_present(ctx.present_queue, &present_info) };
        self.cpu_timer.mark(crate::render::cpu_timer::CpuZone::QueuePresent);
        self.cpu_timer.end_frame();

        match present {
            Ok(false) => {}
            Ok(true) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) | Err(vk::Result::SUBOPTIMAL_KHR) => {
                self.advance_frame();
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        }

        self.advance_frame();
        Ok(true)
    }

    fn advance_frame(&mut self) {
        self.current_frame = (self.current_frame + 1) % self.frames_in_flight;
    }

    pub fn recreate_swapchain(&mut self, ctx: &VulkanContext, new_size: (u32, u32)) -> Result<()> {
        unsafe { ctx.device.device_wait_idle()?; }

        self.swapchain.destroy(ctx);
        self.depth.destroy(&ctx.device);

        let extent = vk::Extent2D { width: new_size.0, height: new_size.1 };

        let temp_rp = crate::render::swapchain::create_render_pass(
            &ctx.device, vk::Format::B8G8R8A8_SRGB,
        )?;
        self.swapchain = Swapchain::new(ctx, temp_rp, extent)?;
        unsafe { ctx.device.destroy_render_pass(temp_rp, None); }

        self.depth = DepthBuffer::new(&ctx.instance, &ctx.device, ctx.physical_device, self.swapchain.extent)?;
        self.swapchain.rebuild_framebuffers_with_depth(ctx, self.render_pass, self.depth.view)?;
        self.framebuffer_size = new_size;

        // r306: the G-buffer is extent-sized — rebuild it to the new size if enabled.
        if let Some(mut gb) = self.gbuffer.take() {
            gb.destroy(&ctx.device);
            self.gbuffer = Some(crate::render::gbuffer::NeuralGBuffer::new(
                &ctx.instance, &ctx.device, ctx.physical_device,
                self.swapchain.extent, self.pipeline.layout,
            )?);
        }
        // r307: the feature buffer is extent-sized too.
        if let Some(mut nf) = self.neural_feat.take() {
            nf.destroy(&ctx.device);
            self.neural_feat = Some(crate::render::neural_feat_pipeline::NeuralFeatPipeline::new(
                &ctx.instance, &ctx.device, ctx.physical_device, self.swapchain.extent,
            )?);
        }

        Ok(())
    }

    // =========================================================================
    // v0.24.26-octree-on-gpu: GPU-resident octree substrate
    // =========================================================================

    /// Upload (or replace) a chunk's octree on the GPU. Call sites
    /// pair this with `upload_prebuilt_mesh_at_lod` in the bin's
    /// streaming-loop drain so the GPU's view of a chunk lands at
    /// the same moment as its mesh.
    ///
    /// The octree comes from `ChunkManager::octree(coord)`. Pass the
    /// `OctreeArray` by reference; the upload memcpys its bytes into
    /// the persistent map.
    ///
    /// Errors here are non-fatal at the bin level — a failed octree
    /// upload means GPU consumers (none in v0.24.26) can't read this
    /// chunk's octree but the rest of the engine continues. The bin
    /// should log and proceed.
    pub fn upload_chunk_octree(
        &mut self,
        coord: ChunkCoord,
        octree: &crate::world::octree::OctreeArray,
    ) -> std::result::Result<(), crate::render::octree_buffer::OctreePoolError> {
        self.octree_pool.lock().expect("octree_pool mutex poisoned")
            .upload(coord, octree).map(|_| ())
    }

    /// v0.24.32-pool-prewarm: upload an octree to the pool only if
    /// its content version differs from what's already resident.
    /// Used by the bin's pre-warm path to ensure neighbor chunks
    /// are in the pool BEFORE the cross-chunk mesher reads them.
    /// Calling this on a chunk that's already resident at the same
    /// version is essentially free (one HashMap lookup).
    pub fn upload_chunk_octree_versioned(
        &mut self,
        coord: ChunkCoord,
        octree: &crate::world::octree::OctreeArray,
        version: u64,
    ) -> std::result::Result<(), crate::render::octree_buffer::OctreePoolError> {
        self.octree_pool.lock().expect("octree_pool mutex poisoned")
            .upload_if_changed(coord, octree, version).map(|_| ())
    }

    /// Number of chunks currently resident in the GPU octree pool.
    /// Used by metrics + the verify path.
    pub fn octree_live_chunks(&self) -> usize {
        self.octree_pool.lock().expect("octree_pool mutex poisoned").live_chunks()
    }

    /// Bytes currently allocated in the GPU octree pool. Used by
    /// metrics. Compare against `octree_pool_capacity` for headroom.
    pub fn octree_allocated_bytes(&self) -> u64 {
        self.octree_pool.lock().expect("octree_pool mutex poisoned").allocated_bytes()
    }

    /// Total capacity of the GPU octree pool, in bytes.
    pub fn octree_pool_capacity(&self) -> u64 {
        self.octree_pool.lock().expect("octree_pool mutex poisoned").capacity()
    }

    /// Largest contiguous free span in the GPU octree pool, in bytes. When
    /// this falls far below `(capacity - allocated)`, the pool is fragmented
    /// and will OOM on big chunks even though the totals say there's room.
    pub fn octree_largest_free_span(&self) -> u64 {
        self.octree_pool.lock().expect("octree_pool mutex poisoned").largest_free_span()
    }

    /// Vertex/index pool stats. Each chunk mesh consumes both. Reported in
    /// `gpu_mem:` so we can see which pool is the binding constraint —
    /// today the octree pool is ~256 MiB while the vert/idx pools are
    /// multi-GiB, so the octree is overwhelmingly first to fill.
    pub fn mesh_vert_capacity(&self) -> u64 { self.mesh_pool.vert_buffer_size() }
    pub fn mesh_idx_capacity(&self) -> u64 { self.mesh_pool.idx_buffer_size() }
    pub fn mesh_vert_allocated(&self) -> u64 { self.mesh_pool.vert_allocated_bytes() }
    pub fn mesh_idx_allocated(&self) -> u64 { self.mesh_pool.idx_allocated_bytes() }
    pub fn mesh_vert_largest_free(&self) -> u64 { self.mesh_pool.vert_largest_free_span() }
    pub fn mesh_idx_largest_free(&self) -> u64 { self.mesh_pool.idx_largest_free_span() }

    /// Capacity of the indirect draw-slot pool (runtime, honoring
    /// `UNENDLICH_MAX_DRAWS`). Used by the run-summary memory line.
    pub fn indirect_capacity(&self) -> u32 {
        self.indirect.capacity()
    }

    /// Count of uploaded meshes per LOD level (Near, Mid, Far, VeryFar). Near
    /// chunks are full-detail (the expensive ones); pulling `UNENDLICH_LOD_BANDS`
    /// inward shifts chunks to coarser LODs, cutting mesher memory pressure.
    /// Surfaced in the run-summary so the effect is visible in the dashboard.
    pub fn lod_chunk_counts(&self) -> [u32; crate::world::lod::LodLevel::COUNT] {
        let mut counts = [0u32; crate::world::lod::LodLevel::COUNT];
        for key in self.meshes.keys() {
            counts[key.lod.index()] += 1;
        }
        counts
    }

    /// v0.24.30-shared-octree: clone the shared pool handle for the
    /// `OctreeGpuMesher` to bind into its compute pipeline. The
    /// returned `Arc` keeps the pool alive as long as either the
    /// renderer or the mesher holds it. Drop both before destroying
    /// the Vulkan device.
    pub fn octree_pool_handle(
        &self,
    ) -> Arc<Mutex<crate::render::octree_buffer::OctreeBufferPool>> {
        Arc::clone(&self.octree_pool)
    }

    /// v0.24.26-octree-on-gpu: end-to-end verification of the GPU
    /// octree against the CPU `OctreeArray`. Constructs the probe
    /// pipeline on first call. Walks every chunk in `chunk_manager`
    /// that also has a GPU octree slot, samples `samples_per_chunk`
    /// random-ish positions in chunk-local space, runs the probe
    /// compute shader, compares each result against
    /// `OctreeArray::sample`.
    ///
    /// Returns a report; the binary asserts `mismatched == 0` and
    /// exits non-zero otherwise so this gates CI.
    ///
    /// Cost: one probe dispatch per (up to) `MAX_PROBE_QUERIES`
    /// queries. With `samples_per_chunk = 16` and ~900 chunks
    /// loaded, that's 14400 queries — one dispatch, finishes in
    /// well under 10 ms on M2/Honeykrisp.
    pub fn run_octree_verify(
        &mut self,
        ctx: &VulkanContext,
        chunk_manager: &crate::world::chunk_manager::ChunkManager,
        samples_per_chunk: u32,
    ) -> std::result::Result<OctreeVerifyReport, String> {
        use crate::render::octree_probe_pipeline::{OctreeProbePipeline, ProbeQuery, MAX_PROBE_QUERIES};

        // Lazy construction.
        if self.octree_probe.is_none() {
            // Probe pipeline binds the pool's buffer at construction.
            // The buffer handle itself doesn't change over the pool's
            // lifetime, so we can capture it under a short lock and
            // release before the unsafe Vulkan call.
            let pool_buffer = {
                let pool = self.octree_pool.lock().expect("octree_pool mutex poisoned");
                pool.buffer
            };
            let probe = unsafe {
                OctreeProbePipeline::new(
                    &ctx.device, &ctx.instance, self.octree_probe_physical_device,
                    pool_buffer,
                    self.octree_probe_queue,
                    self.octree_probe_queue_family,
                )
            }.map_err(|e| format!("octree probe init: {}", e))?;
            self.octree_probe = Some(probe);
        }
        let probe = self.octree_probe.as_mut().unwrap();

        let mut report = OctreeVerifyReport::default();

        // Build the work list: every chunk that has BOTH an octree on
        // the GPU AND an `OctreeArray` available CPU-side (the latter
        // is the truth for comparison).
        let chunks_on_gpu: Vec<_> = self.octree_pool
            .lock().expect("octree_pool mutex poisoned")
            .iter_slots().collect();
        if chunks_on_gpu.is_empty() {
            return Ok(report);
        }

        // Cap samples_per_chunk so we never exceed MAX_PROBE_QUERIES
        // in a single batch. If the workload is bigger than one batch,
        // we'd chunk it; for now this returns early with a clamped value.
        let chunks_per_batch = (MAX_PROBE_QUERIES / samples_per_chunk.max(1))
            .min(chunks_on_gpu.len() as u32) as usize;
        if chunks_per_batch == 0 {
            return Err(format!(
                "samples_per_chunk={} exceeds MAX_PROBE_QUERIES={}",
                samples_per_chunk, MAX_PROBE_QUERIES));
        }

        // Deterministic sample positions per chunk. Not random — a
        // fixed quasi-random pattern that covers the chunk volume.
        // 16 samples land at corners + middle + a few interior cells.
        let sample_positions: Vec<[u32; 3]> = generate_probe_sample_positions(samples_per_chunk);

        // Iterate in batches.
        for batch in chunks_on_gpu.chunks(chunks_per_batch) {
            let mut queries: Vec<ProbeQuery> = Vec::with_capacity(batch.len() * sample_positions.len());
            let mut expected: Vec<(ChunkCoord, [u32; 3], u32)> = Vec::with_capacity(queries.capacity());
            for &(coord, slot) in batch {
                let cpu_octree = match chunk_manager.octree(coord) {
                    Some(o) => o,
                    None => { report.chunks_skipped += 1; continue; }
                };
                report.chunks_examined += 1;
                let base = slot.base_node_index();
                for pos in &sample_positions {
                    queries.push(ProbeQuery {
                        base_node: base, x: pos[0], y: pos[1], z: pos[2],
                    });
                    let cpu_val = cpu_octree.sample(pos[0] as i32, pos[1] as i32, pos[2] as i32) as u8 as u32;
                    expected.push((coord, *pos, cpu_val));
                }
            }
            if queries.is_empty() { continue; }

            let results = unsafe { probe.run_probe(&queries) }
                .map_err(|e| format!("octree probe dispatch: {}", e))?;
            debug_assert_eq!(results.len(), queries.len());

            for (i, gpu_val) in results.iter().copied().enumerate() {
                report.queries += 1;
                let (coord, pos, cpu_val) = expected[i];
                if gpu_val == cpu_val {
                    report.matched += 1;
                } else {
                    report.mismatched += 1;
                    if report.first_mismatch.is_none() {
                        report.first_mismatch = Some((coord, pos, gpu_val, cpu_val));
                    }
                }
            }
        }
        Ok(report)
    }

    pub fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            let _ = ctx.device.device_wait_idle();

            // In-game egui overlay: free its Vulkan objects (pipeline, buffers,
            // textures, sampler, descriptor pool) while the device is idle.
            #[cfg(feature = "ui-overlay")]
            self.egui_overlay.destroy(ctx);

            // Per-mesh GPU resource destruction is no longer needed —
            // the mesh pool owns the underlying buffers, destroyed below.
            // We just clear the metadata maps.
            self.meshes.clear();
            self.pending_destroy.clear();

            self.mesh_pool.destroy(&ctx.device);

            // v0.24.26-octree-on-gpu: probe pipeline (if it was
            // constructed via run_octree_verify) holds descriptors
            // bound to the octree buffer. Destroy it BEFORE the pool
            // so the buffer's lifetime is still valid when its
            // descriptors get torn down.
            if let Some(probe) = self.octree_probe.as_mut() {
                probe.destroy();
            }
            self.octree_probe = None;
            // v0.24.30-shared-octree: the pool is wrapped in
            // `Arc<Mutex>` so the mesher can share it. In practice
            // the mesher Arc is still alive at this point (drop
            // order in `unendlich_world.rs` calls `renderer.destroy`
            // before `drop(mesh_pool)`). That's fine: the mesher's
            // Drop impl doesn't touch the octree buffer (it only
            // destroys ITS OWN buffers: chunk_metas, vertex, index,
            // counter). The cached `octree_pool_buffer` in the
            // mesher becomes a dangling handle but is never used
            // after this point. We log a count just for diagnostics.
            let extra = Arc::strong_count(&self.octree_pool) - 1;
            if extra > 0 {
                log::info!(
                    "octree_pool has {} extra Arc reference(s) at destroy time \
                     (mesher hasn't dropped yet — expected). Destroying the \
                     buffer; any later access will be UB but should not occur \
                     given the bin's drop sequence.",
                    extra,
                );
            }
            self.octree_pool
                .lock().expect("octree_pool mutex poisoned during destroy")
                .destroy(&ctx.device);

            // r306/r307: neural G-buffer + feature pass (gated). Independent
            // resources; free here. Feature pass first (it referenced the G-buffer
            // images only transiently via descriptor writes, so order is not strict).
            if let Some(mut nf) = self.neural_feat.take() {
                nf.destroy(&ctx.device);
            }
            if let Some(mut gb) = self.gbuffer.take() {
                gb.destroy(&ctx.device);
            }
            // r308: stop the background trainer thread (CPU-only; no device resources).
            if let Some(mut t) = self.neural_trainer.take() {
                t.stop();
            }
            self.indirect.destroy();
            // v0.24.0b: destroy view-compute pipeline before view_params
            // since its descriptor set holds a binding pointing at
            // view_params.buffer.
            self.view_compute_pipeline.destroy();
            // v0.24.2b: cascade-fit pipeline has the same constraint.
            self.cascade_fit_pipeline.destroy();
            // v0.24.0a: destroy ViewParams AFTER indirect — indirect's
            // descriptor set holds a binding pointing at view_params.buffer.
            self.view_params.destroy();

            for f in &self.frames {
                ctx.device.unmap_memory(f.ubo_memory);
                ctx.device.destroy_buffer(f.ubo_buffer, None);
                ctx.device.free_memory(f.ubo_memory, None);
                ctx.device.destroy_semaphore(f.image_available, None);
                ctx.device.destroy_semaphore(f.render_finished, None);
                ctx.device.destroy_fence(f.in_flight, None);
            }
            self.frames.clear();

            ctx.device.destroy_descriptor_pool(self.descriptor_pool, None);
            ctx.device.destroy_command_pool(self.command_pool, None);
            self.pipeline.destroy(&ctx.device);
            self.entity_pipeline.destroy(&ctx.device);
            self.shadow_pipeline.destroy(&ctx.device);
            self.shadow.destroy(&ctx.device);
            self.froxel_fill_pipeline.destroy(&ctx.device);
            self.froxel.destroy(&ctx.device);
            self.depth.destroy(&ctx.device);
            self.swapchain.destroy(ctx);
            ctx.device.destroy_render_pass(self.render_pass, None);
        }
    }
}

// =============================================================================
// Frustum culling
// =============================================================================

/// 6-plane view frustum extracted from a view-projection matrix.
///
/// Plane is stored in the form (a, b, c, d) where ax + by + cz + d >= 0
/// means "inside this plane's half-space." The full frustum is the
/// intersection of all 6 half-spaces.
///
/// Extraction follows Gribb & Hartmann's method: each plane's coefficients
/// are a row sum/diff of the view-projection matrix. Works for any
/// projection (RH or LH, any depth convention) because we extract from the
/// final clip-space-producing matrix.
/// headlesshorseman--T3: cache key for the per-frame visibility frustum. The
/// frustum is a pure function of the view inputs and the swapchain extent, so
/// when this key is unchanged the cached planes are reused verbatim (a cache
/// hit is bit-identical to recomputing). `f32` `PartialEq` is fine here: real
/// views are never NaN, and a NaN would simply force a (harmless) rebuild.
#[derive(Clone, Copy, PartialEq)]
struct FrustumKey {
    pos: [f32; 3],
    yaw: f32,
    pitch: f32,
    fov: f32,
    near: f32,
    far: f32,
    extent: [u32; 2],
}

#[derive(Clone, Copy)]
struct Frustum {
    /// (a, b, c, d) per plane: left, right, bottom, top, near, far.
    planes: [[f32; 4]; 6],
}

impl Frustum {
    fn from_view_projection(m: &glm::Mat4) -> Self {
        // glm::Mat4 is column-major. m[(row, col)] retrieves an element. We
        // build the 6 planes by combining rows of the matrix.
        //
        // Notation: m[r, c] means element at (row r, column c).
        // Row 3 + Row 0 -> left plane
        // Row 3 - Row 0 -> right plane
        // Row 3 + Row 1 -> bottom plane
        // Row 3 - Row 1 -> top plane
        // Row 3 + Row 2 -> near plane (for depth in [-1,1]) or Row 2 alone
        //                  for depth in [0,1]. We use [0,1].
        // Row 3 - Row 2 -> far plane
        let r = |row: usize, col: usize| m[(row, col)];

        let mut planes = [[0.0f32; 4]; 6];

        // Left
        planes[0] = [
            r(3,0) + r(0,0),
            r(3,1) + r(0,1),
            r(3,2) + r(0,2),
            r(3,3) + r(0,3),
        ];
        // Right
        planes[1] = [
            r(3,0) - r(0,0),
            r(3,1) - r(0,1),
            r(3,2) - r(0,2),
            r(3,3) - r(0,3),
        ];
        // Bottom
        planes[2] = [
            r(3,0) + r(1,0),
            r(3,1) + r(1,1),
            r(3,2) + r(1,2),
            r(3,3) + r(1,3),
        ];
        // Top
        planes[3] = [
            r(3,0) - r(1,0),
            r(3,1) - r(1,1),
            r(3,2) - r(1,2),
            r(3,3) - r(1,3),
        ];
        // Near (depth-zero-to-one form: just Row 2)
        planes[4] = [r(2,0), r(2,1), r(2,2), r(2,3)];
        // Far
        planes[5] = [
            r(3,0) - r(2,0),
            r(3,1) - r(2,1),
            r(3,2) - r(2,2),
            r(3,3) - r(2,3),
        ];

        // Normalize each plane so distances are in world units. Not strictly
        // required for visibility-only tests, but avoids surprises.
        for p in planes.iter_mut() {
            let len = (p[0]*p[0] + p[1]*p[1] + p[2]*p[2]).sqrt();
            if len > 1e-6 {
                p[0] /= len; p[1] /= len; p[2] /= len; p[3] /= len;
            }
        }

        Self { planes }
    }

    /// Borrow the 6 frustum planes as `[vec4; 6]`.
    ///
    /// Currently unused in v0.24.0b: the GPU `view_compute.comp` derives
    /// planes directly into the SSBO from the view+projection matrices,
    /// so no caller needs to read them out of a CPU `Frustum` object
    /// any longer. The method is kept so the existing `Frustum` type
    /// remains a complete CPU-side reference for tests, debug overlays,
    /// and the v0.19 non-indirect fallback path.
    ///
    /// Plane convention: `(a, b, c, d)` with `ax + by + cz + d >= 0`
    /// for points inside. Order: left, right, bottom, top, near, far.
    #[allow(dead_code)]
    fn planes(&self) -> &[[f32; 4]; 6] {
        &self.planes
    }

    /// Test whether the AABB defined by [min, max] is potentially visible.
    /// Returns true if any part of the AABB is inside the frustum.
    /// Uses the "n-vertex" / "p-vertex" trick: for each plane, find the
    /// AABB corner farthest along the plane normal; if even that's outside,
    /// the box can't intersect.
    fn aabb_visible(&self, min: &glm::Vec3, max: &glm::Vec3) -> bool {
        for p in &self.planes {
            // Pick the corner that maximizes (a*x + b*y + c*z) — the
            // "positive" corner relative to this plane.
            let px = if p[0] >= 0.0 { max.x } else { min.x };
            let py = if p[1] >= 0.0 { max.y } else { min.y };
            let pz = if p[2] >= 0.0 { max.z } else { min.z };

            let dist = p[0] * px + p[1] * py + p[2] * pz + p[3];
            if dist < 0.0 {
                // Even the most-inside corner is outside this plane → all
                // 8 corners are outside → AABB is fully outside the frustum.
                return false;
            }
        }
        true
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn mat4_to_array(m: &glm::Mat4) -> [[f32; 4]; 4] {
    let s = m.as_slice();
    [
        [s[0], s[1], s[2], s[3]],
        [s[4], s[5], s[6], s[7]],
        [s[8], s[9], s[10], s[11]],
        [s[12], s[13], s[14], s[15]],
    ]
}

fn create_ubo(ctx: &VulkanContext) -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8)> {
    let size = std::mem::size_of::<CameraUbo>() as u64;
    let (buffer, memory) = create_host_visible_buffer(ctx, size, vk::BufferUsageFlags::UNIFORM_BUFFER)?;
    let ptr = unsafe {
        ctx.device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8
    };
    Ok((buffer, memory, ptr))
}

fn create_host_visible_buffer(
    ctx: &VulkanContext,
    size: u64,
    usage: vk::BufferUsageFlags,
) -> Result<(vk::Buffer, vk::DeviceMemory)> {
    let info = vk::BufferCreateInfo::builder()
        .size(size)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe { ctx.device.create_buffer(&info, None)? };

    let req = unsafe { ctx.device.get_buffer_memory_requirements(buffer) };
    let props = unsafe { ctx.instance.get_physical_device_memory_properties(ctx.physical_device) };

    let mut chosen = None;
    for i in 0..props.memory_type_count {
        let mt = props.memory_types[i as usize];
        if req.memory_type_bits & (1 << i) != 0
            && mt.property_flags.contains(vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)
        {
            chosen = Some(i);
            break;
        }
    }
    let mt = chosen.ok_or_else(|| WorldRenderError::Other("No host-visible memory type".into()))?;

    let alloc = vk::MemoryAllocateInfo::builder()
        .allocation_size(req.size)
        .memory_type_index(mt);
    let memory = unsafe { ctx.device.allocate_memory(&alloc, None)? };
    unsafe { ctx.device.bind_buffer_memory(buffer, memory, 0)?; }

    Ok((buffer, memory))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_vp() -> glm::Mat4 {
        glm::identity()
    }

    #[test]
    fn frustum_extracts_six_planes() {
        let f = Frustum::from_view_projection(&identity_vp());
        // Identity matrix gives planes at x=±1, y=±1, z=0 to 1 (approximately).
        // Check that we have 6 planes — i.e., didn't crash and they're normalized.
        for p in &f.planes {
            let len_sq = p[0]*p[0] + p[1]*p[1] + p[2]*p[2];
            // Plane normal should be unit length OR zero (degenerate).
            assert!((len_sq - 1.0).abs() < 1e-3 || len_sq < 1e-3,
                    "plane normal not normalized: {:?}", p);
        }
    }

    #[test]
    fn aabb_at_origin_visible_in_default_frustum() {
        // Build a real-ish view-projection matrix.
        let cam = FlyCamera::new(glm::vec3(0.0, 0.0, -10.0));
        let mut cam = cam;
        cam.yaw = 0.0;
        let vp = cam.view_projection(16.0 / 9.0);
        // Camera at z=-10 looking +X. AABB at origin has x in [0, 16].
        // From camera at z=-10 looking down +X, an AABB at (0..16, 0..256, 0..16)
        // should be visible.
        let f = Frustum::from_view_projection(&vp);
        let visible = f.aabb_visible(
            &glm::vec3(0.0, 0.0, 0.0),
            &glm::vec3(16.0, 256.0, 16.0),
        );
        assert!(visible, "AABB at origin should be visible from camera at z=-10");
    }

    #[test]
    fn aabb_far_behind_camera_culled() {
        let cam = FlyCamera::new(glm::vec3(0.0, 64.0, 0.0));
        let mut cam = cam;
        cam.yaw = 0.0; // looking +X
        let vp = cam.view_projection(16.0 / 9.0);
        let f = Frustum::from_view_projection(&vp);
        // AABB far in -X (behind us) should be culled.
        let visible = f.aabb_visible(
            &glm::vec3(-1000.0, 0.0, -8.0),
            &glm::vec3(-900.0, 256.0,  8.0),
        );
        assert!(!visible, "AABB far behind camera should be culled");
    }

    // =========================================================================
    // v0.23.2: LOD double-draw bug fix — stale_lod_key predicate tests
    // =========================================================================

    use crate::world::lod::{LodLevel, MeshSlotKey};

    /// Different-LOD entries for the SAME coord are stale (the bug
    /// from F3a: switching a chunk to a new LOD without evicting
    /// the old one led to chunks being drawn 2-3× simultaneously).
    #[test]
    fn stale_lod_key_evicts_other_lods_same_coord() {
        let coord = ChunkCoord::new(5, 1, -3);
        let existing = MeshSlotKey::at(coord, LodLevel::Far);
        assert!(stale_lod_key(existing, coord, LodLevel::Near),
            "Far entry at same coord must be stale when new upload is Near");

        let existing = MeshSlotKey::at(coord, LodLevel::Near);
        assert!(stale_lod_key(existing, coord, LodLevel::Far),
            "Near entry at same coord must be stale when new upload is Far");
    }

    /// Same-LOD same-coord entries are NOT flagged stale by this
    /// predicate — they get replaced in place by the HashMap insert
    /// in `upload_mesh`, which routes the old slot to
    /// `pending_destroy` via the `insert` return value.
    #[test]
    fn stale_lod_key_keeps_same_lod_same_coord() {
        let coord = ChunkCoord::new(0, 0, 0);
        for lod in LodLevel::iter() {
            let existing = MeshSlotKey::at(coord, lod);
            assert!(!stale_lod_key(existing, coord, lod),
                "{:?} entry at same coord must NOT be stale when new upload is also {:?}",
                lod, lod);
        }
    }

    /// Different-coord entries are NEVER stale, regardless of LOD.
    /// This is critical: we mustn't evict OTHER chunks' meshes when
    /// uploading a new mesh for THIS chunk. A bug here would erase
    /// the world.
    #[test]
    fn stale_lod_key_keeps_other_coords() {
        let upload_coord = ChunkCoord::new(0, 0, 0);
        let other_coord  = ChunkCoord::new(1, 0, 0);
        for new_lod in LodLevel::iter() {
            for existing_lod in LodLevel::iter() {
                let existing = MeshSlotKey::at(other_coord, existing_lod);
                assert!(!stale_lod_key(existing, upload_coord, new_lod),
                    "entry at {:?}/{:?} must NOT be stale when uploading {:?}/{:?}",
                    other_coord, existing_lod, upload_coord, new_lod);
            }
        }
    }

    /// Combined check: a HashMap with multiple LODs at one coord
    /// plus other chunks should yield exactly the other-LOD-same-
    /// coord keys when we sweep with the predicate. This pins the
    /// integration of predicate + filter() that lives in
    /// `upload_prebuilt_mesh_at_lod`.
    #[test]
    fn stale_lod_key_filter_yields_correct_set() {
        let upload_coord = ChunkCoord::new(0, 0, 0);
        let upload_lod   = LodLevel::Near;

        let mut keys: Vec<MeshSlotKey> = Vec::new();
        // Three LODs at the upload coord — Mid and Far should be
        // flagged stale (Near is the new one, so kept by predicate).
        keys.push(MeshSlotKey::at(upload_coord, LodLevel::Near));
        keys.push(MeshSlotKey::at(upload_coord, LodLevel::Mid));
        keys.push(MeshSlotKey::at(upload_coord, LodLevel::Far));
        // Other chunks at various LODs (must NOT be evicted).
        keys.push(MeshSlotKey::at(ChunkCoord::new(1, 0, 0),  LodLevel::Near));
        keys.push(MeshSlotKey::at(ChunkCoord::new(0, 1, 0),  LodLevel::Far));
        keys.push(MeshSlotKey::at(ChunkCoord::new(-1, 0, 0), LodLevel::Mid));

        let stale: Vec<MeshSlotKey> = keys.iter()
            .filter(|k| stale_lod_key(**k, upload_coord, upload_lod))
            .copied()
            .collect();

        assert_eq!(stale.len(), 2, "expected 2 stale keys, got {:?}", stale);
        assert!(stale.contains(&MeshSlotKey::at(upload_coord, LodLevel::Mid)));
        assert!(stale.contains(&MeshSlotKey::at(upload_coord, LodLevel::Far)));
        // Sanity: the new key is NOT in the stale set.
        assert!(!stale.contains(&MeshSlotKey::at(upload_coord, LodLevel::Near)));
    }

    // ===== Push-constant byte layout (v0.24.2c.2 — defensive) =====
    //
    // The shadow vertex shader's `pc.cascade_view_proj` is a `mat4`
    // push constant. GLSL `mat4` is column-major (`m[col][row]` access)
    // with 16-byte column stride. The Vulkan host writes 64 bytes that
    // the shader reads as a single mat4. Layout MUST be column-major
    // for the right transform to happen.
    //
    // Failure mode this catches: if `mat4_to_array(&m)` ever drifts to
    // row-major (e.g. someone replaces nalgebra with a row-major lib
    // and copies the slice naively), shadows would silently transform
    // through the transpose. Every chunk position renders to a wrong
    // texel. Hard to spot in static review but cheap to catch here.

    /// Construct a glm::Mat4 with each cell holding its (row*4+col) index
    /// (so `m[(0,0)]=0, m[(0,1)]=1, ..., m[(3,3)]=15`), then verify the
    /// byte layout produced by `mat4_to_array` is column-major:
    /// bytes 0..16 contain column 0 (= rows 0..3 of column 0 =
    /// m[(0,0)], m[(1,0)], m[(2,0)], m[(3,0)] = 0, 4, 8, 12).
    #[test]
    fn mat4_to_array_is_column_major() {
        let mut m = glm::Mat4::zeros();
        for row in 0..4 {
            for col in 0..4 {
                m[(row, col)] = (row * 4 + col) as f32;
            }
        }
        let arr = mat4_to_array(&m);

        // arr[col] should be column `col` of the matrix.
        // Column 0: m[(0,0)], m[(1,0)], m[(2,0)], m[(3,0)] = 0, 4, 8, 12.
        assert_eq!(arr[0], [0.0, 4.0, 8.0, 12.0], "column 0 wrong");
        assert_eq!(arr[1], [1.0, 5.0, 9.0, 13.0], "column 1 wrong");
        assert_eq!(arr[2], [2.0, 6.0, 10.0, 14.0], "column 2 wrong");
        assert_eq!(arr[3], [3.0, 7.0, 11.0, 15.0], "column 3 wrong");
    }

    /// Verify the byte sequence the shadow VS push constant code feeds
    /// to `cmd_push_constants` is exactly what GLSL's std430 mat4
    /// reader expects: 64 bytes, column-major, ascending row within
    /// column, little-endian f32s.
    #[test]
    fn shadow_push_constant_bytes_are_glsl_compatible() {
        // Build a recognizable test matrix.
        let mut m = glm::Mat4::zeros();
        for row in 0..4 {
            for col in 0..4 {
                m[(row, col)] = (row * 4 + col) as f32 + 100.0;
            }
        }

        // Same conversion the renderer does inline per cascade.
        let arr = mat4_to_array(&m);
        let bytes_unsafe = unsafe {
            std::slice::from_raw_parts(
                arr.as_ptr() as *const u8,
                std::mem::size_of_val(&arr),
            )
        }.to_vec();
        assert_eq!(bytes_unsafe.len(), 64);

        // Hand-build the expected std430 column-major byte sequence.
        let mut expected = Vec::with_capacity(64);
        for col in 0..4 {
            for row in 0..4 {
                let v: f32 = (row * 4 + col) as f32 + 100.0;
                expected.extend_from_slice(&v.to_le_bytes());
            }
        }
        assert_eq!(
            bytes_unsafe, expected,
            "push-constant bytes diverge from GLSL std430 column-major",
        );
    }

    /// Sanity: glm::Mat4 stores columns sequentially in memory (the
    /// nalgebra contract). If this ever changes (e.g. nalgebra major
    /// version bump to row-major), this test fails first instead of
    /// shadows mysteriously breaking.
    #[test]
    fn glm_mat4_storage_is_column_major() {
        let mut m = glm::Mat4::zeros();
        for row in 0..4 {
            for col in 0..4 {
                m[(row, col)] = (row * 4 + col) as f32;
            }
        }
        let slice = m.as_slice();
        // Column 0 first: m[(0,0)]=0, m[(1,0)]=4, m[(2,0)]=8, m[(3,0)]=12.
        assert_eq!(slice[0], 0.0);
        assert_eq!(slice[1], 4.0);
        assert_eq!(slice[2], 8.0);
        assert_eq!(slice[3], 12.0);
        // Column 1: 1, 5, 9, 13.
        assert_eq!(slice[4], 1.0);
        assert_eq!(slice[5], 5.0);
        assert_eq!(slice[6], 9.0);
        assert_eq!(slice[7], 13.0);
    }

    // ─── v0.24.19: mesh cache LRU policy ─────────────────────────────

    fn key(x: i32, y: i32, z: i32) -> crate::world::lod::MeshSlotKey {
        crate::world::lod::MeshSlotKey::at(
            ChunkCoord::new(x, y, z),
            crate::world::lod::LodLevel::Near,
        )
    }

    #[test]
    fn cache_lru_key_empty_input() {
        assert_eq!(cache_lru_key(&[]), None);
    }

    #[test]
    fn cache_lru_key_single_entry() {
        let touches = vec![(key(1, 2, 3), 42)];
        assert_eq!(cache_lru_key(&touches), Some(key(1, 2, 3)));
    }

    #[test]
    fn cache_lru_key_picks_smallest_last_touched() {
        // (1,2,3) was touched at seq=10, (4,5,6) at seq=5, (7,8,9) at seq=20.
        // LRU is (4,5,6).
        let touches = vec![
            (key(1, 2, 3), 10),
            (key(4, 5, 6), 5),
            (key(7, 8, 9), 20),
        ];
        assert_eq!(cache_lru_key(&touches), Some(key(4, 5, 6)));
    }

    #[test]
    fn cache_lru_key_picks_smallest_when_first_is_largest() {
        // Order in the slice shouldn't affect which key wins; smallest
        // last_touched always wins.
        let touches = vec![
            (key(100, 0, 0), 999),
            (key(200, 0, 0), 100),
            (key(300, 0, 0), 1),
        ];
        assert_eq!(cache_lru_key(&touches), Some(key(300, 0, 0)));
    }
}
